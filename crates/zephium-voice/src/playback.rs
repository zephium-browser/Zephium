//! The playback queue: 24 kHz PCM16 in, device-rate float out, with marks,
//! a start buffer, and fades so a flush or a resumed stream never clicks.

use std::collections::VecDeque;

use crate::meter::to_level;
use crate::resample::Resampler;
use crate::VOICE_RATE;

const fn samples(ms: usize) -> usize {
    VOICE_RATE as usize * ms / 1_000
}

/// Queued audio that starts playback without waiting for a mark.
const START: usize = samples(60);
const FADE: usize = samples(5);
const LIMIT: usize = samples(180_000);
const LEVEL_MS: f64 = 40.0;
const MAX_PLAYED: usize = 8;

/// What one render produced for the listener.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Rendered {
    pub level: Option<f32>,
    pub played: [u64; MAX_PLAYED],
    pub count: usize,
}

impl Rendered {
    pub(crate) fn played(&self) -> &[u64] {
        &self.played[..self.count]
    }
}

pub(crate) struct Playback {
    queue: VecDeque<i16>,
    enqueued: u64,
    consumed: u64,
    marks: VecDeque<(u64, u64)>,
    /// Marks waiting for the device: the rendered sample at which each is heard.
    due: VecDeque<(u64, u64)>,
    playing: bool,
    gain: f32,
    stopping: Option<usize>,
    resampler: Resampler,
    pending: VecDeque<f32>,
    /// Device samples between a 24 kHz sample entering the resampler and the speaker.
    delay: u64,
    rendered: u64,
    level_every: usize,
    level_sum: f32,
    level_count: usize,
    quiet: bool,
    went_quiet: bool,
}

impl Playback {
    pub(crate) fn new(rate: f64, latency: u64) -> Self {
        let mut playback = Self {
            queue: VecDeque::new(),
            enqueued: 0,
            consumed: 0,
            marks: VecDeque::new(),
            due: VecDeque::new(),
            playing: false,
            gain: 0.0,
            stopping: None,
            resampler: Resampler::new(f64::from(VOICE_RATE), f64::from(VOICE_RATE)),
            pending: VecDeque::new(),
            delay: 0,
            rendered: 0,
            level_every: 1,
            level_sum: 0.0,
            level_count: 0,
            quiet: true,
            went_quiet: false,
        };
        playback.set_device(rate, latency);
        playback
    }

    /// A new device rate keeps the queue and the marks.
    pub(crate) fn set_device(&mut self, rate: f64, latency: u64) {
        let rate = if rate.is_finite() && rate >= 8_000.0 {
            rate
        } else {
            f64::from(VOICE_RATE)
        };
        self.resampler = Resampler::new(f64::from(VOICE_RATE), rate);
        self.pending.clear();
        let lag = if self.resampler.is_identity() {
            0
        } else {
            (16.0 * rate / f64::from(VOICE_RATE)).ceil() as u64
        };
        self.delay = latency + lag;
        self.level_every = ((rate * LEVEL_MS / 1_000.0) as usize).max(1);
        self.level_sum = 0.0;
        self.level_count = 0;
    }

    /// Returns how many samples were accepted; the rest would exceed the limit.
    pub(crate) fn play(&mut self, pcm: &[i16]) -> usize {
        let room = LIMIT.saturating_sub(self.queue.len());
        let accepted = pcm.len().min(room);
        self.queue.extend(&pcm[..accepted]);
        self.enqueued += accepted as u64;
        accepted
    }

    pub(crate) fn mark(&mut self, id: u64) {
        self.marks.push_back((self.enqueued, id));
    }

    pub(crate) fn flush(&mut self) {
        self.marks.clear();
        self.due.clear();
        if self.playing {
            let keep = self.stopping.unwrap_or(FADE).min(self.queue.len());
            self.queue.truncate(keep);
            if keep == 0 {
                self.stop();
            } else {
                self.stopping = Some(keep);
            }
        } else {
            self.queue.clear();
        }
        self.enqueued = self.consumed + self.queue.len() as u64;
    }

    fn stop(&mut self) {
        self.playing = false;
        self.stopping = None;
        self.went_quiet = true;
    }

