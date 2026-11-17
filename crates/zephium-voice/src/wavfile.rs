//! A WAV file in place of the microphone, for end-to-end checks without a
//! device: fed in real time, then silence; playback is paced and discarded.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::duplex::Engine;
use crate::{Fault, VOICE_RATE};

const MAX_BYTES: u64 = 64 * 1024 * 1024;
const TICK: Duration = Duration::from_millis(10);

/// Mono samples and their rate.
#[derive(Debug, PartialEq)]
pub(crate) struct Wav {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// PCM16 or float32 WAV, any channel count, mixed to mono.
pub(crate) fn parse(bytes: &[u8]) -> Option<Wav> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = bytes.get(at + 8..(at + 8).checked_add(size)?)?;
        match id {
            b"fmt " if body.len() >= 16 => {
                let mut tag = u16::from_le_bytes([body[0], body[1]]);
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = u16::from_le_bytes([body[24], body[25]]);
                }
                let channels = u16::from_le_bytes([body[2], body[3]]) as usize;
                let rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
                let bits = u16::from_le_bytes([body[14], body[15]]);
                format = Some((tag, channels, rate, bits));
            }
            b"data" => {
                let (tag, channels, rate, bits) = format?;
                if channels == 0 || !(8_000..=192_000).contains(&rate) {
                    return None;
                }
                let width = match (tag, bits) {
                    (1, 16) => 2,
                    (3, 32) => 4,
                    _ => return None,
                };
                let samples = body
                    .chunks_exact(width * channels)
                    .map(|group| {
                        let sum: f32 = group
                            .chunks_exact(width)
                            .map(|sample| match width {
                                2 => {
                                    f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0
                                }
                                _ => f32::from_le_bytes(sample.try_into().unwrap_or_default()),
                            })
                            .sum();
                        sum / channels as f32
                    })
                    .collect();
                return Some(Wav { rate, samples });
            }
            _ => {}
        }
        at += 8 + size + (size & 1);
    }
    None
}

pub(crate) struct Feeder {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Feeder {
    pub(crate) fn start(path: &std::path::Path, engine: Arc<Engine>) -> Result<Self, Fault> {
        let size = std::fs::metadata(path)
            .map_err(|_| Fault::NoMicrophone)?
            .len();
        if size > MAX_BYTES {
            return Err(Fault::NoMicrophone);
        }
        let bytes = std::fs::read(path).map_err(|_| Fault::NoMicrophone)?;
        let wav = parse(&bytes).ok_or(Fault::NoMicrophone)?;
        drop(bytes);
        engine.set_input(f64::from(wav.rate));
        engine.set_output(f64::from(VOICE_RATE), 0);
        engine.listener().note(&format!(
            "file input rate={} ms={}",
            wav.rate,
            wav.samples.len() as u64 * 1_000 / u64::from(wav.rate)
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::Builder::new()
            .name("zephium-voice-file".into())
            .spawn(move || {
                let input_tick = (wav.rate / 100) as usize;
                let silence = vec![0.0; input_tick];
                let mut output = vec![0.0; VOICE_RATE as usize / 100];
                let mut position = 0;
                let mut next = Instant::now();
                while !worker_stop.load(Ordering::Acquire) {
                    let end = (position + input_tick).min(wav.samples.len());
                    if position < end {
                        engine.capture(&wav.samples[position..end], 1);
                        position = end;
                    } else {
                        engine.capture(&silence, 1);
                    }
                    engine.render(&mut output, 1);
                    next += TICK;
                    std::thread::sleep(next.saturating_duration_since(Instant::now()));
                }
            })
            .map_err(|_| Fault::NoMicrophone)?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        let mut out = b"RIFF\0\0\0\0WAVE".to_vec();
        out.extend_from_slice(b"LIST");
        out.extend_from_slice(&3_u32.to_le_bytes());
        out.extend_from_slice(b"abc\0");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16_u32.to_le_bytes());
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
        out.extend_from_slice(&(channels * bits / 8).to_le_bytes());
        out.extend_from_slice(&bits.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn pcm16_stereo_and_float_mono_are_read_as_mono() {
        let stereo: Vec<u8> = [16_384_i16, 0, -32_768, -32_768]
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let wav = parse(&header(1, 2, 48_000, 16, &stereo)).unwrap();
        assert_eq!(
            wav,
            Wav {
                rate: 48_000,
                samples: vec![0.25, -1.0]
            }
        );
        let float: Vec<u8> = [0.5_f32, -0.25]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let wav = parse(&header(3, 1, 24_000, 32, &float)).unwrap();
        assert_eq!(wav.samples, vec![0.5, -0.25]);
        assert_eq!(crate::wav(&[0, 1], 16_000).len(), 48);
        assert_eq!(
            parse(&crate::wav(&[0, 16_384], 16_000)).unwrap().samples,
            vec![0.0, 0.5]
        );
    }

    #[test]
    fn malformed_files_are_refused() {
        let data = [0_u8; 8];
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"RIFF\0\0\0\0WAVX"), None);
        assert_eq!(parse(&header(1, 0, 48_000, 16, &data)), None);
        assert_eq!(parse(&header(1, 1, 100, 16, &data)), None);
        assert_eq!(parse(&header(1, 1, 48_000, 24, &data)), None);
        assert_eq!(parse(&header(2, 1, 48_000, 16, &data)), None);
        let mut truncated = header(1, 1, 48_000, 16, &data);
        truncated.truncate(truncated.len() - 3);
        assert_eq!(parse(&truncated), None);
        let mut huge = header(1, 1, 48_000, 16, &data);
        let at = huge.len() - 12;
        huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&huge), None);
        let mut no_format = b"RIFF\0\0\0\0WAVEdata".to_vec();
        no_format.extend_from_slice(&2_u32.to_le_bytes());
        no_format.extend_from_slice(&[0, 0]);
        assert_eq!(parse(&no_format), None);
    }
}
