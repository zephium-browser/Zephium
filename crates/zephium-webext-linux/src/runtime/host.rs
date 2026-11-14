//! The profile's runtime state behind the bridge: the dispatcher and its
//! services, the background lifecycle machine and what it asks the GTK side
//! to do. Nothing here touches GTK; native work leaves as [`NativeOp`]s and
//! native outcomes come back as method calls.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{json, Value};
use url::Url;
use zephium_core::ids::{ExtensionInstallId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_runtime::alarms::{AlarmEnvironment, AlarmHost, Alarms};
use zephium_webext_runtime::context::{ContextKey, ContextKind, ExtensionId, Sender, ViewKind};
use zephium_webext_runtime::dispatch::{
    encode_for, ApiTable, Dispatcher, Handled, Inbound, Invocation, MethodSpec, RegistrationError,
    Responder,
};
use zephium_webext_runtime::envelope::{ApiName, Direction, Envelope, EventSeq, PortId};
use zephium_webext_runtime::errors::ApiError;
use zephium_webext_runtime::events::{
    should_emit_startup, EventFacts, EventImportance, EventQueue, EventRegistry, EventsHost,
    InstalledReason, ListenerId, ListenerRequest, ListenerSnapshot,
};
use zephium_webext_runtime::i18n::{I18nHost, Messages};
use zephium_webext_runtime::lifecycle::{
    BackgroundState, Blockers, Command, Lifecycle, Observation, StartReason, StopReason,
};
use zephium_webext_runtime::limits::{Clock, Limiter};
use zephium_webext_runtime::permissions::{Gate, InstallPermissions};
use zephium_webext_runtime::ports::{
    no_receiver, ListenerOutcome, MessageId, MessageListener, MessageRequest, MessageRoute,
    MessageService, MessageSettlement, MessageTarget, MessagingHost, OneShotDelivery, PortAction,
    PortService,
};
use zephium_webext_runtime::scripting_registry::{ResourceKind, ScriptRegistry, ScriptingHost};
use zephium_webext_runtime::storage::{AccessLevel, Area, StorageChangeEvent};
use zephium_webext_runtime::ContextMask;

use super::errors::{truncate, ErrorLog, ExtensionError, ExtensionErrorKind};
use super::errors::{MAX_ERROR_MESSAGE_BYTES, MAX_ERROR_SOURCE_BYTES};
use super::storage::{Done, Job, StorageExecutor};
use crate::background::{self, BackgroundScript, Bootstrap, Launch};
use crate::bridge::{
    CloseReason, DeliverError, FrameLifecycle, Outbox, PortTraffic, Ports, Runtime, ViewRef,
};

/// Listener id the binding's unfiltered events stand for: it reports
/// interest once per event, not per callback.
const EVENT_LISTENER: u64 = 1 << 53;
/// Direct (unqueued) event deliveries take sequence numbers from here, far
/// from any install queue's, because each context de-duplicates by number.
const DIRECT_SEQUENCE_BASE: u64 = 1 << 48;
/// How long a message or connection waits for a sleeping background.
const PARKED_TIMEOUT_NANOS: u64 = 300_000_000_000;
const MAX_PARKED_PER_INSTALL: usize = 256;
const MAX_UNHANDLED_NAMES: usize = 256;
const MAX_UNINSTALL_URL_BYTES: usize = 1023;

/// GTK-side work the host asks for, in order.
pub(crate) enum NativeOp {
    /// Publish `launch`, create the hidden view, attach and load it.
    StartBackground {
        install: ExtensionInstallId,
        generation: u64,
        launch: Box<Launch>,
    },
    /// Detach, terminate and drop the view, then report it stopped.
    StopBackground {
        install: ExtensionInstallId,
        generation: u64,
    },
    /// Detach and drop the view without reporting it stopped; `crashed`
    /// when its process is already gone.
    DiscardBackground {
        install: ExtensionInstallId,
        generation: u64,
        crashed: bool,
    },
    /// Create the offscreen document view.
    CreateOffscreen {
        install: ExtensionInstallId,
        url: String,
        clipboard: bool,
    },
    /// The offscreen document loaded and its earlier posts were handled.
    OffscreenReady { install: ExtensionInstallId },
    /// Destroy the offscreen document view.
    CloseOffscreen { install: ExtensionInstallId },
    /// For the engine.
    Notify(Notice),
}

/// What the engine host hears about.
#[derive(Clone, Debug)]
pub enum Notice {
    /// A new error record.
    Error(ExtensionError),
    /// The runtime disabled the background after repeated crashes.
    Disabled(ExtensionInstallId),
    /// `runtime.openOptionsPage()`.
    OpenOptionsPage(ExtensionInstallId),
}

/// How an install becomes active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activation {
    /// First install: `runtime.onInstalled` with reason `install`.
    Installed,
    /// A new version replaced an old one: reason `update`.
    Updated {
        /// The version before.
        previous_version: String,
    },
    /// Enabled again, or loaded at browser start: no `onInstalled`.
    Enabled,
}

/// What the façade knows about an install when it admits it.
pub(crate) struct Admitted {
    pub install: ExtensionInstallId,
    pub extension_id: ExtensionId,
    pub manifest: Manifest,
    pub root: PathBuf,
    pub background: Option<BackgroundScript>,
    pub messages: Messages,
    pub permissions: InstallPermissions,
    pub unpacked: bool,
    pub state_directory: Option<PathBuf>,
    pub listeners: Option<Vec<u8>>,
    pub alarms: Option<Vec<u8>>,
}

struct Live {
    generation: u64,
    sender: Option<Sender>,
    booted: bool,
    activated: bool,
    delivering: Option<EventSeq>,
    direct: HashSet<u64>,
    started_at: u64,
}

enum Parked {
    Send {
        sender: Box<Sender>,
        payload: Vec<u8>,
        responder: Responder,
        deadline: u64,
    },
    Connect {
        port: PortId,
        deadline: u64,
    },
}

enum Offscreen {
    Creating(Responder),
    Open,
}

pub(crate) struct InstallState {
    extension_id: ExtensionId,
    manifest: Value,
    root: PathBuf,
    background: Option<BackgroundScript>,
    messages: Messages,
    catalog: Value,
    granted: Vec<String>,
    unlimited: bool,
    clipboard: bool,
    unpacked: bool,
    queue: EventQueue,
    alarms: Alarms,
    scripts: ScriptRegistry,
    fresh: bool,
    live: Option<Live>,
    parked: Vec<Parked>,
    offscreen: Option<Offscreen>,
    errors: ErrorLog,
    unhandled: BTreeMap<String, u64>,
    state_directory: Option<PathBuf>,
    uninstall_url: Option<String>,
    blockers: Blockers,
    startup_ms: Option<f64>,
}

impl InstallState {
    fn record_unhandled(&mut self, name: &str) {
        if let Some(count) = self.unhandled.get_mut(name) {
            *count += 1;
        } else if self.unhandled.len() < MAX_UNHANDLED_NAMES {
            self.unhandled.insert(name.to_owned(), 1);
        }
    }

    fn bootstrap(&self, kind: ContextKind, ui_language: &str, incognito: bool) -> Value {
        Bootstrap {
            extension_id: &self.extension_id,
            kind,
            manifest: &self.manifest,
            messages: &self.catalog,
            ui_language,
            incognito,
            permissions: &self.granted,
        }
        .to_value(None)
    }
}

/// What handlers leave for the host to finish once the dispatcher returns.
enum Work {
    Storage {
        responder: Responder,
        sender: Box<Sender>,
        area: Area,
        member: &'static str,
        args: Vec<Value>,
        unlimited: bool,
    },
    Send {
        responder: Responder,
        sender: Box<Sender>,
        payload: Vec<u8>,
    },
    Connect {
        responder: Responder,
        sender: Box<Sender>,
        name: String,
    },
    Respond {
        sender: Box<Sender>,
        token: u64,
        outcome: ListenerOutcome,
    },
    Lifecycle {
        sender: Box<Sender>,
        state: String,
        detail: Value,
    },
    WorkerError {
        sender: Box<Sender>,
        detail: Value,
    },
    CreateOffscreen {
        responder: Responder,
        sender: Box<Sender>,
        url: String,
        clipboard: bool,
    },
    CloseOffscreen {
        responder: Responder,
        sender: Box<Sender>,
    },
    OpenOptions {
        install: ExtensionInstallId,
    },
    PersistAlarms(ExtensionInstallId),
    PersistListeners(ExtensionInstallId),
}

/// The services handlers run against.
pub(crate) struct State {
    profile: ProfileId,
    incognito: bool,
    ui_language: String,
    installs: HashMap<ExtensionInstallId, InstallState>,
    registry: EventRegistry,
    work: Vec<Work>,
    now: u64,
    unix_millis: u64,
}

struct At(u64);

impl Clock for At {
    fn now_nanos(&self) -> u64 {
        self.0
    }
}

impl State {
    fn install(&mut self, sender: &Sender) -> Result<&mut InstallState, ApiError> {
        if sender.profile() != self.profile || sender.incognito() != self.incognito {
            return Err(ApiError::context_invalidated());
        }
        self.installs
            .get_mut(&sender.install())
            .filter(|install| &install.extension_id == sender.extension_id())
            .ok_or_else(ApiError::context_invalidated)
    }

    fn defer(
        &mut self,
        invocation: Invocation<'_>,
        work: impl FnOnce(Responder) -> Work,
    ) -> Handled {
        let (handled, responder) = invocation.defer();
        self.work.push(work(responder));
        handled
    }
}

impl AlarmHost for State {
    fn alarm_call(
        &mut self,
        sender: &Sender,
        method: &str,
        args: &[Value],
    ) -> Result<Value, ApiError> {
        let now = self.now;
        let unix_millis = self.unix_millis;
        let install = self.install(sender)?;
        let environment = AlarmEnvironment::now(&At(now), unix_millis, install.unpacked);
        let result = install.alarms.handle(method, args, environment);
        if result.is_ok() && matches!(method, "alarms.create" | "alarms.clear" | "alarms.clearAll")
        {
            self.work.push(Work::PersistAlarms(sender.install()));
        }
        result
    }
}

