//! macOS: one AVAudioEngine with the input node (optionally voice processed)
//! into a sink node, and a source node pulling the playback queue into the
//! main mixer, so the echo canceller hears what we play. A configuration
//! change rebuilds the whole graph; the queue lives in [`Engine`] and survives.

use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::AnyThread;
use objc2_avf_audio::{
    AVAudioEngine, AVAudioEngineConfigurationChangeNotification, AVAudioFormat, AVAudioSinkNode,
    AVAudioSourceNode, AVAudioVoiceProcessingOtherAudioDuckingConfiguration,
    AVAudioVoiceProcessingOtherAudioDuckingLevel,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};

use crate::apple::ask_microphone;
use crate::duplex::Engine;
use crate::{DuplexConfig, Fault};

/// Configuration changes arrive in bursts; rebuild once they settle.
const SETTLE: Duration = Duration::from_millis(150);
const RETRY: Duration = Duration::from_millis(300);
/// A started engine that delivers no input for this long is rebuilt.
const STALL: Duration = Duration::from_millis(500);
/// More rebuilds than this within `CHURN_WINDOW` means the device is not settling.
const CHURN: usize = 5;
const CHURN_WINDOW: Duration = Duration::from_secs(10);

type SinkBlock = RcBlock<dyn Fn(NonNull<AudioTimeStamp>, u32, NonNull<AudioBufferList>) -> i32>;
type SourceBlock =
    RcBlock<dyn Fn(NonNull<Bool>, NonNull<AudioTimeStamp>, u32, NonNull<AudioBufferList>) -> i32>;

#[derive(Default)]
struct Signal {
    raised: Mutex<bool>,
    wake: Condvar,
}

impl Signal {
    fn raise(&self) {
        if let Ok(mut raised) = self.raised.lock() {
            *raised = true;
        }
        self.wake.notify_all();
    }
}

struct Graph {
    engine: Retained<AVAudioEngine>,
    observer: Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
    _sink: Retained<AVAudioSinkNode>,
    _source: Retained<AVAudioSourceNode>,
    _sink_block: SinkBlock,
    _source_block: SourceBlock,
    _observer_block: RcBlock<dyn Fn(NonNull<NSNotification>)>,
}

// SAFETY: the graph is only touched under its owner's mutex, one thread at a
// time; AVAudioEngine may be configured from any thread. The render blocks
// reach shared state only through `Engine`, which is Sync.
unsafe impl Send for Graph {}

/// The buffers of a list the engine passed for this render cycle.
///
/// # Safety
/// `list` must be valid for this call, with `mNumberBuffers` buffers.
unsafe fn buffers<'a>(list: NonNull<AudioBufferList>) -> &'a mut [AudioBuffer] {
    // SAFETY: the caller guarantees the list and its trailing buffer array.
    unsafe {
        let list = list.as_ptr();
        let count = (*list).mNumberBuffers as usize;
        std::slice::from_raw_parts_mut((&raw mut (*list).mBuffers).cast::<AudioBuffer>(), count)
    }
}

/// The float samples of one buffer, at most `frames` per channel.
///
/// # Safety
/// `buffer` must describe live float samples.
unsafe fn samples<'a>(buffer: &AudioBuffer, frames: usize) -> &'a mut [f32] {
    let channels = buffer.mNumberChannels.max(1) as usize;
    let len = (buffer.mDataByteSize as usize / 4).min(frames * channels);
    if buffer.mData.is_null() || len == 0 {
        return &mut [];
    }
    // SAFETY: the caller guarantees `mData` holds `mDataByteSize` bytes of floats.
    unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), len) }
}

/// A mono source node at `format` that pulls the playback queue.
///
/// # Safety
/// `format` must be a standard float format.
unsafe fn source_node(
    core: &Arc<Engine>,
    format: &AVAudioFormat,
) -> (Retained<AVAudioSourceNode>, SourceBlock) {
    let render = core.clone();
    let block: SourceBlock = RcBlock::new(
        move |_silence: NonNull<Bool>,
              _time: NonNull<AudioTimeStamp>,
              frames: u32,
              list: NonNull<AudioBufferList>| {
            let frames = frames as usize;
            // SAFETY: the engine passes a live float list for this cycle.
            let buffers = unsafe { buffers(list) };
            let Some((first, rest)) = buffers.split_first_mut() else {
                return 0;
            };
            // SAFETY: as above.
            let data = unsafe { samples(first, frames) };
            render.render(data, first.mNumberChannels.max(1) as usize);
            for buffer in rest {
                // SAFETY: as above.
                let other = unsafe { samples(buffer, frames) };
                let len = other.len().min(data.len());
                other[..len].copy_from_slice(&data[..len]);
            }
            0
        },
    );
    // SAFETY: the node copies the block; it is also kept beside the node.
    let node = unsafe {
        AVAudioSourceNode::initWithFormat_renderBlock(
            AVAudioSourceNode::alloc(),
            format,
            RcBlock::as_ptr(&block),
        )
    };
    (node, block)
}

