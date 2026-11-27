//! Small, cached premultiplied-alpha caption surface. Native BUTTONs own input
//! and accessibility separately; no theme brush can enter this visible bitmap.
use windows::Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*};

pub const HEIGHT: f64 = 32.0;
pub const BUTTON_WIDTH: f64 = 44.0;
pub const PADDING: f64 = 3.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Appearance {
    pub dpi: u32,
    pub dark: bool,
    pub active: bool,
    pub maximized: bool,
    pub hot: Option<usize>,
    pub pressed: Option<usize>,
    pub focused: Option<usize>,
    /// A button the window's styles withdraw is drawn dim and never lights.
    pub enabled: [bool; 3],
    /// Window, text, highlight, highlighted text and disabled text, in
    /// COLORREF order.
    pub contrast: Option<[u32; 5]>,
}

pub fn dimensions(dpi: u32) -> (i32, i32, i32, i32) {
    let scale = dpi.max(96) as f64 / 96.0;
    let button = (BUTTON_WIDTH * scale).round() as i32;
    let padding = (PADDING * scale).round() as i32;
    (
        button
            .saturating_mul(3)
            .saturating_add(padding.saturating_mul(2)),
        (HEIGHT * scale).round() as i32,
        button,
        padding,
    )
}

pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    size: (i32, i32),
    appearance: Option<Appearance>,
    presented: Option<(i32, i32, i32, u8)>,
}

