use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::context::tests::{
    authority, background_sender, committed, content_sender, install, EXTENSION,
};
use crate::context::{ClaimedWorld, InjectedWorld, TabDocument, ViewKind};
use crate::events::ListenerId;
use crate::limits::{ManualClock, MAX_IN_FLIGHT_BYTES_PER_INSTALL};

fn views() -> (Sender, Sender) {
    let mut authority = authority();
    let background = authority
        .open_view(install(), ViewKind::Background)
        .expect("background");
    let popup = authority
        .open_view(install(), ViewKind::Popup)
        .expect("popup");
    let background = authority
        .bind_view(
            &background,
            &committed(&format!("chrome-extension://{EXTENSION}/bg.html"), 1),
        )
        .expect("bind background");
    let popup = authority
        .bind_view(
            &popup,
            &committed(&format!("chrome-extension://{EXTENSION}/popup.html"), 2),
        )
        .expect("bind popup");
    (background, popup)
}

fn listener(sender: &Sender, id: u64) -> MessageListener {
    MessageListener {
        sender: sender.clone(),
        listener: ListenerId::new(id).expect("listener"),
    }
}

fn message_delivery(actions: &[PortAction]) -> DeliveryId {
    actions
        .iter()
        .find_map(|action| {
            if let PortAction::Message { delivery, .. }
            | PortAction::NativeMessage { delivery, .. } = action
            {
                Some(*delivery)
            } else {
                None
            }
        })
        .expect("message action")
}

#[test]
fn opening_buffers_messages_then_connects_before_ordered_relay() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let clock = ManualClock::default();
    let mut limiter = Limiter::new();
    let mut service = PortService::new();
    let (port, actions) = service
        .begin_connect(&source, MessageRoute::Runtime, "test")
        .expect("begin");
    assert!(actions.is_empty());
    assert!(service
        .post_message(&source, port, b"first", &mut limiter, &clock)
        .expect("queue")
        .is_empty());
    assert!(service
        .post_message(&source, port, b"second", &mut limiter, &clock)
        .expect("queue")
        .is_empty());
    let actions = service
        .complete_connect(
            port,
            &[receiver.clone(), receiver.clone(), source.clone()],
            &mut limiter,
            &clock,
        )
        .expect("complete");
    assert_eq!(actions.len(), 3);
    assert!(
        matches!(&actions[0], PortAction::Connect { event:"runtime.onConnect", name, .. } if name == "test")
    );
    assert!(
        matches!(&actions[1], PortAction::Message { payload, .. } if payload.as_ref() == b"first")
    );
    assert!(
        matches!(&actions[2], PortAction::Message { payload, .. } if payload.as_ref() == b"second")
    );
    for action in actions {
        if let PortAction::Message { delivery, .. } = action {
            assert!(!service.acknowledge(&source, delivery, &mut limiter));
            assert!(service.acknowledge(&receiver, delivery, &mut limiter));
            assert!(!service.acknowledge(&receiver, delivery, &mut limiter));
        }
    }
    assert_eq!(limiter.install_bytes(install()), 0);
}

