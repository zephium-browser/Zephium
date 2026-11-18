//! Bounded, single-use Cloud voice requests over one authenticated socket.

#[cfg(test)]
mod tests;
mod wire;

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::{StatusCode, Url};
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{self, protocol::Role, Message};
use tokio_tungstenite::WebSocketStream;
use zephium_core::work::model::WorkModelError;

use super::{LeadCloudAccounting, LeadCloudObserver, LeadCloudRecord, LeadCredential};
use wire::{ClientFrame, ServerFrame};

const PROTOCOL: &str = "zephium.voice.v1";
const TEXT_LIMIT: usize = 64 * 1024;
const BINARY_LIMIT: usize = 256 * 1024;
const AUDIO_FRAME: usize = 960;
const AUDIO_BUDGET: usize = 150 * AUDIO_FRAME;
const QUEUE_CAPACITY: usize = 160;
const EVENT_CAPACITY: usize = 192;
const MAX_SPEECHES: usize = 64;
const MAX_REQUESTS: usize = 4096;

/// The voice endpoint of the account's Cloud root.
#[derive(Clone)]
pub struct VoiceTarget {
    url: Url,
}

impl VoiceTarget {
    /// Builds the endpoint under the account's Cloud root, admitted as every
    /// other Cloud target is: `wss` under an https root, `ws` only under a
    /// loopback http root. The credential belongs to that root.
    pub fn cloud(root: &Url) -> Result<Self, WorkModelError> {
        let local = root.scheme() == "http" && super::is_loopback(root);
        if (root.scheme() != "https" && !local)
            || root.host_str().is_none()
            || !root.username().is_empty()
            || root.password().is_some()
            || root.query().is_some()
            || root.fragment().is_some()
            || root.path() != "/"
        {
            return Err(WorkModelError::BadRequest);
        }
        let mut url = super::join(root, "v1/voice")?;
        url.set_scheme(if local { "ws" } else { "wss" })
            .map_err(|()| WorkModelError::BadRequest)?;
        Ok(Self { url })
    }
}

impl std::fmt::Debug for VoiceTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VoiceTarget(Cloud)")
    }
}

/// The gateway's negotiated voice policy.
#[derive(Clone)]
pub struct VoiceReady {
    /// Pricing policy identity.
    pub policy_revision: Option<String>,
    /// Maximum utterance duration.
    pub listen_max_ms: u32,
    /// Maximum Unicode character count per speech.
    pub speak_max_chars: u32,
    /// Allowed voice identifiers.
    pub voices: Vec<String>,
    /// Default voice identifier.
    pub voice: String,
}

/// Closed failures; gateway messages are never retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceError {
    /// Existing account, admission or network fault.
    Model(WorkModelError),
    /// The request was cancelled.
    Cancelled,
    /// A deadline elapsed.
    Timeout,
    /// The socket or upstream request was interrupted.
    Interrupted,
    /// Speech exceeded policy.
    TooLong,
    /// Capture exceeded policy.
    AudioTooLong,
    /// PCM was invalid.
    InvalidAudio,
}

/// A gateway-directed socket shutdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceClosing {
    /// Gateway shutdown.
    ShuttingDown,
    /// Authentication ended.
    SessionEnded,
    /// Idle socket expiry.
    Idle,
    /// Account policy changed.
    PlanChanged,
}

/// Ordered, private voice results. Debug output redacts every payload.
pub enum VoiceEvent {
    /// Whole interim transcript.
    Partial {
        /// Client request identity.
        id: String,
        /// Private transcript.
        text: String,
    },
    /// Completed transcript, possibly empty.
    Final {
        /// Client request identity.
        id: String,
        /// Private transcript.
        text: String,
    },
    /// Terminal utterance failure.
    ListenFailed {
        /// Client request identity.
        id: String,
        /// Closed failure.
        error: VoiceError,
    },
    /// Subsequent audio belongs to this speech.
    SpeakStart {
        /// Client request identity.
        id: String,
    },
    /// PCM16LE mono, 24 kHz.
    Audio(Vec<u8>),
    /// Terminal successful speech.
    SpeakEnd {
        /// Client request identity.
        id: String,
    },
    /// Terminal speech failure.
    SpeakFailed {
        /// Client request identity.
        id: String,
        /// Closed failure.
        error: VoiceError,
    },
    /// Gateway shutdown notice.
    Closing {
        /// Closed shutdown reason.
        reason: VoiceClosing,
    },
    /// Exactly one terminal event, always last.
    Closed,
}

