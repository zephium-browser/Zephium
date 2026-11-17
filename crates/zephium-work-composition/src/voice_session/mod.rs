//! The voice session: one reducer per window, its effects executed through
//! ports. Capture never waits on the network, speech plays strictly in the
//! order the reducer chose, and a barge-in silences the device first.
#![deny(clippy::print_stderr, clippy::print_stdout, clippy::dbg_macro)]

mod earcon;
pub mod lines;
pub mod port;
mod view;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use zephium_agentic::lead::voice::{VoiceClosing, VoiceError, VoiceEvent, VoiceReady};
use zephium_core::work::model::{HostedRefusal, WorkModelError};
use zephium_ipc::page_assist::{PageAssistChangedV1, PageAssistDecisionV1, PageAssistEventV1};
use zephium_ipc::voice_session::*;
use zephium_page_assist::voice::{
    Config, Effect, Event, Fault, Language, Session, Speech, State, View,
};

use port::*;
pub use view::project;

const POLL_MS: u64 = 20;
const LEVEL_MS: u64 = 40;
const LINK_IDLE_MS: u64 = 120_000;
/// Frames held for utterances the socket has not started: the device's
/// pre-roll plus a socket open's whole deadline.
const MAX_BUFFERED: usize = 550;
const FIRST_BURST: usize = 50;
const BURST: usize = 5;
const MAX_SPEECH_SAMPLES: usize = 24_000 * 90;
const MAX_LINE_SAMPLES: usize = 24_000 * 12;
const CACHE_LINES: usize = 32;
const CACHE_SAMPLES: usize = 24_000 * 60;
const MAX_OUTSTANDING: usize = 56;
const MAX_HELD: usize = 512;
const DEFAULT_MAX_CHARS: usize = 600;

/// Everything the session reacts to.
pub(crate) enum Input {
    Command(Command),
    Assist(Box<PageAssistChangedV1>),
    Device(port::DeviceEvent),
    Prepared {
        generation: u64,
        result: Result<(), Fault>,
    },
    Started {
        generation: u64,
        result: Result<(), Fault>,
    },
    LinkOpened {
        generation: u64,
        result: Result<Opened, Fault>,
    },
    Link {
        generation: u64,
        event: VoiceEvent,
    },
    Asked(Result<Asked, Fault>),
    Steered(Result<Asked, Fault>),
}

/// What the window asks of the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The talk key went down.
    KeyDown,
    /// The talk key went up.
    KeyUp,
    /// Escape, while claimed.
    Escape,
    /// The on-screen button.
    Intent(VoiceSessionIntentV1),
    /// Approve or decline the pending confirm, as a click does.
    Decide(PageAssistDecisionV1),
    /// The person's settings changed.
    Preferences(VoiceSessionPreferencesV1),
    /// Whether voice can be used now.
    Availability(VoiceAvailabilityV1),
    /// The window became active (true), or has been hidden long (false):
    /// the device is prepared ahead of use, or released while idle.
    Foreground(bool),
    /// The window is closing or the app quits.
    Dispose,
}

/// The ports the session works through.
#[derive(Clone)]
pub struct Ports {
    /// Microphone and speaker.
    pub audio: Arc<dyn AudioPort>,
    /// The gateway socket.
    pub link: Arc<dyn LinkPort>,
    /// The sidebar assistant.
    pub assistant: Arc<dyn AssistantPort>,
    /// The interface.
    pub projection: Arc<dyn Projection>,
    /// Time.
    pub clock: Arc<dyn Clock>,
    /// Fresh UUIDv4s.
    pub ids: Ids,
}

/// How a session starts.
#[derive(Clone, Debug)]
pub struct Setup {
    /// The person's settings.
    pub preferences: VoiceSessionPreferencesV1,
    /// Whether voice can be used now.
    pub availability: VoiceAvailabilityV1,
    /// The language "automatic" means here.
    pub system_language: Language,
}

#[derive(Default)]
struct Snapshot {
    view: Option<VoiceSessionViewV1>,
    voices: Vec<String>,
    default_voice: Option<String>,
    preferences: Option<VoiceSessionPreferencesV1>,
}

/// One window's voice session. Cheap to clone.
#[derive(Clone)]
pub struct VoiceSession {
    inputs: mpsc::UnboundedSender<Input>,
    snapshot: Arc<Mutex<Snapshot>>,
}

/// The session's work, for the caller to spawn once on its runtime.
pub type Driver = BoxFuture<'static, ()>;

impl VoiceSession {
    /// A session and its driver.
    pub fn new(ports: Ports, setup: Setup) -> (Self, Driver) {
        let (inputs, receiver) = mpsc::unbounded_channel();
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let core = Core::new(ports, setup, inputs.clone(), snapshot.clone());
        let driver = Box::pin(run(core, receiver, inputs.clone()));
        (Self { inputs, snapshot }, driver)
    }

    /// Sends a command; false once the session has ended.
    pub fn send(&self, command: Command) -> bool {
        self.inputs.send(Input::Command(command)).is_ok()
    }

    /// Tees one assistant event to the session.
    pub fn assist(&self, event: &PageAssistChangedV1) {
        let _ = self.inputs.send(Input::Assist(Box::new(event.clone())));
    }

    /// The last view.
    pub fn view(&self) -> Option<VoiceSessionViewV1> {
        self.lock().view.clone()
    }

