//! Chrome match patterns, URL globs and related-frame resolution.
//!
//! Pattern parsing and path matching reuse [`crate::injection`], the
//! matcher content scripts already use. This module adds what Chrome layers
//! on top for extensions: host permissions ignore the path, a trailing dot on
//! a host is not significant, `include_globs`/`exclude_globs`, and
//! `match_about_blank`/`match_origin_as_fallback` resolution.

use std::borrow::Cow;
use std::fmt;

use crate::injection::{
    MatchPattern, MatchPatternComponents, MatchPatternHost, MatchPatternPort, MatchPatternScheme,
    MatchSet, MatchSetError, MAX_MATCH_PATTERN_BYTES, MAX_MATCH_URL_BYTES,
};
use url::{Host, Url};

pub use crate::injection::{MatchOptions, MatchPatternError};

/// Maximum UTF-8 bytes in one `include_globs` or `exclude_globs` entry.
pub const MAX_GLOB_BYTES: usize = MAX_MATCH_PATTERN_BYTES;
/// Maximum combined `include_globs` and `exclude_globs` entries per script.
pub const MAX_GLOBS_PER_SCRIPT: usize = 128;
/// URLs longer than this match no content script, so an exclusion can never
/// be bypassed by length.
pub const MAX_GLOB_URL_BYTES: usize = 2 * MAX_MATCH_URL_BYTES;

/// A host-permission pattern. Chrome ignores the path of host permissions,
/// so the stored pattern always ends in `/*`.
#[derive(Clone, PartialEq, Eq)]
pub struct HostPattern {
    pattern: MatchPattern,
}

impl HostPattern {
    /// Parses a manifest or `permissions` API origin pattern.
    pub fn parse(source: &str) -> Result<Self, MatchPatternError> {
        let parsed = MatchPattern::parse(source)?;
        Ok(Self {
            pattern: canonical(&parsed, PathMode::AllPaths)?,
        })
    }

    /// `<all_urls>`.
    pub fn all_urls() -> Self {
        Self {
            pattern: MatchSet::all_urls().includes()[0].clone(),
        }
    }

    /// The canonical form, as `permissions.getAll` reports it.
    pub fn as_str(&self) -> &str {
        self.pattern.as_str()
    }

    /// Whether this pattern covers `url`'s origin.
    pub fn matches_url(&self, url: &Url) -> bool {
        self.pattern.matches_url(&canonical_target(url))
    }

    /// Whether every URL `other` matches is matched here too, as
    /// `permissions.contains` decides it.
    pub fn contains(&self, other: &Self) -> bool {
        let (outer, inner) = (self.pattern.components(), other.pattern.components());
        let MatchPatternComponents::Standard {
            scheme: outer_scheme,
            host: outer_host,
            port: outer_port,
            ..
        } = outer
        else {
            return true;
        };
        let MatchPatternComponents::Standard {
            scheme: inner_scheme,
            host: inner_host,
            port: inner_port,
            ..
        } = inner
        else {
            return false;
        };
        scheme_contains(outer_scheme, inner_scheme)
            && host_contains(outer_host, inner_host)
            && (outer_port == MatchPatternPort::Any || outer_port == inner_port)
    }

    /// Whether this pattern grants every web host (`<all_urls>`, `*://*/*`,
    /// `http://*/*` or `https://*/*`).
    pub fn is_all_hosts(&self) -> bool {
        match self.pattern.components() {
            MatchPatternComponents::AllUrls => true,
            MatchPatternComponents::Standard { host, port, .. } => {
                host == Some(MatchPatternHost::Any) && port == MatchPatternPort::Any
            }
        }
    }

    /// Whether this pattern can match `file:` URLs.
    pub fn includes_file(&self) -> bool {
        self.pattern.components().includes_file()
    }
}

impl fmt::Debug for HostPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HostPattern")
            .field(&self.as_str())
            .finish()
    }
}

impl fmt::Display for HostPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

fn scheme_contains(outer: MatchPatternScheme, inner: MatchPatternScheme) -> bool {
    outer == inner
        || (outer == MatchPatternScheme::HttpAndHttps
            && matches!(inner, MatchPatternScheme::Http | MatchPatternScheme::Https))
}

