use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_hdr_async;
use zephium_core::work::model::HostedRefusal;

use super::super::{LeadSecret, LeadStaticCredential};
use super::*;

const LISTEN: &str = "a0000000-0000-4000-8000-000000000001";
const SPEECH: &str = "b0000000-0000-4000-8000-000000000001";
const SECOND: &str = "b0000000-0000-4000-8000-000000000002";

#[test]
fn socket_logging_is_statically_capped_at_info() {
    use log as logging;

    assert!(logging::STATIC_MAX_LEVEL <= logging::LevelFilter::Info);
}

struct Handshake(bool);

impl tungstenite::handshake::server::Callback for Handshake {
    fn on_request(
        self,
        request: &tungstenite::handshake::server::Request,
        mut response: tungstenite::handshake::server::Response,
    ) -> Result<
        tungstenite::handshake::server::Response,
        tungstenite::handshake::server::ErrorResponse,
    > {
        assert!(request.method() == "GET" && request.uri().path() == "/v1/voice");
        assert!(request.version() == tungstenite::http::Version::HTTP_11);
        assert!(request
            .headers()
            .get("authorization")
            .is_some_and(|v| v == "Bearer test-secret"));
        assert!(request
            .headers()
            .get("sec-websocket-protocol")
            .is_some_and(|v| v == PROTOCOL));
        assert!(!request.headers().contains_key("origin"));
        if self.0 {
            response
                .headers_mut()
                .insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
        }
        Ok(response)
    }
}

struct Gateway {
    target: VoiceTarget,
    send: mpsc::Sender<Message>,
    received: mpsc::Receiver<Message>,
    task: JoinHandle<()>,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Gateway {
    async fn new(protocol: bool, ready: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = VoiceTarget {
            url: Url::parse(&format!("ws://{}/v1/voice", listener.local_addr().unwrap())).unwrap(),
        };
        let (send, mut outgoing) = mpsc::channel(256);
        let (incoming, received) = mpsc::channel(256);
        let task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = accept_hdr_async(tcp, Handshake(protocol)).await.unwrap();
            if ready && socket.send(Message::Text(json!({"type":"ready", "policy_revision":"p1", "listen":{"max_ms":240000}, "speak":{"max_chars":600,"voices":["alloy","echo"],"voice":"alloy"}}).to_string().into())).await.is_err() { return; }
            loop {
                tokio::select! {
                    message = outgoing.recv() => {
                        let Some(message) = message else { break; };
                        if socket.send(message).await.is_err() { break; }
                    },
                    message = socket.next() => match message {
                        Some(Ok(message)) => if incoming.send(message).await.is_err() { break; },
                        _ => break,
                    },
                }
            }
        });
        Self {
            target,
            send,
            received,
            task,
        }
    }

    async fn frame(&self, value: Value) {
        self.message(Message::Text(value.to_string().into())).await;
    }
    async fn message(&self, message: Message) {
        self.send.send(message).await.unwrap();
    }
    async fn next(&mut self) -> Message {
        tokio::time::timeout(Duration::from_secs(3), self.received.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn text(&mut self, kind: &str) {
        let Message::Text(text) = self.next().await else {
            panic!("expected client control");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["type"] == kind);
    }
}

fn credential() -> LeadStaticCredential {
    LeadStaticCredential::new(LeadSecret::new("test-secret".into()).unwrap())
}

fn usage() -> Value {
    json!({"request_id":"r1", "usage":"settled", "charged":"23", "audio_ms":20, "characters":5})
}

type Records = Arc<Mutex<Vec<LeadCloudRecord>>>;

async fn open(
    gateway: &Gateway,
    timings: Timings,
) -> (VoiceLink, mpsc::Receiver<VoiceEvent>, Records) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = records.clone();
    let observer: LeadCloudObserver = Arc::new(move |r| sink.lock().unwrap().push(r));
    let (link, ready, events) =
        VoiceLink::open_with_timings(&gateway.target, &credential(), Some(observer), timings)
            .await
            .unwrap();
    assert!(ready.policy_revision.as_deref() == Some("p1"));
    assert!(
        ready.listen_max_ms == 240000 && ready.speak_max_chars == 600 && ready.voice == "alloy"
    );
    (link, events, records)
}

async fn event(events: &mut mpsc::Receiver<VoiceEvent>) -> VoiceEvent {
    tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .expect("expected voice event")
}