impl Graph {
    fn build(
        config: DuplexConfig,
        core: &Arc<Engine>,
        signal: &Arc<Signal>,
    ) -> Result<Self, Fault> {
        let listener = core.listener().clone();
        let started = Instant::now();
        // SAFETY: a fresh engine and its nodes, configured before it starts.
        unsafe {
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            if config.voice_processing {
                if let Err(error) = input.setVoiceProcessingEnabled_error(true) {
                    listener.note(&format!(
                        "duplex failed step=voice_processing code={}",
                        error.code()
                    ));
                    return Err(Fault::NoMicrophone);
                }
                input.setVoiceProcessingOtherAudioDuckingConfiguration(
                    AVAudioVoiceProcessingOtherAudioDuckingConfiguration {
                        enableAdvancedDucking: Bool::NO,
                        duckingLevel: AVAudioVoiceProcessingOtherAudioDuckingLevel::Min,
                    },
                );
            }
            let enabled = started.elapsed();
            let format = input.outputFormatForBus(0);
            let (in_rate, in_channels) = (format.sampleRate(), format.channelCount());
            if !in_rate.is_finite() || in_rate <= 0.0 || in_channels == 0 {
                listener.note(&format!(
                    "duplex failed step=input_format rate={in_rate} channels={in_channels}"
                ));
                return Err(Fault::NoMicrophone);
            }
            core.set_input(in_rate);

            // Voice processing reports one channel per microphone of the
            // array (seven on a MacBook Air), each carrying the same
            // processed signal; the first is enough.
            let first_only = config.voice_processing;
            let capture = core.clone();
            let scratch = Mutex::new(Vec::<f32>::with_capacity(8_192));
            let sink_block: SinkBlock = RcBlock::new(
                move |_time: NonNull<AudioTimeStamp>,
                      frames: u32,
                      list: NonNull<AudioBufferList>| {
                    let frames = frames as usize;
                    let Ok(mut mono) = scratch.lock() else {
                        return 0;
                    };
                    mono.clear();
                    mono.resize(frames, 0.0);
                    let mut used = 0_usize;
                    // SAFETY: the engine passes a live float list for this cycle.
                    for buffer in buffers(list).iter() {
                        let channels = buffer.mNumberChannels.max(1) as usize;
                        let data = samples(buffer, frames);
                        for channel in 0..channels {
                            if first_only && used > 0 {
                                break;
                            }
                            for (out, group) in mono.iter_mut().zip(data.chunks_exact(channels)) {
                                *out += group[channel];
                            }
                            used += 1;
                        }
                    }
                    if used > 1 {
                        let scale = 1.0 / used as f32;
                        mono.iter_mut().for_each(|sample| *sample *= scale);
                    }
                    capture.capture(&mono, 1);
                    0
                },
            );
            let sink = AVAudioSinkNode::initWithReceiverBlock(
                AVAudioSinkNode::alloc(),
                RcBlock::as_ptr(&sink_block),
            );
            engine.attachNode(&sink);
            engine.connect_to_format(&input, &sink, Some(&format));

            let output = engine.outputNode();
            // Under voice processing the output node's hardware side reads
            // as empty and the default mixer connection is 44.1 kHz stereo,
            // which the voice-processing unit (running at the input rate)
            // refuses to initialize (-10875). Connect at an explicit rate.
            let hardware = output.outputFormatForBus(0);
            let (out_rate, out_channels) = if config.voice_processing {
                (in_rate, 2)
            } else {
                (hardware.sampleRate(), hardware.channelCount().max(1))
            };
            if !out_rate.is_finite() || out_rate <= 0.0 {
                listener.note(&format!("duplex failed step=output_format rate={out_rate}"));
                return Err(Fault::NoMicrophone);
            }
            let format_of = |channels| {
                AVAudioFormat::initStandardFormatWithSampleRate_channels(
                    AVAudioFormat::alloc(),
                    out_rate,
                    channels,
                )
                .ok_or(Fault::NoMicrophone)
            };
            let out_format = format_of(1)?;
            let latency = (output.presentationLatency().max(0.0) * out_rate).round() as u64;
            core.set_output(out_rate, latency);
            let (source, source_block) = source_node(core, &out_format);
            engine.attachNode(&source);
            let mixer = engine.mainMixerNode();
            let mixed = format_of(out_channels)?;
            engine.connect_to_format(&mixer, &output, Some(&mixed));
            engine.connect_to_format(&source, &mixer, Some(&out_format));

            let raise = signal.clone();
            let observer_block: RcBlock<dyn Fn(NonNull<NSNotification>)> =
                RcBlock::new(move |_note: NonNull<NSNotification>| raise.raise());
            let mut graph = Self {
                engine,
                observer: None,
                _sink: sink,
                _source: source,
                _sink_block: sink_block,
                _source_block: source_block,
                _observer_block: observer_block,
            };
            let wired = started.elapsed();
            graph.engine.prepare();
            let prepared = started.elapsed();
            if let Err(error) = graph.engine.startAndReturnError() {
                listener.note(&format!(
                    "duplex failed step=start code={} in_rate={in_rate} in_channels={in_channels} out_rate={out_rate} out_channels={out_channels}",
                    error.code()
                ));
                return Err(Fault::NoMicrophone);
            }
            let object: &AnyObject = &graph.engine;
            graph.observer = Some(
                NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
                    Some(AVAudioEngineConfigurationChangeNotification),
                    Some(object),
                    None,
                    &graph._observer_block,
                ),
            );
            listener.note(&format!(
                "duplex started ms={} input_ms={} wire_ms={} prepare_ms={} voice_processing={} in_rate={in_rate} in_channels={in_channels} out_rate={out_rate} out_channels={out_channels} latency_frames={latency}",
                started.elapsed().as_millis(),
                enabled.as_millis(),
                (wired - enabled).as_millis(),
                (prepared - wired).as_millis(),
                config.voice_processing
            ));
            Ok(graph)
        }
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        // SAFETY: removing our own observer and stopping our own engine.
        unsafe {
            if let Some(observer) = self.observer.take() {
                NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref());
            }
            self.engine.stop();
        }
    }
}

