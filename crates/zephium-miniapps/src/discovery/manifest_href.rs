use crate::policy::normalize_https_url::normalize_https_url;
use dom_query::{Document, Selection};
use url::Url;

/// Finds the document's Web App Manifest link. Only the first valid HTTPS
/// candidate is returned; a page can name many links but never more than one
/// installation target per call.
pub fn manifest_href(html: &str, base_url: &Url) -> Option<Url> {
    if html.len() > 4 * 1024 * 1024 {
        return None;
    }
    let document = Document::from(html);
    document
        .select("link")
        .nodes()
        .iter()
        .take(256)
        .find_map(|node| {
            let link = Selection::from(*node);
            let rel = link.attr("rel")?.to_string();
            if !rel
                .split_whitespace()
                .any(|value| value.eq_ignore_ascii_case("manifest"))
            {
                return None;
            }
            let href = link.attr("href")?.to_string();
            normalize_https_url(base_url, &href).ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_safe_manifest_link() {
        let page = Url::parse("https://example.com/app/").unwrap();
        let html = r#"<html><head>
            <link rel="stylesheet" href="/app.css">
            <link rel="manifest" href="/app.webmanifest">
            <link rel="manifest" href="http://insecure.example/manifest.json">
        </head></html>"#;
        assert_eq!(
            manifest_href(html, &page).map(|url| url.to_string()),
            Some("https://example.com/app.webmanifest".to_owned())
        );
    }

    #[test]
    fn ignores_links_without_a_manifest_relation() {
        let page = Url::parse("https://example.com/").unwrap();
        let html = r#"<link rel="icon" href="/manifest.json">"#;
        assert!(manifest_href(html, &page).is_none());
    }

    #[test]
    fn rejects_manifest_links_that_switch_origin_or_scheme() {
        let page = Url::parse("https://example.com/").unwrap();
        let html = r#"<link rel="manifest" href="javascript:alert(1)">"#;
        assert!(manifest_href(html, &page).is_none());
    }
}
