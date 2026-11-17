//! macOS: AVAudioEngine's input tap, and SFSpeechRecognizer held to
//! on-device recognition. SpeechAnalyzer is macOS 26 and Swift-only; the
//! app supports macOS 14, where this recognizer is the on-device engine.

use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::AnyThread;
use objc2_avf_audio::{
    AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioEngine, AVAudioFormat,
    AVAudioInputNode, AVAudioPCMBuffer, AVAudioTime,
};
use objc2_foundation::{NSBundle, NSError, NSLocale, NSOperationQueue, NSString};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognitionTaskHint, SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};

use crate::failure::{domain_tag, Failure};
use crate::stitch::Stitch;
use crate::{pick_locale, Core, Ending, Fault, Listener, Outcome, Permission, Recognition};

/// How long a permission prompt may stay up before the take gives up.
const PROMPT_WAIT: Duration = Duration::from_secs(300);
/// The recognizer's last words usually follow `endAudio` within a second.
const FINAL_WAIT: Duration = Duration::from_secs(4);
const TAP_FRAMES: u32 = 1_024;
/// Silence after the last word: the recognizer holds back a word that ends
/// exactly where the audio does.
const TRAILING_SILENCE_S: f64 = 0.6;

/// Whether this process answers for its own privacy prompts. An unbundled
/// development binary is attributed to the app that launched it (a
/// terminal), and asking on that app's behalf terminates the process when it
/// lacks the usage string; there the launching app's existing answer stands.
fn asks_for_itself(key: &str) -> bool {
    let bundle = NSBundle::mainBundle();
    bundle.bundlePath().to_string().ends_with(".app")
        && bundle
            .objectForInfoDictionaryKey(&NSString::from_str(key))
            .is_some()
}

pub(crate) fn microphone_permission() -> Permission {
    // SAFETY: a class property read with no preconditions.
    let permission = unsafe { AVAudioApplication::sharedInstance().recordPermission() };
    match permission {
        AVAudioApplicationRecordPermission::Granted => Permission::Granted,
        AVAudioApplicationRecordPermission::Undetermined => Permission::Undetermined,
        _ => Permission::Denied,
    }
}

pub(crate) fn ask_microphone() -> Result<(), Fault> {
    match microphone_permission() {
        Permission::Granted => return Ok(()),
        Permission::Denied => return Err(Fault::MicrophoneDenied),
        Permission::Undetermined => {}
    }
    if !asks_for_itself("NSMicrophoneUsageDescription") {
        return Ok(());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let done = RcBlock::new(move |granted: Bool| {
        let _ = sender.try_send(granted.as_bool());
    });
    // SAFETY: the block is copied by the callee and may run on any thread;
    // it only sends on a channel.
    unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&done) };
    match receiver.recv_timeout(PROMPT_WAIT) {
        Ok(true) => Ok(()),
        _ => Err(Fault::MicrophoneDenied),
    }
}

pub(crate) fn recognition_permission() -> Permission {
    // SAFETY: a class property read with no preconditions.
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    match status {
        SFSpeechRecognizerAuthorizationStatus::Authorized => Permission::Granted,
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => Permission::Undetermined,
        _ => Permission::Denied,
    }
}

fn ask_recognition() -> Result<(), Fault> {
    match recognition_permission() {
        Permission::Granted => return Ok(()),
        Permission::Denied => return Err(Fault::RecognitionDenied),
        Permission::Undetermined => {}
    }
    if !asks_for_itself("NSSpeechRecognitionUsageDescription") {
        return Ok(());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let done = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
        let _ = sender.try_send(status == SFSpeechRecognizerAuthorizationStatus::Authorized);
    });
    // SAFETY: as above; the usage string is present, so this cannot abort.
    unsafe { SFSpeechRecognizer::requestAuthorization(&done) };
    match receiver.recv_timeout(PROMPT_WAIT) {
        Ok(true) => Ok(()),
        _ => Err(Fault::RecognitionDenied),
    }
}

pub(crate) fn preferred_languages() -> Vec<String> {
    NSLocale::preferredLanguages()
        .iter()
        .map(|tag| tag.to_string())
        .take(8)
        .collect()
}

