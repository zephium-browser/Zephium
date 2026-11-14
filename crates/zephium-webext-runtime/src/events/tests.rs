use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::context::tests::{
    authority, background_sender, committed, content_sender, install, profile, EXTENSION,
};
use crate::context::ViewKind;
use crate::storage::{AccessLevel, Area, Mutation, Storage};

fn id(value: u64) -> ListenerId {
    ListenerId::new(value).expect("id")
}

fn popup_sender() -> Sender {
    let mut authority = authority();
    let view = authority.open_view(install(), ViewKind::Popup).unwrap();
    authority
        .bind_view(
            &view,
            &committed(&format!("chrome-extension://{EXTENSION}/popup.html"), 2),
        )
        .unwrap()
}

#[test]
fn callback_identity_is_scoped_to_event_and_context() {
    let sender = background_sender();
    let popup = popup_sender();
    let mut registry = EventRegistry::new();
    for event in ["runtime.onMessage", "runtime.onConnect"] {
        registry.add(&sender, id(1), event, None).unwrap();
    }
    registry
        .add(&popup, id(1), "runtime.onMessage", None)
        .unwrap();
    assert!(!registry
        .add(&sender, id(1), "runtime.onMessage", None)
        .unwrap());
    assert!(registry.remove(&sender, "runtime.onMessage", id(1)));
    assert!(!registry.has_listener(&sender, id(1), "runtime.onMessage"));
    assert!(registry.has_listener(&sender, id(1), "runtime.onConnect"));
    assert!(registry.has_listener(&popup, id(1), "runtime.onMessage"));
    assert!(!registry.has_listeners(&sender, "runtime.onMessage"));
}

#[test]
fn web_request_filters_preserve_the_path_and_query() {
    let filter = ListenerFilter::parse(
        "webRequest.onBeforeRequest",
        Some(&json!({"urls":["https://example.com/allowed/*?q=*"]})),
    )
    .unwrap();
    for (url, expected) in [
        ("https://example.com/allowed/page?q=1", true),
        ("https://example.com/private/page?q=1", false),
        ("https://example.com/allowed/page?other=1", false),
    ] {
        let url = url::Url::parse(url).unwrap();
        assert_eq!(
            filter.matches(&EventFacts {
                url: Some(&url),
                ..EventFacts::default()
            }),
            expected
        );
    }
}

#[test]
fn storage_events_require_current_area_access_at_delivery() {
    let trusted = background_sender();
    let content = content_sender("https://example.com/");
    let mut storage = Storage::memory(profile(), install(), false);
    let mut registry = EventRegistry::new();
    for sender in [&trusted, &content] {
        for event in [
            "storage.onChanged",
            "storage.session.onChanged",
            "storage.local.onChanged",
        ] {
            registry.add(sender, id(1), event, None).unwrap();
        }
    }
    let change = storage
        .mutate(
            Area::Session,
            &Mutation::Set(json!({"secret":1}).as_object().unwrap().clone()),
            false,
            0,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        registry.storage_targets(&storage, [&trusted, &content], "storage.onChanged", &change),
        vec![trusted.context_key()]
    );
    assert!(registry
        .targets(install(), "storage.onChanged", &EventFacts::default())
        .is_empty());
    assert!(registry
        .listeners(
            &content,
            "storage.session.onChanged",
            &EventFacts::default()
        )
        .is_empty());
    storage
        .set_access_level(
            &trusted,
            Area::Session,
            AccessLevel::TrustedAndUntrustedContexts,
        )
        .unwrap();
    assert_eq!(
        registry.storage_listeners(&content, &storage, "storage.session.onChanged", &change),
        [id(1)]
    );
    assert!(registry
        .storage_listeners(&content, &storage, "storage.local.onChanged", &change)
        .is_empty());
    storage
        .set_access_level(&trusted, Area::Session, AccessLevel::TrustedContexts)
        .unwrap();
    assert!(registry
        .storage_listeners(&content, &storage, "storage.session.onChanged", &change)
        .is_empty());
    let wrong_store = Storage::memory(zephium_core::ids::ProfileId::from(77), install(), false);
    assert!(registry
        .storage_targets(
            &wrong_store,
            [&trusted, &content],
            "storage.onChanged",
            &change
        )
        .is_empty());
}

