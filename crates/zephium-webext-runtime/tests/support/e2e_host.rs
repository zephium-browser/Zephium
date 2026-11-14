use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, Write};

use runtime::alarms::{AlarmEnvironment, AlarmHost, Alarms};
use runtime::context::{ClaimedWorld, CommittedDocument, InjectedWorld, TabDocument, ViewKind};
use runtime::dispatch::{encode_for, Responder};
use runtime::envelope::{ApiName, Envelope, EventSeq};
use runtime::events::{
    EventFacts, EventImportance, EventQueue, EventRegistry, EventsHost, ListenerRequest,
};
use runtime::lifecycle::{Command, Lifecycle, Observation, StartReason};
use runtime::limits::{Clock, ManualClock};
use runtime::pattern::DocumentOrigin;
use runtime::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
use runtime::ports::{
    MessageId, MessageListener, MessageRequest, MessageRoute, MessageService, MessageSettlement,
    MessageTarget, MessagingHost, PortAction, PortService,
};
use runtime::protocol::{ProtocolHost, ProtocolRequest, WorkerSession};
use runtime::storage::{Storage, StorageChangeEvent, StorageHost};
use runtime::{
    ApiError, ApiTable, Authority, ContextKey, Dispatcher, DocumentGeneration, ExtensionId,
    ExtensionScheme, ExtensionView, FrameId, Handled, Inbound, InstallAdmission,
    InstallPermissions, Invocation, Sender,
};
use serde_json::{json, Value};
use url::Url;
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_runtime as runtime;

const EXTENSION: &str = "abcdefghijklmnopabcdefghijklmnop";
const PREFIX: &str = "ZEPHIUM_E2E:";

enum Work {
    Message(Sender, MessageRequest, Responder),
    Protocol(Sender, ProtocolRequest),
    Changed(StorageChangeEvent),
}

struct Host {
    storage: Storage,
    alarms: Alarms,
    clock: ManualClock,
    events: EventRegistry,
    lifecycle: Lifecycle,
    sessions: HashMap<ContextKey, WorkerSession>,
    work: VecDeque<Work>,
}

impl Host {
    fn environment(&self) -> AlarmEnvironment {
        AlarmEnvironment::now(
            &self.clock,
            1_000_000 + self.clock.now_nanos() / 1_000_000,
            false,
        )
    }
}

impl StorageHost for Host {
    fn storage(&mut self, _: &Sender) -> Result<&mut Storage, ApiError> {
        Ok(&mut self.storage)
    }
    fn storage_now_nanos(&self) -> u64 {
        self.clock.now_nanos()
    }
    fn storage_changed(&mut self, _: &Sender, change: StorageChangeEvent) {
        self.work.push_back(Work::Changed(change));
    }
}

impl AlarmHost for Host {
    fn alarm_call(&mut self, _: &Sender, method: &str, args: &[Value]) -> Result<Value, ApiError> {
        let environment = self.environment();
        self.alarms.handle(method, args, environment)
    }
}

impl EventsHost for Host {
    fn listener_request(&mut self, request: ListenerRequest, call: Invocation<'_>) -> Handled {
        let sender = call.sender();
        let result = match request {
            ListenerRequest::Add {
                event,
                id,
                filter,
                extra,
            } => self
                .events
                .add_with_options(sender, id, event, filter.as_ref(), &extra)
                .map(|_| Value::Null),
            ListenerRequest::Remove { event, id } => {
                Ok(json!(self.events.remove(sender, event, id)))
            }
            ListenerRequest::Has { event, id } => {
                Ok(json!(self.events.has_listener(sender, id, event)))
            }
            ListenerRequest::HasAny { event } => {
                Ok(json!(self.events.has_listeners(sender, event)))
            }
            ListenerRequest::CompleteRegistration => self
                .events
                .finish_registration(sender)
                .map(|()| Value::Null),
        };
        call.reply(result)
    }
}

impl MessagingHost for Host {
    fn message_request(&mut self, request: MessageRequest, call: Invocation<'_>) -> Handled {
        let sender = call.sender().clone();
        let (handled, responder) = call.defer();
        self.work
            .push_back(Work::Message(sender, request, responder));
        handled
    }
}

