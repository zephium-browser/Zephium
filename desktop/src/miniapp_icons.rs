//! Bounded, higher-resolution icon rasters for installed mini-apps.
//!
//! Browser page favicons keep their fixed 32x32 IPC contract. Installed app
//! icons are trusted only after the same strict byte, format, dimension and
//! allocation limits, then rasterized separately for native shortcuts.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, RgbaImage};
use zephium_core::icon::{ICON_SIDE, RGBA32_BYTES};

const NATIVE_ICON_SIDE: u32 = 256;
const MAX_DECODED_SIDE: u32 = 1024;
const MAX_DECODE_ALLOC: u64 = 16 * 1024 * 1024;

/// Decodes a public app icon into a transparent 256x256 raster with bounded
/// memory use. The input byte limit is enforced by the caller before decoding.
pub(crate) fn rasterize(bytes: &[u8]) -> Option<(Vec<u8>, RgbaImage)> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if !matches!(
        reader.format()?,
        ImageFormat::Png
            | ImageFormat::Ico
            | ImageFormat::Gif
            | ImageFormat::WebP
            | ImageFormat::Jpeg
            | ImageFormat::Bmp
    ) {
        return None;
    }

    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);

    let decoded = reader.decode().ok()?.into_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0 || height == 0 || width > MAX_DECODED_SIDE || height > MAX_DECODED_SIDE {
        return None;
    }

    let fit = |side: u32, filter: image::imageops::FilterType| {
        let scale = (f64::from(side) / f64::from(width)).min(f64::from(side) / f64::from(height));
        let draw_width = (f64::from(width) * scale)
            .round()
            .clamp(1.0, f64::from(side)) as u32;
        let draw_height = (f64::from(height) * scale)
            .round()
            .clamp(1.0, f64::from(side)) as u32;
        let scaled = image::imageops::resize(&decoded, draw_width, draw_height, filter);
        let mut canvas = RgbaImage::new(side, side);
        image::imageops::overlay(
            &mut canvas,
            &scaled,
            i64::from((side - draw_width) / 2),
            i64::from((side - draw_height) / 2),
        );
        canvas
    };

    let favicon = fit(ICON_SIDE as u32, image::imageops::FilterType::Triangle).into_raw();
    if favicon.len() != RGBA32_BYTES {
        return None;
    }
    Some((
        favicon,
        fit(NATIVE_ICON_SIDE, image::imageops::FilterType::Lanczos3),
    ))
}

#[cfg(test)]
mod tests {
    use super::{rasterize, MAX_DECODED_SIDE};
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_pixel(width, height, Rgba([230, 40, 20, 255]));
        let mut output = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut output, ImageFormat::Png)
            .expect("encode fixture");
        output.into_inner()
    }

    #[test]
    fn rasterizes_png_to_256_with_transparent_padding() {
        let (_, icon) = rasterize(&png(64, 32)).expect("valid PNG");
        assert_eq!(icon.dimensions(), (256, 256));
        assert_eq!(icon.get_pixel(128, 8).0[3], 0);
        assert!(icon.get_pixel(128, 128).0[3] > 0);
    }

    #[test]
    fn rejects_unsupported_and_zero_sized_images() {
        assert!(rasterize(b"not an image").is_none());
        let mut zero_width_png = png(1, 8);
        zero_width_png[16..20].copy_from_slice(&0_u32.to_be_bytes());
        assert!(rasterize(&zero_width_png).is_none());
    }

    #[test]
    fn rejects_dimensions_over_limit() {
        assert!(rasterize(&png(MAX_DECODED_SIDE + 1, 1)).is_none());
    }

    #[test]
    fn preserves_non_square_aspect_ratio() {
        let (_, icon) = rasterize(&png(80, 40)).expect("valid PNG");
        let visible_rows = (0..256)
            .filter(|y| (0..256).any(|x| icon.get_pixel(x, *y).0[3] > 0))
            .count();
        let visible_columns = (0..256)
            .filter(|x| (0..256).any(|y| icon.get_pixel(*x, y).0[3] > 0))
            .count();
        assert!(visible_columns > visible_rows);
        assert!(visible_columns >= visible_rows * 2 - 4);
    }
}
