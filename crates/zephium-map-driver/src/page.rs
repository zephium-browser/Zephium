use zephium_map::{session::Failure, Doc, DocLimits};

use crate::io::ResultsPage;

pub const MAX_RESULTS_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RESULTS_PAGE_BYTES: usize = DocLimits::RESULTS.input_bytes as usize;

const DROPPED: [&[u8]; 2] = [b"script", b"style"];
const VERBATIM: [&[u8]; 6] = [
    b"textarea",
    b"title",
    b"xmp",
    b"iframe",
    b"noembed",
    b"noframes",
];

pub(crate) fn parse(page: &ResultsPage) -> Result<Doc, Failure> {
    let text = strip(&page.html)?;
    Doc::from_html(
        &page.url,
        text.as_bytes(),
        Some("utf-8"),
        DocLimits::RESULTS,
    )
    .map_err(|_| Failure::Unreadable)
}

pub(crate) fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

struct Out(String);

impl Out {
    fn push(&mut self, text: &str) -> Result<(), Failure> {
        if self.0.len() + text.len() > MAX_RESULTS_PAGE_BYTES {
            return Err(Failure::Limit);
        }
        self.0.push_str(text);
        Ok(())
    }
}

/// Where a script or style ends must agree with the HTML tokenizer, or a
/// dropped span could swallow markup the parser would have read.
pub(crate) fn strip(source: &str) -> Result<String, Failure> {
    let source = &source[..floor_boundary(source, MAX_RESULTS_SOURCE_BYTES)];
    let bytes = source.as_bytes();
    let mut out = Out(String::with_capacity(
        source.len().min(MAX_RESULTS_PAGE_BYTES),
    ));
    let mut copied = 0;
    let mut at = 0;
    while let Some(lt) = find_byte(bytes, at, b'<') {
        let rest = &bytes[lt..];
        if rest.starts_with(b"<!--") {
            at = find(bytes, lt + 4, b"-->").map_or(bytes.len(), |end| end + 3);
        } else if rest.starts_with(b"<![CDATA[") {
            at = find(bytes, lt + 9, b"]]>").map_or(bytes.len(), |end| end + 3);
        } else if rest.get(1).is_some_and(u8::is_ascii_alphabetic) {
            let name_end = bytes[lt + 1..]
                .iter()
                .position(|b| matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/' | b'>'))
                .map_or(bytes.len(), |n| lt + 1 + n);
            let name = &bytes[lt + 1..name_end];
            let open_end = tag_end(bytes, name_end);
            at = open_end;
            if DROPPED.iter().any(|d| d.eq_ignore_ascii_case(name)) {
                out.push(&source[copied..open_end])?;
                let close = if name.eq_ignore_ascii_case(b"script") {
                    script_end(bytes, open_end)
                } else {
                    end_tag(bytes, open_end, name)
                };
                copied = close;
                at = close;
            } else if VERBATIM.iter().any(|v| v.eq_ignore_ascii_case(name)) {
                at = end_tag(bytes, open_end, name);
            } else if name.eq_ignore_ascii_case(b"plaintext") {
                at = bytes.len();
            }
        } else if rest.get(1) == Some(&b'/') && rest.get(2).is_some_and(u8::is_ascii_alphabetic) {
            at = tag_end(bytes, lt + 2);
        } else if matches!(rest.get(1), Some(b'!' | b'?' | b'/')) {
            at = find_byte(bytes, lt + 1, b'>').map_or(bytes.len(), |end| end + 1);
        } else {
            at = lt + 1;
        }
    }
    out.push(&source[copied..])?;
    Ok(out.0)
}

fn find_byte(bytes: &[u8], from: usize, byte: u8) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|&b| b == byte)
        .map(|n| from + n)
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|n| from + n)
}

fn tag_end(bytes: &[u8], from: usize) -> usize {
    let mut at = from;
    while at < bytes.len() {
        match bytes[at] {
            b'>' => return at + 1,
            b'=' => {
                at += 1;
                while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                    at += 1;
                }
                if let Some(&quote @ (b'"' | b'\'')) = bytes.get(at) {
                    at = find_byte(bytes, at + 1, quote).map_or(bytes.len(), |end| end + 1);
                }
            }
            _ => at += 1,
        }
    }
    bytes.len()
}

