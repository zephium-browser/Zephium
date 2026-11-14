use super::*;
use crate::context::tests::{background_sender, content_sender, install, profile};
use crate::dispatch::{Dispatcher, Inbound};
use crate::envelope::{CallId, Direction};
use crate::events::{EventFacts, EventsHost};
use crate::lifecycle::StartReason;
use crate::limits::{Limiter, ManualClock};
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};
use crate::ports::{MessageListener, MessageRoute, MessageService, MessageSettlement};
use zephium_webext::manifest::Manifest;

#[derive(Default)]
struct Host {
    browser: Option<crate::browser::fake::FakeBrowser>,
    events: EventRegistry,
    messages: MessageService,
    limiter: Limiter,
    clock: ManualClock,
    settlements: Vec<MessageSettlement>,
    reports: usize,
    message_request: Option<crate::ports::MessageRequest>,
}

impl crate::browser::BrowserPort for Host {
    fn snapshot(&self, sender: &Sender) -> Result<crate::browser::BrowserSnapshot, ApiError> {
        self.browser
            .as_ref()
            .ok_or_else(crate::browser::unsupported)?
            .snapshot(sender)
    }
    fn frames(
        &self,
        sender: &Sender,
        tab: zephium_core::ids::ItemId,
    ) -> Result<Vec<crate::api::web_navigation::BrowserFrame>, ApiError> {
        self.browser
            .as_ref()
            .ok_or_else(crate::browser::unsupported)?
            .frames(sender, tab)
    }
    fn extension_info(&self, _: &Sender) -> Result<crate::browser::ExtensionInfo, ApiError> {
        Ok(crate::browser::ExtensionInfo::default())
    }
    fn request(&mut self, _: crate::browser::BrowserRequest, call: Invocation<'_>) -> Handled {
        call.reply(Err(crate::browser::unsupported()))
    }
}

impl EventsHost for Host {
    fn listener_request(
        &mut self,
        request: ListenerRequest,
        invocation: Invocation<'_>,
    ) -> Handled {
        let result = match request {
            ListenerRequest::Add {
                event,
                id,
                filter,
                extra,
            } => self
                .events
                .add_with_options(invocation.sender(), id, event, filter.as_ref(), &extra)
                .map(Value::Bool),
            ListenerRequest::Remove { event, id } => Ok(Value::Bool(self.events.remove(
                invocation.sender(),
                event,
                id,
            ))),
            ListenerRequest::CompleteRegistration => self
                .events
                .finish_registration(invocation.sender())
                .map(|()| Value::Null),
            _ => Err(ApiError::unknown_method()),
        };
        invocation.reply(result)
    }
}

impl ProtocolHost for Host {
    fn protocol_request(
        &mut self,
        request: ProtocolRequest,
        invocation: Invocation<'_>,
    ) -> Handled {
        self.reports += 1;
        let result = match request {
            ProtocolRequest::Respond {
                message,
                listener,
                outcome,
            } => self
                .messages
                .observe(
                    invocation.sender(),
                    message,
                    listener,
                    outcome,
                    &mut self.limiter,
                    &self.clock,
                )
                .map(|settlement| {
                    self.settlements.extend(settlement);
                    Value::Null
                }),
            _ => Ok(Value::Null),
        };
        invocation.reply(result)
    }
}

impl crate::ports::MessagingHost for Host {
    fn message_request(
        &mut self,
        request: crate::ports::MessageRequest,
        invocation: Invocation<'_>,
    ) -> Handled {
        self.message_request = Some(request);
        invocation.reply(Ok(Value::Null))
    }
}

fn permissions(names: &[&str]) -> InstallPermissions {
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"manifest_version":3,"permissions":names}),
    ));
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

fn table() -> ApiTable<Host> {
    let mut table = ApiTable::new();
    register(&mut table).unwrap();
    table
}

fn invoke(
    dispatcher: &mut Dispatcher<Host>,
    host: &mut Host,
    sender: &Sender,
    permissions: &InstallPermissions,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    let args = serde_json::to_vec(&args).unwrap();
    let bytes = Envelope::Call {
        id: CallId::new(1).unwrap(),
        name: ApiName::parse(name).unwrap(),
        args: &args,
    }
    .encode(Direction::FromContext, u64::MAX)
    .unwrap();
    let Inbound::Reply(reply) = dispatcher.receive(host, permissions, sender, &bytes, 0) else {
        panic!("expected reply");
    };
    let Envelope::Reply { result, .. } =
        Envelope::decode(&reply, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("expected envelope reply");
    };
    result
        .map(|bytes| serde_json::from_slice(bytes).unwrap())
        .map_err(str::to_owned)
}

