//! Authenticated port handshakes, opaque relay and one-shot response channels.
//!
//! Adapters resolve Chrome tab/document ids, authorize cross-extension routes,
//! supply live listeners, and execute returned actions. Keep one service and one
//! shared [`Limiter`] per profile. Delivery acknowledgements release reservations.

mod api;
mod messages;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use zephium_core::ids::{ExtensionInstallId, ItemId};

use crate::context::{ContextKey, ContextKind, DocumentGeneration, ExtensionId, FrameId, Sender};
use crate::envelope::PortId;
use crate::errors::ApiError;
use crate::limits::{message_limit, Charge, Clock, Limiter, Reservation, MAX_PORTS_PER_INSTALL};

pub use api::{
    parse_request, register, MessageRequest, MessageTarget, MessagingHost, MessagingValidator,
};
pub use messages::{
    ListenerOutcome, MessageId, MessageListener, MessageService, MessageSettlement, OneShotDelivery,
};

/// Largest message received from a native host (Chrome's native protocol limit).
pub const MAX_NATIVE_MESSAGE_BYTES: u64 = 1 << 20;
/// Maximum buffered messages before a port handshake completes.
pub const MAX_BUFFERED_PORT_MESSAGES: usize = 256;
/// Minimum accounting charge for each retained relay message.
pub const RELAY_OVERHEAD_BYTES: u64 = 32;
/// Maximum unacknowledged relays sent by one context.
pub const MAX_PENDING_DELIVERIES_PER_CONTEXT: usize = 256;
/// Maximum connected or opening native hosts owned by one install.
pub const MAX_NATIVE_PORTS_PER_INSTALL: usize = 8;

/// Chrome's no-listening-receiver failure, used by `lastError` and promises.
pub fn no_receiver() -> ApiError {
    ApiError::other("Could not establish connection. Receiving end does not exist.")
}

/// A resolved host-owned route. Cross-extension routes require an adapter check
/// against the target extension's `externally_connectable` policy first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageRoute {
    /// Other extension pages in the calling install.
    Runtime,
    /// Content scripts in a tab and optionally one frame/document.
    Tab {
        /// Native logical tab identity.
        tab: ItemId,
        /// A particular frame, or every frame.
        frame: Option<FrameId>,
        /// A particular committed document, or the current one.
        document: Option<DocumentGeneration>,
    },
    /// A different install after the adapter authorized external messaging.
    External {
        /// Receiver installation.
        install: ExtensionInstallId,
        /// Receiver package id.
        extension_id: ExtensionId,
    },
    /// An adapter-authorized native host.
    Native {
        /// Validated Chrome native-host name.
        host: String,
    },
}

impl MessageRoute {
    /// Receiver event appropriate to the source and this route.
    pub fn event(&self, sender: &Sender, connect: bool) -> &'static str {
        let external =
            matches!(self, Self::External { .. }) || sender.kind() == ContextKind::ExternalPage;
        match (connect, external) {
            (true, false) => "runtime.onConnect",
            (true, true) => "runtime.onConnectExternal",
            (false, false) => "runtime.onMessage",
            (false, true) => "runtime.onMessageExternal",
        }
    }

    fn accepts(&self, source: &Sender, receiver: &Sender) -> bool {
        if source.profile() != receiver.profile()
            || source.incognito() != receiver.incognito()
            || source.context_key() == receiver.context_key()
        {
            return false;
        }
        if let Self::External {
            install,
            extension_id,
        } = self
        {
            return receiver.install() == *install
                && receiver.extension_id() == extension_id
                && receiver.kind().is_extension_page();
        }
        if source.install() != receiver.install() || source.generation() != receiver.generation() {
            return false;
        }
        match self {
            Self::Runtime => receiver.kind().is_extension_page(),
            Self::Tab {
                tab,
                frame,
                document,
            } => {
                receiver.kind() == ContextKind::ContentScript
                    && receiver.tab() == Some(*tab)
                    && frame.is_none_or(|frame| receiver.frame() == Some(frame))
                    && document.is_none_or(|document| receiver.document() == document)
            }
            Self::External { .. } | Self::Native { .. } => false,
        }
    }
}

