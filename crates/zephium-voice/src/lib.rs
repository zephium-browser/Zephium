//! Dictation capture for the Work bar: the microphone, a live level, and
//! either Apple's on-device recognizer or a speech take held in memory for a
//! cloud transcription. Audio is never written to disk or logged, and the
//! device is released the moment a take finishes or is cancelled.

#[cfg(any(target_os = "macos", test))]
mod failure;
mod language;
mod meter;
mod pcm;
#[cfg(any(target_os = "macos", test))]
mod stitch;

#[cfg(target_os = "macos")]
mod apple;
#[cfg(target_os = "windows")]
mod wasapi;

use std::sync::{Arc, Mutex};

#[cfg(all(feature = "file-probe", target_os = "macos"))]
pub use apple::transcribe_file;
pub use language::pick_locale;
pub use meter::{Ending, MAX_MS};
pub use pcm::{wav, SPEECH_RATE};

/// Why a take could not start or finish. Closed; no system text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// The person (or a policy) refused the microphone.
    MicrophoneDenied,
    /// The person (or a policy) refused speech recognition.
    RecognitionDenied,
    /// No input device, or it would not start.
    NoMicrophone,
    /// The on-device recognizer cannot serve this language here.
    LanguageUnavailable,
    /// Siri and Dictation are off in System Settings, so the on-device
    /// recognizer refuses every take.
    DictationOff,
    /// The on-device recognizer failed.
    Recognizer,
    /// Not on this platform or in this build.
    Unsupported,
}

/// What a running take reports. Called from audio and recognizer threads, so
/// implementations hand work off rather than block.
pub trait Listener: Send + Sync + 'static {
    /// Input levels, 0–1, one per 40 ms of audio, oldest first. The device
    /// may deliver a tenth of a second at once, so several can arrive together.
    fn levels(&self, levels: &[f32]);
    /// The words so far, when the recognizer streams them.
    fn partial(&self, _text: &str) {}
    /// Capture ended on its own; the owner should finish the take.
    fn ended(&self, ending: Ending);
    /// A closed fact about the take, for the diagnostic log: no audio, no
    /// words, no system text.
    fn note(&self, _fact: &str) {}
}

/// Who turns the speech into words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Recognition {
    /// Apple's recognizer on this Mac, in a locale from [`on_device_locale`].
    OnDevice { locale: String },
    /// Keep the audio for an upload.
    Upload,
}

/// A finished take.
#[derive(Debug)]
pub enum Outcome {
    Text(String),
    /// A PCM16 mono WAV, in memory only.
    Audio {
        wav: Vec<u8>,
        duration_ms: u64,
    },
    /// No speech was heard.
    Nothing,
}

/// Microphone access as the system reports it, without asking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Permission {
    Granted,
    Undetermined,
    Denied,
}

/// The microphone permission, without prompting.
pub fn microphone_permission() -> Permission {
    #[cfg(target_os = "macos")]
    return apple::microphone_permission();
    #[cfg(not(target_os = "macos"))]
    Permission::Granted
}

/// The on-device speech recognition permission, without prompting.
pub fn recognition_permission() -> Permission {
    #[cfg(target_os = "macos")]
    return apple::recognition_permission();
    #[cfg(not(target_os = "macos"))]
    Permission::Denied
}

/// The recognizer locale to use on this device for `language` (a BCP 47 tag,
/// or the system's preferred languages when `None`), if it recognizes
/// on-device.
pub fn on_device_locale(language: Option<&str>) -> Option<String> {
    #[cfg(target_os = "macos")]
    return apple::on_device_locale(language);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = language;
        None
    }
}

/// The person's preferred languages as BCP 47 tags, most preferred first.
pub fn preferred_languages() -> Vec<String> {
    #[cfg(target_os = "macos")]
    return apple::preferred_languages();
    #[cfg(target_os = "windows")]
    return windows_preferred_languages();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    Vec::new()
}

#[cfg(target_os = "windows")]
fn windows_preferred_languages() -> Vec<String> {
    use windows::core::PWSTR;
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let (mut count, mut length) = (0u32, 0u32);
    // SAFETY: a size query with no buffer, then a fill of exactly that size.
    unsafe {
        if GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut length).is_err() {
            return Vec::new();
        }
        let mut buffer = vec![0u16; length as usize];
        if GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut length,
        )
        .is_err()
        {
            return Vec::new();
        }
        buffer
            .split(|unit| *unit == 0)
            .filter(|tag| !tag.is_empty())
            .take(8)
            .map(String::from_utf16_lossy)
            .collect()
    }
}

/// One live take. Dropping it cancels.
pub struct Capture {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    inner: Option<platform::Capture>,
}

#[cfg(target_os = "macos")]
use apple as platform;
#[cfg(target_os = "windows")]
use wasapi as platform;

