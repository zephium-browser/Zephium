use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use super::*;
use zephium_ipc::page_assist::PageAssistEventV1 as AssistEvent;

type Log = Arc<Mutex<Vec<String>>>;

fn push(log: &Log, entry: String) {
    log.lock().unwrap().push(entry);
}

struct FakeAudio {
    log: Log,
    prepare: Mutex<Result<(), Fault>>,
}

impl AudioPort for FakeAudio {
    fn prepare(&self, _inbox: Arc<AudioInbox>) -> BoxFuture<'static, Result<(), Fault>> {
        push(&self.log, "prepare".into());
        let result = *self.prepare.lock().unwrap();
        Box::pin(async move { result })
    }
    fn release(&self) {
        push(&self.log, "release".into());
    }
    fn start(&self) -> BoxFuture<'static, Result<(), Fault>> {
        push(&self.log, "start".into());
        Box::pin(async { Ok(()) })
    }
    fn standby(&self) {
        push(&self.log, "standby".into());
    }
    fn gate(&self, on: bool) {
        push(&self.log, format!("gate:{on}"));
    }
    fn play(&self, pcm: &[i16]) {
        push(&self.log, format!("play:{}", pcm[0]));
    }
    fn mark(&self, mark: u64) {
        push(&self.log, format!("mark:{mark}"));
    }
    fn flush(&self) {
        push(&self.log, "flush".into());
    }
}

struct FakeSocket {
    log: Log,
}

impl VoiceSocket for FakeSocket {
    fn listen_start(&self, id: &str, language: Option<&str>) -> bool {
        push(&self.log, format!("listen_start:{id}:{language:?}"));
        true
    }
    fn audio(&self, pcm: &[u8]) -> bool {
        assert_eq!(pcm.len(), FRAME * 2);
        push(
            &self.log,
            format!("audio:{}", i16::from_le_bytes([pcm[0], pcm[1]])),
        );
        true
    }
    fn listen_commit(&self, id: &str) -> bool {
        push(&self.log, format!("commit:{id}"));
        true
    }
    fn listen_cancel(&self, id: &str) -> bool {
        push(&self.log, format!("listen_cancel:{id}"));
        true
    }
    fn speak(&self, id: &str, text: &str, _voice: Option<&str>) -> bool {
        push(&self.log, format!("speak:{id}:{text}"));
        true
    }
    fn speak_cancel(&self, id: Option<&str>) -> bool {
        push(&self.log, format!("speak_cancel:{id:?}"));
        true
    }
    fn close(&self) {
        push(&self.log, "close".into());
    }
}

struct FakeLink {
    log: Log,
    refusal: Mutex<Option<WorkModelError>>,
}

impl LinkPort for FakeLink {
    fn open(&self) -> BoxFuture<'static, Result<Opened, WorkModelError>> {
        push(&self.log, "open_link".into());
        let refusal = *self.refusal.lock().unwrap();
        let log = self.log.clone();
        Box::pin(async move {
            if let Some(error) = refusal {
                return Err(error);
            }
            let (_events, receiver) = mpsc::channel(4);
            Ok(Opened {
                socket: Arc::new(FakeSocket { log }),
                ready: VoiceReady {
                    policy_revision: None,
                    listen_max_ms: 240_000,
                    speak_max_chars: 600,
                    voices: vec!["marin".into()],
                    voice: "marin".into(),
                },
                events: receiver,
            })
        })
    }
}

struct FakeAssistant {
    log: Log,
    replies: AtomicU32,
}

impl AssistantPort for FakeAssistant {
    fn ask(&self, text: String, spoken: bool) -> BoxFuture<'static, Result<Asked, Fault>> {
        push(&self.log, format!("ask:{text}"));
        assert!(spoken);
        let reply = self.replies.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(Asked {
                conversation: "c1".into(),
                reply,
            })
        })
    }
    fn steer(
        &self,
        conversation: String,
        reply: u32,
        text: String,
        spoken: bool,
    ) -> BoxFuture<'static, Result<Asked, Fault>> {
        push(&self.log, format!("steer:{reply}:{text}"));
        assert!(spoken);
        let next = self.replies.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            Ok(Asked {
                conversation,
                reply: next,
            })
        })
    }
    fn stop(&self, _conversation: &str, reply: u32) {
        push(&self.log, format!("stop:{reply}"));
    }
    fn decide(
        &self,
        _conversation: &str,
        reply: u32,
        block: u16,
        decision: PageAssistDecisionV1,
    ) -> bool {
        push(&self.log, format!("decide:{reply}:{block}:{decision:?}"));
        true
    }
}