#[test]
fn filter_and_regex_budgets_are_aggregate_and_released_on_context_death() {
    let sender = background_sender();
    let popup = popup_sender();
    let mut registry = EventRegistry::new();
    let filter = json!({"url":[{"urlMatches":"example", "originAndPathMatches":"/"}]});
    for value in 1..=32 {
        registry
            .add(
                &sender,
                id(value),
                "webNavigation.onCommitted",
                Some(&filter),
            )
            .unwrap();
    }
    assert!(registry
        .add(&popup, id(33), "webNavigation.onCommitted", Some(&filter))
        .is_err());
    assert!(!registry.has_listener(&popup, id(33), "webNavigation.onCommitted"));
    registry.cancel_context(&sender.context_key());
    registry
        .add(&popup, id(33), "webNavigation.onCommitted", Some(&filter))
        .unwrap();
    registry.cancel_context(&popup.context_key());
    let filter = json!({"url":[{"pathPrefix":"x".repeat(40_000)}]});
    for value in 1..=26 {
        registry
            .add(
                if value % 2 == 0 { &sender } else { &popup },
                id(value),
                "webNavigation.onCommitted",
                Some(&filter),
            )
            .unwrap();
    }
    assert!(registry
        .add(&sender, id(27), "webNavigation.onCommitted", Some(&filter))
        .is_err());
    assert!(ListenerFilter::parse(
        "webNavigation.onCommitted",
        Some(&json!({"url":vec![json!({"urlMatches":"x"});9]}))
    )
    .is_err());
}

#[test]
fn snapshot_restore_validates_budget_before_replacing_saved_interests() {
    let sender = background_sender();
    let mut registry = EventRegistry::new();
    registry.begin_registration(&sender).unwrap();
    registry
        .add(&sender, id(1), "runtime.onStartup", None)
        .unwrap();
    registry.finish_registration(&sender).unwrap();
    let bad = ListenerSnapshot {
        version: 1,
        listeners: vec![
            ListenerSpec {
                event: "webNavigation.onCommitted".into(),
                filter: Some(json!({"url":[{"urlMatches":"x", "originAndPathMatches":"/"}]}))
            };
            33
        ],
    };
    assert!(registry.restore(install(), bad).is_err());
    assert!(registry.should_wake(install(), "runtime.onStartup", &EventFacts::default()));
    assert!(ListenerSnapshot::decode(&vec![b' '; MAX_LISTENER_SNAPSHOT_BYTES + 1]).is_err());
}

#[test]
fn event_argument_validator_rejects_bad_ids_and_shapes() {
    for args in [
        json!([]),
        json!([0]),
        json!([-1]),
        json!([1.5]),
        json!([9_007_199_254_740_992_u64]),
        json!([1, {}, 3]),
    ] {
        assert!(ManualValidator
            .validate("storage.onChanged.addListener", args.as_array().unwrap())
            .is_err());
    }
    ManualValidator
        .validate("storage.session.onChanged.addListener", &[json!(1)])
        .unwrap();
    ManualValidator
        .validate("storage.local.onChanged.hasListeners", &[])
        .unwrap();
}

#[test]
fn persisted_interests_come_from_first_task_not_live_documents() {
    let sender = background_sender();
    let mut registry = EventRegistry::new();
    registry.begin_registration(&sender).expect("begin");
    registry
        .add(&sender, id(1), "runtime.onStartup", None)
        .expect("add");
    assert!(!registry.should_wake(install(), "runtime.onStartup", &EventFacts::default()));
    registry.finish_registration(&sender).expect("finish");
    registry
        .add(&sender, id(2), "runtime.onMessage", None)
        .expect("late listener");
    assert!(!registry.should_wake(install(), "runtime.onMessage", &EventFacts::default()));
    let serialized = serde_json::to_vec(&registry.snapshot(install())).expect("snapshot");
    registry.cancel_context(&sender.context_key());
    assert!(registry
        .targets(install(), "runtime.onStartup", &EventFacts::default())
        .is_empty());
    assert!(should_emit_startup(&registry, install(), false));
    assert!(!should_emit_startup(&registry, install(), true));
    let mut restored = EventRegistry::new();
    restored
        .restore(
            install(),
            serde_json::from_slice(&serialized).expect("decode"),
        )
        .expect("restore");
    assert!(should_emit_startup(&restored, install(), false));
    restored.begin_registration(&sender).expect("restart");
    restored.cancel_context(&sender.context_key());
    assert!(
        should_emit_startup(&restored, install(), false),
        "crash keeps last saved interests"
    );
    restored.begin_registration(&sender).expect("restart");
    restored
        .finish_registration(&sender)
        .expect("empty registration");
    assert!(!should_emit_startup(&restored, install(), false));
}