impl Capture {
    /// Asks for permission on first use, then opens the microphone. Blocks
    /// while a permission prompt is up; call off the async executor.
    /// `pause` ends the take on a natural pause after speech.
    pub fn start(
        recognition: Recognition,
        pause: bool,
        listener: Arc<dyn Listener>,
    ) -> Result<Self, Fault> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        return Ok(Self {
            inner: Some(platform::Capture::start(recognition, pause, listener)?),
        });
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (recognition, pause, listener);
            Err(Fault::Unsupported)
        }
    }

    /// Releases the microphone and returns the words or the audio. With the
    /// on-device recognizer this waits briefly for its final words.
    pub fn finish(self) -> Result<Outcome, Fault> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let mut capture = self;
            match capture.inner.take() {
                Some(inner) => inner.finish(),
                None => Ok(Outcome::Nothing),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        Ok(Outcome::Nothing)
    }

    /// Releases the microphone and discards everything.
    pub fn cancel(self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let mut capture = self;
            if let Some(inner) = capture.inner.take() {
                inner.cancel();
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(inner) = self.inner.take() {
            inner.cancel();
        }
    }
}

/// The platform-neutral half of a take: level, endings and the upload audio.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub(crate) struct Core {
    state: Mutex<CoreState>,
    listener: Arc<dyn Listener>,
}

struct CoreState {
    rate: f64,
    meter: meter::Meter,
    take: Option<pcm::Take>,
    stopped: bool,
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
impl Core {
    pub(crate) fn new(rate: f64, upload: bool, pause: bool, listener: Arc<dyn Listener>) -> Self {
        Self {
            state: Mutex::new(CoreState {
                rate,
                meter: meter::Meter::new(rate, pause),
                take: upload.then(|| pcm::Take::new(rate, MAX_MS)),
                stopped: false,
            }),
            listener,
        }
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn listener(&self) -> &Arc<dyn Listener> {
        &self.listener
    }

    /// One block of mono samples from the device.
    pub(crate) fn push(&self, mono: &[f32]) {
        let mut levels = Vec::new();
        let mut ending = None;
        {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state.stopped {
                return;
            }
            if let Some(take) = state.take.as_mut() {
                take.push(mono);
            }
            let slice = (state.rate / 50.0).max(1.0) as usize;
            for part in mono.chunks(slice) {
                let step = state.meter.push(part);
                levels.extend(step.level);
                ending = ending.or(step.ending);
            }
        }
        if !levels.is_empty() {
            self.listener.levels(&levels);
        }
        if let Some(ending) = ending {
            self.listener.ended(ending);
        }
    }

    /// Stops accepting audio; returns the upload take and whether speech was heard.
    pub(crate) fn stop(&self) -> (Option<pcm::Take>, bool) {
        match self.state.lock() {
            Ok(mut state) => {
                state.stopped = true;
                (state.take.take(), state.meter.heard())
            }
            Err(_) => (None, false),
        }
    }

    /// The upload outcome for a stopped take.
    pub(crate) fn upload(take: Option<pcm::Take>, heard: bool) -> Outcome {
        match take {
            Some(take) if heard && take.duration_ms() > 0 => {
                let duration_ms = take.duration_ms();
                Outcome::Audio {
                    wav: take.into_wav(),
                    duration_ms,
                }
            }
            _ => Outcome::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        levels: Mutex<Vec<f32>>,
        endings: Mutex<Vec<Ending>>,
    }

    impl Listener for Recorder {
        fn levels(&self, levels: &[f32]) {
            self.levels.lock().unwrap().extend_from_slice(levels);
        }
        fn ended(&self, ending: Ending) {
            self.endings.lock().unwrap().push(ending);
        }
    }

    #[test]
    fn a_stopped_core_keeps_its_take_and_ignores_later_audio() {
        let recorder = Arc::new(Recorder::default());
        let core = Core::new(16_000.0, true, false, recorder.clone());
        let speech: Vec<f32> = (0..16_000).map(|n| 0.3 * (n as f32 * 0.07).sin()).collect();
        // A tenth of a second per callback, as AVAudioEngine's tap delivers.
        for block in speech.chunks(1_600) {
            core.push(block);
        }
        let (take, heard) = core.stop();
        core.push(&speech);
        assert!(heard);
        assert_eq!(recorder.levels.lock().unwrap().len(), 25);
        match Core::upload(take, heard) {
            Outcome::Audio { wav, duration_ms } => {
                assert_eq!(duration_ms, 1_000);
                assert_eq!(wav.len(), 44 + 32_000);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn silence_uploads_nothing() {
        let core = Core::new(16_000.0, true, false, Arc::new(Recorder::default()));
        core.push(&[0.0; 16_000]);
        let (take, heard) = core.stop();
        assert!(matches!(Core::upload(take, heard), Outcome::Nothing));
    }

    #[test]
    fn an_on_device_core_keeps_no_audio() {
        let core = Core::new(16_000.0, false, false, Arc::new(Recorder::default()));
        core.push(&[0.3; 16_000]);
        assert!(core.stop().0.is_none());
    }
}
