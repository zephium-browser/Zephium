use brotli_decompressor::{BrotliDecompressStream, BrotliResult, BrotliState, StandardAlloc};
use flate2::{Crc, Decompress, FlushDecompress, Status};

use crate::limits::{MAX_BROTLI_WINDOW_BITS, MAX_GZIP_HEADER_FIELD};

const OUTPUT_CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecodeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Coding {
    Identity,
    Gzip,
    Brotli,
}

type Emit<'a> = dyn FnMut(&[u8]) -> Result<bool, DecodeError> + 'a;

/// Content decoding, an optional gzip wrapper for `.xml.gz`, and the capped
/// body, all push-driven so nothing past the cap is ever produced or kept.
pub(crate) struct Pipeline {
    content: Stage,
    wrapper: Option<Wrapper>,
    body: Body,
}

impl Pipeline {
    pub(crate) fn new(coding: Coding, wrapped: bool, cap: usize) -> Self {
        Self {
            content: Stage::new(coding),
            wrapper: wrapped.then_some(Wrapper::Undecided),
            body: Body {
                bytes: Vec::new(),
                cap,
                truncated: false,
            },
        }
    }

    /// Feeds wire bytes; `Ok(false)` once the body is full and reading should stop.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<bool, DecodeError> {
        let Self {
            content,
            wrapper,
            body,
        } = self;
        let mut sink = |decoded: &[u8]| match wrapper {
            None => Ok(body.accept(decoded)),
            Some(wrapper) => wrapper.push(decoded, &mut |inner| Ok(body.accept(inner))),
        };
        content.push(chunk, &mut sink)
    }

    pub(crate) fn decoded(&self) -> &[u8] {
        &self.body.bytes
    }

    pub(crate) fn finish(self) -> Result<(Vec<u8>, bool), DecodeError> {
        if !self.body.truncated
            && (!self.content.complete() || self.wrapper.as_ref().is_some_and(|w| !w.complete()))
        {
            return Err(DecodeError);
        }
        Ok((self.body.bytes, self.body.truncated))
    }
}

struct Body {
    bytes: Vec<u8>,
    cap: usize,
    truncated: bool,
}

impl Body {
    fn accept(&mut self, chunk: &[u8]) -> bool {
        if self.truncated {
            return false;
        }
        let room = self.cap - self.bytes.len();
        let (kept, full) = if chunk.len() > room {
            (&chunk[..room], true)
        } else {
            (chunk, false)
        };
        let wanted = self.bytes.len() + kept.len();
        if wanted > self.bytes.capacity() {
            let target = wanted
                .max(self.bytes.capacity().saturating_mul(2))
                .min(self.cap);
            self.bytes.reserve_exact(target - self.bytes.len());
        }
        self.bytes.extend_from_slice(kept);
        self.truncated = full;
        !full
    }
}

enum Stage {
    Identity,
    Gzip(Box<Gunzip>),
    Brotli(Box<Unbrotli>),
}

impl Stage {
    fn new(coding: Coding) -> Self {
        match coding {
            Coding::Identity => Stage::Identity,
            Coding::Gzip => Stage::Gzip(Box::new(Gunzip::new())),
            Coding::Brotli => Stage::Brotli(Box::new(Unbrotli::new())),
        }
    }

    fn push(&mut self, input: &[u8], emit: &mut Emit<'_>) -> Result<bool, DecodeError> {
        match self {
            Stage::Identity if input.is_empty() => Ok(true),
            Stage::Identity => emit(input),
            Stage::Gzip(gunzip) => gunzip.push(input, emit),
            Stage::Brotli(unbrotli) => unbrotli.push(input, emit),
        }
    }

    fn complete(&self) -> bool {
        match self {
            Stage::Identity => true,
            Stage::Gzip(gunzip) => gunzip.complete(),
            Stage::Brotli(unbrotli) => unbrotli.done,
        }
    }
}

enum Wrapper {
    Undecided,
    Gzip(Box<Gunzip>),
    Plain,
}

