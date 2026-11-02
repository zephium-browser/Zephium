//! What clearing browsing data asks of the web engine, and what it answers.
//!
//! Zephium's own history, favicons and download list live in the store and
//! never pass through here; only the website data a persistent profile's
//! engine keeps for the pages it loaded does.

use std::sync::Mutex;

/// The engine-owned data a clear removes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineDataKinds {
    /// The engine's own record of visited pages, which colours visited links.
    pub history: bool,
    /// Cookies, local and session storage, IndexedDB, service workers and
    /// cache storage.
    pub site_data: bool,
    /// The HTTP cache.
    pub cache: bool,
}

impl EngineDataKinds {
    pub const fn is_empty(self) -> bool {
        !(self.history || self.site_data || self.cache)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineDataCall {
    /// Removes the kinds modified at or after `since` (unix seconds), or all
    /// of them when it is absent.
    Clear {
        kinds: EngineDataKinds,
        since: Option<i64>,
    },
    /// The sites the profile holds data for.
    Sites,
    /// Everything stored for `site`: its registrable domain or exact host.
    /// `origin` names the page the request came from, for engines that can
    /// only reach storage origin by origin; it is always on `site`.
    ForgetSite {
        site: String,
        origin: Option<String>,
    },
}

/// A host the engine holds data for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineSite {
    pub name: String,
    pub cookies: bool,
    pub storage: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineDataFailure {
    /// The engine could not take the request now.
    Unavailable,
    /// This engine cannot do what was asked.
    Unsupported,
    /// The engine took the request and reported that it did not complete.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineDataReply {
    Cleared,
    Sites {
        sites: Vec<EngineSite>,
        /// The engine can only name sites by some of what it holds.
        partial: bool,
    },
    Failed(EngineDataFailure),
}

type Done = Box<dyn FnOnce(EngineDataReply) + Send>;

/// Exactly-once answer to a data call. A call the engine refuses or drops
/// at teardown answers `Unavailable` instead of leaving its caller waiting.
pub struct EngineDataCompletion(Mutex<Option<Done>>);

impl EngineDataCompletion {
    pub fn new(done: impl FnOnce(EngineDataReply) + Send + 'static) -> Self {
        Self(Mutex::new(Some(Box::new(done))))
    }

    pub fn finish(self, reply: EngineDataReply) {
        if let Some(done) = self.take() {
            done(reply);
        }
    }

    fn take(&self) -> Option<Done> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
}

impl std::fmt::Debug for EngineDataCompletion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EngineDataCompletion")
    }
}

impl Drop for EngineDataCompletion {
    fn drop(&mut self) {
        if let Some(done) = self.take() {
            done(EngineDataReply::Failed(EngineDataFailure::Unavailable));
        }
    }
}

/// Most sites one listing carries.
pub const MAX_ENGINE_SITES: usize = 4096;

/// Longest host a site name may be.
pub const MAX_SITE_BYTES: usize = 253;

/// Whether `site` is a host name or address a data call can act on: no
/// scheme, port, path or wildcard, so it names exactly one site.
pub fn is_site_name(site: &str) -> bool {
    let bare = site
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'));
    !site.is_empty()
        && site.len() <= MAX_SITE_BYTES
        && site.is_ascii()
        && match bare {
            Some(address) => address.parse::<std::net::Ipv6Addr>().is_ok(),
            None => {
                !site.starts_with(['.', '-'])
                    && !site.ends_with(['.', '-'])
                    && !site.contains("..")
                    && site
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
            }
        }
}

/// Longest page origin a data call may carry.
const MAX_ORIGIN_BYTES: usize = 300;

/// The host of `origin` when it is exactly the serialized origin of a web
/// page: scheme, host and a non-default port, nothing else.
pub fn page_origin_host(origin: &str) -> Option<String> {
    if origin.len() > MAX_ORIGIN_BYTES {
        return None;
    }
    let url = url::Url::parse(origin).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.origin().ascii_serialization() == origin)
        .then(|| url.host_str().map(str::to_owned))
        .flatten()
}

