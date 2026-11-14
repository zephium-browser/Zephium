//! The call pipeline: decode, look up, check the context, check the
//! permission, parse, validate, handle, settle.
//!
//! Handlers either answer at once or keep a [`Responder`] and settle later
//! through [`Dispatcher::settle`], from whatever executor the adapter uses.

use std::collections::HashMap;
use std::fmt;

use serde_json::Value;

use crate::context::{ContextKey, ContextKind, ContextMask, Sender};
use crate::envelope::{
    parse_arguments, ApiName, CallId, DecodeError, Direction, EncodeError, Envelope, EventSeq,
    PortId,
};
use crate::errors::ApiError;
use crate::limits::{message_limit, Charge, Limiter, Reservation};
use crate::permissions::{Gate, InstallPermissions, Target};

/// Checks parsed arguments against the method's schema.
pub type Validator = fn(&[Value]) -> Result<(), ApiError>;

/// A method implementation.
pub type Handler<S> = Box<dyn Fn(&mut S, Invocation<'_>) -> Handled + Send + Sync>;

const EXTERNAL_PAGE_METHODS: [&str; 2] = ["runtime.sendMessage", "runtime.connect"];

/// The contexts section 6 of the design lets `name` reach at most. Framed
/// extension pages and scripts in web pages never get native messaging,
/// `permissions.request`, `cookies`, `management`, `debugger`, `privacy`
/// writes or declarative-net-request mutation; external pages get only
/// `runtime.sendMessage` and `runtime.connect`.
pub fn permitted_contexts(name: &str) -> ContextMask {
    let mut mask = ContextMask::ALL;
    if !EXTERNAL_PAGE_METHODS.contains(&name) {
        mask = mask.without(ContextKind::ExternalPage);
    }
    if is_privileged_only(name) {
        mask = ContextMask::PRIVILEGED_PAGES;
    }
    mask
}

fn is_privileged_only(name: &str) -> bool {
    let Some((namespace, member)) = name.split_once('.') else {
        return true;
    };
    match namespace {
        "runtime" => matches!(member, "connectNative" | "sendNativeMessage"),
        "permissions" => member == "request",
        "cookies" | "management" | "debugger" => true,
        "privacy" => member.ends_with(".set") || member.ends_with(".clear"),
        "declarativeNetRequest" => {
            member.starts_with("update") || member == "setExtensionActionOptions"
        }
        _ => false,
    }
}

/// Why a method could not be registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistrationError {
    /// Not a dotted API name.
    InvalidName(&'static str),
    /// Already registered.
    Duplicate(&'static str),
    /// No context may call it.
    NoContexts(&'static str),
    /// It names contexts [`permitted_contexts`] excludes.
    ForbiddenContexts {
        /// The method.
        name: &'static str,
        /// The excluded contexts it asked for.
        contexts: ContextMask,
    },
}

/// How one method is reached and gated.
pub struct MethodSpec<S> {
    gate: Gate,
    contexts: ContextMask,
    validate: Option<Validator>,
    generated_schema: bool,
    handler: Handler<S>,
}

impl<S> MethodSpec<S> {
    /// A method available to `contexts` with the `gate` permission.
    pub fn new(
        gate: Gate,
        contexts: ContextMask,
        handler: impl Fn(&mut S, Invocation<'_>) -> Handled + Send + Sync + 'static,
    ) -> Self {
        Self {
            gate,
            contexts,
            validate: None,
            generated_schema: false,
            handler: Box::new(handler),
        }
    }

    /// Adds schema validation, run after the gate and before the handler.
    pub fn validate(mut self, validator: Validator) -> Self {
        self.validate = Some(validator);
        self
    }
}

impl<S> fmt::Debug for MethodSpec<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MethodSpec")
            .field("gate", &self.gate)
            .field("contexts", &self.contexts)
            .finish_non_exhaustive()
    }
}

/// `namespace.method` to its gate, contexts and handler.
pub struct ApiTable<S> {
    methods: HashMap<&'static str, MethodSpec<S>>,
}

impl<S> Default for ApiTable<S> {
    fn default() -> Self {
        Self {
            methods: HashMap::new(),
        }
    }
}

impl<S> fmt::Debug for ApiTable<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiTable")
            .field("methods", &self.methods.len())
            .finish()
    }
}