impl EventsHost for State {
    fn listener_request(
        &mut self,
        request: ListenerRequest,
        invocation: Invocation<'_>,
    ) -> Handled {
        let sender = invocation.sender().clone();
        let result = match request {
            ListenerRequest::Add { event, id, filter } => self
                .registry
                .add(&sender, id, event, filter.as_ref())
                .map(|_| Value::Null),
            ListenerRequest::Remove { event, id } => {
                self.registry.remove(&sender, event, id);
                Ok(Value::Null)
            }
            ListenerRequest::Has { event, id } => {
                Ok(Value::Bool(self.registry.has_listener(&sender, id, event)))
            }
            ListenerRequest::HasAny { event } => {
                Ok(Value::Bool(self.registry.has_listeners(&sender, event)))
            }
            ListenerRequest::CompleteRegistration => self
                .registry
                .finish_registration(&sender)
                .map(|()| Value::Null),
        };
        invocation.reply(result)
    }
}

impl MessagingHost for State {
    fn message_request(&mut self, request: MessageRequest, invocation: Invocation<'_>) -> Handled {
        let sender = Box::new(invocation.sender().clone());
        let own = |target: &MessageTarget| {
            matches!(target, MessageTarget::Runtime { extension_id }
                if extension_id.as_ref().is_none_or(|id| id == sender.extension_id()))
        };
        match request {
            MessageRequest::Send { target, payload } if own(&target) => {
                self.defer(invocation, |responder| Work::Send {
                    responder,
                    sender,
                    payload,
                })
            }
            MessageRequest::Connect { target, name } if own(&target) => {
                self.defer(invocation, |responder| Work::Connect {
                    responder,
                    sender,
                    name,
                })
            }
            MessageRequest::Send { .. } | MessageRequest::Connect { .. } => {
                if let Ok(install) = self.install(&sender) {
                    install.record_unhandled(invocation.name());
                }
                invocation.reply(Err(no_receiver()))
            }
            MessageRequest::ConnectNative { .. } | MessageRequest::SendNative { .. } => {
                if let Ok(install) = self.install(&sender) {
                    install.record_unhandled(invocation.name());
                }
                invocation.reply(Err(ApiError::other(
                    "Specified native messaging host not found.",
                )))
            }
        }
    }
}

impl I18nHost for State {
    fn messages(&self, sender: &Sender) -> Result<&Messages, ApiError> {
        self.installs
            .get(&sender.install())
            .filter(|install| &install.extension_id == sender.extension_id())
            .map(|install| &install.messages)
            .ok_or_else(ApiError::context_invalidated)
    }
}

impl ScriptingHost for State {
    fn script_registry(&self, sender: &Sender) -> Result<&ScriptRegistry, ApiError> {
        self.installs
            .get(&sender.install())
            .filter(|install| &install.extension_id == sender.extension_id())
            .map(|install| &install.scripts)
            .ok_or_else(ApiError::context_invalidated)
    }

    fn validate_script_resource(
        &self,
        sender: &Sender,
        path: &str,
        _: ResourceKind,
    ) -> Result<(), ApiError> {
        let install = self
            .installs
            .get(&sender.install())
            .ok_or_else(ApiError::context_invalidated)?;
        let normalized = zephium_webext::manifest::normalize_resource(path)
            .filter(|path| !path.starts_with('_'))
            .ok_or_else(|| ApiError::other(format!("Could not load file: '{path}'.")))?;
        let file = install.root.join(&normalized);
        if std::fs::symlink_metadata(file).is_ok_and(|meta| meta.is_file()) {
            Ok(())
        } else {
            Err(ApiError::other(format!("Could not load file: '{path}'.")))
        }
    }

    fn commit_script_registry(
        &mut self,
        sender: &Sender,
        candidate: ScriptRegistry,
    ) -> Result<(), ApiError> {
        self.install(sender)?.scripts = candidate;
        Ok(())
    }
}

/// Ports, one-time messages and the contexts that have spoken.
pub(crate) struct Router {
    ports: PortService,
    messages: MessageService,
    contexts: HashMap<ContextKey, Sender>,
    acks: Vec<(Sender, EventSeq)>,
    now: u64,
}

fn frame(kind: ContextKind, envelope: &Envelope<'_>) -> Option<Vec<u8>> {
    encode_for(kind, envelope).ok()
}

impl Router {
    fn port_actions(&mut self, actions: Vec<PortAction>, limiter: &mut Limiter, out: &mut Outbox) {
        for action in actions {
            match action {
                PortAction::Connect {
                    port,
                    recipient,
                    sender,
                    name,
                    event,
                } => {
                    let Some(kind) = self.contexts.get(&recipient).map(Sender::kind) else {
                        continue;
                    };
                    let args = json!([port.get().to_string(), name, sender_json(&sender)]);
                    if let Some(frame) = event_frame(kind, direct_sequence(), event, &args) {
                        out.deliver(recipient, frame);
                    }
                }
                PortAction::Message {
                    delivery,
                    port,
                    recipient,
                    payload,
                } => {
                    let Some(receiver) = self.contexts.get(&recipient).cloned() else {
                        continue;
                    };
                    if let Some(frame) = frame(
                        receiver.kind(),
                        &Envelope::PortMessage {
                            port,
                            payload: &payload,
                        },
                    ) {
                        out.deliver(recipient, frame);
                    }
                    self.ports.acknowledge(&receiver, delivery, limiter);
                }
                PortAction::Disconnect {
                    port,
                    recipient,
                    error,
                } => {
                    let Some(kind) = self.contexts.get(&recipient).map(Sender::kind) else {
                        continue;
                    };
                    let message = error.as_ref().map(ApiError::message);
                    if let Some(frame) = frame(
                        kind,
                        &Envelope::PortDisconnect {
                            port,
                            error: message,
                        },
                    ) {
                        out.deliver(recipient, frame);
                    }
                }
                PortAction::ConnectNative { .. }
                | PortAction::NativeMessage { .. }
                | PortAction::DisconnectNative { .. } => {}
            }
        }
    }
}

impl Ports<State> for Router {
    fn traffic(
        &mut self,
        dispatcher: &mut Dispatcher<State>,
        traffic: PortTraffic<'_>,
        out: &mut Outbox,
    ) {
        let clock = At(self.now);
        match traffic {
            PortTraffic::Message {
                sender,
                port,
                payload,
                reservation,
            } => {
                let limiter = dispatcher.limiter_mut();
                match self
                    .ports
                    .post_admitted(sender, port, payload, reservation, limiter, &clock)
                {
                    Ok(actions) => self.port_actions(actions, limiter, out),
                    Err(error) => {
                        if let Some(frame) = frame(
                            sender.kind(),
                            &Envelope::PortDisconnect {
                                port,
                                error: Some(error.message()),
                            },
                        ) {
                            out.deliver(sender.context_key(), frame);
                        }
                    }
                }
            }
            PortTraffic::Disconnect { sender, port } => {
                let limiter = dispatcher.limiter_mut();
                let actions = self.ports.disconnect(sender, port, limiter);
                self.port_actions(actions, limiter, out);
            }
            PortTraffic::EventAck { sender, seq } => self.acks.push((sender.clone(), seq)),
        }
    }

    fn context_closed(&mut self, context: &ContextKey, out: &mut Outbox) {
        let _ = (context, out);
    }
}

thread_local! {
    static NEXT_DIRECT: std::cell::Cell<u64> = const { std::cell::Cell::new(DIRECT_SEQUENCE_BASE) };
}

fn direct_sequence() -> u64 {
    NEXT_DIRECT.with(|next| {
        let value = next.get();
        next.set(value.wrapping_add(1).max(DIRECT_SEQUENCE_BASE));
        value
    })
}

fn event_frame(kind: ContextKind, seq: u64, name: &str, args: &Value) -> Option<Vec<u8>> {
    let args = serde_json::to_vec(args).ok()?;
    frame(
        kind,
        &Envelope::Event {
            seq: EventSeq::new(seq)?,
            name: ApiName::parse(name)?,
            args: &args,
        },
    )
}

fn sender_json(sender: &Sender) -> Value {
    let mut value = json!({
        "id": sender.extension_id().as_str(),
        "url": sender.url().as_str(),
        "origin": sender.origin().serialize(),
    });
    if let (Some(frame), Some(object)) = (sender.frame(), value.as_object_mut()) {
        object.insert("frameId".into(), json!(frame.get()));
    }
    value
}

/// Exactly what one start of the host needs.
pub(crate) struct HostConfig {
    pub profile: ProfileId,
    pub incognito: bool,
    pub ui_language: String,
    pub clock: Rc<dyn Fn() -> u64>,
    pub unix_millis: Rc<dyn Fn() -> u64>,
    pub scale: u64,
}

/// The runtime behind one profile's bridge.
pub struct RuntimeHost {
    dispatcher: Dispatcher<State>,
    state: State,
    router: Router,
    permissions: HashMap<ExtensionInstallId, InstallPermissions>,
    lifecycle: Lifecycle,
    clock: Rc<dyn Fn() -> u64>,
    unix_millis: Rc<dyn Fn() -> u64>,
    storage: StorageExecutor,
    tickets: HashMap<u64, (Responder, Box<Sender>)>,
    next_ticket: u64,
    sends: HashMap<u64, (MessageId, Responder)>,
    awaiting: HashMap<u64, ContextKey>,
    ops: VecDeque<NativeOp>,
    kick: Option<Rc<dyn Fn()>>,
    armed: Option<u64>,
    scale: u64,
    shutting_down: bool,
}

