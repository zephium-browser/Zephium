//! Small, deterministic search policy shared by every browser entry point.
use crate::navigation;
use url::Url;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SearchEngine {
    Google,
    #[default]
    DuckDuckGo,
    Bing,
    Brave,
    Custom,
}

impl SearchEngine {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "google" => Some(Self::Google),
            "duckduckgo" => Some(Self::DuckDuckGo),
            "bing" => Some(Self::Bing),
            "brave" => Some(Self::Brave),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    /// The id the `search.engine` preference stores.
    pub fn id(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::DuckDuckGo => "duckduckgo",
            Self::Bing => "bing",
            Self::Brave => "brave",
            Self::Custom => "custom",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Google => "Google",
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Bing => "Bing",
            Self::Brave => "Brave Search",
            Self::Custom => "Custom search",
        }
    }

    pub fn search(self, query: &str) -> Option<Url> {
        // Keep the navigation policy's sensitive-input and encoded-size guards.
        navigation::search_query(query)?;
        let base = match self {
            Self::Google => "https://www.google.com/search",
            Self::DuckDuckGo => "https://duckduckgo.com/",
            Self::Bing => "https://www.bing.com/search",
            Self::Brave => "https://search.brave.com/search",
            Self::Custom => return None,
        };
        let mut url = Url::parse(base).ok()?;
        url.query_pairs_mut().append_pair("q", query.trim());
        navigation::is_allowed(&url).then_some(url)
    }

    /// The engine's own suggestion endpoint, which answers in the OpenSearch
    /// JSON form browsers read (`["query", ["suggestion", ...]]`). A custom
    /// engine names none.
    pub fn suggestions(self, query: &str) -> Option<Url> {
        let (base, key, extra): (&str, &str, &[(&str, &str)]) = match self {
            Self::Google => (
                "https://www.google.com/complete/search",
                "q",
                &[("client", "firefox"), ("ie", "utf-8"), ("oe", "utf-8")],
            ),
            Self::DuckDuckGo => ("https://duckduckgo.com/ac/", "q", &[("type", "list")]),
            Self::Bing => ("https://www.bing.com/osjson.aspx", "query", &[]),
            Self::Brave => ("https://search.brave.com/api/suggest", "q", &[]),
            Self::Custom => return None,
        };
        let mut url = Url::parse(base).ok()?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair(key, query.trim());
            for (name, value) in extra {
                pairs.append_pair(name, value);
            }
        }
        Some(url)
    }

    pub fn configured_search(self, query: &str, template: &str) -> Option<Url> {
        if self != Self::Custom {
            return self.search(query);
        }
        navigation::search_query(query)?;
        if !valid_template(template) {
            return None;
        }
        let encoded: String =
            url::form_urlencoded::byte_serialize(query.trim().as_bytes()).collect();
        let url = Url::parse(&template.replace("{searchTerms}", &encoded)).ok()?;
        navigation::is_allowed(&url).then_some(url)
    }

    pub fn configured_classify(self, input: &str, template: &str) -> Option<Url> {
        if let Some((engine, query)) = engine_shortcut(input) {
            return engine.configured_search(query, template);
        }
        if navigation::is_query(input) {
            self.configured_search(input, template)
        } else {
            navigation::classify(input)
        }
    }

    pub fn classify(self, input: &str) -> Option<Url> {
        if navigation::is_query(input) {
            self.search(input)
        } else {
            navigation::classify(input)
        }
    }
}

pub fn valid_template(template: &str) -> bool {
    if template.len() > 2048 || template.matches("{searchTerms}").count() != 1 {
        return false;
    }
    let Ok(url) = Url::parse(&template.replace("{searchTerms}", "zephium-query")) else {
        return false;
    };
    url.scheme() == "https"
        && navigation::is_allowed(&url)
        && url
            .query()
            .is_some_and(|query| query.contains("zephium-query"))
        && !url
            .host_str()
            .is_some_and(|host| host.contains("zephium-query"))
}

