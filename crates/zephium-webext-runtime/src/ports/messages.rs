use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use crate::context::{ContextKey, Sender};
use crate::errors::ApiError;
use crate::events::ListenerId;
use crate::limits::{
    message_limit, Charge, Clock, Limiter, Reservation, MAX_OUTSTANDING_CALLS_PER_CONTEXT,
};

use super::{no_receiver, MessageRoute, RELAY_OVERHEAD_BYTES};

const RESPONSE_TIMEOUT_NANOS: u64 = 5 * 60 * 1_000_000_000;
const LISTENER_METADATA_BYTES: u64 = 512;

/// A host-minted one-shot request identity, never reused by its service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageId(u64);

impl MessageId {
    /// Validates a nonzero binding correlation token. The service still checks
    /// its authenticated recipient before accepting any outcome.
    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    /// Numeric identity for binding correlation.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One matching listener from the authenticated event registry.
#[derive(Clone, Debug)]
pub struct MessageListener {
    /// Exact live context that owns this callback.
    pub sender: Sender,
    /// Callback identity within that context.
    pub listener: ListenerId,
}

/// Deliver to every listener even when an earlier listener already responded.
#[derive(Clone, Debug)]
pub struct OneShotDelivery {
    /// One-shot request identity.
    pub message: MessageId,
    /// Exact destination document.
    pub recipient: ContextKey,
    /// Callback to invoke.
    pub listener: ListenerId,
    /// Authenticated `MessageSender` facts.
    pub sender: Sender,
    /// Opaque JSON serialization from the binding.
    pub payload: Arc<[u8]>,
    /// `runtime.onMessage` or `runtime.onMessageExternal`.
    pub event: &'static str,
}

impl OneShotDelivery {
    /// Binding event arguments, including the exact callbacks selected by Rust.
    /// `sender` is the adapter's projection of this delivery's authenticated sender.
    pub fn arguments(&self, sender: serde_json::Value) -> Result<serde_json::Value, ApiError> {
        let text =
            std::str::from_utf8(&self.payload).map_err(|_| ApiError::malformed_arguments())?;
        Ok(serde_json::json!([
            text,
            sender,
            self.message.get().to_string(),
            [self.listener.get()]
        ]))
    }
}

/// Binding observations of a listener invocation or its asynchronous completion.
#[derive(Clone, Debug)]
pub enum ListenerOutcome {
    /// Returned without responding or returning literal true/a promise.
    NoResponse,
    /// Returned literal true and will use `sendResponse` asynchronously.
    AsyncCallback,
    /// Returned a promise whose resolution or rejection will arrive later.
    Promise,
    /// Synchronous `sendResponse`, asynchronous callback, or promise resolution.
    /// The binding normalizes an omitted/undefined response to JSON `null`.
    Response(Vec<u8>),
    /// A thrown error or rejected promise. The first outcome wins globally.
    Rejected(ApiError),
}

/// Exactly one final result for the sending context, retained until acknowledged.
#[derive(Clone, Debug)]
pub struct MessageSettlement {
    /// Request identity.
    pub message: MessageId,
    /// Exact sending context.
    pub recipient: ContextKey,
    /// `None` is no listener response (`undefined`), distinct from JSON `null`.
    pub result: Result<Option<Vec<u8>>, ApiError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListenerState {
    Invoking,
    Callback,
    Promise,
    Closed,
}

#[derive(Debug)]
struct Request {
    source: Sender,
    listeners: HashMap<(ContextKey, ListenerId), ListenerState>,
    deadline: u64,
    asynchronous: bool,
    reservations: Vec<Reservation>,
}

#[derive(Debug)]
struct Completed {
    source: ContextKey,
    reservations: Vec<Reservation>,
}

/// One-shot request/response channels shared by `runtime` and `tabs` messaging.
#[derive(Debug, Default)]
pub struct MessageService {
    next: u64,
    requests: BTreeMap<MessageId, Request>,
    completed: BTreeMap<MessageId, Completed>,
}

impl MessageService {
    /// Empty state. Keep the owner for the profile lifetime to avoid id reuse.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fans opaque bytes out to authorized matching listeners, excluding the
    /// sending context. Receiver lists come from the host's event registry.
    pub fn send(
        &mut self,
        source: &Sender,
        route: &MessageRoute,
        listeners: &[MessageListener],
        payload: &[u8],
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<(MessageId, Vec<OneShotDelivery>), ApiError> {
        if matches!(route, MessageRoute::Native { .. }) {
            return Err(ApiError::malformed_arguments());
        }
        if source.kind() == crate::context::ContextKind::ExternalPage
            && route != &MessageRoute::Runtime
        {
            return Err(ApiError::cannot_access_page());
        }
        if self.pending_for_context(&source.context_key())
            >= MAX_OUTSTANDING_CALLS_PER_CONTEXT as usize
        {
            return Err(ApiError::too_many_pending());
        }
        if payload.len() as u64 > message_limit(source.kind()) {
            return Err(ApiError::message_too_large());
        }
        let mut unique = HashSet::new();
        let listeners: Vec<_> = listeners
            .iter()
            .filter(|listener| {
                route.accepts(source, &listener.sender)
                    && unique.insert((listener.sender.context_key(), listener.listener))
            })
            .collect();
        if listeners.is_empty() {
            return Err(no_receiver());
        }
        if listeners.len() > super::super::events::MAX_LISTENERS {
            return Err(ApiError::too_many_pending());
        }
        let now = clock.now_nanos();
        let mut reservations = Vec::new();
        let mut callback_counts: HashMap<ContextKey, u64> = HashMap::new();
        for listener in &listeners {
            *callback_counts
                .entry(listener.sender.context_key())
                .or_default() += 1;
        }
        let sender_charge =
            (payload.len() as u64).max(listeners.len() as u64 * LISTENER_METADATA_BYTES);
        match limiter.admit(
            &source.context_key(),
            source.kind(),
            sender_charge,
            Charge::Message,
            now,
        ) {
            Ok(reservation) => reservations.push(reservation),
            Err(error) => return Err(error),
        }
        let mut contexts = HashSet::new();
        for listener in &listeners {
            if !contexts.insert(listener.sender.context_key()) {
                continue;
            }
            if payload.len() as u64 > message_limit(listener.sender.kind()) {
                release(reservations, limiter);
                return Err(ApiError::message_too_large());
            }
            let callback_count = callback_counts
                .get(&listener.sender.context_key())
                .copied()
                .unwrap_or(0);
            let receiver_charge =
                (payload.len() as u64).max(callback_count * LISTENER_METADATA_BYTES);
            match limiter.admit(
                &listener.sender.context_key(),
                listener.sender.kind(),
                receiver_charge,
                Charge::Message,
                now,
            ) {
                Ok(reservation) => reservations.push(reservation),
                Err(error) => {
                    release(reservations, limiter);
                    return Err(error);
                }
            }
        }
        let Some(next) = self.next.checked_add(1) else {
            release(reservations, limiter);
            return Err(ApiError::too_many_pending());
        };
        self.next = next;
        let id = MessageId(next);
        let payload: Arc<[u8]> = Arc::from(payload);
        let deliveries = listeners
            .iter()
            .map(|listener| OneShotDelivery {
                message: id,
                recipient: listener.sender.context_key(),
                listener: listener.listener,
                sender: source.clone(),
                payload: payload.clone(),
                event: route.event(source, false),
            })
            .collect();
        self.requests.insert(
            id,
            Request {
                source: source.clone(),
                listeners: listeners
                    .iter()
                    .map(|listener| {
                        (
                            (listener.sender.context_key(), listener.listener),
                            ListenerState::Invoking,
                        )
                    })
                    .collect(),
                deadline: now.saturating_add(RESPONSE_TIMEOUT_NANOS),
                asynchronous: false,
                reservations,
            },
        );
        Ok((id, deliveries))
    }

    /// Observes an exact listener. First response/rejection wins; every later
    /// result is ignored. Returning false/undefined closes only that listener.
    pub fn observe(
        &mut self,
        sender: &Sender,
        message: MessageId,
        listener: ListenerId,
        outcome: ListenerOutcome,
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<Option<MessageSettlement>, ApiError> {
        let Some(request) = self.requests.get_mut(&message) else {
            return Ok(None);
        };
        let Some(state) = request.listeners.get_mut(&(sender.context_key(), listener)) else {
            return Err(ApiError::context_invalidated());
        };
        if *state == ListenerState::Closed {
            return Ok(None);
        }
        if clock.now_nanos() >= request.deadline {
            let error = channel_closed(request.asynchronous);
            return Ok(self.finish(message, Err(error)));
        }
        match outcome {
            ListenerOutcome::NoResponse => {
                if *state != ListenerState::Invoking {
                    return Ok(None);
                }
                *state = ListenerState::Closed;
                if request
                    .listeners
                    .values()
                    .all(|state| *state == ListenerState::Closed)
                {
                    let result = if request.asynchronous {
                        Err(channel_closed(true))
                    } else {
                        Ok(None)
                    };
                    return Ok(self.finish(message, result));
                }
            }
            ListenerOutcome::AsyncCallback | ListenerOutcome::Promise => {
                if *state != ListenerState::Invoking {
                    return Ok(None);
                }
                *state = if matches!(outcome, ListenerOutcome::Promise) {
                    ListenerState::Promise
                } else {
                    ListenerState::Callback
                };
                request.asynchronous = true;
            }
            ListenerOutcome::Rejected(error) => {
                let error = if error.message().len() > crate::envelope::MAX_ERROR_MESSAGE_BYTES {
                    ApiError::message_too_large()
                } else {
                    error
                };
                return Ok(self.finish(message, Err(error)));
            }
            ListenerOutcome::Response(payload) => {
                if payload.len() as u64 > message_limit(sender.kind())
                    || payload.len() as u64 > message_limit(request.source.kind())
                {
                    return Ok(self.finish(message, Err(ApiError::message_too_large())));
                }
                let now = clock.now_nanos();
                let size = (payload.len() as u64).max(RELAY_OVERHEAD_BYTES);
                let responder_reservation = limiter.admit(
                    &sender.context_key(),
                    sender.kind(),
                    size,
                    Charge::Message,
                    now,
                );
                let first = match responder_reservation {
                    Ok(reservation) => reservation,
                    Err(error) => return Ok(self.finish(message, Err(error))),
                };
                match limiter.admit(
                    &request.source.context_key(),
                    request.source.kind(),
                    size,
                    Charge::Message,
                    now,
                ) {
                    Ok(second) => {
                        request.reservations.push(first);
                        request.reservations.push(second);
                    }
                    Err(error) => {
                        limiter.release(first);
                        return Ok(self.finish(message, Err(error)));
                    }
                }
                return Ok(self.finish(message, Ok(Some(payload))));
            }
        }
        Ok(None)
    }

    fn finish(
        &mut self,
        id: MessageId,
        result: Result<Option<Vec<u8>>, ApiError>,
    ) -> Option<MessageSettlement> {
        let request = self.requests.remove(&id)?;
        let recipient = request.source.context_key();
        self.completed.insert(
            id,
            Completed {
                source: recipient.clone(),
                reservations: request.reservations,
            },
        );
        Some(MessageSettlement {
            message: id,
            recipient,
            result,
        })
    }

    /// Receipt after the sender's reply is delivered. Duplicate or forged
    /// receipts do not release reservations belonging to another context.
    pub fn acknowledge(&mut self, sender: &Sender, id: MessageId, limiter: &mut Limiter) -> bool {
        if !self
            .completed
            .get(&id)
            .is_some_and(|completed| completed.source == sender.context_key())
        {
            return false;
        }
        if let Some(completed) = self.completed.remove(&id) {
            release(completed.reservations, limiter);
        }
        true
    }

    /// Context death discards its own calls and closes its receiving listeners.
    /// Remaining listeners can still win. A promised response lost to teardown
    /// settles with Chrome's asynchronous-channel-closed error.
    pub fn cancel_context(
        &mut self,
        context: &ContextKey,
        limiter: &mut Limiter,
    ) -> Vec<MessageSettlement> {
        let mut settlements = Vec::new();
        let ids: Vec<_> = self.requests.keys().copied().collect();
        for id in ids {
            let Some(request) = self.requests.get_mut(&id) else {
                continue;
            };
            if request.source.context_key() == *context {
                if let Some(request) = self.requests.remove(&id) {
                    release(request.reservations, limiter);
                }
                continue;
            }
            for ((key, _), state) in &mut request.listeners {
                if key == context {
                    *state = ListenerState::Closed;
                }
            }
            if request
                .listeners
                .values()
                .all(|state| *state == ListenerState::Closed)
            {
                let result = Err(channel_closed(request.asynchronous));
                if let Some(settlement) = self.finish(id, result) {
                    settlements.push(settlement);
                }
            }
        }
        let ids: Vec<_> = self
            .completed
            .iter()
            .filter(|(_, request)| request.source == *context)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some(request) = self.completed.remove(&id) {
                release(request.reservations, limiter);
            }
        }
        settlements
    }

    /// Enforces the per-event five-minute bound using the supplied clock.
    pub fn expire(&mut self, clock: &impl Clock) -> Vec<MessageSettlement> {
        let now = clock.now_nanos();
        let expired: Vec<_> = self
            .requests
            .iter()
            .filter(|(_, request)| now >= request.deadline)
            .map(|(id, request)| (*id, request.asynchronous))
            .collect();
        expired
            .into_iter()
            .filter_map(|(id, asynchronous)| self.finish(id, Err(channel_closed(asynchronous))))
            .collect()
    }

    /// Next one-shot deadline for the adapter's timer.
    pub fn next_deadline(&self) -> Option<u64> {
        self.requests.values().map(|request| request.deadline).min()
    }

    /// Calls still awaiting a response or reply acknowledgement from this sender.
    pub fn pending_for_context(&self, context: &ContextKey) -> usize {
        self.requests
            .values()
            .filter(|request| request.source.context_key() == *context)
            .count()
            + self
                .completed
                .values()
                .filter(|request| request.source == *context)
                .count()
    }
}

fn release(reservations: Vec<Reservation>, limiter: &mut Limiter) {
    for reservation in reservations {
        limiter.release(reservation);
    }
}

fn channel_closed(asynchronous: bool) -> ApiError {
    if asynchronous {
        ApiError::other("A listener indicated an asynchronous response by returning true, but the message channel closed before a response was received")
    } else {
        ApiError::other("The message port closed before a response was received.")
    }
}
