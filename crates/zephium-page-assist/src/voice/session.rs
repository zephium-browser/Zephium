//! One voice session: key presses, the microphone, the transcript and the
//! assistant's reply go in as events; what to do comes back as plain effects.
//! Time is an argument and ids come from the caller, so every run replays.
use std::collections::BTreeMap;

use super::closing::{self, Closing, Outcome, ReplyFacts, StepKind, StepRef};
use super::cutter::Cutter;
use super::phrases::{self, Ack, Language};
use crate::ipc::{
    PageAssistBlockV1 as Block, PageAssistDecisionV1 as Decision, PageAssistEndV1 as End,
    PageAssistEventV1 as AssistEvent, PageAssistFaultV1 as AssistFault,
    PageAssistMessageV1 as Message, PageAssistPhaseV1 as Phase,
    PageAssistReplyStateV1 as ReplyState, PageAssistStepStateV1 as StepState,
    WorkModelFaultV1 as ModelFault, PAGE_ASSIST_MAX_QUESTION_BYTES,
};
use crate::text;

const MAX_SHOWN_CHARS: usize = 500;
const MAX_LABEL_CHARS: usize = 120;
/// Reply events held while the reply they belong to awaits admission.
const MAX_EARLY_EVENTS: usize = 256;

/// Hands out a fresh utterance or speech id (a UUIDv4 in production).
pub type IdSource = Box<dyn FnMut() -> String + Send>;

/// Session settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    /// Presses at least this long are hold-to-talk; shorter ones latch.
    pub hold_ms: u64,
    /// How long the microphone stays open, gated off, after an utterance.
    pub warm_ms: u64,
    /// How long a committed utterance waits for its final text.
    pub final_timeout_ms: u64,
    /// How long a settled outcome, or a passing fault, stays shown once
    /// speech has finished.
    pub settled_ms: u64,
    /// How long the microphone stays warm for the answer when a reply ends
    /// with a question.
    pub answer_warm_ms: u64,
    /// Spoken replies on.
    pub spoken: bool,
    /// The language the person speaks.
    pub language: Language,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hold_ms: 300,
            warm_ms: 6_000,
            final_timeout_ms: 8_000,
            settled_ms: 4_000,
            answer_warm_ms: 20_000,
            spoken: true,
            language: Language::En,
        }
    }
}

/// How the person is talking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Press {
    /// The key is down.
    Held,
    /// A short press latched listening on until the next press.
    Latched,
}

/// What the assistant is doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Work {
    /// Thinking or preparing.
    Thinking,
    /// Reading a page.
    Reading,
    /// Running steps, opening tabs or styling.
    Acting,
    /// Waiting on the person.
    Confirming,
}

/// The session's state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    /// Nothing is happening.
    Idle,
    /// Listening was asked for; the microphone is opening.
    Arming(Press),
    /// Listening.
    Listening(Press),
    /// The person finished; waiting for the final text.
    Finalizing,
    /// The assistant is working on the reply.
    Working(Work),
    /// The last reply ended.
    Settled(Outcome),
}

/// Why voice could not do something. Closed; never provider text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// Microphone access is denied.
    MicrophoneDenied,
    /// No microphone is connected.
    NoMicrophone,
    /// The voice service cannot be reached.
    Offline,
    /// The plan does not include voice.
    PlanRequired,
    /// Out of credit.
    OutOfCredit,
    /// The daily limit is reached.
    DailyLimit,
    /// Too many requests; try again shortly.
    RateLimited,
    /// The speech provider is down.
    ProviderDown,
    /// No answer in time.
    Timeout,
    /// The connection dropped.
    Interrupted,
    /// The assistant is busy with another reply.
    Busy,
    /// Voice is unavailable.
    Unavailable,
}

impl Fault {
    /// Whether it stays shown until the person acts, instead of fading
    /// with the settled outcome.
    pub fn persists(self) -> bool {
        matches!(
            self,
            Self::MicrophoneDenied
                | Self::NoMicrophone
                | Self::PlanRequired
                | Self::OutOfCredit
                | Self::DailyLimit
        )
    }

    /// The voice fault for an assistant fault.
    pub fn from_assist(fault: &AssistFault) -> Self {
        match fault {
            AssistFault::Busy => Self::Busy,
            AssistFault::TimedOut => Self::Timeout,
            AssistFault::Model { fault, .. } => match fault {
                ModelFault::HostedUsage
                | ModelFault::ProviderCredit
                | ModelFault::FreeUsagePaused => Self::OutOfCredit,
                ModelFault::PlanRequired => Self::PlanRequired,
                ModelFault::DailyLimit => Self::DailyLimit,
                ModelFault::RateLimited | ModelFault::PlanLimit | ModelFault::HostedBusy => {
                    Self::RateLimited
                }
                ModelFault::ProviderDown => Self::ProviderDown,
                ModelFault::Unreachable => Self::Offline,
                _ => Self::Unavailable,
            },
            _ => Self::Unavailable,
        }
    }
}

/// A short on-device sound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Earcon {
    /// Listening began.
    ListeningStarted,
    /// Listening ended and the words are on their way.
    ListeningEnded,
    /// Something failed.
    Error,
}

/// A fixed line the service renders in the person's language.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Line {
    /// An acknowledgment.
    Ack(Ack),
    /// How a reply ended, or that it waits on the person.
    Closing(Closing),
}

/// What one speech says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Speech {
    /// A chunk of the assistant's own reply text.
    Text(String),
    /// A fixed line.
    Line(Line),
}

/// A step waiting on the person's confirm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Confirm {
    /// The step; its category says what it commits.
    pub step: StepRef,
    /// Its label, for display only.
    pub label: String,
}

/// What the interface shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct View {
    /// The session's state.
    pub state: State,
    /// The microphone is open.
    pub mic: bool,
    /// The microphone is open with listening gated off.
    pub warm: bool,
    /// The interim transcript.
    pub interim: String,
    /// The last final transcript.
    pub heard: Option<String>,
    /// The reply's phase.
    pub phase: Option<Phase>,
    /// The step the reply is on, for display only.
    pub step: Option<String>,
    /// The pending confirmation.
    pub confirm: Option<Confirm>,
    /// Speech is queued or playing.
    pub speaking: bool,
    /// How the last reply ended.
    pub outcome: Option<Outcome>,
    /// The last fault.
    pub fault: Option<Fault>,
}

/// What happens to the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The talk key went down.
    KeyDown,
    /// The talk key went up.
    KeyUp,
    /// Escape.
    Escape,
    /// The on-screen button started listening.
    Start,
    /// The on-screen button stopped listening.
    Stop,
    /// The on-screen button cancelled.
    Cancel,
    /// The microphone is open.
    MicOpened,
    /// The microphone could not open, or closed under us.
    MicFailed {
        /// Why.
        fault: Fault,
    },
    /// The voice socket is up.
    LinkReady,
    /// The voice socket closed; `fault` is none for an orderly close.
    LinkLost {
        /// Why.
        fault: Option<Fault>,
    },
    /// An interim transcript.
    Partial {
        /// The utterance.
        id: String,
        /// The whole interim text so far.
        text: String,
    },
    /// The final transcript; it may be empty.
    Final {
        /// The utterance.
        id: String,
        /// The text.
        text: String,
    },
    /// The utterance produced no text.
    ListenFailed {
        /// The utterance.
        id: String,
        /// Why.
        fault: Fault,
    },
    /// The assistant admitted an ask or steer; one per `Ask` or `Steer`
    /// effect, in order, each answered by this or `Refused`.
    Admitted {
        /// The reply it now runs.
        reply: u32,
    },
    /// The assistant refused an ask or steer.
    Refused {
        /// Why.
        fault: Fault,
    },
    /// A change to a reply.
    Reply {
        /// The reply.
        reply: u32,
        /// The change.
        event: AssistEvent,
    },
    /// The pending confirm was decided by click or Enter.
    Decided {
        /// The reply.
        reply: u32,
    },
    /// A speech began playing.
    SpeechStarted {
        /// The speech.
        id: String,
    },
    /// A speech played to its end.
    SpeechEnded {
        /// The speech.
        id: String,
    },
    /// A speech produced no (more) audio.
    SpeechFailed {
        /// The speech.
        id: String,
        /// Why; none when it was cancelled.
        fault: Option<Fault>,
    },
    /// Time passed.
    Tick,
    /// Settings changed.
    Settings {
        /// Spoken replies on.
        spoken: bool,
        /// The person's language.
        language: Language,
    },
    /// The window is closing.
    Dispose,
}

