//! The main document a view last committed, as its request filter sees it.
//! WebView2 names no initiating frame, so on Windows this document stands in
//! as the source of every request the view's filter decides.

use std::cell::Cell;
use std::rc::Rc;

use zephium_core::blocker::MAX_NETWORK_REQUEST_URL_BYTES;

#[derive(Clone, Default)]
pub(crate) struct TopDocument(Rc<Cell<Option<Rc<str>>>>);

impl TopDocument {
    /// Follows a main-frame commit. The previous document stays the source
    /// until then, so its late requests are not judged against the next site.
    /// A commit to anything but an HTTP(S) document leaves requests without a
    /// source.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn committed(&self, url: &str) {
        let attributable = url.len() <= MAX_NETWORK_REQUEST_URL_BYTES
            && ["https://", "http://"].iter().any(|scheme| {
                url.get(..scheme.len())
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
            });
        self.0.set(attributable.then(|| Rc::from(url)));
    }

    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn current(&self) -> Option<Rc<str>> {
        let current = self.0.take();
        self.0.set(current.clone());
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_committed_http_document_is_a_source() {
        let top = TopDocument::default();
        assert_eq!(top.current(), None);
        top.committed("https://news.example/story");
        assert_eq!(top.current().as_deref(), Some("https://news.example/story"));
        top.committed("HTTP://plain.example/");
        assert_eq!(top.current().as_deref(), Some("HTTP://plain.example/"));
        for other in [
            "about:blank",
            "file:///C:/page.html",
            "zephium://settings",
            "data:,x",
        ] {
            top.committed(other);
            assert_eq!(top.current(), None, "{other}");
        }
    }

    #[test]
    fn an_oversized_document_url_is_no_source() {
        let top = TopDocument::default();
        top.committed("https://news.example/");
        top.committed(&format!(
            "https://news.example/{}",
            "a".repeat(MAX_NETWORK_REQUEST_URL_BYTES)
        ));
        assert_eq!(top.current(), None);
    }

    #[test]
    fn clones_follow_one_view() {
        let top = TopDocument::default();
        let filter = top.clone();
        top.committed("https://news.example/");
        assert_eq!(filter.current().as_deref(), Some("https://news.example/"));
    }
}