#[derive(Default)]
struct FakeProjection {
    views: Mutex<Vec<VoiceSessionViewV1>>,
    levels: Mutex<Vec<VoiceSessionLevelsV1>>,
}

impl Projection for FakeProjection {
    fn view(&self, view: &VoiceSessionViewV1) {
        self.views.lock().unwrap().push(view.clone());
    }
    fn levels(&self, levels: VoiceSessionLevelsV1) {
        self.levels.lock().unwrap().push(levels);
    }
}

#[derive(Default)]
struct ManualClock(AtomicU64);

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct Harness {
    core: Core,
    receiver: mpsc::UnboundedReceiver<Input>,
    log: Log,
    clock: Arc<ManualClock>,
    projection: Arc<FakeProjection>,
    audio: Arc<FakeAudio>,
    link: Arc<FakeLink>,
    hold_link: bool,
    hold_ask: bool,
    held: Vec<Input>,
    asks: Vec<Input>,
    generation: u64,
}

fn preferences() -> VoiceSessionPreferencesV1 {
    VoiceSessionPreferencesV1 {
        spoken: true,
        voice: None,
        language: VoiceLanguageV1::Automatic,
        earcons: false,
    }
}

fn done() -> AssistEvent {
    AssistEvent::Done {
        cost: None,
        end: None,
    }
}

impl Harness {
    fn new(availability: VoiceAvailabilityV1, hold_link: bool) -> Self {
        let log: Log = Arc::default();
        let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
        let projection = Arc::new(FakeProjection::default());
        let audio = Arc::new(FakeAudio {
            log: log.clone(),
            prepare: Mutex::new(Ok(())),
        });
        let link = Arc::new(FakeLink {
            log: log.clone(),
            refusal: Mutex::new(None),
        });
        let counter = Arc::new(AtomicU64::new(0));
        let ports = Ports {
            audio: audio.clone(),
            link: link.clone(),
            assistant: Arc::new(FakeAssistant {
                log: log.clone(),
                replies: AtomicU32::new(0),
            }),
            projection: projection.clone(),
            clock: clock.clone(),
            ids: Arc::new(move || {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                format!("00000000-0000-4000-8000-{n:012x}")
            }),
        };
        let (core, receiver) = core_for_tests(
            ports,
            Setup {
                preferences: preferences(),
                availability,
                system_language: Language::En,
            },
        );
        Self {
            core,
            receiver,
            log,
            clock,
            projection,
            audio,
            link,
            hold_link,
            hold_ask: false,
            held: Vec::new(),
            asks: Vec::new(),
            generation: 0,
        }
    }

    async fn settle(&mut self) {
        loop {
            let mut progressed = false;
            for task in std::mem::take(&mut self.core.tasks) {
                let input = task.await;
                if self.hold_link && matches!(input, Input::LinkOpened { .. }) {
                    self.held.push(input);
                } else if self.hold_ask && matches!(input, Input::Asked(_)) {
                    self.asks.push(input);
                } else {
                    self.core.input(input);
                }
                progressed = true;
            }
            for (generation, _) in std::mem::take(&mut self.core.pumps) {
                self.generation = generation;
            }
            while let Ok(input) = self.receiver.try_recv() {
                self.core.input(input);
                progressed = true;
            }
            self.core.poll();
            if !progressed && self.core.tasks.is_empty() {
                break;
            }
        }
    }

    async fn start(&mut self) {
        self.core.start();
        self.settle().await;
    }

    async fn send(&mut self, command: Command) {
        self.core.input(Input::Command(command));
        self.settle().await;
    }

    async fn release_link(&mut self) {
        self.hold_link = false;
        for input in std::mem::take(&mut self.held) {
            self.core.input(input);
        }
        self.settle().await;
    }

    async fn release_ask(&mut self) {
        self.hold_ask = false;
        for input in std::mem::take(&mut self.asks) {
            self.core.input(input);
        }
        self.settle().await;
    }

    async fn link(&mut self, event: VoiceEvent) {
        self.core.input(Input::Link {
            generation: self.generation,
            event,
        });
        self.settle().await;
    }

    async fn reply(&mut self, reply: u32, kind: AssistEvent) {
        self.core.input(Input::Assist(Box::new(PageAssistChangedV1 {
            space: "s1".into(),
            conversation: "c1".into(),
            reply,
            seq: 0,
            kind,
        })));
        self.settle().await;
    }

