use super::{
    arena::Arena,
    sink::{Handle, Sink},
    DocLimits,
};
use encoding_rs::{CoderResult, Encoding, UTF_8};
use html5ever::{
    buffer_queue::BufferQueue,
    tendril::StrTendril,
    tokenizer::{Token, TokenSink, TokenSinkResult, Tokenizer},
    tree_builder::{Tracer, TreeBuilder, TreeBuilderOpts, TreeSink},
    TokenizerResult,
};
use std::cell::Cell;

pub(super) const CHUNK_BYTES: usize = 4096;
pub const MAX_CHARSET_LABEL_BYTES: usize = 64;
const META_PREFIX_BYTES: usize = 4096;
// html5ever can buffer an unfinished token, foster-parented characters, and live handles
// without issuing any arena allocations. Bound that work independently of the sink.
const MAX_UNFINISHED_TOKEN_BYTES: usize = 1024 * 1024;
const MAX_PENDING_TEXT_TOKENS: usize = 4096;
const MAX_LIVE_HANDLES: usize = 512;
const MAX_LIVE_MARKUP_BYTES: usize = super::MAX_ARENA_TEXT_BYTES;

pub(super) fn charset(label: &[u8]) -> Option<&'static Encoding> {
    (label.len() <= MAX_CHARSET_LABEL_BYTES)
        .then(|| Encoding::for_label(label))
        .flatten()
}

fn attrs_charset(mut bytes: &[u8]) -> Option<&'static Encoding> {
    let mut content = None;
    let mut pragma = None;
    let mut direct = None;
    while !bytes.is_empty() {
        let end = bytes
            .iter()
            .position(|c| c.is_ascii_whitespace() || matches!(c, b'=' | b'/'))
            .unwrap_or(bytes.len());
        if end == 0 {
            bytes = &bytes[1..];
            continue;
        }
        let key = &bytes[..end];
        bytes = bytes[end..].trim_ascii_start();
        let mut value = &[][..];
        if let Some(rest) = bytes.strip_prefix(b"=") {
            bytes = rest.trim_ascii_start();
            if let [quote @ (b'\'' | b'"'), rest @ ..] = bytes {
                let end = rest.iter().position(|c| c == quote)?;
                value = &rest[..end];
                bytes = &rest[end + 1..];
            } else {
                let end = bytes
                    .iter()
                    .position(u8::is_ascii_whitespace)
                    .unwrap_or(bytes.len());
                value = &bytes[..end];
                bytes = &bytes[end..];
            }
        }
        if key.eq_ignore_ascii_case(b"charset") {
            direct.get_or_insert(value);
        }
        if key.eq_ignore_ascii_case(b"http-equiv") {
            pragma.get_or_insert(value);
        }
        if key.eq_ignore_ascii_case(b"content") {
            content.get_or_insert(value);
        }
    }
    if let Some(enc) = direct.and_then(charset) {
        return Some(enc);
    }
    if !pragma?.eq_ignore_ascii_case(b"content-type") {
        return None;
    }
    content?.split(|&c| c == b';').skip(1).find_map(|param| {
        let eq = param.iter().position(|&c| c == b'=')?;
        if !param[..eq].trim_ascii().eq_ignore_ascii_case(b"charset") {
            return None;
        }
        let label = param[eq + 1..].trim_ascii();
        let label = match label {
            [b'\'', v @ .., b'\''] | [b'"', v @ .., b'"'] => v,
            _ => label,
        };
        charset(label)
    })
}