pub fn engine_shortcut(input: &str) -> Option<(SearchEngine, &str)> {
    let (prefix, query) = input.trim().split_once(char::is_whitespace)?;
    let engine = match prefix {
        "!g" => SearchEngine::Google,
        "!d" => SearchEngine::DuckDuckGo,
        "!b" => SearchEngine::Bing,
        "!br" => SearchEngine::Brave,
        _ => return None,
    };
    Some((engine, query.trim()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchScope {
    All,
    Tabs,
    History,
    Notes,
    Commands,
}

pub fn scoped_query(input: &str) -> (SearchScope, &str) {
    let input = input.trim();
    for (prefix, scope) in [
        ("@tabs", SearchScope::Tabs),
        ("@history", SearchScope::History),
        ("@notes", SearchScope::Notes),
        (">", SearchScope::Commands),
    ] {
        if let Some(rest) = input.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return (scope, rest.trim_start());
            }
        }
    }
    (SearchScope::All, input)
}

/// Partial queries need a stricter policy than an explicit submitted search.
pub fn remote_query_allowed(input: &str) -> bool {
    let (scope, query) = scoped_query(input);
    scope == SearchScope::All
        && engine_shortcut(input).is_none()
        && (2..=256).contains(&query.chars().count())
        && query.len() <= 1024
        && navigation::is_query(query)
        && !query.contains(['@', '/', '\\', ':'])
        && !query.chars().any(char::is_control)
}

/// Display sections in their fixed presentation order. A result's position
/// never depends on which provider answered first, so late note or suggestion
/// arrivals extend their own section instead of reshuffling the whole list.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResultSection {
    /// What the user typed, plus the online suggestions that continue it.
    Search,
    Tabs,
    History,
    Notes,
    Commands,
}

/// Capacity is counted per kind rather than per section, because a section can
/// mix sources that are worth very different amounts to the user. History holds
/// both pages they visited and queries they previously submitted; under a
/// single shared cap a browser that has been used for a while fills every slot
/// with its own past queries and stops offering pages at all.
pub fn kind_capacity(kind: &str) -> usize {
    match kind {
        // What the user typed. Exactly one, always first.
        "search" | "url" => 1,
        "suggestion" => 3,
        "tab" => 3,
        "history" | "bookmark" => 4,
        // A past query is not a destination, and retyping it is cheap.
        "search_history" => 1,
        "note" => 2,
        "command" => 2,
        _ => 1,
    }
}

/// Kinds that draw on one allowance. A bookmark is a page the user has been
/// to, so adding bookmarks to the list never makes it longer.
pub fn capacity_group(kind: &str) -> &str {
    match kind {
        "bookmark" => "history",
        other => other,
    }
}

/// The wire `kind` is the single source of a result's section.
pub fn result_section(kind: &str) -> ResultSection {
    match kind {
        "tab" => ResultSection::Tabs,
        "history" | "bookmark" | "search_history" => ResultSection::History,
        "note" => ResultSection::Notes,
        "command" => ResultSection::Commands,
        _ => ResultSection::Search,
    }
}

/// Hard ceiling applied after the per-kind caps.
pub const MAX_RESULTS: usize = 14;

/// Inline completion for the field itself, Safari-style. Only a host prefix
/// qualifies: completing mid-sentence prose would fight the typist, and a path
/// completion is rarely the one intended.
pub fn host_completion(query: &str, address: &str) -> Option<String> {
    let typed = query.trim();
    // A bare host prefix only. `is_query` is the wrong gate here: it rejects
    // anything containing a dot, which is exactly the input worth completing.
    if typed.len() < 2
        || typed.contains(char::is_whitespace)
        || typed.contains(['/', ':', '?', '#', '@', '\\'])
    {
        return None;
    }
    let typed = typed.to_lowercase();
    let url = Url::parse(address).ok()?;
    if !matches!(url.scheme(), "https" | "http") || !navigation::is_allowed(&url) {
        return None;
    }
    let host = url.host_str()?.to_lowercase();
    // "git" completes to "github.com" whether or not the visit was to www.
    let bare = host.strip_prefix("www.").unwrap_or(&host);
    let candidate = if bare.starts_with(&typed) {
        bare
    } else if host.starts_with(&typed) {
        &host
    } else {
        return None;
    };
    (candidate.len() > typed.len()).then(|| candidate.to_owned())
}