#[test]
fn filters_fan_out_once_per_context_and_remove_saved_interest() {
    let sender = background_sender();
    let mut registry = EventRegistry::new();
    registry.begin_registration(&sender).expect("begin");
    let filter =
        json!({"url":[{"hostSuffix":"example.com","pathPrefix":"/ok","urlMatches":"^https://"}]});
    for i in 1..=2 {
        registry
            .add(&sender, id(i), "webNavigation.onCommitted", Some(&filter))
            .expect("listener");
    }
    registry.finish_registration(&sender).expect("finish");
    let url = url::Url::parse("https://www.example.com/okay#ignored").expect("URL");
    let facts = EventFacts {
        url: Some(&url),
        ..EventFacts::default()
    };
    assert_eq!(
        registry.targets(install(), "webNavigation.onCommitted", &facts),
        vec![sender.context_key()]
    );
    assert_eq!(
        registry.listeners(&sender, "webNavigation.onCommitted", &facts),
        vec![id(1), id(2)]
    );
    assert!(registry.remove(&sender, "webNavigation.onCommitted", id(1)));
    assert!(registry.should_wake(install(), "webNavigation.onCommitted", &facts));
    assert!(registry.remove(&sender, "webNavigation.onCommitted", id(2)));
    assert!(!registry.should_wake(install(), "webNavigation.onCommitted", &facts));
    assert!(ListenerFilter::parse("runtime.onMessage", Some(&json!({"url":[]}))).is_err());
    assert!(ListenerFilter::parse(
        "webNavigation.onCommitted",
        Some(&json!({"url":[{"unknown":true}]}))
    )
    .is_err());
    assert!(ListenerFilter::parse(
        "webNavigation.onCommitted",
        Some(&json!({"url":[{"urlMatches":"["}]}))
    )
    .is_err());
}

#[test]
fn request_filters_use_host_facts_and_reject_missing_or_malformed_values() {
    let filter = ListenerFilter::parse(
        "webRequest.onBeforeRequest",
        Some(&json!({"urls":["https://*.example.com/*"],"types":["script"],"tabId":8})),
    )
    .expect("filter");
    let url = url::Url::parse("https://example.com/test.js").expect("URL");
    let facts = EventFacts {
        url: Some(&url),
        resource_type: Some("script"),
        tab_id: Some(8),
        ..EventFacts::default()
    };
    assert!(filter.matches(&facts));
    assert!(!filter.matches(&EventFacts {
        tab_id: None,
        ..facts
    }));
    assert!(ListenerFilter::parse("webRequest.onBeforeRequest", None).is_err());
    assert!(ListenerFilter::parse(
        "webRequest.onBeforeRequest",
        Some(&json!({"urls":["https://*/*"],"types":["bad"]}))
    )
    .is_err());
    assert!(ListenerFilter::parse(
        "webRequest.onBeforeRequest",
        Some(&json!({"urls":[],"tabId":1.5}))
    )
    .is_err());
}

#[test]
fn url_filters_cover_components_ports_cidrs_and_fragment_normalization() {
    let filter = ListenerFilter::parse("webNavigation.onCommitted", Some(&json!({"url":[{"hostContains":".127","schemes":["https"],"ports":[443,[8000,9000]],"cidrBlocks":["127.0.0.0/8"],"urlSuffix":"x=1","originAndPathMatches":"/okay$"}]}))).expect("filter");
    let url = url::Url::parse("https://127.0.0.1/okay?x=1#not-matched").expect("URL");
    assert!(filter.matches(&EventFacts {
        url: Some(&url),
        ..EventFacts::default()
    }));
    for bad in [
        json!({"ports":[[9,2]]}),
        json!({"cidrBlocks":["::1/129"]}),
        json!({"schemes":1}),
    ] {
        assert!(
            ListenerFilter::parse("webNavigation.onCommitted", Some(&json!({"url":[bad]})))
                .is_err()
        );
    }
}