#[test]
fn receiver_fanout_disconnect_and_death_preserve_other_receivers() {
    let source = content_sender("https://example.com/");
    let (background, popup) = views();
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (port, _) = service
        .begin_connect(&source, MessageRoute::Runtime, "")
        .expect("begin");
    assert_eq!(
        service
            .complete_connect(
                port,
                &[background.clone(), popup.clone()],
                &mut limiter,
                &clock
            )
            .expect("connect")
            .len(),
        2
    );
    let actions = service
        .post_message(&source, port, b"opaque\xff", &mut limiter, &clock)
        .expect("relay");
    assert_eq!(actions.len(), 2);
    let delivery = message_delivery(&actions);
    let bytes = limiter.install_bytes(install());
    assert!(service.acknowledge(&background, delivery, &mut limiter));
    assert_eq!(
        limiter.install_bytes(install()),
        bytes,
        "source/fanout charge retained until last receipt"
    );
    assert_eq!(service.disconnect(&background, port, &mut limiter).len(), 1);
    assert_eq!(service.port_count(install()), 1);
    assert!(service.acknowledge(&popup, delivery, &mut limiter));
    assert_eq!(limiter.install_bytes(install()), 0);
    assert!(service
        .post_message(&background, port, b"spoof", &mut limiter, &clock)
        .is_err());
    let actions = service
        .post_message(&popup, port, b"reply", &mut limiter, &clock)
        .expect("reply");
    assert!(
        matches!(&actions[0], PortAction::Message { recipient, .. } if *recipient == source.context_key())
    );
    assert_eq!(
        service
            .cancel_context(&source.context_key(), &mut limiter)
            .len(),
        1
    );
    assert_eq!(limiter.install_bytes(install()), 0);
    assert_eq!(service.port_count(install()), 0);
}

#[test]
fn receiver_death_releases_only_after_other_recipients_settle() {
    let source = content_sender("https://example.com/");
    let (background, popup) = views();
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (port, _) = service
        .begin_connect(&source, MessageRoute::Runtime, "")
        .expect("begin");
    service
        .complete_connect(
            port,
            &[background.clone(), popup.clone()],
            &mut limiter,
            &clock,
        )
        .expect("complete");
    let actions = service
        .post_message(&source, port, b"payload", &mut limiter, &clock)
        .expect("message");
    let delivery = message_delivery(&actions);
    assert!(service
        .cancel_context(&background.context_key(), &mut limiter)
        .is_empty());
    assert!(limiter.install_bytes(install()) > 0);
    assert!(service.acknowledge(&popup, delivery, &mut limiter));
    assert_eq!(limiter.install_bytes(install()), 0);
    assert_eq!(
        service
            .cancel_context(&popup.context_key(), &mut limiter)
            .len(),
        1
    );
    assert_eq!(service.port_count(install()), 0);
}

#[test]
fn no_receiver_handshake_reports_last_error_and_releases_opening_bytes() {
    let source = content_sender("https://example.com/");
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (port, _) = service
        .begin_connect(&source, MessageRoute::Runtime, "")
        .expect("begin");
    service
        .post_message(&source, port, b"buffer", &mut limiter, &clock)
        .expect("buffer");
    let actions = service
        .complete_connect(port, std::slice::from_ref(&source), &mut limiter, &clock)
        .expect("failed handshake");
    assert!(
        matches!(&actions[0], PortAction::Disconnect { error:Some(error), recipient, .. } if error == &no_receiver() && *recipient == source.context_key())
    );
    assert_eq!(service.port_count(install()), 0);
    assert_eq!(limiter.install_bytes(install()), 0);
    assert!(service
        .post_message(&source, port, b"late", &mut limiter, &clock)
        .is_err());
}

#[test]
fn tab_route_targets_only_requested_frame_and_document() {
    let mut authority = authority();
    let mut content = Vec::new();
    for frame in 0..2 {
        let committed = committed("https://example.com/", frame + 3);
        authority
            .record_injection(
                ItemId::from(5),
                FrameId::new(frame),
                committed.generation,
                install(),
                InjectedWorld::Isolated,
            )
            .expect("record");
        content.push(
            authority
                .bind_tab(
                    &TabDocument {
                        tab: ItemId::from(5),
                        frame: FrameId::new(frame),
                        committed,
                    },
                    &ClaimedWorld::Isolated(install()),
                )
                .expect("sender"),
        );
    }
    let source = background_sender();
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let route = MessageRoute::Tab {
        tab: ItemId::from(5),
        frame: Some(FrameId::new(1)),
        document: Some(content[1].document()),
    };
    let (port, _) = service.begin_connect(&source, route, "").expect("begin");
    let actions = service
        .complete_connect(port, &content, &mut limiter, &clock)
        .expect("complete");
    assert_eq!(actions.len(), 1);
    assert!(
        matches!(&actions[0], PortAction::Connect { recipient, .. } if *recipient == content[1].context_key())
    );
    service.cancel_context(&content[1].context_key(), &mut limiter);
    assert_eq!(service.port_count(install()), 0);
}

