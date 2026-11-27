use crate::manifest::normalize_text::normalize_text;
use crate::policy::normalize_https_url::normalize_https_url;
use dom_query::{Document, Selection};
use url::Url;

/// Bounded, untrusted metadata extracted from a site's HTML document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SiteMetadata {
    pub name: Option<String>,
    pub icons: Vec<String>,
}

/// Finds a readable app name and a short ordered list of safe HTTPS icons.
/// HTML is untrusted; the parser never executes scripts or follows URLs.
pub fn site_metadata(html: &str, page_url: &Url) -> SiteMetadata {
    if html.len() > 4 * 1024 * 1024 {
        return SiteMetadata::default();
    }
    let document = Document::from(html);
    let name = [
        ("meta[property]", "property", "og:site_name"),
        ("meta[name]", "name", "application-name"),
        ("meta[name]", "name", "apple-mobile-web-app-title"),
    ]
    .into_iter()
    .find_map(|(selector, key, expected)| {
        document
            .select(selector)
            .nodes()
            .iter()
            .take(64)
            .find_map(|node| {
                let meta = Selection::from(*node);
                meta.attr(key)
                    .filter(|value| value.eq_ignore_ascii_case(expected))
                    .and_then(|_| meta.attr("content"))
                    .and_then(|value| normalize_text(&value, 80))
            })
    })
    .or_else(|| {
        let title = document.select("title").text();
        normalize_text(&title, 80)
    });

    let mut icons: Vec<(u8, u32, String)> = document
        .select("link")
        .nodes()
        .iter()
        .take(256)
        .filter_map(|node| {
            let link = Selection::from(*node);
            let rel = link.attr("rel")?.to_string();
            let tokens: Vec<_> = rel.split_whitespace().collect();
            let rank = if tokens
                .iter()
                .any(|value| value.eq_ignore_ascii_case("apple-touch-icon"))
            {
                0
            } else if tokens
                .iter()
                .any(|value| value.eq_ignore_ascii_case("icon"))
            {
                1
            } else {
                return None;
            };
            let href = link.attr("href")?.to_string();
            let url = normalize_https_url(page_url, &href).ok()?;
            let size = link
                .attr("sizes")
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|value| {
                    let (width, height) = value.split_once('x')?;
                    Some(
                        width
                            .parse::<u32>()
                            .ok()?
                            .saturating_mul(height.parse::<u32>().ok()?),
                    )
                })
                .max()
                .unwrap_or_default();
            Some((rank, u32::MAX - size, url.to_string()))
        })
        .collect();
    icons.sort_unstable();
    let mut seen = std::collections::HashSet::new();
    let icons = icons
        .into_iter()
        .filter_map(|(_, _, url)| seen.insert(url.clone()).then_some(url))
        .take(3)
        .collect();
    SiteMetadata { name, icons }
}
