//! The resource vocabulary shared by the compiler and persistent decoder.

use adblock::filters::network::NetworkFilterMask;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ResourceType {
    ChildDocument,
    Fetch,
    Font,
    Image,
    Media,
    Other,
    Ping,
    Script,
    StyleSheet,
    SvgDocument,
    TopDocument,
    Websocket,
}

impl ResourceType {
    // Canonical lexical order is part of the artifact format. Derive the
    // decoder's cardinality bound from this same mapping, never a second
    // platform vocabulary that can drift independently.
    const MAPPING: [(NetworkFilterMask, Self); 12] = [
        (NetworkFilterMask::FROM_SUBDOCUMENT, Self::ChildDocument),
        (NetworkFilterMask::FROM_XMLHTTPREQUEST, Self::Fetch),
        (NetworkFilterMask::FROM_FONT, Self::Font),
        (NetworkFilterMask::FROM_IMAGE, Self::Image),
        (NetworkFilterMask::FROM_MEDIA, Self::Media),
        (NetworkFilterMask::FROM_OTHER, Self::Other),
        (NetworkFilterMask::FROM_PING, Self::Ping),
        (NetworkFilterMask::FROM_SCRIPT, Self::Script),
        (NetworkFilterMask::FROM_STYLESHEET, Self::StyleSheet),
        (NetworkFilterMask::FROM_OBJECT, Self::SvgDocument),
        (NetworkFilterMask::FROM_DOCUMENT, Self::TopDocument),
        (NetworkFilterMask::FROM_WEBSOCKET, Self::Websocket),
    ];

    pub(crate) const COUNT: usize = Self::MAPPING.len();

    pub(crate) fn for_mask(mask: NetworkFilterMask) -> Vec<Self> {
        Self::MAPPING
            .iter()
            .filter_map(|(flag, resource)| mask.contains(*flag).then_some(*resource))
            .collect()
    }
}

/// WebKit supplies parsed, canonical network URLs: even an input consisting
/// only of an HTTP(S)/WS(S) authority has a trailing `/`. For the converter's
/// strict hostname-only pattern the match cannot cross that authority slash,
/// so its end-of-URL alternative is unreachable. Removing the optional `.*`
/// tail avoids a very large native DFA while retaining the existing prefix,
/// separator, scope, resource-type and exception behavior.
///
/// Do not apply this to paths, wildcards, full regexes, or explicit right
/// anchors: their meaningful end-of-URL branch must remain intact.
pub(crate) fn simplify_canonical_host_boundary(pattern: &str) -> Option<String> {
    const PREFIX: &str = r"^[^:]+:(//)?([^/]+\.)?";
    const SUFFIX: &str = "([^A-Za-z0-9_.%-].*)?$";
    let host = pattern.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?;
    let mut labels = 0;
    for label in host.split(r"\.") {
        if label.is_empty()
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return None;
        }
        labels += 1;
    }
    if labels < 2 {
        return None;
    }
    Some(format!("{PREFIX}{host}[^A-Za-z0-9_.%-]"))
}