fn delimited(bytes: &[u8], at: usize) -> bool {
    matches!(
        bytes.get(at),
        Some(b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/' | b'>')
    )
}

fn tag_at(bytes: &[u8], at: usize, prefix: &[u8], name: &[u8]) -> bool {
    let start = at + prefix.len();
    bytes.get(at..start) == Some(prefix)
        && bytes
            .get(start..start + name.len())
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
        && delimited(bytes, start + name.len())
}

fn end_tag(bytes: &[u8], from: usize, name: &[u8]) -> usize {
    let mut at = from;
    while let Some(lt) = find_byte(bytes, at, b'<') {
        if tag_at(bytes, lt, b"</", name) {
            return lt;
        }
        at = lt + 1;
    }
    bytes.len()
}

fn script_end(bytes: &[u8], from: usize) -> usize {
    #[derive(PartialEq)]
    enum State {
        Data,
        Escaped,
        DoubleEscaped,
    }
    let mut state = State::Data;
    let mut at = from;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if state != State::Data && rest.starts_with(b"-->") {
            state = State::Data;
            at += 3;
        } else if state == State::Data && rest.starts_with(b"<!--") {
            state = State::Escaped;
            at += 4;
        } else if tag_at(bytes, at, b"</", b"script") {
            if state != State::DoubleEscaped {
                return at;
            }
            state = State::Escaped;
            at += 8;
        } else if state == State::Escaped && tag_at(bytes, at, b"<", b"script") {
            state = State::DoubleEscaped;
            at += 7;
        } else {
            at += 1;
        }
    }
    bytes.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_and_style_contents_are_dropped_and_their_tags_kept() {
        assert_eq!(
            strip("<p>a</p><script type='x'>var s = 1 < 2;</script><STYLE>p{}</Style ><p>b</p>")
                .unwrap(),
            "<p>a</p><script type='x'></script><STYLE></Style ><p>b</p>"
        );
        assert_eq!(strip("<p>a<script>never closed").unwrap(), "<p>a<script>");
        assert_eq!(
            strip("<scripts>kept</scripts>").unwrap(),
            "<scripts>kept</scripts>"
        );
        assert_eq!(
            strip("<script-x>kept</script-x>").unwrap(),
            "<script-x>kept</script-x>"
        );
    }

    #[test]
    fn a_closing_tag_inside_a_string_ends_the_script_as_the_tokenizer_does() {
        assert_eq!(
            strip("<script>var s = \"</script>\";</script><p>x</p>").unwrap(),
            "<script></script>\";</script><p>x</p>"
        );
        assert_eq!(
            strip("<script>a</scriptx>b</script >c").unwrap(),
            "<script></script >c"
        );
        assert_eq!(
            strip("<script><!--<script>x</script>y--></script><p>z</p>").unwrap(),
            "<script></script><p>z</p>"
        );
        assert_eq!(
            strip("<script><!-- a --></script><p>z</p>").unwrap(),
            "<script></script><p>z</p>"
        );
    }

    #[test]
    fn tags_only_count_where_the_tokenizer_reads_tags() {
        for kept in [
            "<!-- <script> --><p>a</p>",
            "<div title=\"<script>\">a</div><p>b</p>",
            "<div title = '<style>'>a</div>",
            "<textarea><script>a</textarea><p>b</p>",
            "<title><style></title><p>b</p>",
            "</div title=\"<script>\"><p>b</p>",
            "<!doctype html><? <script> ?><p>a</p>",
            "<plaintext><script>a",
            "a < b <3 <<script",
        ] {
            assert_eq!(strip(kept).unwrap(), kept);
        }
    }

    #[test]
    fn the_page_limit_applies_after_stripping() {
        let at = "a".repeat(MAX_RESULTS_PAGE_BYTES);
        assert_eq!(strip(&at).unwrap().len(), MAX_RESULTS_PAGE_BYTES);
        let over = "a".repeat(MAX_RESULTS_PAGE_BYTES + 1);
        assert_eq!(strip(&over), Err(Failure::Limit));
        let wrapped = format!(
            "<p>a</p><script>{}</script>",
            "x".repeat(MAX_RESULTS_PAGE_BYTES)
        );
        assert_eq!(strip(&wrapped).unwrap(), "<p>a</p><script></script>");
    }

    #[test]
    fn the_source_is_cut_at_a_character_boundary() {
        let lead = "<p>a</p><script>";
        let fill = MAX_RESULTS_SOURCE_BYTES - lead.len() - 1;
        let source = format!("{lead}{}é<p>after the cap</p>", "x".repeat(fill));
        assert!(!source.is_char_boundary(MAX_RESULTS_SOURCE_BYTES));
        assert_eq!(strip(&source).unwrap(), "<p>a</p><script>");
        assert_eq!(floor_boundary("é", 1), 0);
        assert_eq!(floor_boundary("é", 9), 2);
    }

    #[test]
    fn a_page_over_the_parser_limit_parses_once_stripped() {
        let url = url::Url::parse("https://duckduckgo.com/").unwrap();
        let html = format!(
            "<main><article><h2><a href='https://acme.com/'>Acme</a></h2></article></main><script>{}</script>",
            "x".repeat(MAX_RESULTS_PAGE_BYTES * 2)
        );
        assert!(Doc::from_html(&url, html.as_bytes(), None, DocLimits::RESULTS).is_err());
        let page = ResultsPage {
            html,
            url,
            truncated: false,
        };
        assert!(parse(&page).is_ok());
    }
}
