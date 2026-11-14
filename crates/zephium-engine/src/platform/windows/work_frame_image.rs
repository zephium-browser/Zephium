//! Person-facing Work thumbnails. No model evidence or browser authority.
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(not(test), deny(clippy::panic, clippy::unwrap_used))]

use image::{ImageEncoder, ImageFormat, ImageReader};
use std::io::{self, Cursor, Write};

pub(super) use crate::platform::preview_image::{input_dimensions, RAW_BYTES as RAW_PNG_BYTES};
use crate::platform::preview_image::{DECODE_BYTES, INPUT_SIDE};
const OUTPUT_SIDE: u32 = 640;
pub(super) const OUTPUT_PNG_BYTES: usize = 1024 * 1024;
pub(super) type FrameImage = Option<(u32, u32, Vec<u8>)>;

/// The source is the actual physical controller bounds, including DPI.
/// A larger native viewport is refused before allocating its capture stream.
fn output_dimensions(width: u32, height: u32) -> Option<(u32, u32)> {
    if !input_dimensions(width, height) {
        return None;
    }
    let longest = width.max(height);
    if longest <= OUTPUT_SIDE {
        return Some((width, height));
    }
    Some((
        (u64::from(width) * u64::from(OUTPUT_SIDE) / u64::from(longest)).max(1) as u32,
        (u64::from(height) * u64::from(OUTPUT_SIDE) / u64::from(longest)).max(1) as u32,
    ))
}

