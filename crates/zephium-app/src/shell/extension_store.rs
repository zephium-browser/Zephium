//! Chrome Web Store listing recognition for the active tab.

pub(super) fn store_listing_url(url: &url::Url) -> bool {
    store_listing_id(url).is_some()
}

/// The extension ID a listing shows.
pub(super) fn store_listing_id(url: &url::Url) -> Option<&str> {
    if url.as_str().len() > 4096
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    let parts = url.path().split('/').collect::<Vec<_>>();
    let id = match (url.host_str(), parts.as_slice()) {
        (Some("chromewebstore.google.com"), ["", "detail", id]) => *id,
        (Some("chromewebstore.google.com"), ["", "detail", slug, id]) if !slug.is_empty() => *id,
        (Some("chrome.google.com"), ["", "webstore", "detail", slug, id]) if !slug.is_empty() => {
            *id
        }
        _ => return None,
    };
    (id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte))).then_some(id)
}
