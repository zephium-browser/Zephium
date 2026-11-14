//! Internal binding messages. These use the ordinary authenticated dispatcher
//! and never appear in the generated extension API surface.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::context::{ContextKey, ContextKind, ContextMask, Sender};
use crate::dispatch::{encode_for, ApiTable, Handled, Invocation, MethodSpec, RegistrationError};
use crate::envelope::{ApiName, Envelope, EventSeq};
use crate::errors::ApiError;
use crate::events::{EventRegistry, EventsHost, ListenerId, ListenerRequest};
use crate::lifecycle::{BackgroundState, Command, Lifecycle, Observation, EVENT_TIMEOUT_NANOS};
use crate::limits::{Clock, MAX_OUTSTANDING_CALLS_PER_CONTEXT};
use crate::permissions::Gate;
use crate::ports::{ListenerOutcome, MessageId};

/// Host notification that permanently invalidates the receiving binding.
pub const ON_INVALIDATED: &str = "zephium.onInvalidated";
/// Host notification carrying the new granted API permission names.
pub const ON_PERMISSIONS_CHANGED: &str = "zephium.onPermissionsChanged";

/// A bounded worker error. It is untrusted diagnostic data, never log authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerError {
    /// Error text, at most 1,024 bytes.
    pub message: String,
    /// Script filename, at most 2,048 bytes; may contain sensitive URL data.
    pub filename: String,
    /// Reported source line.
    pub line: u32,
    /// Reported source column.
    pub column: u32,
}

/// Background first-task evaluation outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerLifecycle {
    /// Install promises completed; accept only after delivering `onInstalled`.
    Installed,
    /// Evaluation and install/activate promises completed successfully.
    Activated,
    /// Evaluation or a lifecycle promise failed.
    Failed(Option<String>),
    /// The binding's lifecycle deadline expired.
    TimedOut,
}

/// A validated internal call, still scoped to its authenticated invocation.
#[derive(Clone, Debug)]
pub enum ProtocolRequest {
    /// One exact callback's message response or asynchronous-channel observation.
    Respond {
        /// Host-minted message identity.
        message: MessageId,
        /// Context-local callback identity.
        listener: ListenerId,
        /// Observation for `MessageService::observe`.
        outcome: ListenerOutcome,
    },
    /// A blocking callback result. Apply only through an expected-result ledger.
    EventResult {
        /// Host event delivery identity.
        sequence: EventSeq,
        /// Context-local callback identity.
        listener: ListenerId,
        /// Untrusted listener result.
        value: Value,
    },
    /// An error observed by the background owner.
    WorkerError(WorkerError),
    /// Background first-task evaluation finished.
    WorkerLifecycle(WorkerLifecycle),
}

/// Adapter seam for existing message, blocking-event and lifetime services.
pub trait ProtocolHost: EventsHost {
    /// Applies a request with the exact invocation's identity. Message responses
    /// use `MessageService::observe`, worker reports use `WorkerSession`, and
    /// blocking results use `EventResults`; none accepts caller-selected scope.
    fn protocol_request(&mut self, request: ProtocolRequest, invocation: Invocation<'_>)
        -> Handled;
}

/// Registers the private binding protocol. Public event operations can still be
/// registered through `events::register`; both registration-complete aliases
/// reach the same `ListenerRequest::CompleteRegistration` operation.
pub fn register<S: ProtocolHost + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for (name, add) in [
        ("zephium.addListener", true),
        ("zephium.removeListener", false),
    ] {
        table.register(
            name,
            MethodSpec::new(
                Gate::Open,
                extension_contexts(),
                move |host: &mut S, invocation| match listener_request(&invocation, add) {
                    Ok(request) => host.listener_request(request, invocation),
                    Err(error) => invocation.reply(Err(error)),
                },
            ),
        )?;
    }
    for name in [
        "events.completeRegistration",
        "runtime.markListenerRegistrationComplete",
    ] {
        if table.contexts(name).is_none() {
            table.register(
                name,
                MethodSpec::new(
                    Gate::Open,
                    ContextMask::of(ContextKind::Background),
                    |host: &mut S, invocation| {
                        if invocation.args().is_empty() {
                            host.listener_request(ListenerRequest::CompleteRegistration, invocation)
                        } else {
                            invocation.reply(Err(ApiError::malformed_arguments()))
                        }
                    },
                ),
            )?;
        }
    }
    for name in [
        "zephium.respond",
        "zephium.eventResult",
        "zephium.workerError",
        "zephium.workerLifecycle",
    ] {
        let contexts = if name == "zephium.workerError" || name == "zephium.workerLifecycle" {
            ContextMask::of(ContextKind::Background)
        } else {
            extension_contexts()
        };
        table.register(
            name,
            MethodSpec::new(Gate::Open, contexts, move |host: &mut S, invocation| {
                match parse_request(name, invocation.args()) {
                    Ok(request) => host.protocol_request(request, invocation),
                    Err(error) => invocation.reply(Err(error)),
                }
            }),
        )?;
    }
    Ok(())
}

