//! What the page edge shows when the assistant's glow and voice meet, and
//! how a voice level becomes the edge's breath. Platform-free: both native
//! glows draw from these numbers.

use crate::page_reader::{PageGlow, PageVoice};

/// The tuning block for the voice edge. Opacities multiply the existing
/// glow's peak (`PEAK_LIGHT`/`PEAK_DARK` in each platform's glow), so the
/// voice edge keeps that glow's palette and its reading on light and dark
/// pages.
pub(crate) mod tune {
    /// Listening's brightness as a share of the glow's peak.
    pub(crate) const LISTEN_SHARE: f32 = 1.25;
    /// Listening's breath at silence, as a share of its full brightness:
    /// the microphone-is-live edge never dims below this.
    pub(crate) const LISTEN_REST: f32 = 0.68;
    /// Speaking's brightness as a share of the glow's peak.
    pub(crate) const SPEAK_SHARE: f32 = 0.85;
    /// Speaking's breath at silence.
    pub(crate) const SPEAK_REST: f32 = 0.55;
    /// Reduced motion: the still breath both voice states hold.
    pub(crate) const STILL_BREATH: f32 = 0.85;
    /// Levels are percent; this bends them before smoothing (1 is linear;
    /// the input meter is already in decibels).
    pub(crate) const LEVEL_CURVE: f32 = 0.8;
    /// Time constant of the breath rising toward a louder level, seconds.
    pub(crate) const ATTACK_TAU: f64 = 0.045;
    /// Time constant of the breath falling toward a quieter level, seconds.
    pub(crate) const RELEASE_TAU: f64 = 0.32;
    /// The breath glides to each new level over this long (about one level
    /// interval), seconds.
    pub(crate) const GLIDE_SECONDS: f64 = 0.06;
    /// After the last level, the breath settles to rest over this long,
    /// seconds; it is only seen when levels stop.
    pub(crate) const TAIL_SECONDS: f64 = 0.9;
    /// When the microphone closes or speech ends, the edge holds this long
    /// before it fades (the assistant's glow holds longer), seconds.
    pub(crate) const VOICE_HOLD_SECONDS: f64 = 0.05;
    /// A gap longer than this between levels counts as this long.
    pub(crate) const MAX_GAP_SECONDS: f64 = 1.0;
    /// Levels arrive about this often; the first level's step.
    pub(crate) const LEVEL_SECONDS: f64 = 0.04;
}

/// What the edge shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Look {
    #[default]
    Off,
    Working,
    Paused,
    Listening,
    Speaking,
}

impl Look {
    /// The microphone is the person's own fact and always shows; the
    /// assistant acting outranks its own speech, which is the quietest.
    pub(crate) const fn of(assist: PageGlow, voice: PageVoice) -> Self {
        match (voice, assist) {
            (PageVoice::Listening, _) => Self::Listening,
            (_, PageGlow::Working) => Self::Working,
            (_, PageGlow::Paused) => Self::Paused,
            (PageVoice::Speaking, PageGlow::Off) => Self::Speaking,
            (PageVoice::Off, PageGlow::Off) => Self::Off,
        }
    }

    pub(crate) const fn voiced(self) -> bool {
        matches!(self, Self::Listening | Self::Speaking)
    }

    /// The colours travel only while the assistant works.
    pub(crate) const fn travels(self, reduce_motion: bool) -> bool {
        matches!(self, Self::Working) && !reduce_motion
    }

    /// Brightness as a share of the glow's peak.
    pub(crate) const fn share(self, paused_share: f32) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Working => 1.0,
            Self::Paused => paused_share,
            Self::Listening => tune::LISTEN_SHARE,
            Self::Speaking => tune::SPEAK_SHARE,
        }
    }

    /// The breath at silence; 1 for the looks that do not breathe.
    pub(crate) const fn rest(self, reduce_motion: bool) -> f32 {
        match self {
            Self::Listening | Self::Speaking if reduce_motion => tune::STILL_BREATH,
            Self::Listening => tune::LISTEN_REST,
            Self::Speaking => tune::SPEAK_REST,
            _ => 1.0,
        }
    }

    /// Whether levels move this look.
    pub(crate) const fn breathes(self, reduce_motion: bool) -> bool {
        self.voiced() && !reduce_motion
    }
}

/// A level smoothed into breath: rising fast, falling slow, by the time
/// between levels rather than their count.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Envelope {
    value: f32,
    at: Option<f64>,
}

impl Envelope {
    /// Feeds one level (percent; above 100 counts as 100) heard at `now`
    /// seconds; the smoothed level, 0 to 1.
    pub(crate) fn feed(&mut self, level: u8, now: f64) -> f32 {
        let target = (f32::from(level.min(100)) / 100.0).powf(tune::LEVEL_CURVE);
        let gap = self.at.map_or(tune::LEVEL_SECONDS, |at| {
            (now - at).clamp(0.0, tune::MAX_GAP_SECONDS)
        });
        self.at = Some(now);
        let tau = if target > self.value {
            tune::ATTACK_TAU
        } else {
            tune::RELEASE_TAU
        };
        let step = 1.0 - (-gap / tau).exp();
        self.value = (self.value + (target - self.value) * step as f32).clamp(0.0, 1.0);
        self.value
    }