/// A trailing `^` becomes `([^A-Za-z0-9_.%-].*)?$`: the pattern then ends
/// the URL or is followed by a separator. That is the same as two patterns
/// without `.*`, the pattern ending the URL and the pattern followed by a
/// separator anywhere, so a rule with no other `.*` need not get an automaton
/// of its own.
pub(crate) fn split_separator_end(pattern: &str) -> Option<[String; 2]> {
    const SUFFIX: &str = "([^A-Za-z0-9_.%-].*)?$";
    let stem = pattern.strip_suffix(SUFFIX)?;
    if stem.is_empty() || stem.contains(".*") || stem.ends_with('\\') {
        return None;
    }
    Some([format!("{stem}$"), format!("{stem}[^A-Za-z0-9_.%-]")])
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    #[test]
    fn only_literal_hostname_boundaries_are_rewritten() {
        let old = r"^[^:]+:(//)?([^/]+\.)?ads\.example([^A-Za-z0-9_.%-].*)?$";
        assert_eq!(
            simplify_canonical_host_boundary(old).as_deref(),
            Some(r"^[^:]+:(//)?([^/]+\.)?ads\.example[^A-Za-z0-9_.%-]")
        );
        for pattern in [
            r"^[^:]+:(//)?([^/]+\.)?ads\.example/path([^A-Za-z0-9_.%-].*)?$",
            r"^[^:]+:(//)?([^/]+\.)?ads.*\.example([^A-Za-z0-9_.%-].*)?$",
            r"^[^:]+:(//)?([^/]+\.)?ads\.example([^A-Za-z0-9_.%-])?$",
            r"/ad([^A-Za-z0-9_.%-].*)?$",
            r"^[^:]+:(//)?([^/]+\.)?localhost([^A-Za-z0-9_.%-].*)?$",
        ] {
            assert_eq!(simplify_canonical_host_boundary(pattern), None, "{pattern}");
        }
    }
    #[test]
    fn rewrite_preserves_matching_for_canonical_network_url_variants() {
        let original = r"^[^:]+:(//)?([^/]+\.)?ads\.example([^A-Za-z0-9_.%-].*)?$";
        let optimized = simplify_canonical_host_boundary(original).unwrap();
        let old = regex::Regex::new(original).unwrap();
        let new = regex::Regex::new(&optimized).unwrap();
        for scheme in ["http", "https", "ws", "wss"] {
            for authority in [
                "ads.example",
                "sub.ads.example",
                "ads.example.evil",
                "safe.example",
                "u:p@ads.example",
                "ads.example@safe.example",
                "ads.example:8080",
                "ads.example.",
            ] {
                for tail in [
                    "",
                    "/",
                    "?q=ads.example",
                    "#fragment",
                    "/path",
                    "/path@other.example",
                    "/ads.example",
                ] {
                    let input = format!("{scheme}://{authority}{tail}");
                    let url = url::Url::parse(&input).unwrap();
                    assert_eq!(
                        old.is_match(url.as_str()),
                        new.is_match(url.as_str()),
                        "{input} -> {url}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_separator_end_splits_into_two_rules_that_match_the_same_urls() {
        for pattern in [
            r"^[^:]+:(//)?([^/]+\.)?ads\.example/path([^A-Za-z0-9_.%-].*)?$",
            r"/banner/ad([^A-Za-z0-9_.%-].*)?$",
            r"\?type=placement([^A-Za-z0-9_.%-].*)?$",
        ] {
            let [at_end, before_separator] = split_separator_end(pattern).unwrap();
            assert!(!at_end.contains(".*") && !before_separator.contains(".*"));
            let original = regex::Regex::new(pattern).unwrap();
            let split = [
                regex::Regex::new(&at_end).unwrap(),
                regex::Regex::new(&before_separator).unwrap(),
            ];
            for stem in [
                "https://ads.example/path",
                "https://sub.ads.example/path",
                "https://x.test/banner/ad",
                "https://x.test/p?type=placement",
            ] {
                for tail in [
                    "", "/", "?q=1", ":80", "x", "-", "_", ".", "%", "a/b", "/x.js",
                ] {
                    let url = format!("{stem}{tail}");
                    assert_eq!(
                        original.is_match(&url),
                        split.iter().any(|regex| regex.is_match(&url)),
                        "{pattern} on {url}"
                    );
                }
            }
        }
        for pattern in [
            r"/a.*/b([^A-Za-z0-9_.%-].*)?$",
            r"([^A-Za-z0-9_.%-].*)?$",
            r"/a\([^A-Za-z0-9_.%-].*)?$",
            r"/ad([^A-Za-z0-9_.%-])?$",
        ] {
            assert_eq!(split_separator_end(pattern), None, "{pattern}");
        }
    }
}