#[test]
fn native_handshake_limits_and_bidirectional_terminal_cleanup() {
    let source = background_sender();
    let untrusted = content_sender("https://example.com/");
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let route = MessageRoute::Native {
        host: "org.example.host".into(),
    };
    assert!(service
        .begin_connect(&untrusted, route.clone(), "")
        .is_err());
    let (port, actions) = service.begin_connect(&source, route, "").expect("begin");
    assert!(
        matches!(&actions[0], PortAction::ConnectNative { host, .. } if host == "org.example.host")
    );
    service
        .post_message(&source, port, b"{}", &mut limiter, &clock)
        .expect("buffer");
    let actions = service
        .native_connected(port, &mut limiter, &clock)
        .expect("connected");
    let delivery = message_delivery(&actions);
    assert!(!service.acknowledge(&source, delivery, &mut limiter));
    assert!(service.acknowledge_native(port, delivery, &mut limiter));
    let inbound = service
        .native_message(port, b"{\"ok\":true}", &mut limiter, &clock)
        .expect("native message");
    let delivery = message_delivery(&[inbound]);
    assert!(service.acknowledge(&source, delivery, &mut limiter));
    assert!(service
        .native_message(
            port,
            &vec![0; MAX_NATIVE_MESSAGE_BYTES as usize + 1],
            &mut limiter,
            &clock
        )
        .is_err());
    let actions = service.native_disconnected(
        port,
        Some(ApiError::other("Native host has exited.")),
        &mut limiter,
    );
    assert_eq!(actions.len(), 2);
    assert_eq!(limiter.install_bytes(install()), 0);
    assert_eq!(service.port_count(install()), 0);
}

#[test]
fn install_port_cap_counts_opening_channels_and_does_not_reuse_ids() {
    let source = background_sender();
    let mut service = PortService::new();
    let mut last = None;
    for _ in 0..MAX_PORTS_PER_INSTALL {
        last = Some(
            service
                .begin_connect(&source, MessageRoute::Runtime, "")
                .expect("within cap")
                .0,
        );
    }
    assert_eq!(
        service
            .begin_connect(&source, MessageRoute::Runtime, "")
            .expect_err("cap"),
        ApiError::too_many_pending()
    );
    let old = last.expect("last");
    service.disconnect(&source, old, &mut Limiter::new());
    let next = service
        .begin_connect(&source, MessageRoute::Runtime, "")
        .expect("freed slot")
        .0;
    assert!(next.get() > old.get());
}

#[test]
fn receiving_endpoints_share_install_cap_and_erase_drains_every_reservation() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    for _ in 0..MAX_PORTS_PER_INSTALL / 2 {
        let (port, _) = service
            .begin_connect(&source, MessageRoute::Runtime, "")
            .expect("begin");
        service
            .complete_connect(port, std::slice::from_ref(&receiver), &mut limiter, &clock)
            .expect("complete");
    }
    assert_eq!(
        service.endpoint_count(install()),
        MAX_PORTS_PER_INSTALL as usize
    );
    assert_eq!(
        service
            .begin_connect(&source, MessageRoute::Runtime, "")
            .expect_err("shared endpoint cap"),
        ApiError::too_many_pending()
    );
    service.erase(install(), &mut limiter);
    assert_eq!(service.endpoint_count(install()), 0);
    let (port, _) = service
        .begin_connect(&source, MessageRoute::Runtime, "")
        .expect("begin");
    service
        .post_message(&source, port, b"buffered", &mut limiter, &clock)
        .expect("buffer");
    assert!(limiter.install_bytes(install()) > 0);
    service.erase(install(), &mut limiter);
    assert_eq!(limiter.install_bytes(install()), 0);
}

