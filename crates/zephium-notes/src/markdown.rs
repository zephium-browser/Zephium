//! What the index needs from a note, read once when the file changes. The
//! editor never depends on this parser agreeing with its own; only titles,
//! previews, search text and link targets come from here.
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use unicode_normalization::UnicodeNormalization;

const MAX_TITLE_CHARS: usize = 256;
const PREVIEW_CHARS: usize = 180;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_LINKS: usize = 128;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outline {
    /// The leading heading's text, when the note opens with one.
    pub heading: Option<String>,
    pub preview: String,
    /// Plain text for full-text search, bounded.
    pub text: String,
    /// `[[target]]` keys, deduplicated, in first-seen order.
    pub links: Vec<String>,
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_WIKILINKS
}

/// Appends `value` with runs of whitespace folded into single spaces.
fn push_collapsed(out: &mut String, value: &str) {
    let leading = value.starts_with(char::is_whitespace);
    for (index, part) in value.split_whitespace().enumerate() {
        if (index > 0 || leading) && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push_str(part);
    }
    if value.ends_with(char::is_whitespace) && !out.is_empty() && !out.ends_with(' ') {
        out.push(' ');
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    let trimmed = value.trim();
    match trimmed.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", trimmed[..end].trim_end()),
        None => trimmed.to_string(),
    }
}

pub fn outline(markdown: &str) -> Outline {
    let mut result = Outline::default();
    let mut heading = String::new();
    let mut preview = String::new();
    // 0: before the first block, 1: inside a leading heading, 2: after it.
    let mut phase = 0u8;
    let mut metadata = false;
    let mut depth = 0usize;
    for event in Parser::new_ext(markdown, options()) {
        let text = match &event {
            Event::Start(Tag::MetadataBlock(_)) => {
                metadata = true;
                continue;
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                metadata = false;
                continue;
            }
            _ if metadata => continue,
            Event::Start(tag) => {
                if depth == 0 && phase == 0 {
                    phase = if matches!(tag, Tag::Heading { .. }) {
                        1
                    } else {
                        2
                    };
                }
                if let Tag::Link {
                    link_type: LinkType::WikiLink { .. },
                    dest_url,
                    ..
                } = tag
                {
                    let key = link_key(dest_url);
                    if !key.is_empty()
                        && result.links.len() < MAX_LINKS
                        && !result.links.contains(&key)
                    {
                        result.links.push(key);
                    }
                }
                depth += 1;
                None
            }
            Event::End(tag) => {
                depth = depth.saturating_sub(1);
                if depth == 0 && phase == 1 {
                    phase = 2;
                    push_collapsed(&mut result.text, " ");
                    continue;
                }
                let inline = matches!(
                    tag,
                    TagEnd::Emphasis
                        | TagEnd::Strong
                        | TagEnd::Strikethrough
                        | TagEnd::Link
                        | TagEnd::Image
                        | TagEnd::Superscript
                        | TagEnd::Subscript
                );
                // Block boundaries separate words in the search text and preview.
                (!inline).then_some(" ")
            }
            Event::Text(value) | Event::Code(value) => Some(value.as_ref()),
            Event::SoftBreak | Event::HardBreak => Some(" "),
            Event::TaskListMarker(_) | Event::Rule => Some(" "),
            _ => None,
        };
        let Some(text) = text else { continue };
        if result.text.len() < MAX_TEXT_BYTES {
            push_collapsed(&mut result.text, text);
        }
        match phase {
            1 => push_collapsed(&mut heading, text),
            2 if preview.chars().count() <= PREVIEW_CHARS => push_collapsed(&mut preview, text),
            _ => {}
        }
    }
    let heading = truncate_chars(&heading, MAX_TITLE_CHARS);
    result.heading = (!heading.is_empty()).then_some(heading);
    result.preview = truncate_chars(&preview, PREVIEW_CHARS);
    if result.text.len() > MAX_TEXT_BYTES {
        let mut end = MAX_TEXT_BYTES;
        while !result.text.is_char_boundary(end) {
            end -= 1;
        }
        result.text.truncate(end);
    }
    result.text = result.text.trim().to_string();
    result
}

/// The comparable form of a link target or a note name: no heading or block
/// anchor, no `.md` suffix, NFC, case-folded, single-spaced.
pub fn link_key(target: &str) -> String {
    let target = target.split(['#', '^']).next().unwrap_or_default().trim();
    let target = target
        .strip_suffix(".md")
        .or_else(|| target.strip_suffix(".MD"))
        .unwrap_or(target);
    let mut key = String::new();
    push_collapsed(&mut key, &target.nfc().collect::<String>().to_lowercase());
    key.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_wikilink_labels_do_not_multiply_following_events() {
        let markdown = "[[a|]] ".repeat(16);
        let limit = 4 * 16 + 1;
        let count = Parser::new_ext(&markdown, options())
            .take(limit + 1)
            .count();
        assert_eq!(count, limit);
    }

    #[test]
    fn a_leading_heading_is_the_title_and_the_rest_previews() {
        let outline = outline("# Trip  *planning*\n\nBook the **train**.\n\n- [ ] pack");
        assert_eq!(outline.heading.as_deref(), Some("Trip planning"));
        assert_eq!(outline.preview, "Book the train. pack");
        assert_eq!(outline.text, "Trip planning Book the train. pack");
    }

    #[test]
    fn a_note_without_a_leading_heading_has_no_heading() {
        let outline = outline("Just a thought.\n\n# Later heading");
        assert_eq!(outline.heading, None);
        assert_eq!(outline.preview, "Just a thought. Later heading");
    }

    #[test]
    fn setext_headings_and_frontmatter_are_understood() {
        let outline = outline("---\ntags: [a]\n---\nTitle\n=====\n\nBody");
        assert_eq!(outline.heading.as_deref(), Some("Title"));
        assert_eq!(outline.preview, "Body");
        assert!(!outline.text.contains("tags"));
    }

    #[test]
    fn wiki_links_are_collected_by_key() {
        let outline = outline(
            "See [[Other Note]], [[other note#Part|alias]] and [[Plans.md]].\n\n`[[not a link]]`",
        );
        assert_eq!(outline.links, vec!["other note", "plans"]);
        assert!(outline.preview.contains("alias"));
    }

    #[test]
    fn previews_are_bounded_on_character_boundaries() {
        let outline = outline(&format!("# T\n\n{}", "é".repeat(400)));
        assert_eq!(outline.preview.chars().count(), PREVIEW_CHARS + 1);
        assert!(outline.preview.ends_with('…'));
    }

    #[test]
    fn keys_fold_case_and_normalization() {
        assert_eq!(link_key("Cafe\u{301}  Notes"), link_key("CAFÉ notes"));
        assert_eq!(link_key(" Plan.md#Goals "), "plan");
    }
}