    async fn advance(&mut self, ms: u64) {
        self.clock.0.fetch_add(ms, Ordering::SeqCst);
        self.core.poll();
        self.settle().await;
    }

    async fn frames(&mut self, values: std::ops::RangeInclusive<i16>) {
        let inbox = self.core.inbox();
        for value in values {
            inbox.frame(&[value; FRAME]);
            if value % 32 == 0 {
                self.core.poll();
            }
        }
        self.core.poll();
        self.settle().await;
    }

    fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }

    fn view(&self) -> VoiceSessionViewV1 {
        self.projection
            .views
            .lock()
            .unwrap()
            .last()
            .cloned()
            .unwrap()
    }

    fn utterance(&self) -> String {
        self.log
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|entry| entry.strip_prefix("listen_start:"))
            .map(|rest| rest[..36].to_owned())
            .expect("an utterance")
    }

    fn speeches(log: &[String]) -> Vec<(String, String)> {
        log.iter()
            .filter_map(|entry| entry.strip_prefix("speak:"))
            .map(|rest| (rest[..36].to_owned(), rest[37..].to_owned()))
            .collect()
    }

    async fn play(&mut self, id: &str, first: i16) {
        self.link(VoiceEvent::SpeakStart { id: id.into() }).await;
        let bytes: Vec<u8> = [first; 240].iter().flat_map(|s| s.to_le_bytes()).collect();
        self.link(VoiceEvent::Audio(bytes)).await;
        self.link(VoiceEvent::SpeakEnd { id: id.into() }).await;
    }

    async fn played(&mut self, mark: u64) {
        self.core
            .input(Input::Device(port::DeviceEvent::Played(mark)));
        self.settle().await;
    }

    /// Talks an utterance that the gateway hears as `heard`.
    async fn talk(&mut self, partial: &str, heard: &str) {
        self.send(Command::KeyDown).await;
        self.frames(1..=5).await;
        let id = self.utterance();
        self.link(VoiceEvent::Partial {
            id: id.clone(),
            text: partial.into(),
        })
        .await;
        self.advance(400).await;
        self.send(Command::KeyUp).await;
        self.link(VoiceEvent::Final {
            id,
            text: heard.into(),
        })
        .await;
    }

    async fn warm_up(&mut self) {
        let log = self.log.lock().unwrap().clone();
        for (n, (id, _)) in Self::speeches(&log).into_iter().enumerate() {
            self.play(&id, 100 + n as i16).await;
        }
    }
}

fn plays(log: &[String]) -> Vec<i16> {
    log.iter()
        .filter_map(|entry| entry.strip_prefix("play:"))
        .map(|value| value.parse().unwrap())
        .collect()
}

#[tokio::test]
async fn pre_roll_frames_reach_the_link_in_order_after_a_slow_open() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, true);
    h.start().await;
    h.send(Command::KeyDown).await;
    h.frames(1..=80).await;
    let before = h.take_log();
    assert!(before.contains(&"start".to_owned()));
    assert!(!before
        .iter()
        .any(|e| e.starts_with("audio:") || e.starts_with("listen_start")));
    h.release_link().await;
    h.advance(400).await;
    h.send(Command::KeyUp).await;
    for _ in 0..20 {
        h.advance(20).await;
    }
    let log = h.take_log();
    let start = log
        .iter()
        .position(|e| e.starts_with("listen_start"))
        .unwrap();
    let audio: Vec<i16> = log
        .iter()
        .filter_map(|e| e.strip_prefix("audio:"))
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(audio, (1..=80).collect::<Vec<i16>>());
    let first_audio = log.iter().position(|e| e.starts_with("audio:")).unwrap();
    let last_audio = log.iter().rposition(|e| e.starts_with("audio:")).unwrap();
    let commit = log.iter().position(|e| e.starts_with("commit:")).unwrap();
    assert!(start < first_audio && last_audio < commit);
}

