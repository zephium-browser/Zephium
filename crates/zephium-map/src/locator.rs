use crate::{
    doc::Role,
    key::{KeyError, MAX_LOCATOR_BYTES},
    Doc,
};
use serde::{Deserialize, Serialize};
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Hash)]
#[serde(try_from = "String")]
pub struct LocatorText(String);
impl TryFrom<String> for LocatorText {
    type Error = KeyError;
    fn try_from(raw: String) -> Result<Self, KeyError> {
        Self::new(&raw)
    }
}

impl LocatorText {
    pub fn new(text: &str) -> Result<Self, KeyError> {
        if text.len() > MAX_LOCATOR_BYTES {
            return Err(KeyError::Locator);
        }
        let cleaned = crate::clean_text(text);
        if cleaned.trim().is_empty() || text.chars().any(char::is_control) {
            return Err(KeyError::Locator);
        }
        Ok(Self(cleaned))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Locator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<LocatorText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<TextQuote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<TextPosition>,
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextQuote {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<LocatorText>,
    pub start: LocatorText,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<LocatorText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<LocatorText>,
}

/// Half-open UTF-8 offsets in visible block text, whitespace collapsed and blocks joined by one space.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextPosition {
    pub start: u32,
    pub end: u32,
}

struct Projection {
    text: String,
    spans: Vec<(usize, std::ops::Range<usize>)>,
}
impl Projection {
    fn new(doc: &Doc) -> Self {
        let nodes = doc.nodes();
        let mut descendants = vec![false; nodes.len()];
        let mut included = vec![false; nodes.len()];
        for (i, n) in nodes.iter().enumerate().rev() {
            if !crate::visible(nodes, i) {
                continue;
            }
            included[i] = !descendants[i] && !n.text.trim().is_empty();
            if let Some(p) = n.parent_index() {
                descendants[p] |= descendants[i] || included[i];
            }
        }
        // Paragraphs and headings already contain the text of their inline descendants.
        let mut captured = vec![false; nodes.len()];
        for (i, n) in nodes.iter().enumerate() {
            let parent_captured = n.parent_index().is_some_and(|p| captured[p]);
            let block = matches!(
                n.role,
                Role::Paragraph
                    | Role::Heading { .. }
                    | Role::Cell
                    | Role::CellHeader
                    | Role::CodeBlock
            ) && !n.text.trim().is_empty()
                && crate::visible(nodes, i);
            included[i] = !parent_captured && (included[i] || block);
            captured[i] = parent_captured || block;
        }
        let mut text = String::new();
        let mut spans = Vec::new();
        for (i, n) in nodes.iter().enumerate().filter(|(i, _)| included[*i]) {
            let words = crate::flatten(&n.text);
            if words.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            let start = text.len();
            text.push_str(&words);
            spans.push((i, start..text.len()));
        }
        Self { text, spans }
    }
    fn nodes(&self, start: usize, end: usize) -> Option<std::ops::Range<u32>> {
        let first = self
            .spans
            .get(self.spans.partition_point(|(_, r)| r.end <= start))?
            .0;
        let last = self
            .spans
            .get(self.spans.partition_point(|(_, r)| r.end < end))?
            .0;
        Some(first as u32..last as u32 + 1)
    }
}

impl Locator {
    pub(crate) fn for_text(doc: &Doc, node: usize, text: &str, id: Option<&str>) -> Option<Self> {
        let mut loc = crate::site::quote(text)?;
        loc.element = id.and_then(|s| LocatorText::new(s).ok());
        let projection = Projection::new(doc);
        let start_node = projection.spans.partition_point(|(i, _)| *i < node);
        let Some((first, span)) = projection.spans.get(start_node) else {
            return Some(loc);
        };
        if !crate::ancestors(doc.nodes(), *first).any(|i| i == node) {
            return Some(loc);
        }
        let quote = loc.quote.as_mut()?;
        let start = span.start;
        let phrase = quote.start.as_str();
        if projection.text[start..].starts_with(phrase) {
            let end = start + phrase.len();
            let full = crate::flatten(text);
            let end_text = if projection.text[start..].starts_with(&full) {
                start + full.len()
            } else {
                end
            };
            loc.position = Some(TextPosition {
                start: start as u32,
                end: end_text as u32,
            });
            if projection.text.match_indices(phrase).nth(1).is_some() {
                let before = projection.text[..start].trim_end();
                let after = projection.text[end_text..].trim_start();
                quote.prefix = LocatorText::new(
                    &before[before
                        .ceil_char_boundary(before.len().saturating_sub(MAX_LOCATOR_BYTES))..],
                )
                .ok();
                quote.suffix =
                    LocatorText::new(&after[..after.floor_char_boundary(MAX_LOCATOR_BYTES)]).ok();
            }
        }
        Some(loc)
    }