    /// The settings and what can be chosen.
    pub fn settings(&self) -> Option<VoiceSessionSettingsV1> {
        let snapshot = self.lock();
        Some(VoiceSessionSettingsV1 {
            version: VOICE_SESSION_VERSION,
            preferences: snapshot.preferences.clone()?,
            voices: snapshot.voices.clone(),
            default_voice: snapshot.default_voice.clone(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Snapshot> {
        self.snapshot.lock().unwrap_or_else(|p| p.into_inner())
    }
}

async fn run(
    mut core: Core,
    mut receiver: mpsc::UnboundedReceiver<Input>,
    inputs: mpsc::UnboundedSender<Input>,
) {
    core.start();
    loop {
        for task in std::mem::take(&mut core.tasks) {
            let inputs = inputs.clone();
            tokio::spawn(async move {
                let _ = inputs.send(task.await);
            });
        }
        for (generation, mut events) in std::mem::take(&mut core.pumps) {
            let inputs = inputs.clone();
            tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    if inputs.send(Input::Link { generation, event }).is_err() {
                        break;
                    }
                }
            });
        }
        if core.disposed {
            break;
        }
        let now = core.now();
        let input = match core.next_wake(now) {
            Some(at) => {
                let wait = std::time::Duration::from_millis(at.saturating_sub(now));
                tokio::select! {
                    input = receiver.recv() => input.map(Some),
                    () = tokio::time::sleep(wait) => Some(None),
                }
            }
            None => receiver.recv().await.map(Some),
        };
        match input {
            Some(Some(input)) => core.input(input),
            Some(None) => {}
            None => break,
        }
        core.poll();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Device {
    Released,
    Preparing,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mic {
    Off,
    Starting,
    On,
}

enum Link {
    Down,
    Opening,
    Ready(Arc<dyn VoiceSocket>),
}

struct Capture {
    id: String,
    hint: Option<&'static str>,
    frames: VecDeque<Frame>,
    started: bool,
    sent: usize,
    commit: bool,
    committed: bool,
    cancelled: bool,
}

struct Job {
    id: String,
    key: Option<String>,
    unsent: VecDeque<String>,
    parts: VecDeque<String>,
    waiting: Option<String>,
    pcm: Vec<i16>,
    record: Option<Vec<i16>>,
    started: bool,
}

impl Job {
    fn complete(&self) -> bool {
        self.unsent.is_empty() && self.parts.is_empty() && self.waiting.is_none()
    }
}

struct Warm {
    id: String,
    key: String,
    pcm: Vec<i16>,
}

#[derive(Default)]
struct Cache {
    lines: VecDeque<(String, Arc<[i16]>)>,
    samples: usize,
}

impl Cache {
    fn get(&self, key: &str) -> Option<Arc<[i16]>> {
        self.lines
            .iter()
            .find(|(line, _)| line == key)
            .map(|(_, pcm)| pcm.clone())
    }

    fn insert(&mut self, key: String, pcm: Vec<i16>) {
        if pcm.is_empty() || pcm.len() > MAX_LINE_SAMPLES || self.get(&key).is_some() {
            return;
        }
        self.samples += pcm.len();
        self.lines.push_back((key, pcm.into()));
        while self.lines.len() > CACHE_LINES || self.samples > CACHE_SAMPLES {
            match self.lines.pop_front() {
                Some((_, pcm)) => self.samples -= pcm.len(),
                None => break,
            }
        }
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.samples = 0;
    }
}

enum Queued {
    Steer(String),
    Stop,
}

#[derive(Default)]
struct Assist {
    conversation: Option<String>,
    reply: Option<u32>,
    asking: bool,
    steering: bool,
    queued: Option<Queued>,
    held: VecDeque<PageAssistChangedV1>,
}

/// The session's state and its effect executor. Synchronous: inputs go in
/// with the clock's time, port calls come out, and async work is handed to
/// the driver as tasks.
pub(crate) struct Core {
    ports: Ports,
    session: Session,
    snapshot: Arc<Mutex<Snapshot>>,
    inbox: Arc<AudioInbox>,
    preferences: VoiceSessionPreferencesV1,
    availability: VoiceAvailabilityV1,
    system_language: Language,
    pending: VecDeque<Event>,
    device: Device,
    device_generation: u64,
    foreground: bool,
    wanted: bool,
    mic: Mic,
    mic_generation: u64,
    gate: bool,
    link: Link,
    link_generation: u64,
    closing: Option<Fault>,
    ready: Option<VoiceReady>,
    refused: bool,
    captures: VecDeque<Capture>,
    buffered: usize,
    jobs: VecDeque<Job>,
    playing: VecDeque<(u64, String)>,
    warming: Vec<Warm>,
    streaming: Option<String>,
    marks: u64,
    cache: Cache,
    warmed: bool,
    assist: Assist,
    shown: Option<View>,
    seq: u32,
    levels_at: Option<u64>,
    tick_at: Option<u64>,
    idle_at: Option<u64>,
    pub(crate) tasks: Vec<BoxFuture<'static, Input>>,
    pub(crate) pumps: Vec<(u64, mpsc::Receiver<VoiceEvent>)>,
    pub(crate) disposed: bool,
}

fn language_of(language: VoiceLanguageV1) -> Option<Language> {
    Some(match language {
        VoiceLanguageV1::Automatic => return None,
        VoiceLanguageV1::En => Language::En,
        VoiceLanguageV1::Es => Language::Es,
        VoiceLanguageV1::De => Language::De,
        VoiceLanguageV1::Fr => Language::Fr,
        VoiceLanguageV1::Pl => Language::Pl,
        VoiceLanguageV1::Pt => Language::Pt,
        VoiceLanguageV1::Ru => Language::Ru,
        VoiceLanguageV1::It => Language::It,
        VoiceLanguageV1::Ja => Language::Ja,
        VoiceLanguageV1::Zh => Language::Zh,
    })
}

/// The voice fault for a socket or upgrade failure; none when it was ours.
pub fn link_fault(error: VoiceError) -> Option<Fault> {
    Some(match error {
        VoiceError::Cancelled => return None,
        VoiceError::Model(error) => model_fault(error),
        VoiceError::Timeout => Fault::Timeout,
        VoiceError::Interrupted => Fault::Interrupted,
        VoiceError::TooLong | VoiceError::AudioTooLong | VoiceError::InvalidAudio => {
            Fault::Unavailable
        }
    })
}

/// The voice fault for an account, admission or network failure.
pub fn model_fault(error: WorkModelError) -> Fault {
    match error {
        WorkModelError::Hosted { refusal, .. } => match refusal {
            HostedRefusal::OutOfCredits | HostedRefusal::FreeUsagePaused => Fault::OutOfCredit,
            HostedRefusal::PlanRequired => Fault::PlanRequired,
            HostedRefusal::DailyLimit => Fault::DailyLimit,
            HostedRefusal::ConfirmationLimited => Fault::RateLimited,
            _ => Fault::Unavailable,
        },
        WorkModelError::OverBudget => Fault::OutOfCredit,
        WorkModelError::RateLimited { .. } | WorkModelError::PlanLimit => Fault::RateLimited,
        WorkModelError::Network => Fault::Offline,
        WorkModelError::Overloaded => Fault::ProviderDown,
        _ => Fault::Unavailable,
    }
}

fn split(text: &str, max: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut count = 0;
    for word in text.split_whitespace() {
        let length = word.chars().count();
        if count > 0 && count + 1 + length > max {
            parts.push(std::mem::take(&mut part));
            count = 0;
        }
        if count > 0 {
            part.push(' ');
            count += 1;
        }
        if length > max {
            let cut: String = word.chars().take(max.saturating_sub(count)).collect();
            count += cut.chars().count();
            part.push_str(&cut);
            continue;
        }
        part.push_str(word);
        count += length;
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

impl Core {
    fn new(
        ports: Ports,
        setup: Setup,
        inputs: mpsc::UnboundedSender<Input>,
        snapshot: Arc<Mutex<Snapshot>>,
    ) -> Self {
        let session = Self::session(&ports, &setup.preferences, setup.system_language);
        let core = Self {
            session,
            snapshot,
            inbox: Arc::new(AudioInbox::new(inputs)),
            preferences: setup.preferences,
            availability: setup.availability,
            system_language: setup.system_language,
            pending: VecDeque::new(),
            device: Device::Released,
            device_generation: 0,
            foreground: false,
            wanted: false,
            mic: Mic::Off,
            mic_generation: 0,
            gate: false,
            link: Link::Down,
            link_generation: 0,
            closing: None,
            ready: None,
            refused: false,
            captures: VecDeque::new(),
            buffered: 0,
            jobs: VecDeque::new(),
            playing: VecDeque::new(),
            warming: Vec::new(),
            streaming: None,
            marks: 0,
            cache: Cache::default(),
            warmed: false,
            assist: Assist::default(),
            shown: None,
            seq: 0,
            levels_at: None,
            tick_at: None,
            idle_at: None,
            tasks: Vec::new(),
            pumps: Vec::new(),
            disposed: false,
            ports,
        };
        core.publish_settings();
        core
    }

    fn session(
        ports: &Ports,
        preferences: &VoiceSessionPreferencesV1,
        system: Language,
    ) -> Session {
        let ids = ports.ids.clone();
        Session::new(
            Config {
                spoken: preferences.spoken,
                language: language_of(preferences.language).unwrap_or(system),
                ..Config::default()
            },
            Box::new(move || ids()),
        )
    }

    fn language(&self) -> Language {
        language_of(self.preferences.language).unwrap_or(self.system_language)
    }

    pub(crate) fn now(&self) -> u64 {
        self.ports.clock.now_ms()
    }

    /// The first view, and the socket opened ahead of use.
    pub(crate) fn start(&mut self) {
        self.project(true);
        if self.availability == VoiceAvailabilityV1::Available {
            self.ensure_link();
        }
    }

    pub(crate) fn input(&mut self, input: Input) {
        if self.disposed {
            return;
        }
        let now = self.now();
        match input {
            Input::Command(command) => self.command(command),
            Input::Assist(event) => self.assist_event(*event),
            Input::Device(event) => self.device_event(event),
            Input::Prepared { generation, result } => self.prepared(generation, result),
            Input::Started { generation, result } => self.started(generation, result),
            Input::LinkOpened { generation, result } => self.link_opened(generation, result),
            Input::Link { generation, event } if generation == self.link_generation => {
                self.link_event(event);
            }
            Input::Link { .. } => {}
            Input::Asked(result) => self.asked(result),
            Input::Steered(result) => self.steered(result),
        }
        self.run(now);
    }

    fn command(&mut self, command: Command) {
        let available = self.availability == VoiceAvailabilityV1::Available;
        match command {
            Command::KeyDown if available => self.pending.push_back(Event::KeyDown),
            Command::KeyUp => self.pending.push_back(Event::KeyUp),
            Command::Escape => self.pending.push_back(Event::Escape),
            Command::Intent(VoiceSessionIntentV1::Start) if available => {
                self.pending.push_back(Event::Start);
            }
            Command::Intent(VoiceSessionIntentV1::Stop) => self.pending.push_back(Event::Stop),
            Command::Intent(VoiceSessionIntentV1::Cancel) => {
                self.pending.push_back(Event::Cancel);
            }
            Command::KeyDown | Command::Intent(VoiceSessionIntentV1::Start) => {
                self.project(true);
            }
            Command::Decide(decision) => self.decide(decision),
            Command::Preferences(preferences) => self.set_preferences(preferences),
            Command::Availability(availability) => self.set_availability(availability),
            Command::Foreground(active) => {
                self.foreground = active;
                if active && available {
                    self.prepare_device();
                } else if !active && !self.busy() && self.jobs.is_empty() {
                    self.release_device();
                }
            }
            Command::Dispose => {
                self.reset();
                self.disposed = true;
            }
        }
    }

    fn decide(&mut self, decision: PageAssistDecisionV1) {
        let (Some(conversation), Some(reply)) =
            (self.assist.conversation.clone(), self.assist.reply)
        else {
            return;
        };
        let Some(confirm) = self.session.view().confirm else {
            return;
        };
        if self
            .ports
            .assistant
            .decide(&conversation, reply, confirm.step.block, decision)
        {
            self.pending.push_back(Event::Decided { reply });
        }
    }

    fn set_preferences(&mut self, preferences: VoiceSessionPreferencesV1) {
        let voice = preferences.voice != self.preferences.voice;
        let language = preferences.language != self.preferences.language;
        self.preferences = preferences;
        if voice || language {
            self.cache.clear();
            self.warmed = false;
        }
        self.publish_settings();
        self.pending.push_back(Event::Settings {
            spoken: self.preferences.spoken,
            language: self.language(),
        });
    }

    fn set_availability(&mut self, availability: VoiceAvailabilityV1) {
        if availability == self.availability {
            return;
        }
        let was = self.availability;
        self.availability = availability;
        if was == VoiceAvailabilityV1::Available {
            self.reset();
        } else if availability == VoiceAvailabilityV1::Available {
            self.refused = false;
            self.ensure_link();
            if self.foreground {
                self.prepare_device();
            }
        }
        self.project(true);
    }

    /// Ends the session's work and releases the microphone and the socket;
    /// a fresh reducer takes over.
    fn reset(&mut self) {
        let now = self.now();
        let working = self.assist.asking
            || self.assist.steering
            || matches!(self.session.view().state, State::Working(_));
        let effects = self.session.on(Event::Dispose, now);
        for effect in effects {
            self.execute(effect);
        }
        self.pending.clear();
        self.close_link();
        self.drop_speech();
        self.captures.clear();
        self.buffered = 0;
        self.gate = false;
        self.release_device();
        if let (true, Some(conversation), Some(reply)) =
            (working, &self.assist.conversation, self.assist.reply)
        {
            self.ports.assistant.stop(conversation, reply);
        }
        self.assist = Assist::default();
        self.session = Self::session(&self.ports, &self.preferences, self.system_language);
        self.shown = None;
        self.levels_at = None;
        self.tick_at = None;
        self.idle_at = None;
    }

    fn run(&mut self, now: u64) {
        while let Some(event) = self.pending.pop_front() {
            for effect in self.session.on(event, now) {
                self.execute(effect);
            }
        }
        self.schedule(now);
    }

    fn execute(&mut self, effect: Effect) {
        match effect {
            Effect::OpenMic => {
                self.wanted = true;
                match self.device {
                    Device::Ready => self.start_mic(),
                    Device::Preparing => {}
                    Device::Released => self.prepare_device(),
                }
            }
            Effect::CloseMic => {
                self.wanted = false;
                self.set_gate(false);
                if self.mic == Mic::On {
                    self.ports.audio.standby();
                }
                if self.mic != Mic::Off {
                    self.mic = Mic::Off;
                    self.mic_generation += 1;
                }
            }
            Effect::Gate { on } => self.set_gate(on),
            Effect::OpenLink => match self.link {
                Link::Ready(_) => self.pending.push_back(Event::LinkReady),
                Link::Opening => {}
                Link::Down => self.ensure_link(),
            },
            Effect::StartUtterance { id, .. } => {
                let hint = language_of(self.preferences.language).map(Language::code);
                self.captures.push_back(Capture {
                    id,
                    hint,
                    frames: VecDeque::new(),
                    started: false,
                    sent: 0,
                    commit: false,
                    committed: false,
                    cancelled: false,
                });
                self.forward();
            }
            Effect::CommitUtterance { id } => {
                self.drain_frames();
                if let Some(capture) = self.captures.iter_mut().find(|c| c.id == id) {
                    capture.commit = true;
                }
                self.forward();
            }
            Effect::CancelUtterance { id } => self.cancel_capture(&id),
            Effect::Ask { text } => {
                self.assist.asking = true;
                self.assist.queued = None;
                self.assist.held.clear();
                self.tasks.push(Box::pin({
                    let ask = self.ports.assistant.ask(text, self.preferences.spoken);
                    async move { Input::Asked(ask.await) }
                }));
            }
            Effect::Steer { reply, text } => match (reply, self.assist.conversation.clone()) {
                (Some(reply), Some(conversation)) if !self.assist.asking => {
                    self.steer(conversation, reply, text);
                }
                _ if self.assist.asking => self.queue(Queued::Steer(text)),
                _ => self.pending.push_back(Event::Refused {
                    fault: Fault::Unavailable,
                }),
            },
            Effect::StopAssistant { reply } => {
                match (reply.or(self.assist.reply), &self.assist.conversation) {
                    _ if self.assist.asking => self.queue(Queued::Stop),
                    (Some(reply), Some(conversation)) => {
                        self.ports.assistant.stop(conversation, reply);
                    }
                    _ => {}
                }
            }
            Effect::Decide { reply, decision } => {
                if let (Some(conversation), Some(confirm)) =
                    (&self.assist.conversation, self.session.view().confirm)
                {
                    self.ports
                        .assistant
                        .decide(conversation, reply, confirm.step.block, decision);
                }
            }
            Effect::Speak { id, speech } => self.speak(id, speech),
            Effect::FlushSpeech => self.flush(),
            Effect::CancelSpeech => {
                if let Link::Ready(socket) = &self.link {
                    socket.speak_cancel(None);
                }
                if !self.warming.is_empty() {
                    self.warming.clear();
                    self.warmed = false;
                }
            }
            Effect::Earcon(earcon) => {
                if self.preferences.earcons && self.device == Device::Ready {
                    self.ports.audio.play(&earcon::tone(earcon));
                }
            }
            Effect::View(view) => {
                self.shown = Some(view);
                self.project(false);
            }
        }
    }

    fn project(&mut self, always: bool) {
        let view = self.shown.clone().unwrap_or_else(|| self.session.view());
        let mut projected = view::project(&view, self.availability, 0);
        let last = self
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .view
            .clone();
        if let Some(last) = &last {
            projected.seq = last.seq;
            if !always && *last == projected {
                return;
            }
        }
        self.seq = self.seq.wrapping_add(1);
        projected.seq = self.seq;
        self.ports.projection.view(&projected);
        self.snapshot.lock().unwrap_or_else(|p| p.into_inner()).view = Some(projected);
    }

    fn publish_settings(&self) {
        let mut snapshot = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        snapshot.preferences = Some(self.preferences.clone());
        if let Some(ready) = &self.ready {
            snapshot.voices = ready.voices.clone();
            snapshot.default_voice = Some(ready.voice.clone());
        }
    }

    // Device.

    fn prepare_device(&mut self) {
        if self.device != Device::Released {
            return;
        }
        self.device = Device::Preparing;
        self.device_generation += 1;
        let generation = self.device_generation;
        let prepare = self.ports.audio.prepare(self.inbox.clone());
        self.tasks.push(Box::pin(async move {
            Input::Prepared {
                generation,
                result: prepare.await,
            }
        }));
    }

    fn release_device(&mut self) {
        self.wanted = false;
        self.mic = Mic::Off;
        self.mic_generation += 1;
        if self.device != Device::Released {
            self.ports.audio.release();
            self.device = Device::Released;
            self.device_generation += 1;
        }
        self.inbox.clear();
    }

    fn start_mic(&mut self) {
        if self.mic != Mic::Off {
            return;
        }
        self.mic = Mic::Starting;
        self.mic_generation += 1;
        let generation = self.mic_generation;
        let start = self.ports.audio.start();
        self.tasks.push(Box::pin(async move {
            Input::Started {
                generation,
                result: start.await,
            }
        }));
    }

    fn set_gate(&mut self, on: bool) {
        if !on {
            self.drain_frames();
        }
        self.gate = on;
        if self.device == Device::Ready {
            self.ports.audio.gate(on);
        }
    }

    fn prepared(&mut self, generation: u64, result: Result<(), Fault>) {
        if generation != self.device_generation || self.device != Device::Preparing {
            if result.is_ok() && self.device == Device::Released {
                self.ports.audio.release();
            }
            return;
        }
        match result {
            Ok(()) => {
                self.device = Device::Ready;
                self.ports.audio.gate(self.gate);
                if self.wanted {
                    self.start_mic();
                }
                self.pump();
            }
            Err(fault) => {
                self.device = Device::Released;
                self.device_failed(fault);
            }
        }
    }

    fn started(&mut self, generation: u64, result: Result<(), Fault>) {
        if generation != self.mic_generation || self.mic != Mic::Starting {
            if result.is_ok() && self.mic == Mic::Off && self.device == Device::Ready {
                self.ports.audio.standby();
            }
            return;
        }
        match result {
            Ok(()) => {
                self.mic = Mic::On;
                self.ports.audio.gate(self.gate);
                self.pending.push_back(Event::MicOpened);
            }
            Err(fault) => {
                self.mic = Mic::Off;
                self.wanted = false;
                self.pending.push_back(Event::MicFailed { fault });
            }
        }
    }

    fn device_failed(&mut self, fault: Fault) {
        self.mic = Mic::Off;
        self.mic_generation += 1;
        if std::mem::replace(&mut self.wanted, false) {
            self.pending.push_back(Event::MicFailed { fault });
        }
        self.gate = false;
        for job in std::mem::take(&mut self.jobs) {
            self.pending.push_back(Event::SpeechFailed {
                id: job.id,
                fault: Some(fault),
            });
        }
        for (_, id) in std::mem::take(&mut self.playing) {
            self.pending.push_back(Event::SpeechFailed {
                id,
                fault: Some(fault),
            });
        }
        self.inbox.clear();
    }

    fn device_event(&mut self, event: port::DeviceEvent) {
        match event {
            port::DeviceEvent::Played(mark) => {
                while self.playing.front().is_some_and(|(at, _)| *at <= mark) {
                    if let Some((_, id)) = self.playing.pop_front() {
                        self.pending.push_back(Event::SpeechEnded { id });
                    }
                }
            }
            port::DeviceEvent::RouteChanged => {}
            port::DeviceEvent::Lost(fault) => {
                if self.device != Device::Released {
                    self.ports.audio.release();
                    self.device = Device::Released;
                    self.device_generation += 1;
                    self.device_failed(fault);
                }
            }
        }
    }

    // Capture.

    fn drain_frames(&mut self) {
        while let Some(frame) = self.inbox.take_frame() {
            let Some(capture) = self
                .captures
                .iter_mut()
                .rev()
                .find(|c| !c.commit && !c.cancelled)
            else {
                continue;
            };
            if !self.gate || self.buffered >= MAX_BUFFERED {
                continue;
            }
            capture.frames.push_back(frame);
            self.buffered += 1;
        }
    }

    /// Starts the head utterance on the socket and sends its frames in
    /// order, a burst at first and then a few per poll, then its commit.
    fn forward(&mut self) {
        loop {
            let Link::Ready(socket) = &self.link else {
                return;
            };
            let socket = socket.clone();
            let Some(capture) = self.captures.front_mut() else {
                return;
            };
            if capture.cancelled {
                return;
            }
            if !capture.started {
                if !socket.listen_start(&capture.id, capture.hint) {
                    return;
                }
                capture.started = true;
                if !self.warmed {
                    self.warm();
                    continue;
                }
            }
            let burst = if capture.sent == 0 {
                FIRST_BURST
            } else {
                BURST
            };
            let mut sent = 0;
            while sent < burst {
                let Some(frame) = capture.frames.pop_front() else {
                    break;
                };
                self.buffered -= 1;
                let bytes: Vec<u8> = frame.iter().flat_map(|s| s.to_le_bytes()).collect();
                if !socket.audio(&bytes) {
                    return;
                }
                sent += 1;
                capture.sent += 1;
            }
            if capture.commit && !capture.committed && capture.frames.is_empty() {
                capture.committed = socket.listen_commit(&capture.id);
            }
            return;
        }
    }

    fn cancel_capture(&mut self, id: &str) {
        let Some(index) = self.captures.iter().position(|c| c.id == id) else {
            return;
        };
        let capture = &mut self.captures[index];
        self.buffered -= capture.frames.len();
        capture.frames.clear();
        if capture.started {
            if !capture.cancelled {
                capture.cancelled = true;
                if let Link::Ready(socket) = &self.link {
                    socket.listen_cancel(id);
                }
            }
        } else {
            self.captures.remove(index);
        }
    }

    fn finish_capture(&mut self, id: &str) {
        if self.captures.front().is_some_and(|c| c.id == id) {
            if let Some(capture) = self.captures.pop_front() {
                self.buffered -= capture.frames.len();
            }
            self.forward();
        }
    }

    // Socket.

    fn ensure_link(&mut self) {
        if !matches!(self.link, Link::Down) {
            return;
        }
        if self.availability != VoiceAvailabilityV1::Available {
            self.pending.push_back(Event::LinkLost {
                fault: Some(Fault::Unavailable),
            });
            return;
        }
        self.link = Link::Opening;
        self.link_generation += 1;
        let generation = self.link_generation;
        let open = self.ports.link.open();
        self.tasks.push(Box::pin(async move {
            Input::LinkOpened {
                generation,
                result: open.await.map_err(model_fault),
            }
        }));
    }

    fn link_opened(&mut self, generation: u64, result: Result<Opened, Fault>) {
        if generation != self.link_generation || !matches!(self.link, Link::Opening) {
            if let Ok(opened) = result {
                opened.socket.close();
            }
            return;
        }
        match result {
            Ok(opened) => {
                self.link = Link::Ready(opened.socket);
                self.ready = Some(opened.ready);
                self.closing = None;
                self.pumps.push((generation, opened.events));
                self.publish_settings();
                self.pending.push_back(Event::LinkReady);
                self.forward();
                self.send_speech();
            }
            Err(fault) => {
                self.link = Link::Down;
                if fault == Fault::PlanRequired {
                    self.refused = true;
                }
                self.link_lost(Some(fault));
            }
        }
    }

    fn close_link(&mut self) {
        if let Link::Ready(socket) = std::mem::replace(&mut self.link, Link::Down) {
            socket.close();
        }
        self.link_generation += 1;
        self.streaming = None;
        self.warming.clear();
        self.warmed = false;
    }

    fn link_lost(&mut self, fault: Option<Fault>) {
        if !self.jobs.is_empty() || !self.playing.is_empty() {
            self.flush();
        }
        self.captures.clear();
        self.buffered = 0;
        self.warming.clear();
        self.warmed = false;
        self.streaming = None;
        self.pending.push_back(Event::LinkLost { fault });
    }

    fn link_event(&mut self, event: VoiceEvent) {
        match event {
            VoiceEvent::Partial { id, text } => self.pending.push_back(Event::Partial { id, text }),
            VoiceEvent::Final { id, text } => {
                self.finish_capture(&id);
                self.pending.push_back(Event::Final { id, text });
            }
            VoiceEvent::ListenFailed { id, error } => {
                self.finish_capture(&id);
                if let Some(fault) = link_fault(error) {
                    self.pending.push_back(Event::ListenFailed { id, fault });
                }
            }
            VoiceEvent::SpeakStart { id } => self.streaming = Some(id),
            VoiceEvent::Audio(bytes) => self.audio(&bytes),
            VoiceEvent::SpeakEnd { id } => self.speech_ended(&id),
            VoiceEvent::SpeakFailed { id, error } => self.speech_failed(&id, error),
            VoiceEvent::Closing { reason } => {
                self.closing = match reason {
                    VoiceClosing::SessionEnded => Some(Fault::Unavailable),
                    VoiceClosing::ShuttingDown | VoiceClosing::Idle | VoiceClosing::PlanChanged => {
                        None
                    }
                };
            }
            VoiceEvent::Closed => {
                let expected = matches!(self.link, Link::Down);
                self.link = Link::Down;
                self.link_generation += 1;
                if !expected {
                    let fault = self.closing.take().or(Some(Fault::Interrupted));
                    let busy = !self.captures.is_empty()
                        || !self.jobs.is_empty()
                        || self.session.view().state != State::Idle;
                    self.link_lost(fault.filter(|_| busy));
                }
            }
        }
    }

    // Speech.

    fn speak(&mut self, id: String, speech: Speech) {
        let mut job = Job {
            id,
            key: None,
            unsent: VecDeque::new(),
            parts: VecDeque::new(),
            waiting: None,
            pcm: Vec::new(),
            record: None,
            started: false,
        };
        match speech {
            Speech::Text(text) => {
                let max = self.max_chars();
                job.unsent.extend(split(&text, max));
            }
            Speech::Line(line) => {
                let key = lines::render(line, self.language()).to_owned();
                if let Some(pcm) = self.cache.get(&key) {
                    job.pcm.extend_from_slice(&pcm);
                } else if self.warming.iter().any(|w| w.key == key) {
                    job.waiting = Some(key.clone());
                } else {
                    job.unsent.push_back(key.clone());
                    job.record = Some(Vec::new());
                }
                job.key = Some(key);
            }
        }
        self.jobs.push_back(job);
        self.send_speech();
        self.pump();
    }

    fn max_chars(&self) -> usize {
        self.ready
            .as_ref()
            .map_or(DEFAULT_MAX_CHARS, |ready| ready.speak_max_chars as usize)
            .max(1)
    }

    fn voice(&self) -> Option<String> {
        let ready = self.ready.as_ref()?;
        self.preferences
            .voice
            .clone()
            .filter(|voice| ready.voices.contains(voice))
    }

    fn outstanding(&self) -> usize {
        self.warming.len() + self.jobs.iter().map(|job| job.parts.len()).sum::<usize>()
    }

    fn send_speech(&mut self) {
        let Link::Ready(socket) = &self.link else {
            return;
        };
        let socket = socket.clone();
        let voice = self.voice();
        let mut failed = Vec::new();
        let mut outstanding = self.outstanding();
        for job in &mut self.jobs {
            while let Some(text) = job.unsent.pop_front() {
                if outstanding >= MAX_OUTSTANDING {
                    failed.push(job.id.clone());
                    break;
                }
                let id = if job.parts.is_empty() && job.pcm.is_empty() && !job.started {
                    job.id.clone()
                } else {
                    (self.ports.ids)()
                };
                if !socket.speak(&id, &text, voice.as_deref()) {
                    return;
                }
                job.parts.push_back(id);
                outstanding += 1;
            }
        }
        for id in failed {
            self.jobs.retain(|job| job.id != id);
            self.pending
                .push_back(Event::SpeechFailed { id, fault: None });
        }
    }

    fn warm(&mut self) {
        self.warmed = true;
        if !self.preferences.spoken {
            return;
        }
        let Link::Ready(socket) = &self.link else {
            return;
        };
        let socket = socket.clone();
        let voice = self.voice();
        let language = self.language();
        for line in lines::WARM {
            let key = lines::render(line, language).to_owned();
            if self.cache.get(&key).is_some() || self.warming.iter().any(|w| w.key == key) {
                continue;
            }
            let id = (self.ports.ids)();
            if !socket.speak(&id, &key, voice.as_deref()) {
                return;
            }
            self.warming.push(Warm {
                id,
                key,
                pcm: Vec::new(),
            });
        }
    }

    fn audio(&mut self, bytes: &[u8]) {
        let Some(streaming) = &self.streaming else {
            return;
        };
        let samples = bytes
            .chunks_exact(2)
            .map(|pair| i16::from_le_bytes([pair[0], pair[1]]));
        if let Some(warm) = self.warming.iter_mut().find(|w| w.id == *streaming) {
            if warm.pcm.len() + bytes.len() / 2 <= MAX_LINE_SAMPLES {
                warm.pcm.extend(samples);
            }
            return;
        }
        let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.parts.front() == Some(streaming))
        else {
            return;
        };
        if job.pcm.len() + bytes.len() / 2 > MAX_SPEECH_SAMPLES {
            return;
        }
        let start = job.pcm.len();
        job.pcm.extend(samples);
        if let Some(record) = &mut job.record {
            if record.len() + bytes.len() / 2 <= MAX_LINE_SAMPLES {
                record.extend_from_slice(&job.pcm[start..]);
            }
        }
        self.pump();
    }

    fn speech_ended(&mut self, id: &str) {
        if self.streaming.as_deref() == Some(id) {
            self.streaming = None;
        }
        if let Some(index) = self.warming.iter().position(|w| w.id == id) {
            let warm = self.warming.remove(index);
            for job in &mut self.jobs {
                if job.waiting.as_deref() == Some(warm.key.as_str()) {
                    job.pcm.extend_from_slice(&warm.pcm);
                    job.waiting = None;
                }
            }
            self.cache.insert(warm.key, warm.pcm);
            self.pump();
            return;
        }
        if let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.parts.front().map(String::as_str) == Some(id))
        {
            job.parts.pop_front();
        }
        self.pump();
    }

    fn speech_failed(&mut self, id: &str, error: VoiceError) {
        if self.streaming.as_deref() == Some(id) {
            self.streaming = None;
        }
        if let Some(index) = self.warming.iter().position(|w| w.id == id) {
            let warm = self.warming.remove(index);
            for job in &mut self.jobs {
                if job.waiting.as_deref() == Some(warm.key.as_str()) {
                    job.waiting = None;
                    job.unsent.push_back(warm.key.clone());
                    job.record = Some(Vec::new());
                }
            }
            self.send_speech();
            self.pump();
            return;
        }
        let Some(index) = self
            .jobs
            .iter()
            .position(|job| job.parts.iter().any(|part| part == id))
        else {
            return;
        };
        if let Some(job) = self.jobs.remove(index) {
            self.pending.push_back(Event::SpeechFailed {
                id: job.id,
                fault: link_fault(error),
            });
        }
        self.pump();
    }

    /// Plays the head speech's audio as it arrives, then the next speech's,
    /// in the order the reducer emitted them.
    fn pump(&mut self) {
        loop {
            let Some(job) = self.jobs.front_mut() else {
                return;
            };
            if !job.pcm.is_empty() {
                if self.device != Device::Ready {
                    self.prepare_device();
                    return;
                }
                self.ports.audio.play(&job.pcm);
                job.pcm.clear();
                if !job.started {
                    job.started = true;
                    self.pending
                        .push_back(Event::SpeechStarted { id: job.id.clone() });
                }
            }
            if !job.complete() {
                return;
            }
            let Some(job) = self.jobs.pop_front() else {
                return;
            };
            if let (Some(key), Some(record)) = (job.key, job.record) {
                self.cache.insert(key, record);
            }
            if job.started {
                self.marks += 1;
                self.ports.audio.mark(self.marks);
                self.playing.push_back((self.marks, job.id));
            } else {
                self.pending.push_back(Event::SpeechEnded { id: job.id });
            }
        }
    }

    fn drop_speech(&mut self) {
        self.jobs.clear();
        self.playing.clear();
        self.streaming = None;
    }

    fn flush(&mut self) {
        if self.device == Device::Ready {
            self.ports.audio.flush();
        }
        self.drop_speech();
    }

    // Assistant.

    /// Holds a steer or stop until the ask it follows is admitted; a steer it
    /// replaces is answered as refused so every steer gets one answer.
    fn queue(&mut self, next: Queued) {
        if let Some(Queued::Steer(_)) = self.assist.queued.replace(next) {
            self.pending
                .push_back(Event::Refused { fault: Fault::Busy });
        }
    }

    fn steer(&mut self, conversation: String, reply: u32, text: String) {
        self.assist.steering = true;
        let steer = self
            .ports
            .assistant
            .steer(conversation, reply, text, self.preferences.spoken);
        self.tasks
            .push(Box::pin(async move { Input::Steered(steer.await) }));
    }

    fn asked(&mut self, result: Result<Asked, Fault>) {
        if !self.assist.asking {
            if let Ok(stale) = result {
                self.ports.assistant.stop(&stale.conversation, stale.reply);
            }
            return;
        }
        self.assist.asking = false;
        let queued = self.assist.queued.take();
        match result {
            Ok(asked) => {
                self.assist.conversation = Some(asked.conversation.clone());
                self.assist.reply = Some(asked.reply);
                match queued {
                    Some(Queued::Steer(text)) => {
                        self.admit(asked.reply);
                        self.steer(asked.conversation, asked.reply, text);
                    }
                    Some(Queued::Stop) => {
                        self.admit(asked.reply);
                        self.ports.assistant.stop(&asked.conversation, asked.reply);
                    }
                    None => self.admit(asked.reply),
                }
            }
            Err(fault) => {
                self.assist.held.clear();
                self.pending.push_back(Event::Refused { fault });
                if let Some(Queued::Steer(_)) = queued {
                    self.pending.push_back(Event::Refused { fault });
                }
            }
        }
    }

    fn steered(&mut self, result: Result<Asked, Fault>) {
        if !self.assist.steering {
            if let Ok(stale) = result {
                self.ports.assistant.stop(&stale.conversation, stale.reply);
            }
            return;
        }
        self.assist.steering = false;
        match result {
            Ok(asked) => {
                self.assist.conversation = Some(asked.conversation);
                self.assist.reply = Some(asked.reply);
                self.pending
                    .push_back(Event::Admitted { reply: asked.reply });
            }
            Err(fault) => self.pending.push_back(Event::Refused { fault }),
        }
    }

    /// The ask's conversation is known only once it is admitted, so its
    /// events wait here until then; the reducer orders them by reply.
    fn admit(&mut self, reply: u32) {
        self.pending.push_back(Event::Admitted { reply });
        let conversation = self.assist.conversation.clone();
        for event in std::mem::take(&mut self.assist.held) {
            if Some(&event.conversation) == conversation.as_ref() {
                self.pending.push_back(Event::Reply {
                    reply: event.reply,
                    event: event.kind,
                });
            }
        }
    }

    fn assist_event(&mut self, event: PageAssistChangedV1) {
        let ours = self.assist.conversation.as_ref() == Some(&event.conversation);
        if matches!(event.kind, PageAssistEventV1::Invalidated) {
            if ours {
                self.pending.push_back(Event::Reply {
                    reply: event.reply,
                    event: event.kind,
                });
            }
            return;
        }
        if self.assist.asking {
            if self.assist.held.len() < MAX_HELD {
                self.assist.held.push_back(event);
            }
            return;
        }
        if ours {
            self.pending.push_back(Event::Reply {
                reply: event.reply,
                event: event.kind,
            });
        }
    }

    // Time.

    fn live(&self) -> bool {
        self.device == Device::Ready
            && ((self.mic == Mic::On && self.gate)
                || !self.playing.is_empty()
                || !self.jobs.is_empty())
    }

    fn busy(&self) -> bool {
        let view = self.session.view();
        !matches!(view.state, State::Idle | State::Settled(_)) || view.mic
    }

    fn schedule(&mut self, now: u64) {
        self.tick_at = self.session.deadline();
        let quiet = !self.busy()
            && self.captures.is_empty()
            && self.jobs.is_empty()
            && self.playing.is_empty();
        if quiet && matches!(self.link, Link::Ready(_)) {
            self.idle_at.get_or_insert(now + LINK_IDLE_MS);
        } else {
            self.idle_at = None;
        }
        if self.live() {
            self.levels_at.get_or_insert(now);
        } else if self.levels_at.take().is_some() {
            self.ports.projection.levels(VoiceSessionLevelsV1 {
                input: 0,
                output: 0,
            });
        }
    }

    /// When the driver should poll next; none while nothing is happening.
    pub(crate) fn next_wake(&self, now: u64) -> Option<u64> {
        let polling = (self.live() || self.buffered > 0 || self.captures.iter().any(|c| c.commit))
            .then_some(now + POLL_MS);
        [polling, self.tick_at, self.idle_at, self.levels_at]
            .into_iter()
            .flatten()
            .min()
    }

    /// Moves frames to the socket, emits levels and ticks; called after
    /// every input and wake.
    pub(crate) fn poll(&mut self) {
        if self.disposed {
            return;
        }
        let now = self.now();
        self.drain_frames();
        self.forward();
        if let Some(at) = self.levels_at.filter(|at| *at <= now) {
            let (input, output) = self.inbox.levels();
            let percent = |level: f32| (level.clamp(0.0, 1.0) * 100.0).round() as u8;
            self.ports.projection.levels(VoiceSessionLevelsV1 {
                input: if self.gate { percent(input) } else { 0 },
                output: if self.playing.is_empty() && self.jobs.is_empty() {
                    0
                } else {
                    percent(output)
                },
            });
            self.levels_at = Some(at.max(now.saturating_sub(LEVEL_MS)) + LEVEL_MS);
        }
        if self.tick_at.is_some_and(|at| at <= now) {
            self.tick_at = None;
            self.pending.push_back(Event::Tick);
        }
        if self.idle_at.is_some_and(|at| at <= now) {
            self.idle_at = None;
            self.close_link();
            self.pending.push_back(Event::LinkLost { fault: None });
        }
        self.run(now);
    }
}

#[cfg(test)]
pub(crate) fn core_for_tests(ports: Ports, setup: Setup) -> (Core, mpsc::UnboundedReceiver<Input>) {
    let (inputs, receiver) = mpsc::unbounded_channel();
    let core = Core::new(
        ports,
        setup,
        inputs,
        Arc::new(Mutex::new(Snapshot::default())),
    );
    (core, receiver)
}

#[cfg(test)]
impl Core {
    pub(crate) fn inbox(&self) -> Arc<AudioInbox> {
        self.inbox.clone()
    }
}