fn extension_contexts() -> ContextMask {
    ContextMask::ALL.without(ContextKind::ExternalPage)
}

fn listener_request(invocation: &Invocation<'_>, add: bool) -> Result<ListenerRequest, ApiError> {
    let args = invocation.args();
    if args.len() != if add { 4 } else { 2 } {
        return Err(ApiError::malformed_arguments());
    }
    let name = args[0].as_str().ok_or_else(ApiError::malformed_arguments)?;
    let event = crate::events::EVENT_APIS
        .iter()
        .copied()
        .find(|event| *event == name)
        .ok_or_else(ApiError::unknown_method)?;
    let access = crate::schema::access(&format!("{event}.addListener"))
        .ok_or_else(ApiError::unknown_method)?;
    if !crate::events::event_contexts(event).contains(invocation.sender().kind()) {
        return Err(ApiError::not_available_in_context(event));
    }
    access
        .gate
        .check(invocation.permissions(), invocation.sender(), None)?;
    let request = crate::events::parse_listener_request(
        event,
        if add { "addListener" } else { "removeListener" },
        &args[1..],
    )?;
    crate::events::check_listener_permissions(&request, invocation)?;
    Ok(request)
}

fn parse_request(name: &str, args: &[Value]) -> Result<ProtocolRequest, ApiError> {
    let bad = ApiError::malformed_arguments;
    match (name, args) {
        ("zephium.respond", [message, listener, value]) => {
            let message = decimal(message).and_then(MessageId::new).ok_or_else(bad)?;
            let listener = listener_id(listener)?;
            let outcome = if value.is_null() {
                ListenerOutcome::NoResponse
            } else {
                let object = value
                    .as_object()
                    .filter(|object| object.len() == 1)
                    .ok_or_else(bad)?;
                if let Some(value) = object.get("response") {
                    let bytes = if value.is_null() {
                        b"null".to_vec()
                    } else {
                        let text = value.as_str().ok_or_else(bad)?;
                        text.as_bytes().to_vec()
                    };
                    ListenerOutcome::Response(bytes)
                } else if let Some(value) = object.get("error") {
                    ListenerOutcome::Rejected(ApiError::other(
                        bounded_text(value, crate::envelope::MAX_ERROR_MESSAGE_BYTES)?.to_owned(),
                    ))
                } else if let Some(value) = object.get("async") {
                    match value.as_str() {
                        Some("callback") => ListenerOutcome::AsyncCallback,
                        Some("promise") => ListenerOutcome::Promise,
                        _ => return Err(bad()),
                    }
                } else {
                    return Err(bad());
                }
            };
            Ok(ProtocolRequest::Respond {
                message,
                listener,
                outcome,
            })
        }
        ("zephium.eventResult", [sequence, listener, value]) => Ok(ProtocolRequest::EventResult {
            sequence: decimal(sequence).and_then(EventSeq::new).ok_or_else(bad)?,
            listener: listener_id(listener)?,
            value: value.clone(),
        }),
        ("zephium.workerLifecycle", [state, detail]) => {
            let detail = if detail.is_null() {
                None
            } else {
                Some(bounded_text(detail, 1024)?.to_owned())
            };
            Ok(ProtocolRequest::WorkerLifecycle(match state.as_str() {
                Some("installed") if detail.is_none() => WorkerLifecycle::Installed,
                Some("activated") if detail.is_none() => WorkerLifecycle::Activated,
                Some("failed") => WorkerLifecycle::Failed(detail),
                Some("timedOut") if detail.is_none() => WorkerLifecycle::TimedOut,
                _ => return Err(bad()),
            }))
        }
        ("zephium.workerError", [detail]) => {
            let object = detail
                .as_object()
                .filter(|object| object.len() == 4)
                .ok_or_else(bad)?;
            Ok(ProtocolRequest::WorkerError(WorkerError {
                message: bounded_text(object.get("message").ok_or_else(bad)?, 1024)?.to_owned(),
                filename: bounded_text(object.get("filename").ok_or_else(bad)?, 2048)?.to_owned(),
                line: object
                    .get("lineno")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(bad)?,
                column: object
                    .get("colno")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(bad)?,
            }))
        }
        _ => Err(bad()),
    }
}