impl Wrapper {
    fn push(&mut self, input: &[u8], emit: &mut Emit<'_>) -> Result<bool, DecodeError> {
        if matches!(self, Wrapper::Undecided) {
            let Some(&first) = input.first() else {
                return Ok(true);
            };
            *self = if first == 0x1f && input.get(1).is_none_or(|second| *second == 0x8b) {
                Wrapper::Gzip(Box::new(Gunzip::new()))
            } else {
                Wrapper::Plain
            };
        }
        match self {
            Wrapper::Gzip(gunzip) => gunzip.push(input, emit),
            Wrapper::Plain if input.is_empty() => Ok(true),
            Wrapper::Plain => emit(input),
            Wrapper::Undecided => Ok(true),
        }
    }

    fn complete(&self) -> bool {
        match self {
            Wrapper::Gzip(gunzip) => gunzip.complete(),
            Wrapper::Undecided | Wrapper::Plain => true,
        }
    }
}

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const FRESERVED: u8 = 0xe0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderStep {
    Fixed,
    ExtraLength,
    Extra,
    Name,
    Comment,
    HeaderCrc,
    Done,
}

struct Header {
    step: HeaderStep,
    fixed: [u8; 10],
    seen: usize,
    remaining: usize,
}

impl Header {
    fn new() -> Self {
        Self {
            step: HeaderStep::Fixed,
            fixed: [0; 10],
            seen: 0,
            remaining: 0,
        }
    }

    fn flags(&self) -> u8 {
        self.fixed[3]
    }

    fn advance(&mut self, after: HeaderStep) {
        let order = [
            (HeaderStep::ExtraLength, FEXTRA),
            (HeaderStep::Name, FNAME),
            (HeaderStep::Comment, FCOMMENT),
            (HeaderStep::HeaderCrc, FHCRC),
        ];
        let start = match after {
            HeaderStep::Fixed => 0,
            HeaderStep::ExtraLength | HeaderStep::Extra => 1,
            HeaderStep::Name => 2,
            HeaderStep::Comment => 3,
            HeaderStep::HeaderCrc | HeaderStep::Done => 4,
        };
        self.seen = 0;
        self.step = order[start.min(order.len())..]
            .iter()
            .find(|(_, flag)| self.flags() & flag != 0)
            .map_or(HeaderStep::Done, |(step, _)| *step);
        if self.step == HeaderStep::HeaderCrc {
            self.remaining = 2;
        }
    }

    fn feed(&mut self, input: &[u8]) -> Result<usize, DecodeError> {
        let mut used = 0;
        while used < input.len() && self.step != HeaderStep::Done {
            let byte = input[used];
            used += 1;
            match self.step {
                HeaderStep::Fixed => {
                    self.fixed[self.seen] = byte;
                    self.seen += 1;
                    if self.seen == self.fixed.len() {
                        if self.fixed[..3] != [0x1f, 0x8b, 8] || self.flags() & FRESERVED != 0 {
                            return Err(DecodeError);
                        }
                        self.advance(HeaderStep::Fixed);
                    }
                }
                HeaderStep::ExtraLength => {
                    self.remaining |= usize::from(byte) << (8 * self.seen);
                    self.seen += 1;
                    if self.seen == 2 {
                        if self.remaining == 0 {
                            self.advance(HeaderStep::Extra);
                        } else {
                            self.step = HeaderStep::Extra;
                        }
                    }
                }
                HeaderStep::Extra => {
                    self.remaining -= 1;
                    if self.remaining == 0 {
                        self.advance(HeaderStep::Extra);
                    }
                }
                HeaderStep::Name | HeaderStep::Comment => {
                    if byte == 0 {
                        self.advance(self.step);
                    } else {
                        self.seen += 1;
                        if self.seen > MAX_GZIP_HEADER_FIELD {
                            return Err(DecodeError);
                        }
                    }
                }
                HeaderStep::HeaderCrc => {
                    self.remaining -= 1;
                    if self.remaining == 0 {
                        self.step = HeaderStep::Done;
                    }
                }
                HeaderStep::Done => {}
            }
        }
        Ok(used)
    }
}

enum GzipStep {
    Header(Header),
    Deflate,
    Trailer { bytes: [u8; 8], seen: usize },
    Done,
}

/// One gzip member; bytes after it are ignored, as browsers do.
struct Gunzip {
    step: GzipStep,
    inflate: Decompress,
    crc: Crc,
    out: Vec<u8>,
}