#[test]
fn admitted_relay_refuses_reservation_from_another_context() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let mut service = PortService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (port, _) = service
        .begin_connect(&source, MessageRoute::Runtime, "")
        .expect("begin");
    let reservation = limiter
        .admit(
            &receiver.context_key(),
            receiver.kind(),
            4,
            Charge::Message,
            0,
        )
        .expect("other context budget");
    assert_eq!(
        service
            .post_admitted(&source, port, b"test", reservation, &mut limiter, &clock)
            .expect_err("wrong reservation"),
        ApiError::context_invalidated()
    );
    assert_eq!(limiter.install_bytes(install()), 0);
}

#[test]
fn one_shot_async_promise_first_response_wins_and_settlement_receipt_is_scoped() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let listeners = [listener(&receiver, 1), listener(&receiver, 2)];
    let mut service = MessageService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (id, deliveries) = service
        .send(
            &source,
            &MessageRoute::Runtime,
            &listeners,
            b"opaque\xff",
            &mut limiter,
            &clock,
        )
        .expect("send");
    assert_eq!(deliveries.len(), 2);
    assert!(service
        .observe(
            &receiver,
            id,
            listeners[0].listener,
            ListenerOutcome::NoResponse,
            &mut limiter,
            &clock
        )
        .expect("decline")
        .is_none());
    assert!(service
        .observe(
            &receiver,
            id,
            listeners[0].listener,
            ListenerOutcome::Response(b"late".to_vec()),
            &mut limiter,
            &clock
        )
        .expect("late callback")
        .is_none());
    assert!(service
        .observe(
            &receiver,
            id,
            listeners[1].listener,
            ListenerOutcome::Promise,
            &mut limiter,
            &clock
        )
        .expect("promise")
        .is_none());
    let settlement = service
        .observe(
            &receiver,
            id,
            listeners[1].listener,
            ListenerOutcome::Response(b"null".to_vec()),
            &mut limiter,
            &clock,
        )
        .expect("resolve")
        .expect("settled");
    assert_eq!(settlement.result, Ok(Some(b"null".to_vec())));
    assert!(service
        .observe(
            &receiver,
            id,
            listeners[1].listener,
            ListenerOutcome::Rejected(ApiError::other("late rejection")),
            &mut limiter,
            &clock
        )
        .expect("late")
        .is_none());
    assert!(!service.acknowledge(&receiver, id, &mut limiter));
    assert!(service.acknowledge(&source, id, &mut limiter));
    assert!(!service.acknowledge(&source, id, &mut limiter));
    assert_eq!(limiter.install_bytes(install()), 0);
}

#[test]
fn one_shot_no_receiver_differs_from_listener_with_no_response() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let mut service = MessageService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    assert_eq!(
        service
            .send(
                &source,
                &MessageRoute::Runtime,
                &[],
                b"null",
                &mut limiter,
                &clock
            )
            .expect_err("no listener"),
        no_receiver()
    );
    let listener = listener(&receiver, 1);
    let (id, _) = service
        .send(
            &source,
            &MessageRoute::Runtime,
            std::slice::from_ref(&listener),
            b"null",
            &mut limiter,
            &clock,
        )
        .expect("send");
    let settlement = service
        .observe(
            &receiver,
            id,
            listener.listener,
            ListenerOutcome::NoResponse,
            &mut limiter,
            &clock,
        )
        .expect("decline")
        .expect("settled");
    assert_eq!(settlement.result, Ok(None));
    assert!(service.acknowledge(&source, id, &mut limiter));
}

