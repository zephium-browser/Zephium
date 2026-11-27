use crate::policy::normalize_https_url::normalize_https_url;
use dom_query::{Document, Selection};
use url::Url;

/// Resolves the first safe HTTPS document base, falling back to the page URL.
pub fn document_base_url(html: &str, page_url: &Url) -> Url {
    if html.len() > 4 * 1024 * 1024 {
        return page_url.clone();
    }
    Document::from(html)
        .select("base[href]")
        .nodes()
        .iter()
        .take(1)
        .find_map(|node| {
            let base = Selection::from(*node);
            normalize_https_url(page_url, &base.attr("href")?).ok()
        })
        .unwrap_or_else(|| page_url.clone())
}
