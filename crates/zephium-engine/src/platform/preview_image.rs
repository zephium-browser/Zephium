use image::{ImageFormat, ImageReader};
use std::io::Cursor;

pub(crate) const RAW_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const INPUT_SIDE: u32 = 4096;
pub(crate) const DECODE_BYTES: u64 = 48 * 1024 * 1024;
const INPUT_PIXELS: u64 = 3840 * 2400;
const OUTPUT_SIDE: u32 = 480;
const OUTPUT_BYTES: usize = 80 * 1024;
const BACKDROP_BYTES: usize = 8 * 1024 * 1024;

pub(crate) fn input_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= INPUT_SIDE
        && height <= INPUT_SIDE
        && u64::from(width) * u64::from(height) <= INPUT_PIXELS
}

pub(crate) fn thumbnail(jpeg: Vec<u8>, expected: (u32, u32)) -> Option<Vec<u8>> {
    if jpeg.len() > RAW_BYTES || !input_dimensions(expected.0, expected.1) {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(jpeg.as_slice()), ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(INPUT_SIDE);
    limits.max_image_height = Some(INPUT_SIDE);
    limits.max_alloc = Some(DECODE_BYTES);
    reader.limits(limits);
    let decoder = reader.into_decoder().ok()?;
    if image::ImageDecoder::dimensions(&decoder) != expected {
        return None;
    }
    let decoded = image::DynamicImage::from_decoder(decoder).ok()?;
    drop(jpeg);
    let small = decoded.thumbnail(OUTPUT_SIDE, OUTPUT_SIDE).to_rgb8();
    drop(decoded);
    let mut output = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 80)
        .encode(
            small.as_raw(),
            small.width(),
            small.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    (output.len() <= OUTPUT_BYTES).then_some(output)
}

pub(crate) fn backdrop(jpeg: Vec<u8>, expected: (u32, u32)) -> Option<Vec<u8>> {
    if jpeg.len() > BACKDROP_BYTES || !input_dimensions(expected.0, expected.1) {
        return None;
    }
    let mut reader = ImageReader::with_format(Cursor::new(jpeg.as_slice()), ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(INPUT_SIDE);
    limits.max_image_height = Some(INPUT_SIDE);
    limits.max_alloc = Some(DECODE_BYTES);
    reader.limits(limits);
    let matches = reader
        .into_decoder()
        .is_ok_and(|decoder| image::ImageDecoder::dimensions(&decoder) == expected);
    matches.then_some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_is_jpeg_preserves_aspect_and_refuses_invalid_capture() {
        let source = image::RgbImage::from_pixel(1200, 800, image::Rgb([30, 50, 90]));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
            .encode_image(&source)
            .unwrap();
        assert!(thumbnail(jpeg.clone(), (800, 1200)).is_none());
        let preview = thumbnail(jpeg, (1200, 800)).unwrap();
        let decoded = image::load_from_memory(&preview).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (480, 320));
        assert_eq!(image::guess_format(&preview).unwrap(), ImageFormat::Jpeg);
        assert!(thumbnail(vec![0; 10], (1200, 800)).is_none());
        assert!(thumbnail(vec![0; RAW_BYTES + 1], (1200, 800)).is_none());
        assert!(!input_dimensions(4096, 4096));
        assert!(!input_dimensions(0, 10));
    }

    #[test]
    fn backdrop_keeps_the_full_capture_and_refuses_a_mismatch() {
        let source = image::RgbImage::from_pixel(1600, 900, image::Rgb([30, 50, 90]));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
            .encode_image(&source)
            .unwrap();
        assert_eq!(backdrop(jpeg.clone(), (1600, 900)), Some(jpeg.clone()));
        assert!(backdrop(jpeg, (900, 1600)).is_none());
        assert!(backdrop(vec![0; 10], (1600, 900)).is_none());
        assert!(backdrop(vec![0; BACKDROP_BYTES + 1], (1600, 900)).is_none());
    }
}