#[tokio::test]
async fn a_held_question_is_asked_and_its_reply_is_spoken_in_order() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.talk("what is", "What is here?").await;
    assert_eq!(h.view().heard.as_deref(), Some("What is here?"));
    let log = h.take_log();
    assert!(log.contains(&"ask:What is here?".to_owned()));
    assert!(matches!(
        h.view().state,
        VoiceSessionStateV1::Working { .. }
    ));
    h.reply(
        1,
        AssistEvent::Text {
            block: 0,
            delta: "The page lists three flights to Paris. The cheapest leaves at nine. ".into(),
        },
    )
    .await;
    h.reply(1, done()).await;
    let log = h.take_log();
    let speeches = Harness::speeches(&log);
    assert!(!speeches.is_empty(), "{log:?}");
    assert!(speeches[0].1.starts_with("The page lists"));
    for (n, (id, _)) in speeches.iter().enumerate() {
        h.play(id, 10 + n as i16).await;
    }
    let log = h.take_log();
    assert_eq!(
        plays(&log),
        (0..speeches.len())
            .map(|n| 10 + n as i16)
            .collect::<Vec<_>>()
    );
    assert!(h.view().speaking);
    let marks = log.iter().filter(|e| e.starts_with("mark:")).count() as u64;
    h.played(marks).await;
    assert!(!h.view().speaking);
    assert_eq!(
        h.view().state,
        VoiceSessionStateV1::Settled {
            outcome: VoiceOutcomeV1::Answered
        }
    );
    h.advance(7_000).await;
    let now = h.core.now();
    assert_eq!(
        h.core.next_wake(now),
        Some(now + LINK_IDLE_MS),
        "an idle session does not tick"
    );
    assert_eq!(
        h.view().state,
        VoiceSessionStateV1::Idle,
        "the outcome faded"
    );
}

#[tokio::test]
async fn barge_in_flushes_the_device_first_and_drops_late_audio() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.talk("what is", "What is here?").await;
    h.reply(
        1,
        AssistEvent::Text {
            block: 0,
            delta: "A long answer starts here. It goes on and on. ".into(),
        },
    )
    .await;
    let speeches = Harness::speeches(&h.take_log());
    let (id, _) = speeches[0].clone();
    h.link(VoiceEvent::SpeakStart { id: id.clone() }).await;
    h.link(VoiceEvent::Audio(vec![7, 0, 7, 0])).await;
    assert!(h.view().speaking);
    h.take_log();
    h.send(Command::KeyDown).await;
    let log = h.take_log();
    assert_eq!(log.first().map(String::as_str), Some("flush"), "{log:?}");
    assert!(log.contains(&"speak_cancel:None".to_owned()));
    h.link(VoiceEvent::Audio(vec![9, 0, 9, 0])).await;
    h.link(VoiceEvent::SpeakEnd { id }).await;
    assert!(plays(&h.take_log()).is_empty());
    assert!(!h.view().speaking);
}

#[tokio::test]
async fn a_cached_line_plays_before_the_streamed_text_that_follows_it() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.send(Command::KeyDown).await;
    let warm = Harness::speeches(&h.log.lock().unwrap().clone());
    assert_eq!(
        warm.len(),
        5,
        "the acknowledgments, the done line and the stopped line warm at the first utterance"
    );
    let id = h.utterance();
    h.warm_up().await;
    h.take_log();
    h.frames(1..=5).await;
    let words = "please find the cheapest flight to paris";
    h.link(VoiceEvent::Partial {
        id: id.clone(),
        text: words.into(),
    })
    .await;
    h.advance(400).await;
    h.send(Command::KeyUp).await;
    let log = h.take_log();
    assert_eq!(
        plays(&log).len(),
        1,
        "the acknowledgment plays at release: {log:?}"
    );
    assert!((100..=102).contains(&plays(&log)[0]));
    assert!(
        Harness::speeches(&log).is_empty(),
        "a cached line is not synthesized"
    );
    h.link(VoiceEvent::Final {
        id,
        text: words.into(),
    })
    .await;
    h.reply(
        1,
        AssistEvent::Text {
            block: 0,
            delta: "The cheapest flight leaves at nine. ".into(),
        },
    )
    .await;
    h.reply(1, done()).await;
    let speeches = Harness::speeches(&h.take_log());
    h.play(&speeches[0].0, 7).await;
    assert_eq!(plays(&h.take_log()), [7]);
}

#[tokio::test]
async fn a_lost_link_fails_the_open_utterance_and_silences_speech() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.send(Command::KeyDown).await;
    h.frames(1..=5).await;
    h.link(VoiceEvent::Closed).await;
    assert_eq!(h.view().fault, Some(VoiceSessionFaultV1::Interrupted));
    assert!(!matches!(
        h.view().state,
        VoiceSessionStateV1::Listening { .. }
    ));
    h.send(Command::KeyUp).await;

    h.talk("what is", "What is here?").await;
    h.reply(
        1,
        AssistEvent::Text {
            block: 0,
            delta: "Something to say here first. And more after it. ".into(),
        },
    )
    .await;
    let speeches = Harness::speeches(&h.take_log());
    h.link(VoiceEvent::SpeakStart {
        id: speeches[0].0.clone(),
    })
    .await;
    h.link(VoiceEvent::Audio(vec![5, 0])).await;
    h.take_log();
    h.link(VoiceEvent::Closed).await;
    let log = h.take_log();
    assert!(log.contains(&"flush".to_owned()), "{log:?}");
    assert!(!h.view().speaking);
}