impl ProtocolHost for Host {
    fn protocol_request(&mut self, request: ProtocolRequest, call: Invocation<'_>) -> Handled {
        if let ProtocolRequest::WorkerLifecycle(report) = request {
            let result = self.sessions[&call.sender().context_key()]
                .observe(
                    call.sender(),
                    report,
                    &mut self.lifecycle,
                    &mut self.events,
                    &self.clock,
                )
                .map(|commands| {
                    assert!(commands
                        .iter()
                        .all(|command| matches!(command, Command::Deliver { .. })));
                    Value::Null
                });
            return call.reply(result);
        }
        self.work
            .push_back(Work::Protocol(call.sender().clone(), request));
        call.reply(Ok(Value::Null))
    }
}

struct Peer {
    authority: Authority,
    host: Host,
    dispatcher: Dispatcher<Host>,
    permissions: InstallPermissions,
    senders: HashMap<String, Sender>,
    views: HashMap<String, ExtensionView>,
    messages: MessageService,
    ports: PortService,
    pending: HashMap<MessageId, Responder>,
    queue: EventQueue,
    sequence: u64,
    background_document: u64,
    acknowledged: usize,
}

fn profile() -> ProfileId {
    ProfileId::from(1)
}
fn install() -> ExtensionInstallId {
    ExtensionInstallId::from(10)
}
fn document(url: &str, generation: u64) -> CommittedDocument {
    let url = Url::parse(url).expect("URL");
    CommittedDocument {
        generation: DocumentGeneration::new(generation).expect("generation"),
        origin: DocumentOrigin::of_url(&url),
        url,
    }
}