impl Gunzip {
    fn new() -> Self {
        Self {
            step: GzipStep::Header(Header::new()),
            inflate: Decompress::new(false),
            crc: Crc::new(),
            out: vec![0; OUTPUT_CHUNK],
        }
    }

    fn complete(&self) -> bool {
        matches!(self.step, GzipStep::Done)
    }

    fn push(&mut self, mut input: &[u8], emit: &mut Emit<'_>) -> Result<bool, DecodeError> {
        loop {
            match &mut self.step {
                GzipStep::Header(header) => {
                    let used = header.feed(input)?;
                    input = &input[used..];
                    if header.step != HeaderStep::Done {
                        return Ok(true);
                    }
                    self.step = GzipStep::Deflate;
                }
                GzipStep::Deflate => {
                    let before_in = self.inflate.total_in();
                    let before_out = self.inflate.total_out();
                    let status = self
                        .inflate
                        .decompress(input, &mut self.out, FlushDecompress::None)
                        .map_err(|_| DecodeError)?;
                    let consumed = usize::try_from(self.inflate.total_in() - before_in)
                        .map_err(|_| DecodeError)?;
                    let produced = usize::try_from(self.inflate.total_out() - before_out)
                        .map_err(|_| DecodeError)?;
                    input = input.get(consumed..).ok_or(DecodeError)?;
                    let output = self.out.get(..produced).ok_or(DecodeError)?;
                    if !output.is_empty() {
                        self.crc.update(output);
                        if !emit(output)? {
                            return Ok(false);
                        }
                    }
                    if status == Status::StreamEnd {
                        self.step = GzipStep::Trailer {
                            bytes: [0; 8],
                            seen: 0,
                        };
                    } else if produced == self.out.len() {
                        continue;
                    } else if input.is_empty() {
                        return Ok(true);
                    } else if consumed == 0 && produced == 0 {
                        return Err(DecodeError);
                    }
                }
                GzipStep::Trailer { bytes, seen } => {
                    let take = (bytes.len() - *seen).min(input.len());
                    bytes[*seen..*seen + take].copy_from_slice(&input[..take]);
                    *seen += take;
                    input = &input[take..];
                    if *seen < bytes.len() {
                        return Ok(true);
                    }
                    let crc = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                    let size = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
                    if crc != self.crc.sum() || size != self.crc.amount() {
                        return Err(DecodeError);
                    }
                    self.step = GzipStep::Done;
                }
                GzipStep::Done => return Ok(true),
            }
        }
    }
}

/// RFC 7932 section 9.1: the window size is in the stream's first bits.
/// `None` is the large-window extension, which HTTP never uses.
pub(crate) fn brotli_window_bits(first: u8) -> Option<u8> {
    if first & 1 == 0 {
        return Some(16);
    }
    let wide = (first >> 1) & 7;
    if wide != 0 {
        return Some(17 + wide);
    }
    match (first >> 4) & 7 {
        0 => Some(17),
        1 => None,
        narrow => Some(8 + narrow),
    }
}

struct Unbrotli {
    state: BrotliState<StandardAlloc, StandardAlloc, StandardAlloc>,
    out: Vec<u8>,
    window_checked: bool,
    done: bool,
}

impl Unbrotli {
    fn new() -> Self {
        Self {
            state: BrotliState::new_strict(
                StandardAlloc::default(),
                StandardAlloc::default(),
                StandardAlloc::default(),
            ),
            out: vec![0; OUTPUT_CHUNK],
            window_checked: false,
            done: false,
        }
    }

