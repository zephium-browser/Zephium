use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, Write};

use crate::FakeBrowser;
use runtime::alarms::{AlarmEnvironment, AlarmHost, Alarms};
use runtime::api::{action, scripting, tabs, web_navigation};
use runtime::browser::{BrowserPort, BrowserRequest, BrowserSnapshot, ExtensionInfo};
use runtime::context::{ClaimedWorld, CommittedDocument, InjectedWorld, TabDocument, ViewKind};
use runtime::dispatch::{encode_for, Responder};
use runtime::envelope::{ApiName, Envelope, EventSeq};
use runtime::events::{
    EventFacts, EventImportance, EventQueue, EventRegistry, EventsHost, ListenerRequest,
};
use runtime::i18n::{I18nHost, LocaleEnvironment, Messages};
use runtime::lifecycle::{Command, Lifecycle, Observation, StartReason};
use runtime::limits::{Clock, ManualClock};
use runtime::pattern::DocumentOrigin;
use runtime::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
use runtime::ports::{
    MessageId, MessageListener, MessageRequest, MessageRoute, MessageService, MessageSettlement,
    MessageTarget, MessagingHost, PortAction, PortService,
};
use runtime::protocol::{ProtocolHost, ProtocolRequest, WorkerSession};
use runtime::scripting_registry::{ResourceKind, ScriptRegistry, ScriptingHost};
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
    browser: FakeBrowser,
    catalog: Messages,
    scripts: ScriptRegistry,
    storage: Storage,
    alarms: Alarms,
    clock: ManualClock,
    events: EventRegistry,
    lifecycle: Lifecycle,
    sessions: HashMap<ContextKey, WorkerSession>,
    work: VecDeque<Work>,
    conformance: bool,
    navigation: VecDeque<web_navigation::NavigationObservation>,
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