#[tokio::test]
async fn plan_and_credit_refusals_become_their_faults() {
    for (error, fault) in [
        (
            WorkModelError::Hosted {
                refusal: HostedRefusal::PlanRequired,
                retry_after_ms: None,
            },
            VoiceSessionFaultV1::PlanRequired,
        ),
        (
            WorkModelError::Hosted {
                refusal: HostedRefusal::OutOfCredits,
                retry_after_ms: None,
            },
            VoiceSessionFaultV1::OutOfCredit,
        ),
        (
            WorkModelError::Hosted {
                refusal: HostedRefusal::DailyLimit,
                retry_after_ms: None,
            },
            VoiceSessionFaultV1::DailyLimit,
        ),
        (WorkModelError::Network, VoiceSessionFaultV1::Offline),
    ] {
        let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
        *h.link.refusal.lock().unwrap() = Some(error);
        h.start().await;
        h.send(Command::KeyDown).await;
        assert_eq!(h.view().fault, Some(fault));
    }
    assert_eq!(
        link_fault(VoiceError::Model(WorkModelError::Hosted {
            refusal: HostedRefusal::FreeUsagePaused,
            retry_after_ms: None,
        })),
        Some(Fault::OutOfCredit)
    );
    assert_eq!(link_fault(VoiceError::Cancelled), None);
}

#[tokio::test]
async fn nothing_ticks_or_meters_while_idle() {
    let mut h = Harness::new(VoiceAvailabilityV1::AiOff, false);
    h.start().await;
    let now = h.core.now();
    assert_eq!(h.core.next_wake(now), None);
    h.send(Command::KeyDown).await;
    assert!(
        h.take_log().is_empty(),
        "an unavailable session does nothing"
    );

    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    let now = h.core.now();
    assert_eq!(h.core.next_wake(now), Some(now + LINK_IDLE_MS));
    h.advance(5_000).await;
    assert!(h.projection.levels.lock().unwrap().is_empty());

    h.talk("what is", "").await;
    h.advance(10_000).await;
    let now = h.core.now();
    assert_eq!(h.view().state, VoiceSessionStateV1::Idle);
    assert!(!h.view().mic);
    assert_eq!(h.core.next_wake(now), Some(now + LINK_IDLE_MS));
    let levels = h.projection.levels.lock().unwrap().len();
    assert!(levels > 0);
    h.advance(3_000).await;
    assert_eq!(h.projection.levels.lock().unwrap().len(), levels);
    assert_eq!(
        *h.projection.levels.lock().unwrap().last().unwrap(),
        VoiceSessionLevelsV1 {
            input: 0,
            output: 0
        }
    );
}

#[tokio::test]
async fn dispose_releases_the_microphone_and_the_socket() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.send(Command::KeyDown).await;
    h.frames(1..=3).await;
    h.take_log();
    h.send(Command::Dispose).await;
    let log = h.take_log();
    assert!(log.contains(&"release".to_owned()), "{log:?}");
    assert!(log.contains(&"close".to_owned()), "{log:?}");
    assert!(
        log.iter().any(|e| e.starts_with("listen_cancel:")),
        "{log:?}"
    );
    assert!(h.core.disposed);
    h.send(Command::KeyDown).await;
    assert!(h.take_log().is_empty());
}

#[tokio::test]
async fn ai_off_resets_the_session_and_stops_the_reply() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.talk("what is", "What is here?").await;
    h.take_log();
    h.send(Command::Availability(VoiceAvailabilityV1::AiOff))
        .await;
    let log = h.take_log();
    assert!(log.contains(&"stop:1".to_owned()), "{log:?}");
    assert!(log.contains(&"close".to_owned()));
    assert_eq!(h.view().availability, VoiceAvailabilityV1::AiOff);
    assert_eq!(h.view().state, VoiceSessionStateV1::Idle);
}

#[tokio::test]
async fn a_failed_device_names_the_microphone_fault() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    *h.audio.prepare.lock().unwrap() = Err(Fault::MicrophoneDenied);
    h.start().await;
    h.send(Command::KeyDown).await;
    assert_eq!(h.view().fault, Some(VoiceSessionFaultV1::MicrophoneDenied));
}