fn host_contains(outer: Option<MatchPatternHost<'_>>, inner: Option<MatchPatternHost<'_>>) -> bool {
    let (Some(outer), Some(inner)) = (outer, inner) else {
        return outer.is_none() && inner.is_none();
    };
    let within = |domain: &str, base: &str| {
        domain == base
            || domain
                .strip_suffix(base)
                .is_some_and(|prefix| prefix.ends_with('.'))
    };
    match (outer, inner) {
        (MatchPatternHost::Any, _) => true,
        (MatchPatternHost::DomainAndSubdomains(base), MatchPatternHost::ExactDomain(domain))
        | (
            MatchPatternHost::DomainAndSubdomains(base),
            MatchPatternHost::DomainAndSubdomains(domain),
        ) => within(domain, base),
        (outer, inner) => outer == inner,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PathMode {
    Keep,
    AllPaths,
}

/// Rebuilds a parsed pattern without a trailing host dot and, for host
/// permissions, with the path replaced by `/*`.
fn canonical(pattern: &MatchPattern, mode: PathMode) -> Result<MatchPattern, MatchPatternError> {
    let MatchPatternComponents::Standard {
        scheme,
        host,
        port,
        path,
    } = pattern.components()
    else {
        return Ok(pattern.clone());
    };
    let path = match mode {
        PathMode::Keep => path.as_str(),
        PathMode::AllPaths => "/*",
    };
    let host_has_trailing_dot = matches!(
        host,
        Some(MatchPatternHost::ExactDomain(domain) | MatchPatternHost::DomainAndSubdomains(domain))
            if domain.ends_with('.')
    );
    if !host_has_trailing_dot && (mode == PathMode::Keep || path_is_all(pattern)) {
        return Ok(pattern.clone());
    }
    let mut source = String::with_capacity(pattern.as_str().len());
    source.push_str(match scheme {
        MatchPatternScheme::Http => "http",
        MatchPatternScheme::Https => "https",
        MatchPatternScheme::File => "file",
        MatchPatternScheme::HttpAndHttps => "*",
    });
    source.push_str("://");
    match host {
        None => {}
        Some(MatchPatternHost::Any) => source.push('*'),
        Some(MatchPatternHost::ExactDomain(domain)) => source.push_str(strip_dot(domain)),
        Some(MatchPatternHost::DomainAndSubdomains(domain)) => {
            source.push_str("*.");
            source.push_str(strip_dot(domain));
        }
        Some(MatchPatternHost::ExactIpv4(address)) => source.push_str(&address.to_string()),
        Some(MatchPatternHost::ExactIpv6(address)) => {
            source.push('[');
            source.push_str(&address.to_string());
            source.push(']');
        }
    }
    if let MatchPatternPort::Exact(port) = port {
        source.push(':');
        source.push_str(&port.to_string());
    }
    source.push_str(path);
    MatchPattern::parse(&source)
}

fn path_is_all(pattern: &MatchPattern) -> bool {
    matches!(
        pattern.components(),
        MatchPatternComponents::Standard { path, .. } if path.as_str() == "/*"
    )
}

fn strip_dot(domain: &str) -> &str {
    domain.strip_suffix('.').unwrap_or(domain)
}

/// Chrome compares hosts without one trailing dot, on both sides.
fn canonical_target(url: &Url) -> Cow<'_, Url> {
    let Some(Host::Domain(domain)) = url.host() else {
        return Cow::Borrowed(url);
    };
    let Some(stripped) = domain.strip_suffix('.').filter(|rest| !rest.is_empty()) else {
        return Cow::Borrowed(url);
    };
    let mut owned = url.clone();
    match owned.set_host(Some(stripped)) {
        Ok(()) => Cow::Owned(owned),
        Err(_) => Cow::Borrowed(url),
    }
}

/// Why a glob was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobError {
    /// The glob is longer than [`MAX_GLOB_BYTES`].
    TooLong,
    /// The glob contains a control character.
    ControlCharacter,
}

/// An `include_globs`/`exclude_globs` entry: `*` matches any run of
/// characters, `?` exactly one, everything else itself. It is matched against
/// the whole serialized URL.
#[derive(Clone, PartialEq, Eq)]
pub struct UrlGlob {
    source: Box<str>,
    segments: Box<[Box<[u8]>]>,
    leading_star: bool,
    trailing_star: bool,
}

impl UrlGlob {
    /// Parses one glob.
    pub fn parse(source: &str) -> Result<Self, GlobError> {
        if source.len() > MAX_GLOB_BYTES {
            return Err(GlobError::TooLong);
        }
        if source.chars().any(char::is_control) {
            return Err(GlobError::ControlCharacter);
        }
        let bytes = source.as_bytes();
        Ok(Self {
            source: source.into(),
            segments: bytes
                .split(|byte| *byte == b'*')
                .filter(|segment| !segment.is_empty())
                .map(Box::from)
                .collect(),
            leading_star: bytes.first() == Some(&b'*'),
            trailing_star: bytes.last() == Some(&b'*'),
        })
    }

