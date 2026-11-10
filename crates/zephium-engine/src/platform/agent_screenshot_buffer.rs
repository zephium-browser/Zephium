#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! Platform-neutral bounded storage for native screenshot encoders.
//!
//! WebView2 requires a caller-provided seekable COM stream. Keeping its byte
//! and cursor rules independent of COM makes every allocation and boundary
//! executable on non-Windows CI without capturing or retaining page pixels.

#[cfg(feature = "agentic-browser")]
use zephium_agentic::SemanticScreenshotBudget;

#[cfg(feature = "agentic-browser")]
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
#[cfg(feature = "agentic-browser")]
const IHDR_DATA_BYTES: u32 = 13;
const MIN_BUFFER_GROWTH: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScreenshotBufferFailure {
    Limit,
    Allocation,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScreenshotSeekOrigin {
    Start,
    Current,
    End,
}

#[cfg(feature = "agentic-browser")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PngHeaderFailure {
    Malformed,
    ResourceExhausted,
}

pub(super) struct BoundedScreenshotBuffer {
    bytes: Vec<u8>,
    cursor: usize,
    limit: usize,
}

impl BoundedScreenshotBuffer {
    pub(super) const fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            cursor: 0,
            limit,
        }
    }

    pub(super) fn write(&mut self, source: &[u8]) -> Result<usize, ScreenshotBufferFailure> {
        let end = self
            .cursor
            .checked_add(source.len())
            .ok_or(ScreenshotBufferFailure::Limit)?;
        if end > self.limit {
            return Err(ScreenshotBufferFailure::Limit);
        }
        // `ISequentialStream::Write` with a zero byte count must not extend a
        // stream even when its seek pointer is already past the current end.
        if source.is_empty() {
            return Ok(0);
        }
        self.ensure_len(end)?;
        self.bytes[self.cursor..end].copy_from_slice(source);
        self.cursor = end;
        Ok(source.len())
    }

    pub(super) fn seek(
        &mut self,
        displacement: i64,
        origin: ScreenshotSeekOrigin,
    ) -> Result<usize, ScreenshotBufferFailure> {
        let base = match origin {
            ScreenshotSeekOrigin::Start => 0,
            ScreenshotSeekOrigin::Current => self.cursor,
            ScreenshotSeekOrigin::End => self.bytes.len(),
        };
        let position = if displacement >= 0 {
            base.checked_add(
                usize::try_from(displacement).map_err(|_| ScreenshotBufferFailure::Limit)?,
            )
        } else {
            base.checked_sub(
                usize::try_from(displacement.unsigned_abs())
                    .map_err(|_| ScreenshotBufferFailure::Invalid)?,
            )
        }
        .ok_or(ScreenshotBufferFailure::Invalid)?;
        if position > self.limit {
            return Err(ScreenshotBufferFailure::Limit);
        }
        self.cursor = position;
        Ok(position)
    }

    pub(super) fn set_len(&mut self, length: usize) -> Result<(), ScreenshotBufferFailure> {
        if length > self.limit {
            return Err(ScreenshotBufferFailure::Limit);
        }
        self.ensure_capacity(length)?;
        self.bytes.resize(length, 0);
        Ok(())
    }

    pub(super) const fn len(&self) -> usize {
        self.bytes.len()
    }

    pub(super) fn take_bytes(&mut self) -> Vec<u8> {
        self.cursor = 0;
        std::mem::take(&mut self.bytes)
    }

    fn ensure_len(&mut self, length: usize) -> Result<(), ScreenshotBufferFailure> {
        self.ensure_capacity(length)?;
        if length > self.bytes.len() {
            self.bytes.resize(length, 0);
        }
        Ok(())
    }

    fn ensure_capacity(&mut self, required: usize) -> Result<(), ScreenshotBufferFailure> {
        if required <= self.bytes.capacity() {
            return Ok(());
        }
        let growth = self.bytes.capacity().max(MIN_BUFFER_GROWTH);
        let desired = required
            .max(self.bytes.capacity().saturating_add(growth))
            .min(self.limit);
        self.bytes
            .try_reserve_exact(desired.saturating_sub(self.bytes.len()))
            .map_err(|_| ScreenshotBufferFailure::Allocation)
    }
}

