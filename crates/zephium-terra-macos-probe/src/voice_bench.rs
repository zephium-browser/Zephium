//! The real VoiceSession on the assist bench's native host. Only the audio
//! device and frame projection are scripted; neither the reducer nor gateway
//! nor PageAssist is replaced.
use super::*;
use crate::voice_audio::{self, ScriptedAudio};
use crate::voice_metrics::{Kind, Metrics, Provenance};
use crate::voice_scenarios::{self, VoiceScenario};
use sha2::{Digest as _, Sha256};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use tokio::sync::mpsc;
use zephium_agentic::lead::voice::{VoiceError, VoiceEvent, VoiceLink, VoiceTarget};
use zephium_agentic::lead::{LeadCloudAccounting, LeadSecret, LeadStaticCredential};
use zephium_ipc::voice_session::*;
use zephium_work_composition::page_assist::core::voice::{Fault, Language};
use zephium_work_composition::voice_session::port as vp;
use zephium_work_composition::voice_session::{self, Command, Setup, VoiceSession};

fn uuid() -> String {
    // Two independent ULID random fields provide 160 random bits. Hash away
    // their timestamp layout, then set the UUIDv4 version and variant.
    let mut random = Sha256::new();
    random.update(ItemId::generate().bytes());
    random.update(ItemId::generate().bytes());
    let mut bytes: [u8; 32] = random.finalize().into();
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h: String = bytes[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

struct Gateway {
    target: VoiceTarget,
    credential: LeadStaticCredential,
}
impl Gateway {
    fn ready_file() -> Result<Arc<Self>, &'static str> {
        let path = std::env::var_os("ZEPHIUM_PROBE_VOICE_READY").ok_or("voice_ready_missing")?;
        let path = std::path::Path::new(&path);
        let before = std::fs::symlink_metadata(path).map_err(|_| "voice_ready_file")?;
        let parent = path
            .parent()
            .and_then(|p| std::fs::metadata(p).ok())
            .ok_or("voice_ready_private")?;
        if !before.is_file()
            || before.permissions().mode() & 0o777 != 0o600
            || parent.permissions().mode() & 0o777 != 0o700
            || before.len() > 8192
            || before.uid() != parent.uid()
        {
            return Err("voice_ready_private");
        }
        let file = std::fs::File::open(path).map_err(|_| "voice_ready_file")?;
        let opened = file.metadata().map_err(|_| "voice_ready_file")?;
        if before.ino() != opened.ino() || before.dev() != opened.dev() {
            return Err("voice_ready_changed");
        }
        let mut bytes = Vec::new();
        file.take(8193)
            .read_to_end(&mut bytes)
            .map_err(|_| "voice_ready_file")?;
        if bytes.len() > 8192 {
            return Err("voice_ready_file");
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "voice_ready_shape")?;
        bytes.fill(0);
        let root = value["root"].as_str().ok_or("voice_ready_shape")?;
        // This bench never sends its ready-file credential beyond loopback.
        let root = root.parse().map_err(|_| "voice_ready_root")?;
        let target = VoiceTarget::cloud(&root).map_err(|_| "voice_ready_root")?;
        if root.scheme() != "http" || root.host_str() != Some("127.0.0.1") {
            return Err("voice_ready_root");
        }
        let serde_json::Value::String(token) = value["token"].take() else {
            return Err("voice_ready_shape");
        };
        let credential = LeadStaticCredential::new(
            LeadSecret::new(token).map_err(|_| "voice_ready_credential")?,
        );
        Ok(Arc::new(Self { target, credential }))
    }
    async fn open(
        &self,
    ) -> Result<
        (
            VoiceLink,
            zephium_agentic::lead::voice::VoiceReady,
            mpsc::Receiver<VoiceEvent>,
        ),
        WorkModelError,
    > {
        VoiceLink::open(&self.target, &self.credential, None).await
    }
    async fn synthesize(&self, text: &str) -> Result<Vec<i16>, &'static str> {
        let (socket, _, mut events) = self.open().await.map_err(|_| "voice_synthesis_open")?;
        let id = uuid();
        if !socket.speak(&id, text, None) {
            return Err("voice_synthesis_send");
        }
        let synthesis = async {
            let mut pcm = Vec::new();
            while let Some(event) = events.recv().await {
                match event {
                    VoiceEvent::Audio(bytes) => {
                        if pcm.len() + bytes.len() / 2 > 24_000 * 90 {
                            return Err("voice_synthesis_limit");
                        }
                        pcm.extend(
                            bytes
                                .chunks_exact(2)
                                .map(|p| i16::from_le_bytes([p[0], p[1]])),
                        );
                    }
                    VoiceEvent::SpeakEnd { id: ended } if ended == id => {
                        return voice_audio::utterance(pcm).ok_or("voice_synthesis_empty")
                    }
                    VoiceEvent::SpeakFailed { .. } | VoiceEvent::Closed => {
                        return Err("voice_synthesis_failed")
                    }
                    _ => {}
                }
            }
            Err("voice_synthesis_closed")
        };
        let result = tokio::time::timeout(Duration::from_secs(90), synthesis)
            .await
            .unwrap_or(Err("voice_synthesis_timeout"));
        socket.close();
        // Drain the close before opening the session's separate socket.
        let _ = tokio::time::timeout(Duration::from_secs(2), async {
            while events.recv().await.is_some() {}
        })
        .await;
        result
    }
}