fn meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    let prefix = &bytes[..META_PREFIX_BYTES.min(bytes.len())];
    let mut bytes = prefix.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(prefix);
    let mut raw: Option<&[u8]> = None;
    let mut templates = 0usize;
    while let Some(i) = bytes.iter().position(|&c| c == b'<') {
        if raw.is_none() && templates == 0 && !bytes[..i].trim_ascii().is_empty() {
            break;
        }
        bytes = &bytes[i + 1..];
        if let Some(tag) = raw {
            let Some(rest) = bytes.strip_prefix(b"/") else {
                continue;
            };
            if !rest
                .get(..tag.len())
                .is_some_and(|v| v.eq_ignore_ascii_case(tag))
                || !rest
                    .get(tag.len())
                    .is_some_and(|v| v.is_ascii_whitespace() || matches!(v, b'>' | b'/'))
            {
                continue;
            }
            raw = None;
        }
        if let Some(comment) = bytes.strip_prefix(b"!--") {
            let end = comment.windows(3).position(|w| w == b"-->")?;
            bytes = &comment[end + 3..];
            continue;
        }
        let closing = bytes.starts_with(b"/");
        if closing {
            bytes = &bytes[1..]
        }
        let split = bytes
            .iter()
            .position(|c| c.is_ascii_whitespace() || matches!(c, b'>' | b'/'))?;
        let name = &bytes[..split];
        let rest = &bytes[split..];
        let mut quote = None;
        let end = rest.iter().position(|&c| {
            if quote == Some(c) {
                quote = None
            } else if quote.is_none() {
                if c == b'>' {
                    return true;
                }
                if matches!(c, b'\'' | b'"') {
                    quote = Some(c)
                }
            }
            false
        })?;
        bytes = &rest[end + 1..];
        if name.eq_ignore_ascii_case(b"template") {
            if closing {
                templates = templates.saturating_sub(1)
            } else {
                templates += 1
            }
        } else if closing {
            if templates == 0
                && [b"head".as_slice(), b"body", b"html", b"br"]
                    .iter()
                    .any(|n| name.eq_ignore_ascii_case(n))
            {
                break;
            }
        } else if [
            b"script".as_slice(),
            b"style",
            b"title",
            b"noscript",
            b"noframes",
            b"textarea",
        ]
        .iter()
        .any(|t| name.eq_ignore_ascii_case(t))
        {
            raw = Some(name);
        } else if templates == 0 {
            if name.eq_ignore_ascii_case(b"meta") {
                if let Some(enc) = attrs_charset(&rest[..end]) {
                    return Some(enc);
                }
            } else if ![
                b"html".as_slice(),
                b"head",
                b"base",
                b"basefont",
                b"bgsound",
                b"link",
                b"!doctype",
            ]
            .iter()
            .any(|t| name.eq_ignore_ascii_case(t))
                && !name.starts_with(b"?")
            {
                break;
            }
        }
    }
    None
}

struct Tokens {
    builder: TreeBuilder<Handle, Sink>,
    progressed: Cell<bool>,
    pending_count: Cell<usize>,
    pending_bytes: Cell<usize>,
    text_limit: usize,
}
impl TokenSink for Tokens {
    type Handle = Handle;
    fn process_token(&self, token: Token, line: u64) -> TokenSinkResult<Handle> {
        let sink = &self.builder.sink;
        if sink.arena.borrow().stopped {
            return TokenSinkResult::Continue;
        }
        if matches!(token, Token::ParseError(_)) {
            return TokenSinkResult::Continue;
        }
        self.progressed.set(true);
        sink.received_text.set(false);
        let bytes = match &token {
            Token::CharacterTokens(t) => t.len(),
            Token::NullCharacterToken => 1,
            _ => 0,
        };
        let result = self.builder.process_token(token, line);
        if bytes == 0 || sink.received_text.get() || !sink.arena.borrow().saw_table {
            self.pending_count.set(0);
            self.pending_bytes.set(0);
        } else {
            self.pending_count.set(self.pending_count.get() + 1);
            self.pending_bytes.set(self.pending_bytes.get() + bytes);
            if self.pending_count.get() > MAX_PENDING_TEXT_TOKENS
                || self.pending_bytes.get() > self.text_limit
            {
                sink.arena.borrow_mut().stopped = true;
            }
        }
        result
    }
    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.builder
            .adjusted_current_node_present_but_not_in_html_namespace()
    }
    fn end(&self) {
        self.builder.end()
    }
}

#[derive(Default)]
struct LiveMarkup {
    handles: Cell<usize>,
    bytes: Cell<usize>,
}
impl Tracer for LiveMarkup {
    type Handle = Handle;
    fn trace_handle(&self, handle: &Handle) {
        self.handles.set(self.handles.get() + 1);
        self.bytes
            .set(self.bytes.get().saturating_add(handle.markup_bytes));
    }
}