/// Host-minted identity for one retained delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeliveryId(u64);

impl DeliveryId {
    /// Numeric value for adapter acknowledgement correlation.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// An effect for the adapter. Payload bytes are never interpreted by this module.
#[derive(Clone, Debug)]
pub enum PortAction {
    /// Deliver a new receiving port and its authenticated `MessageSender`.
    Connect {
        /// Shared channel identity.
        port: PortId,
        /// Receiving context.
        recipient: ContextKey,
        /// Authenticated initiating context.
        sender: Box<Sender>,
        /// Connection name.
        name: String,
        /// `runtime.onConnect` or `runtime.onConnectExternal`.
        event: &'static str,
    },
    /// Begin native-host discovery/spawn through the adapter's policy broker.
    ConnectNative {
        /// Shared channel identity.
        port: PortId,
        /// Native-host manifest name.
        host: String,
    },
    /// Relay one retained payload to an exact live document.
    Message {
        /// Retention acknowledgement identity.
        delivery: DeliveryId,
        /// Channel identity.
        port: PortId,
        /// Exact receiving context.
        recipient: ContextKey,
        /// Opaque JSON serialization from the binding.
        payload: Arc<[u8]>,
    },
    /// Write one retained payload to the connected native host.
    NativeMessage {
        /// Retention acknowledgement identity.
        delivery: DeliveryId,
        /// Native channel identity.
        port: PortId,
        /// Opaque JSON serialization.
        payload: Arc<[u8]>,
    },
    /// Fire `onDisconnect`; an error is exposed only during that callback.
    Disconnect {
        /// Channel identity.
        port: PortId,
        /// Exact surviving endpoint.
        recipient: ContextKey,
        /// `runtime.lastError`, when the channel failed.
        error: Option<ApiError>,
    },
    /// Close and reap the native channel through its adapter.
    DisconnectNative {
        /// Native channel identity.
        port: PortId,
    },
}

#[derive(Debug)]
struct Buffered {
    payload: Arc<[u8]>,
    reservation: Reservation,
}

#[derive(Debug)]
struct Port {
    source: Sender,
    route: MessageRoute,
    name: String,
    open: bool,
    receivers: HashMap<ContextKey, Sender>,
    buffered: VecDeque<Buffered>,
}

#[derive(Debug)]
struct PendingDelivery {
    port: PortId,
    source: Option<ContextKey>,
    recipient: Option<ContextKey>,
    reservations: Vec<Reservation>,
}

/// Profile-scoped port state, including opening handshakes and retained relays.
#[derive(Debug, Default)]
pub struct PortService {
    next_port: u64,
    next_delivery: u64,
    ports: BTreeMap<PortId, Port>,
    deliveries: BTreeMap<DeliveryId, PendingDelivery>,
    receipts: BTreeMap<DeliveryId, Vec<ContextKey>>,
}

impl PortService {
    /// Empty state; channel ids are never reused by this service.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves a channel before receiver discovery or background wakeup.
    /// Context and permission admission precedes this host-facing call.
    pub fn begin_connect(
        &mut self,
        source: &Sender,
        route: MessageRoute,
        name: &str,
    ) -> Result<(PortId, Vec<PortAction>), ApiError> {
        if name.len() > 1_024 {
            return Err(ApiError::message_too_large());
        }
        if let MessageRoute::Native { host } = &route {
            if !source.kind().is_runtime_owned() {
                return Err(ApiError::not_available_in_context("runtime.connectNative"));
            }
            if !valid_native_host(host) {
                return Err(ApiError::malformed_arguments());
            }
            if self
                .ports
                .values()
                .filter(|port| {
                    port.source.install() == source.install()
                        && matches!(port.route, MessageRoute::Native { .. })
                })
                .count()
                >= MAX_NATIVE_PORTS_PER_INSTALL
            {
                return Err(ApiError::too_many_pending());
            }
        }
        if self.endpoint_count(source.install()) >= MAX_PORTS_PER_INSTALL as usize {
            return Err(ApiError::too_many_pending());
        }
        if let MessageRoute::External { install, .. } = &route {
            if self.port_count(*install) >= MAX_PORTS_PER_INSTALL as usize {
                return Err(ApiError::too_many_pending());
            }
        }
        if source.kind() == ContextKind::ExternalPage && route != MessageRoute::Runtime {
            return Err(ApiError::cannot_access_page());
        }
        self.next_port = self
            .next_port
            .checked_add(1)
            .ok_or_else(ApiError::too_many_pending)?;
        let id = PortId::new(self.next_port).ok_or_else(ApiError::too_many_pending)?;
        let actions = if let MessageRoute::Native { host } = &route {
            vec![PortAction::ConnectNative {
                port: id,
                host: host.clone(),
            }]
        } else {
            vec![]
        };
        self.ports.insert(
            id,
            Port {
                source: source.clone(),
                route,
                name: name.to_owned(),
                open: false,
                receivers: HashMap::new(),
                buffered: VecDeque::new(),
            },
        );
        Ok((id, actions))
    }