#[cfg(feature = "agentic-browser")]
pub(super) fn bounded_png_dimensions(
    png: &[u8],
    budget: SemanticScreenshotBudget,
) -> Result<(u32, u32), PngHeaderFailure> {
    if png.len()
        > usize::try_from(budget.max_png_bytes())
            .map_err(|_| PngHeaderFailure::ResourceExhausted)?
    {
        return Err(PngHeaderFailure::ResourceExhausted);
    }
    let header = png.get(..33).ok_or(PngHeaderFailure::Malformed)?;
    if header.get(..8) != Some(PNG_SIGNATURE.as_slice())
        || header.get(8..12) != Some(IHDR_DATA_BYTES.to_be_bytes().as_slice())
        || header.get(12..16) != Some(b"IHDR".as_slice())
    {
        return Err(PngHeaderFailure::Malformed);
    }
    let width = u32::from_be_bytes(
        header[16..20]
            .try_into()
            .map_err(|_| PngHeaderFailure::Malformed)?,
    );
    let height = u32::from_be_bytes(
        header[20..24]
            .try_into()
            .map_err(|_| PngHeaderFailure::Malformed)?,
    );
    let pixels = width
        .checked_mul(height)
        .ok_or(PngHeaderFailure::ResourceExhausted)?;
    if width == 0
        || height == 0
        || width > u32::from(budget.max_width())
        || height > u32::from(budget.max_height())
        || pixels > budget.max_pixels()
    {
        return Err(PngHeaderFailure::ResourceExhausted);
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "agentic-browser")]
    fn png_header(width: u32, height: u32) -> [u8; 33] {
        let mut header = [0_u8; 33];
        header[..8].copy_from_slice(PNG_SIGNATURE);
        header[8..12].copy_from_slice(&IHDR_DATA_BYTES.to_be_bytes());
        header[12..16].copy_from_slice(b"IHDR");
        header[16..20].copy_from_slice(&width.to_be_bytes());
        header[20..24].copy_from_slice(&height.to_be_bytes());
        header
    }

    #[test]
    fn writes_overwrite_and_sparse_extend_without_crossing_limit() {
        let mut buffer = BoundedScreenshotBuffer::new(8);
        assert_eq!(buffer.write(&[1, 2, 3, 4]), Ok(4));
        assert_eq!(buffer.seek(1, ScreenshotSeekOrigin::Start), Ok(1));
        assert_eq!(buffer.write(&[9, 8]), Ok(2));
        assert_eq!(buffer.seek(2, ScreenshotSeekOrigin::End), Ok(6));
        assert_eq!(buffer.write(&[7]), Ok(1));
        assert_eq!(buffer.take_bytes(), vec![1, 9, 8, 4, 0, 0, 7]);
    }

    #[test]
    fn byte_limit_is_enforced_before_growth_or_write() {
        let mut buffer = BoundedScreenshotBuffer::new(4);
        assert_eq!(buffer.write(&[1, 2, 3, 4]), Ok(4));
        assert_eq!(buffer.write(&[5]), Err(ScreenshotBufferFailure::Limit));
        assert_eq!(buffer.len(), 4);
        assert_eq!(
            buffer.seek(5, ScreenshotSeekOrigin::Start),
            Err(ScreenshotBufferFailure::Limit)
        );
        assert_eq!(buffer.set_len(5), Err(ScreenshotBufferFailure::Limit));
        assert_eq!(buffer.take_bytes(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn seek_and_set_size_follow_seekable_stream_rules() {
        let mut buffer = BoundedScreenshotBuffer::new(16);
        buffer.write(&[1, 2, 3, 4]).expect("initial write");
        assert_eq!(buffer.seek(-2, ScreenshotSeekOrigin::Current), Ok(2));
        assert_eq!(buffer.seek(-1, ScreenshotSeekOrigin::End), Ok(3));
        assert_eq!(
            buffer.seek(-4, ScreenshotSeekOrigin::Current),
            Err(ScreenshotBufferFailure::Invalid)
        );
        buffer.set_len(2).expect("truncate");
        assert_eq!(buffer.seek(0, ScreenshotSeekOrigin::Current), Ok(3));
        assert_eq!(buffer.write(&[]), Ok(0));
        assert_eq!(buffer.len(), 2);
        buffer.set_len(6).expect("extend");
        assert_eq!(buffer.take_bytes(), vec![1, 2, 0, 0, 0, 0]);
    }

    #[cfg(feature = "agentic-browser")]
    #[test]
    fn png_dimensions_are_read_only_from_bounded_ihdr() {
        let header = png_header(1280, 800);
        assert_eq!(
            bounded_png_dimensions(&header, SemanticScreenshotBudget::STANDARD),
            Ok((1280, 800))
        );

        let mut wrong_signature = header;
        wrong_signature[0] = 0;
        assert_eq!(
            bounded_png_dimensions(&wrong_signature, SemanticScreenshotBudget::STANDARD),
            Err(PngHeaderFailure::Malformed)
        );

        assert_eq!(
            bounded_png_dimensions(&png_header(1281, 800), SemanticScreenshotBudget::STANDARD,),
            Err(PngHeaderFailure::ResourceExhausted)
        );
        assert_eq!(
            bounded_png_dimensions(&png_header(0, 1), SemanticScreenshotBudget::STANDARD),
            Err(PngHeaderFailure::ResourceExhausted)
        );

        let tiny_budget = SemanticScreenshotBudget::try_new(1, 1, 1, 57).expect("tiny budget");
        let mut oversized = vec![0_u8; 58];
        oversized[..33].copy_from_slice(&png_header(1, 1));
        assert_eq!(
            bounded_png_dimensions(&oversized, tiny_budget),
            Err(PngHeaderFailure::ResourceExhausted)
        );
    }
}