fn bounded_text(value: &Value, max: usize) -> Result<&str, ApiError> {
    value
        .as_str()
        .filter(|text| text.len() <= max)
        .ok_or_else(ApiError::malformed_arguments)
}

fn listener_id(value: &Value) -> Result<ListenerId, ApiError> {
    value
        .as_u64()
        .filter(|id| *id <= 9_007_199_254_740_991)
        .and_then(ListenerId::new)
        .ok_or_else(ApiError::malformed_arguments)
}

fn decimal(value: &Value) -> Option<u64> {
    let text = value.as_str()?;
    if text.is_empty()
        || text.len() > 20
        || text.starts_with('0')
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    text.parse().ok()
}

/// Encodes an invalidation for one authenticated context, outside the public API.
pub fn invalidated(sender: &Sender, sequence: EventSeq) -> Result<Vec<u8>, ApiError> {
    notification(
        sender,
        sequence,
        ON_INVALIDATED,
        json!([ApiError::context_invalidated().message()]),
    )
}

/// Encodes a permissions projection. Permission changes remain authoritative in
/// Rust; the binding only updates which API members it exposes.
pub fn permissions_changed(
    sender: &Sender,
    sequence: EventSeq,
    permissions: &[String],
) -> Result<Vec<u8>, ApiError> {
    if permissions.len() > 256
        || permissions
            .iter()
            .any(|name| crate::permissions::ApiPermission::parse(name).is_none())
    {
        return Err(ApiError::malformed_arguments());
    }
    notification(
        sender,
        sequence,
        ON_PERMISSIONS_CHANGED,
        json!([permissions]),
    )
}

fn notification(
    sender: &Sender,
    sequence: EventSeq,
    name: &str,
    arguments: Value,
) -> Result<Vec<u8>, ApiError> {
    let args = serde_json::to_vec(&arguments).map_err(|_| ApiError::malformed_arguments())?;
    let name = ApiName::parse(name).ok_or_else(ApiError::malformed_arguments)?;
    encode_for(
        sender.kind(),
        &Envelope::Event {
            seq: sequence,
            name,
            args: &args,
        },
    )
    .map_err(|_| ApiError::message_too_large())
}

/// Binds worker observations to one host-selected background process generation.
#[derive(Clone, Debug)]
pub struct WorkerSession {
    context: ContextKey,
    generation: u64,
}

impl WorkerSession {
    /// Captures host facts when a background is admitted and bound.
    pub fn new(sender: &Sender, lifecycle: &Lifecycle, generation: u64) -> Result<Self, ApiError> {
        if sender.kind() != ContextKind::Background
            || sender.profile() != lifecycle.profile()
            || lifecycle.generation(sender.install()) != Some(generation)
            || lifecycle.state(sender.install()) != Some(BackgroundState::Starting)
        {
            return Err(ApiError::context_invalidated());
        }
        Ok(Self {
            context: sender.context_key(),
            generation,
        })
    }