impl std::fmt::Debug for VoiceEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Partial { .. } => "Partial([redacted])",
            Self::Final { .. } => "Final([redacted])",
            Self::ListenFailed { .. } => "ListenFailed([redacted])",
            Self::SpeakStart { .. } => "SpeakStart([redacted])",
            Self::Audio(_) => "Audio([redacted])",
            Self::SpeakEnd { .. } => "SpeakEnd([redacted])",
            Self::SpeakFailed { .. } => "SpeakFailed([redacted])",
            Self::Closing { .. } => "Closing",
            Self::Closed => "Closed",
        })
    }
}

#[derive(Clone, Copy)]
struct Timings {
    open: Duration,
    final_wait: Duration,
    ping: Duration,
    silence: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            open: Duration::from_secs(10),
            final_wait: Duration::from_secs(10),
            ping: Duration::from_secs(20),
            silence: Duration::from_secs(45),
        }
    }
}

struct Listen {
    id: String,
    deadline: Option<Instant>,
    cancelled: bool,
}

struct Speech {
    id: String,
    started: bool,
}

struct State {
    closed: Option<VoiceError>,
    listen: Option<Listen>,
    speeches: VecDeque<Speech>,
    seen: HashSet<String>,
    audio_bytes: usize,
}

struct Shared {
    state: Mutex<State>,
    stop: watch::Sender<bool>,
    changed: Notify,
    ready: VoiceReady,
    timings: Timings,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn close(&self, error: VoiceError) {
        self.close_locked(&mut self.state(), error);
    }

    fn close_locked(&self, state: &mut State, error: VoiceError) {
        if state.closed.is_none() {
            state.closed = Some(error);
            self.stop.send_replace(true);
        }
    }
}

struct Outgoing {
    message: Option<Message>,
    audio_bytes: usize,
}

/// Nonblocking sender for a single socket. Dropping it closes the link.
pub struct VoiceLink {
    shared: Arc<Shared>,
    outgoing: mpsc::Sender<Outgoing>,
}

impl std::fmt::Debug for VoiceLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VoiceLink([redacted])")
    }
}

impl Drop for VoiceLink {
    fn drop(&mut self) {
        self.close();
    }
}

impl VoiceLink {
    /// Upgrades once and waits for policy, without retries or replay.
    pub async fn open(
        target: &VoiceTarget,
        credential: &dyn LeadCredential,
        observer: Option<LeadCloudObserver>,
    ) -> Result<(Self, VoiceReady, mpsc::Receiver<VoiceEvent>), WorkModelError> {
        Self::open_with_timings(target, credential, observer, Timings::default()).await
    }