fn supported_locales() -> Vec<String> {
    // SAFETY: a class method with no preconditions.
    let locales = unsafe { SFSpeechRecognizer::supportedLocales() };
    locales
        .iter()
        .map(|locale| locale.localeIdentifier().to_string())
        .collect()
}

fn recognizer(locale: &str) -> Option<Retained<SFSpeechRecognizer>> {
    let locale = NSLocale::localeWithLocaleIdentifier(&NSString::from_str(locale));
    // SAFETY: a plain initializer; nil (an unknown locale) becomes None.
    unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale) }
}

fn on_device(recognizer: &SFSpeechRecognizer) -> bool {
    // SAFETY: property reads on a live recognizer.
    unsafe { recognizer.supportsOnDeviceRecognition() && recognizer.isAvailable() }
}

pub(crate) fn on_device_locale(language: Option<&str>) -> Option<String> {
    let supported = supported_locales();
    let region = NSLocale::currentLocale()
        .regionCode()
        .map(|code| code.to_string());
    let wanted = match language {
        Some(language) => vec![language.to_owned()],
        None => preferred_languages(),
    };
    wanted.iter().find_map(|tag| {
        let locale = pick_locale(&supported, tag, region.as_deref())?;
        recognizer(&locale)
            .is_some_and(|recognizer| on_device(&recognizer))
            .then_some(locale)
    })
}

/// The recognizer's words, shared with its result handler.
#[derive(Default)]
struct Words {
    state: Mutex<WordsState>,
    changed: Condvar,
}

#[derive(Default)]
struct WordsState {
    text: String,
    /// Every utterance of the take, not only the one since the last pause.
    stitch: Stitch,
    done: bool,
    /// Results seen, and whether the last of them was final.
    results: u32,
    last: bool,
    /// Audio has ended; an error from here on is expected.
    finishing: bool,
    failure: Option<(Failure, i64)>,
}

/// What the recognizer produced by the time a take was settled.
struct Heard {
    text: String,
    results: u32,
    last: bool,
    failure: Option<(Failure, i64)>,
}

impl Words {
    /// Folds a result into the take and returns the take's words so far.
    fn update(&self, text: &str, span: Option<(f64, f64)>, last: bool) -> String {
        let whole = match self.state.lock() {
            Ok(mut state) => {
                let whole = state.stitch.take(text, span);
                state.text.clone_from(&whole);
                state.results += 1;
                state.last = last;
                state.done |= last;
                whole
            }
            Err(_) => text.to_owned(),
        };
        self.changed.notify_all();
        whole
    }

    /// Records the task's error and ends the wait. True when the take should
    /// be ended now: the recognizer failed while the person was still talking.
    fn fail(&self, failure: Failure, code: i64) -> bool {
        let notify = match self.state.lock() {
            Ok(mut state) => {
                state.done = true;
                let first = state.failure.is_none();
                if first {
                    state.failure = Some((failure, code));
                }
                first
                    && !state.finishing
                    && matches!(failure, Failure::DictationOff | Failure::Other)
            }
            Err(_) => false,
        };
        self.changed.notify_all();
        notify
    }

    fn finishing(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.finishing = true;
        }
    }

    /// A failure already on record, if it ends the take.
    fn failed(&self) -> Option<Fault> {
        let state = self.state.lock().ok()?;
        match state.failure? {
            (Failure::DictationOff, _) => Some(Fault::DictationOff),
            (Failure::Other, _) => Some(Fault::Recognizer),
            _ => None,
        }
    }

    fn wait(&self, timeout: Duration) -> Heard {
        let deadline = Instant::now() + timeout;
        let Ok(mut state) = self.state.lock() else {
            return Heard {
                text: String::new(),
                results: 0,
                last: false,
                failure: None,
            };
        };
        while !state.done {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match self.changed.wait_timeout(state, left) {
                Ok((next, _)) => state = next,
                Err(_) => {
                    return Heard {
                        text: String::new(),
                        results: 0,
                        last: false,
                        failure: None,
                    }
                }
            }
        }
        Heard {
            text: state.text.trim().to_owned(),
            results: state.results,
            last: state.last,
            failure: state.failure,
        }
    }
}

