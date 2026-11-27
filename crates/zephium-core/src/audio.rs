//! Whether a tab is making sound, and whether the person muted it.
//!
//! The speaker appears the moment the engine reports sound and stays until the
//! page has been quiet for [`AUDIO_HOLD`], so a run of short sounds (typing
//! clicks, notification blips, a video buffering) reads as one, without a
//! flicker per sound. Mute is the person's choice for the tab and outlives its
//! documents and native views, but not the process.

use std::time::{Duration, Instant};

/// How long the speaker stays after the page goes quiet.
pub const AUDIO_HOLD: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabAudio {
    /// The person muted this tab. Runtime only, never persisted.
    pub muted: bool,
    audible: bool,
    quiet_since: Option<Instant>,
}

impl TabAudio {
    /// Whether the tab shows that it is playing sound.
    pub fn shown(&self) -> bool {
        self.audible || self.quiet_since.is_some()
    }

    /// When the speaker should go, if the page has gone quiet.
    pub fn deadline(&self) -> Option<Instant> {
        self.quiet_since.map(|quiet| quiet + AUDIO_HOLD)
    }

    /// Applies the engine's report. Returns whether [`Self::shown`] changed;
    /// going quiet never hides the speaker at once, it arms the deadline.
    pub fn observe(&mut self, audible: bool, now: Instant) -> bool {
        let was = self.shown();
        if audible {
            self.audible = true;
            self.quiet_since = None;
        } else if self.audible {
            self.audible = false;
            self.quiet_since = Some(now);
        }
        self.shown() != was
    }

    /// Hides the speaker once the page has been quiet long enough. Returns
    /// whether it was hidden.
    pub fn settle(&mut self, now: Instant) -> bool {
        if self.deadline().is_some_and(|deadline| now >= deadline) {
            self.quiet_since = None;
            return true;
        }
        false
    }

    /// The native view is gone, and with it any sound. Mute stays.
    pub fn silence(&mut self) -> bool {
        let was = self.shown();
        self.audible = false;
        self.quiet_since = None;
        was
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sound_shows_at_once_and_quiet_waits_for_the_hold() {
        let start = Instant::now();
        let mut audio = TabAudio::default();
        assert!(!audio.shown());
        assert!(audio.observe(true, start));
        assert!(audio.shown());
        assert!(!audio.observe(false, start + Duration::from_millis(50)));
        assert!(audio.shown());
        assert_eq!(
            audio.deadline(),
            Some(start + Duration::from_millis(50) + AUDIO_HOLD)
        );
        assert!(!audio.settle(start + AUDIO_HOLD));
        assert!(audio.shown());
        assert!(audio.settle(start + Duration::from_millis(50) + AUDIO_HOLD));
        assert!(!audio.shown());
        assert_eq!(audio.deadline(), None);
    }

    #[test]
    fn a_run_of_short_sounds_reads_as_one() {
        let start = Instant::now();
        let mut audio = TabAudio::default();
        let mut changes = 0;
        for blip in 0..10_u64 {
            let at = start + Duration::from_millis(blip * 400);
            changes += usize::from(audio.observe(true, at));
            changes += usize::from(audio.observe(false, at + Duration::from_millis(80)));
            changes += usize::from(audio.settle(at + Duration::from_millis(399)));
        }
        assert_eq!(changes, 1);
        assert!(audio.shown());
        let last_quiet = start + Duration::from_millis(9 * 400 + 80);
        assert!(audio.settle(last_quiet + AUDIO_HOLD));
    }

    #[test]
    fn sound_resuming_inside_the_hold_disarms_it() {
        let start = Instant::now();
        let mut audio = TabAudio::default();
        audio.observe(true, start);
        audio.observe(false, start);
        assert!(!audio.observe(true, start + Duration::from_secs(1)));
        assert_eq!(audio.deadline(), None);
        assert!(!audio.settle(start + AUDIO_HOLD * 3));
        assert!(audio.shown());
    }

    #[test]
    fn repeated_quiet_reports_never_postpone_the_hold() {
        let start = Instant::now();
        let mut audio = TabAudio::default();
        audio.observe(true, start);
        audio.observe(false, start);
        audio.observe(false, start + Duration::from_secs(1));
        assert_eq!(audio.deadline(), Some(start + AUDIO_HOLD));
    }

    #[test]
    fn mute_survives_the_view_going_away() {
        let mut audio = TabAudio {
            muted: true,
            ..TabAudio::default()
        };
        audio.observe(true, Instant::now());
        assert!(audio.silence());
        assert!(!audio.shown());
        assert!(audio.muted);
        assert!(!audio.silence());
    }
}
