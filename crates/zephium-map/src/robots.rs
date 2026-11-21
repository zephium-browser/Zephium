use crate::{key::MAX_URL_BYTES, PageKey};
use serde::{Deserialize, Serialize};
use url::Url;

pub const MAX_ROBOTS_BYTES: usize = 512 * 1024;
pub const MAX_RULES: usize = 512;
pub const MAX_PATTERN_BYTES: usize = 512;
pub const MAX_SITEMAPS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RobotsOutcome {
    Parsed,
    Missing,
    Unreadable,
    Limit,
}
#[derive(Clone, Serialize, Deserialize)]
struct Rule {
    pattern: String,
    allow: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Robots {
    pub outcome: RobotsOutcome,
    #[serde(skip)]
    pub(crate) truncated: bool,
    rules: Vec<Rule>,
    pub sitemaps: Vec<Url>,
}

impl Robots {
    pub(crate) fn limited() -> Self {
        Self::empty(RobotsOutcome::Limit)
    }
    pub fn unavailable(missing: bool) -> Self {
        Self::empty(if missing {
            RobotsOutcome::Missing
        } else {
            RobotsOutcome::Unreadable
        })
    }
    fn empty(outcome: RobotsOutcome) -> Self {
        Self {
            outcome,
            truncated: false,
            rules: Vec::new(),
            sitemaps: Vec::new(),
        }
    }
    pub fn parse(text: &str) -> Self {
        Self::from_sample(text.as_bytes(), false)
    }
    pub fn from_sample(bytes: &[u8], truncated: bool) -> Self {
        let prefix = &bytes[..bytes.len().min(MAX_ROBOTS_BYTES)];
        let readable = match std::str::from_utf8(prefix) {
            Ok(text) => text,
            Err(e) => std::str::from_utf8(&prefix[..e.valid_up_to()]).unwrap_or_default(),
        };
        if readable
            .trim_start_matches(|c: char| c == '\u{feff}' || c.is_whitespace())
            .starts_with('<')
            || prefix.contains(&0)
        {
            return Self::unavailable(false);
        }
        let truncated = truncated || prefix.len() < bytes.len();
        let complete = if truncated {
            prefix
                .iter()
                .rposition(|&b| matches!(b, b'\r' | b'\n'))
                .map_or(&[][..], |end| &prefix[..=end])
        } else {
            prefix
        };
        let Ok(text) = std::str::from_utf8(complete) else {
            return Self::unavailable(false);
        };
        let limited = || Self::empty(RobotsOutcome::Limit);
        let mut result = Self::empty(RobotsOutcome::Parsed);
        result.truncated = truncated;
        let mut groups = [Vec::new(), Vec::new()];
        let mut selected = 0;
        let mut directives = false;
        let mut read_rule = false;
        let mut specific = false;
        let mut rules = 0;
        for line in text.trim_start_matches('\u{feff}').split(['\r', '\n']) {
            let line = line.split('#').next().unwrap_or_default();
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match field.trim().to_ascii_lowercase().as_str() {
                "user-agent" => {
                    if directives {
                        selected = 0;
                        directives = false;
                    }
                    if value.eq_ignore_ascii_case("zephium") {
                        selected = 2;
                        specific = true;
                    } else if value == "*" && selected == 0 {
                        selected = 1;
                    }
                }
                "allow" | "disallow" => {
                    directives = true;
                    read_rule = true;
                    if selected == 0 || value.is_empty() {
                        continue;
                    }
                    if rules >= MAX_RULES || value.len() > MAX_PATTERN_BYTES {
                        return limited();
                    }
                    rules += 1;
                    let pattern = crate::key::normalize_percent(value);
                    if pattern.len() > MAX_PATTERN_BYTES {
                        return limited();
                    }
                    groups[selected - 1].push(Rule {
                        pattern,
                        allow: field.trim().eq_ignore_ascii_case("allow"),
                    });
                }
                "sitemap" if value.len() <= MAX_URL_BYTES => {
                    if let Ok(url) = Url::parse(value) {
                        if PageKey::from_url(&url).is_ok() && !result.sitemaps.contains(&url) {
                            if result.sitemaps.len() >= MAX_SITEMAPS {
                                continue;
                            }
                            result.sitemaps.push(url);
                        }
                    }
                }
                _ => {}
            }
        }
        if truncated && !read_rule {
            let mut unread = Self::unavailable(false);
            unread.truncated = true;
            return unread;
        }
        result.rules = std::mem::take(&mut groups[usize::from(specific)]);
        result
    }
    pub fn allows(&self, path: &str) -> bool {
        if path.len() > MAX_URL_BYTES
            || matches!(
                self.outcome,
                RobotsOutcome::Unreadable | RobotsOutcome::Limit
            )
        {
            return false;
        }
        let path = crate::key::normalize_percent(path);
        self.rules
            .iter()
            .filter(|r| matches_path(&r.pattern, &path))
            .map(|r| {
                (
                    r.pattern
                        .trim_end_matches('$')
                        .bytes()
                        .filter(|&b| b != b'*')
                        .count(),
                    r.allow,
                )
            })
            .max()
            .is_none_or(|(_, allow)| allow)
    }
    pub(crate) fn valid(&self) -> bool {
        self.rules.len() <= MAX_RULES
            && self
                .rules
                .iter()
                .all(|r| r.pattern.len() <= MAX_PATTERN_BYTES)
            && self.sitemaps.len() <= MAX_SITEMAPS
            && self.sitemaps.iter().all(|u| PageKey::from_url(u).is_ok())
    }
}

fn matches_path(pattern: &str, path: &str) -> bool {
    let anchored = pattern.ends_with('$');
    let pattern = pattern.strip_suffix('$').unwrap_or(pattern);
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = path.strip_prefix(first) else {
        return false;
    };
    let Some(last) = parts.next_back() else {
        return !anchored || rest.is_empty();
    };
    if anchored {
        let Some(prefix) = rest.strip_suffix(last) else {
            return false;
        };
        rest = prefix;
    }
    for part in parts {
        let Some((_, tail)) = rest.split_once(part) else {
            return false;
        };
        rest = tail;
    }
    anchored || rest.contains(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reader_prefix_ends_at_last_complete_line_even_inside_unicode() {
        let prefix = b"User-agent: *\rDisallow: /private\r\nAllow: /private/\xc3";
        let parsed = Robots::from_sample(prefix, true);
        assert_eq!(parsed.outcome, RobotsOutcome::Parsed);
        assert!(parsed.allows("/"));
        assert!(!parsed.allows("/private/path"));
        assert_eq!(
            Robots::from_sample(prefix, false).outcome,
            RobotsOutcome::Unreadable
        );
    }

    #[test]
    fn a_truncated_sample_without_a_complete_rule_denies() {
        for (sample, truncated) in [
            (&b"U"[..], true),
            (b"User-agent: *\nDisallow: /", true),
            (
                b"\xef\xbb\xbf  <!doctype html><html><body>User-agent: *",
                true,
            ),
            (b"  <html>\nUser-agent: *\nDisallow: /private\n", true),
            (b"<html>\nUser-agent: *\nAllow: /\n", false),
            (b"# robots for example\n# Disallow: /\n#", true),
            (
                b"User-agent: *\nSitemap: https://example.com/map.xml\nDisa",
                true,
            ),
        ] {
            let parsed = Robots::from_sample(sample, truncated);
            assert_eq!(parsed.outcome, RobotsOutcome::Unreadable, "{sample:?}");
            assert!(!parsed.allows("/"));
            assert!(parsed.sitemaps.is_empty());
        }
        let prefix = b"User-agent: *\nDisallow: /private\nUser-agent: zeph";
        let parsed = Robots::from_sample(prefix, true);
        assert_eq!(parsed.outcome, RobotsOutcome::Parsed);
        assert!(parsed.allows("/") && !parsed.allows("/private"));
        let other = Robots::from_sample(b"User-agent: other\nDisallow: /\nUser", true);
        assert_eq!(other.outcome, RobotsOutcome::Parsed);
        assert!(other.allows("/"));
    }

    #[test]
    fn an_untruncated_empty_or_comment_only_file_allows_all() {
        for text in ["", "\n", "# nothing here\n# Disallow: /", "User-agent: *\n"] {
            let parsed = Robots::parse(text);
            assert_eq!(parsed.outcome, RobotsOutcome::Parsed, "{text:?}");
            assert!(parsed.allows("/"));
        }
        let oversized = format!(
            "{}User-agent: *\nDisallow: /",
            "#\n".repeat(MAX_ROBOTS_BYTES)
        );
        assert_eq!(Robots::parse(&oversized).outcome, RobotsOutcome::Unreadable);
    }
}