const STORAGE_METHODS: [&str; 28] = [
    "storage.local.get",
    "storage.local.set",
    "storage.local.remove",
    "storage.local.clear",
    "storage.local.getBytesInUse",
    "storage.local.getKeys",
    "storage.local.setAccessLevel",
    "storage.sync.get",
    "storage.sync.set",
    "storage.sync.remove",
    "storage.sync.clear",
    "storage.sync.getBytesInUse",
    "storage.sync.getKeys",
    "storage.sync.setAccessLevel",
    "storage.session.get",
    "storage.session.set",
    "storage.session.remove",
    "storage.session.clear",
    "storage.session.getBytesInUse",
    "storage.session.getKeys",
    "storage.session.setAccessLevel",
    "storage.managed.get",
    "storage.managed.set",
    "storage.managed.remove",
    "storage.managed.clear",
    "storage.managed.getBytesInUse",
    "storage.managed.getKeys",
    "storage.managed.setAccessLevel",
];

fn area(name: &str) -> Option<Area> {
    match name.split('.').nth(1)? {
        "local" => Some(Area::Local),
        "sync" => Some(Area::Sync),
        "session" => Some(Area::Session),
        "managed" => Some(Area::Managed),
        _ => None,
    }
}

fn listener_id(value: &Value) -> Result<ListenerId, ApiError> {
    match value {
        Value::Null => ListenerId::new(EVENT_LISTENER).ok_or_else(ApiError::malformed_arguments),
        value => value
            .as_u64()
            .filter(|id| *id < EVENT_LISTENER)
            .and_then(ListenerId::new)
            .ok_or_else(ApiError::malformed_arguments),
    }
}

fn event_name(value: &Value) -> Result<&str, ApiError> {
    value
        .as_str()
        .filter(|name| ApiName::parse(name).is_some())
        .ok_or_else(ApiError::malformed_arguments)
}

fn internal(
    contexts: ContextMask,
    handler: fn(&mut State, Invocation<'_>) -> Handled,
) -> MethodSpec<State> {
    MethodSpec::new(Gate::Open, contexts, handler).validate(|args| {
        if args.len() > 8 {
            Err(ApiError::malformed_arguments())
        } else {
            Ok(())
        }
    })
}

fn add_listener(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let args = invocation.args();
    let result = (|| {
        let event = event_name(args.first().unwrap_or(&Value::Null))?;
        let id = listener_id(args.get(1).unwrap_or(&Value::Null))?;
        let filter = args.get(2).filter(|filter| !filter.is_null());
        let sender = invocation.sender();
        match state.registry.add(sender, id, event, filter) {
            Ok(_) => Ok(Value::Null),
            Err(error) => {
                if let Ok(install) = state.install(sender) {
                    install.record_unhandled(event);
                }
                Err(error)
            }
        }
    })();
    invocation.reply(result)
}

fn remove_listener(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let args = invocation.args();
    let result = event_name(args.first().unwrap_or(&Value::Null)).and_then(|event| {
        let id = listener_id(args.get(1).unwrap_or(&Value::Null))?;
        state.registry.remove(invocation.sender(), event, id);
        Ok(Value::Null)
    });
    invocation.reply(result)
}

fn respond(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let args = invocation.args();
    let token = args
        .first()
        .and_then(Value::as_str)
        .and_then(|token| token.parse::<u64>().ok());
    let outcome = match args.get(1) {
        None | Some(Value::Null) => Some(ListenerOutcome::NoResponse),
        Some(Value::Object(object)) => match (object.get("response"), object.get("error")) {
            (Some(Value::String(text)), None) => serde_json::to_vec(&Value::String(text.clone()))
                .ok()
                .map(ListenerOutcome::Response),
            (Some(Value::Null), None) => Some(ListenerOutcome::Response(b"null".to_vec())),
            (None, Some(Value::String(error))) => Some(ListenerOutcome::Rejected(ApiError::other(
                truncate(error, MAX_ERROR_MESSAGE_BYTES),
            ))),
            _ => None,
        },
        Some(_) => None,
    };
    let (Some(token), Some(outcome)) = (token, outcome) else {
        return invocation.reply(Err(ApiError::malformed_arguments()));
    };
    state.work.push(Work::Respond {
        sender: Box::new(invocation.sender().clone()),
        token,
        outcome,
    });
    invocation.reply(Ok(Value::Null))
}

fn event_result(_: &mut State, invocation: Invocation<'_>) -> Handled {
    invocation.reply(Ok(Value::Null))
}

fn rules(_: &mut State, invocation: Invocation<'_>) -> Handled {
    invocation.reply(Err(ApiError::other("This event does not support rules.")))
}

fn worker_lifecycle(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let args = invocation.args();
    let Some(stage) = args.first().and_then(Value::as_str) else {
        return invocation.reply(Err(ApiError::malformed_arguments()));
    };
    state.work.push(Work::Lifecycle {
        sender: Box::new(invocation.sender().clone()),
        state: truncate(stage, 32),
        detail: args.get(1).cloned().unwrap_or(Value::Null),
    });
    invocation.reply(Ok(Value::Null))
}

fn worker_error(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let detail = invocation.args().first().cloned().unwrap_or(Value::Null);
    state.work.push(Work::WorkerError {
        sender: Box::new(invocation.sender().clone()),
        detail,
    });
    invocation.reply(Ok(Value::Null))
}

fn storage_call(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let Some(area) = area(invocation.name()) else {
        return invocation.reply(Err(ApiError::unknown_method()));
    };
    let member = invocation.name().rsplit('.').next().unwrap_or("");
    let unlimited = invocation.permissions().has_api("unlimitedStorage");
    let sender = Box::new(invocation.sender().clone());
    if state.install(&sender).is_err() {
        return invocation.reply(Err(ApiError::context_invalidated()));
    }
    let mut invocation = invocation;
    let args = invocation.take_args();
    state.defer(invocation, |responder| Work::Storage {
        responder,
        sender,
        area,
        member,
        args,
        unlimited,
    })
}

fn platform_info(_: &mut State, invocation: Invocation<'_>) -> Handled {
    let (arch, nacl) = match std::env::consts::ARCH {
        "x86_64" => ("x86-64", "x86-64"),
        "x86" => ("x86-32", "x86-32"),
        "aarch64" => ("arm64", "arm"),
        "arm" => ("arm", "arm"),
        "mips64" => ("mips64", "mips64"),
        _ => ("x86-64", "x86-64"),
    };
    invocation.reply(Ok(json!({"os": "linux", "arch": arch, "nacl_arch": nacl})))
}

fn set_uninstall_url(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let url = invocation
        .args()
        .first()
        .and_then(Value::as_str)
        .unwrap_or("");
    let result = (|| {
        if url.len() > MAX_UNINSTALL_URL_BYTES {
            return Err(ApiError::other("URL is too long."));
        }
        if !url.is_empty()
            && !Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
        {
            return Err(ApiError::other("Invalid URL: \"".to_owned() + url + "\"."));
        }
        let install = state.install(invocation.sender())?;
        install.uninstall_url = (!url.is_empty()).then(|| url.to_owned());
        Ok(Value::Null)
    })();
    invocation.reply(result)
}

fn open_options_page(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let result = state.install(invocation.sender()).and_then(|install| {
        let has_options = install.manifest["options_ui"]["page"].is_string()
            || install.manifest["options_page"].is_string();
        if has_options {
            Ok(())
        } else {
            Err(ApiError::other("Could not create an options page."))
        }
    });
    if result.is_ok() {
        state.work.push(Work::OpenOptions {
            install: invocation.sender().install(),
        });
    }
    invocation.reply(result.map(|()| Value::Null))
}

fn create_offscreen(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let parameters = invocation.args().first().cloned().unwrap_or(Value::Null);
    let sender = Box::new(invocation.sender().clone());
    let checked = state.install(&sender).and_then(|install| {
        if install.offscreen.is_some() {
            return Err(ApiError::other(
                "Only a single offscreen document may be created.",
            ));
        }
        crate::offscreen::check(&parameters, &install.extension_id, install.clipboard)
            .map(|request| (request.url, request.clipboard))
    });
    match checked {
        Ok((url, clipboard)) => state.defer(invocation, |responder| Work::CreateOffscreen {
            responder,
            sender,
            url,
            clipboard,
        }),
        Err(error) => invocation.reply(Err(error)),
    }
}

fn close_offscreen(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let sender = Box::new(invocation.sender().clone());
    match state.install(&sender) {
        Ok(install) if matches!(install.offscreen, Some(Offscreen::Open)) => {
            state.defer(invocation, |responder| Work::CloseOffscreen {
                responder,
                sender,
            })
        }
        Ok(_) => invocation.reply(Err(ApiError::other("No current offscreen document."))),
        Err(error) => invocation.reply(Err(error)),
    }
}

fn has_offscreen(state: &mut State, invocation: Invocation<'_>) -> Handled {
    let result = state
        .install(invocation.sender())
        .map(|install| Value::Bool(matches!(install.offscreen, Some(Offscreen::Open))));
    invocation.reply(result)
}

fn table() -> Result<ApiTable<State>, RegistrationError> {
    use zephium_webext_runtime::{alarms, events, i18n, ports, scripting_registry};
    let mut table = ApiTable::new();
    alarms::register(&mut table)?;
    events::register(&mut table)?;
    ports::register(&mut table)?;
    i18n::register(&mut table)?;
    scripting_registry::register(&mut table)?;
    for name in STORAGE_METHODS {
        table.register_generated(name, storage_call)?;
    }
    table.register_generated("runtime.getPlatformInfo", platform_info)?;
    table.register_generated("runtime.setUninstallURL", set_uninstall_url)?;
    table.register_generated("runtime.openOptionsPage", open_options_page)?;
    table.register_generated("offscreen.createDocument", create_offscreen)?;
    table.register_generated("offscreen.closeDocument", close_offscreen)?;
    table.register_generated("offscreen.hasDocument", has_offscreen)?;
    let contexts = ContextMask::ALL.without(ContextKind::ExternalPage);
    let background = ContextMask::of(ContextKind::Background);
    table.register("zephium.addListener", internal(contexts, add_listener))?;
    table.register(
        "zephium.removeListener",
        internal(contexts, remove_listener),
    )?;
    table.register("zephium.respond", internal(contexts, respond))?;
    table.register("zephium.eventResult", internal(contexts, event_result))?;
    for name in [
        "zephium.addRules",
        "zephium.getRules",
        "zephium.removeRules",
    ] {
        table.register(name, internal(contexts, rules))?;
    }
    table.register(
        "zephium.workerLifecycle",
        internal(background, worker_lifecycle),
    )?;
    table.register("zephium.workerError", internal(background, worker_error))?;
    Ok(table)
}

/// Builds one message's text for the binding's synchronous lookup: named
/// placeholders applied, positional ones left for call time.
fn binding_catalog(messages: &Messages) -> Value {
    let Value::Object(catalog) = messages.catalog() else {
        return json!({});
    };
    let mut out = serde_json::Map::new();
    for (name, entry) in catalog {
        let Some(text) = entry["message"].as_str() else {
            continue;
        };
        let placeholders = entry["placeholders"].as_object();
        let mut result = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find('$') {
            result.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let close = after.find('$');
            let replaced = close.and_then(|end| {
                let key = &after[..end];
                if key.is_empty()
                    || !key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'@')
                {
                    return None;
                }
                let lower = key.to_ascii_lowercase();
                placeholders?
                    .iter()
                    .find(|(name, _)| name.to_ascii_lowercase() == lower)
                    .and_then(|(_, value)| value["content"].as_str())
                    .map(|content| (content, end))
            });
            match replaced {
                Some((content, end)) => {
                    result.push_str(content);
                    rest = &after[end + 1..];
                }
                None => {
                    result.push('$');
                    rest = after;
                }
            }
        }
        result.push_str(rest);
        out.insert(name, Value::String(result));
    }
    Value::Object(out)
}