    pub fn fragment(&self) -> Option<String> {
        fn encode(s: &LocatorText, quote: bool) -> String {
            use std::fmt::Write;
            let mut out = String::new();
            for b in s.as_str().bytes() {
                if b.is_ascii_alphanumeric() || b"._~".contains(&b) || !quote && b == b'-' {
                    out.push(char::from(b));
                } else {
                    let _ = write!(out, "%{b:02X}");
                }
            }
            out
        }
        if let Some(id) = &self.element {
            return Some(if crate::key::route(id.as_str()) {
                format!("#{}", id.as_str())
            } else {
                format!("#{}", encode(id, false))
            });
        }
        if let Some(q) = &self.quote {
            let mut out = String::from("#:~:text=");
            if let Some(prefix) = &q.prefix {
                out.push_str(&encode(prefix, true));
                out.push_str("-,");
            }
            out.push_str(&encode(&q.start, true));
            if let Some(end) = &q.end {
                out.push(',');
                out.push_str(&encode(end, true));
            }
            if let Some(suffix) = &q.suffix {
                out.push_str(",-");
                out.push_str(&encode(suffix, true));
            }
            Some(out)
        } else {
            None
        }
    }

    pub fn resolve(&self, doc: &Doc) -> Option<std::ops::Range<u32>> {
        let nodes = doc.nodes();
        let element = self.element.as_ref().and_then(|id| {
            let mut matches = nodes
                .iter()
                .enumerate()
                .filter(|(i, n)| n.selector_id() == Some(id.as_str()) && crate::visible(nodes, *i));
            let (at, n) = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            let end = if let Role::Heading { level } = n.role {
                (at + 1..nodes.len())
                    .find(|&i| {
                        matches!(nodes[i].role,Role::Heading{level:l} if l<=level)
                            && crate::visible(nodes, i)
                            || n.parent_index()
                                .is_some_and(|p| !crate::ancestors(nodes, i).any(|a| a == p))
                    })
                    .unwrap_or(nodes.len())
            } else {
                (at + 1..nodes.len())
                    .find(|&i| nodes[i].parent_index().is_none_or(|p| p < at))
                    .unwrap_or(nodes.len())
            };
            Some(at as u32..end as u32)
        });
        if let Some(q) = &self.quote {
            let projection = Projection::new(doc);
            let text = &projection.text;
            let mut candidates = [(0u32, None), (0, None), (0, None), (0, None)];
            let mut ends = q
                .end
                .as_ref()
                .map(|end| text.match_indices(end.as_str()).peekable());
            for (at, phrase) in text.match_indices(q.start.as_str()) {
                let after = at + phrase.len();
                let end = if let Some(matches) = &mut ends {
                    while matches.peek().is_some_and(|(pos, _)| *pos < after) {
                        matches.next();
                    }
                    let Some((pos, s)) = matches.peek() else {
                        break;
                    };
                    pos + s.len()
                } else {
                    after
                };
                let range = projection.nodes(at, end)?;
                let scoped = element
                    .as_ref()
                    .is_some_and(|e| range.start >= e.start && range.end <= e.end);
                let context = q
                    .prefix
                    .as_ref()
                    .is_none_or(|s| text[..at].trim_end().ends_with(s.as_str()))
                    && q.suffix
                        .as_ref()
                        .is_none_or(|s| text[end..].trim_start().starts_with(s.as_str()));
                for (slot, eligible) in candidates.iter_mut().zip([
                    scoped && context,
                    scoped,
                    context,
                    q.prefix.is_none() && q.suffix.is_none(),
                ]) {
                    if eligible {
                        slot.0 += 1;
                        slot.1 = Some(range.clone());
                    }
                }
            }
            for (count, range) in candidates {
                if count == 1 {
                    return range;
                }
            }
        }
        element.map(|e| e.start..e.start + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cleaned_id_falls_back_to_its_quote_and_never_captures_an_unchanged_id() {
        let doc = Doc::from_html(
            &url::Url::parse("https://example.com/").unwrap(),
            "<main><h2 id='a\u{200b}b'>First topic</h2><p>one</p><h2 id='ab'>Second topic</h2><p>two</p></main>".as_bytes(),
            None,
            crate::DocLimits::default(),
        )
        .unwrap();
        let nodes = doc.nodes();
        let at = |text: &str| nodes.iter().position(|n| n.text == text).unwrap();
        let (first, second) = (at("First topic"), at("Second topic"));
        assert_eq!(nodes[first].element_id.as_deref(), Some("ab"));
        let locate =
            |i: usize| Locator::for_text(&doc, i, &nodes[i].text, nodes[i].selector_id()).unwrap();
        let cleaned = locate(first);
        assert!(cleaned.element.is_none() && cleaned.quote.is_some());
        assert_eq!(cleaned.resolve(&doc).map(|r| r.start), Some(first as u32));
        let kept = locate(second);
        assert_eq!(kept.element.as_ref().map(LocatorText::as_str), Some("ab"));
        assert_eq!(kept.fragment().as_deref(), Some("#ab"));
        assert_eq!(kept.resolve(&doc).map(|r| r.start), Some(second as u32));
        let element = Locator {
            element: kept.element,
            ..Locator::default()
        };
        assert_eq!(element.resolve(&doc).map(|r| r.start), Some(second as u32));
    }
}