#[test]
fn queue_retries_same_sequence_and_refuses_wrong_context_and_out_of_order_ack() {
    let sender = background_sender();
    let stranger = content_sender("https://example.com/");
    let mut queue = EventQueue::new(install());
    let first = queue
        .enqueue(
            "runtime.onStartup",
            b"[]".to_vec(),
            EventImportance::Critical,
        )
        .expect("first");
    let second = queue
        .enqueue(
            "alarms.onAlarm",
            b"[{}]".to_vec(),
            EventImportance::Critical,
        )
        .expect("second");
    assert!(!queue.acknowledge(&sender, first), "must first deliver");
    assert_eq!(
        queue
            .deliver(&sender)
            .expect("deliver")
            .expect("event")
            .sequence,
        first
    );
    assert!(!queue.acknowledge(&stranger, first));
    assert!(!queue.acknowledge(&sender, second));
    queue.cancel_context(&sender.context_key());
    assert!(!queue.acknowledge(&sender, first));
    assert_eq!(
        queue
            .deliver(&sender)
            .expect("retry")
            .expect("event")
            .sequence,
        first
    );
    assert!(queue.acknowledge(&sender, first));
    assert_eq!(
        queue
            .deliver(&sender)
            .expect("next")
            .expect("event")
            .sequence,
        second
    );
    assert!(queue.acknowledge(&sender, second));
    assert_eq!(queue.bytes(), 0);
}

#[test]
fn pressure_never_drops_critical_or_in_flight_events_or_mutates_on_refusal() {
    let sender = background_sender();
    let mut queue = EventQueue::new(install());
    for _ in 0..MAX_QUEUED_EVENTS_PER_BACKGROUND {
        queue
            .enqueue("runtime.onStartup", vec![], EventImportance::Critical)
            .expect("critical");
    }
    let before = queue.bytes();
    assert!(queue
        .enqueue("tabs.onUpdated", vec![], EventImportance::Droppable)
        .is_err());
    assert_eq!(queue.bytes(), before);
    let mut droppable = EventQueue::new(install());
    for _ in 0..MAX_QUEUED_EVENTS_PER_BACKGROUND {
        droppable
            .enqueue("tabs.onUpdated", vec![], EventImportance::Droppable)
            .expect("droppable");
    }
    let first = droppable
        .deliver(&sender)
        .expect("deliver")
        .expect("head")
        .sequence;
    droppable
        .enqueue("runtime.onStartup", vec![], EventImportance::Critical)
        .expect("drop second");
    assert_eq!(droppable.len(), MAX_QUEUED_EVENTS_PER_BACKGROUND as usize);
    assert_eq!(
        droppable
            .deliver(&sender)
            .expect("retry")
            .expect("head")
            .sequence,
        first
    );
    assert!(droppable.acknowledge(&sender, first));
    assert_eq!(
        droppable
            .deliver(&sender)
            .expect("deliver")
            .expect("head")
            .sequence
            .get(),
        3
    );
}

#[test]
fn installed_payload_has_only_reason_specific_fields() {
    assert_eq!(
        InstalledReason::Install.arguments(),
        json!([{"reason":"install"}])
    );
    assert_eq!(
        InstalledReason::Update {
            previous_version: "1.0".into()
        }
        .arguments(),
        json!([{"reason":"update","previousVersion":"1.0"}])
    );
    assert_eq!(
        InstalledReason::BrowserUpdate.arguments(),
        json!([{"reason":"chrome_update"}])
    );
}

proptest! {
    #[test]
    fn delivery_order_matches_fifo_model(operations in prop::collection::vec(any::<u8>(), 1..1000)) {
        let sender = background_sender();
        let mut queue = EventQueue::new(install());
        let mut model = VecDeque::new();
        for operation in operations {
            match operation % 4 {
                0 => if let Ok(seq) = queue.enqueue("alarms.onAlarm", vec![operation], EventImportance::Critical) { model.push_back(seq); },
                1 => { prop_assert_eq!(queue.deliver(&sender).expect("deliver").map(|e| e.sequence), model.front().copied()); },
                2 => if let Some(seq) = model.front().copied() { if queue.acknowledge(&sender, seq) { model.pop_front(); } },
                _ => queue.cancel_context(&sender.context_key()),
            }
            prop_assert_eq!(queue.len(), model.len());
            prop_assert_eq!(queue.bytes(), ("alarms.onAlarm".len() as u64 + 1) * model.len() as u64);
        }
    }
}
