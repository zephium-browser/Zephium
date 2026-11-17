//! Input level for the live view, and where speech ends, measured in audio
//! time so the same samples always give the same answer.

/// The view's level updates at most this often (≤ 30 Hz).
const LEVEL_INTERVAL_MS: u32 = 40;
/// dBFS mapped to the bottom and top of the visual level.
const FLOOR_DB: f32 = -58.0;
const CEILING_DB: f32 = -12.0;
/// Above this level a frame counts as speech; below `QUIET`, as a pause.
const SPEECH: f32 = 0.30;
const QUIET: f32 = 0.16;
/// Speech must last this long before a pause can end the take, so a cough
/// or a click does not.
const MIN_SPEECH_MS: u32 = 400;
/// A natural pause: long enough that a person thinking mid-sentence is not
/// cut off.
pub const PAUSE_MS: u32 = 2_800;
/// Nothing said at all for this long ends the take.
pub const NOTHING_HEARD_MS: u32 = 20_000;
/// The longest take. 240 s of 16 kHz PCM16 is 7.7 MB, which fits an 8 MiB
/// request body with room for the multipart framing.
pub const MAX_MS: u32 = 240_000;

/// Why capture ended without being asked to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ending {
    /// Speech, then a natural pause.
    Pause,
    /// No speech at all.
    NothingHeard,
    /// The longest take allowed.
    Limit,
    /// The microphone went away.
    Lost,
    /// The recognizer failed; finishing the take says how.
    Failed,
}

/// What one block of samples produced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    /// A new level for the view, 0–1.
    pub level: Option<f32>,
    pub ending: Option<Ending>,
}

#[derive(Debug)]
pub struct Meter {
    rate: f64,
    /// Samples into the take.
    position: u64,
    /// Samples since the last level went out.
    since_level: u64,
    /// The loudest smoothed level since then.
    peak: f32,
    smoothed: f32,
    speech_ms: u32,
    quiet_ms: u32,
    ended: bool,
    pause: bool,
}

impl Meter {
    /// `rate` in Hz; `pause` ends the take on a natural pause.
    pub fn new(rate: f64, pause: bool) -> Self {
        Self {
            rate: if rate.is_finite() && rate > 0.0 {
                rate
            } else {
                48_000.0
            },
            position: 0,
            since_level: 0,
            peak: 0.0,
            smoothed: 0.0,
            speech_ms: 0,
            quiet_ms: 0,
            ended: false,
            pause,
        }
    }

    fn ms(&self, samples: u64) -> u32 {
        ((samples as f64 * 1_000.0) / self.rate).min(f64::from(u32::MAX)) as u32
    }

    /// Takes one block of mono samples in −1…1.
    pub fn push(&mut self, samples: &[f32]) -> Step {
        let mut step = Step::default();
        if samples.is_empty() {
            return step;
        }
        let energy = samples
            .iter()
            .map(|sample| {
                let sample = if sample.is_finite() { *sample } else { 0.0 };
                sample * sample
            })
            .sum::<f32>()
            / samples.len() as f32;
        let level = to_level(energy.sqrt());
        // Quick to rise, slow to fall: speech reads as speech, not flicker.
        let weight = if level > self.smoothed { 0.6 } else { 0.18 };
        self.smoothed += (level - self.smoothed) * weight;
        self.peak = self.peak.max(self.smoothed);

        let count = samples.len() as u64;
        self.position += count;
        self.since_level += count;
        if self.ms(self.since_level) >= LEVEL_INTERVAL_MS {
            step.level = Some(self.peak);
            self.peak = 0.0;
            self.since_level = 0;
        }

        let block_ms = self.ms(count);
        if self.smoothed >= SPEECH {
            self.speech_ms = self.speech_ms.saturating_add(block_ms);
            self.quiet_ms = 0;
        } else if self.smoothed < QUIET {
            self.quiet_ms = self.quiet_ms.saturating_add(block_ms);
        }
        if !self.ended {
            let heard = self.speech_ms >= MIN_SPEECH_MS;
            let ending = if self.ms(self.position) >= MAX_MS {
                Some(Ending::Limit)
            } else if self.pause && heard && self.quiet_ms >= PAUSE_MS {
                Some(Ending::Pause)
            } else if !heard && self.ms(self.position) >= NOTHING_HEARD_MS {
                Some(Ending::NothingHeard)
            } else {
                None
            };
            if ending.is_some() {
                self.ended = true;
                step.ending = ending;
            }
        }
        step
    }