fn fault_code(error: VoiceError) -> &'static str {
    use zephium_core::work::model::HostedRefusal as H;
    match error {
        VoiceError::Cancelled => "cancelled",
        VoiceError::Timeout => "timeout",
        VoiceError::Interrupted => "interrupted",
        VoiceError::TooLong => "too_long",
        VoiceError::AudioTooLong => "audio_too_long",
        VoiceError::InvalidAudio => "invalid_audio",
        VoiceError::Model(error) => match error {
            WorkModelError::MissingKey => "missing_key",
            WorkModelError::Unauthorized => "unauthorized",
            WorkModelError::Network => "network",
            WorkModelError::Protocol => "protocol",
            WorkModelError::RateLimited { .. } => "rate_limited",
            WorkModelError::Overloaded => "overloaded",
            WorkModelError::BadRequest => "bad_request",
            WorkModelError::ContextTooLong => "context_too_long",
            WorkModelError::Cancelled => "cancelled",
            WorkModelError::OverBudget => "over_budget",
            WorkModelError::PlanLimit => "plan_limit",
            WorkModelError::Hosted { refusal, .. } => match refusal {
                H::PlanRequired => "plan_required",
                H::BillingReview => "billing_review",
                H::OutOfCredits => "out_of_credits",
                H::DailyLimit => "daily_limit",
                H::FreeUsagePaused => "free_usage_paused",
                H::ConfirmationLimited => "confirmation_limited",
                H::Paused => "paused",
                H::AlreadySent => "already_sent",
                H::InvalidAudio => "invalid_audio",
                H::UnsupportedAudio => "unsupported_audio",
                H::AudioTooLong => "audio_too_long",
            },
        },
    }
}

#[derive(Default)]
struct WireFacts {
    utterance: usize,
    sent: Vec<i16>,
    expected: Vec<Vec<i16>>,
    reply_ids: Vec<String>,
    cancelled_ids: Vec<String>,
    warm_ids: Vec<String>,
    listen_ids: Vec<String>,
    receipts: std::collections::BTreeSet<String>,
    pending_speech: std::collections::BTreeSet<String>,
    first_frame_seen: bool,
    listen_indices: BTreeMap<String, usize>,
    sentences: Vec<(Option<u32>, String)>,
}
struct Link {
    gateway: Arc<Gateway>,
    metrics: Arc<Metrics>,
    audio: Arc<ScriptedAudio>,
    wire: Arc<Mutex<WireFacts>>,
    shared: Arc<Shared>,
    expected: &'static [&'static str],
    second_expected: &'static [&'static str],
    spoken_facts: &'static [&'static str],
}
impl Link {
    fn reply_for_speech(&self, text: &str) -> Option<u32> {
        use zephium_work_composition::page_assist::core::voice::phrases::normalize;
        let events = self.shared.events.lock().unwrap_or_else(|p| p.into_inner());
        let mut text_by_reply = BTreeMap::<u32, String>::new();
        for (_, event) in events.iter() {
            if let PageAssistEventV1::Text { delta, .. } = &event.kind {
                text_by_reply
                    .entry(event.reply)
                    .or_default()
                    .push_str(delta);
            }
        }
        let spoken = normalize(text);
        let matching: Vec<_> = text_by_reply
            .iter()
            .rev()
            .filter(|(_, reply)| {
                !spoken.is_empty()
                    && format!(" {} ", normalize(reply)).contains(&format!(" {spoken} "))
            })
            .map(|(reply, _)| *reply)
            .collect();
        if matching.len() == 1 {
            return matching.first().copied();
        }
        // Typed closings have no model text; they belong to the admitted reply.
        // Ambiguous repeated sentences use the current admission as well.
        self.metrics
            .with(|f| f.reply_utterances.keys().next_back().copied())
    }
}
struct Socket {
    inner: VoiceLink,
    link: Arc<Link>,
}