impl<S> ApiTable<S> {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one method.
    pub fn register(
        &mut self,
        name: &'static str,
        mut spec: MethodSpec<S>,
    ) -> Result<(), RegistrationError> {
        if ApiName::parse(name).is_none() {
            return Err(RegistrationError::InvalidName(name));
        }
        if spec.contexts.is_empty() {
            return Err(RegistrationError::NoContexts(name));
        }
        let permitted = permitted_contexts(name);
        if spec.contexts.intersection(permitted) != spec.contexts {
            let forbidden = ContextKind::ALL
                .into_iter()
                .filter(|kind| spec.contexts.contains(*kind) && !permitted.contains(*kind))
                .fold(ContextMask::NONE, ContextMask::with);
            return Err(RegistrationError::ForbiddenContexts {
                name,
                contexts: forbidden,
            });
        }
        if self.methods.contains_key(name) {
            return Err(RegistrationError::Duplicate(name));
        }
        if spec.validate.is_none() {
            spec.validate = crate::api::generated::validator(name);
            spec.generated_schema = spec.validate.is_some();
        }
        self.methods.insert(name, spec);
        Ok(())
    }

    /// The contexts a registered method is available to.
    pub fn contexts(&self, name: &str) -> Option<ContextMask> {
        self.methods.get(name).map(|spec| spec.contexts)
    }

    /// The number of registered methods.
    pub fn len(&self) -> usize {
        self.methods.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }
}

/// The settlement handle of a deferred call. Settling it after its context
/// was cancelled delivers nothing.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a deferred call must be settled"]
pub struct Responder {
    context: ContextKey,
    id: CallId,
    ticket: u64,
}

impl Responder {
    /// The context awaiting the reply.
    pub fn context(&self) -> &ContextKey {
        &self.context
    }
}

/// What a handler did with its call.
#[derive(Debug)]
pub struct Handled(Answer);

#[derive(Debug)]
enum Answer {
    Now(Result<Value, ApiError>),
    Later,
}

/// One authenticated, admitted, permitted call.
#[derive(Debug)]
pub struct Invocation<'a> {
    sender: &'a Sender,
    name: &'static str,
    args: Vec<Value>,
    permissions: &'a InstallPermissions,
    gate: Gate,
    responder: Responder,
}

impl<'a> Invocation<'a> {
    /// The caller.
    pub fn sender(&self) -> &'a Sender {
        self.sender
    }

    /// `namespace.method`.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The parsed argument array. Generated schema validation inserts null
    /// slots for omitted optionals and applies Chromium object conversion.
    pub fn args(&self) -> &[Value] {
        &self.args
    }

    /// Takes the arguments, leaving none.
    pub fn take_args(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.args)
    }

    /// The install's permission state.
    pub fn permissions(&self) -> &'a InstallPermissions {
        self.permissions
    }

    /// Whether the method may act on `target`.
    pub fn check_target(&self, target: &Target<'_>) -> Result<(), ApiError> {
        self.gate.check(self.permissions, self.sender, Some(target))
    }

    /// Answers now.
    pub fn reply(self, result: Result<Value, ApiError>) -> Handled {
        Handled(Answer::Now(result))
    }

    /// Answers later through [`Dispatcher::settle`].
    pub fn defer(self) -> (Handled, Responder) {
        (Handled(Answer::Later), self.responder)
    }
}

/// Why an inbound frame produced nothing to reply to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The frame did not decode.
    Decode(DecodeError),
    /// The context reused an outstanding call id.
    DuplicateCallId(CallId),
    /// The context is over its rate or byte budget.
    Overloaded,
}

/// What the adapter does with one inbound frame.
#[derive(Debug)]
pub enum Inbound<'a> {
    /// Deliver this encoded reply to the sending context.
    Reply(Vec<u8>),
    /// A handler deferred the call.
    Pending,
    /// Route a port message; release `reservation` once it is delivered.
    PortMessage {
        /// The port.
        port: PortId,
        /// The opaque payload.
        payload: &'a [u8],
        /// The budget the message holds.
        reservation: Reservation,
    },
    /// A port message over budget; disconnect the port with `error`.
    PortRefused {
        /// The port.
        port: PortId,
        /// The disconnect error.
        error: ApiError,
    },
    /// The context closed a port.
    PortDisconnect {
        /// The port.
        port: PortId,
    },
    /// The context handled an event.
    EventAck {
        /// The delivery.
        seq: EventSeq,
    },
    /// Nothing to do.
    Refused(Refusal),
}