    /// The glob as written.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether `text` matches. Greedy leftmost placement of each
    /// `*`-separated segment is exact for this glob language.
    pub fn matches(&self, text: &str) -> bool {
        let text = text.as_bytes();
        let has_star = self.leading_star || self.trailing_star || self.segments.len() > 1;
        if !has_star {
            return match self.segments.first() {
                Some(only) => only.len() == text.len() && segment_at(only, text, 0),
                None => text.is_empty(),
            };
        }
        let mut segments: &[Box<[u8]>] = &self.segments;
        let mut start = 0;
        let mut end = text.len();
        if !self.leading_star {
            let Some((first, rest)) = segments.split_first() else {
                return true;
            };
            if first.len() > text.len() || !segment_at(first, text, 0) {
                return false;
            }
            start = first.len();
            segments = rest;
        }
        if !self.trailing_star {
            let Some((last, rest)) = segments.split_last() else {
                return true;
            };
            if last.len() > end - start || !segment_at(last, text, end - last.len()) {
                return false;
            }
            end -= last.len();
            segments = rest;
        }
        for segment in segments {
            let Some(found) = find_segment(segment, text, start, end) else {
                return false;
            };
            start = found + segment.len();
        }
        true
    }
}

impl fmt::Debug for UrlGlob {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("UrlGlob")
            .field(&self.as_str())
            .finish()
    }
}

fn segment_at(segment: &[u8], text: &[u8], at: usize) -> bool {
    text.get(at..at + segment.len()).is_some_and(|window| {
        segment
            .iter()
            .zip(window)
            .all(|(expected, actual)| *expected == b'?' || expected == actual)
    })
}

fn find_segment(segment: &[u8], text: &[u8], start: usize, end: usize) -> Option<usize> {
    let last_start = end.checked_sub(segment.len())?;
    (start..=last_start).find(|at| segment_at(segment, text, *at))
}

/// A scheme-host-port origin. Custom hierarchical schemes such as
/// `chrome-extension` have tuple origins in the engine, so this does not
/// follow the `url` crate, which makes them opaque.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TupleOrigin {
    scheme: Box<str>,
    host: Box<str>,
    port: Option<u16>,
}

impl TupleOrigin {
    /// The tuple origin of `url`, if it has one. `file:`, `about:`, `data:`
    /// and other host-less URLs do not.
    pub fn of_url(url: &Url) -> Option<Self> {
        if url.scheme() == "blob" {
            let inner = Url::parse(url.path()).ok()?;
            return (inner.scheme() != "blob")
                .then(|| Self::of_url(&inner))
                .flatten();
        }
        if url.scheme() == "file" {
            return None;
        }
        let host = url.host_str().filter(|host| !host.is_empty())?;
        Some(Self {
            scheme: url.scheme().into(),
            host: host.into(),
            port: url.port(),
        })
    }

    /// The scheme.
    pub fn scheme(&self) -> &str {
        &self.scheme
    }

