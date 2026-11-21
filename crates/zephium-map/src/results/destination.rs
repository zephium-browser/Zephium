use crate::key::{check_url, MAX_URL_BYTES};
use url::Url;
use zephium_core::search::SearchEngine;

pub(super) struct Rules<'a> {
    pub engine: SearchEngine,
    pub own: &'a str,
    pub page_host: &'a str,
}

pub(super) fn on(host: &str, domain: &str) -> bool {
    host.trim_end_matches('.')
        .strip_suffix(domain)
        .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with('.'))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Drop {
    Paid,
    Own,
    Invalid,
}

const ENGINE_HOSTS: &[(SearchEngine, &[&str])] = &[
    (SearchEngine::Google, &["gstatic.com"]),
    (SearchEngine::Bing, &["bing.com", "bing.net"]),
    (SearchEngine::Brave, &["search.brave.com"]),
    (SearchEngine::DuckDuckGo, &["duckduckgo.com", "duck.com"]),
];
const AD_HOSTS: &[&str] = &["googleadservices.com", "doubleclick.net", "bat.bing.com"];
const AD_PATHS: &[&str] = &[
    "aclk",
    "y.js",
    "/aclick",
    "/pagead/",
    "/adclick",
    "/ad/click",
    "/ads/click",
];
const PLAIN_KEYS: &[&str] = &["url", "u", "dest", "target"];

impl Rules<'_> {
    fn own_host(&self, host: &str) -> bool {
        if self.engine == SearchEngine::Custom {
            return on(host, self.own) || on(host, self.page_host.trim_end_matches('.'));
        }
        let host = host.trim_end_matches('.');
        let google = self.engine == SearchEngine::Google
            && host
                .strip_prefix("www.")
                .unwrap_or(host)
                .strip_prefix("google.")
                .is_some_and(|suffix| psl::suffix_str(host) == Some(suffix));
        google
            || ENGINE_HOSTS
                .iter()
                .any(|(engine, hosts)| self.engine == *engine && hosts.iter().any(|h| on(host, h)))
    }

    pub fn destination(&self, source: &Url) -> Result<Url, Drop> {
        let classify = |u: &Url| {
            check_url(u, MAX_URL_BYTES).map_err(|_| Drop::Invalid)?;
            let host = u.host_str().ok_or(Drop::Invalid)?;
            if AD_HOSTS.iter().any(|h| on(host, h)) {
                return Err(Drop::Paid);
            }
            let own = self.own_host(host);
            let redirect_host = own
                || ENGINE_HOSTS
                    .iter()
                    .flat_map(|(_, hosts)| *hosts)
                    .any(|h| on(host, h))
                || psl::domain_str(host).is_some_and(|d| d.starts_with("google."));
            if redirect_host && is_ad_click(u) {
                return Err(Drop::Paid);
            }
            Ok(own)
        };
        let mut url = if classify(source)? {
            let value = |keys: &[&str]| {
                source
                    .query_pairs()
                    .find(|(key, _)| keys.contains(&key.as_ref()))
                    .map(|(_, v)| v.into_owned())
            };
            let raw = match source.path() {
                "/ck/a" => value(&["u"]).and_then(|s| decode_bing(&s)),
                "/url" => value(&["q", "url"]),
                "/l" | "/l/" => value(&["uddg"]),
                _ => value(PLAIN_KEYS),
            }
            .ok_or(Drop::Own)?;
            let url = Url::parse(&raw).map_err(|_| Drop::Invalid)?;
            if classify(&url)? {
                return Err(Drop::Own);
            }
            url
        } else {
            source.clone()
        };
        if !url.fragment().is_some_and(crate::key::route) {
            url.set_fragment(None);
        }
        Ok(url)
    }
}

fn is_ad_click(url: &Url) -> bool {
    let ad_path = |piece: &str| {
        let lower = piece.to_ascii_lowercase();
        AD_PATHS.iter().any(|p| lower.contains(p))
    };
    url::form_urlencoded::parse(url.path().as_bytes())
        .any(|(key, value)| ad_path(&key) || ad_path(&value))
        || url
            .query_pairs()
            .any(|(key, _)| matches!(key.as_ref(), "adurl" | "ad_domain" | "ad_provider"))
}

fn decode_bing(raw: &str) -> Option<String> {
    let encoded = raw.strip_prefix("a1")?.trim_end_matches('=');
    let mut bytes = Vec::with_capacity(encoded.len());
    let (mut pending, mut bits) = (0u16, 0);
    for byte in encoded.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        pending = (pending << 6) | u16::from(digit);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((pending >> bits) as u8);
            pending &= (1 << bits) - 1;
        }
    }
    if bits >= 6 || pending != 0 {
        return None;
    }
    String::from_utf8(bytes).ok()
}