    async fn open_with_timings(
        target: &VoiceTarget,
        credential: &dyn LeadCredential,
        observer: Option<LeadCloudObserver>,
        timings: Timings,
    ) -> Result<(Self, VoiceReady, mpsc::Receiver<VoiceEvent>), WorkModelError> {
        let (socket, ready) = tokio::time::timeout(timings.open, upgrade(target, credential))
            .await
            .map_err(|_| WorkModelError::Network)??;
        let (outgoing, commands) = mpsc::channel(QUEUE_CAPACITY);
        let (events, receiver) = mpsc::channel(EVENT_CAPACITY);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                closed: None,
                listen: None,
                speeches: VecDeque::new(),
                seen: HashSet::new(),
                audio_bytes: 0,
            }),
            stop: watch::channel(false).0,
            changed: Notify::new(),
            ready: ready.clone(),
            timings,
        });
        let (sink, stream) = socket.split();
        tokio::spawn(writer(sink, commands, shared.clone()));
        tokio::spawn(reader(
            stream,
            outgoing.clone(),
            events,
            shared.clone(),
            observer,
        ));
        Ok((Self { shared, outgoing }, ready, receiver))
    }

    fn enqueue(&self, state: &mut State, message: Message, audio_bytes: usize) -> bool {
        if self
            .outgoing
            .try_send(Outgoing {
                message: Some(message),
                audio_bytes,
            })
            .is_err()
        {
            self.shared
                .close_locked(state, VoiceError::Model(WorkModelError::Overloaded));
            return false;
        }
        state.audio_bytes += audio_bytes;
        true
    }

    fn command(&self, frame: &ClientFrame<'_>, apply: impl FnOnce(&mut State) -> bool) -> bool {
        let mut state = self.shared.state();
        if state.closed.is_some() {
            return false;
        }
        if !apply(&mut state) {
            self.shared
                .close_locked(&mut state, VoiceError::Model(WorkModelError::BadRequest));
            return false;
        }
        let Ok(text) = serde_json::to_string(frame) else {
            self.shared
                .close_locked(&mut state, VoiceError::Model(WorkModelError::Protocol));
            return false;
        };
        let open = self.enqueue(&mut state, Message::Text(text.into()), 0);
        self.shared.changed.notify_one();
        open
    }

    /// Starts one UUIDv4 utterance, with an optional ISO 639-1 hint.
    pub fn listen_start(&self, id: &str, language: Option<&str>) -> bool {
        self.command(&ClientFrame::ListenStart { id, language }, |state| {
            if !valid_id(id)
                || language
                    .is_some_and(|s| s.len() != 2 || !s.bytes().all(|b| b.is_ascii_lowercase()))
                || state.listen.is_some()
                || state.seen.len() >= MAX_REQUESTS
                || !state.seen.insert(id.to_owned())
            {
                return false;
            }
            state.listen = Some(Listen {
                id: id.to_owned(),
                deadline: None,
                cancelled: false,
            });
            true
        })
    }

    /// Enqueues whole 20 ms capture frames, bounded to three seconds including in-flight audio.
    pub fn audio(&self, pcm: &[u8]) -> bool {
        let mut state = self.shared.state();
        if state.closed.is_some() {
            return false;
        }
        if pcm.is_empty() || !pcm.len().is_multiple_of(AUDIO_FRAME) {
            self.shared
                .close_locked(&mut state, VoiceError::InvalidAudio);
            return false;
        }
        if state
            .listen
            .as_ref()
            .is_none_or(|listen| listen.deadline.is_some() || listen.cancelled)
        {
            return true;
        }
        if pcm.len() > AUDIO_BUDGET.saturating_sub(state.audio_bytes) {
            self.shared
                .close_locked(&mut state, VoiceError::Model(WorkModelError::Overloaded));
            return false;
        }
        self.enqueue(&mut state, Message::Binary(pcm.to_vec().into()), pcm.len())
    }

    /// Commits once and starts the final deadline: two seconds past the
    /// gateway's own eight, so its per-request failure arrives first.
    pub fn listen_commit(&self, id: &str) -> bool {
        self.command(&ClientFrame::ListenCommit { id }, |state| {
            let Some(listen) = state
                .listen
                .as_mut()
                .filter(|l| l.id == id && l.deadline.is_none() && !l.cancelled)
            else {
                return false;
            };
            listen.deadline = Some(Instant::now() + self.shared.timings.final_wait);
            true
        })
    }

    /// Cancels capture, retaining the request until its accounting answer.
    pub fn listen_cancel(&self, id: &str) -> bool {
        self.command(&ClientFrame::ListenCancel { id }, |state| {
            let Some(listen) = state.listen.as_mut().filter(|l| l.id == id && !l.cancelled) else {
                return false;
            };
            listen.cancelled = true;
            listen.deadline = Some(Instant::now() + self.shared.timings.final_wait);
            true
        })
    }

    /// Queues speech in gateway order, with a policy-approved voice.
    pub fn speak(&self, id: &str, text: &str, voice: Option<&str>) -> bool {
        self.command(&ClientFrame::Speak { id, text, voice }, |state| {
            if !valid_id(id)
                || text.chars().count() > self.shared.ready.speak_max_chars as usize
                || voice
                    .is_some_and(|v| !self.shared.ready.voices.iter().any(|allowed| allowed == v))
                || state.speeches.len() >= MAX_SPEECHES
                || state.seen.len() >= MAX_REQUESTS
                || !state.seen.insert(id.to_owned())
            {
                return false;
            }
            state.speeches.push_back(Speech {
                id: id.to_owned(),
                started: false,
            });
            true
        })
    }

    /// Cancels one outstanding speech or all outstanding speeches.
    pub fn speak_cancel(&self, id: Option<&str>) -> bool {
        self.command(&ClientFrame::SpeakCancel { id }, |state| {
            id.is_none_or(|id| state.speeches.iter().any(|s| s.id == id))
        })
    }

    /// Stops both socket tasks and settles every outstanding request.
    pub fn close(&self) {
        self.shared.close(VoiceError::Interrupted);
    }
}

fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        && b[14] == b'4'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
}

type Socket = WebSocketStream<reqwest::Upgraded>;

async fn upgrade(
    target: &VoiceTarget,
    credential: &dyn LeadCredential,
) -> Result<(Socket, VoiceReady), WorkModelError> {
    let local = target.url.scheme() == "ws";
    let mut url = target.url.clone();
    url.set_scheme(if local { "http" } else { "https" })
        .map_err(|()| WorkModelError::BadRequest)?;
    let client = super::http_builder(local)
        .http1_only()
        .build()
        .map_err(|_| WorkModelError::Network)?;
    let secret = credential.secret().await?;
    let key = tungstenite::handshake::client::generate_key();
    let response = client
        .get(url)
        .version(reqwest::Version::HTTP_11)
        .header("authorization", secret.header("Bearer ")?)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", &key)
        .header("sec-websocket-protocol", PROTOCOL)
        .send()
        .await
        .map_err(|_| WorkModelError::Network)?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        let status = response.status();
        let headers = response.headers().clone();
        let body = super::bounded_body(response).await;
        let (error, _) = super::refused(status, &headers, &body, true);
        if error == WorkModelError::Unauthorized {
            credential.rejected();
        }
        return Err(error);
    }
    let headers = response.headers();
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    if response.version() != reqwest::Version::HTTP_11
        || header("sec-websocket-protocol") != Some(PROTOCOL)
        || header("sec-websocket-accept")
            != Some(tungstenite::handshake::derive_accept_key(key.as_bytes()).as_str())
        || !header("upgrade").is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
        || !header("connection").is_some_and(|v| {
            v.split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
        })
        || headers.contains_key("sec-websocket-extensions")
    {
        return Err(WorkModelError::Protocol);
    }
    let upgraded = response
        .upgrade()
        .await
        .map_err(|_| WorkModelError::Network)?;
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(BINARY_LIMIT))
        .max_frame_size(Some(BINARY_LIMIT))
        .write_buffer_size(0)
        .max_write_buffer_size(BINARY_LIMIT + 1024);
    let mut socket = WebSocketStream::from_raw_socket(upgraded, Role::Client, Some(config)).await;
    let Some(Ok(Message::Text(text))) = socket.next().await else {
        return Err(WorkModelError::Protocol);
    };
    let ServerFrame::Ready {
        policy_revision,
        listen,
        speak,
    } = wire::parse(&text).map_err(|_| WorkModelError::Protocol)?
    else {
        return Err(WorkModelError::Protocol);
    };
    let ready = VoiceReady {
        policy_revision,
        listen_max_ms: listen.max_ms,
        speak_max_chars: speak.max_chars,
        voices: speak.voices,
        voice: speak.voice,
    };
    wire::validate_ready(&ready).map_err(|_| WorkModelError::Protocol)?;
    Ok((socket, ready))
}

async fn writer(
    mut sink: futures_util::stream::SplitSink<Socket, Message>,
    mut commands: mpsc::Receiver<Outgoing>,
    shared: Arc<Shared>,
) {
    let mut stop = shared.stop.subscribe();
    let mut last = Instant::now();
    loop {
        if shared.state().closed.is_some() {
            break;
        }
        let command = tokio::select! {
            biased;
            _ = stop.changed() => break,
            command = commands.recv() => match command { Some(c) => c, None => break },
            () = tokio::time::sleep_until(last + shared.timings.ping) => Outgoing { message: Some(Message::Text("{\"type\":\"ping\"}".into())), audio_bytes: 0 },
        };
        let send = async {
            match command.message {
                Some(message) => sink.send(message).await,
                None => sink.flush().await,
            }
        };
        let result = tokio::select! {
            biased;
            _ = stop.changed() => break,
            result = send => result,
        };
        shared.state().audio_bytes -= command.audio_bytes;
        if result.is_err() {
            shared.close(VoiceError::Interrupted);
            break;
        }
        last = Instant::now();
    }
    shared.close(VoiceError::Interrupted);
    let code = if shared.state().closed == Some(VoiceError::Model(WorkModelError::Protocol)) {
        tungstenite::protocol::frame::coding::CloseCode::Protocol
    } else {
        tungstenite::protocol::frame::coding::CloseCode::Normal
    };
    let _ = tokio::time::timeout(
        Duration::from_millis(250),
        sink.send(Message::Close(Some(tungstenite::protocol::CloseFrame {
            code,
            reason: "".into(),
        }))),
    )
    .await;
}

