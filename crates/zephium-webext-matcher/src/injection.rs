//! Bounded Chrome-style URL match patterns for injected content.
//!
//! Patterns are compiled once at registration time. Matching performs no heap
//! allocation, uses no regular expressions, and is linear in the pattern plus
//! URL length. This module deliberately models the current Chrome content-script
//! schemes (`http`, `https`, and `file`); privileged/internal schemes must never
//! become injectable merely because a caller supplied a broad pattern.

use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use url::{Host, Url};

/// Maximum UTF-8 byte length accepted for one source match pattern.
pub const MAX_MATCH_PATTERN_BYTES: usize = 2 * 1024;
/// Maximum total positive and excluded patterns accepted for one script.
pub const MAX_MATCH_PATTERNS_PER_SET: usize = 128;
/// Maximum number of `*` tokens compiled for one path glob.
pub const MAX_MATCH_PATTERN_WILDCARDS: usize = 64;
/// Maximum URL path-and-query length examined by direct URL matching.
///
/// Zephium's top-level navigation boundary is tighter (8 KiB). This larger
/// bound leaves headroom for engine-reported child-frame URLs while ensuring a
/// hostile URL cannot make every registered matcher scan an unbounded string.
pub const MAX_MATCH_URL_BYTES: usize = 32 * 1024;

/// Why a Chrome-style match pattern was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchPatternError {
    Empty,
    TooLong { length: usize, max: usize },
    ControlCharacter,
    MissingSchemeSeparator,
    UnsupportedScheme,
    MissingHost,
    InvalidHost,
    InvalidHostWildcard,
    InvalidPort,
    MissingPath,
    InvalidPath,
    FragmentNotAllowed,
    TooManyWildcards { count: usize, max: usize },
}

impl fmt::Display for MatchPatternError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("match pattern is empty"),
            Self::TooLong { length, max } => {
                write!(formatter, "match pattern is {length} bytes; limit is {max}")
            }
            Self::ControlCharacter => {
                formatter.write_str("match pattern contains a control character")
            }
            Self::MissingSchemeSeparator => formatter.write_str("match pattern is missing ://"),
            Self::UnsupportedScheme => formatter.write_str("match pattern scheme is unsupported"),
            Self::MissingHost => formatter.write_str("match pattern host is empty"),
            Self::InvalidHost => formatter.write_str("match pattern host is invalid"),
            Self::InvalidHostWildcard => {
                formatter.write_str("host wildcard must be * or a single leading *.")
            }
            Self::InvalidPort => formatter.write_str("match pattern port is invalid"),
            Self::MissingPath => formatter.write_str("match pattern path is missing"),
            Self::InvalidPath => formatter.write_str("match pattern path is invalid"),
            Self::FragmentNotAllowed => {
                formatter.write_str("match pattern paths cannot contain URL fragments")
            }
            Self::TooManyWildcards { count, max } => {
                write!(
                    formatter,
                    "match pattern has {count} wildcards; limit is {max}"
                )
            }
        }
    }
}

impl Error for MatchPatternError {}

/// Which manifest list contained an invalid pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPatternList {
    Matches,
    ExcludeMatches,
}

impl fmt::Display for MatchPatternList {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Matches => formatter.write_str("matches"),
            Self::ExcludeMatches => formatter.write_str("exclude_matches"),
        }
    }
}

/// Why a complete set of positive and excluded patterns was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchSetError {
    MissingMatches,
    TooManyPatterns {
        count: usize,
        max: usize,
    },
    InvalidPattern {
        list: MatchPatternList,
        index: usize,
        error: MatchPatternError,
    },
    FallbackRequiresAllPaths {
        index: usize,
    },
}

impl fmt::Display for MatchSetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingMatches => {
                formatter.write_str("matches must contain at least one pattern")
            }
            Self::TooManyPatterns { count, max } => {
                write!(formatter, "match set has {count} patterns; limit is {max}")
            }
            Self::InvalidPattern { list, index, error } => {
                write!(formatter, "invalid {list}[{index}]: {error}")
            }
            Self::FallbackRequiresAllPaths { index } => write!(
                formatter,
                "matches[{index}] must use /* when match_origin_as_fallback is enabled"
            ),
        }
    }
}

impl Error for MatchSetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidPattern { error, .. } => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PatternScheme {
    Http,
    Https,
    File,
    WebWildcard,
}

impl PatternScheme {
    fn parse(value: &str) -> Result<Self, MatchPatternError> {
        match value {
            "http" => Ok(Self::Http),
            "https" => Ok(Self::Https),
            "file" => Ok(Self::File),
            "*" => Ok(Self::WebWildcard),
            _ => Err(MatchPatternError::UnsupportedScheme),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::File => "file",
            Self::WebWildcard => "*",
        }
    }