#[test]
fn internal_registration_preserves_event_permissions_contexts_and_ids() {
    let background = background_sender();
    let content = content_sender("https://example.com/");
    let mut host = Host::default();
    let mut dispatcher = Dispatcher::new(table());
    for args in [
        json!(["alarms.onAlarm", null, null, null]),
        json!(["alarms.onAlarm", 0, null, null]),
        json!(["alarms.onAlarm", 9_007_199_254_740_992_u64, null, null]),
        json!(["zephium.onInvalidated", 1, null, null]),
    ] {
        assert!(invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &permissions(&["alarms"]),
            "zephium.addListener",
            args
        )
        .is_err());
    }
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &background,
        &permissions(&[]),
        "zephium.addListener",
        json!(["alarms.onAlarm", 1, null, null])
    )
    .is_err());
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &content,
        &permissions(&["alarms"]),
        "zephium.addListener",
        json!(["alarms.onAlarm", 1, null, null])
    )
    .is_err());
    assert_eq!(
        invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &permissions(&["alarms"]),
            "zephium.addListener",
            json!(["alarms.onAlarm", 1, null, null])
        ),
        Ok(json!(true))
    );
    assert_eq!(
        invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &permissions(&["alarms"]),
            "zephium.removeListener",
            json!(["alarms.onAlarm", 1])
        ),
        Ok(json!(true))
    );
    assert!(!host.events.has_listeners(&background, "alarms.onAlarm"));
    assert!(!crate::schema::contains("zephium.addListener"));
    for name in [
        "zephium.workerLifecycle",
        "zephium.workerError",
        "events.completeRegistration",
        "runtime.markListenerRegistrationComplete",
    ] {
        assert_eq!(
            dispatcher.table().contexts(name),
            Some(ContextMask::of(ContextKind::Background))
        );
    }
    for args in [json!(["activated", null]), json!(["failed", "error"])] {
        assert!(invoke(
            &mut dispatcher,
            &mut host,
            &content,
            &permissions(&[]),
            "zephium.workerLifecycle",
            args
        )
        .is_err());
    }
    assert_eq!(host.reports, 0);
}

#[test]
fn both_registration_aliases_reach_durable_interests_and_work_in_either_order() {
    let background = background_sender();
    for name in [
        "events.completeRegistration",
        "runtime.markListenerRegistrationComplete",
    ] {
        let mut host = Host::default();
        host.events.begin_registration(&background).unwrap();
        host.events
            .add(
                &background,
                ListenerId::new(1).unwrap(),
                "runtime.onStartup",
                None,
            )
            .unwrap();
        let mut dispatcher = Dispatcher::new(table());
        assert_eq!(
            invoke(
                &mut dispatcher,
                &mut host,
                &background,
                &permissions(&[]),
                name,
                json!([])
            ),
            Ok(Value::Null)
        );
        assert!(host
            .events
            .should_wake(install(), "runtime.onStartup", &EventFacts::default()));
        assert!(invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &permissions(&[]),
            name,
            json!([1])
        )
        .is_err());
    }
    let mut table = table();
    crate::events::register(&mut table).unwrap();
    let mut table = ApiTable::<Host>::new();
    crate::events::register(&mut table).unwrap();
    register(&mut table).unwrap();
}

#[test]
fn response_outcomes_reach_message_service_with_exact_context_authority() {
    let background = background_sender();
    let content = content_sender("https://example.com/");
    let id = ListenerId::new(1).unwrap();
    let mut host = Host::default();
    let (message, deliveries) = host
        .messages
        .send(
            &content,
            &MessageRoute::Runtime,
            &[MessageListener {
                sender: background.clone(),
                listener: id,
            }],
            b"{\"hello\":1}",
            &mut host.limiter,
            &host.clock,
        )
        .unwrap();
    assert_eq!(
        deliveries[0].arguments(json!({})).unwrap(),
        json!(["{\"hello\":1}", {}, message.get().to_string(), [1]])
    );
    let mut dispatcher = Dispatcher::new(table());
    assert!(invoke(
        &mut dispatcher,
        &mut host,
        &content,
        &permissions(&[]),
        "zephium.respond",
        json!([message.get().to_string(),1,{"response":"true"}])
    )
    .is_err());
    assert!(host.settlements.is_empty());
    assert_eq!(
        invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &permissions(&[]),
            "zephium.respond",
            json!([message.get().to_string(),1,{"async":"promise"}])
        ),
        Ok(Value::Null)
    );
    assert_eq!(
        invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &permissions(&[]),
            "zephium.respond",
            json!([message.get().to_string(),1,{"response":"true"}])
        ),
        Ok(Value::Null)
    );
    assert_eq!(host.settlements[0].result, Ok(Some(b"true".to_vec())));
    assert!(host
        .messages
        .acknowledge(&content, message, &mut host.limiter));
    assert_eq!(host.limiter.context_bytes(&background.context_key()), 0);
}

