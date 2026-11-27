//! Bounded, higher-resolution icon rasters for installed mini-apps.
//!
//! Browser page favicons keep their fixed 32x32 IPC contract. Installed app
//! icons are trusted only after the same strict byte, format, dimension and
//! allocation limits, then rasterized separately for native shortcuts.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, RgbaImage};

const NATIVE_ICON_SIDE: u32 = 256;
const MAX_DECODED_SIDE: u32 = 1024;
const MAX_DECODE_ALLOC: u64 = 16 * 1024 * 1024;

/// Decodes a public app icon into a transparent 256x256 raster with bounded
/// memory use. The input byte limit is enforced by the caller before decoding.
pub(crate) fn rasterize(bytes: &[u8]) -> Option<RgbaImage> {
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

    let scale = (f64::from(NATIVE_ICON_SIDE) / f64::from(width))
        .min(f64::from(NATIVE_ICON_SIDE) / f64::from(height));
    let draw_width = (f64::from(width) * scale).round().clamp(1.0, 256.0) as u32;
    let draw_height = (f64::from(height) * scale).round().clamp(1.0, 256.0) as u32;
    let scaled = image::imageops::resize(
        &decoded,
        draw_width,
        draw_height,
        image::imageops::FilterType::Lanczos3,
    );

    let mut canvas = RgbaImage::new(NATIVE_ICON_SIDE, NATIVE_ICON_SIDE);
    image::imageops::overlay(
        &mut canvas,
        &scaled,
        i64::from((NATIVE_ICON_SIDE - draw_width) / 2),
        i64::from((NATIVE_ICON_SIDE - draw_height) / 2),
    );
    Some(canvas)
}