    fn next(&mut self) -> f32 {
        if !self.playing
            && (self.queue.len() >= START || (!self.queue.is_empty() && !self.marks.is_empty()))
        {
            self.playing = true;
            self.gain = 0.0;
        }
        if !self.playing {
            return 0.0;
        }
        let Some(sample) = self.queue.pop_front() else {
            self.stop();
            return 0.0;
        };
        self.consumed += 1;
        self.gain = (self.gain + 1.0 / FADE as f32).min(1.0);
        let mut value = f32::from(sample) / 32_768.0 * self.gain;
        if let Some(left) = self.stopping {
            value *= left as f32 / (FADE + 1) as f32;
            if left <= 1 {
                self.stop();
            } else {
                self.stopping = Some(left - 1);
            }
        }
        value
    }

    fn settle_marks(&mut self) {
        while let Some(&(at, id)) = self.marks.front() {
            if at > self.consumed {
                break;
            }
            self.marks.pop_front();
            let heard = self.rendered + self.pending.len() as u64 + self.delay;
            self.due.push_back((heard, id));
        }
    }

    /// Fills `out`, interleaved with `channels` channels, and says what was heard.
    pub(crate) fn render(&mut self, out: &mut [f32], channels: usize) -> Rendered {
        let channels = channels.max(1);
        let mut rendered = Rendered::default();
        for frame in out.chunks_mut(channels) {
            self.settle_marks();
            while self.pending.is_empty() {
                let sample = self.next();
                let pending = &mut self.pending;
                self.resampler
                    .push(&[sample], |value| pending.push_back(value));
                self.settle_marks();
            }
            let value = self.pending.pop_front().unwrap_or(0.0);
            frame.fill(value);
            self.rendered += 1;
            self.level_sum += value * value;
            self.level_count += 1;
            if self.level_count >= self.level_every {
                let level = to_level((self.level_sum / self.level_count as f32).sqrt());
                self.level_sum = 0.0;
                self.level_count = 0;
                if self.playing || level > 0.0 || !self.quiet {
                    rendered.level = Some(level);
                    self.quiet = level == 0.0 && !self.playing;
                }
            }
        }
        while rendered.count < MAX_PLAYED {
            match self.due.front() {
                Some(&(at, id)) if at <= self.rendered => {
                    self.due.pop_front();
                    rendered.played[rendered.count] = id;
                    rendered.count += 1;
                }
                _ => break,
            }
        }
        if std::mem::take(&mut self.went_quiet) && !self.playing {
            rendered.level = Some(0.0);
            self.quiet = true;
            self.level_sum = 0.0;
            self.level_count = 0;
        }
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize) -> Vec<i16> {
        (0..samples(ms))
            .map(|n| ((n as f32 * 0.26).sin() * 8_000.0) as i16)
            .collect()
    }

    /// Renders `ms` of device audio in `block`-frame calls.
    fn render(
        playback: &mut Playback,
        rate: f64,
        ms: usize,
        block: usize,
    ) -> (Vec<f32>, Vec<Rendered>) {
        let total = (rate as usize * ms / 1_000).max(1);
        let mut audio = Vec::new();
        let mut events = Vec::new();
        while audio.len() < total {
            let mut out = vec![0.0; block.min(total - audio.len())];
            events.push(playback.render(&mut out, 1));
            audio.extend(out);
        }
        (audio, events)
    }

    fn loud(audio: &[f32]) -> Option<usize> {
        audio.iter().position(|sample| sample.abs() > 1e-4)
    }

    #[test]
    fn playback_waits_for_sixty_milliseconds_or_a_mark() {
        let mut playback = Playback::new(24_000.0, 0);
        playback.play(&tone(40));
        let (audio, _) = render(&mut playback, 24_000.0, 30, 240);
        assert_eq!(loud(&audio), None);
        playback.play(&tone(20));
        let (audio, _) = render(&mut playback, 24_000.0, 30, 240);
        assert!(loud(&audio).is_some());

        let mut short = Playback::new(24_000.0, 0);
        short.play(&tone(10));
        assert_eq!(loud(&render(&mut short, 24_000.0, 20, 240).0), None);
        short.mark(1);
        let (audio, events) = render(&mut short, 24_000.0, 20, 240);
        assert!(loud(&audio).is_some());
        assert!(events.iter().any(|event| event.played() == [1]));
    }

    #[test]
    fn a_resumed_stream_fades_in_instead_of_clicking() {
        let mut playback = Playback::new(24_000.0, 0);
        playback.play(&vec![16_000; samples(100)]);
        let (audio, _) = render(&mut playback, 24_000.0, 20, 480);
        assert!(audio[0].abs() < 0.01);
        assert!(audio[FADE / 2] < audio[FADE]);
        assert!((audio[FADE + 10] - 16_000.0 / 32_768.0).abs() < 1e-6);
        let steps = audio.windows(2).map(|pair| (pair[1] - pair[0]).abs());
        assert!(steps.fold(0.0, f32::max) < 0.01);
    }