#[test]
fn asynchronous_channel_death_timeout_and_first_rejection_settle_calls() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let listener = listener(&receiver, 1);
    let mut service = MessageService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (id, _) = service
        .send(
            &source,
            &MessageRoute::Runtime,
            std::slice::from_ref(&listener),
            b"null",
            &mut limiter,
            &clock,
        )
        .expect("send");
    service
        .observe(
            &receiver,
            id,
            listener.listener,
            ListenerOutcome::AsyncCallback,
            &mut limiter,
            &clock,
        )
        .expect("async");
    let settlements = service.cancel_context(&receiver.context_key(), &mut limiter);
    assert_eq!(settlements.len(), 1);
    assert!(settlements[0]
        .result
        .as_ref()
        .expect_err("closed")
        .message()
        .contains("asynchronous response"));
    service.acknowledge(&source, id, &mut limiter);
    let (id, _) = service
        .send(
            &source,
            &MessageRoute::Runtime,
            std::slice::from_ref(&listener),
            b"null",
            &mut limiter,
            &clock,
        )
        .expect("send");
    clock.advance_nanos(5 * 60 * 1_000_000_000);
    assert_eq!(service.expire(&clock).len(), 1);
    assert!(service.expire(&clock).is_empty());
    service.acknowledge(&source, id, &mut limiter);
    let (id, _) = service
        .send(
            &source,
            &MessageRoute::Runtime,
            std::slice::from_ref(&listener),
            b"null",
            &mut limiter,
            &clock,
        )
        .expect("send");
    let error = ApiError::other("listener rejected");
    assert_eq!(
        service
            .observe(
                &receiver,
                id,
                listener.listener,
                ListenerOutcome::Rejected(error.clone()),
                &mut limiter,
                &clock
            )
            .expect("rejected")
            .expect("settled")
            .result,
        Err(error)
    );
    service.cancel_context(&source.context_key(), &mut limiter);
    assert_eq!(limiter.install_bytes(install()), 0);
}

#[test]
fn late_response_is_refused_even_before_timer_callback_and_metadata_is_charged() {
    let source = content_sender("https://example.com/");
    let receiver = background_sender();
    let listeners = [listener(&receiver, 1), listener(&receiver, 2)];
    let mut service = MessageService::new();
    let mut limiter = Limiter::new();
    let clock = ManualClock::default();
    let (id, _) = service
        .send(
            &source,
            &MessageRoute::Runtime,
            &listeners,
            b"null",
            &mut limiter,
            &clock,
        )
        .expect("send");
    assert!(limiter.context_bytes(&source.context_key()) >= 2 * 512);
    assert!(limiter.context_bytes(&receiver.context_key()) >= 2 * 512);
    service
        .observe(
            &receiver,
            id,
            listeners[0].listener,
            ListenerOutcome::Promise,
            &mut limiter,
            &clock,
        )
        .expect("promise");
    clock.advance_nanos(5 * 60 * 1_000_000_000);
    let settlement = service
        .observe(
            &receiver,
            id,
            listeners[0].listener,
            ListenerOutcome::Response(b"late".to_vec()),
            &mut limiter,
            &clock,
        )
        .expect("observed")
        .expect("settled");
    assert!(settlement
        .result
        .expect_err("deadline")
        .message()
        .contains("asynchronous response"));
    assert!(service.acknowledge(&source, id, &mut limiter));
    assert_eq!(limiter.install_bytes(install()), 0);
}

#[test]
fn parsers_cover_overloads_and_refuse_malformed_options_targets_and_hosts() {
    assert!(
        matches!(parse_request("runtime.connect", &[]), Ok(MessageRequest::Connect { name, .. }) if name.is_empty())
    );
    assert!(
        matches!(parse_request("runtime.sendMessage", &[json!({"a":1})]), Ok(MessageRequest::Send { payload, .. }) if payload == b"{\"a\":1}")
    );
    assert!(matches!(
        parse_request("runtime.sendMessage", &[json!(EXTENSION), json!({"a":1})]),
        Ok(MessageRequest::Send {
            target: MessageTarget::Runtime {
                extension_id: Some(_)
            },
            ..
        })
    ));
    assert!(matches!(
        parse_request(
            "tabs.connect",
            &[
                json!(5),
                json!({"frameId":2,"documentId":"doc","name":"test"})
            ]
        ),
        Ok(MessageRequest::Connect {
            target: MessageTarget::Tab {
                tab_id: 5,
                frame_id: Some(2),
                ..
            },
            ..
        })
    ));
    for (method, args) in [
        ("runtime.connect", vec![json!({"bad":true})]),
        ("runtime.sendMessage", vec![]),
        ("tabs.sendMessage", vec![json!(-1), json!(null)]),
        ("tabs.sendMessage", vec![json!(1.5), json!(null)]),
        ("tabs.connect", vec![json!(5), json!({"frameId":-1})]),
        ("tabs.connect", vec![json!(5), json!({"documentId":""})]),
        ("runtime.connectNative", vec![json!("bad..host")]),
        ("runtime.connectNative", vec![json!("Bad.Host")]),
    ] {
        assert!(parse_request(method, &args).is_err(), "{method} {args:?}");
    }
}

