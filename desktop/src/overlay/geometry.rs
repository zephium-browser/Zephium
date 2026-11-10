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
/// The tab switcher is one plate holding a grid of page cards, as wide and
/// tall as the cards it shows and centred on the display. Past the last full
/// row the next one peeks in, so there is plainly more to scroll to. The frame
/// lays its cards out from the same numbers.
pub const SWITCHER_CARD: f64 = 204.0;
pub const SWITCHER_CARD_HEIGHT: f64 = 156.0;
pub const SWITCHER_GAP: f64 = 4.0;
pub const SWITCHER_PAD: f64 = 12.0;
pub const SWITCHER_COLUMNS: usize = 4;
pub const SWITCHER_ROWS: usize = 3;
pub const SWITCHER_PEEK: f64 = 36.0;
/// The plate's corner, concentric with a card's 14 at its padding. Only the
/// macOS plate is drawn natively.
#[cfg(target_os = "macos")]
pub const SWITCHER_RADIUS: f64 = 26.0;

/// How many cards across and rows down the plate shows for `tabs` tabs and
/// the new tab card after them, and whether more wait below.
/// How many cards stand across on a work area this wide: four, or as many
/// as fit inside its margins, so a narrow or highly scaled display never cuts
/// a column off. The frame derives the same count from the window's width.
pub fn switcher_columns(work_width: f64, inset: f64) -> usize {
    let room = work_width - 2.0 * MARGIN - 2.0 * inset - 2.0 * SWITCHER_PAD + SWITCHER_GAP;
    let fit = (room / (SWITCHER_CARD + SWITCHER_GAP)).floor();
    if fit.is_finite() && fit >= 1.0 {
        (fit as usize).min(SWITCHER_COLUMNS)
    } else {
        1
    }
}

pub fn switcher_grid(tabs: usize, columns: usize) -> (usize, usize, bool) {
    let columns = columns.clamp(1, SWITCHER_COLUMNS);
    let count = tabs + 1;
    let rows = count.div_ceil(columns);
    (
        count.min(columns),
        rows.min(SWITCHER_ROWS),
        rows > SWITCHER_ROWS,
    )
}

pub fn switcher_size(tabs: usize, columns: usize) -> (f64, f64) {
    let (columns, rows, more) = switcher_grid(tabs, columns);
    let span = |cells: usize, cell: f64| cells as f64 * cell + (cells - 1) as f64 * SWITCHER_GAP;
    (
        2.0 * SWITCHER_PAD + span(columns, SWITCHER_CARD),
        2.0 * SWITCHER_PAD
            + span(rows, SWITCHER_CARD_HEIGHT)
            + if more {
                SWITCHER_GAP + SWITCHER_PEEK
            } else {
                0.0
            },
    )
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
        let (width, height) = switcher_size(count, switcher_columns(work_width, inset));
        let width = (width + 2.0 * inset)
            .min(work_width - 2.0 * MARGIN)
            .max(1.0);
        let height = (height + 2.0 * inset)
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
    fn the_switcher_grows_with_its_cards_up_to_a_grid_and_is_centred() {
        let four = SWITCHER_COLUMNS;
        assert_eq!(switcher_grid(0, four), (1, 1, false));
        assert_eq!(switcher_grid(2, four), (3, 1, false));
        assert_eq!(switcher_grid(4, four), (4, 2, false));
        assert_eq!(switcher_grid(11, four), (4, 3, false));
        assert_eq!(switcher_grid(12, four), (4, 3, true));
        assert_eq!(
            switcher_size(0, four),
            (
                2.0 * SWITCHER_PAD + SWITCHER_CARD,
                2.0 * SWITCHER_PAD + SWITCHER_CARD_HEIGHT
            )
        );
        assert_eq!(switcher_size(40, four).0, switcher_size(3, four).0);
        assert_eq!(
            switcher_size(40, four).1,
            switcher_size(11, four).1 + SWITCHER_GAP + SWITCHER_PEEK
        );
        let placement = Placement::switcher(1440.0, 875.0, SHAPE_INSET, 6);
        let (width, height) = switcher_size(6, four);
        assert_eq!(placement.width, width + 2.0 * SHAPE_INSET);
        assert_eq!(placement.x, (1440.0 - placement.width) / 2.0);
        assert_eq!(placement.height(f64::INFINITY), height + 2.0 * SHAPE_INSET);
        assert_eq!(placement.y, (875.0 - height - 2.0 * SHAPE_INSET) / 2.0);
    }

    #[test]
    fn a_narrow_or_highly_scaled_display_stands_fewer_cards_across() {
        // 1920 px at 250% is 768 points wide.
        assert_eq!(switcher_columns(1440.0, SHAPE_INSET), 4);
        assert_eq!(switcher_columns(768.0, 0.0), 3);
        assert_eq!(switcher_columns(768.0, SHAPE_INSET), 2);
        assert_eq!(switcher_columns(200.0, 0.0), 1);
        for (work, inset) in [(768.0, 0.0), (768.0, SHAPE_INSET), (1024.0, 0.0)] {
            let placement = Placement::switcher(work, 900.0, inset, 30);
            let (width, _) = switcher_size(30, switcher_columns(work, inset));
            assert_eq!(placement.width, width + 2.0 * inset);
            assert!(placement.x >= MARGIN);
        }
        assert_eq!(switcher_grid(30, 2), (2, 3, true));
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