impl vp::VoiceSocket for Socket {
    fn listen_start(&self, id: &str, language: Option<&str>) -> bool {
        let ok = self.inner.listen_start(id, language);
        if ok {
            self.link.metrics.count("utterances", 1);
            let mut wire = self.link.wire.lock().unwrap_or_else(|p| p.into_inner());
            wire.sent.clear();
            wire.listen_ids.push(id.to_owned());
            let index = self
                .link
                .metrics
                .with(|f| f.utterances.len().saturating_sub(1));
            wire.listen_indices.insert(id.to_owned(), index);
        }
        ok
    }
    fn audio(&self, pcm: &[u8]) -> bool {
        let ok = self.inner.audio(pcm);
        if ok {
            self.link.metrics.first("first_frame_sent");
            self.link
                .metrics
                .count("input_samples", (pcm.len() / 2) as u64);
            let mut wire = self.link.wire.lock().unwrap_or_else(|p| p.into_inner());
            wire.first_frame_seen = true;
            if wire.sent.len() + pcm.len() / 2 <= 24_000 * 120 {
                wire.sent.extend(
                    pcm.chunks_exact(2)
                        .map(|p| i16::from_le_bytes([p[0], p[1]])),
                );
            }
        }
        ok
    }
    fn listen_commit(&self, id: &str) -> bool {
        let ok = self.inner.listen_commit(id);
        if ok {
            self.link.metrics.first("commit_sent");
            let mut wire = self.link.wire.lock().unwrap_or_else(|p| p.into_inner());
            let intact = wire.expected.get(wire.utterance).is_some_and(|expected| {
                !expected.is_empty()
                    && wire
                        .sent
                        .windows(expected.len())
                        .any(|part| part == expected)
            });
            self.link
                .metrics
                .count("intact_utterances", u64::from(intact));
            wire.utterance += 1;
        }
        ok
    }
    fn listen_cancel(&self, id: &str) -> bool {
        self.inner.listen_cancel(id)
    }
    fn speak(&self, id: &str, text: &str, voice: Option<&str>) -> bool {
        let sent = self.link.metrics.now();
        let ok = self.inner.speak(id, text, voice);
        if ok {
            let mut wire = self.link.wire.lock().unwrap_or_else(|p| p.into_inner());
            let has_fact = !self.link.spoken_facts.is_empty()
                && scenarios::has_fact(text, self.link.spoken_facts);
            let kind = if wire.first_frame_seen {
                Kind::Answer
            } else {
                Kind::Line
            };
            let reply = if kind == Kind::Answer {
                self.link.reply_for_speech(text)
            } else {
                None
            };
            self.link.audio.sentence(
                id,
                has_fact,
                Provenance {
                    kind,
                    sent: Some(sent),
                    received: None,
                    reply,
                },
            );
            if kind == Kind::Answer {
                wire.sentences.push((reply, text.to_owned()));
                if has_fact {
                    self.link.metrics.count("fact_sent_to_speak", 1);
                }
            }
            self.link.metrics.first("first_gateway_speak_sent");
            self.link.metrics.count("speeches", 1);
            wire.pending_speech.insert(id.to_owned());
            if kind == Kind::Answer {
                wire.reply_ids.push(id.to_owned());
                self.link.metrics.first("first_speak_sent");
                self.link.metrics.count("sentences_sent", 1);
            } else {
                wire.warm_ids.push(id.to_owned());
                self.link.metrics.count("warm_speeches", 1);
            }
        }
        ok
    }
    fn speak_cancel(&self, id: Option<&str>) -> bool {
        let ok = self.inner.speak_cancel(id);
        if ok {
            self.link.metrics.count("speak_cancel_sent", 1);
            let mut wire = self.link.wire.lock().unwrap_or_else(|p| p.into_inner());
            wire.cancelled_ids = wire.reply_ids.clone();
            self.link.audio.obsolete(&wire.cancelled_ids);
        }
        ok
    }
    fn close(&self) {
        self.inner.close();
    }
}
struct LinkAdapter(Arc<Link>);
impl vp::LinkPort for LinkAdapter {
    fn open(&self) -> BoxFuture<'static, Result<vp::Opened, WorkModelError>> {
        let link = self.0.clone();
        Box::pin(async move {
            let accounting = link.clone();
            let observer = Arc::new(move |record: zephium_agentic::lead::LeadCloudRecord| {
                let mut wire = accounting.wire.lock().unwrap_or_else(|p| p.into_inner());
                if !wire.receipts.insert(record.idempotency_key.clone()) {
                    accounting.metrics.count("duplicate_receipts", 1);
                    return;
                }
                if record.request_id.is_some() {
                    let route = if wire.listen_ids.contains(&record.idempotency_key) {
                        "ledger_listen_receipts"
                    } else {
                        "ledger_speak_receipts"
                    };
                    accounting.metrics.count(route, 1);
                } else {
                    accounting.metrics.count("unadmitted_requests", 1);
                }
                if !matches!(
                    record.accounting,
                    LeadCloudAccounting::Settled { .. } | LeadCloudAccounting::Refused
                ) {
                    accounting.metrics.count("unsettled_receipts", 1);
                }
            });
            let (socket, ready, mut incoming) = VoiceLink::open(
                &link.gateway.target,
                &link.gateway.credential,
                Some(observer),
            )
            .await?;
            let (send, events) = mpsc::channel(192);
            let observe = link.clone();
            tokio::spawn(async move {
                let mut speech: Option<String> = None;
                while let Some(event) = incoming.recv().await {
                    let metrics = &observe.metrics;
                    match &event {
                        VoiceEvent::Partial { id, text } => {
                            let index = observe
                                .wire
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .listen_indices
                                .get(id)
                                .copied();
                            if let Some(index) = index {
                                metrics.partial(index, text);
                            }
                        }
                        VoiceEvent::Final { id, text } => {
                            let index = observe
                                .wire
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .listen_indices
                                .get(id)
                                .copied();
                            if let Some(index) = index {
                                metrics.final_transcript(
                                    index,
                                    text,
                                    if index == 0 {
                                        observe.expected
                                    } else {
                                        observe.second_expected
                                    },
                                );
                            }
                        }
                        VoiceEvent::SpeakStart { id } => speech = Some(id.clone()),
                        VoiceEvent::Audio(bytes) => {
                            if let Some(id) = &speech {
                                observe.audio.source(id, bytes);
                                let wire = observe.wire.lock().unwrap_or_else(|p| p.into_inner());
                                if !wire.warm_ids.contains(id) {
                                    metrics.first("first_audio_received");
                                    metrics.count("output_samples", (bytes.len() / 2) as u64);
                                } else {
                                    metrics.first("first_warm_audio_received");
                                    metrics.count("warm_output_samples", (bytes.len() / 2) as u64);
                                }
                            }
                        }
                        VoiceEvent::SpeakEnd { id } => {
                            observe
                                .wire
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .pending_speech
                                .remove(id);
                            metrics.at("gateway_speech_end");
                            speech = None;
                        }
                        VoiceEvent::SpeakFailed { id, error } => {
                            observe
                                .wire
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .pending_speech
                                .remove(id);
                            if *error == VoiceError::Cancelled
                                && observe
                                    .wire
                                    .lock()
                                    .unwrap_or_else(|p| p.into_inner())
                                    .cancelled_ids
                                    .contains(id)
                            {
                                metrics.count("speak_cancel_acknowledged", 1);
                            }
                            metrics.with(|f| {
                                *f.faults.entry(fault_code(*error).to_owned()).or_default() += 1
                            });
                            speech = None;
                        }
                        VoiceEvent::ListenFailed { error, .. } => {
                            metrics.with(|f| {
                                *f.faults.entry(fault_code(*error).to_owned()).or_default() += 1
                            });
                        }
                        VoiceEvent::Closing { reason } => {
                            metrics.with(|f| {
                                *f.faults.entry(format!("closing_{reason:?}")).or_default() += 1
                            });
                        }
                        _ => {}
                    }
                    if send.send(event).await.is_err() {
                        break;
                    }
                }
            });
            Ok(vp::Opened {
                socket: Arc::new(Socket {
                    inner: socket,
                    link,
                }),
                ready,
                events,
            })
        })
    }
}