async fn closed(events: &mut mpsc::Receiver<VoiceEvent>) {
    assert!(matches!(event(events).await, VoiceEvent::Closed));
    assert!(tokio::time::timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .is_none());
}

fn quick() -> Timings {
    Timings {
        open: Duration::from_millis(200),
        final_wait: Duration::from_millis(100),
        ping: Duration::from_millis(40),
        silence: Duration::from_secs(2),
    }
}

#[test]
fn production_target_is_pinned_and_diagnostics_redact_payloads() {
    let target = VoiceTarget::cloud(&Url::parse("https://api.zephium.app").unwrap()).unwrap();
    assert!(target.url.as_str() == "wss://api.zephium.app/v1/voice");
    for bad in [
        "http://api.zephium.app",
        "https://127.0.0.1",
        "https://example.com",
        "https://api.zephium.app:444",
        "https://user@api.zephium.app",
        "https://api.zephium.app?token=secret",
        "https://api.zephium.app/#a",
        "https://api.zephium.app/v1",
    ] {
        assert!(VoiceTarget::cloud(&Url::parse(bad).unwrap()).is_err());
    }
    for private in [
        VoiceEvent::Partial {
            id: LISTEN.into(),
            text: "private-canary".into(),
        },
        VoiceEvent::Final {
            id: LISTEN.into(),
            text: "private-canary".into(),
        },
        VoiceEvent::Audio(b"private-canary".to_vec()),
    ] {
        assert!(!format!("{private:?}").contains("private-canary"));
    }
    assert!(Timings::default().open == Duration::from_secs(10));
    assert!(Timings::default().final_wait == Duration::from_secs(8));
    assert!(Timings::default().ping == Duration::from_secs(20));
    assert!(Timings::default().silence == Duration::from_secs(45));
}

#[tokio::test]
async fn utterance_partials_preroll_commit_final_and_one_record() {
    let mut gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, Timings::default()).await;
    assert!(link.listen_start(LISTEN, Some("pl")));
    let pcm = vec![17; 50 * AUDIO_FRAME];
    assert!(link.audio(&pcm));
    assert!(link.audio(&pcm));
    assert!(link.listen_commit(LISTEN));
    gateway.text("listen.start").await;
    for _ in 0..2 {
        assert!(matches!(gateway.next().await, Message::Binary(bytes) if bytes.as_ref() == pcm));
    }
    gateway.text("listen.commit").await;
    for text in ["private", "private transcript"] {
        gateway
            .frame(json!({"type":"listen.partial","id":LISTEN,"text":text}))
            .await;
        assert!(
            matches!(event(&mut events).await, VoiceEvent::Partial { id, text: got } if id == LISTEN && got == text)
        );
    }
    gateway
        .frame(
            json!({"type":"listen.final","id":LISTEN,"text":"private transcript","usage":usage()}),
        )
        .await;
    assert!(
        matches!(event(&mut events).await, VoiceEvent::Final { id, text } if id == LISTEN && text == "private transcript")
    );
    let expected = LeadCloudRecord {
        idempotency_key: LISTEN.into(),
        request_id: Some("r1".into()),
        policy_revision: Some("p1".into()),
        accounting: LeadCloudAccounting::Settled {
            charged: Some("23".into()),
        },
    };
    assert!(records.lock().unwrap().as_slice() == [expected]);
    link.close();
    link.close();
    closed(&mut events).await;
    assert!(records.lock().unwrap().len() == 1);
}

#[tokio::test]
async fn commit_timeout_is_not_replayed_and_is_unknown() {
    let mut gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, quick()).await;
    assert!(link.listen_start(LISTEN, None));
    assert!(link.listen_commit(LISTEN));
    gateway.text("listen.start").await;
    gateway.text("listen.commit").await;
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::ListenFailed {
            error: VoiceError::Timeout,
            ..
        }
    ));
    closed(&mut events).await;
    assert!(records.lock().unwrap().len() == 1);
    assert!(records.lock().unwrap()[0].accounting == LeadCloudAccounting::Unknown);
    assert!(!link.listen_commit(LISTEN));
}

