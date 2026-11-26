pub use crate::locator::{Locator, LocatorText, TextPosition, TextQuote};
use thiserror::Error;
use url::{form_urlencoded, Position, Url};

pub const MAX_URL_BYTES: usize = 2048;
pub const MAX_LOCATOR_BYTES: usize = 256;

const TRACKING_NAMES: &[&str] = &[
    "srsltid", "gclid", "dclid", "fbclid", "msclkid", "ref_src", "mc_cid", "mc_eid",
];
const TRACKING_PREFIXES: &[&str] = &["utm_"];

#[derive(Debug, PartialEq, Eq, Error)]
pub enum KeyError {
    #[error("URL exceeds byte limit")]
    UrlLength,
    #[error("a credential-free HTTP(S) URL is required")]
    Url,
    #[error("invalid or oversized locator")]
    Locator,
    #[error("entry identity collision")]
    Identity,
}

pub(crate) fn check_url(url: &Url, limit: usize) -> Result<(), KeyError> {
    if url.as_str().len() > limit {
        return Err(KeyError::UrlLength);
    }
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url
            .host_str()
            .is_none_or(|h| h.trim_end_matches('.').is_empty())
    {
        return Err(KeyError::Url);
    }
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(try_from = "String")]
pub struct PageKey(Url);
impl TryFrom<String> for PageKey {
    type Error = KeyError;
    fn try_from(raw: String) -> Result<Self, KeyError> {
        if raw.len() > MAX_URL_BYTES {
            return Err(KeyError::UrlLength);
        }
        if raw.chars().any(char::is_control) {
            return Err(KeyError::Url);
        }
        let page = Url::parse(&raw).map_err(|_| KeyError::Url)?;
        check_url(&page, MAX_URL_BYTES)?;
        let host = page.host_str().ok_or(KeyError::Url)?;
        let authority = raw
            .split_once("://")
            .map(|(_, s)| s.split(['/', '?', '#']).next().unwrap_or_default());
        if !authority.is_some_and(|a| {
            a.strip_prefix(host)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
        }) {
            return Err(KeyError::Url);
        }
        Ok(Self(page))
    }
}

fn keep_pair(pair: &str) -> bool {
    let Some((key, _)) = form_urlencoded::parse(pair.as_bytes()).next() else {
        return true;
    };
    let key = key.to_ascii_lowercase();
    !TRACKING_NAMES.contains(&key.as_str()) && !TRACKING_PREFIXES.iter().any(|p| key.starts_with(p))
}

pub(crate) fn route(s: &str) -> bool {
    s.starts_with(['/', '!'])
}

impl PageKey {
    pub fn from_url(url: &Url) -> Result<Self, KeyError> {
        check_url(url, MAX_URL_BYTES)?;
        let mut page = url.clone();
        if let Some(host) = url.host_str().filter(|h| h.ends_with('.')) {
            page.set_host(Some(host.trim_end_matches('.')))
                .map_err(|_| KeyError::Url)?;
        }
        if !page.fragment().is_some_and(route) {
            page.set_fragment(None);
        }
        if let Some(query) = url.query() {
            let kept = query
                .split('&')
                .filter(|pair| keep_pair(pair))
                .collect::<Vec<_>>()
                .join("&");
            page.set_query((!kept.is_empty()).then_some(&kept));
        }
        Ok(Self(page))
    }

    pub fn as_url(&self) -> &Url {
        &self.0
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(try_from = "String")]
pub struct SiteKey(String);
impl TryFrom<String> for SiteKey {
    type Error = KeyError;
    fn try_from(raw: String) -> Result<Self, KeyError> {
        if raw.len() > MAX_URL_BYTES {
            return Err(KeyError::UrlLength);
        }
        let url = Url::parse(&format!("https://{raw}")).map_err(|_| KeyError::Url)?;
        if !raw.trim_end_matches('.').is_empty() && url.domain() == Some(raw.as_str()) {
            Ok(Self(raw))
        } else {
            Err(KeyError::Url)
        }
    }
}

impl SiteKey {
    pub fn from_url(url: &Url) -> Option<Self> {
        check_url(url, MAX_URL_BYTES).ok()?;
        let host = url.domain()?.trim_end_matches('.');
        psl::domain_str(host).map(|domain| Self(domain.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An ephemeral comparison key whose rules may evolve independently of stored page keys.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageMatch(String);

impl PageMatch {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl PageKey {
    pub fn page_match(&self) -> PageMatch {
        let url = self.as_url();
        let authority = &url[Position::BeforeHost..Position::AfterPort];
        let mut value = authority
            .strip_prefix("www.")
            .unwrap_or(authority)
            .to_owned();
        let path = url.path().trim_end_matches('/');
        value.push_str(&normalize_percent(if path.is_empty() { "/" } else { path }));
        if let Some(query) = url.query() {
            let mut pairs: Vec<_> = query
                .split('&')
                .filter(|s| keep_pair(s))
                .map(normalize_percent)
                .collect();
            pairs.sort_by(|a, b| {
                let key = |pair: &str| pair.find('=').unwrap_or(pair.len());
                a[..key(a)].cmp(&b[..key(b)])
            });
            if !pairs.is_empty() {
                value.push('?');
                value.push_str(&pairs.join("&"));
            }
        }
        if let Some(fragment) = url.fragment().filter(|f| route(f)) {
            value.push('#');
            value.push_str(&normalize_percent(fragment));
        }
        PageMatch(value)
    }
}

const TENANTS: &[(&str, &str, usize)] = &[
    ("github.com", "", 2),
    ("gitlab.com", "", 1),
    ("medium.com", "@", 1),
    ("youtube.com", "@", 1),
    ("sites.google.com", "view/", 2),
    ("x.com", "", 1),
    ("reddit.com", "r/", 2),
];

#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PlaceKey {
    pub host: String,
    pub tenant: Option<String>,
}
impl PlaceKey {
    pub fn from_url(url: &Url) -> Option<Self> {
        SiteKey::from_url(url)?;
        let host = url.domain()?.trim_end_matches('.');
        let host = host.strip_prefix("www.").unwrap_or(host);
        let path = url.path().strip_prefix('/')?;
        let tenant = TENANTS
            .iter()
            .find(|(h, _, _)| *h == host)
            .and_then(|(_, prefix, depth)| {
                let tail = path.strip_prefix(prefix)?;
                if tail.split('/').next()?.is_empty() {
                    return None;
                }
                Some(format!(
                    "/{}",
                    path.split('/')
                        .take(*depth)
                        .take_while(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("/")
                ))
            });
        Some(Self {
            host: host.to_owned(),
            tenant,
        })
    }
    pub(crate) fn root(&self, url: &Url) -> Result<PageKey, KeyError> {
        let mut root = url.clone();
        root.set_path(self.tenant.as_deref().unwrap_or("/"));
        root.set_query(None);
        root.set_fragment(None);
        PageKey::from_url(&root)
    }
}

pub(crate) fn normalize_percent(value: &str) -> String {
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

pub(crate) fn same_page(a: &Url, b: &Url) -> bool {
    PageKey::from_url(a)
        .ok()
        .zip(PageKey::from_url(b).ok())
        .is_some_and(|(a, b)| a.page_match() == b.page_match())
}