struct OnDevice {
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
    words: Arc<Words>,
    _recognizer: Retained<SFSpeechRecognizer>,
    _queue: Retained<NSOperationQueue>,
    _handler: RcBlock<dyn Fn(*mut SFSpeechRecognitionResult, *mut NSError)>,
}

impl OnDevice {
    fn start(locale: &str, listener: Arc<dyn Listener>) -> Result<Self, Fault> {
        let recognizer = recognizer(locale).ok_or(Fault::LanguageUnavailable)?;
        if !on_device(&recognizer) {
            return Err(Fault::LanguageUnavailable);
        }
        // Results arrive on this serial queue, never the main thread, so a
        // take works the same with or without a running main run loop.
        let queue = NSOperationQueue::new();
        queue.setMaxConcurrentOperationCount(1);
        // SAFETY: configuring a live request and recognizer before use.
        let request = unsafe {
            recognizer.setQueue(&queue);
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setShouldReportPartialResults(true);
            request.setRequiresOnDeviceRecognition(true);
            request.setAddsPunctuation(true);
            request.setTaskHint(SFSpeechRecognitionTaskHint::Dictation);
            request
        };
        let words = Arc::new(Words::default());
        let sink = words.clone();
        let handler = RcBlock::new(
            move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                // SAFETY: Speech passes null or a result valid for this call.
                if let Some(result) = unsafe { result.as_ref() } {
                    // SAFETY: reads on the live result.
                    let (text, span, last) = unsafe {
                        let best = result.bestTranscription();
                        let segments = best.segments();
                        let span = segments.firstObject().zip(segments.lastObject()).map(
                            |(first, end)| (first.timestamp(), end.timestamp() + end.duration()),
                        );
                        (best.formattedString().to_string(), span, result.isFinal())
                    };
                    let whole = sink.update(&text, span, last);
                    if !last {
                        listener.partial(&whole);
                    }
                }
                // An error ends the task: no speech, a cancel, or a failure.
                // The words so far stand either way.
                // SAFETY: Speech passes null or an error valid for this call.
                if let Some(error) = unsafe { error.as_ref() } {
                    let domain = error.domain().to_string();
                    let code = error.code() as i64;
                    let failure = Failure::of(&domain, code);
                    listener.note(&format!(
                        "recognizer error kind={} domain={} code={code}",
                        failure.name(),
                        domain_tag(&domain)
                    ));
                    if sink.fail(failure, code) {
                        listener.ended(Ending::Failed);
                    }
                }
            },
        );
        // SAFETY: the request is fully configured; the handler block is
        // copied by Speech and kept alive here until the task ends.
        let task =
            unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler) };
        Ok(Self {
            request,
            task,
            words,
            _recognizer: recognizer,
            _queue: queue,
            _handler: handler,
        })
    }
}

impl OnDevice {
    /// Ends the audio after a moment of silence in `format`.
    fn end(&self, format: &AVAudioFormat) {
        // SAFETY: a fresh buffer of a live format, zeroed within its
        // capacity before it is appended to a live request.
        unsafe {
            let frames = (format.sampleRate() * TRAILING_SILENCE_S) as u32;
            if let Some(buffer) = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                format,
                frames,
            ) {
                buffer.setFrameLength(frames);
                let data = buffer.floatChannelData();
                if !data.is_null() {
                    let stride = buffer.stride().max(1);
                    let planes = if stride == 1 {
                        format.channelCount() as usize
                    } else {
                        1
                    };
                    for plane in 0..planes {
                        std::ptr::write_bytes(
                            (*data.add(plane)).as_ptr(),
                            0,
                            frames as usize * stride,
                        );
                    }
                    self.request.appendAudioPCMBuffer(&buffer);
                }
            }
            self.request.endAudio();
        }
    }
}

pub(crate) struct Capture {
    engine: Retained<AVAudioEngine>,
    format: Retained<AVAudioFormat>,
    input: Retained<AVAudioInputNode>,
    core: Arc<Core>,
    listener: Arc<dyn Listener>,
    on_device: Option<OnDevice>,
    _tap: RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)>,
    running: bool,
}