#[tokio::test]
async fn failure_codes_are_closed_and_accounted_once() {
    let hosted = |refusal| {
        VoiceError::Model(WorkModelError::Hosted {
            refusal,
            retry_after_ms: None,
        })
    };
    for (code, expected) in [
        ("insufficient_credits", hosted(HostedRefusal::OutOfCredits)),
        ("plan_required", hosted(HostedRefusal::PlanRequired)),
        ("daily_limit_reached", hosted(HostedRefusal::DailyLimit)),
        (
            "billing_review_required",
            hosted(HostedRefusal::BillingReview),
        ),
        (
            "free_usage_limit_reached",
            hosted(HostedRefusal::FreeUsagePaused),
        ),
        (
            "usage_confirmation_limited",
            hosted(HostedRefusal::ConfirmationLimited),
        ),
        ("cloud_budget_exhausted", hosted(HostedRefusal::Paused)),
        (
            "rate_limited",
            VoiceError::Model(WorkModelError::RateLimited {
                retry_after_ms: None,
            }),
        ),
        (
            "concurrency_limited",
            VoiceError::Model(WorkModelError::RateLimited {
                retry_after_ms: None,
            }),
        ),
        (
            "provider_rate_limited",
            VoiceError::Model(WorkModelError::RateLimited {
                retry_after_ms: None,
            }),
        ),
        (
            "provider_unavailable",
            VoiceError::Model(WorkModelError::Overloaded),
        ),
        (
            "provider_error",
            VoiceError::Model(WorkModelError::Overloaded),
        ),
        ("offline", VoiceError::Model(WorkModelError::Network)),
        (
            "unauthenticated",
            VoiceError::Model(WorkModelError::Unauthorized),
        ),
        ("cancelled", VoiceError::Cancelled),
        ("timeout", VoiceError::Timeout),
        ("interrupted", VoiceError::Interrupted),
        ("too_long", VoiceError::TooLong),
        ("audio_too_long", VoiceError::AudioTooLong),
        ("invalid_audio", VoiceError::InvalidAudio),
    ] {
        let gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.listen_start(LISTEN, None));
        gateway
            .frame(
                json!({"type":"listen.failed","id":LISTEN,"error":code,"message":"private-canary"}),
            )
            .await;
        assert!(
            matches!(event(&mut events).await, VoiceEvent::ListenFailed { error, .. } if error == expected)
        );
        assert!(records.lock().unwrap().len() == 1);
        assert!(
            records.lock().unwrap()[0].accounting
                == if expected == VoiceError::Interrupted {
                    LeadCloudAccounting::Interrupted
                } else {
                    LeadCloudAccounting::Refused
                }
        );
        link.close();
        closed(&mut events).await;
    }
}

#[tokio::test]
async fn pending_and_interrupted_usage_preserve_request_identity() {
    for (error, accounting) in [
        ("cancelled", LeadCloudAccounting::Pending),
        ("interrupted", LeadCloudAccounting::Interrupted),
    ] {
        let gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.listen_start(LISTEN, None));
        assert!(link.listen_cancel(LISTEN));
        gateway.frame(json!({"type":"listen.failed","id":LISTEN,"error":error,"usage":{"request_id":"r2","usage":"pending"}})).await;
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::ListenFailed { .. }
        ));
        {
            let records = records.lock().unwrap();
            assert!(
                records.len() == 1
                    && records[0].accounting == accounting
                    && records[0].request_id.as_deref() == Some("r2")
            );
        }
        link.close();
        closed(&mut events).await;
    }
}

#[tokio::test]
async fn refused_upgrade_uses_existing_cloud_faults_without_retry() {
    use super::super::tests::{http_reply, serve};
    for (status, code, expected) in [
        (
            "401 Unauthorized",
            "unauthenticated",
            WorkModelError::Unauthorized,
        ),
        (
            "402 Payment Required",
            "insufficient_credits",
            WorkModelError::Hosted {
                refusal: HostedRefusal::OutOfCredits,
                retry_after_ms: None,
            },
        ),
        (
            "403 Forbidden",
            "plan_required",
            WorkModelError::Hosted {
                refusal: HostedRefusal::PlanRequired,
                retry_after_ms: None,
            },
        ),
        (
            "429 Too Many Requests",
            "concurrency_limited",
            WorkModelError::RateLimited {
                retry_after_ms: None,
            },
        ),
        (
            "503 Service Unavailable",
            "cloud_disabled",
            WorkModelError::Overloaded,
        ),
        ("302 Found", "redirect", WorkModelError::Protocol),
    ] {
        let body = json!({"error":{"code":code,"message":"private-canary"}}).to_string();
        let (base, seen) = serve(vec![http_reply(
            status,
            "location: http://127.0.0.1:1/forbidden\r\n",
            &body,
        )]);
        let target = VoiceTarget {
            url: Url::parse(&base.replacen("http:", "ws:", 1)).unwrap(),
        };
        let result = VoiceLink::open(&target, &credential(), None).await;
        assert!(matches!(result, Err(error) if error == expected));
        assert!(seen.lock().unwrap().len() == 1);
    }
}

