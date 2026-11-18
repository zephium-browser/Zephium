//! What the voice session works through. Each port is a seam the desktop
//! fills with the device, the gateway socket and the sidebar assistant, and
//! tests fill with fakes.
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use tokio::sync::mpsc;
use zephium_agentic::lead::voice::{VoiceEvent, VoiceLink, VoiceReady};
use zephium_core::work::model::WorkModelError;
use zephium_ipc::page_assist::PageAssistDecisionV1;
use zephium_ipc::voice_session::{VoiceSessionLevelsV1, VoiceSessionViewV1};
use zephium_page_assist::voice::Fault;

pub use crate::page_assist::port::BoxFuture;

/// One capture frame: 20 ms of PCM16 mono at 24 kHz.
pub const FRAME: usize = 480;
/// A capture frame.
pub type Frame = [i16; FRAME];
const INBOX_FRAMES: usize = 64;

/// The microphone and speaker. Calls never block on the device.
pub trait AudioPort: Send + Sync {
    /// Configures the device in standby, nothing capturing; frames, levels
    /// and played marks go to `inbox`. Slow once, and may wait on a
    /// permission prompt, so it runs off the session.
    fn prepare(&self, inbox: Arc<AudioInbox>) -> BoxFuture<'static, Result<(), Fault>>;
    /// Releases the device entirely.
    fn release(&self);
    /// Starts capturing from standby; resolves once frames flow.
    fn start(&self) -> BoxFuture<'static, Result<(), Fault>>;
    /// Stops capturing and stays configured.
    fn standby(&self);
    /// Delivers frames (on) or holds them back (off).
    fn gate(&self, on: bool);
    /// Enqueues PCM16 mono at 24 kHz; works in standby too.
    fn play(&self, pcm: &[i16]);
    /// Reports `mark` once everything enqueued before it has played.
    fn mark(&self, mark: u64);
    /// Silences playback now and drops what is queued.
    fn flush(&self);
}

/// The gateway voice socket, open.
pub trait VoiceSocket: Send + Sync {
    /// Opens one utterance.
    fn listen_start(&self, id: &str, language: Option<&str>) -> bool;
    /// Whole 20 ms PCM16LE frames for the open utterance.
    fn audio(&self, pcm: &[u8]) -> bool;
    /// Finalizes the utterance.
    fn listen_commit(&self, id: &str) -> bool;
    /// Discards the utterance.
    fn listen_cancel(&self, id: &str) -> bool;
    /// Queues one speech.
    fn speak(&self, id: &str, text: &str, voice: Option<&str>) -> bool;
    /// Cancels one speech, or every speech.
    fn speak_cancel(&self, id: Option<&str>) -> bool;
    /// Closes the socket.
    fn close(&self);
}

impl VoiceSocket for VoiceLink {
    fn listen_start(&self, id: &str, language: Option<&str>) -> bool {
        VoiceLink::listen_start(self, id, language)
    }
    fn audio(&self, pcm: &[u8]) -> bool {
        VoiceLink::audio(self, pcm)
    }
    fn listen_commit(&self, id: &str) -> bool {
        VoiceLink::listen_commit(self, id)
    }
    fn listen_cancel(&self, id: &str) -> bool {
        VoiceLink::listen_cancel(self, id)
    }
    fn speak(&self, id: &str, text: &str, voice: Option<&str>) -> bool {
        VoiceLink::speak(self, id, text, voice)
    }
    fn speak_cancel(&self, id: Option<&str>) -> bool {
        VoiceLink::speak_cancel(self, id)
    }
    fn close(&self) {
        VoiceLink::close(self);
    }
}

/// A socket that came up.
pub struct Opened {
    /// The socket.
    pub socket: Arc<dyn VoiceSocket>,
    /// The gateway's policy.
    pub ready: VoiceReady,
    /// Its events, in order; `Closed` is last.
    pub events: mpsc::Receiver<VoiceEvent>,
}

/// Opens the gateway voice socket with the account's credential. Usage
/// records go to the account layer inside the adapter.
pub trait LinkPort: Send + Sync {
    /// One upgrade attempt.
    fn open(&self) -> BoxFuture<'static, Result<Opened, WorkModelError>>;
}

/// A message the assistant admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asked {
    /// The conversation it runs in.
    pub conversation: String,
    /// The reply it runs.
    pub reply: u32,
}