/// Facts about one install for callers and tests.
#[derive(Clone, Debug, PartialEq)]
pub struct BackgroundStatus {
    /// The lifecycle state.
    pub state: BackgroundState,
    /// The process generation.
    pub generation: u64,
    /// Whether the worker finished its first task.
    pub ready: bool,
    /// Milliseconds from start to listeners registered, last start.
    pub startup_ms: Option<f64>,
}

impl RuntimeHost {
    pub(crate) fn new(config: HostConfig) -> Result<Self, String> {
        let table = table().map_err(|error| format!("{error:?}"))?;
        let storage = StorageExecutor::start().map_err(|error| error.to_string())?;
        let now = (config.clock)();
        Ok(Self {
            dispatcher: Dispatcher::new(table),
            state: State {
                profile: config.profile,
                incognito: config.incognito,
                ui_language: config.ui_language,
                installs: HashMap::new(),
                registry: EventRegistry::new(),
                work: Vec::new(),
                now,
                unix_millis: (config.unix_millis)(),
            },
            router: Router {
                ports: PortService::new(),
                messages: MessageService::new(),
                contexts: HashMap::new(),
                acks: Vec::new(),
                now,
            },
            permissions: HashMap::new(),
            lifecycle: Lifecycle::new(config.profile),
            clock: config.clock,
            unix_millis: config.unix_millis,
            storage,
            tickets: HashMap::new(),
            next_ticket: 0,
            sends: HashMap::new(),
            awaiting: HashMap::new(),
            ops: VecDeque::new(),
            kick: None,
            armed: None,
            scale: config.scale.max(1),
            shutting_down: false,
        })
    }

    pub(crate) fn set_kick(&mut self, kick: Rc<dyn Fn()>) {
        self.kick = Some(kick);
    }

    pub(crate) fn storage_fd(&self) -> std::os::fd::RawFd {
        self.storage.fd()
    }

    pub(crate) fn storage_take(&self) -> Vec<Done> {
        self.storage.take()
    }

    pub(crate) fn take_ops(&mut self) -> Vec<NativeOp> {
        self.ops.drain(..).collect()
    }

    fn op(&mut self, op: NativeOp) {
        self.ops.push_back(op);
        self.kick();
    }

    fn kick(&self) {
        if let Some(kick) = &self.kick {
            kick();
        }
    }

    /// The façade armed its timer for `deadline`.
    pub(crate) fn set_armed(&mut self, deadline: Option<u64>) {
        self.armed = deadline;
    }