    fn matches(self, scheme: &str) -> bool {
        match self {
            Self::Http => scheme == "http",
            Self::Https => scheme == "https",
            Self::File => scheme == "file",
            Self::WebWildcard => matches!(scheme, "http" | "https"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CanonicalHost {
    Domain(Box<str>),
    Ipv4(Ipv4Addr),
    Ipv6(Ipv6Addr),
}

impl CanonicalHost {
    fn parse(value: &str) -> Result<Self, MatchPatternError> {
        if value.is_empty() {
            return Err(MatchPatternError::MissingHost);
        }
        if value.contains(['/', '?', '#', '@']) {
            return Err(MatchPatternError::InvalidHost);
        }

        // `url` supplies the same IDNA/IP canonicalization used for target
        // URLs. Parse through a complete URL so bracketed IPv6 is handled
        // consistently with `Url::host()`.
        let candidate = format!("http://{value}/");
        let parsed = Url::parse(&candidate).map_err(|_| MatchPatternError::InvalidHost)?;
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.port().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.path() != "/"
        {
            return Err(MatchPatternError::InvalidHost);
        }

        match parsed.host() {
            Some(Host::Domain(host)) if !host.is_empty() => {
                Ok(Self::Domain(host.to_owned().into_boxed_str()))
            }
            Some(Host::Ipv4(address)) => Ok(Self::Ipv4(address)),
            Some(Host::Ipv6(address)) => Ok(Self::Ipv6(address)),
            _ => Err(MatchPatternError::InvalidHost),
        }
    }

    fn matches(&self, candidate: Host<&str>) -> bool {
        match (self, candidate) {
            (Self::Domain(expected), Host::Domain(actual)) => expected.as_ref() == actual,
            (Self::Ipv4(expected), Host::Ipv4(actual)) => *expected == actual,
            (Self::Ipv6(expected), Host::Ipv6(actual)) => *expected == actual,
            _ => false,
        }
    }

    fn append_to(&self, target: &mut String) {
        match self {
            Self::Domain(host) => target.push_str(host),
            Self::Ipv4(address) => target.push_str(&address.to_string()),
            Self::Ipv6(address) => {
                target.push('[');
                target.push_str(&address.to_string());
                target.push(']');
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HostPattern {
    Any,
    Exact(CanonicalHost),
    Subdomains(Box<str>),
}

impl HostPattern {
    fn parse(value: &str) -> Result<Self, MatchPatternError> {
        if value == "*" {
            return Ok(Self::Any);
        }
        if let Some(base) = value.strip_prefix("*.") {
            if base.contains('*') || base.is_empty() {
                return Err(MatchPatternError::InvalidHostWildcard);
            }
            return match CanonicalHost::parse(base)? {
                CanonicalHost::Domain(domain) => Ok(Self::Subdomains(domain)),
                CanonicalHost::Ipv4(_) | CanonicalHost::Ipv6(_) => {
                    Err(MatchPatternError::InvalidHostWildcard)
                }
            };
        }
        if value.contains('*') {
            return Err(MatchPatternError::InvalidHostWildcard);
        }
        Ok(Self::Exact(CanonicalHost::parse(value)?))
    }

    fn matches(&self, url: &Url) -> bool {
        match self {
            Self::Any => url.host().is_some(),
            Self::Exact(expected) => url.host().is_some_and(|host| expected.matches(host)),
            Self::Subdomains(expected) => {
                let Some(Host::Domain(actual)) = url.host() else {
                    return false;
                };
                actual == expected.as_ref()
                    || actual
                        .strip_suffix(expected.as_ref())
                        .is_some_and(|prefix| prefix.ends_with('.'))
            }
        }
    }

    fn append_to(&self, target: &mut String) {
        match self {
            Self::Any => target.push('*'),
            Self::Exact(host) => host.append_to(target),
            Self::Subdomains(host) => {
                target.push_str("*.");
                target.push_str(host);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortPattern {
    Any,
    Exact(u16),
}

impl PortPattern {
    fn parse(value: Option<&str>) -> Result<Self, MatchPatternError> {
        match value {
            None | Some("*") => Ok(Self::Any),
            Some("") => Err(MatchPatternError::InvalidPort),
            Some(value) if value.bytes().all(|byte| byte.is_ascii_digit()) => value
                .parse::<u16>()
                .map(Self::Exact)
                .map_err(|_| MatchPatternError::InvalidPort),
            Some(_) => Err(MatchPatternError::InvalidPort),
        }
    }

    fn matches(self, url: &Url) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(expected) => url.port_or_known_default() == Some(expected),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LiteralSegment {
    bytes: Box<[u8]>,
    failure: Box<[u16]>,
}

impl LiteralSegment {
    fn new(bytes: &[u8]) -> Self {
        debug_assert!(!bytes.is_empty());
        debug_assert!(bytes.len() <= MAX_MATCH_PATTERN_BYTES);

        let mut failure = vec![0_u16; bytes.len()];
        let mut prefix_len = 0_usize;
        for index in 1..bytes.len() {
            while prefix_len > 0 && bytes[index] != bytes[prefix_len] {
                prefix_len = usize::from(failure[prefix_len - 1]);
            }
            if bytes[index] == bytes[prefix_len] {
                prefix_len += 1;
            }
            failure[index] = prefix_len as u16;
        }

        Self {
            bytes: bytes.into(),
            failure: failure.into_boxed_slice(),
        }
    }
}

/// A conceptual byte string assembled without allocation.
///
/// URLs use this as `path`, then `?`, then `query`. Tests and origin matching
/// use the single-slice form.
#[derive(Clone, Copy)]
struct ByteInput<'a> {
    first: &'a [u8],
    separator: Option<u8>,
    second: &'a [u8],
}

impl<'a> ByteInput<'a> {
    fn single(value: &'a [u8]) -> Self {
        Self {
            first: value,
            separator: None,
            second: &[],
        }
    }

    fn path_and_query(url: &'a Url) -> Self {
        match url.query() {
            Some(query) => Self {
                first: url.path().as_bytes(),
                separator: Some(b'?'),
                second: query.as_bytes(),
            },
            None => Self::single(url.path().as_bytes()),
        }
    }

    fn len(self) -> usize {
        self.first.len() + usize::from(self.separator.is_some()) + self.second.len()
    }

    fn byte(self, index: usize) -> u8 {
        if index < self.first.len() {
            return self.first[index];
        }
        let index = index - self.first.len();
        match self.separator {
            Some(separator) if index == 0 => separator,
            Some(_) => self.second[index - 1],
            None => self.second[index],
        }
    }

    fn equals(self, expected: &[u8]) -> bool {
        self.len() == expected.len()
            && expected
                .iter()
                .enumerate()
                .all(|(index, byte)| self.byte(index) == *byte)
    }

    fn starts_with(self, expected: &[u8]) -> bool {
        expected.len() <= self.len()
            && expected
                .iter()
                .enumerate()
                .all(|(index, byte)| self.byte(index) == *byte)
    }

    fn ends_with(self, expected: &[u8]) -> bool {
        let length = self.len();
        if expected.len() > length {
            return false;
        }
        let start = length - expected.len();
        expected
            .iter()
            .enumerate()
            .all(|(index, byte)| self.byte(start + index) == *byte)
    }

    fn find(self, segment: &LiteralSegment, start: usize, end: usize) -> Option<usize> {
        if start > end || end > self.len() || segment.bytes.len() > end - start {
            return None;
        }

        let mut matched = 0_usize;
        for index in start..end {
            let byte = self.byte(index);
            while matched > 0 && byte != segment.bytes[matched] {
                matched = usize::from(segment.failure[matched - 1]);
            }
            if byte == segment.bytes[matched] {
                matched += 1;
            }
            if matched == segment.bytes.len() {
                return Some(index + 1 - matched);
            }
        }
        None
    }
}

/// Precompiled `*`-only glob with a worst-case linear matcher.
#[derive(Clone, Debug, PartialEq, Eq)]
struct LinearGlob {
    pattern: Box<str>,
    segments: Box<[LiteralSegment]>,
    starts_with_wildcard: bool,
    ends_with_wildcard: bool,
    has_wildcard: bool,
    chrome_directory_prefix: Option<Box<[u8]>>,
}

impl LinearGlob {
    fn compile(pattern: String) -> Result<Self, MatchPatternError> {
        let bytes = pattern.as_bytes();
        let wildcard_count = bytes.iter().filter(|byte| **byte == b'*').count();
        if wildcard_count > MAX_MATCH_PATTERN_WILDCARDS {
            return Err(MatchPatternError::TooManyWildcards {
                count: wildcard_count,
                max: MAX_MATCH_PATTERN_WILDCARDS,
            });
        }

        let segments = bytes
            .split(|byte| *byte == b'*')
            .filter(|segment| !segment.is_empty())
            .map(LiteralSegment::new)
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let chrome_directory_prefix = pattern
            .strip_suffix("/*")
            .filter(|prefix| !prefix.is_empty())
            .map(|prefix| Box::<[u8]>::from(prefix.as_bytes()));
        let starts_with_wildcard = bytes.first() == Some(&b'*');
        let ends_with_wildcard = bytes.last() == Some(&b'*');

        Ok(Self {
            pattern: pattern.into_boxed_str(),
            segments,
            starts_with_wildcard,
            ends_with_wildcard,
            has_wildcard: wildcard_count != 0,
            chrome_directory_prefix,
        })
    }

    fn matches(&self, input: ByteInput<'_>) -> bool {
        if input.len() > MAX_MATCH_URL_BYTES {
            return false;
        }
        if !self.has_wildcard {
            return input.equals(self.pattern.as_bytes());
        }
        // Chromium treats `/directory/*` as matching `/directory` as well as
        // descendants. Preserve that compatibility edge case explicitly.
        if self
            .chrome_directory_prefix
            .as_deref()
            .is_some_and(|prefix| input.equals(prefix))
        {
            return true;
        }
        if self.segments.is_empty() {
            return true;
        }

        let last_index = self.segments.len() - 1;
        let mut first_unplaced = 0_usize;
        let mut cursor = 0_usize;

        if !self.starts_with_wildcard {
            let first = &self.segments[0];
            if !input.starts_with(&first.bytes) {
                return false;
            }
            cursor = first.bytes.len();
            first_unplaced = 1;
        }

        let suffix_start = if self.ends_with_wildcard {
            input.len()
        } else {
            let last = &self.segments[last_index];
            if !input.ends_with(&last.bytes) {
                return false;
            }
            input.len() - last.bytes.len()
        };

        let middle_end = if self.ends_with_wildcard {
            self.segments.len()
        } else {
            last_index
        };
        for segment in &self.segments[first_unplaced..middle_end] {
            let Some(found) = input.find(segment, cursor, suffix_start) else {
                return false;
            };
            cursor = found + segment.bytes.len();
        }

        if self.ends_with_wildcard {
            true
        } else if first_unplaced > last_index {
            // The single literal was already fixed to the beginning and is
            // also end-anchored.
            cursor == input.len()
        } else {
            cursor <= suffix_start
        }
    }

    fn is_all_paths(&self) -> bool {
        self.pattern.as_ref() == "/*"
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PatternKind {
    AllUrls,
    Standard {
        scheme: PatternScheme,
        host: Option<HostPattern>,
        port: PortPattern,
        path: LinearGlob,
    },
}

/// Canonical scheme semantics for one parsed match pattern.
///
/// This is intentionally not a string. Native adapters can translate a
/// validated pattern without reparsing [`MatchPattern::as_str`] and without
/// accidentally treating `*` as a wildcard for non-web schemes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPatternScheme {
    Http,
    Https,
    File,
    HttpAndHttps,
}

impl MatchPatternScheme {
    pub const fn includes_http_or_https(self) -> bool {
        matches!(self, Self::Http | Self::Https | Self::HttpAndHttps)
    }

    pub const fn includes_file(self) -> bool {
        matches!(self, Self::File)
    }
}

/// Canonical host semantics for one parsed non-file pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPatternHost<'a> {
    Any,
    ExactDomain(&'a str),
    ExactIpv4(Ipv4Addr),
    ExactIpv6(Ipv6Addr),
    /// Matches both the named domain and all of its label-boundary
    /// subdomains, matching Chrome's leading `*.` behavior.
    DomainAndSubdomains(&'a str),
}

/// Canonical port semantics for one parsed non-file pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPatternPort {
    Any,
    Exact(u16),
}

/// Borrowed, canonical path-and-query glob.
///
/// The only metacharacter is `*`; all Unicode, spaces, backslashes, and dot
/// segments have already been normalized by the parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchPatternPath<'a>(&'a str);

impl<'a> MatchPatternPath<'a> {
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

/// Typed, canonical components retained by a parsed match pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPatternComponents<'a> {
    /// Chrome's `<all_urls>` token. Its current generic matcher covers HTTP,
    /// HTTPS, and file; capability-specific callers must still apply their
    /// own scheme grants.
    AllUrls,
    Standard {
        scheme: MatchPatternScheme,
        /// File patterns have no host. Every other scheme has one.
        host: Option<MatchPatternHost<'a>>,
        port: MatchPatternPort,
        path: MatchPatternPath<'a>,
    },
}

impl MatchPatternComponents<'_> {
    pub const fn includes_http_or_https(self) -> bool {
        match self {
            Self::AllUrls => true,
            Self::Standard { scheme, .. } => scheme.includes_http_or_https(),
        }
    }

    pub const fn includes_file(self) -> bool {
        match self {
            Self::AllUrls => true,
            Self::Standard { scheme, .. } => scheme.includes_file(),
        }
    }
}

/// One bounded, precompiled Chrome-style content-script match pattern.
#[derive(Clone, PartialEq, Eq)]
pub struct MatchPattern {
    // Registries and native-dispatch snapshots clone scripts frequently. Keep
    // the compiled KMP tables shared and immutable rather than deep-copying
    // them with every `UserContent` snapshot.
    inner: Arc<MatchPatternInner>,
}

impl fmt::Debug for MatchPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("MatchPattern")
            .field(&self.as_str())
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct MatchPatternInner {
    canonical: Box<str>,
    kind: PatternKind,
}

const RETAINED_HEAP_ALLOCATION_OVERHEAD_BYTES: usize = 2 * std::mem::size_of::<usize>();
const RETAINED_ARC_COUNTER_BYTES: usize = 2 * std::mem::size_of::<usize>();
const MAX_MATCH_PATTERN_LITERAL_SEGMENTS: usize = MAX_MATCH_PATTERN_WILDCARDS + 1;
const MAX_MATCH_PATTERN_HEAP_ALLOCATIONS: usize = 6 + 2 * MAX_MATCH_PATTERN_LITERAL_SEGMENTS;
const MATCH_PATTERN_FIXED_PADDING_BUDGET_BYTES: usize = 128;

/// Conservative upper bound for one valid compiled [`MatchPattern`].
///
/// The bound covers the Arc handle/allocation, canonical string, worst-case
/// domain and path copies, segment slice, every literal/failure-table
/// allocation, Chrome directory-prefix copy, and fixed allocator/alignment
/// padding. It intentionally charges shared Arc state to every owning policy
/// snapshot so admission never depends on incidental sharing.
pub const MAX_MATCH_PATTERN_RETAINED_BUDGET_BYTES: usize = std::mem::size_of::<MatchPattern>()
    + std::mem::size_of::<MatchPatternInner>()
    + RETAINED_ARC_COUNTER_BYTES
    + MAX_MATCH_PATTERN_HEAP_ALLOCATIONS * RETAINED_HEAP_ALLOCATION_OVERHEAD_BYTES
    + MAX_MATCH_PATTERN_LITERAL_SEGMENTS * std::mem::size_of::<LiteralSegment>()
    + 7 * MAX_MATCH_PATTERN_BYTES
    + MATCH_PATTERN_FIXED_PADDING_BUDGET_BYTES;

impl MatchPattern {
    /// Parses and precompiles one match pattern.
    pub fn parse(pattern: &str) -> Result<Self, MatchPatternError> {
        if pattern.is_empty() {
            return Err(MatchPatternError::Empty);
        }
        if pattern.len() > MAX_MATCH_PATTERN_BYTES {
            return Err(MatchPatternError::TooLong {
                length: pattern.len(),
                max: MAX_MATCH_PATTERN_BYTES,
            });
        }
        if pattern.chars().any(char::is_control) {
            return Err(MatchPatternError::ControlCharacter);
        }
        if pattern == "<all_urls>" {
            return Ok(Self::all_urls());
        }

        let (raw_scheme, remainder) = pattern
            .split_once("://")
            .ok_or(MatchPatternError::MissingSchemeSeparator)?;
        let scheme = PatternScheme::parse(raw_scheme)?;
        let path_start = remainder.find('/').ok_or(MatchPatternError::MissingPath)?;
        let authority = &remainder[..path_start];
        let raw_path = &remainder[path_start..];

        let (host, port) = if scheme == PatternScheme::File {
            if !authority.is_empty() {
                return Err(MatchPatternError::InvalidHost);
            }
            (None, PortPattern::Any)
        } else {
            let (raw_host, raw_port) = split_host_and_port(authority)?;
            (
                Some(HostPattern::parse(raw_host)?),
                PortPattern::parse(raw_port)?,
            )
        };

        let canonical_path = canonicalize_path(raw_path)?;
        let path = LinearGlob::compile(canonical_path)?;
        let mut canonical = String::with_capacity(pattern.len());
        canonical.push_str(scheme.as_str());
        canonical.push_str("://");
        if let Some(host) = &host {
            host.append_to(&mut canonical);
            if let PortPattern::Exact(port) = port {
                canonical.push(':');
                canonical.push_str(&port.to_string());
            }
        }
        canonical.push_str(&path.pattern);
        if canonical.len() > MAX_MATCH_PATTERN_BYTES {
            return Err(MatchPatternError::TooLong {
                length: canonical.len(),
                max: MAX_MATCH_PATTERN_BYTES,
            });
        }

        Ok(Self {
            inner: Arc::new(MatchPatternInner {
                canonical: canonical.into_boxed_str(),
                kind: PatternKind::Standard {
                    scheme,
                    host,
                    port,
                    path,
                },
            }),
        })
    }

    /// Returns the canonical form retained by the matcher.
    pub fn as_str(&self) -> &str {
        &self.inner.canonical
    }

    /// Conservative retained-memory charge for this compiled matcher.
    ///
    /// This is the canonical accounting API for every registry or authority
    /// retaining a [`MatchPattern`]. It counts the inline Arc handle and
    /// charges the complete shared allocation to this owner, including parsed
    /// host/path state, literal segments, and `u16` KMP failure tables.
    pub fn retained_budget_bytes(&self) -> usize {
        let mut bytes = std::mem::size_of::<Self>()
            .saturating_add(RETAINED_HEAP_ALLOCATION_OVERHEAD_BYTES)
            .saturating_add(RETAINED_ARC_COUNTER_BYTES)
            .saturating_add(std::mem::size_of::<MatchPatternInner>())
            .saturating_add(retained_boxed_bytes(self.inner.canonical.len()));

        if let PatternKind::Standard { host, path, .. } = &self.inner.kind {
            if let Some(host) = host {
                let domain_bytes = match host {
                    HostPattern::Any => 0,
                    HostPattern::Exact(CanonicalHost::Domain(domain)) => domain.len(),
                    HostPattern::Exact(CanonicalHost::Ipv4(_) | CanonicalHost::Ipv6(_)) => 0,
                    HostPattern::Subdomains(domain) => domain.len(),
                };
                if domain_bytes != 0 {
                    bytes = bytes.saturating_add(retained_boxed_bytes(domain_bytes));
                }
            }
            bytes = bytes
                .saturating_add(retained_boxed_bytes(path.pattern.len()))
                .saturating_add(retained_boxed_bytes(
                    path.segments
                        .len()
                        .saturating_mul(std::mem::size_of::<LiteralSegment>()),
                ));
            for segment in &path.segments {
                bytes = bytes
                    .saturating_add(retained_boxed_bytes(segment.bytes.len()))
                    .saturating_add(retained_boxed_bytes(
                        segment
                            .failure
                            .len()
                            .saturating_mul(std::mem::size_of::<u16>()),
                    ));
            }
            if let Some(prefix) = &path.chrome_directory_prefix {
                bytes = bytes.saturating_add(retained_boxed_bytes(prefix.len()));
            }
        }

        debug_assert!(bytes <= MAX_MATCH_PATTERN_RETAINED_BUDGET_BYTES);
        bytes
    }

    /// Returns the already parsed canonical components.
    ///
    /// Consumers must prefer this over splitting or reparsing [`Self::as_str`].
    /// The returned values borrow this pattern and allocate nothing.
    pub fn components(&self) -> MatchPatternComponents<'_> {
        let PatternKind::Standard {
            scheme,
            host,
            port,
            path,
        } = &self.inner.kind
        else {
            return MatchPatternComponents::AllUrls;
        };

        MatchPatternComponents::Standard {
            scheme: match scheme {
                PatternScheme::Http => MatchPatternScheme::Http,
                PatternScheme::Https => MatchPatternScheme::Https,
                PatternScheme::File => MatchPatternScheme::File,
                PatternScheme::WebWildcard => MatchPatternScheme::HttpAndHttps,
            },
            host: host.as_ref().map(|host| match host {
                HostPattern::Any => MatchPatternHost::Any,
                HostPattern::Exact(CanonicalHost::Domain(domain)) => {
                    MatchPatternHost::ExactDomain(domain)
                }
                HostPattern::Exact(CanonicalHost::Ipv4(address)) => {
                    MatchPatternHost::ExactIpv4(*address)
                }
                HostPattern::Exact(CanonicalHost::Ipv6(address)) => {
                    MatchPatternHost::ExactIpv6(*address)
                }
                HostPattern::Subdomains(domain) => MatchPatternHost::DomainAndSubdomains(domain),
            }),
            port: match port {
                PortPattern::Any => MatchPatternPort::Any,
                PortPattern::Exact(port) => MatchPatternPort::Exact(*port),
            },
            path: MatchPatternPath(&path.pattern),
        }
    }

    /// Returns whether this pattern accepts every path on its matched origins.
    pub fn matches_all_paths(&self) -> bool {
        match &self.inner.kind {
            PatternKind::AllUrls => true,
            PatternKind::Standard { path, .. } => path.is_all_paths(),
        }
    }

    /// Matches a complete URL, excluding its fragment as Chrome does.
    pub fn matches_url(&self, url: &Url) -> bool {
        if ByteInput::path_and_query(url).len() > MAX_MATCH_URL_BYTES {
            return false;
        }
        self.matches_url_unbounded(url)
    }

    fn all_urls() -> Self {
        Self {
            inner: Arc::new(MatchPatternInner {
                canonical: "<all_urls>".into(),
                kind: PatternKind::AllUrls,
            }),
        }
    }

    fn matches_url_unbounded(&self, url: &Url) -> bool {
        match &self.inner.kind {
            PatternKind::AllUrls => is_supported_content_scheme(url.scheme()),
            PatternKind::Standard {
                scheme,
                host,
                port,
                path,
            } => {
                scheme.matches(url.scheme())
                    && host.as_ref().is_none_or(|host| host.matches(url))
                    && port.matches(url)
                    && path.matches(ByteInput::path_and_query(url))
            }
        }
    }

    fn matches_origin(&self, url: &Url) -> bool {
        match &self.inner.kind {
            PatternKind::AllUrls => is_supported_content_scheme(url.scheme()),
            PatternKind::Standard {
                scheme,
                host,
                port,
                path,
            } => {
                scheme.matches(url.scheme())
                    && host.as_ref().is_none_or(|host| host.matches(url))
                    && port.matches(url)
                    && path.matches(ByteInput::single(b"/"))
            }
        }
    }
}

const fn retained_boxed_bytes(payload_bytes: usize) -> usize {
    RETAINED_HEAP_ALLOCATION_OVERHEAD_BYTES.saturating_add(payload_bytes)
}

impl TryFrom<&str> for MatchPattern {
    type Error = MatchPatternError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl std::str::FromStr for MatchPattern {
    type Err = MatchPatternError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for MatchPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.inner.canonical)
    }
}

/// Related-frame matching options from an MV3 content-script declaration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatchOptions {
    pub match_about_blank: bool,
    pub match_origin_as_fallback: bool,
}

/// Native-derived frame relationship data used for related-frame matching.
///
/// Callers must obtain these URLs from the engine's frame/navigation state,
/// never from values supplied by page JavaScript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameMatchContext<'a> {
    pub url: &'a Url,
    /// Closest accessible non-`about:` URL in the parent/opener chain.
    ///
    /// A top-level `about:blank` popup may therefore have this value, while a
    /// browser-opened top-level blank document does not.
    pub parent_or_opener_url: Option<&'a Url>,
    pub initiator_url: Option<&'a Url>,
}

impl<'a> FrameMatchContext<'a> {
    pub const fn new(url: &'a Url) -> Self {
        Self {
            url,
            parent_or_opener_url: None,
            initiator_url: None,
        }
    }
}

/// Positive match patterns, exclusions, and related-frame policy for one script.
///
/// `Default` is intentionally deny-all. Built-in scripts that truly apply to
/// every supported page must opt in with [`MatchSet::all_urls`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MatchSet {
    matches: Box<[MatchPattern]>,
    exclude_matches: Box<[MatchPattern]>,
    options: MatchOptions,
}

impl MatchSet {
    /// Validates an already parsed set. At least one positive pattern is required.
    pub fn new(
        matches: Vec<MatchPattern>,
        exclude_matches: Vec<MatchPattern>,
        options: MatchOptions,
    ) -> Result<Self, MatchSetError> {
        if matches.is_empty() {
            return Err(MatchSetError::MissingMatches);
        }
        let count = matches.len().saturating_add(exclude_matches.len());
        if count > MAX_MATCH_PATTERNS_PER_SET {
            return Err(MatchSetError::TooManyPatterns {
                count,
                max: MAX_MATCH_PATTERNS_PER_SET,
            });
        }
        if options.match_origin_as_fallback {
            if let Some(index) = matches
                .iter()
                .position(|pattern| !pattern.matches_all_paths())
            {
                return Err(MatchSetError::FallbackRequiresAllPaths { index });
            }
        }

        Ok(Self {
            matches: matches.into_boxed_slice(),
            exclude_matches: exclude_matches.into_boxed_slice(),
            options,
        })
    }

    /// Parses both manifest lists while enforcing a single combined count cap.
    pub fn parse<M, MI, E, EI>(
        matches: MI,
        exclude_matches: EI,
        options: MatchOptions,
    ) -> Result<Self, MatchSetError>
    where
        M: AsRef<str>,
        MI: IntoIterator<Item = M>,
        E: AsRef<str>,
        EI: IntoIterator<Item = E>,
    {
        let mut count = 0_usize;
        let matches = parse_pattern_list(matches, MatchPatternList::Matches, &mut count)?;
        let exclude_matches = parse_pattern_list(
            exclude_matches,
            MatchPatternList::ExcludeMatches,
            &mut count,
        )?;
        Self::new(matches, exclude_matches, options)
    }

    /// An explicit set matching every supported content URL.
    pub fn all_urls() -> Self {
        Self {
            matches: vec![MatchPattern::all_urls()].into_boxed_slice(),
            exclude_matches: Box::default(),
            options: MatchOptions::default(),
        }
    }

    pub fn includes(&self) -> &[MatchPattern] {
        &self.matches
    }

    pub fn excludes(&self) -> &[MatchPattern] {
        &self.exclude_matches
    }

    pub const fn options(&self) -> MatchOptions {
        self.options
    }

    /// Returns whether this set is the exact unconditional `<all_urls>` set.
    ///
    /// Native adapters use this as a fail-closed compatibility gate until
    /// they can enforce complete match sets themselves. A broader
    /// all-paths check would be insufficient because scheme/host restrictions
    /// and exclusions still require URL-aware enforcement.
    pub fn is_unconditional_all_urls(&self) -> bool {
        self.matches.len() == 1
            && self.matches[0].as_str() == "<all_urls>"
            && self.exclude_matches.is_empty()
            && self.options == MatchOptions::default()
    }

    /// Conservative retained-memory charge for admission control.
    ///
    /// Each matcher is charged by [`MatchPattern::retained_budget_bytes`], the
    /// single conservative accounting source for its canonical string, Arc
    /// allocation, parsed host/path state, literal segments, and KMP tables.
    /// The boxed collections retain no caller-controlled spare capacity.
    pub fn retained_budget_bytes(&self) -> usize {
        let collection_bytes = usize::from(!self.matches.is_empty())
            .saturating_add(usize::from(!self.exclude_matches.is_empty()))
            .saturating_mul(RETAINED_HEAP_ALLOCATION_OVERHEAD_BYTES);
        self.matches.iter().chain(&self.exclude_matches).fold(
            std::mem::size_of::<Self>().saturating_add(collection_bytes),
            |total, pattern| total.saturating_add(pattern.retained_budget_bytes()),
        )
    }

    /// Exact number of canonical UTF-8 pattern bytes retained by this set.
    ///
    /// This deliberately excludes compiled tables and container overhead;
    /// callers that need conservative admission accounting must use
    /// [`Self::retained_budget_bytes`] as well.
    pub fn canonical_pattern_bytes(&self) -> usize {
        self.matches
            .iter()
            .chain(&self.exclude_matches)
            .map(|pattern| pattern.as_str().len())
            .sum()
    }

    /// Exact number of positive and excluded registrations in this set.
    pub fn pattern_count(&self) -> usize {
        self.matches.len() + self.exclude_matches.len()
    }

    /// Matches a directly addressable document URL.
    pub fn matches_url(&self, url: &Url) -> bool {
        if ByteInput::path_and_query(url).len() > MAX_MATCH_URL_BYTES {
            return false;
        }
        self.matches
            .iter()
            .any(|pattern| pattern.matches_url_unbounded(url))
            && !self
                .exclude_matches
                .iter()
                .any(|pattern| pattern.matches_url_unbounded(url))
    }

    /// Matches a frame, including Chrome's two related-frame fallback modes.
    ///
    /// `match_origin_as_fallback` takes precedence over `match_about_blank`, as
    /// Chrome specifies. Its initiator path is deliberately ignored; the
    /// initiator's origin is what grants access.
    pub fn matches_frame(&self, context: FrameMatchContext<'_>) -> bool {
        if self.matches_url(context.url) {
            return true;
        }

        if self.options.match_origin_as_fallback && is_fallback_scheme(context.url.scheme()) {
            return context
                .initiator_url
                .is_some_and(|initiator| self.matches_origin(initiator));
        }

        self.options.match_about_blank
            && is_about_blank_or_srcdoc(context.url)
            && context
                .parent_or_opener_url
                .is_some_and(|source| self.matches_url(source))
    }

    fn matches_origin(&self, url: &Url) -> bool {
        self.matches
            .iter()
            .any(|pattern| pattern.matches_origin(url))
            && !self
                .exclude_matches
                .iter()
                .any(|pattern| pattern.matches_origin(url))
    }
}

fn parse_pattern_list<S, I>(
    patterns: I,
    list: MatchPatternList,
    total: &mut usize,
) -> Result<Vec<MatchPattern>, MatchSetError>
where
    S: AsRef<str>,
    I: IntoIterator<Item = S>,
{
    let mut parsed = Vec::new();
    for (index, pattern) in patterns.into_iter().enumerate() {
        *total = total.saturating_add(1);
        if *total > MAX_MATCH_PATTERNS_PER_SET {
            return Err(MatchSetError::TooManyPatterns {
                count: *total,
                max: MAX_MATCH_PATTERNS_PER_SET,
            });
        }
        parsed.push(
            MatchPattern::parse(pattern.as_ref())
                .map_err(|error| MatchSetError::InvalidPattern { list, index, error })?,
        );
    }
    Ok(parsed)
}

fn split_host_and_port(authority: &str) -> Result<(&str, Option<&str>), MatchPatternError> {
    if authority.is_empty() {
        return Err(MatchPatternError::MissingHost);
    }
    if authority.starts_with('[') {
        let close = authority.find(']').ok_or(MatchPatternError::InvalidHost)?;
        let host = &authority[..=close];
        let remainder = &authority[close + 1..];
        return if remainder.is_empty() {
            Ok((host, None))
        } else if let Some(port) = remainder.strip_prefix(':') {
            Ok((host, Some(port)))
        } else {
            Err(MatchPatternError::InvalidHost)
        };
    }

    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => Ok((host, Some(port))),
        Some(_) => Err(MatchPatternError::InvalidHost),
        None => Ok((authority, None)),
    }
}

fn canonicalize_path(path: &str) -> Result<String, MatchPatternError> {
    if !path.starts_with('/') {
        return Err(MatchPatternError::MissingPath);
    }
    if path.contains('#') {
        return Err(MatchPatternError::FragmentNotAllowed);
    }

    // Canonicalize Unicode, spaces, backslashes, and dot segments exactly once
    // so the compiled pattern is compared with `Url`'s serialized path form.
    let candidate = format!("https://match.invalid{path}");
    let parsed = Url::parse(&candidate).map_err(|_| MatchPatternError::InvalidPath)?;
    if parsed.fragment().is_some() {
        return Err(MatchPatternError::FragmentNotAllowed);
    }
    let mut canonical = parsed.path().to_owned();
    if let Some(query) = parsed.query() {
        canonical.push('?');
        canonical.push_str(query);
    }
    if canonical.len() > MAX_MATCH_PATTERN_BYTES {
        return Err(MatchPatternError::TooLong {
            length: canonical.len(),
            max: MAX_MATCH_PATTERN_BYTES,
        });
    }
    Ok(canonical)
}

fn is_supported_content_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "file")
}

fn is_fallback_scheme(scheme: &str) -> bool {
    matches!(scheme, "about" | "data" | "blob" | "filesystem")
}

fn is_about_blank_or_srcdoc(url: &Url) -> bool {
    url.scheme() == "about" && matches!(url.path(), "blank" | "srcdoc")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("test URL must be valid")
    }

    fn pattern(value: &str) -> MatchPattern {
        MatchPattern::parse(value).expect("test pattern must be valid")
    }

    fn set(matches: &[&str], excludes: &[&str], options: MatchOptions) -> MatchSet {
        MatchSet::parse(matches.iter().copied(), excludes.iter().copied(), options)
            .expect("test match set must be valid")
    }

    #[test]
    fn chrome_spec_examples_match() {
        let cases = [
            (
                "http://*/*",
                vec!["http://google.com", "http://127.0.0.1/search"],
                vec!["https://google.com", "file:///tmp/file"],
            ),
            (
                "https://*/foo*",
                vec![
                    "https://example.com/foo/bar.html",
                    "https://www.google.com/foo",
                ],
                vec!["https://example.com/bar", "http://example.com/foo"],
            ),
            (
                "https://*.google.com/foo*bar",
                vec![
                    "https://google.com/foobar",
                    "https://www.google.com/foo/baz/bar",
                    "https://docs.google.com/foobar",
                ],
                vec!["https://notgoogle.com/foobar", "https://google.com/foobaz"],
            ),
            (
                "file:///foo*",
                vec!["file:///foo", "file:///foo/bar.html"],
                vec!["file:///bar/foo"],
            ),
            (
                "http://127.0.0.1/*",
                vec!["http://127.0.0.1/", "http://127.0.0.1:8123/a"],
                vec!["http://127.0.0.2/", "https://127.0.0.1/"],
            ),
            (
                "*://mail.google.com/*",
                vec!["http://mail.google.com/", "https://mail.google.com/inbox"],
                vec!["file://mail.google.com/inbox", "ftp://mail.google.com/a"],
            ),
        ];

        for (source, matches, misses) in cases {
            let parsed = pattern(source);
            for candidate in matches {
                assert!(
                    parsed.matches_url(&url(candidate)),
                    "{source} != {candidate}"
                );
            }
            for candidate in misses {
                assert!(
                    !parsed.matches_url(&url(candidate)),
                    "{source} == {candidate}"
                );
            }
        }
    }

    #[test]
    fn all_urls_is_broad_only_across_supported_content_schemes() {
        let all = pattern("<all_urls>");
        assert!(all.matches_url(&url("http://example.com/")));
        assert!(all.matches_url(&url("https://example.com/")));
        assert!(all.matches_url(&url("file:///tmp/a")));
        assert!(!all.matches_url(&url("about:blank")));
        assert!(!all.matches_url(&url("data:text/plain,hello")));
        assert!(!all.matches_url(&url("zephium://settings/")));
    }

    #[test]
    fn wildcard_subdomains_include_the_root_but_respect_label_boundaries() {
        let google = pattern("https://*.google.com/*");
        assert!(google.matches_url(&url("https://google.com/")));
        assert!(google.matches_url(&url("https://maps.google.com/")));
        assert!(!google.matches_url(&url("https://notgoogle.com/")));
        assert!(!google.matches_url(&url("https://google.com.example/")));
    }

    #[test]
    fn hosts_are_canonicalized_and_ipv6_is_supported() {
        let international = pattern("https://BÜCHER.example/*");
        assert_eq!(international.as_str(), "https://xn--bcher-kva.example/*");
        assert!(international.matches_url(&url("https://bücher.example/a")));

        let ipv6 = pattern("http://[::1]:8080/*");
        assert!(ipv6.matches_url(&url("http://[::1]:8080/a")));
        assert!(!ipv6.matches_url(&url("http://[::1]:8081/a")));
    }

    #[test]
    fn typed_components_expose_canonical_semantics_without_reparsing() {
        let international = pattern("*://*.BÜCHER.example:8443/a/../🐱*?q=*");
        assert_eq!(
            international.components(),
            MatchPatternComponents::Standard {
                scheme: MatchPatternScheme::HttpAndHttps,
                host: Some(MatchPatternHost::DomainAndSubdomains(
                    "xn--bcher-kva.example"
                )),
                port: MatchPatternPort::Exact(8443),
                path: MatchPatternPath("/%F0%9F%90%B1*?q=*"),
            }
        );

        let file = pattern("file:///tmp/*");
        assert_eq!(
            file.components(),
            MatchPatternComponents::Standard {
                scheme: MatchPatternScheme::File,
                host: None,
                port: MatchPatternPort::Any,
                path: MatchPatternPath("/tmp/*"),
            }
        );
        assert_eq!(
            pattern("<all_urls>").components(),
            MatchPatternComponents::AllUrls
        );
    }

    #[test]
    fn omitted_and_wildcard_ports_match_every_port() {
        let omitted = pattern("https://example.com/*");
        let wildcard = pattern("https://example.com:*/*");
        for candidate in ["https://example.com/", "https://example.com:8443/"] {
            assert!(omitted.matches_url(&url(candidate)));
            assert!(wildcard.matches_url(&url(candidate)));
        }

        let exact = pattern("http://example.com:80/*");
        assert!(exact.matches_url(&url("http://example.com/")));
        assert!(exact.matches_url(&url("http://example.com:80/a")));
        assert!(!exact.matches_url(&url("http://example.com:8080/a")));
    }

    #[test]
    fn path_matching_includes_query_and_ignores_fragment() {
        let with_query = pattern("https://example.com/search?q=*");
        assert!(with_query.matches_url(&url("https://example.com/search?q=rust")));
        assert!(!with_query.matches_url(&url("https://example.com/search")));

        let fragment_agnostic = pattern("https://example.com/page");
        assert!(fragment_agnostic.matches_url(&url("https://example.com/page#section")));
        let large_fragment = url(&format!(
            "https://example.com/page#{}",
            "x".repeat(MAX_MATCH_URL_BYTES + 1)
        ));
        assert!(fragment_agnostic.matches_url(&large_fragment));
    }

    #[test]
    fn directory_wildcard_matches_directory_without_trailing_slash() {
        let directory = pattern("https://example.com/example/*");
        assert!(directory.matches_url(&url("https://example.com/example")));
        assert!(directory.matches_url(&url("https://example.com/example/child")));
        assert!(!directory.matches_url(&url("https://example.com/examples")));
    }

    #[test]
    fn unicode_and_dot_segments_are_canonicalized_once() {
        let parsed = pattern("https://example.com/a/../🐱*");
        assert_eq!(parsed.as_str(), "https://example.com/%F0%9F%90%B1*");
        assert!(parsed.matches_url(&url("https://example.com/🐱-photo")));
    }

    #[test]
    fn invalid_patterns_are_rejected() {
        let cases = [
            "",
            "http://*",
            "http:/example.com/*",
            "ws://example.com/*",
            "HTTP://example.com/*",
            "https:///foo",
            "https://*foo.example/*",
            "https://foo.*.example/*",
            "https://example.com:notaport/*",
            "https://example.com:65536/*",
            "https://user@example.com/*",
            "file://localhost/foo",
            "https://example.com/path#fragment",
            "https://example.com/\n",
            "https://*.127.0.0.1/*",
        ];
        for candidate in cases {
            assert!(
                MatchPattern::parse(candidate).is_err(),
                "accepted {candidate:?}"
            );
        }
    }

    #[test]
    fn pattern_length_wildcard_and_url_bounds_are_enforced() {
        let oversized = format!(
            "https://example.com/{}",
            "a".repeat(MAX_MATCH_PATTERN_BYTES)
        );
        assert!(matches!(
            MatchPattern::parse(&oversized),
            Err(MatchPatternError::TooLong { .. })
        ));

        let too_many_wildcards = format!(
            "https://example.com/{}",
            "*a".repeat(MAX_MATCH_PATTERN_WILDCARDS + 1)
        );
        assert!(matches!(
            MatchPattern::parse(&too_many_wildcards),
            Err(MatchPatternError::TooManyWildcards { .. })
        ));

        let oversized_url = url(&format!(
            "https://example.com/{}",
            "a".repeat(MAX_MATCH_URL_BYTES)
        ));
        assert!(!pattern("<all_urls>").matches_url(&oversized_url));
    }

    #[test]
    fn worst_valid_compiled_pattern_stays_inside_the_conservative_bound() {
        let prefix = "https://example.com/";
        let wildcard_skeleton = format!("a{}", "*a".repeat(MAX_MATCH_PATTERN_WILDCARDS));
        let fill = MAX_MATCH_PATTERN_BYTES - prefix.len() - wildcard_skeleton.len();
        let source = format!("{prefix}{wildcard_skeleton}{}", "b".repeat(fill));
        assert_eq!(source.len(), MAX_MATCH_PATTERN_BYTES);

        let compiled = pattern(&source);
        assert_eq!(compiled.as_str().len(), MAX_MATCH_PATTERN_BYTES);
        assert!(compiled.retained_budget_bytes() <= MAX_MATCH_PATTERN_RETAINED_BUDGET_BYTES);
    }

    #[test]
    fn match_set_discards_caller_spare_capacity_before_accounting() {
        let include = pattern("https://example.com/*");
        let exclude = pattern("https://example.com/private/*");
        let mut includes_with_spare = Vec::with_capacity(4_096);
        includes_with_spare.push(include.clone());
        let mut excludes_with_spare = Vec::with_capacity(4_096);
        excludes_with_spare.push(exclude.clone());

        let with_spare = MatchSet::new(
            includes_with_spare,
            excludes_with_spare,
            MatchOptions::default(),
        )
        .unwrap();
        let compact = MatchSet::new(
            vec![include.clone()],
            vec![exclude.clone()],
            MatchOptions::default(),
        )
        .unwrap();
        let expected = std::mem::size_of::<MatchSet>()
            + 2 * RETAINED_HEAP_ALLOCATION_OVERHEAD_BYTES
            + include.retained_budget_bytes()
            + exclude.retained_budget_bytes();

        assert_eq!(
            with_spare.retained_budget_bytes(),
            compact.retained_budget_bytes()
        );
        assert_eq!(with_spare.retained_budget_bytes(), expected);
        assert_eq!(with_spare.includes().len(), 1);
        assert_eq!(with_spare.excludes().len(), 1);
    }

    #[test]
    fn match_set_applies_exclusions_after_positive_matches() {
        let patterns = set(
            &["https://*.example.com/*"],
            &[
                "https://private.example.com/*",
                "https://example.com/admin*",
            ],
            MatchOptions::default(),
        );
        assert!(patterns.matches_url(&url("https://www.example.com/page")));
        assert!(!patterns.matches_url(&url("https://private.example.com/page")));
        assert!(!patterns.matches_url(&url("https://example.com/admin/users")));
        assert!(!patterns.matches_url(&url("https://other.test/page")));
    }

    #[test]
    fn match_set_default_is_fail_closed_and_all_urls_is_explicit() {
        let target = url("https://example.com/");
        assert!(!MatchSet::default().matches_url(&target));
        assert!(MatchSet::all_urls().matches_url(&target));
    }

    #[test]
    fn unconditional_all_urls_is_exact_and_exclusion_free() {
        assert!(MatchSet::all_urls().is_unconditional_all_urls());
        assert!(!set(&["https://*/*"], &[], MatchOptions::default()).is_unconditional_all_urls());
        assert!(!set(
            &["<all_urls>"],
            &["https://private.example/*"],
            MatchOptions::default(),
        )
        .is_unconditional_all_urls());
        assert!(!set(
            &["<all_urls>"],
            &[],
            MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: false,
            },
        )
        .is_unconditional_all_urls());
    }

    #[test]
    fn match_set_requires_a_positive_pattern_and_caps_combined_count() {
        assert_eq!(
            MatchSet::new(Vec::new(), Vec::new(), MatchOptions::default()),
            Err(MatchSetError::MissingMatches)
        );

        let all = pattern("<all_urls>");
        let at_limit = vec![all.clone(); MAX_MATCH_PATTERNS_PER_SET];
        assert!(MatchSet::new(at_limit, Vec::new(), MatchOptions::default()).is_ok());
        let over_limit = vec![all; MAX_MATCH_PATTERNS_PER_SET + 1];
        assert!(matches!(
            MatchSet::new(over_limit, Vec::new(), MatchOptions::default()),
            Err(MatchSetError::TooManyPatterns { .. })
        ));
    }

    #[test]
    fn parse_errors_identify_the_manifest_list_and_index() {
        let result = MatchSet::parse(
            ["https://example.com/*"],
            ["https://safe.example/*", "not a pattern"],
            MatchOptions::default(),
        );
        assert!(matches!(
            result,
            Err(MatchSetError::InvalidPattern {
                list: MatchPatternList::ExcludeMatches,
                index: 1,
                ..
            })
        ));
    }

    #[test]
    fn about_blank_uses_native_parent_or_opener_context() {
        let patterns = set(
            &["https://example.com/*"],
            &[],
            MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: false,
            },
        );
        let blank = url("about:blank");
        let srcdoc = url("about:srcdoc");
        let matching_parent = url("https://example.com/page");
        let other_parent = url("https://other.test/page");

        assert!(patterns.matches_frame(FrameMatchContext {
            url: &blank,
            parent_or_opener_url: Some(&matching_parent),
            initiator_url: None,
        }));
        assert!(patterns.matches_frame(FrameMatchContext {
            url: &srcdoc,
            parent_or_opener_url: Some(&matching_parent),
            initiator_url: None,
        }));
        assert!(!patterns.matches_frame(FrameMatchContext {
            url: &blank,
            parent_or_opener_url: Some(&other_parent),
            initiator_url: None,
        }));
        assert!(!patterns.matches_frame(FrameMatchContext::new(&blank)));
        assert!(!patterns.matches_frame(FrameMatchContext::new(&url("about:version"))));
    }

    #[test]
    fn origin_fallback_uses_only_initiator_origin_for_related_schemes() {
        let patterns = set(
            &["https://example.com/*"],
            &[],
            MatchOptions {
                match_about_blank: false,
                match_origin_as_fallback: true,
            },
        );
        let initiator = url("https://example.com/a/path/that/is/ignored");
        for related in [
            "about:blank",
            "data:text/html,hello",
            "blob:https://example.com/id",
            "filesystem:https://example.com/temporary/file",
        ] {
            let target = url(related);
            assert!(patterns.matches_frame(FrameMatchContext {
                url: &target,
                parent_or_opener_url: None,
                initiator_url: Some(&initiator),
            }));
        }

        let unrelated = url("mailto:user@example.com");
        assert!(!patterns.matches_frame(FrameMatchContext {
            url: &unrelated,
            parent_or_opener_url: None,
            initiator_url: Some(&initiator),
        }));
    }

    #[test]
    fn origin_fallback_requires_all_paths_and_honors_exclusions() {
        let options = MatchOptions {
            match_about_blank: false,
            match_origin_as_fallback: true,
        };
        assert_eq!(
            MatchSet::new(
                vec![pattern("https://example.com/account/*")],
                Vec::new(),
                options,
            ),
            Err(MatchSetError::FallbackRequiresAllPaths { index: 0 })
        );

        let excluded = set(
            &["https://*.example.com/*"],
            &["https://private.example.com/*"],
            options,
        );
        let target = url("data:text/html,hello");
        let denied_initiator = url("https://private.example.com/path");
        assert!(!excluded.matches_frame(FrameMatchContext {
            url: &target,
            parent_or_opener_url: None,
            initiator_url: Some(&denied_initiator),
        }));
    }

    #[test]
    fn origin_fallback_takes_priority_over_about_blank() {
        let patterns = set(
            &["https://example.com/*"],
            &[],
            MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: true,
            },
        );
        let blank = url("about:blank");
        let matching_parent = url("https://example.com/page");
        let nonmatching_initiator = url("https://other.test/page");
        assert!(!patterns.matches_frame(FrameMatchContext {
            url: &blank,
            parent_or_opener_url: Some(&matching_parent),
            initiator_url: Some(&nonmatching_initiator),
        }));
    }