/// The sidebar assistant, in this window's Space and front tab.
pub trait AssistantPort: Send + Sync {
    /// Asks with the person's words.
    /// `spoken` asks for a reply meant to be heard; otherwise it is shown.
    fn ask(&self, text: String, spoken: bool) -> BoxFuture<'static, Result<Asked, Fault>>;
    /// Stops `reply` at its next safe step and continues the conversation
    /// with the new words.
    fn steer(
        &self,
        conversation: String,
        reply: u32,
        text: String,
        spoken: bool,
    ) -> BoxFuture<'static, Result<Asked, Fault>>;
    /// Starts a reply on words that are not final yet and holds it unseen:
    /// nothing of it shows, sounds or takes effect until it is released.
    /// Resolves to its handle.
    fn ask_ahead(&self, text: String, spoken: bool) -> BoxFuture<'static, Result<u64, Fault>>;
    /// Asks with the final words `text` by releasing held reply `held`,
    /// started on exactly these words; it then runs as an ask's would. An
    /// error means nothing was released and the held reply is gone.
    fn release(&self, held: u64, text: String) -> BoxFuture<'static, Result<Asked, Fault>>;
    /// Discards held reply `held` without a trace.
    fn discard(&self, held: u64);
    /// Stops `reply` at its next safe point.
    fn stop(&self, conversation: &str, reply: u32);
    /// Answers the confirm at `block` the way a click does.
    fn decide(
        &self,
        conversation: &str,
        reply: u32,
        block: u16,
        decision: PageAssistDecisionV1,
    ) -> bool;
}

/// Where the interface's projection goes.
pub trait Projection: Send + Sync {
    /// The view changed.
    fn view(&self, view: &VoiceSessionViewV1);
    /// Levels, while the microphone listens or speech plays.
    fn levels(&self, levels: VoiceSessionLevelsV1);
}

/// Monotonic time.
pub trait Clock: Send + Sync {
    /// Milliseconds on a monotonic clock.
    fn now_ms(&self) -> u64;
}

/// Hands out fresh UUIDv4 strings.
pub type Ids = Arc<dyn Fn() -> String + Send + Sync>;

/// What happened on the device, for the session.
pub(crate) enum DeviceEvent {
    Played(u64),
    RouteChanged,
    Lost(Fault),
}

/// The audio threads' side of the session: frames go through a bounded
/// lock-free queue, levels through atomics; rare device events through a
/// channel. Nothing here blocks.
pub struct AudioInbox {
    frames: ArrayQueue<Frame>,
    input: AtomicU32,
    output: AtomicU32,
    dropped: AtomicU64,
    events: mpsc::UnboundedSender<super::Input>,
}

impl AudioInbox {
    pub(crate) fn new(events: mpsc::UnboundedSender<super::Input>) -> Self {
        Self {
            frames: ArrayQueue::new(INBOX_FRAMES),
            input: AtomicU32::new(0),
            output: AtomicU32::new(0),
            dropped: AtomicU64::new(0),
            events,
        }
    }

    /// One 20 ms capture frame; anything else is dropped.
    pub fn frame(&self, pcm: &[i16]) {
        let Ok(frame) = <Frame>::try_from(pcm) else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if self.frames.push(frame).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The latest microphone level, 0–1.
    pub fn input_level(&self, level: f32) {
        self.input
            .store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The latest speaker level, 0–1.
    pub fn output_level(&self, level: f32) {
        self.output
            .store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Everything enqueued before `mark` has played.
    pub fn played(&self, mark: u64) {
        let _ = self
            .events
            .send(super::Input::Device(DeviceEvent::Played(mark)));
    }

    /// The device changed and recovered.
    pub fn route_changed(&self) {
        let _ = self
            .events
            .send(super::Input::Device(DeviceEvent::RouteChanged));
    }

    /// The device could not recover.
    pub fn lost(&self, fault: Fault) {
        let _ = self
            .events
            .send(super::Input::Device(DeviceEvent::Lost(fault)));
    }

    pub(crate) fn take_frame(&self) -> Option<Frame> {
        self.frames.pop()
    }

    pub(crate) fn clear(&self) {
        while self.frames.pop().is_some() {}
        self.input.store(0, Ordering::Relaxed);
        self.output.store(0, Ordering::Relaxed);
    }

    pub(crate) fn levels(&self) -> (f32, f32) {
        (
            f32::from_bits(self.input.load(Ordering::Relaxed)),
            f32::from_bits(self.output.load(Ordering::Relaxed)),
        )
    }
}