#[tokio::test]
async fn missing_subprotocol_and_ready_are_refused() {
    let gateway = Gateway::new(false, true).await;
    assert!(matches!(
        VoiceLink::open(&gateway.target, &credential(), None).await,
        Err(WorkModelError::Protocol)
    ));
    let gateway = Gateway::new(true, false).await;
    assert!(matches!(
        VoiceLink::open_with_timings(&gateway.target, &credential(), None, quick()).await,
        Err(WorkModelError::Network)
    ));
}

#[tokio::test]
async fn speech_audio_is_ordered_and_cancellation_has_one_record() {
    let mut gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, Timings::default()).await;
    assert!(link.speak(SPEECH, "private speech", Some("echo")));
    assert!(link.speak(SECOND, "private next speech", None));
    gateway.text("speak").await;
    gateway.text("speak").await;
    gateway
        .frame(json!({"type":"speak.start","id":SPEECH}))
        .await;
    gateway.message(Message::Binary(vec![1, 2].into())).await;
    gateway
        .message(Message::Binary(vec![3, 4, 5, 6].into()))
        .await;
    gateway
        .frame(json!({"type":"speak.end","id":SPEECH,"usage":usage()}))
        .await;
    assert!(matches!(event(&mut events).await, VoiceEvent::SpeakStart { id } if id == SPEECH));
    assert!(matches!(event(&mut events).await, VoiceEvent::Audio(a) if a == [1,2]));
    assert!(matches!(event(&mut events).await, VoiceEvent::Audio(a) if a == [3,4,5,6]));
    assert!(matches!(event(&mut events).await, VoiceEvent::SpeakEnd { id } if id == SPEECH));
    gateway
        .frame(json!({"type":"speak.start","id":SECOND}))
        .await;
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::SpeakStart { .. }
    ));
    assert!(link.speak_cancel(Some(SECOND)));
    gateway.text("speak.cancel").await;
    gateway
        .frame(json!({"type":"speak.failed","id":SECOND,"error":"cancelled","usage":usage()}))
        .await;
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::SpeakFailed {
            error: VoiceError::Cancelled,
            ..
        }
    ));
    assert!(records.lock().unwrap().len() == 2);
    link.close();
    closed(&mut events).await;
}

#[tokio::test]
async fn cancel_all_covers_queued_speeches() {
    let mut gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, Timings::default()).await;
    assert!(link.speak(SPEECH, "one", None));
    assert!(link.speak(SECOND, "two", None));
    assert!(link.speak_cancel(None));
    gateway.text("speak").await;
    gateway.text("speak").await;
    gateway.text("speak.cancel").await;
    for id in [SPEECH, SECOND] {
        gateway
            .frame(json!({"type":"speak.failed","id":id,"error":"cancelled"}))
            .await;
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::SpeakFailed {
                error: VoiceError::Cancelled,
                ..
            }
        ));
    }
    link.close();
    closed(&mut events).await;
    assert!(records.lock().unwrap().len() == 2);
}

#[tokio::test]
async fn unexpected_drop_interrupts_every_open_request_closed_last() {
    for committed in [false, true] {
        let mut gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.listen_start(LISTEN, None));
        assert!(link.speak(SPEECH, "private", None));
        if committed {
            assert!(link.listen_commit(LISTEN));
        }
        gateway.text("listen.start").await;
        gateway.text("speak").await;
        gateway
            .frame(json!({"type":"speak.start","id":SPEECH}))
            .await;
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::SpeakStart { .. }
        ));
        drop(gateway);
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::ListenFailed {
                error: VoiceError::Interrupted,
                ..
            }
        ));
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::SpeakFailed {
                error: VoiceError::Interrupted,
                ..
            }
        ));
        closed(&mut events).await;
        {
            let records = records.lock().unwrap();
            assert!(
                records.len() == 2
                    && records
                        .iter()
                        .all(|r| r.accounting == LeadCloudAccounting::Unknown)
            );
        }
        assert!(!link.audio(&[0; AUDIO_FRAME]));
    }
}