#[test]
fn malformed_internal_reports_never_reach_the_host() {
    let background = background_sender();
    let mut host = Host::default();
    let mut dispatcher = Dispatcher::new(table());
    for (method, args) in [
        ("zephium.respond", json!(["01", 1, null])),
        ("zephium.respond", json!(["18446744073709551616", 1, null])),
        (
            "zephium.respond",
            json!(["1",1,{"response":"true","error":"bad"}]),
        ),
        ("zephium.respond", json!(["1",1,{"async":"yes"}])),
        ("zephium.eventResult", json!([1, 1, {}])),
        ("zephium.workerLifecycle", json!(["running", null])),
        (
            "zephium.workerLifecycle",
            json!(["activated", "unexpected"]),
        ),
        (
            "zephium.workerLifecycle",
            json!(["failed", "x".repeat(1025)]),
        ),
        (
            "zephium.workerError",
            json!([{"message":"bad","filename":"","lineno":-1,"colno":0}]),
        ),
    ] {
        assert!(
            invoke(
                &mut dispatcher,
                &mut host,
                &background,
                &permissions(&[]),
                method,
                args
            )
            .is_err(),
            "{method}"
        );
    }
    assert_eq!(host.reports, 0);
}

#[test]
fn lifecycle_success_commits_registrations_and_stale_worker_cannot_revive_itself() {
    let background = background_sender();
    let clock = ManualClock::default();
    let mut lifecycle = Lifecycle::new(profile());
    lifecycle.register(install());
    lifecycle
        .observe(
            Observation::Wake {
                install: install(),
                reason: StartReason::Install,
            },
            &clock,
        )
        .unwrap();
    let generation = lifecycle.generation(install()).unwrap();
    let session = WorkerSession::new(&background, &lifecycle, generation).unwrap();
    let mut events = EventRegistry::new();
    events.begin_registration(&background).unwrap();
    events
        .add(
            &background,
            ListenerId::new(1).unwrap(),
            "runtime.onStartup",
            None,
        )
        .unwrap();
    session
        .observe(
            &background,
            WorkerLifecycle::Activated,
            &mut lifecycle,
            &mut events,
            &clock,
        )
        .unwrap();
    assert_eq!(lifecycle.state(install()), Some(BackgroundState::Running));
    assert!(events.should_wake(install(), "runtime.onStartup", &EventFacts::default()));
    let effects = session
        .error(&background, &mut lifecycle, &mut events, &clock)
        .unwrap();
    assert!(matches!(
        effects.last(),
        Some(Command::Terminate {
            reason: crate::lifecycle::StopReason::WorkerFailure,
            ..
        })
    ));
    assert_eq!(
        lifecycle.state(install()),
        Some(BackgroundState::Terminating)
    );
    assert_eq!(lifecycle.occupied_slots(), 1);
    assert!(session
        .observe(
            &background,
            WorkerLifecycle::Activated,
            &mut lifecycle,
            &mut events,
            &clock
        )
        .is_err());
    assert!(events.should_wake(install(), "runtime.onStartup", &EventFacts::default()));
    lifecycle
        .observe(
            Observation::Terminated {
                install: install(),
                generation,
            },
            &clock,
        )
        .unwrap();
    assert_eq!(lifecycle.state(install()), Some(BackgroundState::Crashed));
    assert_eq!(lifecycle.occupied_slots(), 0);
}

#[test]
fn blocking_result_authority_is_exact_one_shot_and_expires() {
    let background = background_sender();
    let content = content_sender("https://example.com/");
    let clock = ManualClock::default();
    let mut events = EventRegistry::new();
    let listener = ListenerId::new(1).unwrap();
    events
        .add_with_options(
            &background,
            listener,
            "webRequest.onAuthRequired",
            Some(&json!({"urls":["<all_urls>"]})),
            &["asyncBlocking".into()],
        )
        .unwrap();
    let sequence = EventSeq::new(1).unwrap();
    let mut results = EventResults::new();
    results
        .expect(
            &background,
            &events,
            "webRequest.onAuthRequired",
            sequence,
            listener,
            &clock,
        )
        .unwrap();
    assert!(results.take(&content, sequence, listener, &clock).is_err());
    results
        .take(&background, sequence, listener, &clock)
        .unwrap();
    assert!(results
        .take(&background, sequence, listener, &clock)
        .is_err());
    results
        .expect(
            &background,
            &events,
            "webRequest.onAuthRequired",
            sequence,
            listener,
            &clock,
        )
        .unwrap();
    results.cancel_context(&background.context_key());
    assert!(results
        .take(&background, sequence, listener, &clock)
        .is_err());
    results
        .expect(
            &background,
            &events,
            "webRequest.onAuthRequired",
            sequence,
            listener,
            &clock,
        )
        .unwrap();
    clock.advance_nanos(EVENT_TIMEOUT_NANOS);
    assert!(results
        .take(&background, sequence, listener, &clock)
        .is_err());
}