    /// Asks the façade to re-arm when the next deadline moved earlier.
    fn check_timer(&self) {
        let next = self.next_deadline();
        let earlier = match (next, self.armed) {
            (Some(next), Some(armed)) => next < armed,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if earlier {
            self.kick();
        }
    }

    fn tick_clock(&mut self) -> u64 {
        let now = (self.clock)();
        self.state.now = now;
        self.state.unix_millis = (self.unix_millis)();
        self.router.now = now;
        now
    }

    /// Scaled monotonic time of the next timer the host needs.
    pub(crate) fn next_deadline(&self) -> Option<u64> {
        let alarms = self
            .state
            .installs
            .values()
            .filter_map(|install| install.alarms.next_wake());
        let parked = self.state.installs.values().flat_map(|install| {
            install.parked.iter().map(|parked| match parked {
                Parked::Send { deadline, .. } | Parked::Connect { deadline, .. } => *deadline,
            })
        });
        [
            self.lifecycle.next_wake(),
            self.router.messages.next_deadline(),
        ]
        .into_iter()
        .flatten()
        .chain(alarms)
        .chain(parked)
        .min()
    }

    pub(crate) fn now(&self) -> u64 {
        (self.clock)()
    }

    // ---- installs -------------------------------------------------------

    pub(crate) fn admit(&mut self, admitted: Admitted, activation: &Activation, out: &mut Outbox) {
        let now = self.tick_clock();
        let install = admitted.install;
        let granted = admitted.permissions.snapshot().permissions;
        let unlimited = admitted.permissions.has_api("unlimitedStorage");
        let clipboard = admitted.permissions.has_api("clipboardRead")
            || admitted.permissions.has_api("clipboardWrite");
        let scripts = ScriptRegistry::from_manifest(&admitted.manifest, |path, _| {
            zephium_webext::manifest::normalize_resource(path)
                .filter(|path| admitted.root.join(path).is_file())
                .map(|_| ())
                .ok_or_else(|| ApiError::other(format!("Could not load file: '{path}'.")))
        })
        .unwrap_or_default();
        let mut state = InstallState {
            extension_id: admitted.extension_id,
            catalog: binding_catalog(&admitted.messages),
            manifest: admitted.manifest.raw().clone(),
            root: admitted.root,
            background: admitted.background,
            messages: admitted.messages,
            granted,
            unlimited,
            clipboard,
            unpacked: admitted.unpacked,
            queue: EventQueue::new(install),
            alarms: Alarms::new(),
            scripts,
            fresh: matches!(
                activation,
                Activation::Installed | Activation::Updated { .. }
            ),
            live: None,
            parked: Vec::new(),
            offscreen: None,
            errors: ErrorLog::new(),
            unhandled: BTreeMap::new(),
            state_directory: admitted.state_directory.clone(),
            uninstall_url: None,
            blockers: Blockers::default(),
            startup_ms: None,
        };
        let fresh_install = matches!(activation, Activation::Installed);
        if !fresh_install {
            if let Some(bytes) = admitted.listeners.as_deref() {
                if let Ok(snapshot) = ListenerSnapshot::decode(bytes) {
                    let _ = self.state.registry.restore(install, snapshot);
                }
            }
            if let (Some(bytes), false) = (
                admitted.alarms.as_deref(),
                matches!(activation, Activation::Updated { .. }),
            ) {
                let environment =
                    AlarmEnvironment::now(&At(now), self.state.unix_millis, state.unpacked);
                let _ = state.alarms.restore_bytes(bytes, environment);
            }
        }
        if matches!(activation, Activation::Updated { .. }) {
            self.persist_path(&state, "alarms.json", None);
        }
        self.permissions.insert(install, admitted.permissions);
        self.storage.submit(Job::Open {
            install,
            profile: self.state.profile,
            incognito: self.state.incognito,
            directory: admitted.state_directory,
        });
        self.lifecycle.register(install);
        let reason = match activation {
            Activation::Installed => Some((InstalledReason::Install, StartReason::Install)),
            Activation::Updated { previous_version } => Some((
                InstalledReason::Update {
                    previous_version: previous_version.clone(),
                },
                StartReason::Update,
            )),
            Activation::Enabled => None,
        };
        let has_background = state.background.is_some();
        self.state.installs.insert(install, state);
        if let (Some((installed, start)), true) = (reason, has_background) {
            self.enqueue(
                install,
                "runtime.onInstalled",
                &installed.arguments(),
                EventImportance::Critical,
            );
            self.observe(
                Observation::Wake {
                    install,
                    reason: start,
                },
                out,
            );
        }
        self.settle_blockers(out);
    }

    fn persist_path(&self, install: &InstallState, name: &str, bytes: Option<Vec<u8>>) {
        let Some(directory) = &install.state_directory else {
            return;
        };
        let path = directory.join(name);
        self.storage.submit(match bytes {
            Some(bytes) => Job::Write { path, bytes },
            None => Job::Remove { path },
        });
    }

    /// Stops the background for disable, removal or update. The façade
    /// executes the stop, then calls [`Self::forget`].
    pub(crate) fn stop(&mut self, install: ExtensionInstallId, out: &mut Outbox) {
        self.tick_clock();
        if !self.state.installs.contains_key(&install) {
            return;
        }
        self.fail_parked(install, &ApiError::context_invalidated(), out);
        if self.lifecycle.state(install).is_some() {
            self.observe(Observation::Disable { install }, out);
        }
        if self
            .state
            .installs
            .get(&install)
            .is_some_and(|state| state.offscreen.is_some())
        {
            self.close_offscreen_now(install, out);
        }
    }

    /// Forgets a stopped install. `erase` also deletes its data.
    pub(crate) fn forget(&mut self, install: ExtensionInstallId, erase: bool, out: &mut Outbox) {
        let Some(state) = self.state.installs.remove(&install) else {
            return;
        };
        let contexts: Vec<ContextKey> = self
            .router
            .contexts
            .keys()
            .filter(|key| key.install() == install)
            .cloned()
            .collect();
        for context in contexts {
            self.close_context(&context, out);
        }
        let limiter = self.dispatcher.limiter_mut();
        let actions = self.router.ports.erase(install, limiter);
        self.router.port_actions(actions, limiter, out);
        self.state.registry.erase(install);
        self.lifecycle.forget(install);
        self.permissions.remove(&install);
        if erase {
            self.storage.submit(Job::Destroy {
                install,
                directory: state.state_directory,
            });
        } else {
            self.storage.submit(Job::Close { install });
        }
    }

    /// `runtime.onStartup` for installs that listen to it.
    pub(crate) fn browser_started(&mut self, out: &mut Outbox) {
        self.tick_clock();
        let installs: Vec<ExtensionInstallId> = self
            .state
            .installs
            .iter()
            .filter(|(id, install)| {
                install.background.is_some()
                    && should_emit_startup(&self.state.registry, **id, self.state.incognito)
            })
            .map(|(id, _)| *id)
            .collect();
        for install in installs {
            self.enqueue(
                install,
                "runtime.onStartup",
                &json!([]),
                EventImportance::Critical,
            );
            self.observe(
                Observation::Wake {
                    install,
                    reason: StartReason::BrowserStartup,
                },
                out,
            );
        }
    }

    pub(crate) fn memory_pressure(&mut self, reclaim: usize, out: &mut Outbox) {
        self.tick_clock();
        self.observe(Observation::MemoryPressure { reclaim }, out);
    }

    pub(crate) fn shutdown(&mut self, out: &mut Outbox) {
        self.tick_clock();
        self.shutting_down = true;
        let installs: Vec<_> = self.state.installs.keys().copied().collect();
        for install in installs {
            self.fail_parked(install, &ApiError::context_invalidated(), out);
            if self
                .state
                .installs
                .get(&install)
                .is_some_and(|state| state.offscreen.is_some())
            {
                self.close_offscreen_now(install, out);
            }
        }
        self.observe(Observation::Shutdown, out);
    }

    pub(crate) fn stop_storage(&mut self) {
        self.storage.stop();
    }

    // ---- timers and native outcomes -----------------------------------

    pub(crate) fn tick(&mut self, out: &mut Outbox) {
        let now = self.tick_clock();
        self.observe(Observation::Tick, out);
        let unix = self.state.unix_millis;
        let installs: Vec<ExtensionInstallId> = self.state.installs.keys().copied().collect();
        for install in installs {
            let due = {
                let Some(state) = self.state.installs.get_mut(&install) else {
                    continue;
                };
                let environment = AlarmEnvironment::now(&At(now), unix, state.unpacked);
                state.alarms.take_due(environment)
            };
            if !due.is_empty() {
                self.persist_alarms(install);
            }
            for alarm in due {
                let Ok(alarm) = serde_json::to_value(&alarm) else {
                    continue;
                };
                self.emit(
                    install,
                    "alarms.onAlarm",
                    &json!([alarm]),
                    StartReason::Alarm,
                    out,
                );
            }
            self.expire_parked(install, now, out);
        }
        let clock = At(now);
        let settlements = self.router.messages.expire(&clock);
        for settlement in settlements {
            self.finish_send(settlement, out);
        }
        self.settle_blockers(out);
    }

    pub(crate) fn background_crashed(
        &mut self,
        install: ExtensionInstallId,
        generation: u64,
        out: &mut Outbox,
    ) {
        self.tick_clock();
        let live = self.live_generation(install) == Some(generation);
        if !live {
            return;
        }
        self.record_error(
            install,
            ExtensionErrorKind::Crash,
            ContextKind::Background,
            "The background's web process ended unexpectedly.".into(),
            Value::Null,
        );
        self.observe(
            Observation::Crashed {
                install,
                generation,
            },
            out,
        );
        self.op(NativeOp::DiscardBackground {
            install,
            generation,
            crashed: true,
        });
    }

    pub(crate) fn background_failed(
        &mut self,
        install: ExtensionInstallId,
        generation: u64,
        message: &str,
        out: &mut Outbox,
    ) {
        self.tick_clock();
        if self.live_generation(install) != Some(generation) {
            return;
        }
        self.record_error(
            install,
            ExtensionErrorKind::Startup,
            ContextKind::Background,
            message.to_owned(),
            Value::Null,
        );
        self.observe(
            Observation::Crashed {
                install,
                generation,
            },
            out,
        );
        self.op(NativeOp::DiscardBackground {
            install,
            generation,
            crashed: false,
        });
    }

    pub(crate) fn background_stopped(
        &mut self,
        install: ExtensionInstallId,
        generation: u64,
        out: &mut Outbox,
    ) {
        self.tick_clock();
        self.observe(
            Observation::Terminated {
                install,
                generation,
            },
            out,
        );
    }

    fn live_generation(&self, install: ExtensionInstallId) -> Option<u64> {
        self.state
            .installs
            .get(&install)
            .and_then(|state| state.live.as_ref())
            .map(|live| live.generation)
    }

    pub(crate) fn offscreen_loaded(
        &mut self,
        install: ExtensionInstallId,
        loaded: bool,
        out: &mut Outbox,
    ) {
        self.tick_clock();
        let Some(state) = self.state.installs.get_mut(&install) else {
            return;
        };
        let Some(Offscreen::Creating(_)) = state.offscreen else {
            return;
        };
        let Some(Offscreen::Creating(responder)) = state.offscreen.take() else {
            return;
        };
        if loaded {
            state.offscreen = Some(Offscreen::Open);
            self.settle(responder, Ok(Value::Null), out);
        } else {
            self.settle(responder, Err(ApiError::other("Page failed to load.")), out);
            self.op(NativeOp::CloseOffscreen { install });
        }
        self.settle_blockers(out);
    }

    /// The offscreen document's view went away on its own.
    pub(crate) fn offscreen_gone(&mut self, install: ExtensionInstallId, out: &mut Outbox) {
        if let Some(state) = self.state.installs.get_mut(&install) {
            if let Some(Offscreen::Creating(responder)) = state.offscreen.take() {
                self.settle(responder, Err(ApiError::other("Page failed to load.")), out);
            }
        }
        self.settle_blockers(out);
    }

    fn close_offscreen_now(&mut self, install: ExtensionInstallId, out: &mut Outbox) {
        if let Some(state) = self.state.installs.get_mut(&install) {
            if let Some(Offscreen::Creating(responder)) = state.offscreen.take() {
                self.settle(responder, Err(ApiError::context_invalidated()), out);
            }
        }
        self.op(NativeOp::CloseOffscreen { install });
    }

    pub(crate) fn storage_done(&mut self, done: Vec<Done>, out: &mut Outbox) {
        self.tick_clock();
        for done in done {
            match done {
                Done::Call {
                    ticket,
                    result,
                    change,
                } => {
                    let Some((responder, sender)) = self.tickets.remove(&ticket) else {
                        continue;
                    };
                    self.settle(responder, result, out);
                    if let Some(change) = change {
                        self.storage_changed(&sender, &change, out);
                    }
                }
                Done::Failed { install, message } => {
                    if let Some(install) = install {
                        self.record_error(
                            install,
                            ExtensionErrorKind::Console,
                            ContextKind::Background,
                            message.into(),
                            Value::Null,
                        );
                    }
                }
            }
        }
        self.settle_blockers(out);
    }

    // ---- queries ---------------------------------------------------------

    pub(crate) fn errors(&self, install: ExtensionInstallId) -> Vec<ExtensionError> {
        self.state
            .installs
            .get(&install)
            .map(|state| state.errors.records())
            .unwrap_or_default()
    }

    pub(crate) fn unhandled(&self, install: ExtensionInstallId) -> BTreeMap<String, u64> {
        self.state
            .installs
            .get(&install)
            .map(|state| state.unhandled.clone())
            .unwrap_or_default()
    }

    pub(crate) fn status(&self, install: ExtensionInstallId) -> Option<BackgroundStatus> {
        let state = self.state.installs.get(&install)?;
        Some(BackgroundStatus {
            state: self.lifecycle.state(install)?,
            generation: self.lifecycle.generation(install)?,
            ready: state.live.as_ref().is_some_and(|live| live.activated),
            startup_ms: state.startup_ms,
        })
    }

    pub(crate) fn page_bootstrap(
        &self,
        install: ExtensionInstallId,
        kind: ContextKind,
    ) -> Option<Value> {
        self.state
            .installs
            .get(&install)
            .map(|state| state.bootstrap(kind, &self.state.ui_language, self.state.incognito))
    }

    pub(crate) fn has_offscreen(&self, install: ExtensionInstallId) -> bool {
        self.state
            .installs
            .get(&install)
            .is_some_and(|state| matches!(state.offscreen, Some(Offscreen::Open)))
    }

    pub(crate) fn uninstall_url(&self, install: ExtensionInstallId) -> Option<String> {
        self.state.installs.get(&install)?.uninstall_url.clone()
    }

    // ---- the lifecycle machine ------------------------------------------

    fn observe(&mut self, observation: Observation, out: &mut Outbox) {
        let clock = At(self.state.now);
        let Ok(commands) = self.lifecycle.observe(observation, &clock) else {
            return;
        };
        self.execute(commands, out);
    }

    fn execute(&mut self, commands: Vec<Command>, out: &mut Outbox) {
        for command in commands {
            match command {
                Command::Start {
                    install,
                    generation,
                    reason: _,
                } => self.start(install, generation, out),
                Command::Deliver { install, .. } => self.deliver(install, out),
                Command::CancelWork {
                    install,
                    generation,
                } => {
                    self.cancel_work(install, generation, out);
                }
                Command::Terminate {
                    install,
                    generation,
                    reason,
                } => {
                    if reason != StopReason::Shutdown
                        && self
                            .state
                            .installs
                            .get(&install)
                            .is_some_and(|state| state.offscreen.is_some())
                    {
                        self.close_offscreen_now(install, out);
                    }
                    if let Some(state) = self.state.installs.get_mut(&install) {
                        state.live = None;
                    }
                    self.op(NativeOp::StopBackground {
                        install,
                        generation,
                    });
                }
                Command::Disabled { install } => {
                    self.fail_parked(install, &ApiError::context_invalidated(), out);
                    self.record_error(
                        install,
                        ExtensionErrorKind::Disabled,
                        ContextKind::Background,
                        "The background was disabled after crashing repeatedly.".into(),
                        Value::Null,
                    );
                    self.op(NativeOp::Notify(Notice::Disabled(install)));
                }
            }
        }
    }

    fn start(&mut self, install: ExtensionInstallId, generation: u64, out: &mut Outbox) {
        let now_real = self.state.now;
        let ui_language = self.state.ui_language.clone();
        let incognito = self.state.incognito;
        let Some(state) = self.state.installs.get_mut(&install) else {
            return;
        };
        let Some(script) = state.background.clone() else {
            return;
        };
        let bootstrap = Bootstrap {
            extension_id: &state.extension_id,
            kind: ContextKind::Background,
            manifest: &state.manifest,
            messages: &state.catalog,
            ui_language: &ui_language,
            incognito,
            permissions: &state.granted,
        };
        let launch = background::launch(
            &state.extension_id,
            &script,
            &bootstrap,
            state.fresh,
            state.unlimited,
        );
        state.live = Some(Live {
            generation,
            sender: None,
            booted: false,
            activated: false,
            delivering: None,
            direct: HashSet::new(),
            started_at: now_real,
        });
        match launch {
            Ok(launch) => self.op(NativeOp::StartBackground {
                install,
                generation,
                launch: Box::new(launch),
            }),
            Err(error) => {
                let message = format!("The background could not be prepared: {error}");
                self.background_failed(install, generation, &message, out);
            }
        }
    }

    fn cancel_work(&mut self, install: ExtensionInstallId, generation: u64, out: &mut Outbox) {
        let key = self
            .state
            .installs
            .get(&install)
            .and_then(|state| state.live.as_ref())
            .filter(|live| live.generation == generation)
            .and_then(|live| live.sender.as_ref())
            .map(Sender::context_key);
        if let Some(key) = key {
            self.close_context(&key, out);
        }
        if let Some(live) = self
            .state
            .installs
            .get_mut(&install)
            .and_then(|state| state.live.as_mut())
            .filter(|live| live.generation == generation)
        {
            live.sender = None;
            live.activated = false;
            live.delivering = None;
            live.direct.clear();
        }
    }

    /// Delivers the queue head to a ready background.
    fn deliver(&mut self, install: ExtensionInstallId, out: &mut Outbox) {
        let Some(state) = self.state.installs.get_mut(&install) else {
            return;
        };
        let Some(live) = state.live.as_mut() else {
            return;
        };
        if !live.activated || live.delivering.is_some() {
            return;
        }
        let Some(sender) = live.sender.clone() else {
            return;
        };
        let Ok(Some(event)) = state.queue.deliver(&sender) else {
            return;
        };
        let Some(frame) = (|| {
            frame(
                ContextKind::Background,
                &Envelope::Event {
                    seq: event.sequence,
                    name: ApiName::parse(&event.name)?,
                    args: &event.arguments,
                },
            )
        })() else {
            return;
        };
        live.delivering = Some(event.sequence);
        let generation = live.generation;
        out.deliver(sender.context_key(), frame);
        self.observe(
            Observation::EventStarted {
                install,
                generation,
                event: event.sequence.get(),
            },
            out,
        );
    }

    fn enqueue(
        &mut self,
        install: ExtensionInstallId,
        name: &str,
        args: &Value,
        importance: EventImportance,
    ) -> bool {
        let Some(state) = self.state.installs.get_mut(&install) else {
            return false;
        };
        let Ok(bytes) = serde_json::to_vec(args) else {
            return false;
        };
        state.queue.enqueue(name, bytes, importance).is_ok()
    }

    /// An event for every context of `install` listening to `name`: direct
    /// to live pages, queued (waking it) for the background.
    fn emit(
        &mut self,
        install: ExtensionInstallId,
        name: &str,
        args: &Value,
        reason: StartReason,
        out: &mut Outbox,
    ) {
        let facts = EventFacts::default();
        let pages: Vec<Sender> = self
            .router
            .contexts
            .values()
            .filter(|sender| {
                sender.install() == install
                    && sender.kind() != ContextKind::Background
                    && sender.kind().is_extension_page()
                    && self.state.registry.has_listeners(sender, name)
            })
            .cloned()
            .collect();
        for page in pages {
            if let Some(frame) = event_frame(page.kind(), direct_sequence(), name, args) {
                out.deliver(page.context_key(), frame);
            }
        }
        let background = self.state.installs.get(&install).and_then(|state| {
            state.background.as_ref()?;
            Some(state.live.as_ref().and_then(|live| live.sender.clone()))
        });
        let Some(background) = background else {
            return;
        };
        let listening = background
            .as_ref()
            .is_some_and(|sender| self.state.registry.has_listeners(sender, name))
            || self.state.registry.should_wake(install, name, &facts);
        if listening && self.enqueue(install, name, args, EventImportance::Droppable) {
            self.observe(Observation::Wake { install, reason }, out);
        }
    }

    fn storage_changed(&mut self, source: &Sender, change: &StorageChangeEvent, out: &mut Outbox) {
        let install = source.install();
        let mut changes = serde_json::Map::new();
        for (key, value) in &change.changes {
            if let Ok(value) = serde_json::to_value(value) {
                changes.insert(key.clone(), value);
            }
        }
        let changes = Value::Object(changes);
        let area_event = format!("storage.{}.onChanged", change.area.as_str());
        for (name, args) in [
            ("storage.onChanged", json!([changes, change.area.as_str()])),
            (area_event.as_str(), json!([changes])),
        ] {
            let trusted_only = change.access_level == AccessLevel::TrustedContexts;
            let pages: Vec<Sender> = self
                .router
                .contexts
                .values()
                .filter(|sender| {
                    sender.install() == install
                        && sender.kind() != ContextKind::Background
                        && sender.kind() != ContextKind::ExternalPage
                        && (!trusted_only || sender.kind().is_runtime_owned())
                        && self.state.registry.has_listeners(sender, name)
                })
                .cloned()
                .collect();
            for page in pages {
                if let Some(frame) = event_frame(page.kind(), direct_sequence(), name, &args) {
                    out.deliver(page.context_key(), frame);
                }
            }
            let background = self
                .state
                .installs
                .get(&install)
                .and_then(|state| state.live.as_ref())
                .and_then(|live| live.sender.clone());
            let listening = background
                .as_ref()
                .is_some_and(|sender| self.state.registry.has_listeners(sender, name))
                || self
                    .state
                    .registry
                    .should_wake(install, name, &EventFacts::default());
            if listening && self.enqueue(install, name, &args, EventImportance::Droppable) {
                self.observe(
                    Observation::Wake {
                        install,
                        reason: StartReason::Event,
                    },
                    out,
                );
            }
        }
    }

    /// Feeds current resource counts of every running background.
    fn settle_blockers(&mut self, out: &mut Outbox) {
        let mut changed = Vec::new();
        for (install, state) in &mut self.state.installs {
            let Some(live) = state.live.as_ref().filter(|live| live.activated) else {
                continue;
            };
            let Some(sender) = &live.sender else {
                continue;
            };
            let key = sender.context_key();
            let awaiting = self
                .awaiting
                .values()
                .filter(|waiting| **waiting == key)
                .count();
            let blockers = Blockers {
                calls: self
                    .dispatcher
                    .limiter()
                    .context_calls(&key)
                    .saturating_add(u32::try_from(awaiting).unwrap_or(u32::MAX)),
                ports: u32::try_from(self.router.ports.context_port_count(&key))
                    .unwrap_or(u32::MAX),
                pending_events: u32::try_from(state.queue.len() + live.direct.len())
                    .unwrap_or(u32::MAX),
                offscreen_document: state.offscreen.is_some(),
                native_ports: 0,
            };
            if blockers != state.blockers {
                state.blockers = blockers;
                changed.push((*install, live.generation, blockers));
            }
        }
        for (install, generation, blockers) in changed {
            self.observe(
                Observation::Blockers {
                    install,
                    generation,
                    blockers,
                },
                out,
            );
        }
    }

    fn record_error(
        &mut self,
        install: ExtensionInstallId,
        kind: ExtensionErrorKind,
        context: ContextKind,
        message: String,
        detail: Value,
    ) {
        let now = self.state.now;
        let Some(state) = self.state.installs.get_mut(&install) else {
            return;
        };
        let number = |key: &str| {
            detail[key]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .unwrap_or(0)
        };
        let error = ExtensionError {
            install,
            extension_id: state.extension_id.as_str().to_owned(),
            kind,
            context,
            message: truncate(&message, MAX_ERROR_MESSAGE_BYTES),
            source: truncate(
                detail["filename"].as_str().unwrap_or(""),
                MAX_ERROR_SOURCE_BYTES,
            ),
            line: number("lineno"),
            column: number("colno"),
        };
        if state.errors.push(error.clone(), now) {
            self.op(NativeOp::Notify(Notice::Error(error)));
        }
    }

    // ---- messaging -------------------------------------------------------

    fn settle(&mut self, responder: Responder, result: Result<Value, ApiError>, out: &mut Outbox) {
        if let Some(settled) = self.dispatcher.settle(responder, result) {
            out.deliver(settled.context, settled.frame);
        }
    }

    /// Whether a request to `install`'s pages must first wake its background.
    fn needs_background(&self, install: ExtensionInstallId, source: &Sender, event: &str) -> bool {
        let Some(state) = self.state.installs.get(&install) else {
            return false;
        };
        if state.background.is_none() || source.kind() == ContextKind::Background {
            return false;
        }
        let ready = state.live.as_ref().is_some_and(|live| live.activated);
        !ready
            && self
                .state
                .registry
                .should_wake(install, event, &EventFacts::default())
    }

    fn park(
        &mut self,
        install: ExtensionInstallId,
        parked: Parked,
        out: &mut Outbox,
    ) -> Result<(), Parked> {
        let full = self
            .state
            .installs
            .get(&install)
            .is_none_or(|state| state.parked.len() >= MAX_PARKED_PER_INSTALL);
        if full || self.lifecycle.state(install) == Some(BackgroundState::Disabled) {
            return Err(parked);
        }
        if let Some(state) = self.state.installs.get_mut(&install) {
            state.parked.push(parked);
        }
        self.observe(
            Observation::Wake {
                install,
                reason: StartReason::Message,
            },
            out,
        );
        Ok(())
    }

    fn send(
        &mut self,
        sender: Box<Sender>,
        payload: Vec<u8>,
        responder: Responder,
        out: &mut Outbox,
    ) {
        let install = sender.install();
        if self.needs_background(install, &sender, "runtime.onMessage") {
            let parked = Parked::Send {
                sender,
                payload,
                responder,
                deadline: self.state.now.saturating_add(PARKED_TIMEOUT_NANOS),
            };
            if let Err(Parked::Send { responder, .. }) = self.park(install, parked, out) {
                self.settle(responder, Err(no_receiver()), out);
            }
            return;
        }
        let Some(listener) = ListenerId::new(EVENT_LISTENER) else {
            return;
        };
        let listeners: Vec<MessageListener> = self
            .router
            .contexts
            .values()
            .filter(|context| {
                context.install() == install
                    && context.kind().is_extension_page()
                    && self
                        .state
                        .registry
                        .has_listeners(context, "runtime.onMessage")
            })
            .map(|context| MessageListener {
                sender: context.clone(),
                listener,
            })
            .collect();
        let clock = At(self.state.now);
        let sent = self.router.messages.send(
            &sender,
            &MessageRoute::Runtime,
            &listeners,
            &payload,
            self.dispatcher.limiter_mut(),
            &clock,
        );
        match sent {
            Err(error) => self.settle(responder, Err(error), out),
            Ok((id, deliveries)) => {
                self.sends.insert(id.get(), (id, responder));
                for delivery in deliveries {
                    self.deliver_message(delivery, out);
                }
            }
        }
    }

    fn deliver_message(&mut self, delivery: OneShotDelivery, out: &mut Outbox) {
        let Some(recipient) = self.router.contexts.get(&delivery.recipient).cloned() else {
            return;
        };
        let Ok(message) = serde_json::from_slice::<Value>(&delivery.payload) else {
            return;
        };
        let args = json!([
            message,
            sender_json(&delivery.sender),
            delivery.message.get().to_string()
        ]);
        let seq = direct_sequence();
        let Some(frame) = event_frame(recipient.kind(), seq, delivery.event, &args) else {
            return;
        };
        out.deliver(delivery.recipient.clone(), frame);
        if recipient.kind() == ContextKind::Background {
            self.awaiting
                .insert(delivery.message.get(), delivery.recipient.clone());
            let install = recipient.install();
            let generation = self.state.installs.get_mut(&install).and_then(|state| {
                let live = state.live.as_mut()?;
                live.direct.insert(seq);
                Some(live.generation)
            });
            if let Some(generation) = generation {
                self.observe(
                    Observation::EventStarted {
                        install,
                        generation,
                        event: seq,
                    },
                    out,
                );
            }
        }
    }

    fn finish_send(&mut self, settlement: MessageSettlement, out: &mut Outbox) {
        self.awaiting.remove(&settlement.message.get());
        let Some((id, responder)) = self.sends.remove(&settlement.message.get()) else {
            return;
        };
        let result = settlement.result.and_then(|response| match response {
            None => Ok(Value::Null),
            Some(bytes) => {
                let mut text = Vec::with_capacity(bytes.len() + 13);
                text.extend_from_slice(b"{\"response\":");
                text.extend_from_slice(&bytes);
                text.push(b'}');
                serde_json::from_slice(&text).map_err(|_| ApiError::malformed_arguments())
            }
        });
        self.settle(responder, result, out);
        if let Some(source) = self.router.contexts.get(&settlement.recipient).cloned() {
            self.router
                .messages
                .acknowledge(&source, id, self.dispatcher.limiter_mut());
        }
    }

    fn connect(&mut self, sender: Box<Sender>, name: &str, responder: Responder, out: &mut Outbox) {
        let begun = self
            .router
            .ports
            .begin_connect(&sender, MessageRoute::Runtime, name);
        let (port, _) = match begun {
            Ok(begun) => begun,
            Err(error) => {
                self.settle(responder, Err(error), out);
                return;
            }
        };
        self.settle(responder, Ok(Value::String(port.get().to_string())), out);
        let install = sender.install();
        if self.needs_background(install, &sender, "runtime.onConnect") {
            let parked = Parked::Connect {
                port,
                deadline: self.state.now.saturating_add(PARKED_TIMEOUT_NANOS),
            };
            if self.park(install, parked, out).is_err() {
                let limiter = self.dispatcher.limiter_mut();
                let actions = self.router.ports.fail_connect(port, no_receiver(), limiter);
                self.router.port_actions(actions, limiter, out);
            }
            return;
        }
        self.complete_connect(install, port, out);
    }

    fn complete_connect(&mut self, install: ExtensionInstallId, port: PortId, out: &mut Outbox) {
        let receivers: Vec<Sender> = self
            .router
            .contexts
            .values()
            .filter(|context| {
                context.install() == install
                    && context.kind().is_extension_page()
                    && self
                        .state
                        .registry
                        .has_listeners(context, "runtime.onConnect")
            })
            .cloned()
            .collect();
        let clock = At(self.state.now);
        let limiter = self.dispatcher.limiter_mut();
        let actions = match self
            .router
            .ports
            .complete_connect(port, &receivers, limiter, &clock)
        {
            Ok(actions) => actions,
            Err(_) => return,
        };
        self.router.port_actions(actions, limiter, out);
    }

    fn release_parked(&mut self, install: ExtensionInstallId, out: &mut Outbox) {
        let parked = match self.state.installs.get_mut(&install) {
            Some(state) => std::mem::take(&mut state.parked),
            None => return,
        };
        for parked in parked {
            match parked {
                Parked::Send {
                    sender,
                    payload,
                    responder,
                    ..
                } => self.send(sender, payload, responder, out),
                Parked::Connect { port, .. } => self.complete_connect(install, port, out),
            }
        }
    }

    fn fail_parked(&mut self, install: ExtensionInstallId, error: &ApiError, out: &mut Outbox) {
        let parked = match self.state.installs.get_mut(&install) {
            Some(state) => std::mem::take(&mut state.parked),
            None => return,
        };
        for parked in parked {
            self.fail_one(parked, error, out);
        }
    }

    fn fail_one(&mut self, parked: Parked, error: &ApiError, out: &mut Outbox) {
        match parked {
            Parked::Send { responder, .. } => self.settle(responder, Err(error.clone()), out),
            Parked::Connect { port, .. } => {
                let limiter = self.dispatcher.limiter_mut();
                let actions = self.router.ports.fail_connect(port, no_receiver(), limiter);
                self.router.port_actions(actions, limiter, out);
            }
        }
    }

    fn expire_parked(&mut self, install: ExtensionInstallId, now: u64, out: &mut Outbox) {
        let expired: Vec<Parked> = match self.state.installs.get_mut(&install) {
            Some(state) => {
                let (expired, kept) = std::mem::take(&mut state.parked).into_iter().partition(
                    |parked| match parked {
                        Parked::Send { deadline, .. } | Parked::Connect { deadline, .. } => {
                            *deadline <= now
                        }
                    },
                );
                state.parked = kept;
                expired
            }
            None => return,
        };
        for parked in expired {
            self.fail_one(parked, &no_receiver(), out);
        }
    }

    // ---- work the handlers left ------------------------------------------

    fn process_work(&mut self, out: &mut Outbox) {
        while !self.state.work.is_empty() {
            let work = std::mem::take(&mut self.state.work);
            for item in work {
                self.work(item, out);
            }
        }
        let acks = std::mem::take(&mut self.router.acks);
        for (sender, seq) in acks {
            self.acknowledge(&sender, seq, out);
        }
    }

    fn work(&mut self, work: Work, out: &mut Outbox) {
        match work {
            Work::Storage {
                responder,
                sender,
                area,
                member,
                args,
                unlimited,
            } => {
                self.next_ticket += 1;
                let ticket = self.next_ticket;
                let submitted = self.storage.submit(Job::Call {
                    ticket,
                    install: sender.install(),
                    sender: sender.clone(),
                    area,
                    member,
                    args,
                    unlimited,
                    now: self.state.now,
                });
                if submitted {
                    self.tickets.insert(ticket, (responder, sender));
                } else {
                    self.settle(
                        responder,
                        Err(ApiError::other("Storage operation failed.")),
                        out,
                    );
                }
            }
            Work::Send {
                responder,
                sender,
                payload,
            } => self.send(sender, payload, responder, out),
            Work::Connect {
                responder,
                sender,
                name,
            } => self.connect(sender, &name, responder, out),
            Work::Respond {
                sender,
                token,
                outcome,
            } => {
                let Some(id) = self.sends.get(&token).map(|(id, _)| *id) else {
                    return;
                };
                let Some(listener) = ListenerId::new(EVENT_LISTENER) else {
                    return;
                };
                let clock = At(self.state.now);
                if let Ok(Some(settlement)) = self.router.messages.observe(
                    &sender,
                    id,
                    listener,
                    outcome,
                    self.dispatcher.limiter_mut(),
                    &clock,
                ) {
                    self.finish_send(settlement, out);
                }
            }
            Work::Lifecycle {
                sender,
                state,
                detail,
            } => self.worker_lifecycle(&sender, &state, &detail, out),
            Work::WorkerError { sender, detail } => {
                let kind = match detail["kind"].as_str() {
                    Some("console") => ExtensionErrorKind::Console,
                    Some("rejection") => ExtensionErrorKind::UnhandledRejection,
                    _ => ExtensionErrorKind::Uncaught,
                };
                let message = detail["message"].as_str().unwrap_or("").to_owned();
                let install = sender.install();
                let booting = self
                    .state
                    .installs
                    .get(&install)
                    .and_then(|state| state.live.as_ref())
                    .is_some_and(|live| !live.booted);
                if booting && kind == ExtensionErrorKind::Uncaught {
                    let generation = self.live_generation(install);
                    self.record_error(
                        install,
                        ExtensionErrorKind::Startup,
                        ContextKind::Background,
                        message,
                        detail,
                    );
                    if let Some(generation) = generation {
                        self.observe(
                            Observation::Crashed {
                                install,
                                generation,
                            },
                            out,
                        );
                        self.op(NativeOp::DiscardBackground {
                            install,
                            generation,
                            crashed: false,
                        });
                    }
                } else {
                    self.record_error(install, kind, sender.kind(), message, detail);
                }
            }
            Work::CreateOffscreen {
                responder,
                sender,
                url,
                clipboard,
            } => {
                let install = sender.install();
                match self.state.installs.get_mut(&install) {
                    Some(state) if state.offscreen.is_none() => {
                        state.offscreen = Some(Offscreen::Creating(responder));
                        self.op(NativeOp::CreateOffscreen {
                            install,
                            url,
                            clipboard,
                        });
                    }
                    _ => self.settle(
                        responder,
                        Err(ApiError::other(
                            "Only a single offscreen document may be created.",
                        )),
                        out,
                    ),
                }
            }
            Work::CloseOffscreen { responder, sender } => {
                let install = sender.install();
                let open = self
                    .state
                    .installs
                    .get_mut(&install)
                    .is_some_and(|state| state.offscreen.take().is_some());
                if open {
                    self.op(NativeOp::CloseOffscreen { install });
                    self.settle(responder, Ok(Value::Null), out);
                } else {
                    self.settle(
                        responder,
                        Err(ApiError::other("No current offscreen document.")),
                        out,
                    );
                }
            }
            Work::OpenOptions { install } => {
                self.op(NativeOp::Notify(Notice::OpenOptionsPage(install)))
            }
            Work::PersistAlarms(install) => self.persist_alarms(install),
            Work::PersistListeners(install) => {
                let snapshot = self.state.registry.snapshot(install);
                if let (Some(state), Ok(bytes)) = (
                    self.state.installs.get(&install),
                    serde_json::to_vec(&snapshot),
                ) {
                    self.persist_path(state, "listeners.json", Some(bytes));
                }
            }
        }
    }

    fn persist_alarms(&mut self, install: ExtensionInstallId) {
        let now = self.state.now;
        let unix = self.state.unix_millis;
        let Some(state) = self.state.installs.get(&install) else {
            return;
        };
        let environment = AlarmEnvironment::now(&At(now), unix, state.unpacked);
        if let Ok(bytes) = serde_json::to_vec(&state.alarms.snapshot_at(environment)) {
            self.persist_path(state, "alarms.json", Some(bytes));
        }
    }

    fn worker_lifecycle(&mut self, sender: &Sender, stage: &str, detail: &Value, out: &mut Outbox) {
        let install = sender.install();
        let Some(generation) = self.live_generation(install) else {
            return;
        };
        match stage {
            "booted" => {
                if let Some(live) = self
                    .state
                    .installs
                    .get_mut(&install)
                    .and_then(|s| s.live.as_mut())
                {
                    live.booted = true;
                }
            }
            "activated" => {
                let _ = self.state.registry.finish_registration(sender);
                self.state.work.push(Work::PersistListeners(install));
                let now = self.state.now;
                if let Some(state) = self.state.installs.get_mut(&install) {
                    state.fresh = false;
                    if let Some(live) = state.live.as_mut() {
                        live.activated = true;
                        live.booted = true;
                        state.startup_ms = Some(
                            now.saturating_sub(live.started_at) as f64 / 1e6 / self.scale as f64,
                        );
                    }
                }
                self.observe(
                    Observation::Started {
                        install,
                        generation,
                    },
                    out,
                );
                self.release_parked(install, out);
                self.deliver(install, out);
            }
            "failed" | "timedOut" => {
                let message = match detail.as_str() {
                    Some("install") => "The service worker's install handler failed.".to_owned(),
                    Some(text) => text.to_owned(),
                    None if stage == "timedOut" => {
                        "The service worker did not activate in time.".to_owned()
                    }
                    None => "The service worker failed to start.".to_owned(),
                };
                self.background_failed(install, generation, &message, out);
            }
            _ => {}
        }
    }

    fn acknowledge(&mut self, sender: &Sender, seq: EventSeq, out: &mut Outbox) {
        let install = sender.install();
        if sender.kind() != ContextKind::Background {
            return;
        }
        let Some(state) = self.state.installs.get_mut(&install) else {
            return;
        };
        let Some(live) = state.live.as_mut() else {
            return;
        };
        let generation = live.generation;
        let finished = if live.delivering == Some(seq) && state.queue.acknowledge(sender, seq) {
            live.delivering = None;
            true
        } else {
            live.direct.remove(&seq.get())
        };
        if finished {
            self.observe(
                Observation::EventFinished {
                    install,
                    generation,
                    event: seq.get(),
                },
                out,
            );
            self.deliver(install, out);
        }
    }

    fn close_context(&mut self, context: &ContextKey, out: &mut Outbox) {
        self.dispatcher.cancel_context(context);
        let limiter = self.dispatcher.limiter_mut();
        let actions = self.router.ports.cancel_context(context, limiter);
        self.router.port_actions(actions, limiter, out);
        let settlements = self
            .router
            .messages
            .cancel_context(context, self.dispatcher.limiter_mut());
        for settlement in settlements {
            self.finish_send(settlement, out);
        }
        self.awaiting.retain(|_, waiting| waiting != context);
        self.state.registry.cancel_context(context);
        if let Some(state) = self.state.installs.get_mut(&context.install()) {
            state.queue.cancel_context(context);
            if let Some(live) = state.live.as_mut() {
                if live
                    .sender
                    .as_ref()
                    .is_some_and(|sender| &sender.context_key() == context)
                {
                    live.sender = None;
                    live.delivering = None;
                    live.direct.clear();
                }
            }
        }
        self.tickets
            .retain(|_, (_, sender)| &sender.context_key() != context);
        self.router.contexts.remove(context);
    }

    fn note_sender(&mut self, sender: &Sender) {
        let key = sender.context_key();
        if self.router.contexts.contains_key(&key) {
            return;
        }
        self.router.contexts.insert(key, sender.clone());
        if sender.kind() != ContextKind::Background {
            return;
        }
        if let Some(live) = self
            .state
            .installs
            .get_mut(&sender.install())
            .and_then(|state| state.live.as_mut())
        {
            if live.sender.is_none() {
                live.sender = Some(sender.clone());
                let _ = self.state.registry.begin_registration(sender);
            }
        }
    }
}

impl Runtime for RuntimeHost {
    fn receive(&mut self, sender: &Sender, envelope: &[u8], out: &mut Outbox) {
        let now = self.tick_clock();
        let _ = now;
        self.note_sender(sender);
        let decoded = Envelope::decode(envelope, Direction::FromContext, u64::MAX);
        if let Ok(Envelope::Call { name, .. }) = &decoded {
            if self.dispatcher.table().contexts(name.as_str()).is_none() {
                let name = name.as_str().to_owned();
                if let Some(state) = self.state.installs.get_mut(&sender.install()) {
                    state.record_unhandled(&name);
                }
            }
        }
        if sender.kind() == ContextKind::Background {
            if let Some(generation) = self.live_generation(sender.install()) {
                self.observe(
                    Observation::Activity {
                        install: sender.install(),
                        generation,
                    },
                    out,
                );
            }
        }
        let Some(permissions) = self.permissions.get(&sender.install()) else {
            if let Ok(Envelope::Call { id, .. }) = decoded {
                let error = ApiError::context_invalidated();
                let reply = Envelope::Reply {
                    id,
                    result: Err(error.message()),
                };
                if let Some(frame) = frame(sender.kind(), &reply) {
                    out.deliver(sender.context_key(), frame);
                }
            }
            return;
        };
        let real = self.state.now;
        let traffic =
            match self
                .dispatcher
                .receive(&mut self.state, permissions, sender, envelope, real)
            {
                Inbound::Reply(frame) => {
                    out.deliver(sender.context_key(), frame);
                    None
                }
                Inbound::Pending | Inbound::Refused(_) => None,
                Inbound::PortRefused { port, error } => {
                    if let Some(frame) = frame(
                        sender.kind(),
                        &Envelope::PortDisconnect {
                            port,
                            error: Some(error.message()),
                        },
                    ) {
                        out.deliver(sender.context_key(), frame);
                    }
                    None
                }
                Inbound::PortMessage {
                    port,
                    payload,
                    reservation,
                } => Some(PortTraffic::Message {
                    sender,
                    port,
                    payload,
                    reservation,
                }),
                Inbound::PortDisconnect { port } => Some(PortTraffic::Disconnect { sender, port }),
                Inbound::EventAck { seq } => Some(PortTraffic::EventAck { sender, seq }),
            };
        if let Some(traffic) = traffic {
            self.router.traffic(&mut self.dispatcher, traffic, out);
        }
        self.process_work(out);
        self.settle_blockers(out);
        self.check_timer();
    }

    fn context_closed(&mut self, context: &ContextKey, _: CloseReason, out: &mut Outbox) {
        self.tick_clock();
        self.close_context(context, out);
        self.router.context_closed(context, out);
        self.settle_blockers(out);
        self.check_timer();
    }

    fn frame_lifecycle(&mut self, event: &FrameLifecycle, _: &mut Outbox) {
        // Resolve createDocument once the bridge saw the load: posts the
        // document made before it are handled by the next idle.
        if let FrameLifecycle::DocumentLoaded {
            owner:
                ViewRef::Extension {
                    install,
                    kind: ViewKind::Offscreen,
                },
            frame,
            ..
        } = event
        {
            let creating = self
                .state
                .installs
                .get(install)
                .is_some_and(|state| matches!(state.offscreen, Some(Offscreen::Creating(_))));
            if frame.is_main() && creating {
                self.op(NativeOp::OffscreenReady { install: *install });
            }
        }
    }

    fn undeliverable(&mut self, context: &ContextKey, error: DeliverError) {
        let _ = (context, error);
    }
}

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;