/// A settled deferred call.
#[derive(Debug, PartialEq, Eq)]
pub struct Settled {
    /// The context to deliver to.
    pub context: ContextKey,
    /// The encoded reply.
    pub frame: Vec<u8>,
}

#[derive(Debug)]
struct PendingCall {
    ticket: u64,
    kind: ContextKind,
    reservation: Reservation,
}

/// A table plus the per-context state the pipeline needs.
#[derive(Debug)]
pub struct Dispatcher<S> {
    table: ApiTable<S>,
    limiter: Limiter,
    pending: HashMap<(ContextKey, CallId), PendingCall>,
    next_ticket: u64,
}

/// Encodes a frame for a context of `kind`.
pub fn encode_for(kind: ContextKind, envelope: &Envelope<'_>) -> Result<Vec<u8>, EncodeError> {
    envelope.encode(Direction::ToContext, message_limit(kind))
}

fn reply_frame(kind: ContextKind, id: CallId, result: &Result<Value, ApiError>) -> Vec<u8> {
    let encoded = match result {
        Ok(value) => serde_json::to_vec(value).ok().and_then(|json| {
            encode_for(
                kind,
                &Envelope::Reply {
                    id,
                    result: Ok(&json),
                },
            )
            .ok()
        }),
        Err(error) => encode_for(
            kind,
            &Envelope::Reply {
                id,
                result: Err(error.message()),
            },
        )
        .ok(),
    };
    encoded.unwrap_or_else(|| error_frame(kind, id, &ApiError::message_too_large()))
}

fn error_frame(kind: ContextKind, id: CallId, error: &ApiError) -> Vec<u8> {
    let message = error.message();
    let bounded =
        &message[..floor_char_boundary(message, crate::envelope::MAX_ERROR_MESSAGE_BYTES)];
    encode_for(
        kind,
        &Envelope::Reply {
            id,
            result: Err(bounded),
        },
    )
    .unwrap_or_default()
}

fn floor_char_boundary(text: &str, max: usize) -> usize {
    if text.len() <= max {
        return text.len();
    }
    (0..=max)
        .rev()
        .find(|at| text.is_char_boundary(*at))
        .unwrap_or(0)
}

/// The call id of a frame too large to decode, read from its fixed header
/// only, so the refusal can still be answered.
fn oversize_call_id(bytes: &[u8]) -> Option<CallId> {
    let header = bytes.get(..10)?;
    if header[4] != crate::envelope::ENVELOPE_VERSION || header[5] != 1 {
        return None;
    }
    CallId::new(u32::from_le_bytes([
        header[6], header[7], header[8], header[9],
    ]))
}

impl<S> Dispatcher<S> {
    /// A dispatcher over `table`.
    pub fn new(table: ApiTable<S>) -> Self {
        Self {
            table,
            limiter: Limiter::new(),
            pending: HashMap::new(),
            next_ticket: 1,
        }
    }

    /// The table.
    pub fn table(&self) -> &ApiTable<S> {
        &self.table
    }

    /// The accounting ledger.
    pub fn limiter(&self) -> &Limiter {
        &self.limiter
    }

    /// Shared accounting for service deliveries. Return every reservation through
    /// this ledger; API calls and port traffic share the same install budget.
    pub fn limiter_mut(&mut self) -> &mut Limiter {
        &mut self.limiter
    }

    /// Calls awaiting settlement.
    pub fn pending_calls(&self) -> usize {
        self.pending.len()
    }