struct Shared {
    graph: Mutex<Option<Graph>>,
    signal: Arc<Signal>,
    closing: AtomicBool,
}

pub(crate) struct Device {
    shared: Arc<Shared>,
    supervisor: Option<JoinHandle<()>>,
}

impl Device {
    pub(crate) fn open(config: DuplexConfig, core: Arc<Engine>) -> Result<Self, Fault> {
        ask_microphone()?;
        let signal = Arc::new(Signal::default());
        let graph = build_settled(config, &core, &signal)?;
        let shared = Arc::new(Shared {
            graph: Mutex::new(Some(graph)),
            signal,
            closing: AtomicBool::new(false),
        });
        let watch = shared.clone();
        let supervisor = std::thread::Builder::new()
            .name("zephium-voice-route".into())
            .spawn(move || supervise(&watch, config, &core))
            .map_err(|_| Fault::NoMicrophone)?;
        Ok(Self {
            shared,
            supervisor: Some(supervisor),
        })
    }
}

/// Builds the graph, once more after a pause if the first attempt fails:
/// right after another engine (ours or another app's) releases voice
/// processing, the devices briefly report formats the I/O unit refuses.
fn build_settled(
    config: DuplexConfig,
    core: &Arc<Engine>,
    signal: &Arc<Signal>,
) -> Result<Graph, Fault> {
    let build = || objc2::rc::autoreleasepool(|_| Graph::build(config, core, signal));
    build().or_else(|_| {
        std::thread::sleep(RETRY);
        core.listener().note("duplex retry");
        build()
    })
}