impl Surface {
    pub fn new(dpi: u32) -> Option<Self> {
        let (width, height, _, _) = dimensions(dpi);
        // Bound a native allocation even if a malformed DPI reaches this adapter.
        if width > 4096 || height > 1024 {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            let dc = CreateCompatibleDC(None);
            if dc.0.is_null() || bits.is_null() {
                let _ = DeleteObject(bitmap.into());
                if !dc.0.is_null() {
                    let _ = DeleteDC(dc);
                }
                return None;
            }
            let previous = SelectObject(dc, bitmap.into());
            if previous.0.is_null() || previous.0 as isize == -1 {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(bitmap.into());
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                previous,
                bits: bits.cast(),
                size: (width, height),
                appearance: None,
                presented: None,
            })
        }
    }

    pub fn present(
        &mut self,
        hwnd: HWND,
        position: POINT,
        shown: i32,
        alpha: u8,
        appearance: Appearance,
    ) -> bool {
        let (width, height, _, _) = dimensions(appearance.dpi);
        if self.size != (width, height) {
            let Some(replacement) = Self::new(appearance.dpi) else {
                return false;
            };
            *self = replacement;
        }
        let shown = shown.clamp(0, height);
        if shown == 0 || alpha == 0 {
            return true;
        }
        if self.appearance != Some(appearance) {
            // This buffer is private to the UI thread and remains selected into
            // our memory DC until Drop. Animation reuses it without rasterizing.
            let pixels =
                unsafe { std::slice::from_raw_parts_mut(self.bits, (width * height) as usize) };
            raster(pixels, appearance);
            self.appearance = Some(appearance);
            self.presented = None;
        }
        let presentation = (position.x, position.y, shown, alpha);
        if self.presented == Some(presentation) {
            return true;
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // Crop the travelling portion rather than using a binary window region.
        let result = unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&position),
                Some(&SIZE {
                    cx: width,
                    cy: shown,
                }),
                Some(self.dc),
                Some(&POINT {
                    x: 0,
                    y: height - shown,
                }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        };
        if result.is_ok() {
            self.presented = Some(presentation);
        }
        result.is_ok()
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

fn rounded_distance(x: f64, y: f64, half_width: f64, half_height: f64, radius: f64) -> f64 {
    let dx = x.abs() - half_width + radius;
    let dy = y.abs() - half_height + radius;
    dx.max(dy).min(0.0) + dx.max(0.0).hypot(dy.max(0.0)) - radius
}

fn line_distance(x: f64, y: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let vx = b.0 - a.0;
    let vy = b.1 - a.1;
    let t = (((x - a.0) * vx + (y - a.1) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
    (x - a.0 - t * vx).hypot(y - a.1 - t * vy)
}

fn ink_distance(index: usize, maximized: bool, x: f64, y: f64) -> f64 {
    match index {
        0 => line_distance(x, y, (-4.5, 0.5), (4.5, 0.5)),
        1 if maximized => {
            let front = (x + 1.0).abs().max((y - 1.0).abs());
            (front - 3.5)
                .abs()
                .min(line_distance(x, y, (-2.5, -4.5), (4.5, -4.5)))
                .min(line_distance(x, y, (4.5, -4.5), (4.5, 2.5)))
        }
        1 => (x.abs().max(y.abs()) - 4.0).abs(),
        _ => line_distance(x, y, (-4.0, -4.0), (4.0, 4.0)).min(line_distance(
            x,
            y,
            (4.0, -4.0),
            (-4.0, 4.0),
        )),
    }
}

fn color(value: u32) -> [f64; 3] {
    [
        (value & 255) as f64,
        ((value >> 8) & 255) as f64,
        ((value >> 16) & 255) as f64,
    ]
}

fn over(pixel: &mut [f64; 4], rgb: [f64; 3], alpha: f64) {
    for channel in 0..3 {
        pixel[channel] = rgb[channel] * alpha + pixel[channel] * (1.0 - alpha);
    }
    pixel[3] = alpha * 255.0 + pixel[3] * (1.0 - alpha);
}

fn raster(pixels: &mut [u32], appearance: Appearance) {
    let (width, height, button, padding) = dimensions(appearance.dpi);
    let scale = appearance.dpi.max(96) as f64 / 96.0;
    for row in 0..height {
        for column in 0..width {
            let mut pixel = [0.0; 4];
            if let Some(colors) = appearance.contrast {
                over(&mut pixel, color(colors[0]), 1.0);
            }
            let index = (column - padding).div_euclid(button);
            if (0..3).contains(&index) {
                let index = index as usize;
                let x = (column as f64 + 0.5
                    - (padding + index as i32 * button) as f64
                    - button as f64 / 2.0)
                    / scale;
                let y = (row as f64 + 0.5 - height as f64 / 2.0) / scale;
                let live = appearance.enabled[index];
                let hot = live && appearance.hot == Some(index);
                let pressed = appearance.pressed == Some(index) && hot;
                let edge = rounded_distance(
                    x,
                    y,
                    button as f64 / scale / 2.0 - 2.0,
                    HEIGHT / 2.0 - PADDING,
                    4.0,
                ) * scale;
                let coverage = (0.5 - edge).clamp(0.0, 1.0);
                if hot {
                    let (rgb, alpha) = if let Some(colors) = appearance.contrast {
                        (color(colors[2]), 1.0)
                    } else if index == 2 {
                        ([196.0, 43.0, 28.0], if pressed { 0.85 } else { 1.0 })
                    } else {
                        (
                            [if appearance.dark { 255.0 } else { 0.0 }; 3],
                            if pressed { 0.18 } else { 0.10 },
                        )
                    };
                    over(&mut pixel, rgb, alpha * coverage);
                }
                let ink = if let Some(colors) = appearance.contrast {
                    let slot = if !live {
                        4
                    } else if hot {
                        3
                    } else {
                        1
                    };
                    color(colors[slot])
                } else {
                    [if appearance.dark || (hot && index == 2) {
                        255.0
                    } else {
                        24.0
                    }; 3]
                };
                let ink_alpha = if appearance.contrast.is_some() {
                    1.0
                } else if !live {
                    0.36
                } else if hot || appearance.active {
                    0.9
                } else {
                    0.65
                };
                let stroke = (0.5 + 0.5 * scale
                    - ink_distance(index, appearance.maximized, x, y) * scale)
                    .clamp(0.0, 1.0);
                over(&mut pixel, ink, ink_alpha * stroke);
                if appearance.focused == Some(index) {
                    let ring = (0.5 + 0.5 * scale - (edge + scale).abs()).clamp(0.0, 1.0);
                    over(&mut pixel, ink, ring * 0.9);
                }
            }
            let [r, g, b, a] = pixel.map(|value| value.round().clamp(0.0, 255.0) as u32);
            pixels[(row * width + column) as usize] = (a << 24) | (r << 16) | (g << 8) | b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_alpha_surface_accepts_cropped_frames_and_dpi_changes() {
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                let _ = unsafe { DestroyWindow(self.0) };
            }
        }
        let window = Window(
            unsafe {
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                    windows::core::w!("STATIC"),
                    windows::core::w!(""),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("hidden test surface"),
        );
        let mut surface = Surface::new(96).expect("caption DIB");
        for dpi in [96, 120, 144, 192, 96] {
            let appearance = Appearance {
                dpi,
                dark: true,
                active: true,
                maximized: false,
                hot: Some(1),
                pressed: None,
                focused: None,
                enabled: [true; 3],
                contrast: None,
            };
            let (_, height, _, _) = dimensions(dpi);
            for shown in [1, height / 2, height] {
                assert!(surface.present(window.0, POINT { x: 0, y: 0 }, shown, 255, appearance));
            }
        }
        assert!(!unsafe { IsWindowVisible(window.0) }.as_bool());
    }
    #[test]
    fn caption_pixels_have_real_transparency_and_premultiplied_edges_at_every_dpi() {
        for dpi in [96, 120, 144, 192] {
            let appearance = Appearance {
                dpi,
                dark: true,
                active: true,
                maximized: false,
                hot: Some(1),
                pressed: None,
                focused: None,
                enabled: [true; 3],
                contrast: None,
            };
            let (width, height, _, _) = dimensions(dpi);
            let mut pixels = vec![0; (width * height) as usize];
            raster(&mut pixels, appearance);
            assert_eq!(pixels[0], 0);
            assert!(pixels.iter().any(|pixel| (1..255).contains(&(pixel >> 24))));
            for pixel in pixels {
                let alpha = pixel >> 24;
                assert!(((pixel >> 16) & 255) <= alpha);
                assert!(((pixel >> 8) & 255) <= alpha);
                assert!((pixel & 255) <= alpha);
            }
        }
    }
    #[test]
    fn a_withdrawn_button_is_dim_and_never_lights() {
        let appearance = |hot, enabled| Appearance {
            dpi: 96,
            dark: true,
            active: true,
            maximized: false,
            hot,
            pressed: None,
            focused: None,
            enabled,
            contrast: None,
        };
        let (width, height, _, _) = dimensions(96);
        let render = |appearance| {
            let mut pixels = vec![0; (width * height) as usize];
            raster(&mut pixels, appearance);
            pixels
        };
        let withdrawn = [true, false, true];
        assert_eq!(
            render(appearance(Some(1), withdrawn)),
            render(appearance(None, withdrawn))
        );
        // Ink is the only thing painted when the pointer is elsewhere.
        let ink = |pixels: &[u32]| {
            pixels
                .iter()
                .map(|pixel| u64::from(pixel >> 24))
                .sum::<u64>()
        };
        assert!(
            ink(&render(appearance(None, withdrawn))) < ink(&render(appearance(None, [true; 3])))
        );
    }
    #[test]
    fn caption_bitmap_is_bounded_and_high_contrast_is_opaque() {
        let appearance = Appearance {
            dpi: 192,
            dark: true,
            active: true,
            maximized: true,
            hot: Some(2),
            pressed: Some(2),
            focused: Some(0),
            enabled: [true, false, true],
            contrast: Some([0, 0xffffff, 0xffffff, 0, 0x808080]),
        };
        let (width, height, _, _) = dimensions(appearance.dpi);
        let mut pixels = vec![0; (width * height) as usize];
        assert!(pixels.len() * size_of::<u32>() < 72 * 1024);
        raster(&mut pixels, appearance);
        assert!(pixels.iter().all(|pixel| pixel >> 24 == 255));
    }
}