#[test]
fn private_notifications_encode_expected_binding_arguments() {
    let background = background_sender();
    let seq = EventSeq::new(9).unwrap();
    let bytes = invalidated(&background, seq).unwrap();
    let Envelope::Event { name, args, .. } =
        Envelope::decode(&bytes, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("event");
    };
    assert_eq!(name.as_str(), ON_INVALIDATED);
    assert_eq!(
        serde_json::from_slice::<Value>(args).unwrap(),
        json!(["Extension context invalidated."])
    );
    let bytes = permissions_changed(&background, seq, &["storage".into()]).unwrap();
    let Envelope::Event { name, args, .. } =
        Envelope::decode(&bytes, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("event");
    };
    assert_eq!(name.as_str(), ON_PERMISSIONS_CHANGED);
    assert_eq!(
        serde_json::from_slice::<Value>(args).unwrap(),
        json!([["storage"]])
    );
    assert!(permissions_changed(&background, seq, &["bad permission".into()]).is_err());
}

#[test]
fn dispatcher_preserves_opaque_message_text_and_rejects_object_wire_payloads() {
    let background = background_sender();
    let mut host = Host::default();
    let mut table = table();
    crate::ports::register(&mut table).unwrap();
    let mut dispatcher = Dispatcher::new(table);
    let granted =
        crate::api::scripting::tests::permissions(&["nativeMessaging"], &["https://example.com/*"]);
    host.browser = Some(crate::api::scripting::tests::fixture());
    for (method, args, text) in [
        (
            "runtime.sendMessage",
            json!([null, "  {\"key\":true}  ", null]),
            "  {\"key\":true}  ",
        ),
        ("tabs.sendMessage", json!([1, "{invalid", null]), "{invalid"),
        (
            "runtime.sendNativeMessage",
            json!(["org.example.host", "not JSON"]),
            "not JSON",
        ),
    ] {
        assert_eq!(
            invoke(
                &mut dispatcher,
                &mut host,
                &background,
                &granted,
                method,
                args
            ),
            Ok(Value::Null)
        );
        let payload = match host.message_request.take().unwrap() {
            crate::ports::MessageRequest::Send { payload, .. }
            | crate::ports::MessageRequest::SendNative { payload, .. } => payload,
            other => panic!("expected send, got {other:?}"),
        };
        assert_eq!(payload, text.as_bytes());
    }
    for (method, args) in [
        ("runtime.sendMessage", json!([null, {}, null])),
        ("tabs.sendMessage", json!([3, {}, null])),
        ("runtime.sendNativeMessage", json!(["org.example.host", {}])),
        ("runtime.sendNativeMessage", json!(["bad/host", "{}"])),
        ("tabs.sendMessage", json!(["3", "{}", null])),
    ] {
        assert!(invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &granted,
            method,
            args
        )
        .is_err());
        assert!(host.message_request.is_none());
    }
}

#[test]
fn window_filter_wire_registration_uses_host_window_facts_and_rejects_bad_values() {
    let background = background_sender();
    let mut host = Host::default();
    let mut dispatcher = Dispatcher::new(table());
    let granted = permissions(&[]);
    assert_eq!(
        invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &granted,
            "zephium.addListener",
            json!(["windows.onCreated",1,{"windowTypes":["popup"]},null])
        ),
        Ok(Value::Bool(true))
    );
    let id = ListenerId::new(1).unwrap();
    assert_eq!(
        host.events.listeners(
            &background,
            "windows.onCreated",
            &EventFacts {
                window_type: Some("popup"),
                ..EventFacts::default()
            }
        ),
        [id]
    );
    assert!(host
        .events
        .listeners(
            &background,
            "windows.onCreated",
            &EventFacts {
                window_type: Some("normal"),
                ..EventFacts::default()
            }
        )
        .is_empty());
    assert!(host
        .events
        .listeners(&background, "windows.onCreated", &EventFacts::default())
        .is_empty());
    for filter in [
        json!({"windowTypes":"popup"}),
        json!({"windowTypes":["invented"]}),
    ] {
        assert!(invoke(
            &mut dispatcher,
            &mut host,
            &background,
            &granted,
            "zephium.addListener",
            json!(["windows.onCreated", 2, filter, null])
        )
        .is_err());
    }
    assert!(!host.events.has_listener(
        &background,
        ListenerId::new(2).unwrap(),
        "windows.onCreated"
    ));
}
