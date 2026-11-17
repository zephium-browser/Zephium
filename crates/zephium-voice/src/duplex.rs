//! The full-duplex voice engine: push-to-talk capture in exact 20 ms frames
//! and low-latency playback that can be silenced at once. Device layers only
//! feed input samples ([`Engine::capture`]) and pull output samples
//! ([`Engine::render`]) in the device's own rate and channel layout.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::meter::Meter;
use crate::playback::Playback;
use crate::resample::Resampler;
use crate::Fault;

/// Capture and playback rate of the voice engine.
pub const VOICE_RATE: u32 = 24_000;
/// One frame: 20 ms of mono PCM16 at [`VOICE_RATE`].
pub const FRAME_SAMPLES: usize = VOICE_RATE as usize / 50;
/// Frames kept while not listening, delivered when listening starts, so a
/// word begun with the key press is not clipped.
const PREROLL_FRAMES: usize = 10;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DuplexConfig {
    /// OS echo cancellation and noise suppression on the microphone.
    pub voice_processing: bool,
}

/// What a running engine reports. Called from audio threads, some of them
/// real-time: implementations hand work off and never block.
pub trait DuplexListener: Send + Sync + 'static {
    /// Exactly [`FRAME_SAMPLES`] of PCM16 mono at [`VOICE_RATE`], in order and
    /// without gaps while listening.
    fn frame(&self, pcm: &[i16]);
    /// Input levels, 0–1, on the same scale and cadence as `Listener::levels`.
    fn input_levels(&self, levels: &[f32]);
    /// What is being played, 0–1, about 25 per second; one 0 when it falls silent.
    fn output_level(&self, level: f32);
    /// Everything enqueued before `mark` has left the speaker.
    fn played(&self, mark: u64);
    /// The device changed and the engine recovered.
    fn route_changed(&self);
    /// The engine could not recover; the owner closes it.
    fn lost(&self, fault: Fault);
    /// A closed diagnostic fact, numbers only.
    fn note(&self, _fact: &str) {}
}

struct Input {
    resampler: Resampler,
    mono: Vec<f32>,
    frame: Vec<i16>,
    preroll: VecDeque<i16>,
    meter: Meter,
    listening: bool,
}

/// The platform-neutral engine shared by the device layers.
pub(crate) struct Engine {
    listener: Arc<dyn DuplexListener>,
    listening: AtomicBool,
    input: Mutex<Input>,
    output: Mutex<Playback>,
}

impl Engine {
    pub(crate) fn new(listener: Arc<dyn DuplexListener>) -> Self {
        let rate = f64::from(VOICE_RATE);
        Self {
            listener,
            listening: AtomicBool::new(false),
            input: Mutex::new(Input {
                resampler: Resampler::new(rate, rate),
                mono: Vec::new(),
                frame: Vec::with_capacity(FRAME_SAMPLES),
                preroll: VecDeque::with_capacity(PREROLL_FRAMES * FRAME_SAMPLES),
                meter: Meter::new(rate, false),
                listening: false,
            }),
            output: Mutex::new(Playback::new(rate, 0)),
        }
    }

    pub(crate) fn listener(&self) -> &Arc<dyn DuplexListener> {
        &self.listener
    }

    /// The input device's rate. A partial frame carries over.
    pub(crate) fn set_input(&self, rate: f64) {
        if let Ok(mut input) = self.input.lock() {
            input.resampler = Resampler::new(rate, f64::from(VOICE_RATE));
        }
    }

    /// The output device's rate and latency in its own frames. Queued audio stays.
    pub(crate) fn set_output(&self, rate: f64, latency: u64) {
        if let Ok(mut output) = self.output.lock() {
            output.set_device(rate, latency);
        }
    }