pub(super) fn parse(bytes: &[u8], label: Option<&str>, limits: DocLimits) -> Arena {
    let enc = label
        .and_then(|v| charset(v.as_bytes()))
        .or_else(|| {
            meta_charset(bytes).map(|hint| {
                if hint == encoding_rs::UTF_16LE || hint == encoding_rs::UTF_16BE {
                    UTF_8
                } else {
                    hint
                }
            })
        })
        .unwrap_or(UTF_8);
    let mut decoder = enc.new_decoder();
    let sink = Sink::new(Arena::new(limits));
    let builder = TreeBuilder::new(
        sink,
        TreeBuilderOpts {
            scripting_enabled: false,
            drop_doctype: true,
            ..Default::default()
        },
    );
    let tok = Tokenizer::new(
        Tokens {
            builder,
            progressed: Cell::default(),
            pending_count: Cell::default(),
            pending_bytes: Cell::default(),
            text_limit: limits.arena_text_bytes as usize,
        },
        Default::default(),
    );
    let input = BufferQueue::default();
    let mut decoded = String::with_capacity(CHUNK_BYTES);
    let mut unfinished = 0;
    let mut chunks = bytes.chunks(CHUNK_BYTES).peekable();
    'decode: while let Some(mut chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        loop {
            decoded.clear();
            let (status, read, _) = decoder.decode_to_string(chunk, &mut decoded, last);
            chunk = &chunk[read..];
            input.push_back(StrTendril::from_slice(&decoded));
            while !matches!(tok.feed(&input), TokenizerResult::Done) {}
            let live = LiveMarkup::default();
            tok.sink.builder.trace_handles(&live);
            if tok.sink.progressed.replace(false) {
                unfinished = 0
            }
            unfinished += decoded.len();
            let mut arena = tok.sink.builder.sink.arena.borrow_mut();
            if unfinished >= MAX_UNFINISHED_TOKEN_BYTES
                || live.handles.get() > MAX_LIVE_HANDLES
                || live.bytes.get() > MAX_LIVE_MARKUP_BYTES
            {
                arena.stopped = true
            }
            if arena.stopped {
                break 'decode;
            }
            if status == CoderResult::InputEmpty {
                break;
            }
        }
    }
    if !tok.sink.builder.sink.arena.borrow().stopped {
        tok.end()
    }
    tok.sink.builder.sink.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescan_observes_head_and_quoted_markup() {
        let meta = "<meta charset=shift_jis>";
        for markup in [
            format!("<p>{meta}"),
            format!("body text{meta}"),
            format!("<script>const x='{meta}'</script>"),
            format!("<link title='{meta}'>"),
            format!("<template><template></template>{meta}</template>"),
            format!("<noscript>{meta}</noscript>"),
        ] {
            assert!(meta_charset(markup.as_bytes()).is_none())
        }
        let valid =
            format!("<!DOCTYPE html><title>hello</title><!-- comment --><link title='>'>{meta}");
        assert_eq!(meta_charset(valid.as_bytes()), Some(encoding_rs::SHIFT_JIS));
    }

    #[test]
    fn transient_parser_budgets() {
        let l = DocLimits::default();
        let large_comment = format!("<!--{}--><p>end", "x".repeat(MAX_UNFINISHED_TOKEN_BYTES));
        assert!(parse(large_comment.as_bytes(), None, l).stopped);
        let short_comment = format!(
            "<!--{}--><p>end",
            "x".repeat(MAX_UNFINISHED_TOKEN_BYTES - CHUNK_BYTES)
        );
        assert!(!parse(short_comment.as_bytes(), None, l).stopped);
        for (count, stopped) in [
            (MAX_PENDING_TEXT_TOKENS, false),
            (MAX_PENDING_TEXT_TOKENS + 1, true),
        ] {
            let deferred = format!("<table>{}</table>", "&amp;".repeat(count));
            assert_eq!(parse(deferred.as_bytes(), None, l).stopped, stopped);
        }
        let attr = "x".repeat(CHUNK_BYTES);
        let mut formatting = String::new();
        for id in 0..MAX_LIVE_MARKUP_BYTES / (2 * CHUNK_BYTES) {
            formatting.push_str(&format!("<b data-ignored='{id}{attr}'>"));
        }
        assert!(parse(formatting.as_bytes(), None, l).stopped);
        let deep = format!("{}<p>end", "<div>".repeat(MAX_LIVE_HANDLES));
        assert!(parse(deep.as_bytes(), None, l).stopped);
    }
}
