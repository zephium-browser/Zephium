//! Match declarations compiled by the same implementation in both processes.

use url::Url;
pub use zephium_webext_matcher::{
    ContentMatchError, ContentScriptMatcher, DocumentOrigin, FrameDocument, HostPattern,
    MatchOptions,
};

use crate::protocol::{document_origin, FrameInfo, MAX_URI_BYTES};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MatchRule {
    pub matcher: ContentScriptMatcher,
    pub all_frames: bool,
    pub file_access: bool,
}

impl MatchRule {
    /// Compatibility helper for a host-authored, exact HTTP(S) origin grant.
    pub fn for_origin(origin: &str) -> Result<Self, ContentMatchError> {
        let source = if document_origin(origin).as_deref() == Some(origin)
            && (origin.starts_with("http://") || origin.starts_with("https://"))
        {
            format!("{origin}/*")
        } else {
            String::new()
        };
        Ok(Self {
            matcher: ContentScriptMatcher::new(&[source], &[], &[], &[], MatchOptions::default())?,
            all_frames: true,
            file_access: false,
        })
    }

    pub fn matches(&self, frame: &FrameInfo) -> bool {
        if (!self.all_frames && !frame.is_main)
            || frame.uri.len() > MAX_URI_BYTES
            || frame.uri.chars().any(char::is_control)
        {
            return false;
        }
        let Ok(url) = Url::parse(&frame.uri) else {
            return false;
        };
        let creator = frame.creator_uri.as_deref().and_then(|uri| {
            (uri.len() <= MAX_URI_BYTES && !uri.chars().any(char::is_control))
                .then(|| Url::parse(uri).ok())
                .flatten()
        });
        // A sandboxed ordinary URL must not acquire an extension principal.
        if frame.opaque && !matches!(url.scheme(), "about" | "data" | "blob" | "filesystem") {
            return false;
        }
        let origin = if frame.opaque {
            DocumentOrigin::opaque(creator.as_ref())
        } else if url.scheme() == "about" {
            creator
                .as_ref()
                .map(DocumentOrigin::of_url)
                .unwrap_or_else(|| DocumentOrigin::of_url(&url))
        } else {
            DocumentOrigin::of_url(&url)
        };
        self.matcher.matches(
            &FrameDocument {
                url: &url,
                origin: &origin,
                creator_url: creator.as_ref(),
            },
            self.file_access,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalRule {
    pub install_id: String,
    pub matches: Vec<MatchRule>,
    pub bootstrap: String,
}

impl ExternalRule {
    pub fn admits(&self, frame: &FrameInfo) -> bool {
        if frame.opaque
            || frame.uri.len() > MAX_URI_BYTES
            || frame.uri.chars().any(char::is_control)
        {
            return false;
        }
        Url::parse(&frame.uri).ok().is_some_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && !zephium_webext_matcher::is_extension_gallery(&url)
                && self.matches.iter().any(|rule| rule.matches(frame))
        })
    }
}

pub fn external_matches(rules: &[ExternalRule], frame: &FrameInfo) -> bool {
    rules.iter().any(|rule| rule.admits(frame))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(uri: &str) -> FrameInfo {
        FrameInfo {
            uri: uri.into(),
            is_main: true,
            ..FrameInfo::default()
        }
    }

    #[test]
    fn broad_grants_reach_new_origins_but_preserve_exclusions_and_frame_scope() {
        let mut rule = MatchRule {
            matcher: ContentScriptMatcher::new(
                &["<all_urls>"],
                &["https://private.test/*"],
                &["*/news/*"],
                &["*draft*"],
                MatchOptions::default(),
            )
            .expect("matcher"),
            all_frames: false,
            file_access: false,
        };
        assert!(rule.matches(&frame("https://previously-unknown.test/news/1")));
        assert!(!rule.matches(&frame("https://private.test/news/1")));
        assert!(!rule.matches(&frame("https://a.test/news/draft")));
        let mut child = frame("https://a.test/news/1");
        child.is_main = false;
        assert!(!rule.matches(&child));
        rule.all_frames = true;
        assert!(rule.matches(&child));
        child.opaque = true;
        assert!(!rule.matches(&child));
    }

    #[test]
    fn related_documents_require_explicit_options_and_creator_facts() {
        let rule = MatchRule {
            matcher: ContentScriptMatcher::new(
                &["https://a.test/page"],
                &[],
                &[],
                &[],
                MatchOptions {
                    match_about_blank: true,
                    match_origin_as_fallback: false,
                },
            )
            .expect("matcher"),
            all_frames: true,
            file_access: false,
        };
        let mut blank = frame("about:blank");
        assert!(!rule.matches(&blank));
        blank.creator_uri = Some("https://a.test/page".into());
        assert!(rule.matches(&blank));
        blank.opaque = true;
        assert!(!rule.matches(&blank));
        assert!(MatchRule::for_origin("https://a.test/path").is_err());
    }
    #[test]
    fn external_matches_preserve_url_paths_and_refuse_opaque_documents() {
        let external = ExternalRule {
            install_id: "a".repeat(32),
            matches: vec![MatchRule {
                matcher: ContentScriptMatcher::new(
                    &["https://a.test/allowed/*"],
                    &[],
                    &[],
                    &[],
                    MatchOptions::default(),
                )
                .expect("external matcher"),
                all_frames: true,
                file_access: false,
            }],
            bootstrap: "void 0".into(),
        };
        assert!(external.admits(&frame("https://a.test/allowed/page")));
        assert!(!external.admits(&frame("https://a.test/other/page")));
        let mut opaque = frame("https://a.test/allowed/page");
        opaque.opaque = true;
        assert!(!external.admits(&opaque));
        assert!(!external.admits(&frame("data:text/html,allowed/page")));
    }
}