struct Assistant {
    shared: Arc<Shared>,
    front: String,
    metrics: Arc<Metrics>,
    replies: Arc<Mutex<Vec<vp::Asked>>>,
}
impl Assistant {
    fn ask(
        &self,
        text: String,
        steer: bool,
        spoken: bool,
    ) -> BoxFuture<'static, Result<vp::Asked, Fault>> {
        let shared = self.shared.clone();
        let front = self.front.clone();
        let replies = self.replies.clone();
        let metrics = self.metrics.clone();
        let utterance = metrics.with(|f| f.utterances.len().saturating_sub(1));
        Box::pin(async move {
            let service = shared.service.get().ok_or(Fault::Unavailable)?;
            let status = service.status(&shared.space, Some(&front)).await;
            let conversation = status.conversation.id;
            let request = PageAssistAskV1 {
                space: shared.space.clone(),
                conversation: conversation.clone(),
                text,
                suggestion: None,
                attachments: vec![PageAssistAttachV1::Page { tab: front }],
            };
            let delivery = if spoken {
                Delivery::Spoken
            } else {
                Delivery::Shown
            };
            let (reply, task) = if steer {
                service.steer(request, delivery).await
            } else {
                service.ask_as(request, delivery).await
            }
            .map_err(|fault| Fault::from_assist(&fault))?;
            let asked = vp::Asked {
                conversation,
                reply,
            };
            replies
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(asked.clone());
            metrics.with(|f| {
                f.reply_utterances.insert(asked.reply, utterance);
            });
            tokio::spawn(task);
            Ok(asked)
        })
    }
}
impl vp::AssistantPort for Assistant {
    fn ask(&self, text: String, spoken: bool) -> BoxFuture<'static, Result<vp::Asked, Fault>> {
        self.metrics.count("assistant_asks", 1);
        self.ask(text, false, spoken)
    }
    fn steer(
        &self,
        _: String,
        _: u32,
        text: String,
        spoken: bool,
    ) -> BoxFuture<'static, Result<vp::Asked, Fault>> {
        self.metrics.count("assistant_steers", 1);
        self.ask(text, true, spoken)
    }
    fn stop(&self, _: &str, _: u32) {
        if let Some(service) = self.shared.service.get() {
            service.stop(&self.shared.space);
        }
    }
    fn decide(
        &self,
        conversation: &str,
        reply: u32,
        block: u16,
        decision: PageAssistDecisionV1,
    ) -> bool {
        self.shared
            .service
            .get()
            .is_some_and(|s| s.decide(conversation, reply, block, decision))
    }
}

