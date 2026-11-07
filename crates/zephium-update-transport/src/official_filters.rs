//! Bounded requests to the official filter list publisher endpoints.
//!
//! HTTPS authenticates the server and transport. It does not provide a
//! publisher signature or signed freshness. Callers validate and stage sources
//! before native compilation and durable activation.

use futures_util::TryStreamExt;
use reqwest::header::{
    ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED, RETRY_AFTER,
};
use reqwest::{Client, StatusCode};
use std::time::Duration;

/// The entire allowlist of remotely maintained subscription sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficialFilter {
    /// Advertising filters published by EasyList.
    EasyList,
    /// Tracking filters published by EasyList.
    EasyPrivacy,
    /// uBlock Origin's own filters.
    UblockFilters,
    /// uBlock Origin's quick fixes, updated several times a day.
    UblockQuickFixes,
    /// uBlock Origin's filters for link shorteners.
    UblockLinkShorteners,
    /// uBlock Origin's privacy filters.
    UblockPrivacy,
    /// uBlock Origin's filters against CPU and bandwidth abuse.
    UblockResourceAbuse,
    /// uBlock Origin's exceptions for pages the other lists break.
    UblockUnbreak,
    /// uBlock Origin's list of scam and malware sites.
    UblockBadware,
}
impl OfficialFilter {
    /// Every source, in catalog order.
    pub const ALL: [Self; 9] = [
        Self::EasyList,
        Self::EasyPrivacy,
        Self::UblockFilters,
        Self::UblockQuickFixes,
        Self::UblockLinkShorteners,
        Self::UblockPrivacy,
        Self::UblockResourceAbuse,
        Self::UblockUnbreak,
        Self::UblockBadware,
    ];

    /// Fixed canonical HTTPS URL. No caller-controlled URL is accepted.
    pub const fn url(self) -> &'static str {
        match self {
            Self::EasyList => "https://easylist.to/easylist/easylist.txt",
            Self::EasyPrivacy => "https://easylist.to/easylist/easyprivacy.txt",
            // uBlock Origin's own build of filters.txt with its includes:
            // the general filters and one file per year.
            Self::UblockFilters => "https://ublockorigin.github.io/uAssets/filters/filters.min.txt",
            Self::UblockQuickFixes => {
                "https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt"
            }
            Self::UblockLinkShorteners => {
                "https://ublockorigin.github.io/uAssets/filters/ubo-link-shorteners.txt"
            }
            Self::UblockPrivacy => "https://ublockorigin.github.io/uAssets/filters/privacy.min.txt",
            Self::UblockResourceAbuse => {
                "https://ublockorigin.github.io/uAssets/filters/resource-abuse.txt"
            }
            Self::UblockUnbreak => "https://ublockorigin.github.io/uAssets/filters/unbreak.min.txt",
            Self::UblockBadware => "https://ublockorigin.github.io/uAssets/filters/badware.min.txt",
        }
    }

    /// Stable source identifier, as compiled.
    pub const fn id(self) -> &'static str {
        match self {
            Self::EasyList => "easylist",
            Self::EasyPrivacy => "easyprivacy",
            Self::UblockFilters => "ublock-filters",
            Self::UblockQuickFixes => "ublock-quick-fixes",
            Self::UblockLinkShorteners => "ublock-link-shorteners",
            Self::UblockPrivacy => "ublock-privacy",
            Self::UblockResourceAbuse => "ublock-resource-abuse",
            Self::UblockUnbreak => "ublock-unbreak",
            Self::UblockBadware => "ublock-badware",
        }
    }
}

/// Bounded publisher cache validators, never browsing or profile identifiers.
#[derive(Clone, Debug, Default)]
pub struct FilterValidators {
    /// Last accepted entity tag, used only at the same fixed URL.
    pub etag: Option<String>,
    /// Last accepted modification date, used if no entity tag is available.
    pub last_modified: Option<String>,
}

/// One bounded, untrusted publisher response.
#[derive(Debug)]
pub enum FilterResponse {
    /// Exact cached source is unchanged; the caller must still verify it.
    NotModified,
    /// New bounded source bytes and optional validators.
    Modified {
        /// UTF-8 is not assumed at this transport boundary.
        bytes: Vec<u8>,
        /// Cache hints associated with precisely these bytes.
        validators: FilterValidators,
    },
}