fn supervise(shared: &Shared, config: DuplexConfig, core: &Arc<Engine>) {
    let mut rebuilds = std::collections::VecDeque::with_capacity(CHURN);
    let mut watching = Some(core.captured());
    loop {
        let stalled = {
            let Ok(mut raised) = shared.signal.raised.lock() else {
                return;
            };
            let deadline = Instant::now() + STALL;
            let mut stalled = false;
            while !*raised && !shared.closing.load(Ordering::Acquire) {
                let Some(seen) = watching else {
                    raised = match shared.signal.wake.wait(raised) {
                        Ok(raised) => raised,
                        Err(_) => return,
                    };
                    continue;
                };
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    watching = None;
                    stalled = core.captured() == seen;
                    if stalled {
                        break;
                    }
                    continue;
                }
                raised = match shared.signal.wake.wait_timeout(raised, left) {
                    Ok((raised, _)) => raised,
                    Err(_) => return,
                };
            }
            stalled
        };
        if shared.closing.load(Ordering::Acquire) {
            return;
        }
        if stalled {
            core.listener().note("duplex stalled");
        } else {
            std::thread::sleep(SETTLE);
        }
        if shared.closing.load(Ordering::Acquire) {
            return;
        }
        if let Ok(mut raised) = shared.signal.raised.lock() {
            *raised = false;
        }
        let Ok(mut graph) = shared.graph.lock() else {
            return;
        };
        drop(graph.take());
        let now = Instant::now();
        while rebuilds
            .front()
            .is_some_and(|at: &Instant| now.duration_since(*at) > CHURN_WINDOW)
        {
            rebuilds.pop_front();
        }
        rebuilds.push_back(now);
        let rebuilt = if rebuilds.len() > CHURN {
            Err(Fault::NoMicrophone)
        } else {
            build_settled(config, core, &shared.signal)
        };
        match rebuilt {
            Ok(rebuilt) => {
                *graph = Some(rebuilt);
                drop(graph);
                watching = Some(core.captured());
                if !stalled {
                    core.listener().route_changed();
                }
            }
            Err(fault) => {
                drop(graph);
                core.listener().lost(fault);
                return;
            }
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        self.shared.signal.raise();
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        if let Ok(mut graph) = self.shared.graph.lock() {
            graph.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DuplexListener;
    use objc2_avf_audio::{
        AVAudioEngineManualRenderingMode, AVAudioEngineManualRenderingStatus, AVAudioPCMBuffer,
    };

    #[derive(Default)]
    struct Heard {
        played: Mutex<Vec<u64>>,
        levels: Mutex<Vec<f32>>,
    }

    impl DuplexListener for Heard {
        fn frame(&self, _pcm: &[i16]) {}
        fn input_levels(&self, _levels: &[f32]) {}
        fn output_level(&self, level: f32) {
            self.levels.lock().unwrap().push(level);
        }
        fn played(&self, mark: u64) {
            self.played.lock().unwrap().push(mark);
        }
        fn route_changed(&self) {}
        fn lost(&self, _fault: Fault) {}
    }

    /// The playback half of the graph, rendered offline: no device is opened.
    #[test]
    fn the_source_node_plays_the_queue_through_the_mixer_offline() {
        let heard = Arc::new(Heard::default());
        let core = Arc::new(Engine::new(heard.clone()));
        let tone: Vec<i16> = (0..4_800)
            .map(|n| ((n as f32 * 0.26).sin() * 8_000.0) as i16)
            .collect();
        let mut left = Vec::new();
        let mut right = Vec::new();
        // SAFETY: an engine in offline manual rendering mode, rendered on this
        // thread into a buffer of its own manual format.
        unsafe {
            let engine = AVAudioEngine::new();
            let stereo = AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                48_000.0,
                2,
            )
            .unwrap();
            engine
                .enableManualRenderingMode_format_maximumFrameCount_error(
                    AVAudioEngineManualRenderingMode::Offline,
                    &stereo,
                    512,
                )
                .unwrap();
            let mono = AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                48_000.0,
                1,
            )
            .unwrap();
            core.set_output(48_000.0, 0);
            let (source, _block) = source_node(&core, &mono);
            engine.attachNode(&source);
            engine.connect_to_format(&source, &engine.mainMixerNode(), Some(&mono));
            engine.prepare();
            engine.startAndReturnError().unwrap();
            core.play(&tone);
            core.mark(1);
            let render = &*engine.manualRenderingBlock();
            let buffer = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &stereo,
                512,
            )
            .unwrap();
            for cycle in 0..40 {
                if cycle == 12 {
                    core.flush();
                }
                buffer.setFrameLength(512);
                let status =
                    render.call((512, buffer.mutableAudioBufferList(), std::ptr::null_mut()));
                assert_eq!(status, AVAudioEngineManualRenderingStatus::Success);
                let data = buffer.floatChannelData();
                let frames = buffer.frameLength() as usize;
                left.extend_from_slice(std::slice::from_raw_parts((*data).as_ptr(), frames));
                right
                    .extend_from_slice(std::slice::from_raw_parts((*data.add(1)).as_ptr(), frames));
            }
            engine.stop();
        }
        let rms = |samples: &[f32]| {
            (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
        };
        assert!(rms(&left[512..6_000]) > 0.1, "{}", rms(&left[512..6_000]));
        assert!(rms(&right[512..6_000]) > 0.1);
        assert!(
            rms(&left[12 * 512 + 512..]) < 1e-4,
            "{}",
            rms(&left[12 * 512 + 512..])
        );
        assert!(heard.played.lock().unwrap().is_empty());
        assert!(heard.levels.lock().unwrap().contains(&0.0));
    }
}