#[tokio::test]
async fn malformed_unknown_stray_and_oversized_frames_close_and_settle() {
    for message in [
        Message::Text("{private-canary".into()),
        Message::Text("{\"type\":\"future\"}".into()),
        Message::Binary(vec![0, 0].into()),
        Message::Text("x".repeat(TEXT_LIMIT + 1).into()),
        Message::Binary(vec![0; BINARY_LIMIT + 1].into()),
        Message::Text(
            json!({"type":"listen.partial","id":SECOND,"text":"private"})
                .to_string()
                .into(),
        ),
        Message::Text(
            json!({"type":"listen.failed","id":LISTEN,"error":"private-unknown-error"})
                .to_string()
                .into(),
        ),
        Message::Text(
            json!({"type":"listen.final","id":LISTEN,"text":"private","usage":usage()})
                .to_string()
                .into(),
        ),
    ] {
        let gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.listen_start(LISTEN, None));
        gateway.message(message).await;
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::ListenFailed {
                error: VoiceError::Model(WorkModelError::Protocol),
                ..
            }
        ));
        closed(&mut events).await;
        {
            let records = records.lock().unwrap();
            assert!(records.len() == 1 && records[0].accounting == LeadCloudAccounting::Unknown);
        }
    }
}

#[tokio::test]
async fn binary_requires_active_speech_and_even_pcm() {
    for message in [
        Message::Binary(vec![0].into()),
        Message::Binary(vec![0; BINARY_LIMIT + 2].into()),
    ] {
        let gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.speak(SPEECH, "private", None));
        gateway
            .frame(json!({"type":"speak.start","id":SPEECH}))
            .await;
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::SpeakStart { .. }
        ));
        gateway.message(message).await;
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::SpeakFailed {
                error: VoiceError::Model(WorkModelError::Protocol),
                ..
            }
        ));
        closed(&mut events).await;
        assert!(records.lock().unwrap().len() == 1);
    }
}

#[tokio::test]
async fn idle_ping_and_incoming_traffic_keep_the_socket_alive() {
    let mut gateway = Gateway::new(true, true).await;
    let mut timings = quick();
    timings.silence = Duration::from_millis(180);
    let (link, mut events, _) = open(&gateway, timings).await;
    for _ in 0..5 {
        gateway.text("ping").await;
        gateway.frame(json!({"type":"pong"})).await;
    }
    assert!(link.speak_cancel(None));
    link.close();
    closed(&mut events).await;
}

#[tokio::test]
async fn silence_closes_even_while_outgoing_pings_continue() {
    let gateway = Gateway::new(true, true).await;
    let mut timings = quick();
    timings.silence = Duration::from_millis(120);
    let (_link, mut events, _) = open(&gateway, timings).await;
    closed(&mut events).await;
}

#[tokio::test]
async fn backpressure_closes_instead_of_buffering_more_audio() {
    let gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, Timings::default()).await;
    assert!(link.listen_start(LISTEN, None));
    assert!(link.audio(&vec![0; AUDIO_BUDGET]));
    assert!(!link.audio(&[0; AUDIO_FRAME]));
    assert!(link.shared.state().audio_bytes == AUDIO_BUDGET);
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::ListenFailed {
            error: VoiceError::Model(WorkModelError::Overloaded),
            ..
        }
    ));
    closed(&mut events).await;
    assert!(records.lock().unwrap().len() == 1);
}

#[tokio::test]
async fn saturated_control_channel_and_dropped_handle_close_once() {
    let gateway = Gateway::new(true, true).await;
    let (link, mut events, _) = open(&gateway, Timings::default()).await;
    for _ in 0..QUEUE_CAPACITY {
        assert!(link.speak_cancel(None));
    }
    assert!(!link.speak_cancel(None));
    closed(&mut events).await;
    let (link, mut events, _) = open(&Gateway::new(true, true).await, Timings::default()).await;
    drop(link);
    closed(&mut events).await;
}

#[tokio::test]
async fn gateway_closing_precedes_failures_and_closed() {
    let gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, Timings::default()).await;
    assert!(link.listen_start(LISTEN, None));
    gateway
        .frame(json!({"type":"closing","reason":"session_ended"}))
        .await;
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::Closing {
            reason: VoiceClosing::SessionEnded
        }
    ));
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::ListenFailed {
            error: VoiceError::Interrupted,
            ..
        }
    ));
    closed(&mut events).await;
    assert!(records.lock().unwrap().len() == 1);
}