proptest! {
    #[test]
    fn port_state_machine_matches_channel_model_and_drains_budgets(operations in prop::collection::vec(any::<u8>(), 1..400)) {
        let source = content_sender("https://example.com/");
        let receiver = background_sender();
        let mut service = PortService::new();
        let mut limiter = Limiter::new();
        let clock = ManualClock::default();
        let mut model: BTreeMap<PortId, bool> = BTreeMap::new();
        let mut receipts = Vec::new();
        for operation in operations {
            clock.advance_nanos(1_000_000_000);
            match operation % 6 {
                0 => { let (id, _) = service.begin_connect(&source, MessageRoute::Runtime, "").expect("begin"); model.insert(id, false); }
                1 => if let Some((id, open)) = model.iter_mut().find(|(_, open)| !**open) {
                    let actions = service.complete_connect(*id, std::slice::from_ref(&receiver), &mut limiter, &clock).expect("complete");
                    *open = true;
                    for action in actions { if let PortAction::Message { delivery, .. } = action { receipts.push(delivery); } }
                },
                2 => if let Some(id) = model.keys().next().copied() {
                    if let Ok(actions) = service.post_message(&source, id, &[operation], &mut limiter, &clock) {
                        for action in actions { if let PortAction::Message { delivery, .. } = action { receipts.push(delivery); } }
                    }
                },
                3 => if let Some(delivery) = receipts.pop() { service.acknowledge(&receiver, delivery, &mut limiter); },
                4 => if let Some(id) = model.keys().next().copied() { service.disconnect(&source, id, &mut limiter); model.remove(&id); },
                _ => { service.cancel_context(&source.context_key(), &mut limiter); model.clear(); }
            }
            prop_assert_eq!(service.port_count(install()), model.len());
            prop_assert!(limiter.install_bytes(install()) <= MAX_IN_FLIGHT_BYTES_PER_INSTALL);
        }
        service.cancel_context(&source.context_key(), &mut limiter);
        prop_assert_eq!(service.port_count(install()), 0);
        prop_assert_eq!(limiter.install_bytes(install()), 0);
    }

    #[test]
    fn first_one_shot_responder_wins_for_any_response_order(order in prop::collection::vec(0_usize..4, 1..30)) {
        let source = content_sender("https://example.com/");
        let receiver = background_sender();
        let listeners: Vec<_> = (1..=4).map(|id| listener(&receiver, id)).collect();
        let mut service = MessageService::new();
        let mut limiter = Limiter::new();
        let clock = ManualClock::default();
        let (id, _) = service.send(&source, &MessageRoute::Runtime, &listeners, b"null", &mut limiter, &clock).expect("send");
        let mut winner = None;
        for index in order {
            let result = service.observe(&receiver, id, listeners[index].listener, ListenerOutcome::Response(vec![index as u8]), &mut limiter, &clock).expect("response");
            if winner.is_none() { prop_assert_eq!(result.expect("first").result, Ok(Some(vec![index as u8]))); winner = Some(index); }
            else { prop_assert!(result.is_none()); }
        }
        prop_assert!(service.acknowledge(&source, id, &mut limiter));
        prop_assert_eq!(limiter.install_bytes(install()), 0);
    }
}