struct Projection(Arc<Metrics>);
impl vp::Projection for Projection {
    fn view(&self, view: &VoiceSessionViewV1) {
        let state = match view.state {
            VoiceSessionStateV1::Idle => "idle".into(),
            VoiceSessionStateV1::Arming { .. } => "arming".into(),
            VoiceSessionStateV1::Listening { .. } => "listening".into(),
            VoiceSessionStateV1::Finalizing => "finalizing".into(),
            VoiceSessionStateV1::Working { work } => format!("working_{work:?}").to_lowercase(),
            VoiceSessionStateV1::Settled { .. } => "settled".into(),
        };
        self.0.with(|f| {
            if f.states.last() != Some(&state) {
                f.states.push(state);
            }
            if let Some(fault) = view.fault {
                let fault = serde_json::to_value(fault).unwrap_or_default();
                if f.session_faults.last() != Some(&fault) {
                    f.session_faults.push(fault);
                }
            }
        });
    }
    fn levels(&self, _: VoiceSessionLevelsV1) {
        self.0.count("level_events", 1);
    }
}
struct Clock(Arc<Metrics>);
impl vp::Clock for Clock {
    fn now_ms(&self) -> u64 {
        self.0.now()
    }
}

async fn press(
    session: &VoiceSession,
    audio: &ScriptedAudio,
    metrics: &Metrics,
    barge: bool,
) -> Result<(), &'static str> {
    metrics.begin_utterance();
    metrics.at(if barge { "barge_key_down" } else { "key_down" });
    if barge {
        metrics.count("barge_while_playing", u64::from(audio.playing_answer()));
    }
    if !session.send(Command::KeyDown) {
        return Err("voice_session_ended");
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    let pressed = metrics.now();
    while !audio.delivered() || metrics.now().saturating_sub(pressed) < 300 {
        if Instant::now() > deadline {
            return Err("voice_capture_timeout");
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    // Hold-to-talk uses >=300 ms; even a synthesized single word has padding.
    metrics.keyup(barge);
    if !session.send(Command::KeyUp) {
        return Err("voice_session_ended");
    }
    Ok(())
}

async fn scenario_run(
    shared: Arc<Shared>,
    scenario: &'static VoiceScenario,
    fixture: Option<Arc<Fixture>>,
    gateway: Arc<Gateway>,
) -> Result<serde_json::Value, &'static str> {
    let base = scenario.fixture().ok_or("voice_fixture")?;
    let (service, front, status) = prepare(&shared, base, fixture.as_deref()).await?;
    if status.availability != PageAssistAvailabilityV1::Ready {
        return Ok(
            serde_json::json!({"scenario": scenario.name, "harness_fault": "page_model_unavailable", "availability": availability_name(status.availability)}),
        );
    }
    let input = if scenario.silence {
        vec![0; 24_000 * 3 / 2]
    } else {
        gateway
            .synthesize(scenario.text().ok_or("voice_request")?)
            .await?
    };
    let stop = if scenario.barge {
        Some(gateway.synthesize(scenario.second_text()).await?)
    } else {
        None
    };
    let origin = Instant::now();
    let _ = shared.asked.set(origin);
    shared.count(|c| *c = Counters::default());
    let metrics = Metrics::new(origin);
    let _ = shared.voice_metrics.set(metrics.clone());
    metrics.count("earcons", 0);
    let audio = ScriptedAudio::new(metrics.clone());
    let wire = Arc::new(Mutex::new(WireFacts {
        expected: [Some(input.clone()), stop.clone()]
            .into_iter()
            .flatten()
            .collect(),
        ..WireFacts::default()
    }));
    audio.arm(input);
    let link = Arc::new(Link {
        gateway,
        metrics: metrics.clone(),
        audio: audio.clone(),
        wire,
        shared: shared.clone(),
        expected: scenario.expected,
        second_expected: scenario.second_expected(),
        spoken_facts: if scenario.name == "voice-count" {
            &["5", "five"]
        } else if scenario.name == "voice-fact" {
            match base.grade {
                Grade::Fact(facts) => facts,
                _ => &[],
            }
        } else {
            &[]
        },
    });
    let replies = Arc::new(Mutex::new(Vec::new()));
    let (session, driver) = VoiceSession::new(
        voice_session::Ports {
            audio: audio.clone(),
            link: Arc::new(LinkAdapter(link.clone())),
            assistant: Arc::new(Assistant {
                shared: shared.clone(),
                front,
                metrics: metrics.clone(),
                replies: replies.clone(),
            }),
            projection: Arc::new(Projection(metrics.clone())),
            clock: Arc::new(Clock(metrics.clone())),
            ids: Arc::new(uuid),
        },
        Setup {
            preferences: VoiceSessionPreferencesV1 {
                spoken: true,
                voice: None,
                language: VoiceLanguageV1::En,
                earcons: false,
            },
            availability: VoiceAvailabilityV1::Available,
            system_language: Language::En,
        },
    );
    let _ = shared.voice.set(session.clone());
    let driver = tokio::spawn(driver);
    let pump = audio.spawn();
    session.send(Command::Foreground(true));
    // Keep link-opening latency in the measurement, as on a cold talk press.
    let mut confirms = Vec::new();
    let mut first_text = None;
    let mut ended = None;
    let run = async {
        press(&session, &audio, &metrics, false).await?;
        let deadline = Instant::now()+REPLY_WAIT;
        let mut seen = 0;
        let mut steps = BTreeMap::new();
        let mut decided = Vec::new();
        let mut barged = false;
        loop {
            let fresh = { let events = shared.events.lock().unwrap_or_else(|p| p.into_inner()); events[seen..].to_vec() };
            seen += fresh.len();
            let first = replies.lock().unwrap_or_else(|p| p.into_inner()).first().cloned();
            for (at, event) in fresh {
                match event.kind {
                    PageAssistEventV1::Text { .. } => { metrics.with(|f| { f.times.entry("assistant_first_text").or_insert(at); }); if first.as_ref().is_some_and(|r| r.reply == event.reply) { first_text.get_or_insert(at); } }
                    PageAssistEventV1::Block { block, value: PageAssistBlockV1::Steps { steps: rows } } if first.as_ref().is_some_and(|r| r.reply == event.reply) => { steps.insert(block, rows); }
                    PageAssistEventV1::Failed { fault: PageAssistFaultV1::NoModel } => return Err("page_model_unavailable"),
                    PageAssistEventV1::Failed { fault: PageAssistFaultV1::Model { fault: WorkModelFaultV1::MissingKey, .. } } => return Err("page_model_missing_key"),
                    PageAssistEventV1::Failed { fault: PageAssistFaultV1::Model { fault: WorkModelFaultV1::KeyRefused, .. } } => return Err("page_model_key_refused"),
                    PageAssistEventV1::Done { .. } | PageAssistEventV1::Failed { .. } | PageAssistEventV1::Cancelled => {
                        metrics.with(|f| { f.times.insert("reply_end", at); });
                        if first.as_ref().is_some_and(|r| r.reply == event.reply) { ended = Some(at); }
                    }
                    _ => {}
                }
            }
            if let Some(first) = &first { decide_confirms(&shared, base, &first.conversation, first.reply, &steps, &mut decided, &mut confirms); }
            if let Some(at) = shared.counters.lock().unwrap_or_else(|p| p.into_inner()).first_action_ms { metrics.with(|f| { f.times.entry("first_action").or_insert(at); }); }
            if scenario.barge && !barged && metrics.with(|f| f.times.get("first_answer_audio_played").is_some_and(|at| metrics.now() >= at+600)) {
                audio.arm(stop.clone().ok_or("voice_stop_audio")?);
                press(&session, &audio, &metrics, true).await?;
                barged = true;
            }
            let final_count = metrics.with(|f| f.utterances.iter().filter(|u| u.final_at.is_some()).count());
            let completed = session.view().is_some_and(|v| matches!(v.state, VoiceSessionStateV1::Idle | VoiceSessionStateV1::Settled { .. }) && !v.speaking);
            if completed && audio.idle() && ((scenario.silence && final_count >= 1) || (!scenario.barge && ended.is_some()) || (scenario.barge && barged && final_count >= 2)) { break; }
            if Instant::now() >= deadline { return Err("voice_reply_timeout"); }
            if session.view().is_some_and(|v| v.fault.is_some()) { return Err("voice_session_fault"); }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        metrics.at("loop_end");
        let ledger_deadline = Instant::now() + Duration::from_secs(15);
        while !link.wire.lock().unwrap_or_else(|p| p.into_inner()).pending_speech.is_empty() {
            if Instant::now() >= ledger_deadline { return Err("voice_speech_settlement_timeout"); }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        shared.frozen.store(true, Ordering::Release);
        let first = replies.lock().unwrap_or_else(|p| p.into_inner()).first().cloned();
        let mut row = if let Some(first) = first {
            grade_reply(&shared, base, fixture.as_deref(), ReplyResult { reply: first.reply, ended, first_text, timed_out: false, confirms: &confirms, model: status.model.clone(), availability: "ready", labels: false }).await
        } else { serde_json::json!({"model": status.model.clone(), "task_verified": null, "false_done": false, "claimed": "quiet", "model_calls": 0, "steps": 0, "confirms": []}) };
        add_speech_diagnostics(&mut row, &shared, &link, &metrics, scenario);
        row["assist_reply_end"] = row["reply_end"].take();
        row["voice"] = metrics.result();
        row["scenario"] = scenario.name.into();
        row["synthesis_speeches"] = (if scenario.silence { 0 } else if scenario.barge { 2 } else { 1 }).into();
        row["earcons_enabled"] = false.into();
        Ok(row)
    }.await;
    session.send(Command::Dispose);
    let _ = tokio::time::timeout(Duration::from_secs(2), driver).await;
    pump.abort();
    service.stop(&shared.space);
    match run {
        Ok(row) => Ok(row),
        Err(stage) => {
            metrics.first("loop_end");
            shared.frozen.store(true, Ordering::Release);
            let first = replies
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .first()
                .cloned();
            let mut row = if let Some(first) = first {
                grade_reply(
                    &shared,
                    base,
                    fixture.as_deref(),
                    ReplyResult {
                        reply: first.reply,
                        ended,
                        first_text,
                        timed_out: stage.ends_with("timeout"),
                        confirms: &confirms,
                        model: status.model.clone(),
                        availability: "ready",
                        labels: false,
                    },
                )
                .await
            } else {
                serde_json::json!({"model": status.model})
            };
            row["scenario"] = scenario.name.into();
            row["harness_fault"] = stage.into();
            add_speech_diagnostics(&mut row, &shared, &link, &metrics, scenario);
            row["voice"] = metrics.result();
            row["synthesis_speeches"] = (if scenario.silence {
                0
            } else if scenario.barge {
                2
            } else {
                1
            })
            .into();
            Ok(row)
        }
    }
}

fn unspoken_sentences(text: &str, remaining: &mut String) -> usize {
    use zephium_work_composition::page_assist::core::voice::{phrases::normalize, Cutter};
    let mut cutter = Cutter::default();
    let mut chunks = cutter.push(text);
    chunks.extend(cutter.finish());
    chunks
        .iter()
        .filter(|chunk| {
            let spoken = format!(" {} ", normalize(chunk));
            if let Some(at) = remaining.find(&spoken) {
                remaining.replace_range(at..at + spoken.len(), " ");
                false
            } else {
                true
            }
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn voice_unspoken_sentences_count_missing_repetitions_and_number_tail() {
        let mut sent = " action finished ".to_owned();
        assert_eq!(
            unspoken_sentences("Action finished. It serves five.", &mut sent),
            1
        );
        let mut sent = " it serves five ".to_owned();
        assert_eq!(
            unspoken_sentences("It serves five. It serves five.", &mut sent),
            1
        );
        let mut sent = " it serves five ".to_owned();
        assert_eq!(unspoken_sentences("It serves **five**.", &mut sent), 0);
    }
}

fn add_speech_diagnostics(
    row: &mut serde_json::Value,
    shared: &Shared,
    link: &Link,
    metrics: &Metrics,
    scenario: &VoiceScenario,
) {
    use zephium_work_composition::page_assist::core::voice::phrases::normalize;
    let saved = shared.saved.lock().unwrap_or_else(|p| p.into_inner());
    let wire = link.wire.lock().unwrap_or_else(|p| p.into_inner());
    let first_reply = metrics.with(|f| {
        f.reply_utterances
            .iter()
            .find(|(_, u)| **u == 0)
            .map(|(r, _)| *r)
    });
    let view = first_reply.and_then(|reply| reply_view(saved.as_deref(), reply));
    let mut unspoken = 0;
    let mut blocks_trace = Vec::new();
    let mut count_in_reply = false;
    let spoken = wire
        .sentences
        .iter()
        .filter(|(reply, _)| *reply == first_reply)
        .map(|(_, text)| normalize(text))
        .collect::<Vec<_>>()
        .join(" ");
    let mut remaining = format!(" {spoken} ");
    if let Some((state, blocks, _, _, _)) = view {
        for (block, value) in blocks.iter().enumerate() {
            if let PageAssistBlockV1::Text { text, .. } = value {
                count_in_reply |= scenarios::has_fact(text, &["5", "five"]);
                let missing = unspoken_sentences(text, &mut remaining);
                unspoken += missing;
                if missing > 0 {
                    blocks_trace.push(serde_json::json!({"reply": first_reply, "block": block, "reply_state": state,
                        "session_state": metrics.with(|f| f.states.last().cloned()), "sentences_unspoken": missing,
                        "count_in_text": scenarios::has_fact(text, &["5", "five"])}));
                }
            }
        }
    }
    row["reply_sentences_unspoken"] = unspoken.into();
    row["unspoken_reply_blocks"] = blocks_trace.into();
    if scenario.name == "voice-count" {
        row["answer_fact"] = count_in_reply.into();
        row["count_in_reply_text"] = count_in_reply.into();
        row["count_in_spoken_text"] = wire
            .sentences
            .iter()
            .any(|(reply, text)| *reply == first_reply && scenarios::has_fact(text, &["5", "five"]))
            .into();
    }
}

pub(in crate::assist_bench) fn run(
    name: &std::ffi::OsStr,
) -> Result<(), super::super::ProbeFailure> {
    let scenario = name
        .to_str()
        .and_then(|name| voice_scenarios::SCENARIOS.iter().find(|s| s.name == name))
        .ok_or(super::super::ProbeFailure::Authority)?;
    page_effort_override().map_err(|stage| {
        say(format_args!("voice-run-failure: stage={stage}"));
        super::super::ProbeFailure::Engine(stage)
    })?;
    let gateway = Gateway::ready_file().map_err(|stage| {
        say(format_args!("voice-run-failure: stage={stage}"));
        super::super::ProbeFailure::Engine(stage)
    })?;
    run_host(
        scenario
            .fixture()
            .ok_or(super::super::ProbeFailure::Authority)?,
        move |shared, fixture| Box::pin(scenario_run(shared, scenario, fixture, gateway)),
        "voice-run",
    )
}