impl BrowserPort for Host {
    fn snapshot(&self, sender: &Sender) -> Result<BrowserSnapshot, ApiError> {
        self.browser.snapshot(sender)
    }
    fn extension_info(&self, sender: &Sender) -> Result<ExtensionInfo, ApiError> {
        self.browser.extension_info(sender)
    }
    fn request(&mut self, request: BrowserRequest, call: Invocation<'_>) -> Handled {
        if let BrowserRequest::Scripting(request) = &request {
            if let scripting::Request::Execute {
                source: scripting::ScriptSource::Function { source, args },
                ..
            } = request
            {
                assert_eq!(source, "(value) => value");
                assert_eq!(args, &[Value::Null]);
            }
            let results = request
                .target()
                .frames
                .iter()
                .rev()
                .map(|frame| scripting::FrameResult {
                    frame: frame.clone(),
                    result: Ok(scripting::ExecutionResult::Value(
                        json!({"frame": frame.frame.get(), "value": null}),
                    )),
                })
                .collect();
            let result = scripting::reply(request, results);
            self.browser
                .requests
                .push(BrowserRequest::Scripting(request.clone()));
            return call.reply(result);
        }
        if self.conformance && matches!(request, BrowserRequest::Tabs(_)) {
            let previous = self.browser.snapshot.clone();
            let permissions = call.permissions().clone();
            let handled = self.browser.request(request, call);
            let created = self
                .browser
                .snapshot
                .tabs
                .iter()
                .filter(|tab| !previous.tabs.iter().any(|old| old.id == tab.id))
                .map(|tab| (tab.id, tab.url().expect("tab URL").to_string()))
                .collect::<Vec<_>>();
            for (id, url) in created {
                self.browser
                    .navigate(id, &url, &permissions)
                    .expect("fake load");
            }
            for tab in &mut self.browser.snapshot.tabs {
                if previous
                    .tabs
                    .iter()
                    .find(|old| old.id == tab.id)
                    .is_none_or(|old| old.document != tab.document)
                {
                    let url = tab.url().expect("tab URL");
                    let title = if url.path() == "/page.html" {
                        "Fixture"
                    } else {
                        tab.tab.title()
                    };
                    tab.tab = zephium_core::extensions::ExtensionBrowserTab::from_snapshot(
                        Some(&tab.tab),
                        tab.tab.id(),
                        tab.tab.resident(),
                        title,
                        tab.tab.url().and_then(|url| Url::parse(url).ok()).as_ref(),
                        tab.tab.loading(),
                        tab.tab.pinned(),
                    )
                    .expect("fixture title");
                    self.browser
                        .frames
                        .retain(|frame| frame.tab != tab.tab.id());
                    let frame = web_navigation::BrowserFrame {
                        tab: tab.tab.id(),
                        frame: FrameId::MAIN,
                        document: tab.document,
                        document_id: tab.document_id.clone(),
                        origin: DocumentOrigin::of_url(&url),
                        url,
                        parent_frame_id: None,
                        parent_document_id: None,
                        document_lifecycle: "active".into(),
                        frame_type: "outermost_frame".into(),
                        error_occurred: false,
                    };
                    self.browser.frames.push(frame.clone());
                    for event in [
                        web_navigation::NavigationEvent::BeforeNavigate,
                        web_navigation::NavigationEvent::Committed,
                        web_navigation::NavigationEvent::DomContentLoaded,
                        web_navigation::NavigationEvent::Completed,
                    ] {
                        self.navigation
                            .push_back(web_navigation::NavigationObservation {
                                tab_id: tab.id,
                                frame: frame.clone(),
                                event,
                                time_stamp: 1_000_000.0,
                                transition_type: (event
                                    == web_navigation::NavigationEvent::Committed)
                                    .then(|| "link".into()),
                                transition_qualifiers: vec![],
                                error: None,
                            });
                    }
                }
            }
            return handled;
        }
        use runtime::browser::ServiceRequest;
        use zephium_core::extension_data::{ExtensionDataCall, ExtensionDataReply};
        let result = match &request {
            BrowserRequest::Data(ExtensionDataCall::Bookmarks(_)) => {
                runtime::api::bookmarks::reply(
                    call.name(),
                    call.args(),
                    &ExtensionDataReply::Bookmarks(
                        zephium_core::bookmarks::BookmarkReply::Entries(vec![]),
                    ),
                )
            }
            BrowserRequest::Data(ExtensionDataCall::RecentlyClosed { .. }) => {
                runtime::api::sessions::reply(
                    call.name(),
                    &ExtensionDataReply::Closed(vec![]),
                    None,
                )
            }
            BrowserRequest::Notifications(runtime::api::notifications::Request::GetAll) => {
                Ok(json!({}))
            }
            BrowserRequest::Service(ServiceRequest::QueryIdle(_)) => Ok(json!("active")),
            BrowserRequest::Service(ServiceRequest::FontList) => {
                Ok(json!([{"fontId":"Fixture Mono","displayName":"Fixture Mono"}]))
            }
            BrowserRequest::Service(ServiceRequest::Search(search)) => {
                let tab = self
                    .browser
                    .snapshot
                    .tabs
                    .iter()
                    .find(|tab| Some(tab.tab.id()) == search.tab)
                    .expect("search tab")
                    .id;
                let mut url = Url::parse("https://example.com/search").expect("search URL");
                url.query_pairs_mut().append_pair("q", &search.text);
                self.browser
                    .navigate(tab, url.as_str(), call.permissions())
                    .map(|()| Value::Null)
            }
            _ => return self.browser.request(request, call),
        };
        self.browser.requests.push(request);
        call.reply(result)
    }
    fn frames(
        &self,
        sender: &Sender,
        tab: ItemId,
    ) -> Result<Vec<web_navigation::BrowserFrame>, ApiError> {
        self.browser.frames(sender, tab)
    }
    fn validate_resource(&self, sender: &Sender, path: &str) -> Result<(), ApiError> {
        self.browser.validate_resource(sender, path)
    }
    fn now_ms(&self) -> u64 {
        self.browser.now_ms()
    }
}

impl I18nHost for Host {
    fn messages(&self, _: &Sender) -> Result<&Messages, ApiError> {
        Ok(&self.catalog)
    }
}

