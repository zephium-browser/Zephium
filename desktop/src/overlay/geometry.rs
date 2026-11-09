//! The launcher has one width and a height that follows its content. It is
//! not resizable or draggable, so there is no geometry to persist: it opens
//! at the same place on whichever display the user is working on.
pub const WIDTH: f64 = 680.0;
/// Room around natively drawn shapes for their shadow. Liquid Glass casts a
/// wide, soft one; any less and the window's edge cuts it off in a visible
/// rectangle. The window is transparent there.
pub const SHAPE_INSET: f64 = 40.0;
/// The field alone.
pub const RESTING_HEIGHT: f64 = 56.0;
// The ordinary three-row home fits at first reveal; subsequent layouts use content height.
pub const INITIAL_HEIGHT: f64 = if cfg!(target_os = "windows") {
    250.0
} else {
    RESTING_HEIGHT
};
pub const MAX_HEIGHT: f64 = 600.0;
/// The tab switcher is one plate holding a row of page cards, as wide as the
/// cards it shows and centred on the display, the way the system's own
/// switcher stands. The frame lays its cards out from the same numbers.
pub const SWITCHER_CARD: f64 = 204.0;
pub const SWITCHER_GAP: f64 = 4.0;
pub const SWITCHER_PAD: f64 = 12.0;
pub const SWITCHER_VISIBLE: usize = 4;
pub const SWITCHER_HEIGHT: f64 = 180.0;
/// The plate's corner, concentric with a card's 14 at its padding.
pub const SWITCHER_RADIUS: f64 = 26.0;

pub fn switcher_width(count: usize) -> f64 {
    let cards = count.clamp(1, SWITCHER_VISIBLE) as f64;
    2.0 * SWITCHER_PAD + cards * SWITCHER_CARD + (cards - 1.0) * SWITCHER_GAP
}
const MARGIN: f64 = 24.0;
/// Where the top edge sits, as a share of the work area's height. High
/// enough that results grow into open space, low enough to be near the eye.
const TOP: f64 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    max_height: f64,
}

impl Placement {
    /// `work_width`/`work_height` are the display's usable area in logical
    /// points; the returned origin is relative to that area.
    pub fn new(work_width: f64, work_height: f64, inset: f64) -> Self {
        let width = (WIDTH + 2.0 * inset)
            .min(work_width - 2.0 * MARGIN)
            .max(1.0);
        let max_height = MAX_HEIGHT.min(work_height - 2.0 * MARGIN).max(1.0);
        // Keep the top edge fixed for every height the launcher can reach, so
        // it never moves upward on a short display.
        let y = (work_height * TOP)
            .min(work_height - max_height - MARGIN)
            .max(MARGIN.min(work_height - max_height).max(0.0));
        Self {
            x: ((work_width - width) / 2.0).max(0.0),
            y,
            width,
            max_height,
        }
    }

    /// The switcher's window: its plate and the room around it for a shadow,
    /// centred on the work area.
    pub fn switcher(work_width: f64, work_height: f64, inset: f64, count: usize) -> Self {
        let width = (switcher_width(count) + 2.0 * inset)
            .min(work_width - 2.0 * MARGIN)
            .max(1.0);
        let height = (SWITCHER_HEIGHT + 2.0 * inset)
            .min(work_height - 2.0 * MARGIN)
            .max(1.0);
        Self {
            x: ((work_width - width) / 2.0).max(0.0),
            y: ((work_height - height) / 2.0).max(0.0),
            width,
            max_height: height,
        }
    }

    pub fn height(&self, content: f64) -> f64 {
        content.clamp(RESTING_HEIGHT.min(self.max_height), self.max_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_centred_near_the_top_of_an_ordinary_display() {
        let placement = Placement::new(1440.0, 875.0, 0.0);
        assert_eq!(placement.width, WIDTH);
        assert_eq!(placement.x, (1440.0 - WIDTH) / 2.0);
        assert_eq!(placement.y, 875.0 * TOP);
        assert_eq!(placement.height(20.0), RESTING_HEIGHT);
        assert_eq!(placement.height(9000.0), MAX_HEIGHT);
    }

    #[test]
    fn the_switcher_is_as_wide_as_its_cards_and_centred() {
        assert_eq!(switcher_width(1), 2.0 * SWITCHER_PAD + SWITCHER_CARD);
        assert_eq!(switcher_width(9), switcher_width(SWITCHER_VISIBLE));
        assert_eq!(switcher_width(0), switcher_width(1));
        let placement = Placement::switcher(1440.0, 875.0, SHAPE_INSET, 3);
        assert_eq!(placement.width, switcher_width(3) + 2.0 * SHAPE_INSET);
        assert_eq!(placement.x, (1440.0 - placement.width) / 2.0);
        let height = placement.height(9000.0);
        assert_eq!(height, SWITCHER_HEIGHT + 2.0 * SHAPE_INSET);
        assert_eq!(placement.y, (875.0 - height) / 2.0);
    }

    #[test]
    fn the_tallest_launcher_still_fits_a_short_display() {
        let placement = Placement::new(1024.0, 600.0, SHAPE_INSET);
        assert!(placement.y >= MARGIN);
        assert!(placement.y + placement.height(9000.0) <= 600.0 - MARGIN);
    }

    #[test]
    fn a_tiny_display_bounds_width_and_height() {
        let placement = Placement::new(300.0, 120.0, SHAPE_INSET);
        assert!(placement.x >= 0.0 && placement.x + placement.width <= 300.0);
        assert!(placement.y >= 0.0 && placement.y + placement.height(9000.0) <= 120.0);
    }
}