    /// The serialized host.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The serialization, such as `https://example.com:8443`.
    pub fn serialize(&self) -> String {
        match self.port {
            Some(port) => format!("{}://{}:{port}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }
}

/// A document's security origin as the engine reports it. Sandboxed frames,
/// `data:` documents and the like have an opaque origin that may remember
/// the tuple origin that created it (Chrome's precursor).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DocumentOrigin {
    /// A scheme-host-port origin.
    Tuple(TupleOrigin),
    /// An opaque origin and, when known, the origin that created it.
    Opaque {
        /// The creating document's tuple origin.
        precursor: Option<TupleOrigin>,
    },
}

impl DocumentOrigin {
    /// The origin a document committed at `url` has when nothing sandboxes
    /// it. URLs without a tuple origin give an opaque origin with no precursor.
    pub fn of_url(url: &Url) -> Self {
        match TupleOrigin::of_url(url) {
            Some(origin) => Self::Tuple(origin),
            None => Self::Opaque { precursor: None },
        }
    }

    /// An opaque origin created by a document at `precursor`.
    pub fn opaque(precursor: Option<&Url>) -> Self {
        Self::Opaque {
            precursor: precursor.and_then(TupleOrigin::of_url),
        }
    }

    /// Whether the origin is opaque.
    pub fn is_opaque(&self) -> bool {
        matches!(self, Self::Opaque { .. })
    }

    /// The tuple origin, or for an opaque origin its precursor.
    pub fn tuple_or_precursor(&self) -> Option<&TupleOrigin> {
        match self {
            Self::Tuple(origin) => Some(origin),
            Self::Opaque { precursor } => precursor.as_ref(),
        }
    }

    /// Whether this is exactly the tuple origin of `url`.
    pub fn is_tuple_of(&self, url: &Url) -> bool {
        match self {
            Self::Tuple(origin) => TupleOrigin::of_url(url).as_ref() == Some(origin),
            Self::Opaque { .. } => false,
        }
    }

    /// `MessageSender.origin`: the serialization, `null` when opaque.
    pub fn serialize(&self) -> String {
        match self {
            Self::Tuple(origin) => origin.serialize(),
            Self::Opaque { .. } => "null".to_owned(),
        }
    }

    /// The URL that stands for this origin in pattern checks: the tuple (or
    /// precursor) origin with path `/`.
    pub fn as_match_url(&self) -> Option<Url> {
        let origin = self.tuple_or_precursor()?;
        Url::parse(&format!("{}/", origin.serialize())).ok()
    }
}

/// The engine's view of one frame document, used for related-frame matching.
#[derive(Clone, Copy, Debug)]
pub struct FrameDocument<'a> {
    /// The committed document URL.
    pub url: &'a Url,
    /// The committed document's security origin.
    pub origin: &'a DocumentOrigin,
    /// The URL of the closest parent or opener document whose URL is not
    /// `about:`. Host state only, never page-reported.
    pub creator_url: Option<&'a Url>,
}

/// The URL Chrome matches a content script against for `document`.
///
/// With `match_origin_as_fallback`, an `about:`, `data:`, `blob:` or
/// `filesystem:` document is matched by its origin (or opaque precursor).
/// Otherwise, with `match_about_blank`, an `about:` document that shares its
/// creator's origin is matched by the creator's URL. Anything else is
/// matched by its own URL.
pub fn effective_match_url<'a>(
    document: &FrameDocument<'a>,
    options: MatchOptions,
) -> Cow<'a, Url> {
    let scheme = document.url.scheme();
    if options.match_origin_as_fallback
        && matches!(scheme, "about" | "data" | "blob" | "filesystem")
    {
        return match document.origin.as_match_url() {
            Some(url) => Cow::Owned(url),
            None => Cow::Borrowed(document.url),
        };
    }
    if options.match_about_blank && scheme == "about" {
        if let Some(creator) = document.creator_url {
            if creator.scheme() != "about" && document.origin.is_tuple_of(creator) {
                return Cow::Borrowed(creator);
            }
        }
    }
    Cow::Borrowed(document.url)
}

/// Why a content-script match declaration was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentMatchError {
    /// `matches`/`exclude_matches` were refused.
    Patterns(MatchSetError),
    /// A glob at this index (includes first, then excludes) was refused.
    Glob {
        /// Position across `include_globs` followed by `exclude_globs`.
        index: usize,
        /// Why.
        error: GlobError,
    },
    /// More than [`MAX_GLOBS_PER_SCRIPT`] globs.
    TooManyGlobs,
}

/// The complete Chrome match declaration of one content script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentScriptMatcher {
    set: MatchSet,
    include_globs: Box<[UrlGlob]>,
    exclude_globs: Box<[UrlGlob]>,
}

impl ContentScriptMatcher {
    /// Parses a declaration as `content_scripts` or
    /// `scripting.registerContentScripts` gives it.
    pub fn new<S: AsRef<str>>(
        matches: &[S],
        exclude_matches: &[S],
        include_globs: &[S],
        exclude_globs: &[S],
        options: MatchOptions,
    ) -> Result<Self, ContentMatchError> {
        let normalize = |source: &S| {
            MatchPattern::parse(source.as_ref())
                .and_then(|pattern| canonical(&pattern, PathMode::Keep))
                .map(|pattern| pattern.as_str().to_owned())
                .unwrap_or_else(|_| source.as_ref().to_owned())
        };
        let set = MatchSet::parse(
            matches.iter().map(normalize),
            exclude_matches.iter().map(normalize),
            options,
        )
        .map_err(ContentMatchError::Patterns)?;
        if include_globs.len().saturating_add(exclude_globs.len()) > MAX_GLOBS_PER_SCRIPT {
            return Err(ContentMatchError::TooManyGlobs);
        }
        let parse_globs = |globs: &[S], offset: usize| {
            globs
                .iter()
                .enumerate()
                .map(|(index, glob)| {
                    UrlGlob::parse(glob.as_ref()).map_err(|error| ContentMatchError::Glob {
                        index: offset + index,
                        error,
                    })
                })
                .collect::<Result<Box<[_]>, _>>()
        };
        Ok(Self {
            set,
            include_globs: parse_globs(include_globs, 0)?,
            exclude_globs: parse_globs(exclude_globs, include_globs.len())?,
        })
    }

    /// The parsed `matches`/`exclude_matches` set.
    pub fn match_set(&self) -> &MatchSet {
        &self.set
    }

    /// Compiled include globs in declaration order.
    pub fn include_globs(&self) -> &[UrlGlob] {
        &self.include_globs
    }

    /// Compiled exclude globs in declaration order.
    pub fn exclude_globs(&self) -> &[UrlGlob] {
        &self.exclude_globs
    }