async fn reader(
    mut stream: futures_util::stream::SplitStream<Socket>,
    outgoing: mpsc::Sender<Outgoing>,
    events: mpsc::Sender<VoiceEvent>,
    shared: Arc<Shared>,
    observer: Option<LeadCloudObserver>,
) {
    let mut stop = shared.stop.subscribe();
    let mut last = Instant::now();
    let mut pending = None;
    loop {
        if shared.state().closed.is_some() {
            break;
        }
        let deadline = shared
            .state()
            .listen
            .as_ref()
            .and_then(|l| l.deadline)
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            () = events.closed() => { shared.close(VoiceError::Interrupted); break; },
            () = shared.changed.notified() => {},
            () = tokio::time::sleep_until(deadline) => { shared.close(VoiceError::Timeout); break; },
            () = tokio::time::sleep_until(last + shared.timings.silence) => { shared.close(VoiceError::Interrupted); break; },
            message = stream.next() => {
                last = Instant::now();
                let result = match message {
                    Some(Ok(Message::Text(text))) => wire::parse(&text).and_then(|frame| wire::receive(frame, &shared, observer.as_ref())),
                    Some(Ok(Message::Binary(bytes))) => {
                        if bytes.len() > BINARY_LIMIT || !bytes.len().is_multiple_of(2) || !shared.state().speeches.front().is_some_and(|s| s.started) {
                            Err(())
                        } else { Ok(Some(VoiceEvent::Audio(bytes.to_vec()))) }
                    },
                    Some(Ok(Message::Ping(_))) => {
                        if outgoing.try_send(Outgoing { message: None, audio_bytes: 0 }).is_err() { shared.close(VoiceError::Model(WorkModelError::Overloaded)); }
                        Ok(None)
                    },
                    Some(Ok(Message::Pong(_))) => Ok(None),
                    Some(Ok(Message::Close(_))) | None => { shared.close(VoiceError::Interrupted); break; },
                    Some(Err(tungstenite::Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake))) => { shared.close(VoiceError::Interrupted); break; },
                    Some(Err(tungstenite::Error::Capacity(_) | tungstenite::Error::Protocol(_) | tungstenite::Error::Utf8(_) | tungstenite::Error::AttackAttempt)) => Err(()),
                    Some(Err(_)) => { shared.close(VoiceError::Interrupted); break; },
                    Some(Ok(Message::Frame(_))) => Err(()),
                };
                match result {
                    Ok(Some(event)) => if let Err(error) = events.try_send(event) { pending = Some(error.into_inner()); shared.close(VoiceError::Model(WorkModelError::Overloaded)); },
                    Ok(None) => {},
                    Err(()) => shared.close(VoiceError::Model(WorkModelError::Protocol)),
                }
            },
        }
    }
    drop(stream);
    let (listen, speeches, error) = {
        let mut state = shared.state();
        (
            state.listen.take(),
            std::mem::take(&mut state.speeches),
            state.closed.unwrap_or(VoiceError::Interrupted),
        )
    };
    let mut terminal = Vec::with_capacity(MAX_SPEECHES + 2);
    terminal.extend(pending);
    if let Some(listen) = listen {
        wire::record(
            &shared,
            observer.as_ref(),
            &listen.id,
            None,
            LeadCloudAccounting::Unknown,
        );
        terminal.push(VoiceEvent::ListenFailed {
            id: listen.id,
            error,
        });
    }
    for speech in speeches {
        wire::record(
            &shared,
            observer.as_ref(),
            &speech.id,
            None,
            LeadCloudAccounting::Unknown,
        );
        terminal.push(VoiceEvent::SpeakFailed {
            id: speech.id,
            error: if error == VoiceError::Timeout {
                VoiceError::Interrupted
            } else {
                error
            },
        });
    }
    terminal.push(VoiceEvent::Closed);
    for event in terminal {
        if events.send(event).await.is_err() {
            break;
        }
    }
}