impl ScriptingHost for Host {
    fn script_registry(&self, _: &Sender) -> Result<&ScriptRegistry, ApiError> {
        Ok(&self.scripts)
    }
    fn validate_script_resource(
        &self,
        sender: &Sender,
        path: &str,
        _: ResourceKind,
    ) -> Result<(), ApiError> {
        self.browser.validate_resource(sender, path)
    }
    fn commit_script_registry(
        &mut self,
        _: &Sender,
        registry: ScriptRegistry,
    ) -> Result<(), ApiError> {
        self.scripts = registry;
        Ok(())
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
    fn alarm_call(
        &mut self,
        sender: &Sender,
        method: &str,
        args: &[Value],
    ) -> Result<Value, ApiError> {
        let environment = AlarmEnvironment::now(
            &self.clock,
            1_000_000 + self.clock.now_nanos() / 1_000_000,
            sender.unpacked(),
        );
        let (value, warnings) = self
            .alarms
            .handle_with_warnings(method, args, environment)?;
        if self.conformance {
            for warning in warnings {
                output(json!({"console": {"type": "warning", "text": warning}}));
            }
        }
        Ok(value)
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
                unpacked: false,
                externally_connectable: None,
            })
            .expect("admit");
        let permissions = [
            "storage",
            "alarms",
            "nativeMessaging",
            "tabs",
            "activeTab",
            "scripting",
            "webNavigation",
            "history",
            "bookmarks",
            "sessions",
            "topSites",
            "search",
            "notifications",
            "idle",
            "fontSettings",
        ];
        let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
            json!({"permissions": permissions, "host_permissions": ["https://example.com/*"], "optional_permissions": ["downloads"], "action": {}, "commands": {}}),
        ));
        let permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let mut table = ApiTable::new();
        runtime::api::register(&mut table).expect("all namespaces");
        runtime::protocol::register(&mut table).expect("binding protocol");
        let mut lifecycle = Lifecycle::new(profile());
        assert!(lifecycle.register(install()));
        let mut browser = FakeBrowser::new(profile());
        let window = browser.add_window(false);
        browser.add_tab(window, "https://example.com/", true);
        browser.add_tab(window, "https://other.test/", false);
        for tab in &browser.snapshot.tabs {
            browser.frames.push(web_navigation::BrowserFrame {
                tab: tab.tab.id(),
                frame: FrameId::MAIN,
                document: tab.document,
                document_id: tab.document_id.clone(),
                url: tab.url().expect("URL"),
                origin: DocumentOrigin::of_url(&tab.url().expect("URL")),
                parent_frame_id: None,
                parent_document_id: None,
                document_lifecycle: "active".into(),
                frame_type: "outermost_frame".into(),
                error_occurred: false,
            });
        }
        let mut child = browser.frames[0].clone();
        child.frame = FrameId::new(3);
        child.document = DocumentGeneration::new(3).expect("child generation");
        child.document_id = "child-document".into();
        child.parent_frame_id = Some(FrameId::MAIN);
        child.parent_document_id = Some(browser.frames[0].document_id.clone());
        child.frame_type = "sub_frame".into();
        browser.frames.push(child);
        browser.info.manifest =
            json!({"manifest_version":3,"action":{},"name":"Fixture","version":"1"});
        browser.resources.insert("script.js".into());
        Self {
            authority,
            host: Host {
                browser,
                catalog: Messages::new(
                    ExtensionId::parse(EXTENSION).expect("extension"),
                    LocaleEnvironment {
                        ui_language: "en".into(),
                        accept_languages: vec!["en".into()],
                        right_to_left: false,
                    },
                    None,
                    &Default::default(),
                )
                .expect("messages"),
                scripts: ScriptRegistry::default(),
                storage: Storage::memory(profile(), install(), false),
                alarms: Alarms::new(),
                clock: ManualClock::default(),
                events: EventRegistry::new(),
                lifecycle,
                sessions: HashMap::new(),
                work: VecDeque::new(),
                conformance: false,
                navigation: VecDeque::new(),
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

    fn add_context(&mut self, label: &str, kind: &str, frame_id: u64, tab_id: u64) {
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
        } else if kind == "extension" {
            let view = self
                .authority
                .open_view(install(), ViewKind::Options)
                .expect("extension view");
            self.authority
                .bind_view(
                    &view,
                    &document(&format!("chrome-extension://{EXTENSION}/runner.html"), 100),
                )
                .expect("extension sender")
        } else {
            let frame = self
                .host
                .browser
                .frames
                .iter()
                .find(|frame| {
                    frame.tab == ItemId::from(tab_id as u128) && frame.frame.get() == frame_id
                })
                .expect("content frame");
            let tab = frame.tab;
            let document = TabDocument {
                tab,
                frame: frame.frame,
                committed: CommittedDocument {
                    generation: frame.document,
                    origin: frame.origin.clone(),
                    url: frame.url.clone(),
                },
            };
            self.authority
                .record_injection(
                    tab,
                    document.frame,
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
        while let Some(observation) = self.host.navigation.pop_front() {
            let facts = web_navigation::event_facts(&observation.frame, observation.tab_id);
            for target in self
                .host
                .events
                .targets(install(), observation.event.name(), &facts)
            {
                let sender = self
                    .senders
                    .values()
                    .find(|sender| sender.context_key() == target)
                    .expect("navigation sender");
                if let Ok(mut args) =
                    web_navigation::observation(&self.host, sender, &self.permissions, &observation)
                {
                    let listeners =
                        self.host
                            .events
                            .listeners(sender, observation.event.name(), &facts);
                    args.as_array_mut()
                        .expect("event args")
                        .push(json!(listeners
                            .iter()
                            .map(|id| id.get())
                            .collect::<Vec<_>>()));
                    self.event(&target, observation.event.name(), args);
                }
            }
        }
        for (name, args) in std::mem::take(&mut self.host.browser.events) {
            for target in self
                .host
                .events
                .targets(install(), &name, &EventFacts::default())
            {
                let mut args = args.clone();
                if matches!(
                    name.as_str(),
                    "windows.onCreated" | "windows.onRemoved" | "windows.onFocusChanged"
                ) {
                    let sender = self
                        .senders
                        .values()
                        .find(|sender| sender.context_key() == target)
                        .expect("window event sender");
                    let listeners =
                        self.host
                            .events
                            .listeners(sender, &name, &EventFacts::default());
                    args.as_array_mut()
                        .expect("window arguments")
                        .push(json!(listeners
                            .iter()
                            .map(|id| id.get())
                            .collect::<Vec<_>>()));
                }
                self.event(&target, &name, args);
            }
        }
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
                        let mut documents = None;
                        let route = match target {
                            MessageTarget::Runtime { extension_id }
                                if extension_id
                                    .as_ref()
                                    .is_none_or(|id| id == source.extension_id()) =>
                            {
                                MessageRoute::Runtime
                            }
                            MessageTarget::ResolvedTab { routes } => {
                                let Some(MessageRoute::Tab { tab, .. }) = routes.first() else {
                                    panic!("resolved tab routes")
                                };
                                let route = MessageRoute::Tab {
                                    tab: *tab,
                                    frame: None,
                                    document: None,
                                };
                                documents = Some(routes);
                                route
                            }
                            _ => {
                                self.settle(responder, Err(runtime::ports::no_receiver()));
                                continue;
                            }
                        };
                        let listeners = self
                            .senders
                            .values()
                            .filter(|sender| documents.as_ref().is_none_or(|routes| routes.iter().any(|route| {
                                matches!(route, MessageRoute::Tab { tab, frame, document } if sender.tab() == Some(*tab) && sender.frame() == *frame && Some(sender.document()) == *document)
                            })))
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

    fn configure(&mut self, command: &Value) -> Value {
        assert!(self.senders.is_empty(), "configure before contexts");
        let manifest = command["manifest"].clone();
        self.authority = Authority::new(profile(), false, ExtensionScheme::CHROME_EXTENSION);
        self.authority
            .admit(InstallAdmission {
                install: install(),
                extension_id: ExtensionId::parse(EXTENSION).expect("extension"),
                generation: ExtensionRuntimeGeneration::new(1).expect("generation"),
                allowed_in_incognito: false,
                unpacked: true,
                externally_connectable: None,
            })
            .expect("conformance installation");
        let (declaration, _) =
            PermissionDeclaration::from_manifest(&Manifest::from_value(manifest.clone()));
        self.permissions = InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        );
        let catalogs: BTreeMap<String, Value> =
            serde_json::from_value(command["catalogs"].clone()).expect("catalogs");
        self.host.catalog = Messages::new(
            ExtensionId::parse(EXTENSION).expect("extension"),
            LocaleEnvironment {
                ui_language: "en-US".into(),
                accept_languages: vec!["en-US".into(), "en".into()],
                right_to_left: false,
            },
            manifest["default_locale"].as_str(),
            &catalogs,
        )
        .expect("catalog");
        let mut localized = manifest.clone();
        for key in ["name", "description"] {
            if let Some(text) = manifest[key].as_str() {
                localized[key] = self
                    .host
                    .catalog
                    .localize(text, true)
                    .expect("localized manifest")
                    .into();
            }
        }
        self.host.browser = FakeBrowser::new(profile());
        let window = self.host.browser.add_window(false);
        self.host.browser.add_tab(window, "about:blank", false);
        self.host.browser.add_tab(
            window,
            &format!("chrome-extension://{EXTENSION}/runner.html"),
            true,
        );
        self.host.browser.resources =
            serde_json::from_value(command["resources"].clone()).expect("resources");
        self.host.browser.info.manifest = localized.clone();
        self.host.browser.info.commands = manifest["commands"].as_object().map(|commands| commands.iter().map(|(name, command)| json!({"name": name, "description": command["description"].as_str().unwrap_or(""), "shortcut": command["suggested_key"][std::env::consts::OS].as_str().or_else(|| command["suggested_key"]["default"].as_str()).unwrap_or("")})).collect()).unwrap_or_default();
        self.host.conformance = true;
        json!({"manifest": localized, "messages": self.host.catalog.catalog(), "uiLanguage": self.host.catalog.ui_language()})
    }

    fn command(&mut self, command: &Value) -> Value {
        let label = command["context"].as_str().unwrap_or("background");
        match command["op"].as_str().expect("operation") {
            "configure" => return self.configure(command),
            "context" => self.add_context(
                label,
                command["kind"].as_str().expect("kind"),
                command["frameId"].as_u64().unwrap_or(0),
                command["tabId"].as_u64().unwrap_or(1),
            ),
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
                if command["requestedDue"] == true {
                    let now = self.host.environment();
                    if self
                        .host
                        .alarms
                        .get_all()
                        .iter()
                        .any(|alarm| alarm.scheduled_time <= now.unix_millis as f64)
                    {
                        if let Some(deadline) = self.host.alarms.next_wake() {
                            self.host
                                .clock
                                .advance_nanos(deadline.saturating_sub(now.monotonic_nanos));
                        }
                    }
                }
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
            "restrictPermissions" => {
                let manifest = Manifest::from_value(
                    json!({"permissions": ["storage", "alarms", "activeTab", "scripting", "webNavigation"], "optional_permissions": ["downloads"], "action": {}}),
                );
                let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
                self.permissions = InstallPermissions::new(
                    declaration,
                    &PermissionRequest::default(),
                    SiteAccess::OnAllSites,
                    false,
                );
            }
            "click" => {
                let tab = self
                    .host
                    .browser
                    .snapshot
                    .tab(command["tabId"].as_i64().expect("tab") as i32)
                    .expect("tab");
                assert_eq!(
                    self.host
                        .browser
                        .action
                        .trusted_invoke(tab, &mut self.permissions)
                        .expect("action click"),
                    action::Activation::Click
                );
                let args = json!([tabs::project_for(
                    tab,
                    &self.senders[label],
                    &self.permissions
                )]);
                for target in
                    self.host
                        .events
                        .targets(install(), "action.onClicked", &EventFacts::default())
                {
                    self.event(&target, "action.onClicked", args.clone());
                }
            }
            "navigation" => {
                let frame = self
                    .host
                    .browser
                    .frames
                    .iter()
                    .find(|frame| {
                        frame.tab == ItemId::from(command["tabId"].as_u64().expect("tab") as u128)
                            && frame.frame == FrameId::MAIN
                    })
                    .expect("frame")
                    .clone();
                let observation = web_navigation::NavigationObservation {
                    tab_id: command["tabId"].as_i64().expect("tab") as i32,
                    frame,
                    event: web_navigation::NavigationEvent::Committed,
                    time_stamp: 123.5,
                    transition_type: Some("link".into()),
                    transition_qualifiers: vec![],
                    error: None,
                };
                let facts = web_navigation::event_facts(&observation.frame, observation.tab_id);
                for target in self
                    .host
                    .events
                    .targets(install(), observation.event.name(), &facts)
                {
                    let sender = self
                        .senders
                        .values()
                        .find(|sender| sender.context_key() == target)
                        .expect("sender");
                    if let Ok(mut args) = web_navigation::observation(
                        &self.host,
                        sender,
                        &self.permissions,
                        &observation,
                    ) {
                        let listeners =
                            self.host
                                .events
                                .listeners(sender, observation.event.name(), &facts);
                        args.as_array_mut()
                            .expect("event args")
                            .push(json!(listeners
                                .iter()
                                .map(|id| id.get())
                                .collect::<Vec<_>>()));
                        self.event(&target, observation.event.name(), args);
                    }
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