    /// The breath opacity for `look` at the current level.
    pub(crate) fn breath(&self, look: Look, reduce_motion: bool) -> f32 {
        let rest = look.rest(reduce_motion);
        if look.breathes(reduce_motion) {
            rest + (1.0 - rest) * self.value
        } else {
            rest
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_outranks_working_which_outranks_speaking() {
        use PageGlow as G;
        use PageVoice as V;
        let cases = [
            (G::Off, V::Off, Look::Off),
            (G::Working, V::Off, Look::Working),
            (G::Paused, V::Off, Look::Paused),
            (G::Off, V::Listening, Look::Listening),
            (G::Working, V::Listening, Look::Listening),
            (G::Paused, V::Listening, Look::Listening),
            (G::Off, V::Speaking, Look::Speaking),
            (G::Working, V::Speaking, Look::Working),
            (G::Paused, V::Speaking, Look::Paused),
        ];
        for (assist, voice, look) in cases {
            assert_eq!(Look::of(assist, voice), look, "{assist:?} {voice:?}");
        }
    }

    #[test]
    fn only_working_travels_and_only_voice_breathes() {
        for look in [
            Look::Off,
            Look::Working,
            Look::Paused,
            Look::Listening,
            Look::Speaking,
        ] {
            assert!(!look.travels(true));
            assert!(!look.breathes(true), "reduced motion holds still");
            assert_eq!(look.travels(false), look == Look::Working);
            assert_eq!(look.breathes(false), look.voiced());
        }
        assert!(Look::Listening.share(0.45) > Look::Working.share(0.45));
        assert!(Look::Speaking.share(0.45) < Look::Working.share(0.45));
        assert_eq!(Look::Off.share(0.45), 0.0);
    }

    #[test]
    fn the_breath_rises_fast_and_falls_slow() {
        let mut envelope = Envelope::default();
        let mut t = 0.0;
        let mut rise = 0;
        while envelope.feed(100, t) < 0.9 {
            rise += 1;
            t += 0.04;
        }
        let mut fall = 0;
        while envelope.feed(0, t) > 0.1 {
            fall += 1;
            t += 0.04;
        }
        assert!(rise <= 3, "attack within three levels, took {rise}");
        assert!(fall >= 4 * rise.max(1), "release slower: {fall} vs {rise}");
        assert!(fall <= 25, "but settles within a second, took {fall}");
    }

    #[test]
    fn the_breath_is_bounded_and_steady() {
        let mut envelope = Envelope::default();
        for k in 0..200 {
            let value = envelope.feed(255, f64::from(k) * 0.04);
            assert!((0.0..=1.0).contains(&value), "an out-of-range level clamps");
        }
        assert!((envelope.feed(100, 8.0) - 1.0).abs() < 1e-4);
        let mut steady = Envelope::default();
        for k in 0..100 {
            steady.feed(50, f64::from(k) * 0.04);
        }
        let expected = 0.5_f32.powf(tune::LEVEL_CURVE);
        assert!((steady.feed(50, 4.0) - expected).abs() < 1e-3);
    }

    #[test]
    fn gaps_and_clock_faults_stay_bounded() {
        let mut envelope = Envelope::default();
        envelope.feed(100, 10.0);
        envelope.feed(100, 10.04);
        let before = envelope.feed(100, 10.08);
        let after_backwards = envelope.feed(0, 5.0);
        assert_eq!(after_backwards, before, "a clock going back moves nothing");
        let mut long = Envelope::default();
        long.feed(100, 0.0);
        long.feed(100, 0.04);
        let held = long.feed(0, 3_600.0);
        let mut second = Envelope::default();
        second.feed(100, 0.0);
        second.feed(100, 0.04);
        let capped = second.feed(0, 0.04 + tune::MAX_GAP_SECONDS);
        assert!((held - capped).abs() < 1e-5, "a long gap counts as the cap");
        assert!(held > 0.0 && held < 0.2);
    }

    #[test]
    fn breath_spans_rest_to_full_only_while_moving() {
        let mut envelope = Envelope::default();
        assert_eq!(envelope.breath(Look::Listening, false), tune::LISTEN_REST);
        assert_eq!(envelope.breath(Look::Speaking, false), tune::SPEAK_REST);
        for k in 0..100 {
            envelope.feed(100, f64::from(k) * 0.04);
        }
        assert!((envelope.breath(Look::Listening, false) - 1.0).abs() < 1e-4);
        assert_eq!(envelope.breath(Look::Listening, true), tune::STILL_BREATH);
        assert_eq!(envelope.breath(Look::Working, false), 1.0);
        assert_eq!(envelope.breath(Look::Off, false), 1.0);
        const { assert!(tune::LISTEN_REST > tune::SPEAK_REST) };
        const { assert!(tune::LISTEN_SHARE * tune::LISTEN_REST >= 0.8) };
    }
}
