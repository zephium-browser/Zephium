use crate::{key::MAX_URL_BYTES, PageKey};
use serde::{Deserialize, Serialize};
use url::Url;

pub const MAX_ROBOTS_BYTES: usize = 128 * 1024;
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
            rules: Vec::new(),
            sitemaps: Vec::new(),
        }
    }
    pub fn parse(text: &str) -> Self {
        let limited = || Self::empty(RobotsOutcome::Limit);
        if text.len() > MAX_ROBOTS_BYTES {
            return limited();
        }
        if text
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with('<')
            || text.contains('\0')
        {
            return Self::unavailable(false);
        }
        let mut result = Self::empty(RobotsOutcome::Parsed);
        let mut groups = [Vec::new(), Vec::new()];
        let mut selected = 0;
        let mut directives = false;
        let mut specific = false;
        let mut rules = 0;
        for line in text.trim_start_matches('\u{feff}').lines() {
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
                    if selected == 0 || value.is_empty() {
                        continue;
                    }
                    if rules >= MAX_RULES || value.len() > MAX_PATTERN_BYTES {
                        return limited();
                    }
                    rules += 1;
                    let pattern = normalized_path(value);
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
                                return limited();
                            }
                            result.sitemaps.push(url);
                        }
                    }
                }
                _ => {}
            }
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
        let path = normalized_path(path);
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

fn normalized_path(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut text = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let mut look = chars.clone();
            if let Some((high, low)) = look
                .next()
                .and_then(|v| v.to_digit(16))
                .zip(look.next().and_then(|v| v.to_digit(16)))
            {
                let byte = (high * 16 + low) as u8;
                if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                    text.push(char::from(byte));
                } else {
                    text.push('%');
                    text.push(char::from(HEX[high as usize]));
                    text.push(char::from(HEX[low as usize]));
                }
                chars = look;
                continue;
            }
        }
        if c.is_ascii() {
            text.push(c);
        } else {
            let mut bytes = [0; 4];
            for b in c.encode_utf8(&mut bytes).bytes() {
                text.push('%');
                text.push(char::from(HEX[(b >> 4) as usize]));
                text.push(char::from(HEX[(b & 15) as usize]));
            }
        }
    }
    text
}