/// Stable transport failure without server-supplied text or response bodies.
#[derive(Clone, Copy, Debug)]
pub struct FilterFetchFailure {
    /// Optional bounded Retry-After delay, in seconds.
    pub retry_after_seconds: Option<u64>,
}
impl std::fmt::Display for FilterFetchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("official filter request failed")
    }
}
impl std::error::Error for FilterFetchFailure {}
const FAILURE: FilterFetchFailure = FilterFetchFailure {
    retry_after_seconds: None,
};
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Reusable client without cookies, authorization, redirects or a referrer.
#[derive(Clone)]
pub struct OfficialFilterClient(Client);
impl OfficialFilterClient {
    /// Creates a verified-TLS, system-proxy client with a finite request deadline.
    pub fn new() -> Result<Self, FilterFetchFailure> {
        Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Zephium-Filter-Updates/1")
            .build()
            .map(Self)
            .map_err(|_| FAILURE)
    }

    /// Fetches only the selected official source, with a streamed size limit.
    /// A 304 is never treated as source material by this API.
    pub async fn fetch(
        &self,
        source: OfficialFilter,
        cached: Option<&FilterValidators>,
    ) -> Result<FilterResponse, FilterFetchFailure> {
        let mut request = self
            .0
            .get(source.url())
            .header(ACCEPT, "text/plain")
            .header(ACCEPT_ENCODING, "identity");
        if let Some(cached) = cached {
            if let Some(etag) = cached.etag.as_deref().filter(|v| valid_header(v)) {
                request = request.header(IF_NONE_MATCH, etag);
            } else if let Some(modified) =
                cached.last_modified.as_deref().filter(|v| valid_header(v))
            {
                request = request.header(IF_MODIFIED_SINCE, modified);
            }
        }
        let response = request.send().await.map_err(|_| FAILURE)?;
        if response.url().as_str() != source.url() {
            return Err(FAILURE);
        }
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(FilterResponse::NotModified);
        }
        if response.status() != StatusCode::OK {
            return Err(FilterFetchFailure {
                retry_after_seconds: response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| retry_delay(s, std::time::SystemTime::now())),
            });
        }
        if response
            .headers()
            .get(CONTENT_ENCODING)
            .is_some_and(|value| value.as_bytes() != b"identity")
            || response
                .content_length()
                .is_some_and(|n| n > MAX_BYTES as u64)
        {
            return Err(FAILURE);
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .filter(|s| valid_header(s))
                .map(str::to_owned)
        };
        let validators = FilterValidators {
            etag: header(ETAG),
            last_modified: header(LAST_MODIFIED),
        };
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.try_next().await.map_err(|_| FAILURE)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(FAILURE);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(FAILURE);
        }
        Ok(FilterResponse::Modified { bytes, validators })
    }
}
/// A list's `Last modified` header date as a 12-digit UTC version
/// (`YYYYMMDDHHMM`), for lists that publish no version number.
pub fn version_from_date(value: &str) -> Option<String> {
    let date =
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822).ok()?;
    let date = date.to_offset(time::UtcOffset::UTC);
    (1970..=9999).contains(&date.year()).then(|| {
        format!(
            "{:04}{:02}{:02}{:02}{:02}",
            date.year(),
            u8::from(date.month()),
            date.day(),
            date.hour(),
            date.minute()
        )
    })
}

fn valid_header(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

fn retry_delay(value: &str, now: std::time::SystemTime) -> Option<u64> {
    if !valid_header(value) {
        return None;
    }
    let seconds = value.parse::<u64>().ok().or_else(|| {
        let date =
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc2822)
                .ok()?;
        let now = now.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        Some(
            u64::try_from(date.unix_timestamp())
                .ok()?
                .saturating_sub(now),
        )
    })?;
    Some(seconds.clamp(60, 30 * 24 * 60 * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conditional_headers_are_bounded_and_cannot_inject_requests() {
        assert!(valid_header("W/\"abc\""));
        assert!(!valid_header("abc\r\nX-Test: value"));
        assert!(!valid_header(&"a".repeat(513)));
        assert_eq!(retry_delay("3600", std::time::UNIX_EPOCH), Some(3600));
        assert_eq!(
            retry_delay("Thu, 01 Jan 1970 01:00:00 GMT", std::time::UNIX_EPOCH),
            Some(3600)
        );
        assert_eq!(retry_delay("invalid", std::time::UNIX_EPOCH), None);
    }

    #[test]
    fn header_dates_become_utc_versions() {
        assert_eq!(
            version_from_date("Mon, 02 Nov 2026 12:27:09 +0000").as_deref(),
            Some("202611021227")
        );
        assert_eq!(
            version_from_date("Mon, 02 Nov 2026 23:30:00 -0100").as_deref(),
            Some("202611030030")
        );
        assert_eq!(version_from_date("yesterday"), None);
    }
}