/// Only WebView2's fixed RGB/RGBA8 PNG can enter the decoder. Dimensions are
/// checked before decompression; the codec additionally enforces its allocation
/// ceiling and verifies the PNG structure/CRC. The input Vec is consumed here.
pub(super) fn thumbnail(png: Vec<u8>, expected: (u32, u32)) -> FrameImage {
    encode_image(png, expected, None)
}
pub(super) fn region(
    png: Vec<u8>,
    expected: (u32, u32),
    crop: crate::platform::record_media::Crop,
) -> FrameImage {
    encode_image(png, expected, Some(crop))
}
pub(super) fn region_geometry(
    physical: (u32, u32),
    crop: crate::platform::record_media::Crop,
) -> Option<zephium_agentic::WorkBrowserCropGeometry> {
    let (vw, vh) = crop.proof.viewport();
    let [x, y, w, h] = crop.rect;
    if !input_dimensions(physical.0, physical.1)
        || !crop.proof.clear()
        || ![vw, vh, x, y, w, h].iter().all(|v| v.is_finite())
        || vw <= 0.0
        || vh <= 0.0
        || x < 0.0
        || y < 0.0
        || w <= 0.0
        || h <= 0.0
        || x + w > vw
        || y + h > vh
    {
        return None;
    }
    let (sx, sy) = (f64::from(physical.0) / vw, f64::from(physical.1) / vh);
    if (sx - sy).abs() > 0.005 {
        return None;
    }
    let left = (x * sx).ceil() as u32;
    let top = (y * sy).ceil() as u32;
    let width = ((x + w) * sx).floor() as u32;
    let height = ((y + h) * sy).floor() as u32;
    let (width, height) = (width.checked_sub(left)?, height.checked_sub(top)?);
    if width == 0
        || height == 0
        || left.checked_add(width)? > physical.0
        || top.checked_add(height)? > physical.1
    {
        return None;
    }
    Some(zephium_agentic::WorkBrowserCropGeometry {
        captured: [physical.0, physical.1],
        viewport: [vw, vh],
        css_rect: crop.rect,
        raster_rect: [left, top, width, height],
    })
}
fn encode_image(
    png: Vec<u8>,
    expected: (u32, u32),
    crop: Option<crate::platform::record_media::Crop>,
) -> FrameImage {
    if png.len() > RAW_PNG_BYTES || !input_dimensions(expected.0, expected.1) {
        return None;
    }
    let header = png.get(..33)?;
    if header.get(..8)? != b"\x89PNG\r\n\x1a\n"
        || header.get(8..12)? != 13_u32.to_be_bytes()
        || header.get(12..16)? != b"IHDR"
        || header[24] != 8
        || !matches!(header[25], 2 | 6)
        || header[26..29] != [0, 0, 0]
    {
        return None;
    }
    let actual = (
        u32::from_be_bytes(header.get(16..20)?.try_into().ok()?),
        u32::from_be_bytes(header.get(20..24)?.try_into().ok()?),
    );
    if actual != expected {
        return None;
    }
    let geometry = match crop {
        Some(crop) => Some(region_geometry(actual, crop)?),
        None => None,
    };
    let (width, height) = if let Some(geometry) = geometry {
        let [_, _, w, h] = geometry.raster_rect;
        let fit = 1.0_f64
            .min(geometry.css_rect[2].min(320.0) / f64::from(w))
            .min(geometry.css_rect[3].min(240.0) / f64::from(h));
        (
            ((f64::from(w) * fit).floor() as u32).max(1),
            ((f64::from(h) * fit).floor() as u32).max(1),
        )
    } else {
        output_dimensions(actual.0, actual.1)?
    };
    let mut reader = ImageReader::with_format(Cursor::new(png), ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(INPUT_SIDE);
    limits.max_image_height = Some(INPUT_SIDE);
    limits.max_alloc = Some(DECODE_BYTES);
    reader.limits(limits);
    let decoded = reader.decode().ok()?;
    if decoded.width() != actual.0 || decoded.height() != actual.1 {
        return None;
    }

    // Fractional area weights preserve equal footprints at non-integral DPI
    // ratios (e.g. 1600 -> 640). Premultiplied color avoids alpha fringes.
    let (bytes, channels) = match &decoded {
        image::DynamicImage::ImageRgb8(image) => (image.as_raw().as_slice(), 3_usize),
        image::DynamicImage::ImageRgba8(image) => (image.as_raw().as_slice(), 4_usize),
        _ => return None,
    };
    let small = if let Some(geometry) = geometry {
        area_rgba_region(
            bytes,
            actual,
            channels,
            (width, height),
            geometry.raster_rect,
        )?
    } else {
        area_rgba(bytes, actual, channels, (width, height))?
    };
    drop(decoded);
    let mut encoded = OutputSink(Vec::new());
    image::codecs::png::PngEncoder::new(&mut encoded)
        .write_image(&small, width, height, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some((width, height, encoded.0))
}

/// Exact integer overlap in units of 1/target source pixels. Each axis has
/// at most source+target entries, so the tables remain below 160KiB in total.
fn axis_weights(source: u32, target: u32) -> Vec<Vec<(u32, u64)>> {
    (0..target)
        .map(|out| {
            let left = u64::from(out) * u64::from(source);
            let right = u64::from(out + 1) * u64::from(source);
            let start = left / u64::from(target);
            let end = right.div_ceil(u64::from(target));
            (start..end)
                .map(|pixel| {
                    let pixel_left = pixel * u64::from(target);
                    let pixel_right = (pixel + 1) * u64::from(target);
                    (pixel as u32, right.min(pixel_right) - left.max(pixel_left))
                })
                .collect()
        })
        .collect()
}

fn area_rgba(
    bytes: &[u8],
    source: (u32, u32),
    channels: usize,
    target: (u32, u32),
) -> Option<Vec<u8>> {
    area_rgba_region(bytes, source, channels, target, [0, 0, source.0, source.1])
}
fn area_rgba_region(
    bytes: &[u8],
    source: (u32, u32),
    channels: usize,
    target: (u32, u32),
    region: [u32; 4],
) -> Option<Vec<u8>> {
    let [left, top, width, height] = region;
    if width == 0
        || height == 0
        || left.checked_add(width)? > source.0
        || top.checked_add(height)? > source.1
    {
        return None;
    }
    let horizontal = axis_weights(width, target.0);
    let vertical = axis_weights(height, target.1);
    let mut small = vec![0_u8; target.0 as usize * target.1 as usize * 4];
    let total = u64::from(width) * u64::from(height);
    for (y, rows) in vertical.iter().enumerate() {
        for (x, columns) in horizontal.iter().enumerate() {
            let mut sums = [0_u64; 4];
            for &(sy, weight_y) in rows {
                for &(sx, weight_x) in columns {
                    let start =
                        ((sy + top) as usize * source.0 as usize + (sx + left) as usize) * channels;
                    let pixel = bytes.get(start..start + channels)?;
                    let alpha = if channels == 4 {
                        u64::from(pixel[3])
                    } else {
                        255
                    };
                    let weighted_alpha = alpha * weight_x * weight_y;
                    sums[0] += u64::from(pixel[0]) * weighted_alpha;
                    sums[1] += u64::from(pixel[1]) * weighted_alpha;
                    sums[2] += u64::from(pixel[2]) * weighted_alpha;
                    sums[3] += weighted_alpha;
                }
            }
            let out = (y * target.0 as usize + x) * 4;
            for channel in 0..3 {
                small[out + channel] = (sums[channel] + sums[3] / 2)
                    .checked_div(sums[3])
                    .unwrap_or(0) as u8;
            }
            small[out + 3] = ((sums[3] + total / 2) / total) as u8;
        }
    }
    Some(small)
}

struct OutputSink(Vec<u8>);
impl Write for OutputSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let end = self
            .0
            .len()
            .checked_add(bytes.len())
            .filter(|end| *end <= OUTPUT_PNG_BYTES)
            .ok_or_else(|| io::Error::other("Work frame output exceeds its byte ceiling"))?;
        self.0
            .try_reserve_exact(end - self.0.len())
            .map_err(io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "work_frame_image_tests.rs"]
mod tests;