#[tokio::test]
async fn standby_and_prepare_follow_the_window_and_the_warm_tail() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.send(Command::Foreground(true)).await;
    assert_eq!(h.take_log(), ["open_link", "prepare", "gate:false"]);
    h.send(Command::KeyDown).await;
    let log = h.take_log();
    assert!(!log.contains(&"prepare".to_owned()));
    assert!(log.contains(&"start".to_owned()));
    h.send(Command::Escape).await;
    h.send(Command::KeyUp).await;
    h.advance(7_000).await;
    assert!(h.take_log().contains(&"standby".to_owned()));
    h.send(Command::Foreground(false)).await;
    assert!(h.take_log().contains(&"release".to_owned()));
}

#[test]
fn long_speech_splits_at_words_within_the_limit() {
    let parts = split("one two three four five", 9);
    assert_eq!(parts, ["one two", "three", "four five"]);
    assert!(split(&"x".repeat(30), 10)
        .iter()
        .all(|p| p.chars().count() <= 10));
    assert!(split("   ", 10).is_empty());
}

#[tokio::test]
async fn a_confirm_is_approved_through_the_click_path_and_a_follow_up_steers() {
    use zephium_ipc::page_assist::{
        PageAssistBlockV1 as Block, PageAssistStepStateV1 as StepState, PageAssistStepV1 as Step,
        WorkConfirmCategoryV1 as Category,
    };
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.talk("buy it", "Buy the blue one").await;
    h.reply(
        1,
        AssistEvent::Block {
            block: 1,
            value: Block::Steps {
                steps: vec![Step {
                    index: 0,
                    label: "Click \"Pay\"".into(),
                    category: Some(Category::Purchase),
                    state: StepState::Confirm,
                    reason: None,
                }],
            },
        },
    )
    .await;
    let confirm = h.view().confirm.expect("a pending confirm");
    assert_eq!(
        (confirm.block, confirm.category),
        (1, Some(Category::Purchase))
    );
    h.take_log();
    h.send(Command::Decide(PageAssistDecisionV1::Approve)).await;
    assert!(h.take_log().contains(&"decide:1:1:Approve".to_owned()));
    assert_eq!(h.view().confirm, None);

    h.talk("and also", "and also check the price").await;
    let log = h.take_log();
    assert!(
        log.contains(&"steer:1:and also check the price".to_owned()),
        "{log:?}"
    );
    h.reply(
        2,
        AssistEvent::Text {
            block: 0,
            delta: "The price is twelve euros. ".into(),
        },
    )
    .await;
    h.reply(2, done()).await;
    let speeches = Harness::speeches(&h.take_log());
    assert!(
        speeches
            .iter()
            .any(|(_, text)| text.starts_with("The price")),
        "{speeches:?}"
    );
}

#[tokio::test]
async fn words_spoken_while_the_ask_is_admitted_steer_its_reply() {
    let mut h = Harness::new(VoiceAvailabilityV1::Available, false);
    h.start().await;
    h.hold_ask = true;
    h.talk("open the", "open the cheapest flight").await;
    h.reply(
        1,
        AssistEvent::Text {
            block: 0,
            delta: "Looking for flights. ".into(),
        },
    )
    .await;
    h.talk("and book", "and book it").await;
    h.talk("and pick", "and pick the window seat").await;
    let log = h.take_log();
    assert!(
        !log.iter().any(|entry| entry.starts_with("steer:")),
        "{log:?}"
    );
    h.release_ask().await;
    let log = h.take_log();
    assert!(
        log.contains(&"steer:1:and pick the window seat".to_owned()),
        "{log:?}"
    );
    assert!(!log.contains(&"steer:1:and book it".to_owned()));
    assert_eq!(h.view().fault, None);
    h.reply(
        2,
        AssistEvent::Text {
            block: 0,
            delta: "Booked the window seat. ".into(),
        },
    )
    .await;
    h.reply(2, done()).await;
    let speeches = Harness::speeches(&h.take_log());
    assert!(
        speeches.iter().any(|(_, text)| text.starts_with("Booked")),
        "{speeches:?}"
    );
    assert!(!speeches.iter().any(|(_, text)| text.starts_with("Looking")));
    assert_eq!(
        h.view().state,
        VoiceSessionStateV1::Settled {
            outcome: VoiceOutcomeV1::Answered
        }
    );
}
