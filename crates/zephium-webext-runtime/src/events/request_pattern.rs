//! webRequest filters accept more schemes than content-script admission.

use crate::ApiError;
use url::Url;
use zephium_core::injection::{
    MatchPattern, MatchPatternComponents, MatchPatternPort, MAX_MATCH_PATTERN_BYTES,
    MAX_MATCH_URL_BYTES,
};

const EXTRA_SCHEMES: &[&str] = &["ftp", "ws", "wss", "chrome-extension", "uuid-in-package"];

#[derive(Clone, Debug)]
pub(super) enum RequestPattern {
    All,
    Web(MatchPattern),
    Extra {
        scheme: &'static str,
        pattern: MatchPattern,
    },
}

impl RequestPattern {
    pub(super) fn parse(source: &str) -> Result<Self, ApiError> {
        let invalid = || ApiError::other(format!("'{source}' is not a valid URL pattern."));
        if source.len() > MAX_MATCH_PATTERN_BYTES {
            return Err(ApiError::message_too_large());
        }
        if source.chars().any(char::is_control) {
            return Err(invalid());
        }
        if source == "<all_urls>" {
            return Ok(Self::All);
        }
        let (scheme, rest) = source.split_once("://").ok_or_else(invalid)?;
        if scheme == "file" {
            let path = rest.find('/').map_or("/*", |index| &rest[index..]);
            return MatchPattern::parse(&format!("file://{path}"))
                .map(Self::Web)
                .map_err(|_| invalid());
        }
        if let Some(scheme) = EXTRA_SCHEMES
            .iter()
            .copied()
            .find(|candidate| *candidate == scheme)
        {
            let pattern = MatchPattern::parse(&format!("http://{rest}")).map_err(|_| invalid())?;
            return Ok(Self::Extra { scheme, pattern });
        }
        MatchPattern::parse(source)
            .map(Self::Web)
            .map_err(|_| invalid())
    }

    pub(super) fn matches_url(&self, url: &Url) -> bool {
        if url.as_str().len() > MAX_MATCH_URL_BYTES {
            return false;
        }
        match self {
            Self::All => {
                matches!(url.scheme(), "http" | "https" | "file")
                    || EXTRA_SCHEMES.contains(&url.scheme())
            }
            Self::Web(pattern) => pattern.matches_url(url),
            Self::Extra { scheme, pattern } => {
                if url.scheme() != *scheme {
                    return false;
                }
                if let MatchPatternComponents::Standard {
                    port: MatchPatternPort::Exact(port),
                    ..
                } = pattern.components()
                {
                    if url.port_or_known_default() != Some(port) {
                        return false;
                    }
                }
                let Ok(mut compatible) =
                    Url::parse(&format!("http{}", &url[url::Position::AfterScheme..]))
                else {
                    return false;
                };
                if compatible.set_port(url.port_or_known_default()).is_err() {
                    return false;
                }
                pattern.matches_url(&compatible)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_schemes_do_not_expand_content_admission() {
        for (pattern, target) in [
            (
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/*.html*",
                "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/popup.html?x=1",
            ),
            ("file://*/*.user.js", "file:///tmp/a.user.js"),
            ("wss://example.test/*", "wss://example.test/socket"),
            ("ftp://example.test:21/*", "ftp://example.test/file"),
        ] {
            assert!(RequestPattern::parse(pattern)
                .unwrap()
                .matches_url(&Url::parse(target).unwrap()));
            assert!(MatchPattern::parse(pattern).is_err());
        }
        let own =
            RequestPattern::parse("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/*").unwrap();
        assert!(!own.matches_url(
            &Url::parse("chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/popup.html").unwrap()
        ));
        assert!(!RequestPattern::parse("*://*/*")
            .unwrap()
            .matches_url(&Url::parse("wss://example.test/socket").unwrap()));
        for pattern in ["data://*/*", "chrome://*/*", "about:blank", "https://a.*/*"] {
            assert_eq!(
                RequestPattern::parse(pattern).unwrap_err().message(),
                format!("'{pattern}' is not a valid URL pattern.")
            );
        }
    }
}
