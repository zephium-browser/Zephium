//! Where a hidden sidebar's card stands in physical pixels, and where its
//! shadow lies in a page's own window. Pure, so the Windows stage and the
//! tests share one answer.

use zephium_core::geometry::Rect;

/// `(x, y, width, height)` in physical pixels.
pub(crate) type PixelRect = (i32, i32, i32, i32);

/// The card cut to a client area of `client` pixels. Only its width is taken
/// as given: the window may have been resized since the card was asked for,
/// so it keeps its top inset at the bottom too.
pub(crate) fn card_pixels(card: Rect, client: (i32, i32), scale: f64) -> Option<PixelRect> {
    if !scale.is_finite()
        || scale <= 0.0
        || ![card.x, card.y, card.width].iter().all(|v| v.is_finite())
    {
        return None;
    }
    let x = (card.x * scale).round() as i32;
    let y = (card.y * scale).round() as i32;
    let width = ((card.width * scale).round() as i32).min(client.0 - x);
    let height = client.1 - 2 * y;
    (x >= 0 && y >= 0 && width > 0 && height > 0).then_some((x, y, width, height))
}

/// The card's rectangle in the coordinates of a page window placed at
/// `container` in the same space, as `(offset, size)`.
pub(crate) fn in_container(card: PixelRect, container: PixelRect) -> ((i32, i32), (i32, i32)) {
    (
        (card.0 - container.0, card.1 - container.1),
        (card.2, card.3),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_scales_with_the_display_and_keeps_its_insets() {
        let card = Rect::new(8.0, 8.0, 256.0, 784.0);
        assert_eq!(card_pixels(card, (1200, 800), 1.0), Some((8, 8, 256, 784)));
        assert_eq!(
            card_pixels(card, (1800, 1200), 1.5),
            Some((12, 12, 384, 1176))
        );
    }

    #[test]
    fn a_resized_window_moves_the_cards_bottom_and_a_narrow_one_trims_it() {
        let card = Rect::new(8.0, 8.0, 256.0, 784.0);
        assert_eq!(card_pixels(card, (1200, 600), 1.0), Some((8, 8, 256, 584)));
        assert_eq!(card_pixels(card, (200, 600), 1.0), Some((8, 8, 192, 584)));
    }

    #[test]
    fn nothing_is_cut_for_a_window_too_small_or_a_card_that_is_not_a_number() {
        let card = Rect::new(8.0, 8.0, 256.0, 784.0);
        assert_eq!(card_pixels(card, (1200, 16), 1.0), None);
        assert_eq!(card_pixels(card, (8, 800), 1.0), None);
        assert_eq!(card_pixels(card, (1200, 800), 0.0), None);
        assert_eq!(
            card_pixels(Rect::new(f64::NAN, 8.0, 256.0, 1.0), (1200, 800), 1.0),
            None
        );
    }

    #[test]
    fn the_shadow_lies_where_the_card_does_within_each_page_window() {
        assert_eq!(
            in_container((8, 8, 256, 784), (8, 8, 1184, 784)),
            ((0, 0), (256, 784))
        );
        // A pane further in sees the card start before its own left edge.
        assert_eq!(
            in_container((8, 8, 256, 784), (400, 40, 700, 500)),
            ((-392, -32), (256, 784))
        );
    }
}