    fn reference_glob(pattern: &[u8], input: &[u8]) -> bool {
        let mut previous = vec![false; input.len() + 1];
        previous[0] = true;
        for token in pattern {
            let mut next = vec![false; input.len() + 1];
            if *token == b'*' {
                next[0] = previous[0];
                for index in 1..=input.len() {
                    next[index] = previous[index] || next[index - 1];
                }
            } else {
                for index in 1..=input.len() {
                    next[index] = previous[index - 1] && input[index - 1] == *token;
                }
            }
            previous = next;
        }
        previous[input.len()]
    }

    proptest! {
        #[test]
        fn linear_glob_agrees_with_reference_matcher(
            source in "[a-c*]{0,64}",
            target in "[a-c]{0,64}",
        ) {
            let compiled = LinearGlob::compile(source.clone()).expect("generated glob is bounded");
            let actual = compiled.matches(ByteInput::single(target.as_bytes()));
            prop_assert_eq!(actual, reference_glob(source.as_bytes(), target.as_bytes()));
        }

        #[test]
        fn arbitrary_pattern_parse_and_match_never_panics(source in any::<String>()) {
            if let Ok(parsed) = MatchPattern::parse(&source) {
                prop_assert!(source.len() <= MAX_MATCH_PATTERN_BYTES);
                prop_assert!(parsed.as_str().len() <= MAX_MATCH_PATTERN_BYTES);
                let target = url("https://example.com/path?query=value#fragment");
                let _ = parsed.matches_url(&target);
            }
        }

        #[test]
        fn arbitrary_match_sets_never_panic_and_never_exceed_count_cap(
            positive in prop::collection::vec(any::<String>(), 0..150),
            excluded in prop::collection::vec(any::<String>(), 0..150),
            about_blank in any::<bool>(),
            origin_fallback in any::<bool>(),
        ) {
            let result = MatchSet::parse(
                positive.iter(),
                excluded.iter(),
                MatchOptions {
                    match_about_blank: about_blank,
                    match_origin_as_fallback: origin_fallback,
                },
            );
            if let Ok(parsed) = result {
                prop_assert!(!parsed.includes().is_empty());
                prop_assert!(
                    parsed.includes().len() + parsed.excludes().len()
                        <= MAX_MATCH_PATTERNS_PER_SET
                );
                let target = url("about:blank");
                let parent = url("https://example.com/");
                let _ = parsed.matches_frame(FrameMatchContext {
                    url: &target,
                    parent_or_opener_url: Some(&parent),
                    initiator_url: Some(&parent),
                });
            }
        }
    }
}