    /// Whether any speech was heard.
    pub fn heard(&self) -> bool {
        self.speech_ms >= MIN_SPEECH_MS
    }
}

pub(crate) fn to_level(rms: f32) -> f32 {
    if rms <= 0.0 || !rms.is_finite() {
        return 0.0;
    }
    let db = 20.0 * rms.log10();
    ((db - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 16_000.0;

    fn tone(amplitude: f32, ms: u32) -> Vec<f32> {
        let count = (RATE as u32 * ms / 1_000) as usize;
        (0..count)
            .map(|n| amplitude * (n as f32 * 0.1).sin())
            .collect()
    }

    /// Feeds `samples` in 20 ms blocks and gathers what came out.
    fn feed(meter: &mut Meter, samples: &[f32]) -> (Vec<f32>, Option<Ending>) {
        let mut levels = Vec::new();
        let mut ending = None;
        for block in samples.chunks(320) {
            let step = meter.push(block);
            levels.extend(step.level);
            ending = ending.or(step.ending);
        }
        (levels, ending)
    }

    #[test]
    fn silence_is_zero_and_a_loud_voice_is_near_full() {
        assert_eq!(to_level(0.0), 0.0);
        assert_eq!(to_level(f32::NAN), 0.0);
        assert!(to_level(0.0005) < 0.05);
        assert!(to_level(0.3) > 0.9);
    }

    #[test]
    fn levels_go_out_at_most_every_forty_milliseconds() {
        let mut meter = Meter::new(RATE, false);
        let (levels, _) = feed(&mut meter, &tone(0.2, 1_000));
        assert!((24..=26).contains(&levels.len()), "{}", levels.len());
        assert!(levels.iter().all(|level| (0.0..=1.0).contains(level)));
        assert!(*levels.last().unwrap() > 0.5);
    }

    #[test]
    fn a_pause_after_speech_ends_the_take_but_not_a_short_one() {
        let mut meter = Meter::new(RATE, true);
        assert_eq!(feed(&mut meter, &tone(0.2, 1_500)).1, None);
        assert_eq!(feed(&mut meter, &tone(0.0, 1_500)).1, None);
        assert_eq!(feed(&mut meter, &tone(0.2, 500)).1, None);
        assert_eq!(feed(&mut meter, &tone(0.0, 3_200)).1, Some(Ending::Pause));
        assert!(meter.heard());
    }

    #[test]
    fn without_pause_ending_only_the_limit_stops_speech() {
        let mut meter = Meter::new(RATE, false);
        assert_eq!(feed(&mut meter, &tone(0.2, 1_000)).1, None);
        assert_eq!(feed(&mut meter, &tone(0.0, 10_000)).1, None);
    }

    #[test]
    fn a_click_is_not_speech_and_long_silence_ends_with_nothing_heard() {
        let mut meter = Meter::new(RATE, true);
        assert_eq!(feed(&mut meter, &tone(0.3, 100)).1, None);
        let (_, ending) = feed(&mut meter, &tone(0.0, NOTHING_HEARD_MS));
        assert_eq!(ending, Some(Ending::NothingHeard));
        assert!(!meter.heard());
    }

    #[test]
    fn the_take_ends_once_at_the_limit() {
        let mut meter = Meter::new(1_000.0, false);
        let block = vec![0.2_f32; 1_000];
        let mut endings = Vec::new();
        for _ in 0..(MAX_MS / 1_000 + 5) {
            endings.extend(meter.push(&block).ending);
        }
        assert_eq!(endings, vec![Ending::Limit]);
    }
}