/// What to do, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Open the microphone and buffer frames from now.
    OpenMic,
    /// Close the microphone.
    CloseMic,
    /// Forward frames to the open utterance, or hold them back.
    Gate {
        /// Forward.
        on: bool,
    },
    /// Open the voice socket.
    OpenLink,
    /// Open an utterance; buffered frames belong to it.
    StartUtterance {
        /// Its id.
        id: String,
        /// The person's language.
        language: Language,
    },
    /// Finalize the utterance.
    CommitUtterance {
        /// Its id.
        id: String,
    },
    /// Discard the utterance.
    CancelUtterance {
        /// Its id.
        id: String,
    },
    /// Ask the assistant.
    Ask {
        /// The person's words.
        text: String,
    },
    /// Stop at the next safe step boundary, then continue this conversation
    /// with these words.
    Steer {
        /// The reply, once admitted.
        reply: Option<u32>,
        /// The person's words.
        text: String,
    },
    /// Stop the reply at its next safe point.
    StopAssistant {
        /// The reply, once admitted.
        reply: Option<u32>,
    },
    /// Decide the pending confirm.
    Decide {
        /// The reply.
        reply: u32,
        /// The decision.
        decision: Decision,
    },
    /// Synthesize and play.
    Speak {
        /// The speech id.
        id: String,
        /// What to say.
        speech: Speech,
    },
    /// Silence playback on the device now and drop what is queued.
    FlushSpeech,
    /// Cancel every speech on the socket.
    CancelSpeech,
    /// Play a sound.
    Earcon(Earcon),
    /// Show this.
    View(View),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mic {
    Closed,
    Opening,
    Open,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Link {
    Down,
    Opening,
    Ready,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ConfirmKey {
    reply: u32,
    step: StepRef,
}

struct Utterance {
    id: String,
    press: Press,
    at: u64,
    confirm: Option<ConfirmKey>,
    words: String,
}

struct Pending {
    id: String,
    held: bool,
    confirm: Option<ConfirmKey>,
    at: u64,
    ack: Option<String>,
    result: Option<Result<String, Fault>>,
}

/// One voice session.
pub struct Session {
    config: Config,
    ids: IdSource,
    disposed: bool,
    key: bool,
    mic: Mic,
    warm_until: Option<u64>,
    link: Link,
    open: Option<Utterance>,
    finals: Vec<Pending>,
    reply: Option<Reply>,
    settled: Option<Outcome>,
    fading_since: Option<u64>,
    answer_tail: bool,
    early: Vec<(u32, AssistEvent)>,
    speeches: Vec<String>,
    interim: String,
    heard: Option<String>,
    fault: Option<Fault>,
    shown: Option<View>,
}

impl Session {
    /// An idle session.
    pub fn new(config: Config, ids: IdSource) -> Self {
        Self {
            config,
            ids,
            disposed: false,
            key: false,
            mic: Mic::Closed,
            warm_until: None,
            link: Link::Down,
            open: None,
            finals: Vec::new(),
            reply: None,
            settled: None,
            fading_since: None,
            answer_tail: false,
            early: Vec::new(),
            speeches: Vec::new(),
            interim: String::new(),
            heard: None,
            fault: None,
            shown: None,
        }
    }

    /// Applies `event` at `now` (milliseconds); returns what to do, in order.
    pub fn on(&mut self, event: Event, now: u64) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.disposed {
            return fx;
        }
        self.expire(now, &mut fx);
        match event {
            Event::KeyDown => self.key_down(now, &mut fx),
            Event::KeyUp => self.key_up(now, &mut fx),
            Event::Escape | Event::Cancel => self.escape(now, &mut fx),
            Event::Start if self.open.is_none() => self.begin(Press::Latched, now, &mut fx),
            Event::Start => {}
            Event::Stop => self.commit(false, now, &mut fx),
            Event::MicOpened => self.mic_opened(now, &mut fx),
            Event::MicFailed { fault } => self.mic_failed(fault, &mut fx),
            Event::LinkReady => self.link = Link::Ready,
            Event::LinkLost { fault } => self.link_lost(fault, now, &mut fx),
            Event::Partial { id, text } => self.partial(&id, &text),
            Event::Final { id, text } => self.resolve(&id, Ok(text), now, &mut fx),
            Event::ListenFailed { id, fault } => self.resolve(&id, Err(fault), now, &mut fx),
            Event::Admitted { reply } => self.admitted(reply, &mut fx),
            Event::Refused { fault } => self.refused(fault, &mut fx),
            Event::Reply { reply, event } => self.reply_event(reply, event, &mut fx),
            Event::Decided { reply } => self.decided(reply),
            Event::SpeechStarted { .. } | Event::Tick => {}
            Event::SpeechEnded { id } => self.speeches.retain(|speech| *speech != id),
            Event::SpeechFailed { id, fault } => {
                self.speeches.retain(|speech| *speech != id);
                self.fault = fault.or(self.fault);
            }
            Event::Settings { spoken, language } => {
                self.config.spoken = spoken;
                self.config.language = language;
                if !spoken {
                    self.flush(&mut fx);
                }
            }
            Event::Dispose => {
                self.dispose(&mut fx);
                return fx;
            }
        }
        self.settle(now, &mut fx);
        fx
    }

    /// When the session next needs a [`Event::Tick`]; none while nothing
    /// waits on time.
    pub fn deadline(&self) -> Option<u64> {
        if self.disposed {
            return None;
        }
        let timeout = self.config.final_timeout_ms;
        let finals = self
            .finals
            .iter()
            .filter(|pending| pending.result.is_none())
            .map(|pending| pending.at.saturating_add(timeout));
        let warm = self
            .warm_until
            .filter(|_| self.open.is_none() && self.mic == Mic::Open);
        let fade = self
            .fading_since
            .map(|since| since.saturating_add(self.config.settled_ms));
        finals.chain(warm).chain(fade).min()
    }

    /// What the interface shows now.
    pub fn view(&self) -> View {
        let reply = self.reply.as_ref();
        View {
            state: self.state(),
            mic: self.mic == Mic::Open,
            warm: self.mic == Mic::Open && self.open.is_none(),
            interim: self.interim.clone(),
            heard: self.heard.clone(),
            phase: reply.and_then(|reply| reply.phase.clone()),
            step: reply.and_then(Reply::step_label),
            confirm: reply.and_then(Reply::confirm),
            speaking: !self.speeches.is_empty(),
            outcome: self.settled,
            fault: self.fault,
        }
    }

    fn state(&self) -> State {
        if let Some(utterance) = &self.open {
            return match self.mic {
                Mic::Open => State::Listening(utterance.press),
                Mic::Closed | Mic::Opening => State::Arming(utterance.press),
            };
        }
        if !self.finals.is_empty() {
            return State::Finalizing;
        }
        if let Some(reply) = &self.reply {
            return State::Working(reply.work());
        }
        self.settled.map_or(State::Idle, State::Settled)
    }

    fn key_down(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if std::mem::replace(&mut self.key, true) {
            return;
        }
        match self.open.as_ref().map(|utterance| utterance.press) {
            None => self.begin(Press::Held, now, fx),
            Some(Press::Latched) => self.commit(false, now, fx),
            Some(Press::Held) => {}
        }
    }

    fn key_up(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if !std::mem::replace(&mut self.key, false) {
            return;
        }
        let hold = self.config.hold_ms;
        let Some(utterance) = self.open.as_mut().filter(|u| u.press == Press::Held) else {
            return;
        };
        if now.saturating_sub(utterance.at) >= hold {
            self.commit(true, now, fx);
        } else {
            utterance.press = Press::Latched;
        }
    }

    fn begin(&mut self, press: Press, now: u64, fx: &mut Vec<Effect>) {
        let flushed = !self.speeches.is_empty();
        if flushed {
            fx.push(Effect::FlushSpeech);
            self.speeches.clear();
        }
        if let Some(reply) = &mut self.reply {
            reply.muted = true;
        }
        if self.mic == Mic::Closed {
            fx.push(Effect::OpenMic);
            self.mic = Mic::Opening;
        }
        fx.push(Effect::Gate { on: true });
        let id = (self.ids)();
        fx.push(Effect::StartUtterance {
            id: id.clone(),
            language: self.config.language,
        });
        self.link_up(fx);
        if flushed {
            fx.push(Effect::CancelSpeech);
        }
        if self.mic == Mic::Open {
            fx.push(Effect::Earcon(Earcon::ListeningStarted));
        }
        self.warm_until = None;
        self.fault = None;
        self.settled = None;
        self.interim.clear();
        let confirm = self.confirm_key();
        self.open = Some(Utterance {
            id,
            press,
            at: now,
            confirm,
            words: String::new(),
        });
    }

    fn commit(&mut self, held: bool, now: u64, fx: &mut Vec<Effect>) {
        let Some(utterance) = self.open.take() else {
            return;
        };
        fx.push(Effect::Gate { on: false });
        fx.push(Effect::CommitUtterance {
            id: utterance.id.clone(),
        });
        fx.push(Effect::Earcon(Earcon::ListeningEnded));
        self.warm_until = Some(now.saturating_add(self.config.warm_ms));
        let held = held && utterance.press == Press::Held;
        let ack = if held && utterance.confirm.is_some() {
            None
        } else {
            phrases::ack(&text::clean(&utterance.words), self.config.language)
                .and_then(|ack| self.speak(Speech::Line(Line::Ack(ack)), fx))
        };
        self.finals.push(Pending {
            id: utterance.id,
            held,
            confirm: utterance.confirm,
            at: now,
            ack,
            result: None,
        });
    }

    fn end_listening(&mut self, now: u64, fx: &mut Vec<Effect>) -> bool {
        let Some(utterance) = self.open.take() else {
            return false;
        };
        fx.push(Effect::Gate { on: false });
        fx.push(Effect::CancelUtterance { id: utterance.id });
        self.warm_until = Some(now.saturating_add(self.config.warm_ms));
        self.interim.clear();
        true
    }

    fn escape(&mut self, now: u64, fx: &mut Vec<Effect>) {
        let mut discarded = self.end_listening(now, fx);
        let mut acked = None;
        for pending in std::mem::take(&mut self.finals) {
            fx.push(Effect::CancelUtterance { id: pending.id });
            acked = acked.or(pending.ack);
            discarded = true;
        }
        if discarded {
            self.retract(acked, fx);
            return;
        }
        if let Some(reply) = &mut self.reply {
            fx.push(Effect::StopAssistant { reply: reply.id });
            reply.muted = true;
        } else {
            self.settled = None;
            self.fault = None;
        }
        self.flush(fx);
    }

    fn mic_opened(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if self.mic != Mic::Opening {
            return;
        }
        self.mic = Mic::Open;
        if self.open.is_some() {
            fx.push(Effect::Earcon(Earcon::ListeningStarted));
        } else if self.warm_until.is_none() {
            self.warm_until = Some(now.saturating_add(self.config.warm_ms));
        }
    }

    fn mic_failed(&mut self, fault: Fault, fx: &mut Vec<Effect>) {
        self.mic = Mic::Closed;
        self.warm_until = None;
        if let Some(utterance) = self.open.take() {
            fx.push(Effect::CancelUtterance { id: utterance.id });
            self.interim.clear();
            self.fail(fault, fx);
        }
    }

    fn link_up(&mut self, fx: &mut Vec<Effect>) {
        if self.link == Link::Down {
            fx.push(Effect::OpenLink);
            self.link = Link::Opening;
        }
    }

    fn link_lost(&mut self, fault: Option<Fault>, now: u64, fx: &mut Vec<Effect>) {
        self.link = Link::Down;
        self.speeches.clear();
        let mut lost = self.end_listening(now, fx);
        for pending in self.finals.drain(..) {
            fx.push(Effect::CancelUtterance { id: pending.id });
            lost = true;
        }
        if lost {
            self.fail(fault.unwrap_or(Fault::Interrupted), fx);
        }
    }

    fn partial(&mut self, id: &str, text: &str) {
        let ours = self.open.as_ref().is_some_and(|u| u.id == id)
            || self.finals.iter().any(|p| p.id == id && p.result.is_none());
        if ours {
            self.interim = text::bounded(&text::clean(text), MAX_SHOWN_CHARS);
        }
        if let Some(utterance) = self.open.as_mut().filter(|u| u.id == id) {
            utterance.words = text::bounded(text, MAX_SHOWN_CHARS);
        }
    }

    fn resolve(&mut self, id: &str, result: Result<String, Fault>, now: u64, fx: &mut Vec<Effect>) {
        if self.open.as_ref().is_some_and(|u| u.id == id) {
            if let Err(fault) = result {
                self.end_listening(now, fx);
                self.fail(fault, fx);
            }
            return;
        }
        let Some(pending) = self
            .finals
            .iter_mut()
            .find(|p| p.id == id && p.result.is_none())
        else {
            return;
        };
        pending.result = Some(result);
        self.drain(fx);
    }

    fn expire(&mut self, now: u64, fx: &mut Vec<Effect>) {
        let timeout = self.config.final_timeout_ms;
        for pending in &mut self.finals {
            if pending.result.is_none() && now.saturating_sub(pending.at) >= timeout {
                pending.result = Some(Err(Fault::Timeout));
                fx.push(Effect::CancelUtterance {
                    id: pending.id.clone(),
                });
            }
        }
        self.drain(fx);
        let shown = self.config.settled_ms;
        if self
            .fading_since
            .is_some_and(|since| now >= since.saturating_add(shown))
        {
            self.fading_since = None;
            self.settled = None;
            self.fault = self.fault.filter(|fault| fault.persists());
        }
    }

    fn cool(&mut self, now: u64, fx: &mut Vec<Effect>) {
        if self.open.is_none()
            && self.mic == Mic::Open
            && self.warm_until.is_none_or(|until| now >= until)
        {
            fx.push(Effect::CloseMic);
            self.mic = Mic::Closed;
            self.warm_until = None;
        }
    }

    fn drain(&mut self, fx: &mut Vec<Effect>) {
        while self.finals.first().is_some_and(|p| p.result.is_some()) {
            let Pending {
                held,
                confirm,
                ack,
                result,
                ..
            } = self.finals.remove(0);
            match result {
                Some(Ok(text)) => self.heard(held, confirm, ack, &text, fx),
                Some(Err(fault)) => {
                    self.retract(ack, fx);
                    self.fail(fault, fx);
                }
                None => {}
            }
        }
    }

    fn retract(&mut self, ack: Option<String>, fx: &mut Vec<Effect>) {
        if ack.is_some_and(|id| self.speeches.contains(&id)) {
            self.flush(fx);
        }
    }

    fn heard(
        &mut self,
        held: bool,
        confirm: Option<ConfirmKey>,
        acked: Option<String>,
        text: &str,
        fx: &mut Vec<Effect>,
    ) {
        let text =
            text::verbatim_prefix(&text::clean(text), PAGE_ASSIST_MAX_QUESTION_BYTES).to_owned();
        if self.open.is_none() && self.finals.is_empty() {
            self.interim.clear();
        }
        if text.is_empty() {
            self.retract(acked, fx);
            return;
        }
        self.heard = Some(text::bounded(&text, MAX_SHOWN_CHARS));
        let language = self.config.language;
        match &mut self.reply {
            Some(reply) => {
                let decision = phrases::decision(&text, language);
                if let (true, Some(key), Some(decision)) = (held, confirm, decision) {
                    if reply.id == Some(key.reply) && reply.pending() == Some(key.step) {
                        fx.push(Effect::Decide {
                            reply: key.reply,
                            decision,
                        });
                        reply.decided.push(key.step);
                        reply.muted = false;
                        return;
                    }
                }
                fx.push(Effect::Steer {
                    reply: reply.id,
                    text: text.clone(),
                });
                reply.steering = true;
                reply.awaiting = reply.awaiting.saturating_add(1);
                reply.muted = true;
            }
            None => {
                fx.push(Effect::Ask { text: text.clone() });
                self.reply = Some(Reply {
                    awaiting: 1,
                    ..Reply::default()
                });
            }
        }
        if acked.is_none() {
            if let Some(ack) = phrases::ack(&text, language) {
                self.speak(Speech::Line(Line::Ack(ack)), fx);
            }
        }
    }

    fn admitted(&mut self, id: u32, fx: &mut Vec<Effect>) {
        let Some(reply) = self.reply.as_mut().filter(|reply| reply.awaiting > 0) else {
            return;
        };
        reply.awaiting -= 1;
        if reply.awaiting == 0 {
            let muted = reply.muted && !reply.steering;
            if reply.id == Some(id) {
                reply.restart();
            } else {
                *reply = Reply::default();
            }
            reply.muted = muted;
        } else if reply.id != Some(id) {
            *reply = Reply {
                muted: reply.muted,
                steering: reply.steering,
                awaiting: reply.awaiting,
                ..Reply::default()
            };
        }
        reply.id = Some(id);
        let last = reply.awaiting == 0;
        let (now, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.early)
            .into_iter()
            .partition(|(reply, _)| *reply == id);
        if !last {
            self.early = later;
        }
        for (_, event) in now {
            self.reply_event(id, event, fx);
        }
    }

    fn refused(&mut self, fault: Fault, fx: &mut Vec<Effect>) {
        let Some(reply) = self.reply.as_mut().filter(|reply| reply.awaiting > 0) else {
            return;
        };
        reply.awaiting -= 1;
        if reply.awaiting > 0 {
            return;
        }
        if reply.id.is_none() {
            self.reply = None;
            self.flush(fx);
            self.fail(fault, fx);
            return;
        }
        if reply.steering {
            reply.steering = false;
            let ended = reply.ended;
            self.fail(fault, fx);
            if ended {
                self.end(fx);
            }
        }
    }

    fn reply_event(&mut self, id: u32, event: AssistEvent, fx: &mut Vec<Effect>) {
        if matches!(event, AssistEvent::Invalidated) {
            if let Some(reply) = &mut self.reply {
                reply.state = Some(ReplyState::Cancelled);
                reply.steering = false;
                reply.awaiting = 0;
                self.end(fx);
            }
            return;
        }
        let Some(reply) = self.reply.as_mut() else {
            return;
        };
        if reply.id != Some(id) {
            if reply.awaiting > 0 && self.early.len() < MAX_EARLY_EVENTS {
                self.early.push((id, event));
            }
            return;
        }
        let mut speech = Vec::new();
        let ended = match event {
            AssistEvent::Message { message } => match *message {
                Message::Reply {
                    state,
                    phase,
                    blocks,
                    fault,
                    end,
                    ..
                } => {
                    reply.phase = phase;
                    reply.fault = fault;
                    reply.end = end;
                    for (index, block) in (0..=u16::MAX).zip(blocks) {
                        speech.extend(reply.block(index, block));
                    }
                    speech.extend(reply.set_state(state));
                    terminal(state)
                }
                Message::Person { .. } => false,
            },
            AssistEvent::Phase { phase } => {
                reply.phase = phase;
                false
            }
            AssistEvent::Text { block, delta } => {
                speech.extend(reply.text(block, &delta));
                false
            }
            AssistEvent::Block { block, value } => {
                speech.extend(reply.block(block, value));
                false
            }
            AssistEvent::State { state } => {
                speech.extend(reply.set_state(state));
                terminal(state)
            }
            AssistEvent::Done { end, .. } => {
                reply.state = Some(ReplyState::Done);
                reply.end = end;
                true
            }
            AssistEvent::Failed { fault } => {
                reply.state = Some(ReplyState::Failed);
                reply.fault = Some(fault);
                true
            }
            AssistEvent::Cancelled => {
                reply.state = Some(ReplyState::Cancelled);
                true
            }
            AssistEvent::Cites { .. }
            | AssistEvent::Cost { .. }
            | AssistEvent::Inert
            | AssistEvent::Titled { .. }
            | AssistEvent::Invalidated => false,
        };
        for line in speech {
            self.speak(line, fx);
        }
        if ended {
            self.end(fx);
        }
    }

    fn end(&mut self, fx: &mut Vec<Effect>) {
        let Some(mut reply) = self.reply.take() else {
            return;
        };
        if reply.steering {
            reply.ended = true;
            self.reply = Some(reply);
            return;
        }
        let tail = reply.cutter.finish();
        let spoken = reply.route(tail);
        let facts = reply.facts();
        self.settled = match reply.end {
            Some(End::Redirected) => None,
            Some(End::Asked) => Some(Outcome::NeedsYou),
            None => Some(closing::outcome(&facts)),
        };
        if reply.muted || reply.end == Some(End::Redirected) {
            return;
        }
        for speech in spoken {
            self.speak(speech, fx);
        }
        if reply.end == Some(End::Asked) {
            for chunk in std::mem::take(&mut reply.held) {
                self.speak(Speech::Text(chunk), fx);
            }
            self.answer_tail = true;
            return;
        }
        match closing::closing(&facts) {
            Closing::Say => {
                let held = std::mem::take(&mut reply.held);
                if held.is_empty() && self.settled == Some(Outcome::Acted) {
                    self.speak(Speech::Line(Line::Closing(Closing::Done)), fx);
                }
                for chunk in held {
                    self.speak(Speech::Text(chunk), fx);
                }
            }
            Closing::Paused if reply.paused => {}
            Closing::NeedsYou { step: Some(step) } if reply.prompted.contains(&step) => {}
            line => {
                self.speak(Speech::Line(Line::Closing(line)), fx);
            }
        }
    }

    fn decided(&mut self, id: u32) {
        if let Some(reply) = self.reply.as_mut().filter(|reply| reply.id == Some(id)) {
            if let Some(step) = reply.pending() {
                reply.decided.push(step);
            }
        }
    }

    fn confirm_key(&self) -> Option<ConfirmKey> {
        let reply = self.reply.as_ref()?;
        Some(ConfirmKey {
            reply: reply.id?,
            step: reply.pending()?,
        })
    }

    fn speak(&mut self, speech: Speech, fx: &mut Vec<Effect>) -> Option<String> {
        if !self.config.spoken || self.open.is_some() {
            return None;
        }
        let id = (self.ids)();
        self.link_up(fx);
        fx.push(Effect::Speak {
            id: id.clone(),
            speech,
        });
        self.speeches.push(id.clone());
        Some(id)
    }

    fn flush(&mut self, fx: &mut Vec<Effect>) {
        if !self.speeches.is_empty() {
            self.speeches.clear();
            fx.push(Effect::FlushSpeech);
            fx.push(Effect::CancelSpeech);
        }
    }

    fn fail(&mut self, fault: Fault, fx: &mut Vec<Effect>) {
        self.fault = Some(fault);
        fx.push(Effect::Earcon(Earcon::Error));
    }

    fn dispose(&mut self, fx: &mut Vec<Effect>) {
        if let Some(utterance) = self.open.take() {
            fx.push(Effect::CancelUtterance { id: utterance.id });
        }
        for pending in self.finals.drain(..) {
            fx.push(Effect::CancelUtterance { id: pending.id });
        }
        if self.mic != Mic::Closed {
            fx.push(Effect::Gate { on: false });
            fx.push(Effect::CloseMic);
            self.mic = Mic::Closed;
        }
        self.flush(fx);
        self.disposed = true;
    }

    fn settle(&mut self, now: u64, fx: &mut Vec<Effect>) {
        let state = self.state();
        if state == State::Idle {
            self.flush(fx);
        }
        if self.reply.as_ref().is_none_or(|reply| reply.awaiting == 0) {
            self.early.clear();
        }
        if std::mem::take(&mut self.answer_tail) && self.open.is_none() {
            if self.mic == Mic::Closed {
                fx.push(Effect::OpenMic);
                self.mic = Mic::Opening;
            }
            let until = now.saturating_add(self.config.answer_warm_ms);
            self.warm_until = Some(self.warm_until.map_or(until, |warm| warm.max(until)));
        }
        let fading = self.speeches.is_empty()
            && ((matches!(state, State::Idle | State::Settled(_)) && self.settled.is_some())
                || self.fault.is_some_and(|fault| !fault.persists()));
        if fading {
            self.fading_since.get_or_insert(now);
        } else {
            self.fading_since = None;
        }
        self.cool(now, fx);
        let view = self.view();
        if self.shown.as_ref() != Some(&view) {
            fx.push(Effect::View(view.clone()));
            self.shown = Some(view);
        }
    }
}

fn terminal(state: ReplyState) -> bool {
    matches!(
        state,
        ReplyState::Done | ReplyState::Failed | ReplyState::Cancelled
    )
}

#[derive(Default)]
struct Reply {
    id: Option<u32>,
    state: Option<ReplyState>,
    phase: Option<Phase>,
    blocks: BTreeMap<u16, Block>,
    fault: Option<AssistFault>,
    cutter: Cutter,
    text_block: Option<u16>,
    held: Vec<String>,
    end: Option<End>,
    muted: bool,
    steering: bool,
    awaiting: u8,
    ended: bool,
    paused: bool,
    decided: Vec<StepRef>,
    prompted: Vec<StepRef>,
}

impl Reply {
    fn restart(&mut self) {
        self.state = None;
        self.cutter = Cutter::default();
        self.text_block = None;
        self.held.clear();
        self.steering = false;
        self.ended = false;
        self.paused = false;
    }

    fn facts(&self) -> ReplyFacts {
        ReplyFacts::from_blocks(
            self.state.unwrap_or(ReplyState::Working),
            self.blocks.iter().map(|(index, block)| (*index, block)),
            self.fault,
        )
    }

    fn work(&self) -> Work {
        match (self.state, &self.phase) {
            (Some(ReplyState::Waiting | ReplyState::Paused), _) => Work::Confirming,
            (_, Some(Phase::Reading { .. })) => Work::Reading,
            (_, Some(Phase::Acting | Phase::Opening | Phase::Styling)) => Work::Acting,
            _ => Work::Thinking,
        }
    }

    fn set_state(&mut self, state: ReplyState) -> Vec<Speech> {
        self.state = Some(state);
        if state != ReplyState::Paused {
            self.paused = false;
            return Vec::new();
        }
        if std::mem::replace(&mut self.paused, true) || self.muted {
            return Vec::new();
        }
        vec![Speech::Line(Line::Closing(Closing::Paused))]
    }

    fn unsettled(&self) -> Vec<StepRef> {
        self.facts()
            .steps
            .into_iter()
            .filter(|fact| matches!(fact.state, StepState::Running | StepState::Confirm))
            .map(|fact| fact.step)
            .collect()
    }

    fn pending(&self) -> Option<StepRef> {
        self.facts()
            .steps
            .into_iter()
            .find(|fact| {
                fact.state == StepState::Confirm
                    && fact.step.kind != StepKind::Handoff
                    && !self.decided.contains(&fact.step)
            })
            .map(|fact| fact.step)
    }

    fn block(&mut self, index: u16, value: Block) -> Vec<Speech> {
        if matches!(value, Block::Text { .. }) {
            return Vec::new();
        }
        let mut speech = Vec::new();
        let appended = !self.blocks.contains_key(&index);
        if appended && self.text_block.is_some_and(|text| text < index) {
            self.text_block = None;
            let chunks = self.cutter.end_block();
            speech.extend(self.route(chunks));
        }
        let before = self.unsettled();
        self.blocks.insert(index, value);
        if self.unsettled().iter().any(|step| !before.contains(step)) {
            speech.extend(self.held.drain(..).map(Speech::Text));
        }
        let waiting: Vec<StepRef> = self
            .facts()
            .steps
            .into_iter()
            .filter(|fact| fact.state == StepState::Confirm && !self.prompted.contains(&fact.step))
            .map(|fact| fact.step)
            .collect();
        for step in waiting {
            self.prompted.push(step);
            speech.push(Speech::Line(Line::Closing(Closing::NeedsYou {
                step: Some(step),
            })));
        }
        if self.muted {
            speech.clear();
        }
        speech
    }

    fn text(&mut self, block: u16, delta: &str) -> Vec<Speech> {
        let mut chunks = Vec::new();
        if self.text_block.is_some_and(|current| current != block) {
            chunks.extend(self.cutter.end_block());
        }
        self.text_block = Some(block);
        chunks.extend(self.cutter.push(delta));
        self.route(chunks)
    }

    fn route(&mut self, chunks: Vec<String>) -> Vec<Speech> {
        if self.muted {
            return Vec::new();
        }
        if self.facts().steps.is_empty() {
            return chunks.into_iter().map(Speech::Text).collect();
        }
        self.held.extend(chunks);
        Vec::new()
    }

    fn label(&self, step: StepRef) -> Option<String> {
        let label = match self.blocks.get(&step.block)? {
            Block::Steps { steps } => steps.iter().find(|s| s.index == step.index)?.label.clone(),
            Block::Search { query, .. } => query.clone(),
            Block::Opened { tabs } => tabs.iter().find(|t| t.index == step.index)?.host.clone(),
            Block::Style { style } => style.summary.clone(),
            Block::Handoff { handoff } => handoff.title.clone(),
            Block::Text { .. } | Block::Items { .. } | Block::Paused { .. } => return None,
        };
        Some(text::bounded(&text::clean(&label), MAX_LABEL_CHARS))
    }

    fn step_label(&self) -> Option<String> {
        let facts = self.facts();
        let current = facts
            .steps
            .iter()
            .rev()
            .find(|fact| fact.state == StepState::Running)
            .or_else(|| {
                facts
                    .steps
                    .iter()
                    .find(|fact| fact.state == StepState::Confirm)
            })?;
        self.label(current.step)
    }

    fn confirm(&self) -> Option<Confirm> {
        let step = self.pending()?;
        Some(Confirm {
            step,
            label: self.label(step).unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{
        PageAssistStepReasonV1 as Reason, PageAssistStepV1 as Step,
        WorkConfirmCategoryV1 as Category,
    };
    use proptest::prelude::*;

    fn ids() -> IdSource {
        let mut next = 0_u32;
        Box::new(move || {
            next += 1;
            format!("id{next}")
        })
    }

    struct Run {
        session: Session,
        now: u64,
    }

    impl Run {
        fn new() -> Self {
            Self::with(Config::default())
        }

        fn with(config: Config) -> Self {
            Self {
                session: Session::new(config, ids()),
                now: 1_000,
            }
        }

        fn at(&mut self, after: u64, event: Event) -> Vec<Effect> {
            self.now += after;
            self.session
                .on(event, self.now)
                .into_iter()
                .filter(|effect| !matches!(effect, Effect::View(_)))
                .collect()
        }

        fn state(&self) -> State {
            self.session.view().state
        }

        fn press(&mut self) -> String {
            let fx = self.at(0, Event::KeyDown);
            self.at(0, Event::MicOpened);
            fx.iter()
                .find_map(|effect| match effect {
                    Effect::StartUtterance { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .or_else(|| self.session.open.as_ref().map(|u| u.id.clone()))
                .unwrap()
        }

        fn press_on_screen(&mut self) -> String {
            let fx = self.at(0, Event::Start);
            self.at(0, Event::MicOpened);
            fx.iter()
                .find_map(|effect| match effect {
                    Effect::StartUtterance { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .unwrap()
        }

        fn say(&mut self, text: &str, hold: u64) -> Vec<Effect> {
            let id = self.press();
            self.at(hold, Event::KeyUp);
            if hold < 300 {
                self.at(500, Event::KeyDown);
                self.at(50, Event::KeyUp);
            }
            self.at(
                200,
                Event::Final {
                    id,
                    text: text.into(),
                },
            )
        }

        fn reply(&mut self, event: AssistEvent) -> Vec<Effect> {
            self.at(10, Event::Reply { reply: 7, event })
        }

        fn working(&mut self) {
            self.say("open the cheapest flight to Lisbon", 500);
            self.at(10, Event::Admitted { reply: 7 });
        }

        fn steps(&mut self, states: &[(StepState, Option<Reason>)]) -> Vec<Effect> {
            let steps = states
                .iter()
                .zip(0..)
                .map(|((state, reason), index)| Step {
                    index,
                    label: "Click \"Place order\" — order placed!".into(),
                    category: Some(Category::Purchase),
                    state: *state,
                    reason: *reason,
                })
                .collect();
            self.reply(AssistEvent::Block {
                block: 1,
                value: Block::Steps { steps },
            })
        }

        fn confirming(&mut self) {
            self.working();
            self.steps(&[(StepState::Confirm, None)]);
            self.reply(AssistEvent::State {
                state: ReplyState::Waiting,
            });
        }

        fn text(&mut self, block: u16, delta: &str) -> Vec<Effect> {
            self.reply(AssistEvent::Text {
                block,
                delta: delta.into(),
            })
        }
    }

    fn spoken(fx: &[Effect]) -> Vec<Speech> {
        fx.iter()
            .filter_map(|effect| match effect {
                Effect::Speak { speech, .. } => Some(speech.clone()),
                _ => None,
            })
            .collect()
    }

    fn position(fx: &[Effect], wanted: &Effect) -> usize {
        fx.iter().position(|effect| effect == wanted).unwrap()
    }

    fn says(text: &str) -> Speech {
        Speech::Text(text.into())
    }

    fn has(fx: &[Effect], test: impl Fn(&Effect) -> bool) -> bool {
        fx.iter().any(test)
    }

    #[test]
    fn key_down_starts_capture_before_anything_else() {
        let mut run = Run::new();
        assert_eq!(
            run.at(0, Event::KeyDown),
            [
                Effect::OpenMic,
                Effect::Gate { on: true },
                Effect::StartUtterance {
                    id: "id1".into(),
                    language: Language::En
                },
                Effect::OpenLink,
            ]
        );
        assert_eq!(run.state(), State::Arming(Press::Held));
        assert_eq!(
            run.at(40, Event::MicOpened),
            [Effect::Earcon(Earcon::ListeningStarted)]
        );
        assert_eq!(run.state(), State::Listening(Press::Held));
        assert!(run.at(5, Event::KeyDown).is_empty());
    }

    #[test]
    fn long_presses_commit_and_short_ones_latch_until_the_next_press() {
        let mut run = Run::new();
        run.press();
        assert_eq!(
            run.at(300, Event::KeyUp),
            [
                Effect::Gate { on: false },
                Effect::CommitUtterance { id: "id1".into() },
                Effect::Earcon(Earcon::ListeningEnded),
            ]
        );
        assert_eq!(run.state(), State::Finalizing);

        let mut run = Run::new();
        run.press();
        assert!(run.at(299, Event::KeyUp).is_empty());
        assert_eq!(run.state(), State::Listening(Press::Latched));
        let fx = run.at(20_000, Event::KeyDown);
        assert!(has(&fx, |e| matches!(e, Effect::CommitUtterance { .. })));
        assert!(!has(&fx, |e| matches!(e, Effect::StartUtterance { .. })));
        assert!(run.at(100, Event::KeyUp).is_empty());
        assert_eq!(run.state(), State::Finalizing);

        let mut run = Run::new();
        run.at(0, Event::Start);
        run.at(0, Event::MicOpened);
        assert_eq!(run.state(), State::Listening(Press::Latched));
        assert!(has(&run.at(3_000, Event::Stop), |e| matches!(
            e,
            Effect::CommitUtterance { .. }
        )));
    }

    #[test]
    fn a_final_asks_with_a_neutral_acknowledgment_and_an_empty_one_is_silent() {
        let mut run = Run::new();
        let fx = run.say("open the cheapest flight to Lisbon", 500);
        assert_eq!(
            fx[0],
            Effect::Ask {
                text: "open the cheapest flight to Lisbon".into()
            }
        );
        assert!(matches!(spoken(&fx)[..], [Speech::Line(Line::Ack(_))]));
        assert_eq!(run.state(), State::Working(Work::Thinking));

        let mut run = Run::new();
        let fx = run.say("   ", 500);
        assert!(fx.is_empty(), "{fx:?}");
        assert_eq!(run.state(), State::Idle);
    }

    #[test]
    fn barge_in_flushes_speech_before_the_microphone_and_mutes_the_old_reply() {
        let mut run = Run::new();
        run.working();
        let fx = run.text(
            0,
            "The cheapest flight leaves at 9 tonight. It costs 120 euros. And",
        );
        assert_eq!(
            spoken(&fx),
            [
                says("The cheapest flight leaves at 9 tonight."),
                says("It costs 120 euros.")
            ]
        );
        run.now += 10_000;
        let fx = run.at(0, Event::KeyDown);
        assert_eq!(fx[0], Effect::FlushSpeech);
        assert!(position(&fx, &Effect::FlushSpeech) < position(&fx, &Effect::Gate { on: true }));
        assert!(!has(&fx, |e| matches!(
            e,
            Effect::OpenMic | Effect::CloseMic
        )));
        assert!(has(&fx, |e| *e == Effect::CancelSpeech));
        assert!(!run.session.view().speaking);
        assert!(spoken(&run.text(0, " there is more. Much more. ")).is_empty());
        run.at(0, Event::MicOpened);
        run.at(600, Event::KeyUp);
        let fx = run.at(
            100,
            Event::Final {
                id: "id5".into(),
                text: "make it a window seat".into(),
            },
        );
        assert_eq!(
            fx[0],
            Effect::Steer {
                reply: Some(7),
                text: "make it a window seat".into()
            }
        );
        assert!(spoken(&run.text(0, "Still the old reply. ")).is_empty());
        run.at(10, Event::Admitted { reply: 8 });
        let fx = run.at(
            10,
            Event::Reply {
                reply: 8,
                event: AssistEvent::Text {
                    block: 0,
                    delta: "Window seat it is.\n".into(),
                },
            },
        );
        assert_eq!(spoken(&fx), [says("Window seat it is.")]);
    }

    #[test]
    fn a_held_whole_phrase_decides_the_confirm_that_was_pending_at_key_down() {
        let mut run = Run::new();
        run.confirming();
        assert_eq!(run.state(), State::Working(Work::Confirming));
        let confirm = run.session.view().confirm.unwrap();
        assert_eq!(confirm.step.category, Some(Category::Purchase));
        let fx = run.say("Yes.", 500);
        assert_eq!(
            fx[0],
            Effect::Decide {
                reply: 7,
                decision: Decision::Approve
            }
        );
        assert!(!has(&fx, |e| matches!(e, Effect::Steer { .. })));
        assert!(run.session.view().confirm.is_none());
        assert!(!has(&run.say("yes", 500), |e| matches!(
            e,
            Effect::Decide { .. }
        )));

        let mut run = Run::new();
        run.confirming();
        let fx = run.say("no", 500);
        assert_eq!(
            fx[0],
            Effect::Decide {
                reply: 7,
                decision: Decision::Decline
            }
        );
    }

    #[test]
    fn anything_else_steers_instead_of_deciding() {
        let steer = |fx: &[Effect]| {
            has(fx, |e| matches!(e, Effect::Steer { .. }))
                && !has(fx, |e| matches!(e, Effect::Decide { .. }))
        };
        let mut run = Run::new();
        run.confirming();
        assert!(steer(&run.say("yes", 100)), "latched");

        let mut run = Run::new();
        run.confirming();
        assert!(steer(&run.say("yes but check the price first", 500)));

        let mut run = Run::new();
        run.confirming();
        let id = run.press_on_screen();
        run.at(900, Event::Stop);
        assert!(
            steer(&run.at(
                100,
                Event::Final {
                    id,
                    text: "yes".into()
                }
            )),
            "on-screen"
        );

        let mut run = Run::new();
        run.working();
        let id = run.press();
        run.steps(&[(StepState::Confirm, None)]);
        run.at(600, Event::KeyUp);
        assert!(
            steer(&run.at(
                100,
                Event::Final {
                    id,
                    text: "yes".into()
                }
            )),
            "appeared mid-press"
        );

        let mut run = Run::new();
        run.confirming();
        run.at(10, Event::Decided { reply: 7 });
        assert!(steer(&run.say("yes", 500)), "decided elsewhere");
    }

    #[test]
    fn finals_are_delivered_in_commit_order() {
        let mut run = Run::new();
        let first = run.press();
        run.at(400, Event::KeyUp);
        let second = run.press();
        assert_ne!(first, second);
        run.at(400, Event::KeyUp);
        let fx = run.at(
            10,
            Event::Final {
                id: second,
                text: "and make it two tickets".into(),
            },
        );
        assert!(!has(&fx, |e| matches!(
            e,
            Effect::Ask { .. } | Effect::Steer { .. }
        )));
        let fx = run.at(
            10,
            Event::Final {
                id: first,
                text: "find flights to Lisbon tomorrow".into(),
            },
        );
        let ask = position(
            &fx,
            &Effect::Ask {
                text: "find flights to Lisbon tomorrow".into(),
            },
        );
        let steer = position(
            &fx,
            &Effect::Steer {
                reply: None,
                text: "and make it two tickets".into(),
            },
        );
        assert!(ask < steer);
    }

    #[test]
    fn the_microphone_stays_warm_for_the_tail_then_closes() {
        let mut run = Run::new();
        run.press();
        run.at(400, Event::KeyUp);
        assert!(run.session.view().warm);
        assert!(!has(&run.at(5_999, Event::Tick), |e| *e == Effect::CloseMic));
        let fx = run.at(0, Event::KeyDown);
        assert!(!has(&fx, |e| *e == Effect::OpenMic));
        assert!(has(&fx, |e| *e == Effect::Earcon(Earcon::ListeningStarted)));
        run.at(400, Event::KeyUp);
        assert!(!has(&run.at(5_999, Event::Tick), |e| *e == Effect::CloseMic));
        assert!(has(&run.at(1, Event::Tick), |e| *e == Effect::CloseMic));
        assert!(!run.session.view().mic);
    }

    #[test]
    fn faults_end_the_utterance_and_return_to_idle() {
        let mut run = Run::new();
        run.at(0, Event::KeyDown);
        let fx = run.at(
            30,
            Event::MicFailed {
                fault: Fault::MicrophoneDenied,
            },
        );
        assert_eq!(
            fx,
            [
                Effect::CancelUtterance { id: "id1".into() },
                Effect::Earcon(Earcon::Error)
            ]
        );
        assert_eq!(run.state(), State::Idle);
        assert_eq!(run.session.view().fault, Some(Fault::MicrophoneDenied));

        let mut run = Run::new();
        let id = run.press();
        run.at(400, Event::KeyUp);
        let fx = run.at(8_000, Event::Tick);
        assert!(has(&fx, |e| *e == Effect::CancelUtterance { id: id.clone() }));
        assert_eq!(run.session.view().fault, Some(Fault::Timeout));
        assert!(run
            .at(
                10,
                Event::Final {
                    id,
                    text: "late words".into()
                }
            )
            .is_empty());
        assert_eq!(run.state(), State::Idle);

        let mut run = Run::new();
        run.press();
        let fx = run.at(100, Event::LinkLost { fault: None });
        assert!(has(&fx, |e| *e == Effect::Gate { on: false }));
        assert_eq!(run.session.view().fault, Some(Fault::Interrupted));
        assert_eq!(run.state(), State::Idle);

        let mut run = Run::new();
        let id = run.press();
        run.at(400, Event::KeyUp);
        run.at(
            100,
            Event::ListenFailed {
                id,
                fault: Fault::OutOfCredit,
            },
        );
        assert_eq!(run.session.view().fault, Some(Fault::OutOfCredit));
        assert_eq!(run.state(), State::Idle);

        let mut run = Run::new();
        let fx = run.say("open the cheapest flight to Lisbon", 500);
        assert!(!spoken(&fx).is_empty());
        let fx = run.at(10, Event::Refused { fault: Fault::Busy });
        assert!(has(&fx, |e| *e == Effect::FlushSpeech));
        assert_eq!(run.state(), State::Idle);
        assert_eq!(run.session.view().fault, Some(Fault::Busy));
        assert!(!run.session.view().speaking);
    }

    #[test]
    fn a_speech_failure_never_fails_the_reply() {
        let mut run = Run::new();
        run.working();
        let fx = run.text(0, "It leaves at nine.\n");
        let Some(Effect::Speak { id, .. }) = fx.iter().find(|e| matches!(e, Effect::Speak { .. }))
        else {
            panic!("{fx:?}");
        };
        run.at(
            10,
            Event::SpeechFailed {
                id: id.clone(),
                fault: Some(Fault::ProviderDown),
            },
        );
        assert_eq!(run.state(), State::Working(Work::Thinking));
        run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert_eq!(run.state(), State::Settled(Outcome::Answered));
    }

    #[test]
    fn success_is_spoken_only_when_every_step_is_verified() {
        let mut run = Run::new();
        run.working();
        assert_eq!(
            spoken(&run.text(0, "I'll open the cart and place it. ")),
            []
        );
        assert_eq!(
            spoken(&run.steps(&[(StepState::Running, None)])),
            [says("I'll open the cart and place it.")]
        );
        assert_eq!(
            run.session.view().step.as_deref(),
            Some("Click \"Place order\" — order placed!")
        );
        assert!(spoken(&run.text(2, "Your order is placed. ")).is_empty());
        run.steps(&[(StepState::Done, None)]);
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert_eq!(spoken(&fx), [says("Your order is placed.")]);
        assert_eq!(run.state(), State::Settled(Outcome::Acted));

        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Running, None)]);
        assert!(spoken(&run.text(2, "Your order is placed. ")).is_empty());
        run.steps(&[(StepState::Failed, Some(Reason::NoEffect))]);
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert_eq!(
            spoken(&fx),
            [Speech::Line(Line::Closing(Closing::NotConfirmed {
                step: Some(StepRef {
                    kind: StepKind::Page,
                    block: 1,
                    index: 0,
                    category: Some(Category::Purchase)
                })
            }))]
        );
        assert_eq!(run.state(), State::Settled(Outcome::NotConfirmed));

        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Done, None)]);
        run.text(2, "Done, order placed!");
        let fx = run.reply(AssistEvent::Failed {
            fault: AssistFault::StepFailed,
        });
        assert!(matches!(
            spoken(&fx)[..],
            [Speech::Line(Line::Closing(Closing::Failed { .. }))]
        ));
    }

    #[test]
    fn answers_stream_and_failures_add_a_typed_line() {
        let mut run = Run::new();
        run.working();
        assert_eq!(
            spoken(&run.text(0, "It is sunny ⟦1⟧. Highs of 24")),
            [says("It is sunny.")]
        );
        let fx = run.reply(AssistEvent::Failed {
            fault: AssistFault::TimedOut,
        });
        assert_eq!(
            spoken(&fx),
            [
                says("Highs of 24."),
                Speech::Line(Line::Closing(Closing::Failed {
                    step: None,
                    reason: None,
                    fault: Some(AssistFault::TimedOut)
                }))
            ]
        );
        assert_eq!(run.state(), State::Settled(Outcome::Failed));
    }

    #[test]
    fn a_confirm_is_announced_once_with_a_typed_line() {
        let mut run = Run::new();
        run.working();
        let fx = run.steps(&[(StepState::Confirm, None)]);
        assert!(matches!(
            spoken(&fx)[..],
            [Speech::Line(Line::Closing(Closing::NeedsYou {
                step: Some(_)
            }))]
        ));
        assert!(spoken(&run.steps(&[(StepState::Confirm, None)])).is_empty());
    }

    #[test]
    fn spoken_replies_off_never_speak_and_change_nothing_else() {
        let script = |spoken_on: bool| {
            let mut run = Run::with(Config {
                spoken: spoken_on,
                ..Config::default()
            });
            let mut all = run.say("open the cheapest flight to Lisbon", 500);
            all.extend(run.at(10, Event::Admitted { reply: 7 }));
            all.extend(run.text(0, "Looking now. "));
            all.extend(run.steps(&[(StepState::Confirm, None)]));
            all.extend(run.say("yes", 500));
            all.extend(run.steps(&[(StepState::Done, None)]));
            all.extend(run.text(2, "Booked."));
            all.extend(run.reply(AssistEvent::Done {
                cost: None,
                end: None,
            }));
            let state = run.state();
            all.extend(run.at(10_000, Event::Tick));
            (all, state)
        };
        let (on, on_state) = script(true);
        let (off, off_state) = script(false);
        assert!(!spoken(&on).is_empty());
        assert!(spoken(&off).is_empty());
        let rest = |fx: Vec<Effect>| -> Vec<Effect> {
            fx.into_iter()
                .filter(|e| {
                    !matches!(
                        e,
                        Effect::Speak { .. } | Effect::FlushSpeech | Effect::CancelSpeech
                    )
                })
                .collect()
        };
        let (on, off) = (rest(on), rest(off));
        assert_eq!(on.len(), off.len());
        for (a, b) in on.iter().zip(&off) {
            match (a, b) {
                (Effect::StartUtterance { .. }, Effect::StartUtterance { .. })
                | (Effect::CommitUtterance { .. }, Effect::CommitUtterance { .. }) => {}
                _ => assert_eq!(a, b),
            }
        }
        assert_eq!(on_state, off_state);
    }

    #[test]
    fn escape_discards_listening_then_stops_work_then_flushes() {
        let mut run = Run::new();
        run.working();
        run.text(0, "One. Two. Three");
        let id = run.press();
        let fx = run.at(100, Event::Escape);
        assert!(has(&fx, |e| *e == Effect::CancelUtterance { id: id.clone() }));
        assert!(!has(&fx, |e| matches!(e, Effect::StopAssistant { .. })));
        let fx = run.at(10, Event::Escape);
        assert_eq!(fx, [Effect::StopAssistant { reply: Some(7) }]);
        let fx = run.reply(AssistEvent::Cancelled);
        assert!(spoken(&fx).is_empty());
        assert_eq!(run.state(), State::Settled(Outcome::Cancelled));

        let mut run = Run::new();
        run.working();
        run.text(0, "Sunny all day.\n");
        run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert!(run.session.view().speaking);
        assert_eq!(
            run.at(10, Event::Escape),
            [Effect::FlushSpeech, Effect::CancelSpeech]
        );
        assert!(!run.session.view().speaking);
    }

    fn is_ack(effect: &Effect) -> bool {
        matches!(
            effect,
            Effect::Speak {
                speech: Speech::Line(Line::Ack(_)),
                ..
            }
        )
    }

    fn partial(run: &mut Run, id: &str, text: &str) {
        run.at(
            50,
            Event::Partial {
                id: id.into(),
                text: text.into(),
            },
        );
    }

    #[test]
    fn a_qualifying_interim_is_acknowledged_at_release_and_only_once() {
        let mut run = Run::new();
        let id = run.press();
        partial(&mut run, &id, "open the cheapest flight to Lisbon");
        let fx = run.at(400, Event::KeyUp);
        let commit = position(&fx, &Effect::CommitUtterance { id: id.clone() });
        let ack = fx.iter().position(is_ack).unwrap();
        assert!(commit < ack);
        let fx = run.at(
            500,
            Event::Final {
                id,
                text: "open the cheapest flight to Lisbon".into(),
            },
        );
        assert!(has(&fx, |e| matches!(e, Effect::Ask { .. })));
        assert!(!has(&fx, is_ack));

        let mut run = Run::new();
        let id = run.press();
        partial(&mut run, &id, "open it");
        assert!(!has(&run.at(400, Event::KeyUp), is_ack));
        let fx = run.at(
            500,
            Event::Final {
                id,
                text: "open it and book the cheapest seat".into(),
            },
        );
        assert!(has(&fx, is_ack));

        let mut run = Run::new();
        let id = run.press();
        partial(&mut run, &id, "find a quiet hotel near the station");
        run.at(100, Event::KeyUp);
        assert_eq!(run.state(), State::Listening(Press::Latched));
        let fx = run.at(2_000, Event::KeyDown);
        assert!(
            position(&fx, &Effect::CommitUtterance { id }) < fx.iter().position(is_ack).unwrap()
        );
    }

    #[test]
    fn an_early_acknowledgment_is_cut_when_the_words_come_back_empty_or_fail() {
        for result in [None, Some(Fault::RateLimited)] {
            let mut run = Run::new();
            let id = run.press();
            partial(&mut run, &id, "open the cheapest flight to Lisbon");
            assert!(has(&run.at(400, Event::KeyUp), is_ack));
            let event = match result {
                None => Event::Final {
                    id,
                    text: String::new(),
                },
                Some(fault) => Event::ListenFailed { id, fault },
            };
            let fx = run.at(500, event);
            assert!(has(&fx, |e| *e == Effect::FlushSpeech));
            assert!(has(&fx, |e| *e == Effect::CancelSpeech));
            assert!(spoken(&fx).is_empty());
            assert!(!run.session.view().speaking);
            assert_eq!(run.state(), State::Idle);
        }

        let mut run = Run::new();
        let id = run.press();
        partial(&mut run, &id, "open the cheapest flight to Lisbon");
        let fx = run.at(400, Event::KeyUp);
        let Some(Effect::Speak { id: speech, .. }) = fx.iter().find(|e| is_ack(e)) else {
            panic!("{fx:?}");
        };
        run.at(100, Event::SpeechEnded { id: speech.clone() });
        let fx = run.at(
            400,
            Event::Final {
                id,
                text: String::new(),
            },
        );
        assert!(!has(&fx, |e| *e == Effect::FlushSpeech));

        let mut run = Run::new();
        let id = run.press();
        partial(&mut run, &id, "open the cheapest flight to Lisbon");
        run.at(400, Event::KeyUp);
        assert!(has(&run.at(10, Event::Escape), |e| *e == Effect::FlushSpeech));
    }

    #[test]
    fn a_held_press_over_a_pending_confirm_waits_for_the_final() {
        let mut run = Run::new();
        run.confirming();
        let id = run.press();
        partial(&mut run, &id, "yes and also add the insurance please");
        assert!(!has(&run.at(500, Event::KeyUp), is_ack));
        let fx = run.at(
            400,
            Event::Final {
                id,
                text: "yes".into(),
            },
        );
        assert!(has(&fx, |e| matches!(e, Effect::Decide { .. })));
        assert!(spoken(&fx).is_empty());

        let mut run = Run::new();
        run.confirming();
        let id = run.press();
        partial(&mut run, &id, "what does the insurance cover exactly");
        run.at(100, Event::KeyUp);
        assert!(
            has(&run.at(2_000, Event::KeyDown), is_ack),
            "latched cannot decide"
        );

        let mut run = Run::with(Config {
            spoken: false,
            ..Config::default()
        });
        let id = run.press();
        partial(&mut run, &id, "open the cheapest flight to Lisbon");
        assert!(spoken(&run.at(400, Event::KeyUp)).is_empty());
    }

    #[test]
    fn nothing_happens_after_dispose() {
        let mut run = Run::new();
        run.press();
        let fx = run.at(10, Event::Dispose);
        assert!(has(&fx, |e| *e == Effect::CloseMic));
        assert!(has(&fx, |e| matches!(e, Effect::CancelUtterance { .. })));
        for event in [Event::KeyDown, Event::Tick, Event::Start, Event::MicOpened] {
            assert!(run.session.on(event, 1_000_000).is_empty());
        }
    }

    #[test]
    fn a_settled_reply_fades_once_speech_ends_and_escape_dismisses_it() {
        let mut run = Run::new();
        run.working();
        let fx = run.text(0, "Sunny all day.\n");
        let Some(Effect::Speak { id, .. }) = fx.iter().find(|e| matches!(e, Effect::Speak { .. }))
        else {
            panic!("{fx:?}");
        };
        run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert_eq!(run.session.fading_since, None, "speech is still playing");
        run.at(10_000, Event::Tick);
        assert_eq!(run.state(), State::Settled(Outcome::Answered));
        for speech in run.session.speeches.clone() {
            if speech != *id {
                run.at(0, Event::SpeechEnded { id: speech });
            }
        }
        assert_eq!(run.session.fading_since, None);
        run.at(10, Event::SpeechEnded { id: id.clone() });
        assert_eq!(run.session.deadline(), Some(run.now + 4_000));
        run.at(3_999, Event::Tick);
        assert_eq!(run.state(), State::Settled(Outcome::Answered));
        run.at(1, Event::Tick);
        assert_eq!(run.state(), State::Idle);
        assert_eq!(run.session.view().outcome, None);
        assert_eq!(run.session.deadline(), None);

        let mut run = Run::new();
        run.working();
        run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert_eq!(run.state(), State::Settled(Outcome::Answered));
        let fx = run.at(10, Event::Escape);
        assert!(!has(&fx, |e| matches!(e, Effect::StopAssistant { .. })));
        assert_eq!(run.state(), State::Idle);
    }

    #[test]
    fn passing_faults_fade_and_ones_that_need_the_person_stay() {
        let mut run = Run::new();
        run.say("open the cheapest flight to Lisbon", 500);
        run.at(10, Event::Refused { fault: Fault::Busy });
        assert_eq!(run.session.view().fault, Some(Fault::Busy));
        run.at(4_000, Event::Tick);
        assert_eq!(run.session.view().fault, None);

        for fault in [
            Fault::PlanRequired,
            Fault::MicrophoneDenied,
            Fault::NoMicrophone,
            Fault::OutOfCredit,
            Fault::DailyLimit,
        ] {
            let mut run = Run::new();
            let id = run.press();
            run.at(400, Event::KeyUp);
            run.at(100, Event::ListenFailed { id, fault });
            run.at(60_000, Event::Tick);
            assert_eq!(run.session.view().fault, Some(fault));
            assert_eq!(run.session.deadline(), None);
            run.at(10, Event::Escape);
            assert_eq!(run.session.view().fault, None);
        }

        let mut run = Run::new();
        run.at(0, Event::KeyDown);
        run.at(
            10,
            Event::MicFailed {
                fault: Fault::MicrophoneDenied,
            },
        );
        run.at(10, Event::KeyUp);
        run.at(10, Event::KeyDown);
        assert_eq!(
            run.session.view().fault,
            None,
            "the next press dismisses it"
        );
    }

    #[test]
    fn verified_steps_with_nothing_said_after_them_end_with_done() {
        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Running, None)]);
        run.steps(&[(StepState::Done, None)]);
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert_eq!(spoken(&fx), [Speech::Line(Line::Closing(Closing::Done))]);
        assert_eq!(run.state(), State::Settled(Outcome::Acted));

        let mut run = Run::new();
        run.working();
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert!(
            spoken(&fx).is_empty(),
            "an answer with no words says nothing"
        );

        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Done, None), (StepState::Running, None)]);
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert!(!spoken(&fx).contains(&Speech::Line(Line::Closing(Closing::Done))));
        assert_eq!(run.state(), State::Settled(Outcome::Partial));
    }

    #[test]
    fn a_steer_before_admission_keeps_steering_and_early_events_are_kept() {
        let mut run = Run::new();
        run.say("open the cheapest flight to Lisbon", 500);
        run.text(0, "Looking now. ");
        let fx = run.say("and book the window seat please", 500);
        assert!(has(&fx, |e| matches!(e, Effect::Steer { reply: None, .. })));
        run.at(10, Event::Admitted { reply: 7 });
        assert!(run.session.reply.as_ref().unwrap().steering);
        run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert!(matches!(run.state(), State::Working(_)));
        assert!(spoken(&run.at(
            10,
            Event::Reply {
                reply: 8,
                event: AssistEvent::Text {
                    block: 0,
                    delta: "Booked the window seat.\n".into(),
                },
            },
        ))
        .is_empty());
        let fx = run.at(10, Event::Admitted { reply: 8 });
        assert_eq!(spoken(&fx), [says("Booked the window seat.")]);
        assert!(!run.session.reply.as_ref().unwrap().steering);
        run.at(
            10,
            Event::Reply {
                reply: 8,
                event: AssistEvent::Done {
                    cost: None,
                    end: None,
                },
            },
        );
        assert_eq!(run.state(), State::Settled(Outcome::Answered));
        assert!(run.session.early.is_empty());

        let mut run = Run::new();
        run.say("open the cheapest flight to Lisbon", 500);
        run.say("and book the window seat please", 500);
        let fx = run.at(10, Event::Refused { fault: Fault::Busy });
        assert!(!has(&fx, |e| *e == Effect::Earcon(Earcon::Error)));
        assert!(matches!(run.state(), State::Working(_)));
        let fx = run.at(10, Event::Refused { fault: Fault::Busy });
        assert!(has(&fx, |e| *e == Effect::Earcon(Earcon::Error)));
        assert_eq!(run.state(), State::Idle);
        assert!(run.at(10, Event::Admitted { reply: 9 }).is_empty());
    }

    #[test]
    fn early_events_are_bounded_and_dropped_for_other_replies() {
        let mut run = Run::new();
        run.say("open the cheapest flight to Lisbon", 500);
        for _ in 0..(MAX_EARLY_EVENTS + 40) {
            run.at(
                1,
                Event::Reply {
                    reply: 3,
                    event: AssistEvent::Phase { phase: None },
                },
            );
        }
        assert_eq!(run.session.early.len(), MAX_EARLY_EVENTS);
        run.at(10, Event::Admitted { reply: 7 });
        assert!(run.session.early.is_empty());
        run.reply(AssistEvent::Text {
            block: 0,
            delta: "x".into(),
        });
        assert!(
            run.session.early.is_empty(),
            "nothing is held once admitted"
        );
    }

    #[test]
    fn a_reply_that_asks_speaks_its_question_and_waits_warm_for_the_answer() {
        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Done, None)]);
        run.text(2, "Which date works for you? ");
        run.at(30_000, Event::Tick);
        assert!(!run.session.view().mic, "the first tail cooled");
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: Some(End::Asked),
        });
        assert_eq!(spoken(&fx), [says("Which date works for you?")]);
        assert!(has(&fx, |e| *e == Effect::OpenMic));
        assert_eq!(run.state(), State::Settled(Outcome::NeedsYou));
        run.at(10, Event::MicOpened);
        run.at(19_000, Event::Tick);
        assert!(run.session.view().mic);
        assert!(has(&run.at(1_000, Event::Tick), |e| *e == Effect::CloseMic));
    }

    #[test]
    fn a_redirected_reply_ends_silently() {
        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Done, None)]);
        run.text(2, "Booking it now. ");
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: Some(End::Redirected),
        });
        assert!(spoken(&fx).is_empty());
        assert_eq!(run.session.view().outcome, None);
        assert_eq!(run.state(), State::Idle);

        let mut run = Run::new();
        run.working();
        run.say("actually make it the aisle seat", 500);
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: Some(End::Redirected),
        });
        assert!(spoken(&fx).is_empty());
        assert!(
            matches!(run.state(), State::Working(_)),
            "waits for the steer"
        );
    }

    #[test]
    fn an_unconfirmed_step_is_never_success() {
        let mut run = Run::new();
        run.working();
        run.steps(&[(StepState::Failed, Some(Reason::Unconfirmed))]);
        let fx = run.reply(AssistEvent::Done {
            cost: None,
            end: None,
        });
        assert!(matches!(
            spoken(&fx)[..],
            [Speech::Line(Line::Closing(Closing::NotConfirmed {
                step: Some(_)
            }))]
        ));
        assert_eq!(run.state(), State::Settled(Outcome::NotConfirmed));
    }

    struct Driver {
        run: Run,
        open: Vec<String>,
        pending: Vec<String>,
        held: Vec<String>,
        speeches: Vec<String>,
        mic: bool,
        may_speak: bool,
        acked: Vec<String>,
        reply: u32,
        outstanding: u32,
    }

    impl Driver {
        fn event(&mut self, code: u8, pick: u8) -> Event {
            let text = [
                "",
                "yes",
                "no",
                "open the cheapest flight please",
                "and book it",
            ];
            let first = |list: &Vec<String>| list.first().cloned().unwrap_or_else(|| "none".into());
            let states = [
                StepState::Running,
                StepState::Confirm,
                StepState::Done,
                StepState::Failed,
            ];
            let reply = |event| Event::Reply {
                reply: self.reply,
                event,
            };
            match code % 26 {
                0 | 1 => Event::KeyDown,
                2 | 3 => Event::KeyUp,
                4 => Event::Escape,
                5 => Event::Start,
                6 => Event::Stop,
                7 => Event::MicOpened,
                8 => Event::MicFailed {
                    fault: Fault::NoMicrophone,
                },
                9 => Event::LinkReady,
                10 => Event::LinkLost {
                    fault: pick.is_multiple_of(2).then_some(Fault::Offline),
                },
                11 => Event::Partial {
                    id: first(&self.open),
                    text: ["open the", "open the cheapest flight please", "yes", ""]
                        [usize::from(pick) % 4]
                        .into(),
                },
                12 | 13 => Event::Final {
                    id: self
                        .pending
                        .get(usize::from(pick) % self.pending.len().max(1))
                        .cloned()
                        .unwrap_or_default(),
                    text: text[usize::from(pick) % text.len()].into(),
                },
                14 => Event::ListenFailed {
                    id: first(&self.pending),
                    fault: Fault::RateLimited,
                },
                15 => {
                    self.reply += u32::from(pick % 2);
                    Event::Admitted { reply: self.reply }
                }
                16 => Event::Refused { fault: Fault::Busy },
                17 => reply(AssistEvent::Text {
                    block: u16::from(pick % 3),
                    delta: [
                        "Done. ",
                        "Clicked it",
                        ", then more words here and there. ",
                        "\n",
                    ][usize::from(pick) % 4]
                        .into(),
                }),
                18 => reply(AssistEvent::Block {
                    block: 1 + u16::from(pick % 2),
                    value: Block::Steps {
                        steps: vec![Step {
                            index: 0,
                            label: "Click \"Buy\"".into(),
                            category: Some(Category::Purchase),
                            state: states[usize::from(pick) % states.len()],
                            reason: None,
                        }],
                    },
                }),
                19 => reply(
                    [
                        AssistEvent::Done {
                            cost: None,
                            end: None,
                        },
                        AssistEvent::Failed {
                            fault: AssistFault::StepFailed,
                        },
                        AssistEvent::Cancelled,
                        AssistEvent::State {
                            state: ReplyState::Paused,
                        },
                        AssistEvent::State {
                            state: ReplyState::Waiting,
                        },
                        AssistEvent::Invalidated,
                        AssistEvent::Done {
                            cost: None,
                            end: Some(End::Asked),
                        },
                        AssistEvent::Done {
                            cost: None,
                            end: Some(End::Redirected),
                        },
                    ][usize::from(pick) % 8]
                        .clone(),
                ),
                20 => Event::SpeechEnded {
                    id: first(&self.speeches),
                },
                21 => Event::SpeechFailed {
                    id: first(&self.speeches),
                    fault: None,
                },
                22 => Event::Decided { reply: self.reply },
                23 => Event::Settings {
                    spoken: !pick.is_multiple_of(4),
                    language: Language::En,
                },
                24 => Event::SpeechStarted {
                    id: first(&self.speeches),
                },
                _ => Event::Tick,
            }
        }

        fn apply(&mut self, event: Event, after: u64) -> Result<(), TestCaseError> {
            if matches!(event, Event::Final { ref text, .. } if !text.trim().is_empty())
                || matches!(event, Event::Admitted { .. } | Event::Settings { .. })
                || matches!(event, Event::Partial { ref text, .. } if text.split(' ').count() >= 4)
            {
                self.may_speak = true;
            }
            let mut committed = None;
            let mut acks = 0;
            let key_up = event == Event::KeyUp;
            let candidates = self.pending.clone();
            if matches!(event, Event::MicFailed { .. }) {
                self.mic = false;
            }
            if matches!(event, Event::Admitted { .. } | Event::Refused { .. }) {
                self.outstanding = self.outstanding.saturating_sub(1);
            }
            if let Event::SpeechEnded { id } | Event::SpeechFailed { id, .. } = &event {
                self.speeches.retain(|s| s != id);
            }
            self.run.now += after;
            let fx = self.run.session.on(event, self.run.now);
            for effect in &fx {
                match effect {
                    Effect::StartUtterance { id, .. } => {
                        prop_assert!(self.open.is_empty(), "two open utterances");
                        self.open.push(id.clone());
                    }
                    Effect::CommitUtterance { id } => {
                        committed = Some(id.clone());
                        self.open.retain(|o| o != id);
                        self.pending.push(id.clone());
                        if key_up {
                            self.held.push(id.clone());
                        }
                    }
                    Effect::CancelUtterance { id } => {
                        self.open.retain(|o| o != id);
                        self.pending.retain(|p| p != id);
                    }
                    Effect::OpenMic => {
                        prop_assert!(!self.mic, "opened twice");
                        self.mic = true;
                    }
                    Effect::CloseMic => self.mic = false,
                    Effect::Speak { id, speech } => {
                        if matches!(speech, Speech::Line(Line::Ack(_))) {
                            acks += 1;
                            if let Some(utterance) = committed.take() {
                                self.acked.push(utterance);
                            } else {
                                let fresh = candidates
                                    .iter()
                                    .filter(|c| !self.acked.contains(c))
                                    .count();
                                prop_assert!(
                                    acks <= fresh,
                                    "two acknowledgments for one utterance"
                                );
                            }
                        }
                        prop_assert!(self.may_speak, "spoke after a flush without new words");
                        prop_assert!(self.open.is_empty(), "spoke over the person");
                        self.speeches.push(id.clone());
                    }
                    Effect::FlushSpeech => {
                        self.may_speak = false;
                        self.speeches.clear();
                    }
                    Effect::Ask { .. } | Effect::Steer { .. } => self.outstanding += 1,
                    Effect::Decide { .. } => {
                        prop_assert!(
                            candidates.iter().any(|c| self.held.contains(c)),
                            "decided without a held utterance"
                        );
                    }
                    _ => {}
                }
            }
            let resolved: Vec<String> = self
                .pending
                .iter()
                .filter(|p| !self.run.session.finals.iter().any(|f| f.id == **p))
                .cloned()
                .collect();
            self.pending.retain(|p| !resolved.contains(p));
            if self.run.session.open.is_none() {
                self.open.clear();
            }
            let view = self.run.session.view();
            let session = &self.run.session;
            match &session.reply {
                Some(reply) => {
                    prop_assert_eq!(u32::from(reply.awaiting), self.outstanding);
                    if reply.awaiting == 0 {
                        prop_assert!(session.early.is_empty(), "held events with nothing awaited");
                    }
                }
                None => {
                    self.outstanding = 0;
                    prop_assert!(session.early.is_empty());
                }
            }
            prop_assert!(session.early.len() <= MAX_EARLY_EVENTS);
            let done = Speech::Line(Line::Closing(Closing::Done));
            if spoken(&fx).contains(&done) {
                prop_assert_eq!(
                    view.outcome,
                    Some(Outcome::Acted),
                    "done without verified steps"
                );
            }
            let fades = matches!(view.state, State::Settled(_))
                || (view.state == State::Idle && view.fault.is_some_and(|f| !f.persists()));
            if fades && !view.speaking {
                prop_assert!(session.deadline().is_some(), "a settled view never fades");
            }
            prop_assert!(self.open.len() <= 1);
            prop_assert_eq!(view.mic, self.mic && self.run.session.mic == Mic::Open);
            if view.state == State::Idle {
                prop_assert!(!view.speaking, "speaking while idle");
            }
            if !self.mic {
                prop_assert!(!view.mic);
            }
            Ok(())
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]
        #[test]
        fn invariants_hold_under_any_sequence(
            steps in proptest::collection::vec((0u8..26, 0u8..255, 0u64..3_000), 1..120),
        ) {
            let mut driver = Driver {
                run: Run::new(),
                open: Vec::new(),
                pending: Vec::new(),
                held: Vec::new(),
                speeches: Vec::new(),
                mic: false,
                may_speak: false,
                acked: Vec::new(),
                reply: 7,
                outstanding: 0,
            };
            for (code, pick, after) in steps {
                let event = driver.event(code, pick);
                driver.apply(event, after)?;
            }
            driver.apply(Event::Escape, 0)?;
            driver.apply(Event::Escape, 0)?;
            driver.apply(Event::KeyUp, 0)?;
            driver.apply(Event::MicFailed { fault: Fault::NoMicrophone }, 0)?;
            driver.apply(Event::MicOpened, 0)?;
            driver.apply(Event::Tick, 20_000)?;
            for id in driver.speeches.clone() {
                driver.apply(Event::SpeechEnded { id }, 0)?;
            }
            driver.apply(Event::Tick, 5_000)?;
            let view = driver.run.session.view();
            prop_assert!(!matches!(view.state, State::Settled(_)), "settled outlived its display");
            prop_assert!(view.fault.is_none_or(Fault::persists));
            prop_assert!(!view.mic && !driver.mic, "microphone left open");
            prop_assert!(driver.run.session.open.is_none() && driver.run.session.finals.is_empty());
            prop_assert!(driver.run.session.on(Event::Dispose, driver.run.now).iter().all(|e| !matches!(e, Effect::Speak { .. })), "spoke on dispose");
            for code in 0..26 {
                let event = driver.event(code, 3);
                prop_assert!(driver.run.session.on(event, driver.run.now).is_empty(), "effect after dispose");
            }
        }
    }
}
