use thiserror::Error;
use url::{form_urlencoded, Url};

pub const MAX_URL_BYTES: usize = 2048;
pub const MAX_LOCATOR_BYTES: usize = 256;

const TRACKING_NAMES: &[&str] = &[
    "gclid", "dclid", "fbclid", "msclkid", "ref_src", "mc_cid", "mc_eid",
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

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageKey(Url);

impl PageKey {
    pub fn from_url(url: &Url) -> Result<Self, KeyError> {
        check_url(url, MAX_URL_BYTES)?;
        let mut page = url.clone();
        if let Some(host) = url.host_str().filter(|h| h.ends_with('.')) {
            page.set_host(Some(host.trim_end_matches('.')))
                .map_err(|_| KeyError::Url)?;
        }
        page.set_fragment(None);
        if let Some(query) = url.query() {
            let kept = query
                .split('&')
                .filter(|pair| {
                    let Some((key, _)) = form_urlencoded::parse(pair.as_bytes()).next() else {
                        return true;
                    };
                    let key = key.to_ascii_lowercase();
                    !TRACKING_NAMES.contains(&key.as_str())
                        && !TRACKING_PREFIXES.iter().any(|p| key.starts_with(p))
                })
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

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SiteKey(String);

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

#[derive(Clone, PartialEq, Eq)]
pub struct LocatorText(String);

impl LocatorText {
    pub fn new(text: &str) -> Result<Self, KeyError> {
        if text.len() > MAX_LOCATOR_BYTES
            || text.trim().is_empty()
            || text.chars().any(char::is_control)
        {
            return Err(KeyError::Locator);
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq, Default)]
pub enum Locator {
    #[default]
    None,
    Element(LocatorText),
    TextQuote {
        start: LocatorText,
        end: Option<LocatorText>,
    },
}

impl Locator {
    pub fn fragment(&self) -> Option<String> {
        fn encode(value: &LocatorText) -> String {
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            let mut fragment = String::with_capacity(value.as_str().len());
            for byte in value.as_str().bytes() {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'~') {
                    fragment.push(char::from(byte));
                } else {
                    fragment.push('%');
                    fragment.push(char::from(HEX[usize::from(byte >> 4)]));
                    fragment.push(char::from(HEX[usize::from(byte & 15)]));
                }
            }
            fragment
        }
        match self {
            Self::None => None,
            Self::Element(id) => Some(encode(id)),
            Self::TextQuote { start, end } => {
                let mut fragment = format!(":~:text={}", encode(start));
                if let Some(end) = end {
                    fragment.push(',');
                    fragment.push_str(&encode(end));
                }
                Some(fragment)
            }
        }
    }
}