    /// Input samples as the device delivered them, interleaved.
    pub(crate) fn capture(&self, samples: &[f32], channels: usize) {
        let Ok(mut input) = self.input.lock() else {
            return;
        };
        let channels = channels.max(1);
        let Input {
            resampler,
            mono,
            frame,
            preroll,
            meter,
            listening,
        } = &mut *input;
        let listen = self.listening.load(Ordering::Acquire);
        if listen && !*listening {
            for chunk in preroll.make_contiguous().chunks(FRAME_SAMPLES) {
                self.listener.frame(chunk);
            }
            preroll.clear();
        }
        *listening = listen;
        mono.clear();
        if channels == 1 {
            mono.extend_from_slice(samples);
        } else {
            let scale = 1.0 / channels as f32;
            mono.extend(
                samples
                    .chunks_exact(channels)
                    .map(|group| group.iter().sum::<f32>() * scale),
            );
        }
        resampler.push(mono, |sample| {
            let value = if sample.is_finite() { sample } else { 0.0 };
            frame.push((value.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16);
            if frame.len() < FRAME_SAMPLES {
                return;
            }
            let block: [f32; FRAME_SAMPLES] =
                std::array::from_fn(|n| f32::from(frame[n]) / 32_768.0);
            let level = meter.push(&block).level;
            if listen {
                self.listener.frame(frame);
                if let Some(level) = level {
                    self.listener.input_levels(&[level]);
                }
            } else {
                if preroll.len() >= PREROLL_FRAMES * FRAME_SAMPLES {
                    preroll.drain(..FRAME_SAMPLES);
                }
                preroll.extend(frame.iter());
            }
            frame.clear();
        });
    }

    /// Fills `out` (interleaved, `channels` per frame) with what should play now.
    pub(crate) fn render(&self, out: &mut [f32], channels: usize) {
        let rendered = match self.output.lock() {
            Ok(mut output) => output.render(out, channels),
            Err(_) => {
                out.fill(0.0);
                return;
            }
        };
        if let Some(level) = rendered.level {
            self.listener.output_level(level);
        }
        for mark in rendered.played() {
            self.listener.played(*mark);
        }
    }

    pub(crate) fn listen(&self, on: bool) {
        self.listening.store(on, Ordering::Release);
    }

    pub(crate) fn play(&self, pcm: &[i16]) {
        let accepted = match self.output.lock() {
            Ok(mut output) => output.play(pcm),
            Err(_) => return,
        };
        if accepted < pcm.len() {
            self.listener
                .note(&format!("playback full dropped={}", pcm.len() - accepted));
        }
    }

    pub(crate) fn mark(&self, id: u64) {
        if let Ok(mut output) = self.output.lock() {
            output.mark(id);
        }
    }

    pub(crate) fn flush(&self) {
        if let Ok(mut output) = self.output.lock() {
            output.flush();
        }
    }
}

enum Device {
    #[cfg(target_os = "macos")]
    Apple(#[allow(dead_code)] crate::apple_duplex::Device),
    #[cfg(feature = "file-probe")]
    File(#[allow(dead_code)] crate::wavfile::Feeder),
}

/// A live duplex engine. Dropping it releases the microphone and stops playback.
pub struct Duplex {
    engine: Arc<Engine>,
    _device: Device,
}

impl Duplex {
    /// Asks for the microphone on first use, then opens input and output.
    /// Blocks while a permission prompt is up; call off the async executor.
    pub fn open(config: DuplexConfig, listener: Arc<dyn DuplexListener>) -> Result<Self, Fault> {
        #[cfg(target_os = "macos")]
        {
            let engine = Arc::new(Engine::new(listener));
            let device = crate::apple_duplex::Device::open(config, engine.clone())?;
            Ok(Self {
                engine,
                _device: Device::Apple(device),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (config, listener);
            Err(Fault::Unsupported)
        }
    }

    /// A WAV file plays the microphone in real time, then silence; playback is
    /// paced in real time and discarded.
    #[cfg(feature = "file-probe")]
    pub fn open_file(
        path: &std::path::Path,
        listener: Arc<dyn DuplexListener>,
    ) -> Result<Self, Fault> {
        let engine = Arc::new(Engine::new(listener));
        let feeder = crate::wavfile::Feeder::start(path, engine.clone())?;
        Ok(Self {
            engine,
            _device: Device::File(feeder),
        })
    }

    /// Gates frame and level delivery; the device stays open when off.
    pub fn listen(&self, on: bool) {
        self.engine.listen(on);
    }

    /// Enqueues PCM16 mono at [`VOICE_RATE`]; never waits for the device.
    pub fn play(&self, pcm: &[i16]) {
        self.engine.play(pcm);
    }

    /// Reports `id` through [`DuplexListener::played`] once everything
    /// enqueued so far has been heard. A mark also ends the start buffer.
    pub fn mark(&self, id: u64) {
        self.engine.mark(id);
    }

    /// Drops everything queued, fading to silence within 5 ms of output;
    /// marks not yet played are never reported.
    pub fn flush(&self) {
        self.engine.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        frames: Mutex<Vec<Vec<i16>>>,
        levels: Mutex<Vec<f32>>,
        output: Mutex<Vec<f32>>,
        played: Mutex<Vec<u64>>,
    }

    impl DuplexListener for Recorder {
        fn frame(&self, pcm: &[i16]) {
            self.frames.lock().unwrap().push(pcm.to_vec());
        }
        fn input_levels(&self, levels: &[f32]) {
            self.levels.lock().unwrap().extend_from_slice(levels);
        }
        fn output_level(&self, level: f32) {
            self.output.lock().unwrap().push(level);
        }
        fn played(&self, mark: u64) {
            self.played.lock().unwrap().push(mark);
        }
        fn route_changed(&self) {}
        fn lost(&self, _fault: Fault) {}
    }

    fn ramp(count: usize) -> Vec<f32> {
        (0..count).map(|n| (n % 1_000) as f32 / 2_000.0).collect()
    }

    #[test]
    fn frames_are_exact_gapless_and_ordered_under_ragged_callbacks() {
        let recorder = Arc::new(Recorder::default());
        let engine = Engine::new(recorder.clone());
        engine.listen(true);
        let input = ramp(48_000);
        let mut rest = &input[..];
        for size in [1_usize, 7, 480, 4_800, 13, 1_024, 333].iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let size = (*size).min(rest.len());
            engine.capture(&rest[..size], 1);
            rest = &rest[size..];
        }
        let frames = recorder.frames.lock().unwrap();
        assert!(frames.iter().all(|frame| frame.len() == FRAME_SAMPLES));
        assert_eq!(frames.len(), 100);
        let joined: Vec<i16> = frames.concat();
        let expected: Vec<i16> = input
            .iter()
            .map(|sample| (sample * f32::from(i16::MAX)).round() as i16)
            .take(joined.len())
            .collect();
        assert_eq!(joined, expected);
    }

    #[test]
    fn device_rates_resample_to_whole_frames_with_stereo_downmixed() {
        let recorder = Arc::new(Recorder::default());
        let engine = Engine::new(recorder.clone());
        engine.set_input(48_000.0);
        engine.listen(true);
        let stereo: Vec<f32> = (0..96_000)
            .map(|n| if n % 2 == 0 { 0.2 } else { 0.4 })
            .collect();
        for block in stereo.chunks(1_022) {
            engine.capture(block, 2);
        }
        let frames = recorder.frames.lock().unwrap();
        assert!((48..=50).contains(&frames.len()), "{}", frames.len());
        let last = frames.last().unwrap();
        let expected = (0.3 * f32::from(i16::MAX)).round() as i16;
        assert!(last.iter().all(|sample| sample.abs_diff(expected) <= 2));
        assert_eq!(recorder.levels.lock().unwrap().len(), frames.len() / 2);
    }

    #[test]
    fn a_closed_gate_keeps_only_the_preroll() {
        let recorder = Arc::new(Recorder::default());
        let engine = Engine::new(recorder.clone());
        let input = ramp(24_000);
        engine.capture(&input, 1);
        assert!(recorder.frames.lock().unwrap().is_empty());
        assert!(recorder.levels.lock().unwrap().is_empty());
        engine.listen(true);
        engine.capture(&[], 1);
        let frames = recorder.frames.lock().unwrap().clone();
        assert_eq!(frames.len(), PREROLL_FRAMES);
        let start = 24_000 - PREROLL_FRAMES * FRAME_SAMPLES;
        assert_eq!(
            frames[0][0],
            (input[start] * f32::from(i16::MAX)).round() as i16
        );
        engine.listen(false);
        engine.capture(&input[..4_800], 1);
        assert_eq!(recorder.frames.lock().unwrap().len(), PREROLL_FRAMES);
    }

    #[test]
    fn non_finite_and_loud_input_is_clamped() {
        let recorder = Arc::new(Recorder::default());
        let engine = Engine::new(recorder.clone());
        engine.listen(true);
        let mut input = vec![f32::NAN; FRAME_SAMPLES];
        input[1] = 4.0;
        input[2] = f32::NEG_INFINITY;
        engine.capture(&input, 1);
        let frames = recorder.frames.lock().unwrap();
        assert_eq!(frames[0][..3], [0, i16::MAX, 0]);
    }

    #[test]
    fn playback_reports_levels_and_marks_through_the_listener() {
        let recorder = Arc::new(Recorder::default());
        let engine = Engine::new(recorder.clone());
        engine.set_output(48_000.0, 0);
        engine.play(&vec![8_000; 4_800]);
        engine.mark(3);
        let mut out = vec![0.0; 960];
        for _ in 0..30 {
            engine.render(&mut out, 2);
        }
        assert_eq!(*recorder.played.lock().unwrap(), vec![3]);
        let output = recorder.output.lock().unwrap();
        assert!(output.iter().any(|level| *level > 0.5));
        assert_eq!(*output.last().unwrap(), 0.0);
    }
}