    fn check(&self, sender: &Sender, lifecycle: &Lifecycle) -> Result<(), ApiError> {
        if sender.context_key() != self.context
            || sender.profile() != lifecycle.profile()
            || lifecycle.generation(sender.install()) != Some(self.generation)
            || !matches!(
                lifecycle.state(sender.install()),
                Some(BackgroundState::Starting | BackgroundState::Running)
            )
        {
            return Err(ApiError::context_invalidated());
        }
        Ok(())
    }

    /// Applies evaluation success or failure after checking this exact session.
    /// Persist the registry's snapshot before executing returned delivery effects.
    pub fn observe(
        &self,
        sender: &Sender,
        report: WorkerLifecycle,
        lifecycle: &mut Lifecycle,
        events: &mut EventRegistry,
        clock: &impl Clock,
    ) -> Result<Vec<Command>, ApiError> {
        self.check(sender, lifecycle)?;
        let observation = match report {
            WorkerLifecycle::Installed => return Ok(Vec::new()),
            WorkerLifecycle::Activated => {
                if lifecycle.state(sender.install()) == Some(BackgroundState::Running) {
                    return Ok(Vec::new());
                }
                events.finish_registration(sender)?;
                Observation::Started {
                    install: sender.install(),
                    generation: self.generation,
                }
            }
            WorkerLifecycle::Failed(_) | WorkerLifecycle::TimedOut => {
                events.cancel_context(&self.context);
                Observation::WorkerFailed {
                    install: sender.install(),
                    generation: self.generation,
                }
            }
        };
        lifecycle.observe(observation, clock)
    }

    /// Applies an owner-observed worker failure; diagnostic fields confer no scope.
    pub fn error(
        &self,
        sender: &Sender,
        lifecycle: &mut Lifecycle,
        events: &mut EventRegistry,
        clock: &impl Clock,
    ) -> Result<Vec<Command>, ApiError> {
        self.observe(
            sender,
            WorkerLifecycle::Failed(None),
            lifecycle,
            events,
            clock,
        )
    }
}

/// Bounded exact-callback authority for outstanding blocking event results.
#[derive(Debug, Default)]
pub struct EventResults {
    expected: HashMap<(ContextKey, EventSeq, ListenerId), u64>,
}

impl EventResults {
    /// An empty result ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an actually delivered blocking callback, using registry authority.
    pub fn expect(
        &mut self,
        sender: &Sender,
        events: &EventRegistry,
        event: &str,
        sequence: EventSeq,
        listener: ListenerId,
        clock: &impl Clock,
    ) -> Result<(), ApiError> {
        if !events.is_blocking(sender, listener, event) {
            return Err(ApiError::context_invalidated());
        }
        self.expire(clock);
        let key = (sender.context_key(), sequence, listener);
        if self.expected.contains_key(&key) {
            return Ok(());
        }
        if self
            .expected
            .keys()
            .filter(|(context, _, _)| *context == sender.context_key())
            .count()
            >= MAX_OUTSTANDING_CALLS_PER_CONTEXT as usize
            || self.expected.len() >= crate::events::MAX_LISTENERS_PER_PROFILE
        {
            return Err(ApiError::too_many_pending());
        }
        self.expected
            .insert(key, clock.now_nanos().saturating_add(EVENT_TIMEOUT_NANOS));
        Ok(())
    }

    /// Accepts exactly one result from the context/callback it was delivered to.
    pub fn take(
        &mut self,
        sender: &Sender,
        sequence: EventSeq,
        listener: ListenerId,
        clock: &impl Clock,
    ) -> Result<(), ApiError> {
        self.expire(clock);
        self.expected
            .remove(&(sender.context_key(), sequence, listener))
            .map(|_| ())
            .ok_or_else(ApiError::context_invalidated)
    }

    /// Drops results whose processing deadline elapsed.
    pub fn expire(&mut self, clock: &impl Clock) {
        let now = clock.now_nanos();
        self.expected.retain(|_, deadline| now < *deadline);
    }

    /// Revokes all pending result authority for a cancelled document.
    pub fn cancel_context(&mut self, context: &ContextKey) {
        self.expected.retain(|(owner, _, _), _| owner != context);
    }
}

#[cfg(test)]
mod tests;
