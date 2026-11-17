//! Three short, quiet tones made in code: PCM16 mono at 24 kHz.
use zephium_page_assist::voice::Earcon;

const RATE: f32 = 24_000.0;
const PEAK: f32 = 0.12;

/// One note: frequency in hertz and length in milliseconds.
fn notes(earcon: Earcon) -> &'static [(f32, u32)] {
    match earcon {
        Earcon::ListeningStarted => &[(660.0, 60), (880.0, 70)],
        Earcon::ListeningEnded => &[(880.0, 60), (660.0, 70)],
        Earcon::Error => &[(330.0, 90), (0.0, 40), (330.0, 90)],
    }
}

/// The tone for `earcon`, with soft edges so it never clicks.
pub fn tone(earcon: Earcon) -> Vec<i16> {
    let mut pcm = Vec::new();
    for &(frequency, ms) in notes(earcon) {
        let count = (RATE as u32 * ms / 1000) as usize;
        let edge = (count / 6).max(1) as f32;
        for n in 0..count {
            let t = n as f32 / RATE;
            let fade = (n as f32 / edge).min((count - n) as f32 / edge).min(1.0);
            let value = if frequency > 0.0 {
                (std::f32::consts::TAU * frequency * t).sin() * PEAK * fade
            } else {
                0.0
            };
            pcm.push((value * f32::from(i16::MAX)) as i16);
        }
    }
    pcm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_are_short_quiet_and_start_and_end_silent() {
        for earcon in [
            Earcon::ListeningStarted,
            Earcon::ListeningEnded,
            Earcon::Error,
        ] {
            let pcm = tone(earcon);
            assert!(!pcm.is_empty() && pcm.len() < 24_000 / 4);
            let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap();
            assert!(peak > 1000 && f32::from(peak) <= PEAK * f32::from(i16::MAX) + 1.0);
            assert!(pcm[0].unsigned_abs() < 200 && pcm[pcm.len() - 1].unsigned_abs() < 600);
        }
    }
}
