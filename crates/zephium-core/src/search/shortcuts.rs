use super::SearchEngine;
use crate::navigation;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use url::Url;

pub const MAX_SHORTCUTS: usize = 64;
pub const MAX_SHORTCUT_SETTINGS_BYTES: usize = 32 * 1024;
/// Defaults that were once offered and no longer are.
const RETIRED: &[&str] = &[
    "rust-docs",
    "npm",
    "pypi",
    "gitlab",
    "crates",
    "imdb",
    "goodreads",
    "openstreetmap",
];

const PREFIX_STOPLIST: &[&str] = &[
    "a",
    "i",
    "e",
    "x",
    "r",
    "b",
    "d",
    "g",
    "k",
    "w",
    "so",
    "aw",
    "gm",
    "gl",
    "gr",
    "tw",
    "wa",
    "ia",
    "tr",
    "sp",
    "ok",
    "hi",
    "no",
    "yo",
    "go",
    "to",
    "in",
    "on",
    "at",
    "it",
    "is",
    "me",
    "my",
    "we",
    "us",
    "or",
    "an",
    "as",
    "be",
    "do",
    "he",
    "if",
    "of",
    "up",
    "ya",
    "lol",
    "omg",
    "idk",
    "brb",
    "btw",
    "hey",
    "yes",
    "google",
    "brave",
    "perplexity",
    "wiki",
    "images",
    "maps",
    "twitter",
    "crates",
    "crate",
    "translate",
    "rust",
    "twitch",
    "archive",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SearchShortcut {
    pub id: String,
    pub name: String,
    pub keywords: Vec<String>,
    pub search_url: String,
    pub home_url: String,
    pub domain: String,
    pub suggestions: Option<SearchEngine>,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutTrigger {
    Keyword,
    Bang,
    None,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedSearch<'registry, 'input> {
    pub shortcut: Option<&'registry str>,
    pub query: Cow<'input, str>,
    pub trigger: ShortcutTrigger,
}

#[derive(Clone, Debug)]
pub struct ShortcutRegistry(Vec<SearchShortcut>);

impl Default for ShortcutRegistry {
    fn default() -> Self {
        Self(serde_json::from_str(include_str!("shortcuts.json")).expect("bundled shortcuts"))
    }
}

impl ShortcutRegistry {
    pub fn entries(&self) -> &[SearchShortcut] {
        &self.0
    }

    pub fn from_setting(value: &str) -> Option<Self> {
        if value.is_empty() {
            return Some(Self::default());
        }
        if value.len() > MAX_SHORTCUT_SETTINGS_BYTES {
            return None;
        }
        let mut entries: Vec<SearchShortcut> = serde_json::from_str(value).ok()?;
        if entries.len() > MAX_SHORTCUTS {
            return None;
        }
        // A list saved before a default was taken out still names it; it is
        // left behind rather than the whole list refused.
        entries.retain(|entry| !RETIRED.contains(&entry.id.as_str()));
        let defaults = Self::default();
        let mut ids = std::collections::HashSet::new();
        let mut keywords = std::collections::HashSet::new();
        for entry in &entries {
            if entry.id.len() > 80
                || !entry
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-:".contains(&b))
                || !ids.insert(&entry.id)
                || entry.name.trim().is_empty()
                || entry.name.len() > 80
                || entry.name.chars().any(char::is_control)
                || !(1..=8).contains(&entry.keywords.len())
                || entry.keywords.iter().any(|keyword| {
                    !valid_keyword(keyword) || !keywords.insert(keyword.to_ascii_lowercase())
                })
                || !valid_shortcut_template(&entry.search_url)
                || !valid_home(&entry.home_url, &entry.domain)
            {
                return None;
            }
            if let Some(original) = defaults.0.iter().find(|item| item.id == entry.id) {
                if entry.name != original.name
                    || entry.search_url != original.search_url
                    || entry.home_url != original.home_url
                    || entry.domain != original.domain
                    || entry.suggestions != original.suggestions
                {
                    return None;
                }
            } else if !entry.id.starts_with("custom:") || entry.suggestions.is_some() {
                return None;
            }
        }
        Some(Self(entries))
    }

    pub fn get(&self, id: &str) -> Option<&SearchShortcut> {
        self.0.iter().find(|entry| entry.enabled && entry.id == id)
    }

    fn keyword(&self, keyword: &str) -> Option<&SearchShortcut> {
        self.0.iter().find(|entry| {
            entry.enabled
                && entry
                    .keywords
                    .iter()
                    .any(|key| key.eq_ignore_ascii_case(keyword))
        })
    }

    pub fn parse<'registry, 'input>(
        &'registry self,
        input: &'input str,
    ) -> ParsedSearch<'registry, 'input> {
        if let Some((prefix, query)) = input.split_once(' ').filter(|(prefix, _)| {
            prefix.len() >= 2
                && !PREFIX_STOPLIST
                    .iter()
                    .any(|word| word.eq_ignore_ascii_case(prefix))
        }) {
            if let Some(shortcut) = self.keyword(prefix) {
                return ParsedSearch {
                    shortcut: Some(&shortcut.id),
                    query: Cow::Borrowed(query.trim()),
                    trigger: ShortcutTrigger::Keyword,
                };
            }
        }
        for token in input.split_whitespace() {
            if let Some(shortcut) = token.strip_prefix('!').and_then(|key| self.keyword(key)) {
                let start = token.as_ptr() as usize - input.as_ptr() as usize;
                let before = input[..start].trim();
                let after = input[start + token.len()..].trim();
                let query = match (before.is_empty(), after.is_empty()) {
                    (true, _) => Cow::Borrowed(after),
                    (_, true) => Cow::Borrowed(before),
                    _ => Cow::Owned(format!("{before} {after}")),
                };
                return ParsedSearch {
                    shortcut: Some(&shortcut.id),
                    query,
                    trigger: ShortcutTrigger::Bang,
                };
            }
        }
        ParsedSearch {
            shortcut: None,
            query: Cow::Borrowed(input.trim()),
            trigger: ShortcutTrigger::None,
        }
    }

    pub fn resolve(&self, input: &str, engine: SearchEngine, template: &str) -> Option<Url> {
        let parsed = self.parse(input);
        if let Some(shortcut) = parsed.shortcut.and_then(|id| self.get(id)) {
            return shortcut.resolve(&parsed.query);
        }
        engine.configured_classify(input, template)
    }
}

impl SearchShortcut {
    pub fn resolve(&self, query: &str) -> Option<Url> {
        if query.trim().is_empty() {
            return Url::parse(&self.home_url)
                .ok()
                .filter(navigation::is_allowed);
        }
        navigation::search_query(query)?;
        let encoded: String = url::form_urlencoded::byte_serialize(query.trim().as_bytes())
            .collect::<String>()
            .replace('+', "%20");
        let url = Url::parse(&self.search_url.replace("{query}", &encoded)).ok()?;
        navigation::is_allowed(&url).then_some(url)
    }
}

pub fn valid_keyword(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

fn valid_home(value: &str, domain: &str) -> bool {
    value.len() <= 2048
        && Url::parse(value).is_ok_and(|url| {
            url.scheme() == "https"
                && navigation::is_allowed(&url)
                && url.host_str() == Some(domain)
        })
}

pub fn valid_shortcut_template(value: &str) -> bool {
    if value.len() > 2048 || value.matches("{query}").count() != 1 || value.contains(['\r', '\n']) {
        return false;
    }
    let Ok(url) = Url::parse(&value.replace("{query}", "zephium-query")) else {
        return false;
    };
    url.scheme() == "https"
        && navigation::is_allowed(&url)
        && !url
            .host_str()
            .is_some_and(|host| host.contains("zephium-query"))
        && url.fragment().is_none()
        && (url.path().contains("zephium-query")
            || url
                .query()
                .is_some_and(|query| query.contains("zephium-query")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Case {
        input: String,
        shortcut: Option<String>,
        query: String,
        trigger: ShortcutTrigger,
        #[serde(default)]
        disabled: Vec<String>,
        #[serde(default)]
        custom: bool,
        #[serde(default)]
        custom_keywords: Option<Vec<String>>,
    }

    fn custom() -> SearchShortcut {
        SearchShortcut {
            id: "custom:example".into(),
            name: "Example".into(),
            keywords: vec!["ex".into()],
            search_url: "https://example.com/search?q={query}".into(),
            home_url: "https://example.com/".into(),
            domain: "example.com".into(),
            suggestions: None,
            enabled: true,
        }
    }

    #[test]
    fn shared_parser_cases() {
        let cases: Vec<Case> = serde_json::from_str(include_str!("shortcut-cases.json")).unwrap();
        for case in cases {
            let mut registry = ShortcutRegistry::default();
            for entry in &mut registry.0 {
                entry.enabled = !case.disabled.contains(&entry.id);
            }
            if case.custom {
                let mut entry = custom();
                if let Some(keywords) = case.custom_keywords {
                    entry.keywords = keywords;
                }
                registry.0.push(entry);
            }
            let parsed = registry.parse(&case.input);
            assert_eq!(parsed.shortcut, case.shortcut.as_deref(), "{}", case.input);
            assert_eq!(parsed.query, case.query, "{}", case.input);
            assert_eq!(parsed.trigger, case.trigger, "{}", case.input);
        }
    }

    #[test]
    fn defaults_and_settings_are_bounded_and_round_trip() {
        let registry = ShortcutRegistry::default();
        assert_eq!(registry.entries().len(), 28);
        let stored = serde_json::to_string(registry.entries()).unwrap();
        assert!(stored.len() < MAX_SHORTCUT_SETTINGS_BYTES);
        assert_eq!(
            ShortcutRegistry::from_setting(&stored).unwrap().entries(),
            registry.entries()
        );
        let mut entries = registry.0;
        entries[0].enabled = false;
        entries[9].keywords = vec!["video".into()];
        entries.push(custom());
        let saved = serde_json::to_string(&entries).unwrap();
        let loaded = ShortcutRegistry::from_setting(&saved).unwrap();
        assert_eq!(loaded.parse("video cats").shortcut, Some("youtube"));
        assert_eq!(loaded.parse("g cats").shortcut, None);
        assert_eq!(loaded.parse("ex cats").shortcut, Some("custom:example"));
        assert_eq!(
            ShortcutRegistry::from_setting("").unwrap().entries().len(),
            28
        );
    }

    #[test]
    fn a_list_saved_with_a_retired_default_loads_without_it() {
        let mut entries = ShortcutRegistry::default().0;
        entries[1].enabled = false;
        entries.push(SearchShortcut {
            id: "npm".into(),
            name: "npm".into(),
            keywords: vec!["npm".into()],
            search_url: "https://www.npmjs.com/search?q={query}".into(),
            home_url: "https://www.npmjs.com/".into(),
            domain: "www.npmjs.com".into(),
            suggestions: None,
            enabled: true,
        });
        let loaded =
            ShortcutRegistry::from_setting(&serde_json::to_string(&entries).unwrap()).unwrap();
        assert_eq!(loaded.entries().len(), 28);
        assert!(loaded.entries().iter().all(|entry| entry.id != "npm"));
        assert!(!loaded.entries()[1].enabled);
        assert_eq!(loaded.parse("npm cats").shortcut, None);
    }

    #[test]
    fn malformed_and_ambiguous_settings_are_refused() {
        for invalid in [
            "null",
            "{}",
            "not json",
            &" ".repeat(MAX_SHORTCUT_SETTINGS_BYTES + 1),
        ] {
            assert!(ShortcutRegistry::from_setting(invalid).is_none());
        }
        for edit in [0, 1, 2, 3, 4, 5, 6] {
            let mut entries = vec![custom()];
            match edit {
                0 => entries.push(custom()),
                1 => entries[0].keywords = vec!["a b".into()],
                2 => entries[0].keywords = vec!["EX".into(), "ex".into()],
                3 => entries[0].search_url = "https://{query}.example.com/".into(),
                4 => entries[0].suggestions = Some(SearchEngine::Google),
                5 => entries[0].home_url = "file:///secret".into(),
                _ => entries[0].domain = "other.example".into(),
            }
            assert!(
                ShortcutRegistry::from_setting(&serde_json::to_string(&entries).unwrap()).is_none(),
                "{edit}"
            );
        }
        for invalid in [
            "http://example.com/?q={query}",
            "https://user:secret@example.com/?q={query}",
            "https://example.com/",
            "https://example.com/?q={query}&q={query}",
            "https://example.com/#{query}",
        ] {
            assert!(!valid_shortcut_template(invalid), "{invalid}");
        }
    }

    #[test]
    fn resolution_preserves_query_encoding_home_and_default_engine() {
        let registry = ShortcutRegistry::default();
        let url = registry
            .resolve("yt café & cats? #/", SearchEngine::Bing, "")
            .unwrap();
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            [("search_query".into(), "café & cats? #/".into())]
        );
        let url = registry
            .resolve("!gm a b/c", SearchEngine::Bing, "")
            .unwrap();
        assert!(url.as_str().ends_with("/a%20b%2Fc"));
        assert_eq!(
            registry
                .resolve("yt ", SearchEngine::Bing, "")
                .unwrap()
                .as_str(),
            "https://www.youtube.com/"
        );
        assert_eq!(
            registry
                .resolve("!yt", SearchEngine::Bing, "")
                .unwrap()
                .as_str(),
            "https://www.youtube.com/"
        );
        assert_eq!(
            registry
                .resolve("cats", SearchEngine::Bing, "")
                .unwrap()
                .host_str(),
            Some("www.bing.com")
        );
        assert!(registry
            .resolve("yt file:///secret", SearchEngine::Bing, "")
            .is_none());
    }

    #[test]
    fn ordinary_prefix_parsing_borrows_the_query() {
        let registry = ShortcutRegistry::default();
        assert!(matches!(
            registry.parse("a dog").query,
            Cow::Borrowed("a dog")
        ));
        assert!(matches!(
            registry.parse("yt cats").query,
            Cow::Borrowed("cats")
        ));
        assert!(matches!(
            registry.parse("cats !yt").query,
            Cow::Borrowed("cats")
        ));
    }

    #[test]
    fn all_aliases_keep_bang_support_and_custom_aliases_follow_prefix_rules() {
        let mut registry = ShortcutRegistry::default();
        for entry in registry.entries().iter().filter(|entry| entry.enabled) {
            for keyword in &entry.keywords {
                let input = format!("!{keyword} cats");
                let parsed = registry.parse(&input);
                assert_eq!(parsed.shortcut, Some(entry.id.as_str()), "{keyword}");
                assert_eq!(parsed.trigger, ShortcutTrigger::Bang, "{keyword}");
                assert_eq!(parsed.query, "cats", "{keyword}");
            }
        }
        registry.0 = vec![custom()];
        for keyword in PREFIX_STOPLIST.iter().copied().chain(["z"]) {
            registry.0[0].keywords = vec![keyword.to_ascii_uppercase()];
            assert_eq!(registry.parse(&format!("{keyword} cats")).shortcut, None);
            assert_eq!(
                registry.parse(&format!("!{keyword} cats")).shortcut,
                Some("custom:example")
            );
        }
    }
}