impl Peer {
    fn new() -> Self {
        let mut authority = Authority::new(profile(), false, ExtensionScheme::CHROME_EXTENSION);
        authority
            .admit(InstallAdmission {
                install: install(),
                extension_id: ExtensionId::parse(EXTENSION).expect("extension"),
                generation: ExtensionRuntimeGeneration::new(1).expect("generation"),
                allowed_in_incognito: false,
                externally_connectable: None,
            })
            .expect("admit");
        let permissions = ["storage", "alarms", "nativeMessaging"];
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions": permissions}),
        ));
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let mut table = ApiTable::new();
        runtime::storage::register(&mut table).expect("storage methods");
        runtime::alarms::register(&mut table).expect("alarm methods");
        runtime::events::register(&mut table).expect("events");
        runtime::ports::register(&mut table).expect("messages");
        runtime::protocol::register(&mut table).expect("binding protocol");
        let mut lifecycle = Lifecycle::new(profile());
        assert!(lifecycle.register(install()));
        Self {
            authority,
            host: Host {
                storage: Storage::memory(profile(), install(), false),
                alarms: Alarms::new(),
                clock: ManualClock::default(),
                events: EventRegistry::new(),
                lifecycle,
                sessions: HashMap::new(),
                work: VecDeque::new(),
            },
            dispatcher: Dispatcher::new(table),
            permissions,
            senders: HashMap::new(),
            views: HashMap::new(),
            messages: MessageService::new(),
            ports: PortService::new(),
            pending: HashMap::new(),
            queue: EventQueue::new(install()),
            sequence: 1_000_000,
            background_document: 10,
            acknowledged: 0,
        }
    }

    fn add_context(&mut self, label: &str, kind: &str) {
        let sender = if kind == "background" {
            let commands = self
                .host
                .lifecycle
                .observe(
                    Observation::Wake {
                        install: install(),
                        reason: StartReason::Event,
                    },
                    &self.host.clock,
                )
                .expect("wake background");
            let generation = commands
                .iter()
                .find_map(|command| match command {
                    Command::Start { generation, .. } => Some(*generation),
                    _ => None,
                })
                .expect("start effect");
            self.background_document += 1;
            let view = self
                .authority
                .open_view(install(), ViewKind::Background)
                .expect("background view");
            let sender = self
                .authority
                .bind_view(
                    &view,
                    &document(
                        &format!("chrome-extension://{EXTENSION}/background.html"),
                        self.background_document,
                    ),
                )
                .expect("bind background");
            self.views.insert(label.to_owned(), view);
            self.host
                .events
                .begin_registration(&sender)
                .expect("begin registration");
            self.host.sessions.insert(
                sender.context_key(),
                WorkerSession::new(&sender, &self.host.lifecycle, generation)
                    .expect("worker session"),
            );
            sender
        } else {
            let tab = ItemId::from(5);
            let document = TabDocument {
                tab,
                frame: FrameId::MAIN,
                committed: document("https://example.com/", 3),
            };
            self.authority
                .record_injection(
                    tab,
                    FrameId::MAIN,
                    document.committed.generation,
                    install(),
                    InjectedWorld::Isolated,
                )
                .expect("injection");
            self.authority
                .bind_tab(&document, &ClaimedWorld::Isolated(install()))
                .expect("bind content")
        };
        self.senders.insert(label.to_owned(), sender);
    }

    fn label(&self, context: &ContextKey) -> Option<String> {
        self.senders
            .iter()
            .find(|(_, sender)| sender.context_key() == *context)
            .map(|(label, _)| label.clone())
    }

    fn emit(&self, context: &ContextKey, bytes: Vec<u8>) {
        if let Some(label) = self.label(context) {
            output(json!({"context": label, "frame": bytes}));
        }
    }

    fn event(&mut self, context: &ContextKey, name: &str, args: Value) {
        self.sequence += 1;
        let bytes = serde_json::to_vec(&args).expect("event args");
        self.event_bytes(
            context,
            EventSeq::new(self.sequence).expect("sequence"),
            name,
            &bytes,
        );
    }

    fn event_bytes(&self, context: &ContextKey, sequence: EventSeq, name: &str, args: &[u8]) {
        let sender = self
            .senders
            .values()
            .find(|sender| sender.context_key() == *context)
            .expect("event recipient");
        self.emit(
            context,
            encode_for(
                sender.kind(),
                &Envelope::Event {
                    seq: sequence,
                    name: ApiName::parse(name).expect("event name"),
                    args,
                },
            )
            .expect("event frame"),
        );
    }

    fn settle(&mut self, responder: Responder, result: Result<Value, ApiError>) {
        if let Some(reply) = self.dispatcher.settle(responder, result) {
            self.emit(&reply.context, reply.frame);
        }
    }

    fn message_settled(&mut self, settlement: MessageSettlement) {
        if let Some(responder) = self.pending.remove(&settlement.message) {
            let result = settlement.result.map(|payload| {
                payload.map_or(
                    Value::Null,
                    |bytes| json!({"response": String::from_utf8(bytes).expect("response JSON")}),
                )
            });
            self.settle(responder, result);
            let sender = self
                .senders
                .values()
                .find(|sender| sender.context_key() == settlement.recipient)
                .expect("source");
            assert!(self.messages.acknowledge(
                sender,
                settlement.message,
                self.dispatcher.limiter_mut()
            ));
        }
    }

    fn port_actions(&mut self, actions: Vec<PortAction>) {
        for action in actions {
            match action {
                PortAction::Connect {
                    recipient,
                    port,
                    name,
                    sender,
                    event,
                } => self.event(
                    &recipient,
                    event,
                    json!([port.get().to_string(), name, {"id": sender.extension_id().as_str()}]),
                ),
                PortAction::Message {
                    recipient,
                    port,
                    payload,
                    delivery,
                } => {
                    let sender = self
                        .senders
                        .values()
                        .find(|sender| sender.context_key() == recipient)
                        .expect("port recipient");
                    self.emit(
                        &recipient,
                        encode_for(
                            sender.kind(),
                            &Envelope::PortMessage {
                                port,
                                payload: &payload,
                            },
                        )
                        .expect("port frame"),
                    );
                    assert!(self.ports.acknowledge(
                        sender,
                        delivery,
                        self.dispatcher.limiter_mut()
                    ));
                }
                PortAction::Disconnect {
                    recipient,
                    port,
                    error,
                } => {
                    let sender = self
                        .senders
                        .values()
                        .find(|sender| sender.context_key() == recipient)
                        .expect("port recipient");
                    self.emit(
                        &recipient,
                        encode_for(
                            sender.kind(),
                            &Envelope::PortDisconnect {
                                port,
                                error: error.as_ref().map(ApiError::message),
                            },
                        )
                        .expect("disconnect frame"),
                    );
                }
                other => panic!("unexpected native port action {other:?}"),
            }
        }
    }

    fn receive(&mut self, label: &str, bytes: &[u8]) {
        let sender = self.senders[label].clone();
        let now = self.host.clock.now_nanos();
        match self
            .dispatcher
            .receive(&mut self.host, &self.permissions, &sender, bytes, now)
        {
            Inbound::Reply(bytes) => self.emit(&sender.context_key(), bytes),
            Inbound::Pending => {}
            Inbound::EventAck { seq } => {
                if self.queue.acknowledge(&sender, seq) {
                    self.acknowledged += 1;
                    self.host
                        .lifecycle
                        .observe(
                            Observation::EventFinished {
                                install: install(),
                                generation: self
                                    .host
                                    .lifecycle
                                    .generation(install())
                                    .expect("generation"),
                                event: seq.get(),
                            },
                            &self.host.clock,
                        )
                        .expect("event completion");
                }
            }
            Inbound::PortMessage {
                port,
                payload,
                reservation,
            } => {
                let actions = self
                    .ports
                    .post_admitted(
                        &sender,
                        port,
                        payload,
                        reservation,
                        self.dispatcher.limiter_mut(),
                        &self.host.clock,
                    )
                    .expect("port relay");
                self.port_actions(actions);
            }
            Inbound::PortDisconnect { port } => {
                let actions = self
                    .ports
                    .disconnect(&sender, port, self.dispatcher.limiter_mut());
                self.port_actions(actions);
            }
            other => panic!("unexpected inbound {other:?}"),
        }
        self.drain();
    }

    fn drain(&mut self) {
        while let Some(work) = self.host.work.pop_front() {
            match work {
                Work::Changed(change) => {
                    for name in [
                        "storage.onChanged".to_owned(),
                        format!("storage.{}.onChanged", change.area.as_str()),
                    ] {
                        let targets = self.host.events.storage_targets(
                            &self.host.storage,
                            self.senders.values(),
                            &name,
                            &change,
                        );
                        for target in targets {
                            let args = if name == "storage.onChanged" {
                                json!([change.changes, change.area.as_str()])
                            } else {
                                json!([change.changes])
                            };
                            self.event(&target, &name, args);
                        }
                    }
                }
                Work::Message(source, request, responder) => match request {
                    MessageRequest::Send { payload, target } => {
                        let route = match target {
                            MessageTarget::Runtime { extension_id }
                                if extension_id
                                    .as_ref()
                                    .is_none_or(|id| id == source.extension_id()) =>
                            {
                                MessageRoute::Runtime
                            }
                            MessageTarget::Tab {
                                tab_id: 5,
                                frame_id,
                                document_id: None,
                            } => MessageRoute::Tab {
                                tab: ItemId::from(5),
                                frame: frame_id.map(FrameId::new),
                                document: None,
                            },
                            _ => {
                                self.settle(responder, Err(runtime::ports::no_receiver()));
                                continue;
                            }
                        };
                        let listeners = self
                            .senders
                            .values()
                            .flat_map(|sender| {
                                self.host
                                    .events
                                    .listeners(sender, "runtime.onMessage", &EventFacts::default())
                                    .into_iter()
                                    .map(|listener| MessageListener {
                                        sender: sender.clone(),
                                        listener,
                                    })
                            })
                            .collect::<Vec<_>>();
                        match self.messages.send(
                            &source,
                            &route,
                            &listeners,
                            &payload,
                            self.dispatcher.limiter_mut(),
                            &self.host.clock,
                        ) {
                            Ok((id, deliveries)) => {
                                self.pending.insert(id, responder);
                                for delivery in deliveries {
                                    self.event(
                                        &delivery.recipient,
                                        delivery.event,
                                        delivery
                                            .arguments(
                                                json!({"id": source.extension_id().as_str()}),
                                            )
                                            .expect("message event"),
                                    );
                                }
                            }
                            Err(error) => self.settle(responder, Err(error)),
                        }
                    }
                    MessageRequest::Connect { name, .. } => {
                        let receivers = self
                            .senders
                            .values()
                            .filter(|sender| {
                                self.host.events.has_listeners(sender, "runtime.onConnect")
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        let (port, _) = self
                            .ports
                            .begin_connect(&source, MessageRoute::Runtime, &name)
                            .expect("begin connect");
                        let actions = self
                            .ports
                            .complete_connect(
                                port,
                                &receivers,
                                self.dispatcher.limiter_mut(),
                                &self.host.clock,
                            )
                            .expect("complete connect");
                        self.port_actions(actions);
                        self.settle(responder, Ok(json!(port.get().to_string())));
                    }
                    MessageRequest::SendNative { host, payload } => {
                        assert_eq!(host, "fixture.host");
                        self.settle(responder, Ok(json!({"response": String::from_utf8(payload).expect("native JSON")})));
                    }
                    _ => self.settle(
                        responder,
                        Err(ApiError::other("Test native host unavailable.")),
                    ),
                },
                Work::Protocol(
                    sender,
                    ProtocolRequest::Respond {
                        message,
                        listener,
                        outcome,
                    },
                ) => {
                    if let Some(settlement) = self
                        .messages
                        .observe(
                            &sender,
                            message,
                            listener,
                            outcome,
                            self.dispatcher.limiter_mut(),
                            &self.host.clock,
                        )
                        .expect("message observation")
                    {
                        self.message_settled(settlement);
                    }
                }
                Work::Protocol(_, _) => {}
            }
        }
    }

    fn cancel(&mut self, label: &str) {
        let sender = self.senders[label].clone();
        let context = sender.context_key();
        self.dispatcher.cancel_context(&context);
        self.host.events.cancel_context(&context);
        self.queue.cancel_context(&context);
        let settlements = self
            .messages
            .cancel_context(&context, self.dispatcher.limiter_mut());
        for settlement in settlements {
            self.message_settled(settlement);
        }
        let actions = self
            .ports
            .cancel_context(&context, self.dispatcher.limiter_mut());
        self.port_actions(actions);
        if let Some(view) = self.views.remove(label) {
            let generation = self
                .host
                .lifecycle
                .generation(install())
                .expect("live generation");
            self.host
                .lifecycle
                .observe(
                    Observation::Crashed {
                        install: install(),
                        generation,
                    },
                    &self.host.clock,
                )
                .expect("background death");
            self.host.sessions.remove(&context);
            self.authority.close_view(view);
            self.host.clock.advance_nanos(1_000_000_000);
        }
        self.senders.remove(label);
    }

    fn command(&mut self, command: &Value) -> Value {
        let label = command["context"].as_str().unwrap_or("background");
        match command["op"].as_str().expect("operation") {
            "context" => self.add_context(label, command["kind"].as_str().expect("kind")),
            "frame" => {
                let bytes = command["frame"]
                    .as_array()
                    .expect("frame")
                    .iter()
                    .map(|byte| u8::try_from(byte.as_u64().expect("byte")).expect("u8"))
                    .collect::<Vec<_>>();
                self.receive(label, &bytes);
            }
            "advance" => {
                self.host
                    .clock
                    .advance_nanos(command["millis"].as_u64().expect("millis") * 1_000_000);
                let environment = self.host.environment();
                for alarm in self.host.alarms.take_due(environment) {
                    for target in self.host.events.targets(
                        install(),
                        "alarms.onAlarm",
                        &EventFacts::default(),
                    ) {
                        self.event(&target, "alarms.onAlarm", json!([alarm]));
                    }
                }
                for settlement in self.messages.expire(&self.host.clock) {
                    self.message_settled(settlement);
                }
            }
            "cancel" => self.cancel(label),
            "restore" => {
                let bytes = serde_json::to_vec(&self.host.events.snapshot(install()))
                    .expect("listener snapshot");
                let mut restored = EventRegistry::new();
                restored
                    .restore(
                        install(),
                        runtime::events::ListenerSnapshot::decode(&bytes).expect("decode snapshot"),
                    )
                    .expect("restore snapshot");
                self.host.events = restored;
            }
            "queue" => {
                assert!(self.host.events.should_wake(
                    install(),
                    "alarms.onAlarm",
                    &EventFacts::default()
                ));
                self.queue
                    .enqueue(
                        "alarms.onAlarm",
                        serde_json::to_vec(&json!([{"name":"replayed","scheduledTime":1_000_000}]))
                            .expect("queued event"),
                        EventImportance::Critical,
                    )
                    .expect("enqueue");
            }
            "replay" => {
                let sender = &self.senders[label];
                if let Some(event) = self.queue.deliver(sender).expect("deliver") {
                    self.host
                        .lifecycle
                        .observe(
                            Observation::EventStarted {
                                install: install(),
                                generation: self
                                    .host
                                    .lifecycle
                                    .generation(install())
                                    .expect("generation"),
                                event: event.sequence.get(),
                            },
                            &self.host.clock,
                        )
                        .expect("event start");
                    self.event_bytes(
                        &sender.context_key(),
                        event.sequence,
                        &event.name,
                        &event.arguments,
                    );
                }
            }
            "stats" => {}
            op => panic!("unknown operation {op}"),
        }
        json!({"queued": self.queue.len(), "acknowledged": self.acknowledged, "pending": self.dispatcher.pending_calls(), "ports": self.ports.port_count(install()), "bytes": self.dispatcher.limiter().install_bytes(install())})
    }
}

fn output(value: Value) {
    println!("\n{PREFIX}{value}");
    std::io::stdout().flush().expect("flush");
}

pub(super) fn serve() {
    let mut peer = Peer::new();
    for line in std::io::stdin().lock().lines() {
        let command: Value = serde_json::from_str(&line.expect("stdin")).expect("command JSON");
        let result = peer.command(&command);
        output(json!({"id": command["id"], "result": result}));
    }
}