/// Whether `host` is `site` or one of its subdomains.
pub fn host_is_on_site(host: &str, site: &str) -> bool {
    let host = host.trim_start_matches('.').trim_end_matches('.');
    let site = site.trim_start_matches('.').trim_end_matches('.');
    !site.is_empty()
        && (host.eq_ignore_ascii_case(site)
            || (host.len() > site.len() + 1
                && host.as_bytes()[host.len() - site.len() - 1] == b'.'
                && host.is_char_boundary(host.len() - site.len())
                && host[host.len() - site.len()..].eq_ignore_ascii_case(site)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_name_is_one_bare_host() {
        for site in [
            "example.com",
            "a-b.example.co.uk",
            "localhost",
            "127.0.0.1",
            "[::1]",
        ] {
            assert!(is_site_name(site), "{site}");
        }
        for site in [
            "",
            "https://example.com",
            "example.com/path",
            "example.com:8080",
            "*.example.com",
            ".example.com",
            "example..com",
            "-example.com",
            "exa mple.com",
            "[not-an-address]",
            "bücher.example",
        ] {
            assert!(!is_site_name(site), "{site}");
        }
        assert!(!is_site_name(&"a".repeat(MAX_SITE_BYTES + 1)));
    }

    #[test]
    fn a_host_is_on_its_site_and_the_subdomains_of_it_only() {
        assert!(host_is_on_site("example.com", "example.com"));
        assert!(host_is_on_site("www.Example.com", "example.com"));
        assert!(host_is_on_site(".example.com", "example.com"));
        assert!(host_is_on_site("a.b.example.com", "b.example.com"));
        assert!(!host_is_on_site("notexample.com", "example.com"));
        assert!(!host_is_on_site("example.com.evil.test", "example.com"));
        assert!(!host_is_on_site("example.com", "www.example.com"));
        assert!(!host_is_on_site("example.com", ""));
    }

    #[test]
    fn a_page_origin_is_a_bare_web_origin() {
        for (origin, host) in [
            ("https://example.com", "example.com"),
            ("https://www.example.com", "www.example.com"),
            ("http://example.com:8080", "example.com"),
            ("https://[::1]:3000", "[::1]"),
        ] {
            assert_eq!(page_origin_host(origin).as_deref(), Some(host), "{origin}");
        }
        for origin in [
            "https://example.com/",
            "https://example.com/path",
            "https://user@example.com",
            "https://example.com?x=1",
            "https://example.com#top",
            "ftp://example.com",
            "javascript:alert(1)",
            "about:blank",
            "",
        ] {
            assert_eq!(page_origin_host(origin), None, "{origin}");
        }
    }

    #[test]
    fn an_abandoned_call_answers_unavailable_once() {
        let (send, receive) = std::sync::mpsc::channel();
        drop(EngineDataCompletion::new(move |reply| {
            send.send(reply).unwrap();
        }));
        assert_eq!(
            receive.recv().unwrap(),
            EngineDataReply::Failed(EngineDataFailure::Unavailable)
        );
        assert!(receive.recv().is_err());
    }

    #[test]
    fn a_finished_call_does_not_answer_again_when_dropped() {
        let (send, receive) = std::sync::mpsc::channel();
        EngineDataCompletion::new(move |reply| {
            send.send(reply).unwrap();
        })
        .finish(EngineDataReply::Cleared);
        assert_eq!(receive.recv().unwrap(), EngineDataReply::Cleared);
        assert!(receive.recv().is_err());
    }

    #[test]
    fn no_kind_is_nothing_to_clear() {
        assert!(EngineDataKinds::default().is_empty());
        for kinds in [
            EngineDataKinds {
                history: true,
                ..Default::default()
            },
            EngineDataKinds {
                site_data: true,
                ..Default::default()
            },
            EngineDataKinds {
                cache: true,
                ..Default::default()
            },
        ] {
            assert!(!kinds.is_empty());
        }
    }
}