#[tokio::test]
async fn stalled_event_consumer_still_gets_terminal_results_and_accounting() {
    let gateway = Gateway::new(true, true).await;
    let (link, mut events, records) = open(&gateway, Timings::default()).await;
    assert!(link.listen_start(LISTEN, None));
    assert!(link.speak(SPEECH, "private", None));
    let mut stop = link.shared.stop.subscribe();
    for _ in 0..=EVENT_CAPACITY {
        gateway
            .frame(json!({"type":"listen.partial","id":LISTEN,"text":"private"}))
            .await;
    }
    if !*stop.borrow() {
        tokio::time::timeout(Duration::from_secs(3), stop.changed())
            .await
            .unwrap()
            .unwrap();
    }
    {
        let records = records.lock().unwrap();
        assert!(
            records.len() == 2
                && records
                    .iter()
                    .all(|r| r.accounting == LeadCloudAccounting::Unknown)
        );
    }
    for _ in 0..=EVENT_CAPACITY {
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::Partial { .. }
        ));
    }
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::ListenFailed {
            error: VoiceError::Model(WorkModelError::Overloaded),
            ..
        }
    ));
    assert!(matches!(
        event(&mut events).await,
        VoiceEvent::SpeakFailed {
            error: VoiceError::Model(WorkModelError::Overloaded),
            ..
        }
    ));
    closed(&mut events).await;
}

#[tokio::test]
async fn duplicate_ids_malformed_capture_and_unapproved_options_fail_closed() {
    for invalid in 0..7 {
        let gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.listen_start(LISTEN, None));
        assert!(!match invalid {
            0 => link.speak(LISTEN, "private", None),
            1 => link.listen_start(SECOND, None),
            2 => link.listen_commit(SECOND),
            3 => link.audio(&[0; AUDIO_FRAME - 1]),
            4 => link.speak(SPEECH, "private", Some("unknown")),
            5 => link.speak(SPEECH, &"a".repeat(601), None),
            _ => link.speak("not-a-uuid", "private", None),
        });
        assert!(matches!(
            event(&mut events).await,
            VoiceEvent::ListenFailed { .. }
        ));
        closed(&mut events).await;
        assert!(records.lock().unwrap().len() == 1);
    }
}

#[tokio::test]
async fn invalid_usage_duplicate_ready_and_speech_reordering_are_protocol_errors() {
    for frame in [
        json!({"type":"ready","listen":{"max_ms":240000},"speak":{"max_chars":600,"voices":["alloy"],"voice":"alloy"}}),
        json!({"type":"speak.start","id":SECOND}),
        json!({"type":"speak.end","id":SPEECH,"usage":usage()}),
        json!({"type":"speak.failed","id":SPEECH,"error":"cancelled","usage":{"request_id":"r1","usage":"unknown"}}),
        json!({"type":"speak.failed","id":SPEECH,"error":"cancelled","usage":{"request_id":"r1","usage":"settled","charged":"-1"}}),
    ] {
        let gateway = Gateway::new(true, true).await;
        let (link, mut events, records) = open(&gateway, Timings::default()).await;
        assert!(link.speak(SPEECH, "one", None));
        assert!(link.speak(SECOND, "two", None));
        gateway.frame(frame).await;
        for _ in 0..2 {
            assert!(matches!(
                event(&mut events).await,
                VoiceEvent::SpeakFailed {
                    error: VoiceError::Model(WorkModelError::Protocol),
                    ..
                }
            ));
        }
        closed(&mut events).await;
        assert!(records.lock().unwrap().len() == 2);
    }
}

#[tokio::test]
async fn websocket_ping_is_answered_and_protocol_failure_sends_close() {
    let mut gateway = Gateway::new(true, true).await;
    let (link, mut events, _) = open(&gateway, Timings::default()).await;
    gateway.message(Message::Ping(vec![1, 2].into())).await;
    assert!(matches!(gateway.next().await, Message::Pong(p) if p.as_ref() == [1,2]));
    gateway.frame(json!({"type":"unknown"})).await;
    closed(&mut events).await;
    assert!(
        matches!(gateway.next().await, Message::Close(Some(close)) if close.code == tungstenite::protocol::frame::coding::CloseCode::Protocol)
    );
    assert!(!link.speak_cancel(None));
}
