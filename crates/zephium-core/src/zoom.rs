//! Page zoom: the levels a person steps through, the choices for the default,
//! and the bounds saved levels keep.

pub const MIN_PERCENT: u16 = 30;
pub const MAX_PERCENT: u16 = 300;
/// Sites a profile keeps a level for; the oldest are dropped past this.
pub const MAX_SITE_LEVELS: usize = 512;
pub const DEFAULT_PERCENT: u16 = 100;

/// What zoom in and zoom out step through, as Chrome's do within the range
/// the engines accept.
pub const STEPS: [u16; 14] = [
    30, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300,
];

/// What the default page zoom may be set to.
pub const DEFAULTS: [u16; 11] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

pub fn valid_percent(percent: u16) -> bool {
    (MIN_PERCENT..=MAX_PERCENT).contains(&percent)
}

/// A native scale as the whole percent a person sees, within the range.
pub fn percent(scale: f64) -> u16 {
    if !scale.is_finite() {
        return DEFAULT_PERCENT;
    }
    (scale * 100.0)
        .round()
        .clamp(f64::from(MIN_PERCENT), f64::from(MAX_PERCENT)) as u16
}

pub fn scale(percent: u16) -> f64 {
    f64::from(percent) / 100.0
}

/// The step above or below `from`, which need not be one itself, staying
/// put at the ends.
pub fn step(from: u16, up: bool) -> u16 {
    if up {
        STEPS.iter().copied().find(|step| *step > from)
    } else {
        STEPS.iter().rev().copied().find(|step| *step < from)
    }
    .unwrap_or(from)
}

/// A level kept for a site, with when it was last set in Unix seconds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedLevel {
    pub site: String,
    pub percent: u16,
    pub set_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_walk_the_ladder_from_any_level() {
        assert_eq!(step(100, true), 110);
        assert_eq!(step(110, true), 125);
        assert_eq!(step(100, false), 90);
        assert_eq!(step(67, false), 50);
        // Off the ladder, the nearest step in the direction asked.
        assert_eq!(step(120, true), 125);
        assert_eq!(step(120, false), 110);
        // The ends hold.
        assert_eq!(step(MAX_PERCENT, true), MAX_PERCENT);
        assert_eq!(step(MIN_PERCENT, false), MIN_PERCENT);
    }

    #[test]
    fn every_default_choice_is_a_step_inside_the_range() {
        for choice in DEFAULTS {
            assert!(STEPS.contains(&choice), "{choice}");
            assert!(valid_percent(choice));
        }
        assert!(DEFAULTS.contains(&DEFAULT_PERCENT));
    }

    #[test]
    fn percent_and_scale_round_trip_and_clamp() {
        for step in STEPS {
            assert_eq!(percent(scale(step)), step);
        }
        assert_eq!(percent(1.0999999999999999), 110);
        assert_eq!(percent(0.25), MIN_PERCENT);
        assert_eq!(percent(5.0), MAX_PERCENT);
        assert_eq!(percent(f64::NAN), DEFAULT_PERCENT);
    }
}
