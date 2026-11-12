//! Platform-neutral current-main-document request provenance for Windows.

#[derive(Default)]
pub(crate) struct NavigationWitness {
    pub(crate) bootstrap: Option<u64>,
    document: Option<u64>,
    main_uri: Option<String>,
    get_uri: Option<String>,
    non_get_observed: bool,
}

impl NavigationWitness {
    pub(crate) fn started(&mut self, id: u64, uri: String, allow_bootstrap: bool) -> bool {
        if allow_bootstrap && uri == "about:blank" && self.document.is_none() {
            if self.bootstrap.is_some_and(|prior| prior != id) {
                return false;
            }
            self.bootstrap = Some(id);
            return true;
        }
        if !(uri.starts_with("http://") || uri.starts_with("https://")) {
            return false;
        }
        // NavigationStarting is top-level. A new identity or redirect replaces
        // the request we need to witness; it is not a lifetime replay veto.
        self.document = Some(id);
        self.main_uri = Some(uri);
        self.get_uri = None;
        self.non_get_observed = false;
        true
    }

    pub(crate) fn requested(&mut self, uri: String, method: &str) {
        // DOCUMENT resource notifications include child frames. Only the URI
        // of the current top-level start can provide its GET provenance.
        if self.main_uri.as_deref() != Some(uri.as_str()) {
            return;
        }
        self.non_get_observed |= method != "GET";
        self.get_uri = (!self.non_get_observed && method == "GET").then_some(uri);
    }

    pub(crate) fn allows(&self, current_url: &str) -> bool {
        self.get_uri
            .as_deref()
            .is_some_and(|uri| uri.split('#').next() == current_url.split('#').next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_get_can_be_replayed_after_reload_or_replacement() {
        let mut witness = NavigationWitness::default();
        assert!(witness.started(1, "https://example.test/a".into(), false));
        witness.requested("https://example.test/a".into(), "GET");
        assert!(witness.allows("https://example.test/a#fragment"));
        assert!(witness.started(2, "https://example.test/b".into(), false));
        assert!(!witness.allows("https://example.test/a"));
        witness.requested("https://example.test/b".into(), "GET");
        assert!(witness.allows("https://example.test/b"));
        assert!(witness.started(3, "https://example.test/b".into(), false));
        witness.requested("https://example.test/b".into(), "GET");
        assert!(witness.allows("https://example.test/b"));
    }

    #[test]
    fn child_documents_cannot_overwrite_main_provenance_and_post_never_grants_it() {
        let mut witness = NavigationWitness::default();
        witness.started(1, "https://example.test/".into(), false);
        witness.requested("https://example.test/".into(), "GET");
        witness.requested("https://frame.test/".into(), "POST");
        witness.requested("https://frame.test/".into(), "GET");
        for child in [
            "about:blank",
            "data:text/html,child",
            "blob:https://example.test/child",
        ] {
            witness.requested(child.into(), "GET");
        }
        assert!(witness.allows("https://example.test/"));
        assert!(!witness.allows("https://frame.test/"));
        witness.started(2, "https://example.test/post".into(), false);
        witness.requested("https://example.test/post".into(), "POST");
        assert!(!witness.allows("https://example.test/post"));
        // Even a same-URL iframe GET cannot turn an observed POST into GET.
        witness.requested("https://example.test/post".into(), "GET");
        assert!(!witness.allows("https://example.test/post"));
        witness.started(2, "https://example.test/redirect".into(), false);
        assert!(!witness.allows("https://example.test/post"));
        witness.requested("https://example.test/redirect".into(), "GET");
        assert!(witness.allows("https://example.test/redirect"));
    }

    #[test]
    fn owned_blank_does_not_grant_request_provenance() {
        let mut witness = NavigationWitness::default();
        assert!(witness.started(1, "about:blank".into(), true));
        assert!(!witness.started(2, "about:blank".into(), true));
        assert!(!witness.allows("about:blank"));
        assert!(!witness.started(3, "file:///private".into(), true));
    }
}