    /// Completes receiver discovery. Supply only contexts with matching live
    /// `onConnect` listeners. Unrelated, stale and duplicate contexts are ignored.
    pub fn complete_connect(
        &mut self,
        id: PortId,
        receivers: &[Sender],
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<Vec<PortAction>, ApiError> {
        let port = self.ports.get(&id).ok_or_else(disconnected)?;
        if port.open || matches!(port.route, MessageRoute::Native { .. }) {
            return Err(disconnected());
        }
        let mut admitted = HashMap::new();
        let mut additions: HashMap<ExtensionInstallId, usize> = HashMap::new();
        for receiver in receivers {
            if port.route.accepts(&port.source, receiver) {
                if admitted.contains_key(&receiver.context_key()) {
                    continue;
                }
                let additional = additions.entry(receiver.install()).or_default();
                if self.endpoint_count(receiver.install()) + *additional
                    >= MAX_PORTS_PER_INSTALL as usize
                {
                    return Ok(self.fail_connect(id, ApiError::too_many_pending(), limiter));
                }
                *additional += 1;
                admitted.insert(receiver.context_key(), receiver.clone());
            }
        }
        let port = self.ports.get_mut(&id).ok_or_else(disconnected)?;
        port.receivers = admitted;
        if port.receivers.is_empty() {
            return Ok(self.fail_connect(id, no_receiver(), limiter));
        }
        port.open = true;
        let actions = port
            .receivers
            .keys()
            .map(|recipient| PortAction::Connect {
                port: id,
                recipient: recipient.clone(),
                sender: Box::new(port.source.clone()),
                name: port.name.clone(),
                event: port.route.event(&port.source, true),
            })
            .collect();
        self.flush_buffered(id, actions, limiter, clock)
    }

    /// Observes successful native-host connection, then flushes opening messages.
    pub fn native_connected(
        &mut self,
        id: PortId,
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<Vec<PortAction>, ApiError> {
        let port = self.ports.get_mut(&id).ok_or_else(disconnected)?;
        if port.open || !matches!(port.route, MessageRoute::Native { .. }) {
            return Err(disconnected());
        }
        port.open = true;
        self.flush_buffered(id, vec![], limiter, clock)
    }

    fn flush_buffered(
        &mut self,
        id: PortId,
        mut actions: Vec<PortAction>,
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<Vec<PortAction>, ApiError> {
        let buffered = self
            .ports
            .get_mut(&id)
            .map(|p| std::mem::take(&mut p.buffered))
            .unwrap_or_default();
        let mut buffered = buffered.into_iter();
        while let Some(message) = buffered.next() {
            let Some(source) = self.ports.get(&id).map(|p| p.source.clone()) else {
                limiter.release(message.reservation);
                continue;
            };
            match self.relay_reserved(
                &source,
                id,
                message.payload,
                message.reservation,
                limiter,
                clock.now_nanos(),
            ) {
                Ok(deliveries) => actions.extend(deliveries),
                Err(error) => {
                    for message in buffered {
                        limiter.release(message.reservation);
                    }
                    return Ok(self.fail_connect(id, error, limiter));
                }
            }
        }
        Ok(actions)
    }

    /// Connect failure closes the initiating port with Chrome `lastError` and
    /// returns every buffered or in-flight reservation.
    pub fn fail_connect(
        &mut self,
        id: PortId,
        error: ApiError,
        limiter: &mut Limiter,
    ) -> Vec<PortAction> {
        self.close_all(id, Some(error), limiter, true)
    }

    /// Charges and relays opaque bytes. Messages sent during the opening
    /// handshake are buffered and delivered in order after `onConnect`.
    pub fn post_message(
        &mut self,
        source: &Sender,
        id: PortId,
        payload: &[u8],
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<Vec<PortAction>, ApiError> {
        self.check_endpoint(source, id)?;
        if payload.len() as u64 > message_limit(source.kind()) {
            return Err(ApiError::message_too_large());
        }
        let reservation = limiter.admit(
            &source.context_key(),
            source.kind(),
            (payload.len() as u64).max(RELAY_OVERHEAD_BYTES),
            Charge::Message,
            clock.now_nanos(),
        )?;
        self.post_admitted(source, id, payload, reservation, limiter, clock)
    }

    /// Relays a `Dispatcher::receive` port-message reservation without charging
    /// its sender twice. The reservation is consumed and released on any error.
    pub fn post_admitted(
        &mut self,
        source: &Sender,
        id: PortId,
        payload: &[u8],
        reservation: Reservation,
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<Vec<PortAction>, ApiError> {
        if reservation.context() != &source.context_key() {
            limiter.release(reservation);
            return Err(ApiError::context_invalidated());
        }
        if let Err(error) = self.check_endpoint(source, id) {
            limiter.release(reservation);
            return Err(error);
        }
        if payload.len() as u64 > message_limit(source.kind())
            || reservation.bytes() < payload.len() as u64
        {
            limiter.release(reservation);
            return Err(ApiError::message_too_large());
        }
        if self.pending_for_context(&source.context_key()) >= MAX_PENDING_DELIVERIES_PER_CONTEXT {
            limiter.release(reservation);
            return Err(ApiError::too_many_pending());
        }
        let Some(port) = self.ports.get_mut(&id) else {
            limiter.release(reservation);
            return Err(disconnected());
        };
        if !port.open {
            if port.buffered.len() >= MAX_BUFFERED_PORT_MESSAGES {
                limiter.release(reservation);
                return Err(ApiError::too_many_pending());
            }
            port.buffered.push_back(Buffered {
                payload: Arc::from(payload),
                reservation,
            });
            return Ok(vec![]);
        }
        self.relay_reserved(
            source,
            id,
            Arc::from(payload),
            reservation,
            limiter,
            clock.now_nanos(),
        )
    }

    fn relay_reserved(
        &mut self,
        source: &Sender,
        id: PortId,
        payload: Arc<[u8]>,
        reservation: Reservation,
        limiter: &mut Limiter,
        now: u64,
    ) -> Result<Vec<PortAction>, ApiError> {
        let Some(port) = self.ports.get(&id) else {
            limiter.release(reservation);
            return Err(disconnected());
        };
        let from_source = source.context_key() == port.source.context_key();
        let recipients: Vec<Option<Sender>> = if from_source {
            if matches!(port.route, MessageRoute::Native { .. }) {
                vec![None]
            } else {
                port.receivers.values().cloned().map(Some).collect()
            }
        } else {
            vec![Some(port.source.clone())]
        };
        if recipients.is_empty() {
            limiter.release(reservation);
            return Err(disconnected());
        }
        let mut reservations = vec![reservation];
        for recipient in recipients.iter().flatten() {
            if self.pending_for_context(&recipient.context_key())
                >= MAX_PENDING_DELIVERIES_PER_CONTEXT
            {
                for reservation in reservations {
                    limiter.release(reservation);
                }
                return Err(ApiError::too_many_pending());
            }
            if payload.len() as u64 > message_limit(recipient.kind()) {
                for reservation in reservations {
                    limiter.release(reservation);
                }
                return Err(ApiError::message_too_large());
            }
            match limiter.admit(
                &recipient.context_key(),
                recipient.kind(),
                (payload.len() as u64).max(RELAY_OVERHEAD_BYTES),
                Charge::Message,
                now,
            ) {
                Ok(reservation) => reservations.push(reservation),
                Err(error) => {
                    for reservation in reservations {
                        limiter.release(reservation);
                    }
                    return Err(error);
                }
            }
        }
        let delivery = match self.mint_delivery() {
            Ok(id) => id,
            Err(error) => {
                for reservation in reservations {
                    limiter.release(reservation);
                }
                return Err(error);
            }
        };
        let mut actions = Vec::new();
        let mut contexts = Vec::new();
        for recipient in recipients {
            if let Some(recipient) = recipient {
                contexts.push(recipient.context_key());
                actions.push(PortAction::Message {
                    delivery,
                    port: id,
                    recipient: recipient.context_key(),
                    payload: payload.clone(),
                });
            } else {
                actions.push(PortAction::NativeMessage {
                    delivery,
                    port: id,
                    payload: payload.clone(),
                });
            }
        }
        // One receipt tracks the whole fan-out. Per-recipient receipts below
        // share the original source charge until the last destination settles.
        self.deliveries.insert(
            delivery,
            PendingDelivery {
                port: id,
                source: Some(source.context_key()),
                recipient: None,
                reservations,
            },
        );
        self.receipts.insert(delivery, contexts);
        Ok(actions)
    }

    fn mint_delivery(&mut self) -> Result<DeliveryId, ApiError> {
        self.next_delivery = self
            .next_delivery
            .checked_add(1)
            .ok_or_else(ApiError::too_many_pending)?;
        Ok(DeliveryId(self.next_delivery))
    }

    fn check_endpoint(&self, sender: &Sender, id: PortId) -> Result<(), ApiError> {
        let port = self.ports.get(&id).ok_or_else(disconnected)?;
        let key = sender.context_key();
        if key != port.source.context_key() && !port.receivers.contains_key(&key) {
            return Err(disconnected());
        }
        Ok(())
    }

    /// Native-host bytes become a relay to the exact initiating document.
    pub fn native_message(
        &mut self,
        id: PortId,
        payload: &[u8],
        limiter: &mut Limiter,
        clock: &impl Clock,
    ) -> Result<PortAction, ApiError> {
        let port = self.ports.get(&id).ok_or_else(disconnected)?;
        if !port.open || !matches!(port.route, MessageRoute::Native { .. }) {
            return Err(disconnected());
        }
        if payload.len() as u64 > MAX_NATIVE_MESSAGE_BYTES {
            return Err(ApiError::message_too_large());
        }
        let source = port.source.clone();
        if self.pending_for_context(&source.context_key()) >= MAX_PENDING_DELIVERIES_PER_CONTEXT {
            return Err(ApiError::too_many_pending());
        }
        let reservation = limiter.admit(
            &source.context_key(),
            source.kind(),
            (payload.len() as u64).max(RELAY_OVERHEAD_BYTES),
            Charge::Message,
            clock.now_nanos(),
        )?;
        let delivery = match self.mint_delivery() {
            Ok(id) => id,
            Err(error) => {
                limiter.release(reservation);
                return Err(error);
            }
        };
        self.deliveries.insert(
            delivery,
            PendingDelivery {
                port: id,
                source: None,
                recipient: Some(source.context_key()),
                reservations: vec![reservation],
            },
        );
        self.receipts.insert(delivery, vec![source.context_key()]);
        Ok(PortAction::Message {
            delivery,
            port: id,
            recipient: source.context_key(),
            payload: Arc::from(payload),
        })
    }

    /// A delivery is retained until every addressed document has accepted it.
    /// A forged or repeated receipt returns false and releases nothing.
    pub fn acknowledge(&mut self, sender: &Sender, id: DeliveryId, limiter: &mut Limiter) -> bool {
        let Some(receipts) = self.receipts.get_mut(&id) else {
            return false;
        };
        let Some(index) = receipts.iter().position(|key| *key == sender.context_key()) else {
            return false;
        };
        receipts.swap_remove(index);
        if receipts.is_empty() {
            self.release_delivery(id, limiter);
        }
        true
    }

    /// Receipt from the adapter after writing the complete native message.
    pub fn acknowledge_native(
        &mut self,
        port: PortId,
        id: DeliveryId,
        limiter: &mut Limiter,
    ) -> bool {
        if !self
            .deliveries
            .get(&id)
            .is_some_and(|delivery| delivery.port == port && delivery.recipient.is_none())
            || !self.receipts.get(&id).is_some_and(Vec::is_empty)
            || !self
                .ports
                .get(&port)
                .is_some_and(|port| matches!(port.route, MessageRoute::Native { .. }))
        {
            return false;
        }
        self.release_delivery(id, limiter);
        true
    }

    fn release_delivery(&mut self, id: DeliveryId, limiter: &mut Limiter) {
        self.receipts.remove(&id);
        if let Some(delivery) = self.deliveries.remove(&id) {
            for reservation in delivery.reservations {
                limiter.release(reservation);
            }
        }
    }

    /// Endpoint-initiated disconnect. Closing one receiver notifies only the
    /// initiating endpoint; its other receivers stay connected.
    pub fn disconnect(
        &mut self,
        sender: &Sender,
        id: PortId,
        limiter: &mut Limiter,
    ) -> Vec<PortAction> {
        let Some(port) = self.ports.get_mut(&id) else {
            return vec![];
        };
        let key = sender.context_key();
        if port.source.context_key() == key {
            return self.close_all(id, None, limiter, false);
        }
        if port.receivers.remove(&key).is_none() {
            return vec![];
        }
        let source = port.source.context_key();
        let empty = port.receivers.is_empty();
        self.release_context_deliveries(&key, Some(id), limiter);
        if empty {
            self.remove_port(id, limiter);
        }
        vec![PortAction::Disconnect {
            port: id,
            recipient: source,
            error: None,
        }]
    }

    /// Navigation, suspension and context death propagate terminal cleanup.
    pub fn cancel_context(
        &mut self,
        context: &ContextKey,
        limiter: &mut Limiter,
    ) -> Vec<PortAction> {
        let affected: Vec<_> = self
            .ports
            .iter()
            .filter(|(_, p)| {
                p.source.context_key() == *context || p.receivers.contains_key(context)
            })
            .map(|(id, _)| *id)
            .collect();
        let mut actions = Vec::new();
        for id in affected {
            let Some(port) = self.ports.get_mut(&id) else {
                continue;
            };
            if port.source.context_key() == *context {
                actions.extend(self.close_all(id, None, limiter, false));
            } else {
                port.receivers.remove(context);
                if port.receivers.is_empty() {
                    actions.push(PortAction::Disconnect {
                        port: id,
                        recipient: port.source.context_key(),
                        error: None,
                    });
                    self.remove_port(id, limiter);
                }
            }
        }
        self.release_context_deliveries(context, None, limiter);
        actions
    }

    /// Native process exit closes its initiating endpoint with optional error.
    pub fn native_disconnected(
        &mut self,
        id: PortId,
        error: Option<ApiError>,
        limiter: &mut Limiter,
    ) -> Vec<PortAction> {
        if !self
            .ports
            .get(&id)
            .is_some_and(|p| matches!(p.route, MessageRoute::Native { .. }))
        {
            return vec![];
        }
        self.close_all(id, error, limiter, true)
    }

    /// Uninstall/revocation closes every channel involving the install and
    /// releases all opening/in-flight relay budgets before adapter teardown.
    pub fn erase(&mut self, install: ExtensionInstallId, limiter: &mut Limiter) -> Vec<PortAction> {
        let ids: Vec<_> = self.ports.iter().filter(|(_, port)| port.source.install() == install || port.receivers.keys().any(|key| key.install() == install)
            || matches!(port.route, MessageRoute::External { install: target, .. } if target == install)).map(|(id, _)| *id).collect();
        let mut actions = Vec::new();
        for id in ids {
            actions.extend(self.close_all(
                id,
                Some(ApiError::context_invalidated()),
                limiter,
                true,
            ));
        }
        actions
    }

    /// Open and opening logical channels owned by an install.
    pub fn port_count(&self, install: ExtensionInstallId) -> usize {
        self.ports.values().filter(|p| p.source.install() == install || matches!(p.route, MessageRoute::External { install: receiver, .. } if receiver == install)).count()
    }

    /// Open channels involving an exact context, for background idle accounting.
    pub fn context_port_count(&self, context: &ContextKey) -> usize {
        self.ports
            .values()
            .filter(|p| p.source.context_key() == *context || p.receivers.contains_key(context))
            .count()
    }

    /// Initiating and receiving port objects held by an install. Both count
    /// against the shared install cap, including opening initiators.
    pub fn endpoint_count(&self, install: ExtensionInstallId) -> usize {
        self.ports
            .values()
            .map(|port| {
                usize::from(port.source.install() == install)
                    + port
                        .receivers
                        .values()
                        .filter(|receiver| receiver.install() == install)
                        .count()
            })
            .sum()
    }

    fn pending_for_context(&self, context: &ContextKey) -> usize {
        self.deliveries
            .iter()
            .filter(|(id, d)| {
                d.source.as_ref() == Some(context)
                    || d.recipient.as_ref() == Some(context)
                    || self
                        .receipts
                        .get(id)
                        .is_some_and(|receipts| receipts.contains(context))
            })
            .count()
            + self
                .ports
                .values()
                .filter(|p| p.source.context_key() == *context)
                .map(|p| p.buffered.len())
                .sum::<usize>()
    }

    fn release_context_deliveries(
        &mut self,
        context: &ContextKey,
        port: Option<PortId>,
        limiter: &mut Limiter,
    ) {
        let ids: Vec<_> = self
            .deliveries
            .iter()
            .filter(|(_, delivery)| port.is_none_or(|port| port == delivery.port))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if self.deliveries.get(&id).is_some_and(|delivery| {
                delivery.source.as_ref() == Some(context)
                    || delivery.recipient.as_ref() == Some(context)
            }) {
                self.release_delivery(id, limiter);
            } else if let Some(receipts) = self.receipts.get_mut(&id) {
                let before = receipts.len();
                receipts.retain(|key| key != context);
                if before != receipts.len() && receipts.is_empty() {
                    self.release_delivery(id, limiter);
                }
            }
        }
    }

    fn remove_port(&mut self, id: PortId, limiter: &mut Limiter) -> Option<Port> {
        let mut port = self.ports.remove(&id)?;
        for message in port.buffered.drain(..) {
            limiter.release(message.reservation);
        }
        let deliveries: Vec<_> = self
            .deliveries
            .iter()
            .filter(|(_, d)| d.port == id)
            .map(|(id, _)| *id)
            .collect();
        for id in deliveries {
            self.release_delivery(id, limiter);
        }
        Some(port)
    }

    fn close_all(
        &mut self,
        id: PortId,
        error: Option<ApiError>,
        limiter: &mut Limiter,
        notify_source: bool,
    ) -> Vec<PortAction> {
        let Some(port) = self.remove_port(id, limiter) else {
            return vec![];
        };
        let mut actions: Vec<_> = port
            .receivers
            .keys()
            .map(|recipient| PortAction::Disconnect {
                port: id,
                recipient: recipient.clone(),
                error: error.clone(),
            })
            .collect();
        if notify_source {
            actions.push(PortAction::Disconnect {
                port: id,
                recipient: port.source.context_key(),
                error,
            });
        }
        if matches!(port.route, MessageRoute::Native { .. }) {
            actions.push(PortAction::DisconnectNative { port: id });
        }
        actions
    }
}

fn valid_native_host(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.'))
}

fn disconnected() -> ApiError {
    ApiError::other("Attempting to use a disconnected port object")
}

#[cfg(test)]
mod tests;