// SAFETY: every Objective-C object here is used by one owner at a time (the
// take is moved, never shared); AVAudioEngine and the Speech request and task
// may be driven from any thread. Audio-thread access goes only through the tap
// block and `Core`, which is itself Sync.
unsafe impl Send for Capture {}

/// Mono samples from one tap buffer, whatever its channel layout.
///
/// # Safety
/// `buffer` must be a live float PCM buffer.
unsafe fn mono(buffer: &AVAudioPCMBuffer, out: &mut Vec<f32>) {
    out.clear();
    // SAFETY: the caller guarantees a live float buffer.
    let (data, frames, stride, channels) = unsafe {
        (
            buffer.floatChannelData(),
            buffer.frameLength() as usize,
            buffer.stride().max(1),
            buffer.format().channelCount() as usize,
        )
    };
    if data.is_null() || frames == 0 || channels == 0 {
        return;
    }
    let scale = 1.0 / channels as f32;
    out.reserve(frames);
    for frame in 0..frames {
        let mut sum = 0.0;
        for channel in 0..channels {
            // SAFETY: deinterleaved buffers hold one pointer per channel, each
            // with `frames` samples; interleaved ones hold one pointer with
            // `frames * stride` samples. Both are inside the buffer.
            sum += unsafe {
                if stride == 1 {
                    *(*data.add(channel)).as_ptr().add(frame)
                } else {
                    *(*data).as_ptr().add(frame * stride + channel)
                }
            };
        }
        out.push(sum * scale);
    }
}

impl Capture {
    pub(crate) fn start(
        recognition: Recognition,
        pause: bool,
        listener: Arc<dyn Listener>,
    ) -> Result<Self, Fault> {
        ask_microphone()?;
        let on_device = match &recognition {
            Recognition::OnDevice { locale } => {
                ask_recognition()?;
                Some(OnDevice::start(locale, listener.clone())?)
            }
            Recognition::Upload => None,
        };
        // SAFETY: a fresh engine and its input node, used only by this take.
        let (engine, input, format) = unsafe {
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let format = input.outputFormatForBus(0);
            (engine, input, format)
        };
        // SAFETY: property reads on a live format.
        let (rate, channels) = unsafe { (format.sampleRate(), format.channelCount()) };
        if !rate.is_finite() || rate <= 0.0 || channels == 0 {
            if let Some(on_device) = &on_device {
                // SAFETY: cancelling a live task.
                unsafe { on_device.task.cancel() };
            }
            return Err(Fault::NoMicrophone);
        }
        let core = Arc::new(Core::new(
            rate,
            recognition == Recognition::Upload,
            pause,
            listener.clone(),
        ));
        let tap_core = core.clone();
        let request = on_device
            .as_ref()
            .map(|on_device| on_device.request.clone());
        let tap = RcBlock::new(
            move |buffer: NonNull<AVAudioPCMBuffer>, _time: NonNull<AVAudioTime>| {
                // SAFETY: AVAudioEngine passes a buffer valid for this call.
                let buffer = unsafe { buffer.as_ref() };
                let mut samples = Vec::new();
                // SAFETY: the tap was installed with the node's float format.
                unsafe { mono(buffer, &mut samples) };
                tap_core.push(&samples);
                if let Some(request) = &request {
                    // SAFETY: appending a live buffer to a live request; Speech
                    // copies what it keeps.
                    unsafe { request.appendAudioPCMBuffer(buffer) };
                }
            },
        );
        let mut capture = Self {
            engine,
            format: format.clone(),
            input,
            core,
            listener,
            on_device,
            _tap: tap,
            running: false,
        };
        // SAFETY: the tap block outlives the tap: it is removed in `release`
        // before `_tap` drops.
        let started = unsafe {
            capture.input.installTapOnBus_bufferSize_format_block(
                0,
                TAP_FRAMES,
                Some(&format),
                RcBlock::as_ptr(&capture._tap),
            );
            capture.running = true;
            capture.engine.prepare();
            capture.engine.startAndReturnError()
        };
        if started.is_err() {
            capture.listener.note("engine did not start");
            capture.abandon();
            return Err(Fault::NoMicrophone);
        }
        capture.listener.note(&format!(
            "capture started rate={rate} channels={channels} on_device={}",
            capture.on_device.is_some()
        ));
        // The recognizer refuses a take within milliseconds when it cannot
        // run at all; there is no point listening to it.
        if let Some(fault) = capture
            .on_device
            .as_ref()
            .and_then(|on_device| on_device.words.failed())
        {
            capture.abandon();
            return Err(fault);
        }
        Ok(capture)
    }