    #[test]
    fn flush_fades_to_silence_within_five_milliseconds_and_drops_marks() {
        let mut playback = Playback::new(48_000.0, 0);
        playback.play(&vec![16_000; samples(500)]);
        playback.mark(7);
        render(&mut playback, 48_000.0, 50, 512);
        playback.flush();
        let (audio, events) = render(&mut playback, 48_000.0, 100, 512);
        let last = audio
            .iter()
            .rposition(|sample| sample.abs() > 1e-3)
            .unwrap();
        assert!(last < 48 * 7, "{last}");
        let steps = audio.windows(2).map(|pair| (pair[1] - pair[0]).abs());
        assert!(steps.fold(0.0, f32::max) < 0.02);
        assert_eq!(events[0].level, Some(0.0));
        assert!(events.iter().all(|event| event.played().is_empty()));
        playback.mark(8);
        let (_, events) = render(&mut playback, 48_000.0, 10, 512);
        assert!(events.iter().any(|event| event.played() == [8]));
    }

    #[test]
    fn audio_queued_after_a_flush_plays_after_the_fade() {
        let mut playback = Playback::new(24_000.0, 0);
        playback.play(&vec![16_000; samples(200)]);
        render(&mut playback, 24_000.0, 20, 240);
        playback.flush();
        playback.play(&vec![-16_000; samples(100)]);
        let (audio, _) = render(&mut playback, 24_000.0, 60, 240);
        assert!(audio[..FADE].iter().all(|sample| *sample >= 0.0));
        assert!(audio[FADE..].iter().all(|sample| *sample <= 0.0));
        assert!(audio[FADE * 4] < -0.4);
    }

    #[test]
    fn marks_arrive_once_their_audio_has_reached_the_speaker() {
        let latency = 480;
        let mut playback = Playback::new(48_000.0, latency);
        playback.play(&tone(100));
        playback.mark(1);
        playback.play(&tone(100));
        playback.mark(2);
        let mut seen = Vec::new();
        let mut at = Vec::new();
        for call in 0..100 {
            let mut out = vec![0.0; 480];
            for id in playback.render(&mut out, 1).played() {
                seen.push(*id);
                at.push((call + 1) * 480);
            }
        }
        assert_eq!(seen, vec![1, 2]);
        let first = at[0] as u64;
        assert!(
            (4_800 + latency..4_800 + latency + 960).contains(&first),
            "{first}"
        );
    }

    #[test]
    fn an_underrun_is_silence_and_reports_a_zero_level_once() {
        let mut playback = Playback::new(24_000.0, 0);
        playback.play(&tone(80));
        let (_, events) = render(&mut playback, 24_000.0, 400, 240);
        let levels: Vec<f32> = events.iter().filter_map(|event| event.level).collect();
        assert!(levels.len() >= 2, "{levels:?}");
        assert!(levels[0] > 0.3);
        assert_eq!(levels.iter().filter(|level| **level == 0.0).count(), 1);
        assert_eq!(*levels.last().unwrap(), 0.0);
    }

    #[test]
    fn levels_arrive_about_twenty_five_times_a_second_while_playing() {
        let mut playback = Playback::new(48_000.0, 0);
        playback.play(&tone(2_000));
        let (_, events) = render(&mut playback, 48_000.0, 1_000, 441);
        let count = events.iter().filter(|event| event.level.is_some()).count();
        assert!((24..=26).contains(&count), "{count}");
    }

    #[test]
    fn the_queue_is_bounded_and_a_device_change_keeps_it() {
        let mut playback = Playback::new(48_000.0, 0);
        let chunk = vec![1_000; samples(60_000)];
        assert_eq!(playback.play(&chunk), chunk.len());
        assert_eq!(playback.play(&chunk), chunk.len());
        assert_eq!(playback.play(&chunk), chunk.len());
        assert_eq!(playback.play(&chunk), 0);
        playback.set_device(44_100.0, 256);
        let mut out = vec![0.0; 4_410 * 2];
        playback.render(&mut out, 2);
        assert!(out[4_000] > 0.0 && out[4_000] == out[4_001]);
    }
}
