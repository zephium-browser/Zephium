//! The count of Work pages waiting on the person, drawn onto the taskbar
//! button. Windows gives a desktop app no badge count, only an overlay icon.

const SIZE: usize = 16;
const SCALE: usize = 2;
const BADGE: [u8; 4] = [0xC4, 0x2B, 0x1C, 0xFF];
const MARK: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// Three columns by five rows, one row per entry, high bit on the left.
fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 7, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 1, 1, 1],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        _ => [0, 2, 7, 2, 0],
    }
}

/// A rounded square holding the count, or "9+" above nine, as RGBA.
fn pixels(count: usize) -> Vec<u8> {
    let label = if count > 9 {
        "9+".to_owned()
    } else {
        count.to_string()
    };
    let glyphs: Vec<[u8; 5]> = label.chars().map(glyph).collect();
    let width = glyphs.len() * 3 * SCALE + (glyphs.len() - 1) * SCALE;
    let left = (SIZE - width) / 2;
    let top = (SIZE - 5 * SCALE) / 2;
    let mut rgba = vec![0; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            if !inside(x, y) {
                continue;
            }
            let ink = if marked(&glyphs, x.wrapping_sub(left), y.wrapping_sub(top)) {
                MARK
            } else {
                BADGE
            };
            let at = (y * SIZE + x) * 4;
            rgba[at..at + 4].copy_from_slice(&ink);
        }
    }
    rgba
}

/// Within the square once its corners are rounded off.
fn inside(x: usize, y: usize) -> bool {
    const RADIUS: f32 = 4.0;
    let edge = |v: usize| {
        let v = v as f32 + 0.5;
        if v < RADIUS {
            RADIUS - v
        } else if v > SIZE as f32 - RADIUS {
            v - (SIZE as f32 - RADIUS)
        } else {
            0.0
        }
    };
    let (dx, dy) = (edge(x), edge(y));
    dx * dx + dy * dy <= RADIUS * RADIUS
}

fn marked(glyphs: &[[u8; 5]], x: usize, y: usize) -> bool {
    let (column, row) = (x / SCALE, y / SCALE);
    let (index, within) = (column / 4, column % 4);
    row < 5
        && within < 3
        && glyphs
            .get(index)
            .is_some_and(|glyph| glyph[row] & (4 >> within) != 0)
}

/// Shows `count` on the taskbar button, or clears it at zero.
pub(crate) fn show(window: &tauri::WebviewWindow, count: usize) {
    let icon = (count > 0).then(|| tauri::image::Image::new_owned(pixels(count), 16, 16));
    if let Err(error) = window.set_overlay_icon(icon) {
        eprintln!("taskbar: the Work count was not shown: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
        let at = (y * SIZE + x) * 4;
        rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn the_badge_is_a_rounded_square_with_white_strokes() {
        let one = pixels(1);
        assert_eq!(at(&one, 0, 0), [0; 4]);
        assert_eq!(at(&one, 15, 15), [0; 4]);
        assert_eq!(at(&one, 0, 8), BADGE);
        // "1" is centred: its foot spans columns 5 to 10 on the bottom rows.
        for x in 5..11 {
            assert_eq!(at(&one, x, 12), MARK);
        }
        assert_eq!(at(&one, 4, 12), BADGE);
    }

    #[test]
    fn above_nine_reads_nine_plus() {
        let many = pixels(12);
        assert_eq!(many, pixels(250));
        assert_ne!(many, pixels(9));
        // The plus sign's bar sits on the middle row, right of the nine.
        for x in 9..15 {
            assert_eq!(at(&many, x, 7), MARK);
        }
    }
}