    /// Whether the script runs in `document`. `file_access` is the person's
    /// "Allow access to file URLs" choice for the install.
    pub fn matches(&self, document: &FrameDocument<'_>, file_access: bool) -> bool {
        let effective = effective_match_url(document, self.set.options());
        let url = canonical_target(&effective);
        if url.as_str().len() > MAX_GLOB_URL_BYTES
            || (url.scheme() == "file" && !file_access)
            || is_extension_gallery(&url)
        {
            return false;
        }
        self.set.matches_url(&url)
            && (self.include_globs.is_empty()
                || self
                    .include_globs
                    .iter()
                    .any(|glob| glob.matches(url.as_str())))
            && !self
                .exclude_globs
                .iter()
                .any(|glob| glob.matches(url.as_str()))
    }
}

/// Chrome never lets extensions script its Web Store.
pub fn is_extension_gallery(url: &Url) -> bool {
    match url
        .host_str()
        .map(|host| host.strip_suffix('.').unwrap_or(host))
    {
        Some("chromewebstore.google.com") => true,
        Some("chrome.google.com") => {
            let path = url.path();
            path == "/webstore" || path.starts_with("/webstore/")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("test URL")
    }

    fn host(value: &str) -> HostPattern {
        HostPattern::parse(value).expect("test pattern")
    }

    fn script(matches: &[&str], options: MatchOptions) -> ContentScriptMatcher {
        ContentScriptMatcher::new(matches, &[], &[], &[], options).expect("test script")
    }

    #[test]
    fn host_permissions_ignore_the_path() {
        let pattern = host("https://example.com/only/this");
        assert_eq!(pattern.as_str(), "https://example.com/*");
        assert!(pattern.matches_url(&url("https://example.com/elsewhere?q")));
        assert!(host("https://*/").matches_url(&url("https://a.test/x")));
    }

    #[test]
    fn wildcard_scheme_is_http_and_https_only() {
        let pattern = host("*://mail.example.com/*");
        assert!(pattern.matches_url(&url("http://mail.example.com/")));
        assert!(pattern.matches_url(&url("https://mail.example.com/")));
        assert!(!pattern.matches_url(&url("file:///mail.example.com/")));
        assert!(HostPattern::parse("ftp://example.com/*").is_err());
        assert!(HostPattern::parse("chrome://settings/*").is_err());
    }

    #[test]
    fn ports_default_to_any_and_exact_ports_use_the_scheme_default() {
        assert!(host("http://localhost/*").matches_url(&url("http://localhost:8080/")));
        let exact = host("https://example.com:443/*");
        assert!(exact.matches_url(&url("https://example.com/")));
        assert!(!exact.matches_url(&url("https://example.com:8443/")));
        assert!(host("http://localhost:*/*").matches_url(&url("http://localhost:1/")));
    }

    #[test]
    fn a_trailing_host_dot_is_insignificant_on_both_sides() {
        assert!(host("https://example.com/*").matches_url(&url("https://example.com./")));
        assert!(host("https://example.com./*").matches_url(&url("https://example.com/")));
        assert!(host("https://*.example.com/*").matches_url(&url("https://a.example.com./")));
        let excluded = ContentScriptMatcher::new(
            &["https://*/*"],
            &["https://bank.test/*"],
            &[],
            &[],
            MatchOptions::default(),
        )
        .expect("script");
        let target = url("https://bank.test./login");
        let origin = DocumentOrigin::of_url(&target);
        let document = FrameDocument {
            url: &target,
            origin: &origin,
            creator_url: None,
        };
        assert!(!excluded.matches(&document, false));
    }

    #[test]
    fn file_patterns_need_three_slashes_and_the_persons_consent() {
        assert!(HostPattern::parse("file://host/*").is_err());
        let file = host("file:///*");
        assert!(file.includes_file());
        let target = url("file:///home/a.html");
        assert!(file.matches_url(&target));
        let origin = DocumentOrigin::of_url(&target);
        let document = FrameDocument {
            url: &target,
            origin: &origin,
            creator_url: None,
        };
        let all = script(&["<all_urls>"], MatchOptions::default());
        assert!(!all.matches(&document, false));
        assert!(all.matches(&document, true));
    }

    #[test]
    fn containment_follows_scheme_host_and_port() {
        assert!(HostPattern::all_urls().contains(&host("file:///*")));
        assert!(!host("https://*/*").contains(&HostPattern::all_urls()));
        assert!(host("*://*.a.test/*").contains(&host("https://b.a.test/*")));
        assert!(host("*://*.a.test/*").contains(&host("http://*.b.a.test/*")));
        assert!(!host("https://*.a.test/*").contains(&host("http://a.test/*")));
        assert!(!host("https://*.a.test/*").contains(&host("https://ba.test/*")));
        assert!(!host("https://a.test:8080/*").contains(&host("https://a.test/*")));
        assert!(host("https://a.test/*").contains(&host("https://a.test:8080/x")));
        assert!(host("https://*/*").is_all_hosts());
        assert!(!host("https://*:8080/*").is_all_hosts());
    }

    #[test]
    fn globs_use_star_and_question_mark() {
        let glob = UrlGlob::parse("https://???.example.com/foo/*").expect("glob");
        assert!(glob.matches("https://the.example.com/foo/bar"));
        assert!(!glob.matches("https://example.com/foo/"));
        let nyt = UrlGlob::parse("*nytimes.com/???s/*").expect("glob");
        assert!(nyt.matches("https://www.nytimes.com/arts/index.html"));
        assert!(!nyt.matches("https://www.nytimes.com/sports/index.html"));
        assert!(UrlGlob::parse("").expect("empty").matches(""));
        assert!(!UrlGlob::parse("").expect("empty").matches("a"));
        assert!(UrlGlob::parse("*").expect("star").matches(""));
    }

    #[test]
    fn globs_narrow_after_matches() {
        let matcher = ContentScriptMatcher::new(
            &["https://*.example.com/*"],
            &[],
            &["*/news/*"],
            &["*?draft*"],
            MatchOptions::default(),
        )
        .expect("script");
        let check = |value: &str| {
            let target = url(value);
            let origin = DocumentOrigin::of_url(&target);
            matcher.matches(
                &FrameDocument {
                    url: &target,
                    origin: &origin,
                    creator_url: None,
                },
                false,
            )
        };
        assert!(check("https://www.example.com/news/1"));
        assert!(!check("https://www.example.com/sport/1"));
        assert!(!check("https://www.example.com/news/1?draft=1"));
        assert!(!check("https://www.other.com/news/1"));
    }

    #[test]
    fn about_blank_inherits_its_same_origin_creators_url() {
        let creator = url("https://a.test/page");
        let blank = url("about:blank");
        let inherited = DocumentOrigin::of_url(&creator);
        let matcher = script(
            &["https://a.test/page"],
            MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: false,
            },
        );
        let document = FrameDocument {
            url: &blank,
            origin: &inherited,
            creator_url: Some(&creator),
        };
        assert!(matcher.matches(&document, false));

        let sandboxed = DocumentOrigin::opaque(Some(&creator));
        let document = FrameDocument {
            origin: &sandboxed,
            ..document
        };
        assert!(!matcher.matches(&document, false));

        let without = script(&["https://a.test/page"], MatchOptions::default());
        let document = FrameDocument {
            url: &blank,
            origin: &inherited,
            creator_url: Some(&creator),
        };
        assert!(!without.matches(&document, false));
    }

    #[test]
    fn about_blank_refuses_a_creator_of_another_origin() {
        let creator = url("https://evil.test/");
        let blank = url("about:blank");
        let origin = DocumentOrigin::of_url(&url("https://a.test/"));
        let matcher = script(
            &["https://evil.test/*"],
            MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: false,
            },
        );
        assert!(!matcher.matches(
            &FrameDocument {
                url: &blank,
                origin: &origin,
                creator_url: Some(&creator),
            },
            false
        ));
    }

    #[test]
    fn origin_fallback_uses_the_origin_or_its_precursor() {
        let creator = url("https://a.test/deep/path");
        let options = MatchOptions {
            match_about_blank: false,
            match_origin_as_fallback: true,
        };
        let matcher = script(&["https://a.test/*"], options);
        for (target, origin) in [
            ("data:text/html,x", DocumentOrigin::opaque(Some(&creator))),
            (
                "blob:https://a.test/0f1d",
                DocumentOrigin::of_url(&url("blob:https://a.test/0f1d")),
            ),
            ("about:srcdoc", DocumentOrigin::of_url(&creator)),
        ] {
            let target = url(target);
            assert!(
                matcher.matches(
                    &FrameDocument {
                        url: &target,
                        origin: &origin,
                        creator_url: None,
                    },
                    false
                ),
                "{target}"
            );
        }
        let lost = DocumentOrigin::opaque(None);
        let data = url("data:text/html,x");
        assert!(!matcher.matches(
            &FrameDocument {
                url: &data,
                origin: &lost,
                creator_url: Some(&creator),
            },
            false
        ));
        assert_eq!(
            ContentScriptMatcher::new(&["https://a.test/x*"], &[], &[], &[], options),
            Err(ContentMatchError::Patterns(
                MatchSetError::FallbackRequiresAllPaths { index: 0 }
            ))
        );
    }

    #[test]
    fn origin_fallback_takes_priority_over_about_blank() {
        let creator = url("https://a.test/exact");
        let blank = url("about:blank");
        let origin = DocumentOrigin::of_url(&creator);
        let document = FrameDocument {
            url: &blank,
            origin: &origin,
            creator_url: Some(&creator),
        };
        let both = MatchOptions {
            match_about_blank: true,
            match_origin_as_fallback: true,
        };
        assert_eq!(
            effective_match_url(&document, both).as_str(),
            "https://a.test/"
        );
    }

    #[test]
    fn the_web_store_is_never_matched() {
        let all = script(&["<all_urls>"], MatchOptions::default());
        for target in [
            "https://chromewebstore.google.com/detail/x",
            "https://chrome.google.com/webstore/detail/x",
            "https://chromewebstore.google.com./",
        ] {
            let target = url(target);
            let origin = DocumentOrigin::of_url(&target);
            assert!(!all.matches(
                &FrameDocument {
                    url: &target,
                    origin: &origin,
                    creator_url: None,
                },
                false
            ));
        }
        assert!(!is_extension_gallery(&url(
            "https://chrome.google.com/webstorex"
        )));
    }

    #[test]
    fn too_many_globs_or_long_globs_are_refused() {
        let many = vec!["*"; MAX_GLOBS_PER_SCRIPT + 1];
        assert_eq!(
            ContentScriptMatcher::new(&["<all_urls>"], &[], &many, &[], MatchOptions::default()),
            Err(ContentMatchError::TooManyGlobs)
        );
        assert_eq!(
            UrlGlob::parse(&"a".repeat(MAX_GLOB_BYTES + 1)),
            Err(GlobError::TooLong)
        );
        assert_eq!(UrlGlob::parse("a\nb"), Err(GlobError::ControlCharacter));
    }

    // A deliberately naive model of Chrome's documented semantics.
    mod model {
        pub fn glob(pattern: &[u8], text: &[u8]) -> bool {
            match pattern.split_first() {
                None => text.is_empty(),
                Some((b'*', rest)) => (0..=text.len()).any(|skip| glob(rest, &text[skip..])),
                Some((b'?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
                Some((byte, rest)) => text.first() == Some(byte) && glob(rest, &text[1..]),
            }
        }

        fn star_glob(pattern: &[u8], text: &[u8]) -> bool {
            match pattern.split_first() {
                None => text.is_empty(),
                Some((b'*', rest)) => (0..=text.len()).any(|skip| star_glob(rest, &text[skip..])),
                Some((byte, rest)) => text.first() == Some(byte) && star_glob(rest, &text[1..]),
            }
        }

        pub struct Pattern<'a> {
            pub scheme: &'a str,
            pub host: &'a str,
            pub port: Option<&'a str>,
            pub path: &'a str,
        }

        pub struct Target<'a> {
            pub scheme: &'a str,
            pub host: &'a str,
            pub port: Option<u16>,
            pub path: &'a str,
        }

        pub fn matches(pattern: &Pattern<'_>, target: &Target<'_>, ignore_path: bool) -> bool {
            let scheme = match pattern.scheme {
                "*" => matches!(target.scheme, "http" | "https"),
                scheme => scheme == target.scheme,
            };
            if !scheme {
                return false;
            }
            if target.scheme != "file" {
                let wanted = pattern.host.trim_end_matches('.');
                let actual = target.host.trim_end_matches('.');
                let host = match wanted {
                    "*" => true,
                    _ => match wanted.strip_prefix("*.") {
                        Some(base) => actual == base || actual.ends_with(&format!(".{base}")),
                        None => actual == wanted,
                    },
                };
                let effective =
                    target
                        .port
                        .unwrap_or(if target.scheme == "https" { 443 } else { 80 });
                let port = match pattern.port {
                    None | Some("*") => true,
                    Some(port) => port.parse::<u16>().ok() == Some(effective),
                };
                if !host || !port {
                    return false;
                }
            }
            ignore_path
                || star_glob(pattern.path.as_bytes(), target.path.as_bytes())
                || pattern
                    .path
                    .strip_suffix("/*")
                    .is_some_and(|directory| !directory.is_empty() && directory == target.path)
        }
    }

    fn pattern_parts() -> impl Strategy<Value = (String, String, Option<String>, String)> {
        let scheme = prop::sample::select(vec!["http", "https", "*", "file"]);
        let host = prop::sample::select(vec![
            "*",
            "a.test",
            "*.a.test",
            "b.a.test",
            "*.b.a.test",
            "127.0.0.1",
            "a.test.",
        ]);
        let port = prop::option::of(prop::sample::select(vec!["*", "80", "443", "8080"]));
        let path = "/[ab/*]{0,5}";
        (scheme, host, port, path).prop_map(|(scheme, host, port, path)| {
            (
                scheme.to_owned(),
                host.to_owned(),
                port.map(str::to_owned),
                path,
            )
        })
    }

    fn target_parts() -> impl Strategy<Value = (String, String, Option<u16>, String)> {
        let scheme = prop::sample::select(vec!["http", "https", "file"]);
        let host = prop::sample::select(vec![
            "a.test",
            "b.a.test",
            "c.b.a.test",
            "ba.test",
            "127.0.0.1",
            "a.test.",
        ]);
        let port = prop::option::of(prop::sample::select(vec![80_u16, 443, 8080]));
        let path = "/[ab/]{0,6}";
        (scheme, host, port, path)
            .prop_map(|(scheme, host, port, path)| (scheme.to_owned(), host.to_owned(), port, path))
    }

    fn build_pattern(scheme: &str, host: &str, port: &Option<String>, path: &str) -> String {
        if scheme == "file" {
            return format!("file://{path}");
        }
        match port {
            Some(port) => format!("{scheme}://{host}:{port}{path}"),
            None => format!("{scheme}://{host}{path}"),
        }
    }

    fn build_target(scheme: &str, host: &str, port: Option<u16>, path: &str) -> Option<Url> {
        let text = if scheme == "file" {
            format!("file://{path}")
        } else {
            match port {
                Some(port) => format!("{scheme}://{host}:{port}{path}"),
                None => format!("{scheme}://{host}{path}"),
            }
        };
        let parsed = Url::parse(&text).ok()?;
        // Skip targets the URL parser rewrites (`//` collapsing is not one,
        // but default ports are dropped), so the model sees what we match.
        (parsed.path() == path).then_some(parsed)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(4096))]

        #[test]
        fn glob_agrees_with_the_reference(pattern in "[ab?*]{0,8}", text in "[ab]{0,10}") {
            let glob = UrlGlob::parse(&pattern).expect("glob");
            prop_assert_eq!(
                glob.matches(&text),
                model::glob(pattern.as_bytes(), text.as_bytes())
            );
        }

        #[test]
        fn patterns_agree_with_the_reference(
            (p_scheme, p_host, p_port, p_path) in pattern_parts(),
            (t_scheme, t_host, t_port, t_path) in target_parts(),
        ) {
            let source = build_pattern(&p_scheme, &p_host, &p_port, &p_path);
            let Some(target) = build_target(&t_scheme, &t_host, t_port, &t_path) else {
                return Ok(());
            };
            let effective_port = target.port();
            let reference_target = model::Target {
                scheme: &t_scheme,
                host: &t_host,
                port: effective_port,
                path: &t_path,
            };
            let reference_pattern = model::Pattern {
                scheme: &p_scheme,
                host: &p_host,
                port: p_port.as_deref(),
                path: &p_path,
            };
            let origin = DocumentOrigin::of_url(&target);
            let document = FrameDocument { url: &target, origin: &origin, creator_url: None };
            if let Ok(matcher) = ContentScriptMatcher::new(
                &[source.as_str()], &[], &[], &[], MatchOptions::default(),
            ) {
                prop_assert_eq!(
                    matcher.matches(&document, true),
                    model::matches(&reference_pattern, &reference_target, false),
                    "content {} vs {}", source, target
                );
            }
            if let Ok(pattern) = HostPattern::parse(&source) {
                prop_assert_eq!(
                    pattern.matches_url(&target),
                    model::matches(&reference_pattern, &reference_target, true),
                    "host {} vs {}", source, target
                );
            }
        }

        #[test]
        fn containment_is_sound(
            (a_scheme, a_host, a_port, a_path) in pattern_parts(),
            (b_scheme, b_host, b_port, b_path) in pattern_parts(),
            targets in prop::collection::vec(target_parts(), 16),
        ) {
            let (Ok(outer), Ok(inner)) = (
                HostPattern::parse(&build_pattern(&a_scheme, &a_host, &a_port, &a_path)),
                HostPattern::parse(&build_pattern(&b_scheme, &b_host, &b_port, &b_path)),
            ) else {
                return Ok(());
            };
            prop_assert!(outer.contains(&outer));
            if outer.contains(&inner) {
                for (scheme, host, port, path) in &targets {
                    if let Some(target) = build_target(scheme, host, *port, path) {
                        prop_assert!(
                            !inner.matches_url(&target) || outer.matches_url(&target),
                            "{:?} contains {:?} but not {}", outer, inner, target
                        );
                    }
                }
            }
        }

        #[test]
        fn arbitrary_inputs_never_panic(source in any::<String>(), glob in any::<String>()) {
            let _ = HostPattern::parse(&source);
            if let Ok(glob) = UrlGlob::parse(&glob) {
                let _ = glob.matches(&source);
            }
        }
    }
}