    fn push(&mut self, input: &[u8], emit: &mut Emit<'_>) -> Result<bool, DecodeError> {
        if self.done || input.is_empty() {
            return Ok(true);
        }
        if !self.window_checked {
            // The decoder sizes its ring buffer from these bits; refuse a
            // hostile window before it can ask for one.
            if brotli_window_bits(input[0]).is_none_or(|bits| bits > MAX_BROTLI_WINDOW_BITS) {
                return Err(DecodeError);
            }
            self.window_checked = true;
        }
        let mut available_in = input.len();
        let mut input_offset = 0;
        loop {
            let mut available_out = self.out.len();
            let mut output_offset = 0;
            let mut total_out = 0;
            let result = BrotliDecompressStream(
                &mut available_in,
                &mut input_offset,
                input,
                &mut available_out,
                &mut output_offset,
                &mut self.out,
                &mut total_out,
                &mut self.state,
            );
            let output = self.out.get(..output_offset).ok_or(DecodeError)?;
            if !output.is_empty() && !emit(output)? {
                return Ok(false);
            }
            match result {
                BrotliResult::NeedsMoreInput => return Ok(true),
                BrotliResult::NeedsMoreOutput => {}
                BrotliResult::ResultSuccess => {
                    self.done = true;
                    return Ok(true);
                }
                BrotliResult::ResultFailure => return Err(DecodeError),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::io::Write;

    pub(crate) fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    pub(crate) fn brotli_with_window(bytes: &[u8], lgwin: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut out, 4096, 5, lgwin);
            writer.write_all(bytes).unwrap();
        }
        out
    }

    pub(crate) fn brotli(bytes: &[u8]) -> Vec<u8> {
        brotli_with_window(bytes, 20)
    }

    fn run(
        coding: Coding,
        wrapped: bool,
        cap: usize,
        chunks: &[&[u8]],
    ) -> Result<(Vec<u8>, bool), DecodeError> {
        let mut pipeline = Pipeline::new(coding, wrapped, cap);
        for chunk in chunks {
            if !pipeline.push(chunk)? {
                break;
            }
        }
        pipeline.finish()
    }

    fn bytewise(
        coding: Coding,
        wrapped: bool,
        cap: usize,
        encoded: &[u8],
    ) -> Result<(Vec<u8>, bool), DecodeError> {
        let chunks: Vec<&[u8]> = encoded.chunks(1).collect();
        run(coding, wrapped, cap, &chunks)
    }

    fn sample() -> Vec<u8> {
        (0..40_000u32)
            .flat_map(|n| format!("<url><loc>https://example.com/{n}</loc></url>\n").into_bytes())
            .take(200_000)
            .collect()
    }

    #[test]
    fn gzip_and_brotli_decode_whole_and_byte_by_byte() {
        let plain = sample();
        for (coding, encoded) in [
            (Coding::Gzip, gzip(&plain)),
            (Coding::Brotli, brotli(&plain)),
            (Coding::Identity, plain.clone()),
        ] {
            assert_eq!(
                run(coding, false, 1 << 20, &[&encoded]),
                Ok((plain.clone(), false))
            );
            assert_eq!(
                bytewise(coding, false, 1 << 20, &encoded),
                Ok((plain.clone(), false))
            );
        }
    }

    #[test]
    fn gzip_header_fields_are_skipped_and_bounded() {
        let plain = b"<html>hello</html>".to_vec();
        let mut member = gzip(&plain);
        let mut header = vec![
            0x1f,
            0x8b,
            8,
            FEXTRA | FNAME | FCOMMENT | FHCRC,
            0,
            0,
            0,
            0,
            0,
            3,
        ];
        header.extend_from_slice(&[3, 0, b'a', b'b', b'c']);
        header.extend_from_slice(b"name.html\0");
        header.extend_from_slice(b"a comment\0");
        header.extend_from_slice(&[0xaa, 0xbb]);
        let body = member.split_off(10);
        let framed = [header.clone(), body.clone()].concat();
        assert_eq!(
            bytewise(Coding::Gzip, false, 1024, &framed),
            Ok((plain.clone(), false))
        );
        let mut empty_extra = vec![0x1f, 0x8b, 8, FEXTRA, 0, 0, 0, 0, 0, 3, 0, 0];
        empty_extra.extend_from_slice(&body);
        assert_eq!(
            run(Coding::Gzip, false, 1024, &[&empty_extra]),
            Ok((plain.clone(), false))
        );
        let mut long_name = vec![0x1f, 0x8b, 8, FNAME, 0, 0, 0, 0, 0, 3];
        long_name.extend(std::iter::repeat_n(b'n', MAX_GZIP_HEADER_FIELD));
        long_name.push(0);
        long_name.extend_from_slice(&body);
        assert_eq!(
            run(Coding::Gzip, false, 1024, &[&long_name]),
            Ok((plain.clone(), false))
        );
        let mut too_long = vec![0x1f, 0x8b, 8, FNAME, 0, 0, 0, 0, 0, 3];
        too_long.extend(std::iter::repeat_n(b'n', MAX_GZIP_HEADER_FIELD + 1));
        assert_eq!(
            run(Coding::Gzip, false, 1024, &[&too_long]),
            Err(DecodeError)
        );
        let reserved = [0x1f, 0x8b, 8, 0x20, 0, 0, 0, 0, 0, 3];
        assert_eq!(
            run(Coding::Gzip, false, 1024, &[&reserved]),
            Err(DecodeError)
        );
        assert_eq!(
            run(Coding::Gzip, false, 1024, &[b"<html>plain</html>"]),
            Err(DecodeError)
        );
    }

    #[test]
    fn a_damaged_or_truncated_stream_is_refused() {
        let plain = sample();
        let mut corrupt = gzip(&plain);
        let crc = corrupt.len() - 8;
        corrupt[crc] ^= 0xff;
        assert_eq!(
            run(Coding::Gzip, false, 1 << 20, &[&corrupt]),
            Err(DecodeError)
        );
        let mut size = gzip(&plain);
        let last = size.len() - 1;
        size[last] ^= 0x01;
        assert_eq!(
            run(Coding::Gzip, false, 1 << 20, &[&size]),
            Err(DecodeError)
        );
        for encoded in [gzip(&plain), brotli(&plain)] {
            let coding = if encoded[0] == 0x1f {
                Coding::Gzip
            } else {
                Coding::Brotli
            };
            for cut in [1, 5, 11, encoded.len() / 2, encoded.len() - 1] {
                assert_eq!(
                    run(coding, false, 1 << 20, &[&encoded[..cut]]),
                    Err(DecodeError),
                    "{coding:?} cut at {cut}"
                );
            }
        }
        let mut garbage = brotli(&plain);
        let middle = garbage.len() / 2;
        garbage[middle] ^= 0x55;
        garbage[middle + 1] ^= 0xaa;
        assert!(
            run(Coding::Brotli, false, 1 << 20, &[&garbage]).map(|(body, _)| body) != Ok(plain)
        );
    }

    #[test]
    fn bytes_after_a_finished_member_are_ignored() {
        let plain = b"<urlset></urlset>".to_vec();
        let trailing = [gzip(&plain), b"junk after the member".to_vec()].concat();
        assert_eq!(
            run(Coding::Gzip, false, 1024, &[&trailing]),
            Ok((plain.clone(), false))
        );
        let trailing = [brotli(&plain), b"junk".to_vec()].concat();
        assert_eq!(
            run(Coding::Brotli, false, 1024, &[&trailing]),
            Ok((plain, false))
        );
    }

    #[test]
    fn a_bomb_stops_at_the_cap_without_expanding_further() {
        let zeros = vec![0u8; 8 * 1024 * 1024];
        let cap = 64 * 1024;
        for (coding, bomb) in [
            (Coding::Gzip, gzip(&zeros)),
            (Coding::Brotli, brotli(&zeros)),
        ] {
            assert!(bomb.len() < 64 * 1024, "{coding:?} {}", bomb.len());
            let mut pipeline = Pipeline::new(coding, false, cap);
            let mut fed = 0;
            for chunk in bomb.chunks(4) {
                fed += chunk.len();
                if !pipeline.push(chunk).unwrap() {
                    break;
                }
            }
            assert!(fed < bomb.len(), "{coding:?} read the whole bomb");
            let (body, truncated) = pipeline.finish().unwrap();
            assert!(truncated);
            assert_eq!(body.len(), cap);
            assert!(body.capacity() <= cap);
        }
    }

    #[test]
    fn the_cap_is_exact_and_only_an_overflow_truncates() {
        let plain = vec![b'a'; 1000];
        assert_eq!(
            run(Coding::Identity, false, 1000, &[&plain]),
            Ok((plain.clone(), false))
        );
        assert_eq!(
            run(Coding::Identity, false, 999, &[&plain]),
            Ok((plain[..999].to_vec(), true))
        );
        assert_eq!(
            run(Coding::Gzip, false, 1000, &[&gzip(&plain)]),
            Ok((plain.clone(), false))
        );
        assert_eq!(
            run(Coding::Gzip, false, 999, &[&gzip(&plain)]),
            Ok((plain[..999].to_vec(), true))
        );
    }

    #[test]
    fn the_brotli_window_is_refused_above_its_limit() {
        assert_eq!(brotli_window_bits(0b0000_0000), Some(16));
        assert_eq!(brotli_window_bits(0b0000_1111), Some(24));
        assert_eq!(brotli_window_bits(0b0000_1011), Some(22));
        assert_eq!(brotli_window_bits(0b0000_1101), Some(23));
        assert_eq!(brotli_window_bits(0b0000_0001), Some(17));
        assert_eq!(brotli_window_bits(0b0001_0001), None);
        assert_eq!(brotli_window_bits(0b0010_0001), Some(10));
        assert_eq!(brotli_window_bits(0b0111_0001), Some(15));
        let plain = sample();
        let allowed = brotli_with_window(&plain, u32::from(MAX_BROTLI_WINDOW_BITS));
        assert_eq!(brotli_window_bits(allowed[0]), Some(MAX_BROTLI_WINDOW_BITS));
        assert_eq!(
            run(Coding::Brotli, false, 1 << 20, &[&allowed]),
            Ok((plain.clone(), false))
        );
        let over = brotli_with_window(&plain, u32::from(MAX_BROTLI_WINDOW_BITS) + 1);
        assert_eq!(
            brotli_window_bits(over[0]),
            Some(MAX_BROTLI_WINDOW_BITS + 1)
        );
        assert_eq!(
            run(Coding::Brotli, false, 1 << 20, &[&over]),
            Err(DecodeError)
        );
        assert_eq!(
            run(Coding::Brotli, false, 1 << 20, &[&[0x11, 0, 0, 0]]),
            Err(DecodeError)
        );
    }

    #[test]
    fn a_gzip_wrapper_is_decoded_once_or_twice_or_passed_through() {
        let xml = b"<?xml version=\"1.0\"?><urlset></urlset>".to_vec();
        assert_eq!(
            run(Coding::Identity, true, 1024, &[&gzip(&xml)]),
            Ok((xml.clone(), false))
        );
        assert_eq!(
            bytewise(Coding::Gzip, true, 1024, &gzip(&gzip(&xml))),
            Ok((xml.clone(), false))
        );
        assert_eq!(
            run(Coding::Gzip, true, 1024, &[&gzip(&xml)]),
            Ok((xml.clone(), false))
        );
        assert_eq!(
            run(Coding::Identity, true, 1024, &[&xml]),
            Ok((xml.clone(), false))
        );
        let inner = gzip(&xml);
        assert_eq!(
            run(Coding::Identity, true, 1024, &[&inner[..inner.len() - 3]]),
            Err(DecodeError)
        );
        let bomb = gzip(&gzip(&vec![b'x'; 4 * 1024 * 1024]));
        assert_eq!(
            run(Coding::Gzip, true, 4096, &[&bomb]).map(|(b, t)| (b.len(), t)),
            Ok((4096, true))
        );
    }

    proptest! {
        #[test]
        fn hostile_bytes_never_panic_or_pass_the_cap(
            bytes in proptest::collection::vec(any::<u8>(), 0..4096),
            split in 1usize..64,
            cap in 1usize..2048,
            coding in prop_oneof![Just(Coding::Identity), Just(Coding::Gzip), Just(Coding::Brotli)],
            wrapped in any::<bool>(),
        ) {
            let mut pipeline = Pipeline::new(coding, wrapped, cap);
            for chunk in bytes.chunks(split) {
                match pipeline.push(chunk) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => break,
                }
                prop_assert!(pipeline.decoded().len() <= cap);
            }
            if let Ok((body, _)) = pipeline.finish() {
                prop_assert!(body.len() <= cap);
            }
        }

        #[test]
        fn mutated_real_streams_never_panic(
            seed in proptest::collection::vec(any::<u8>(), 1..512),
            flips in proptest::collection::vec((any::<usize>(), any::<u8>()), 0..8),
            brotli_stream in any::<bool>(),
        ) {
            let mut encoded = if brotli_stream { brotli(&seed) } else { gzip(&seed) };
            for (at, value) in flips {
                let index = at % encoded.len();
                encoded[index] ^= value;
            }
            let coding = if brotli_stream { Coding::Brotli } else { Coding::Gzip };
            let _ = run(coding, false, 4096, &[&encoded]);
        }
    }
}