    /// Releases the microphone and cancels the recognizer.
    fn abandon(&mut self) {
        self.release();
        if let Some(on_device) = self.on_device.take() {
            // SAFETY: cancelling a live task.
            unsafe { on_device.task.cancel() };
        }
    }

    /// Stops the engine and removes the tap: the microphone is free and the
    /// in-use indicator goes out.
    fn release(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        // SAFETY: stopping a live engine and removing the tap installed on bus 0.
        unsafe {
            self.engine.stop();
            self.input.removeTapOnBus(0);
        }
    }

    pub(crate) fn finish(mut self) -> Result<Outcome, Fault> {
        self.release();
        let (take, heard) = self.core.stop();
        let Some(on_device) = self.on_device.take() else {
            return Ok(Core::upload(take, heard));
        };
        on_device.words.finishing();
        on_device.end(&self.format);
        let heard_words = on_device.words.wait(FINAL_WAIT);
        // SAFETY: a finished task ignores cancel; a slow one stops here.
        unsafe { on_device.task.cancel() };
        let Heard {
            text,
            results,
            last,
            failure,
        } = heard_words;
        self.listener.note(&format!(
            "finish heard={heard} results={results} final={last} chars={} failure={}",
            text.chars().count(),
            failure.map_or("none", |(failure, _)| failure.name())
        ));
        if !text.is_empty() {
            return Ok(Outcome::Text(text));
        }
        match failure {
            Some((Failure::DictationOff, _)) => Err(Fault::DictationOff),
            Some((Failure::Other, _)) => Err(Fault::Recognizer),
            _ => Ok(Outcome::Nothing),
        }
    }

    pub(crate) fn cancel(mut self) {
        self.release();
        self.core.stop();
        if let Some(on_device) = self.on_device.take() {
            // SAFETY: cancelling a live task discards its words.
            unsafe { on_device.task.cancel() };
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.release();
        if let Some(on_device) = self.on_device.take() {
            // SAFETY: cancelling a live task.
            unsafe { on_device.task.cancel() };
        }
    }
}

/// Feeds an audio file through the same on-device path as the microphone,
/// for automated checks without a microphone permission.
#[cfg(feature = "file-probe")]
pub fn transcribe_file(path: &std::path::Path, locale: &str) -> Result<String, Fault> {
    use objc2_avf_audio::AVAudioFile;
    use objc2_foundation::NSURL;

    struct Quiet;
    impl Listener for Quiet {
        fn levels(&self, _levels: &[f32]) {}
        fn ended(&self, _ending: crate::Ending) {}
    }

    ask_recognition()?;
    let on_device = OnDevice::start(locale, Arc::new(Quiet))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // SAFETY: reading a local file into buffers of its own processing format.
    unsafe {
        let file = AVAudioFile::initForReading_error(AVAudioFile::alloc(), &url)
            .map_err(|_| Fault::NoMicrophone)?;
        let format = file.processingFormat();
        loop {
            let Some(buffer) = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &format,
                4_096,
            ) else {
                break;
            };
            if file.readIntoBuffer_error(&buffer).is_err() || buffer.frameLength() == 0 {
                break;
            }
            on_device.request.appendAudioPCMBuffer(&buffer);
        }
        on_device.end(&format);
    }
    let Heard { text, failure, .. } = on_device.words.wait(Duration::from_secs(30));
    // SAFETY: cancelling a live task.
    unsafe { on_device.task.cancel() };
    match failure {
        Some((Failure::DictationOff, _)) if text.is_empty() => Err(Fault::DictationOff),
        _ => Ok(text),
    }
}