/// Matching is deterministic and independent of provider timing. Higher is better.
pub fn match_quality(query: &str, title: &str, address: &str) -> u16 {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return 1;
    }
    let title = title.to_lowercase();
    let address = address.to_lowercase();
    let host = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))
        .unwrap_or(&address);
    if title == query || host.trim_end_matches('/') == query {
        return 100;
    }
    if title.starts_with(&query) || host.starts_with(&query) {
        return 80;
    }
    if query
        .split_whitespace()
        .all(|word| title.contains(word) || address.contains(word))
    {
        return 40;
    }
    0
}

/// Merges bookmarks into visited pages, each scored by `match_quality`. Pages
/// keep the order they arrived in, which is their own frecency; a bookmark
/// goes ahead of the first page it matches at least as well, so at equal
/// quality the page the user chose to keep wins.
pub fn interleave_bookmarks<T>(mut bookmarks: Vec<(u16, T)>, pages: Vec<(u16, T)>) -> Vec<T> {
    bookmarks.sort_by_key(|(quality, _)| std::cmp::Reverse(*quality));
    let mut bookmarks = bookmarks.into_iter().peekable();
    let mut merged = Vec::with_capacity(bookmarks.len() + pages.len());
    for (quality, page) in pages {
        while let Some((_, bookmark)) = bookmarks.next_if(|(rank, _)| *rank >= quality) {
            merged.push(bookmark);
        }
        merged.push(page);
    }
    merged.extend(bookmarks.map(|(_, bookmark)| bookmark));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_named_engine_suggests_over_https_and_a_custom_one_does_not() {
        for engine in [
            SearchEngine::Google,
            SearchEngine::DuckDuckGo,
            SearchEngine::Bing,
            SearchEngine::Brave,
        ] {
            let url = engine.suggestions(" rust book ").expect("named engine");
            assert_eq!(url.scheme(), "https");
            assert!(url.query_pairs().any(|(_, value)| value == "rust book"));
        }
        assert!(SearchEngine::Custom.suggestions("rust").is_none());
    }
    #[test]
    fn engines_encode_queries_and_preserve_navigation_policy() {
        for engine in [
            SearchEngine::Google,
            SearchEngine::DuckDuckGo,
            SearchEngine::Bing,
            SearchEngine::Brave,
        ] {
            assert_eq!(SearchEngine::from_id(engine.id()), Some(engine));
            let url = engine.search("rust & svelte 日本").unwrap();
            assert_eq!(
                url.query_pairs().collect::<Vec<_>>(),
                vec![("q".into(), "rust & svelte 日本".into())]
            );
            assert_eq!(
                engine.classify("example.com").unwrap().as_str(),
                "https://example.com/"
            );
            for value in [
                "file:///private.txt",
                "/Users/me/secret",
                "https://user:password@example.com",
                "javascript:alert(1)",
            ] {
                assert!(engine.classify(value).is_none());
                assert!(engine.search(value).is_none());
            }
        }
    }
    #[test]
    fn relevance_prefers_exact_and_prefix_matches() {
        assert!(
            match_quality("github", "GitHub", "") > match_quality("github", "GitHub issue", "")
        );
        assert!(
            match_quality("github", "GitHub issue", "")
                > match_quality("github", "A github issue", "")
        );
        assert!(match_quality("rust guide", "Guide to Rust", "") > 0);
        assert_eq!(match_quality("rust guide", "Rust", ""), 0);
    }

    #[test]
    fn a_bookmark_leads_a_page_it_matches_equally_well() {
        let merged = interleave_bookmarks(
            vec![(80, "bookmark"), (40, "bookmark weak")],
            vec![(80, "page"), (100, "page exact"), (40, "page weak")],
        );
        assert_eq!(
            merged,
            [
                "bookmark",
                "page",
                "page exact",
                "bookmark weak",
                "page weak"
            ]
        );
        assert_eq!(interleave_bookmarks(vec![(1, 'b')], vec![]), ['b']);
        assert_eq!(interleave_bookmarks(vec![], vec![(1, 'p')]), ['p']);
    }

    #[test]
    fn bookmarks_share_the_history_allowance_and_section() {
        assert_eq!(capacity_group("bookmark"), "history");
        assert_eq!(capacity_group("tab"), "tab");
        assert_eq!(result_section("bookmark"), ResultSection::History);
        assert_eq!(
            kind_capacity(capacity_group("bookmark")),
            kind_capacity("history")
        );
    }

    #[test]
    fn sections_order_independently_of_provider_arrival() {
        let mut sections = [
            ResultSection::Commands,
            ResultSection::Notes,
            ResultSection::Search,
            ResultSection::History,
            ResultSection::Tabs,
        ];
        sections.sort();
        assert_eq!(
            sections,
            [
                ResultSection::Search,
                ResultSection::Tabs,
                ResultSection::History,
                ResultSection::Notes,
                ResultSection::Commands,
            ]
        );
        assert_eq!(result_section("search"), ResultSection::Search);
        assert_eq!(result_section("suggestion"), ResultSection::Search);
        assert_eq!(result_section("url"), ResultSection::Search);
        assert_eq!(result_section("search_history"), ResultSection::History);
        assert_eq!(result_section("note"), ResultSection::Notes);
    }

    #[test]
    fn inline_completion_only_extends_a_host_prefix() {
        assert_eq!(
            host_completion("git", "https://github.com/rust-lang").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host_completion("git", "https://www.github.com/").as_deref(),
            Some("github.com")
        );
        // A partially typed host is the case this exists for.
        assert_eq!(
            host_completion("doc.rust-lang", "https://doc.rust-lang.org/book/").as_deref(),
            Some("doc.rust-lang.org")
        );
        for (query, address) in [
            // Already complete: nothing to append.
            ("github.com", "https://github.com/"),
            // Too short to be a confident completion.
            ("g", "https://github.com/"),
            // Multi-word input is prose, not a host.
            ("git hub", "https://github.com/"),
            // The prefix must match the host, not the path.
            ("rust", "https://github.com/rust-lang"),
            // Anything already carrying a scheme or path is not a host prefix.
            ("https", "https://github.com/"),
            ("github.com/rust", "https://github.com/rust-lang"),
            // Never complete toward a disallowed destination.
            ("pri", "file:///private.txt"),
            ("exa", "https://user:password@example.com/"),
        ] {
            assert_eq!(host_completion(query, address), None, "{query} {address}");
        }
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn partial_input_policy_never_sends_local_scopes_or_addresses() {
        for query in [
            "@notes personal",
            "@history invoice",
            "@tabs mail",
            "> reload",
            "someone@example.com",
            "https://example.com",
            "/Users/me/private",
            "!g private",
            "x",
            "a\nb",
        ] {
            assert!(!remote_query_allowed(query), "{query}");
        }
        assert!(remote_query_allowed("rust language"));
    }
    #[test]
    fn custom_templates_cannot_change_origin_or_escape_query_encoding() {
        let template = "https://example.com/search?q={searchTerms}";
        assert!(valid_template(template));
        for bad in [
            "http://example.com/?q={searchTerms}",
            "https://{searchTerms}.example.com/",
            "https://u:p@example.com/?q={searchTerms}",
            "https://example.com/",
            "https://example.com/?q={searchTerms}&x={searchTerms}",
        ] {
            assert!(!valid_template(bad));
        }
        let url = SearchEngine::Custom
            .configured_search("a&admin=true", template)
            .unwrap();
        assert_eq!(url.query_pairs().count(), 1);
        assert_eq!(url.query_pairs().next().unwrap().1, "a&admin=true");
        assert_eq!(
            SearchEngine::DuckDuckGo
                .configured_classify("!d example.com", "")
                .unwrap()
                .host_str(),
            Some("duckduckgo.com")
        );
        assert_eq!(
            SearchEngine::DuckDuckGo
                .configured_classify("!g example.com", "")
                .unwrap()
                .host_str(),
            Some("www.google.com")
        );
    }
}

/// Recognizes built-in search result pages for local source labeling.
pub fn is_search_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        matches!(
            url.host_str(),
            Some("www.google.com" | "duckduckgo.com" | "www.bing.com" | "search.brave.com")
        ) && url.query_pairs().any(|(key, _)| key == "q")
    })
}