    /// Handles one frame `sender`'s context delivered. `permissions` is
    /// the state of `sender.install()`; `now` is [`crate::limits::Clock`] time.
    pub fn receive<'a>(
        &mut self,
        state: &mut S,
        permissions: &InstallPermissions,
        sender: &Sender,
        bytes: &'a [u8],
        now: u64,
    ) -> Inbound<'a> {
        let kind = sender.kind();
        let envelope = match Envelope::decode(bytes, Direction::FromContext, message_limit(kind)) {
            Ok(envelope) => envelope,
            Err(DecodeError::TooLarge) => {
                return match oversize_call_id(bytes) {
                    Some(id) => {
                        Inbound::Reply(error_frame(kind, id, &ApiError::message_too_large()))
                    }
                    None => Inbound::Refused(Refusal::Decode(DecodeError::TooLarge)),
                };
            }
            Err(error) => return Inbound::Refused(Refusal::Decode(error)),
        };
        let context = sender.context_key();
        let length = bytes.len() as u64;
        match envelope {
            Envelope::Call { id, name, args } => {
                if self.pending.contains_key(&(context.clone(), id)) {
                    return Inbound::Refused(Refusal::DuplicateCallId(id));
                }
                let reservation =
                    match self
                        .limiter
                        .admit(&context, kind, length, Charge::Call, now)
                    {
                        Ok(reservation) => reservation,
                        Err(error) => return Inbound::Reply(error_frame(kind, id, &error)),
                    };
                let ticket = self.next_ticket;
                self.next_ticket = ticket.wrapping_add(1);
                let responder = Responder {
                    context: context.clone(),
                    id,
                    ticket,
                };
                match self.invoke(state, permissions, sender, name, args, responder) {
                    Handled(Answer::Now(result)) => {
                        self.limiter.release(reservation);
                        Inbound::Reply(reply_frame(kind, id, &result))
                    }
                    Handled(Answer::Later) => {
                        self.pending.insert(
                            (context, id),
                            PendingCall {
                                ticket,
                                kind,
                                reservation,
                            },
                        );
                        Inbound::Pending
                    }
                }
            }
            Envelope::PortMessage { port, payload } => {
                match self
                    .limiter
                    .admit(&context, kind, length, Charge::Message, now)
                {
                    Ok(reservation) => Inbound::PortMessage {
                        port,
                        payload,
                        reservation,
                    },
                    Err(error) => Inbound::PortRefused { port, error },
                }
            }
            Envelope::PortDisconnect { port, .. } => {
                match self.limiter.admit(&context, kind, 0, Charge::Message, now) {
                    Ok(reservation) => {
                        self.limiter.release(reservation);
                        Inbound::PortDisconnect { port }
                    }
                    Err(_) => Inbound::Refused(Refusal::Overloaded),
                }
            }
            Envelope::EventAck { seq } => {
                match self.limiter.admit(&context, kind, 0, Charge::Message, now) {
                    Ok(reservation) => {
                        self.limiter.release(reservation);
                        Inbound::EventAck { seq }
                    }
                    Err(_) => Inbound::Refused(Refusal::Overloaded),
                }
            }
            Envelope::Reply { .. } | Envelope::Event { .. } => {
                Inbound::Refused(Refusal::Decode(DecodeError::WrongDirection))
            }
        }
    }

    fn invoke(
        &self,
        state: &mut S,
        permissions: &InstallPermissions,
        sender: &Sender,
        name: ApiName<'_>,
        args: &[u8],
        responder: Responder,
    ) -> Handled {
        let Some((&name, spec)) = self.table.methods.get_key_value(name.as_str()) else {
            return Handled(Answer::Now(Err(ApiError::unknown_method())));
        };
        if !spec.contexts.contains(sender.kind()) {
            return Handled(Answer::Now(Err(ApiError::not_available_in_context(name))));
        }
        if let Err(error) = spec.gate.check(permissions, sender, None) {
            return Handled(Answer::Now(Err(error)));
        }
        let mut args = match parse_arguments(args) {
            Ok(args) => args,
            Err(_) => return Handled(Answer::Now(Err(ApiError::malformed_arguments()))),
        };
        if spec.generated_schema {
            args = match crate::schema::normalize(name, &args) {
                Ok(args) => args,
                Err(error) => return Handled(Answer::Now(Err(error))),
            };
        } else if let Some(validate) = spec.validate {
            if let Err(error) = validate(&args) {
                return Handled(Answer::Now(Err(error)));
            }
        }
        (spec.handler)(
            state,
            Invocation {
                sender,
                name,
                args,
                permissions,
                gate: spec.gate,
                responder,
            },
        )
    }

    /// Settles a deferred call. `None` when its context was cancelled.
    pub fn settle(
        &mut self,
        responder: Responder,
        result: Result<Value, ApiError>,
    ) -> Option<Settled> {
        let key = (responder.context, responder.id);
        if self
            .pending
            .get(&key)
            .is_none_or(|pending| pending.ticket != responder.ticket)
        {
            return None;
        }
        let pending = self.pending.remove(&key)?;
        self.limiter.release(pending.reservation);
        let frame = reply_frame(pending.kind, key.1, &result);
        Some(Settled {
            context: key.0,
            frame,
        })
    }

    /// Returns a port message's budget after delivery.
    pub fn release(&mut self, reservation: Reservation) {
        self.limiter.release(reservation);
    }

    /// Forgets a dead context's calls; their responders settle nothing.
    pub fn cancel_context(&mut self, context: &ContextKey) -> usize {
        let keys: Vec<(ContextKey, CallId)> = self
            .pending
            .keys()
            .filter(|(owner, _)| owner == context)
            .cloned()
            .collect();
        for key in &keys {
            if let Some(pending) = self.pending.remove(key) {
                self.limiter.release(pending.reservation);
            }
        }
        self.limiter.forget_idle(context);
        keys.len()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use zephium_webext::manifest::Manifest;

    use super::*;
    use zephium_core::ids::ItemId;

    use crate::context::tests::{
        authority, background_sender, committed, content_sender, EXTENSION,
    };
    use crate::context::{ClaimedWorld, FrameId, TabDocument};
    use crate::limits::{MAX_MESSAGE_BYTES_IN_WEB_RENDERER, RATE_BURST};
    use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};

    #[derive(Default)]
    struct State {
        calls: Vec<&'static str>,
        parked: Vec<Responder>,
    }

    fn table() -> ApiTable<State> {
        let mut table = ApiTable::new();
        table
            .register(
                "runtime.getPlatformInfo",
                MethodSpec::new(
                    Gate::Open,
                    ContextMask::EXTENSION_PAGES,
                    |state: &mut State, call| {
                        state.calls.push(call.name());
                        call.reply(Ok(json!({"os": "linux"})))
                    },
                ),
            )
            .expect("register");
        table
            .register(
                "history.search",
                MethodSpec::new(
                    Gate::Requires("history"),
                    ContextMask::EXTENSION_PAGES,
                    |_: &mut State, call| call.reply(Ok(json!([]))),
                ),
            )
            .expect("register");
        table
            .register(
                "runtime.connectNative",
                MethodSpec::new(
                    Gate::Requires("nativeMessaging"),
                    ContextMask::PRIVILEGED_PAGES,
                    |_: &mut State, call| call.reply(Ok(Value::Null)),
                ),
            )
            .expect("register");
        table
            .register(
                "storage.get",
                MethodSpec::new(
                    Gate::Open,
                    ContextMask::EXTENSION_PAGES.union(ContextMask::PAGE_SCRIPTS),
                    |state: &mut State, call| {
                        let (handled, responder) = call.defer();
                        state.parked.push(responder);
                        handled
                    },
                )
                .validate(|args| match args {
                    [Value::String(_)] => Ok(()),
                    _ => Err(ApiError::no_matching_signature(
                        "storage.get",
                        "string keys",
                    )),
                }),
            )
            .expect("register");
        table
    }

    fn permissions(manifest: Value) -> InstallPermissions {
        let (declaration, _) =
            PermissionDeclaration::from_manifest(&Manifest::from_value(manifest));
        InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        )
    }

    fn call_bytes(id: u32, name: &str, args: &str) -> Vec<u8> {
        Envelope::Call {
            id: CallId::new(id).expect("id"),
            name: ApiName::parse(name).expect("name"),
            args: args.as_bytes(),
        }
        .encode(Direction::FromContext, u64::MAX)
        .expect("encode")
    }

    fn reply(inbound: Inbound<'_>) -> (u32, Result<String, String>) {
        let Inbound::Reply(bytes) = inbound else {
            panic!("expected a reply, got {inbound:?}");
        };
        match Envelope::decode(&bytes, Direction::ToContext, u64::MAX).expect("reply decodes") {
            Envelope::Reply { id, result } => (
                id.get(),
                result
                    .map(|json| String::from_utf8(json.to_vec()).expect("utf8"))
                    .map_err(str::to_owned),
            ),
            other => panic!("expected a reply, got {other:?}"),
        }
    }

    #[test]
    fn a_call_runs_the_whole_pipeline() {
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let sender = background_sender();
        let bytes = call_bytes(4, "runtime.getPlatformInfo", "[]");
        let inbound = dispatcher.receive(&mut state, &permissions(json!({})), &sender, &bytes, 0);
        assert_eq!(reply(inbound), (4, Ok(r#"{"os":"linux"}"#.to_owned())));
        assert_eq!(state.calls, ["runtime.getPlatformInfo"]);
        assert_eq!(dispatcher.limiter().context_calls(&sender.context_key()), 0);
    }

    #[test]
    fn generic_failures_settle_with_chromes_errors_in_pipeline_order() {
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let none = permissions(json!({}));
        let background = background_sender();
        let content = content_sender("https://example.com/");
        let mut run = |sender: &Sender, permissions: &InstallPermissions, bytes: Vec<u8>| {
            reply(dispatcher.receive(&mut state, permissions, sender, &bytes, 0)).1
        };

        assert_eq!(
            run(&background, &none, call_bytes(1, "tabs.nothing", "[]")),
            Err("Access to extension API denied.".into())
        );
        assert_eq!(
            run(&content, &none, call_bytes(2, "history.search", "[]")),
            Err("'history.search' is not available in this context.".into())
        );
        assert_eq!(
            run(&background, &none, call_bytes(3, "history.search", "[]")),
            Err("Access to extension API denied.".into())
        );
        let history = permissions(json!({"permissions": ["history"]}));
        assert_eq!(
            run(&background, &history, call_bytes(4, "history.search", "{")),
            Err(ApiError::malformed_arguments().message().to_owned())
        );
        assert_eq!(
            run(
                &background,
                &history,
                call_bytes(5, "history.search", r#"[{"text":""}]"#)
            ),
            Ok("[]".into())
        );
        assert_eq!(
            run(&content, &none, call_bytes(6, "storage.get", "[1]")),
            Err("Error in invocation of storage.get(string keys): No matching signature.".into())
        );
    }

    #[test]
    fn generated_schemas_refuse_before_handlers_and_normalize_arguments() {
        let mut table = ApiTable::new();
        for name in ["tabs.query", "runtime.sendMessage", "storage.local.get"] {
            table
                .register(
                    name,
                    MethodSpec::new(
                        Gate::Open,
                        ContextMask::EXTENSION_PAGES,
                        |state: &mut Vec<Vec<Value>>, call| {
                            state.push(call.args().to_vec());
                            call.reply(Ok(Value::Null))
                        },
                    ),
                )
                .expect("register");
        }
        let mut dispatcher = Dispatcher::new(table);
        let mut state = Vec::new();
        let sender = background_sender();
        let none = permissions(json!({}));
        let (_, result) = reply(dispatcher.receive(
            &mut state,
            &none,
            &sender,
            &call_bytes(1, "tabs.query", r#"[{"active":"yes"}]"#),
            0,
        ));
        assert!(result
            .expect_err("schema error")
            .contains("Error at property 'active'"));
        assert!(state.is_empty());
        for (id, name, arguments) in [
            (2, "runtime.sendMessage", r#"[{"payload":null}]"#),
            (3, "storage.local.get", r#"[{"key":null}]"#),
            (4, "tabs.query", r#"[{"active":null}]"#),
        ] {
            let (_, result) = reply(dispatcher.receive(
                &mut state,
                &none,
                &sender,
                &call_bytes(id, name, arguments),
                0,
            ));
            assert!(result.is_ok(), "{name}: {result:?}");
        }
        assert_eq!(
            state,
            [
                vec![Value::Null, json!({"payload":null}), Value::Null],
                vec![json!({"key":null})],
                vec![json!({})],
            ]
        );
    }

    #[test]
    fn explicit_validator_keeps_its_argument_contract() {
        let mut table = ApiTable::new();
        table
            .register(
                "tabs.query",
                MethodSpec::new(
                    Gate::Open,
                    ContextMask::EXTENSION_PAGES,
                    |state: &mut Vec<Value>, call| {
                        state.extend_from_slice(call.args());
                        call.reply(Ok(Value::Null))
                    },
                )
                .validate(|_| Ok(())),
            )
            .expect("register");
        let mut dispatcher = Dispatcher::new(table);
        let mut state = Vec::new();
        let (_, result) = reply(dispatcher.receive(
            &mut state,
            &permissions(json!({})),
            &background_sender(),
            &call_bytes(1, "tabs.query", r#"["custom"]"#),
            0,
        ));
        assert!(result.is_ok());
        assert_eq!(state, [json!("custom")]);
    }

    #[test]
    fn deferred_calls_settle_once_and_not_after_cancellation() {
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let sender = content_sender("https://example.com/");
        let none = permissions(json!({}));
        let bytes = call_bytes(9, "storage.get", r#"["k"]"#);
        assert!(matches!(
            dispatcher.receive(&mut state, &none, &sender, &bytes, 0),
            Inbound::Pending
        ));
        assert!(matches!(
            dispatcher.receive(&mut state, &none, &sender, &bytes, 0),
            Inbound::Refused(Refusal::DuplicateCallId(_))
        ));
        assert_eq!(dispatcher.limiter().context_calls(&sender.context_key()), 1);
        let responder = state.parked.pop().expect("parked");
        let settled = dispatcher
            .settle(responder, Ok(json!({"k": 1})))
            .expect("settled");
        assert_eq!(settled.context, sender.context_key());
        assert_eq!(dispatcher.pending_calls(), 0);
        assert_eq!(dispatcher.limiter().context_bytes(&sender.context_key()), 0);

        assert!(matches!(
            dispatcher.receive(&mut state, &none, &sender, &bytes, 0),
            Inbound::Pending
        ));
        assert_eq!(dispatcher.cancel_context(&sender.context_key()), 1);
        let late = state.parked.pop().expect("parked");
        assert_eq!(dispatcher.settle(late, Ok(Value::Null)), None);
        assert_eq!(dispatcher.limiter().install_bytes(sender.install()), 0);
    }

    #[test]
    fn a_reused_call_id_cannot_settle_a_newer_call() {
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let sender = content_sender("https://example.com/");
        let none = permissions(json!({}));
        let bytes = call_bytes(9, "storage.get", r#"["k"]"#);
        let _ = dispatcher.receive(&mut state, &none, &sender, &bytes, 0);
        let stale = state.parked.pop().expect("parked");
        assert_eq!(dispatcher.cancel_context(&sender.context_key()), 1);
        let _ = dispatcher.receive(&mut state, &none, &sender, &bytes, 0);
        assert_eq!(dispatcher.settle(stale, Ok(Value::Null)), None);
        assert_eq!(dispatcher.pending_calls(), 1);
    }

    #[test]
    fn framed_extension_pages_cannot_reach_privileged_methods() {
        let page = format!("chrome-extension://{EXTENSION}/frame.html");
        let framed = authority()
            .bind_tab(
                &TabDocument {
                    tab: ItemId::from(5),
                    frame: FrameId::new(3),
                    committed: committed(&page, 2),
                },
                &ClaimedWorld::Page,
            )
            .expect("framed page");
        assert_eq!(framed.kind(), ContextKind::ExtensionFrame);
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let native = permissions(json!({"permissions": ["nativeMessaging"]}));
        let connect = call_bytes(1, "runtime.connectNative", "[\"host\"]");
        let (_, result) = reply(dispatcher.receive(&mut state, &native, &framed, &connect, 0));
        assert_eq!(
            result,
            Err("'runtime.connectNative' is not available in this context.".into())
        );
        let (_, result) =
            reply(dispatcher.receive(&mut state, &native, &background_sender(), &connect, 0));
        assert_eq!(result, Ok("null".into()));
    }

    #[test]
    fn registration_enforces_the_section_six_restrictions() {
        let mut table: ApiTable<State> = ApiTable::new();
        let handler = |_: &mut State, call: Invocation<'_>| call.reply(Ok(Value::Null));
        for name in [
            "runtime.connectNative",
            "runtime.sendNativeMessage",
            "permissions.request",
            "cookies.get",
            "management.getSelf",
            "debugger.attach",
            "privacy.services.passwordSavingEnabled.set",
            "privacy.network.webRTCIPHandlingPolicy.clear",
            "declarativeNetRequest.updateDynamicRules",
            "declarativeNetRequest.updateSessionRules",
            "declarativeNetRequest.updateEnabledRulesets",
            "declarativeNetRequest.setExtensionActionOptions",
        ] {
            assert_eq!(
                table.register(
                    name,
                    MethodSpec::new(Gate::Open, ContextMask::EXTENSION_PAGES, handler)
                ),
                Err(RegistrationError::ForbiddenContexts {
                    name,
                    contexts: ContextMask::of(ContextKind::ExtensionFrame),
                }),
                "{name}"
            );
            assert_eq!(permitted_contexts(name), ContextMask::PRIVILEGED_PAGES);
        }
        assert_eq!(
            permitted_contexts("privacy.services.passwordSavingEnabled.get"),
            ContextMask::ALL.without(ContextKind::ExternalPage)
        );
        assert_eq!(
            permitted_contexts("declarativeNetRequest.getDynamicRules"),
            ContextMask::ALL.without(ContextKind::ExternalPage)
        );
        assert!(permitted_contexts("runtime.sendMessage").contains(ContextKind::ExternalPage));
        assert!(permitted_contexts("runtime.connect").contains(ContextKind::ExternalPage));
        assert_eq!(
            table.register(
                "tabs.query",
                MethodSpec::new(Gate::Open, ContextMask::ALL, handler)
            ),
            Err(RegistrationError::ForbiddenContexts {
                name: "tabs.query",
                contexts: ContextMask::of(ContextKind::ExternalPage),
            })
        );
        assert_eq!(
            table.register(
                "tabs",
                MethodSpec::new(Gate::Open, ContextMask::ALL, handler)
            ),
            Err(RegistrationError::InvalidName("tabs"))
        );
        assert_eq!(
            table.register(
                "tabs.query",
                MethodSpec::new(Gate::Open, ContextMask::NONE, handler)
            ),
            Err(RegistrationError::NoContexts("tabs.query"))
        );
        table
            .register(
                "tabs.query",
                MethodSpec::new(Gate::Open, ContextMask::EXTENSION_PAGES, handler),
            )
            .expect("first");
        assert_eq!(
            table.register(
                "tabs.query",
                MethodSpec::new(Gate::Open, ContextMask::EXTENSION_PAGES, handler)
            ),
            Err(RegistrationError::Duplicate("tabs.query"))
        );
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn oversize_and_overloaded_calls_are_answered_not_fatal() {
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let none = permissions(json!({}));
        let content = content_sender("https://example.com/");
        let big = format!(
            "[\"{}\"]",
            "x".repeat(MAX_MESSAGE_BYTES_IN_WEB_RENDERER as usize)
        );
        let (id, result) = reply(dispatcher.receive(
            &mut state,
            &none,
            &content,
            &call_bytes(77, "storage.get", &big),
            0,
        ));
        assert_eq!(
            (id, result),
            (
                77,
                Err("Message exceeded maximum allowed size of 64MiB.".into())
            )
        );

        let background = background_sender();
        let mut limited = 0;
        for id in 1..=RATE_BURST + 1 {
            if let Inbound::Reply(bytes) = dispatcher.receive(
                &mut state,
                &none,
                &background,
                &call_bytes(id, "runtime.getPlatformInfo", "[]"),
                0,
            ) {
                if reply(Inbound::Reply(bytes)).1 == Err(ApiError::rate_limited().message().into())
                {
                    limited += 1;
                }
            }
        }
        assert_eq!(limited, 1);
        assert!(matches!(
            dispatcher.receive(&mut state, &none, &background, b"\x01", 0),
            Inbound::Refused(Refusal::Decode(DecodeError::Truncated))
        ));
    }

    #[test]
    fn port_messages_hold_budget_until_released() {
        let mut dispatcher = Dispatcher::new(table());
        let mut state = State::default();
        let none = permissions(json!({}));
        let sender = content_sender("https://example.com/");
        let port = PortId::new(5).expect("port");
        let bytes = Envelope::PortMessage {
            port,
            payload: b"{\"opaque\":true}",
        }
        .encode(Direction::FromContext, u64::MAX)
        .expect("encode");
        let Inbound::PortMessage {
            payload,
            reservation,
            ..
        } = dispatcher.receive(&mut state, &none, &sender, &bytes, 0)
        else {
            panic!("expected a port message");
        };
        assert_eq!(payload, b"{\"opaque\":true}");
        assert_eq!(
            dispatcher.limiter().context_bytes(&sender.context_key()),
            bytes.len() as u64
        );
        dispatcher.release(reservation);
        assert_eq!(dispatcher.limiter().context_bytes(&sender.context_key()), 0);

        let ack = Envelope::EventAck {
            seq: EventSeq::new(3).expect("seq"),
        }
        .encode(Direction::FromContext, u64::MAX)
        .expect("encode");
        assert!(matches!(
            dispatcher.receive(&mut state, &none, &sender, &ack, 0),
            Inbound::EventAck { .. }
        ));
    }
}
