//! A windowed-sinc rate converter with a precomputed kernel. The output does
//! not depend on how the input is split into blocks.

use std::f64::consts::PI;

/// Taps on each side of the output position, in input samples.
const HALF: usize = 16;
const PHASES: usize = 256;

pub(crate) struct Resampler {
    /// Input samples per output sample.
    step: f64,
    kernel: Vec<f32>,
    history: Vec<f32>,
    /// Outputs produced, and input samples dropped from the front of
    /// `history`: positions come from these counts, never an accumulated sum.
    produced: u64,
    dropped: u64,
}

impl Resampler {
    pub(crate) fn new(from: f64, to: f64) -> Self {
        let from = if from.is_finite() && from >= 1.0 {
            from
        } else {
            to
        };
        let step = from / to;
        let cutoff = (to / from).min(1.0) * 0.92;
        let mut kernel = Vec::with_capacity(PHASES * 2 * HALF);
        for phase in 0..PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let mut taps = [0.0_f64; 2 * HALF];
            for (tap, value) in taps.iter_mut().enumerate() {
                let x = tap as f64 - (HALF as f64 - 1.0) - fraction;
                let sinc = if x.abs() < 1e-9 {
                    1.0
                } else {
                    (PI * cutoff * x).sin() / (PI * cutoff * x)
                };
                let w = (x / HALF as f64).clamp(-1.0, 1.0);
                let window = 0.42 + 0.5 * (PI * w).cos() + 0.08 * (2.0 * PI * w).cos();
                *value = cutoff * sinc * window;
            }
            let sum: f64 = taps.iter().sum();
            kernel.extend(taps.iter().map(|tap| (tap / sum) as f32));
        }
        Self {
            step,
            kernel,
            history: vec![0.0; HALF - 1],
            produced: 0,
            dropped: 0,
        }
    }

    pub(crate) fn is_identity(&self) -> bool {
        self.step == 1.0
    }

    pub(crate) fn push(&mut self, input: &[f32], mut out: impl FnMut(f32)) {
        if self.is_identity() {
            input.iter().for_each(|sample| out(*sample));
            return;
        }
        self.history.extend(
            input
                .iter()
                .map(|sample| if sample.is_finite() { *sample } else { 0.0 }),
        );
        loop {
            let at = (HALF - 1) as f64 + self.produced as f64 * self.step;
            let whole = at.floor();
            let base = (whole as u64 - self.dropped) as usize;
            if base + HALF >= self.history.len() {
                break;
            }
            let phase = (((at - whole) * PHASES as f64) as usize).min(PHASES - 1);
            let taps = &self.kernel[phase * 2 * HALF..(phase + 1) * 2 * HALF];
            let window = &self.history[base + 1 - HALF..=base + HALF];
            out(taps
                .iter()
                .zip(window)
                .map(|(tap, sample)| tap * sample)
                .sum());
            self.produced += 1;
        }
        let next = ((HALF - 1) as f64 + self.produced as f64 * self.step).floor() as u64;
        let keep_from = (next + 1 - HALF as u64).saturating_sub(self.dropped) as usize;
        let keep_from = keep_from.min(self.history.len());
        self.history.drain(..keep_from);
        self.dropped += keep_from as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(resampler: &mut Resampler, input: &[f32], blocks: &[usize]) -> Vec<f32> {
        let mut out = Vec::new();
        let mut rest = input;
        let mut sizes = blocks.iter().cycle();
        while !rest.is_empty() {
            let size = (*sizes.next().unwrap()).min(rest.len());
            resampler.push(&rest[..size], |sample| out.push(sample));
            rest = &rest[size..];
        }
        out
    }

    fn sine(rate: f64, hz: f64, count: usize) -> Vec<f32> {
        (0..count)
            .map(|n| (0.5 * (2.0 * PI * hz * n as f64 / rate).sin()) as f32)
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn block_sizes_do_not_change_the_output() {
        let input = sine(48_000.0, 440.0, 48_000);
        let whole = run(&mut Resampler::new(48_000.0, 24_000.0), &input, &[48_000]);
        let ragged = run(
            &mut Resampler::new(48_000.0, 24_000.0),
            &input,
            &[1, 511, 7, 4_800, 160, 3],
        );
        assert_eq!(whole, ragged);
        let odd = run(
            &mut Resampler::new(44_100.0, 24_000.0),
            &input,
            &[441, 1, 999],
        );
        assert_eq!(
            odd,
            run(&mut Resampler::new(44_100.0, 24_000.0), &input, &[13])
        );
    }

    #[test]
    fn rates_convert_with_the_same_duration() {
        for (from, to) in [
            (48_000.0, 24_000.0),
            (44_100.0, 24_000.0),
            (24_000.0, 48_000.0),
            (16_000.0, 24_000.0),
        ] {
            let input = vec![0.0_f32; from as usize];
            let out = run(&mut Resampler::new(from, to), &input, &[512]);
            let expected = to as usize;
            assert!(
                expected.abs_diff(out.len()) <= HALF * 2,
                "{from}->{to}: {}",
                out.len()
            );
        }
    }

    #[test]
    fn passband_survives_and_aliases_are_removed() {
        let mut down = Resampler::new(48_000.0, 24_000.0);
        let kept = run(&mut down, &sine(48_000.0, 1_000.0, 48_000), &[480]);
        assert!(
            (rms(&kept[1_000..]) - 0.3535).abs() < 0.01,
            "{}",
            rms(&kept[1_000..])
        );
        let mut down = Resampler::new(48_000.0, 24_000.0);
        let alias = run(&mut down, &sine(48_000.0, 18_000.0, 48_000), &[480]);
        assert!(rms(&alias[1_000..]) < 0.01, "{}", rms(&alias[1_000..]));
        let mut up = Resampler::new(24_000.0, 48_000.0);
        let raised = run(&mut up, &sine(24_000.0, 1_000.0, 24_000), &[240]);
        assert!((rms(&raised[2_000..]) - 0.3535).abs() < 0.01);
    }

    #[test]
    fn equal_rates_pass_through_and_bad_input_is_silence() {
        let mut same = Resampler::new(24_000.0, 24_000.0);
        assert!(same.is_identity());
        assert_eq!(run(&mut same, &[0.25, -0.5], &[1]), vec![0.25, -0.5]);
        let mut down = Resampler::new(48_000.0, 24_000.0);
        let out = run(&mut down, &[f32::NAN; 4_800], &[100]);
        assert!(out.iter().all(|sample| *sample == 0.0));
        let broken = Resampler::new(f64::NAN, 24_000.0);
        assert!(broken.is_identity());
    }
}
