//! Trusted document-finalization policy, never model destination authority.
use crate::ContextNavigationTarget;

/// Closed trusted policy supplied before native allocation or dispatch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkBrowserDocumentPolicy {
    /// The native current URL must remain exactly the requested URL.
    #[default]
    Exact,
    /// After the exact initial HTTPS document finishes, freeze its one native
    /// location sample. A query-free request may acquire an opaque nonempty
    /// query, but no other URL bytes may change. No second load is authorized.
    InitialQueryFinalization,
    /// Freeze a query-free HTTPS request after bounded native document
    /// finalization. May also be frozen into an explicitly authorized successor
    /// operation. Each operation owns its own finalization and exact receipt.
    DocumentQueryFinalization,
    /// Public browsing may rewrite its query during initial page setup. Freeze
    /// one safe native URL on the exact same origin/path and committed load;
    /// this grants neither a redirect nor any subsequent location change.
    /// A redirect of the initial load is refused; the gate keeps its safe
    /// public destination so the read's owner may admit a new read there.
    PublicQueryFinalization,
    /// Public interactions may update a safe query within the committed document.
    /// A bounded first-page GET reload of the exact requested address may settle
    /// automatic browser verification; no post-action replay is authorized.
    /// The native gate still refuses every other unapproved load, origin/path change,
    /// fragment and credential-bearing location. The admitted URL remains the
    /// resource's stable citation address; query state grants no new destination.
    /// Redirects are refused and kept as for `PublicQueryFinalization`.
    PublicSameDocumentQuery,
    /// A browser search's results page, read by Rust and never cited by its
    /// own address: before its first document is ready, its server and
    /// script redirects may move it anywhere on the requested site (safe
    /// public HTTPS only). Once ready it is held like
    /// `PublicSameDocumentQuery`; cross-site redirects are refused.
    PublicResultsPage,
    /// Work in the person's session on one site: redirects, committed loads
    /// and same-document location changes may move anywhere on the requested
    /// site (registrable domain, same scheme and port). Page-initiated loads
    /// are cancelled, never followed; cross-site locations are refused.
    SiteSession,
}

impl WorkBrowserDocumentPolicy {
    /// Anonymous public reading, whose loads must be safe public HTTPS.
    pub const fn is_public(self) -> bool {
        matches!(
            self,
            Self::PublicQueryFinalization | Self::PublicSameDocumentQuery | Self::PublicResultsPage
        )
    }
    /// The native gate follows same-site loads while the first document
    /// settles, instead of refusing them.
    pub const fn follows_site_loads(self) -> bool {
        matches!(self, Self::SiteSession | Self::PublicResultsPage)
    }
    /// Checks trusted admission, before any request is sent to the native port.
    pub fn admits_request(self, requested: &ContextNavigationTarget) -> bool {
        let url = requested.as_url();
        if self.is_public() {
            return url.scheme() == "https"
                && url.fragment().is_none()
                && crate::semantic_wire::model_safe_public_url(requested);
        }
        if self == Self::SiteSession {
            return site_scheme(requested)
                && url.fragment().is_none()
                && crate::semantic_wire::model_safe_public_url(requested);
        }
        self == Self::Exact
            || (url.scheme() == "https"
                && url.query().is_none()
                && url.fragment().is_none()
                && url.username().is_empty()
                && url.password().is_none())
    }

    /// Rechecks a native document receipt. This relation alone is not a
    /// native identity/commit proof; only the original request may supply one.
    pub fn admits_final_document(
        self,
        requested: &ContextNavigationTarget,
        effective: &ContextNavigationTarget,
    ) -> bool {
        if !self.admits_request(requested) {
            return false;
        }
        if requested == effective {
            return true;
        }
        if self == Self::SiteSession {
            return same_site(requested, effective);
        }
        if self == Self::PublicResultsPage {
            return same_site(requested, effective) && self.admits_request(effective);
        }
        if matches!(
            self,
            Self::PublicQueryFinalization | Self::PublicSameDocumentQuery
        ) {
            return self.admits_request(effective)
                && requested.as_url().as_str().split('?').next()
                    == effective.as_url().as_str().split('?').next();
        }
        matches!(
            self,
            Self::InitialQueryFinalization | Self::DocumentQueryFinalization
        ) && effective
            .as_url()
            .query()
            .is_some_and(|query| !query.is_empty())
            && effective.as_url().fragment().is_none()
            && effective
                .as_url()
                .as_str()
                .split_once('?')
                .map(|(base, _)| base)
                == Some(requested.as_url().as_str())
    }
}

/// Where a public read's first document was sent, by its server or its own
/// script, before it was ready. The native gate refuses that load and keeps
/// its safe public destination here, once; it grants no navigation itself.
#[derive(Clone, Default)]
pub struct WorkBrowserRedirect(std::sync::Arc<std::sync::Mutex<Option<ContextNavigationTarget>>>);

impl WorkBrowserRedirect {
    /// Keeps the first refused destination; later ones change nothing.
    pub fn record(&self, destination: ContextNavigationTarget) {
        if let Ok(mut slot) = self.0.lock() {
            slot.get_or_insert(destination);
        }
    }
    /// The refused destination, if the read was redirected.
    pub fn destination(&self) -> Option<ContextNavigationTarget> {
        self.0.lock().ok().and_then(|slot| slot.clone())
    }
}

impl std::fmt::Debug for WorkBrowserRedirect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkBrowserRedirect([redacted])")
    }
}

/// Set once when a read's main-frame load could not reach its site: no
/// connection, an unknown host, a refused or insecure connection. The page
/// still fails; its owner names it unreachable rather than lost.
#[derive(Clone, Default)]
pub struct WorkBrowserUnreachable(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl WorkBrowserUnreachable {
    /// Marks the load as never having reached its site.
    pub fn record(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
    /// Whether the load never reached its site.
    pub fn unreachable(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl std::fmt::Debug for WorkBrowserUnreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WorkBrowserUnreachable")
            .field(&self.unreachable())
            .finish()
    }
}

/// HTTPS, or plain HTTP on a loopback host (local qualification sites only).
pub fn site_scheme(target: &ContextNavigationTarget) -> bool {
    let url = target.as_url();
    url.username().is_empty()
        && url.password().is_none()
        && match url.scheme() {
            "https" => true,
            "http" => {
                matches!(
                    url.host(),
                    Some(url::Host::Ipv4(ip)) if ip.is_loopback()
                ) || url.host_str() == Some("localhost")
            }
            _ => false,
        }
}

/// The site a location belongs to: its registrable domain under a known
/// public suffix, or the exact host for an IP address or an unknown suffix.
pub fn registrable_site(target: &ContextNavigationTarget) -> Option<String> {
    let host = target
        .as_url()
        .host_str()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if matches!(target.as_url().host(), Some(url::Host::Domain(_))) {
        if let Some(domain) =
            psl::domain(host.as_bytes()).filter(|domain| domain.suffix().is_known())
        {
            return std::str::from_utf8(domain.as_bytes())
                .ok()
                .map(str::to_owned);
        }
    }
    Some(host)
}

/// Both locations are on one site: same scheme and port, same registrable
/// domain (private suffixes keep hosted tenants apart; IPs stay exact).
pub fn same_site(source: &ContextNavigationTarget, target: &ContextNavigationTarget) -> bool {
    site_scheme(source) && site_scheme(target) && crate::same_work_human_site(source, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_finalization_preserves_the_exact_base_and_refuses_secret_locations() {
        let policy = WorkBrowserDocumentPolicy::PublicQueryFinalization;
        let requested =
            ContextNavigationTarget::parse("https://example.test/catalog?sort=price").unwrap();
        for changed in [
            "https://example.test/catalog",
            "https://example.test/catalog?sort=price&page=1",
            "https://example.test/catalog?sort=name",
        ] {
            let effective = ContextNavigationTarget::parse(changed).unwrap();
            assert!(policy.admits_final_document(&requested, &effective));
            assert!(!WorkBrowserDocumentPolicy::Exact.admits_final_document(&requested, &effective));
        }
        for changed in [
            "https://example.test/other?sort=price",
            "https://other.test/catalog?sort=price",
            "http://example.test/catalog?sort=price",
            "https://example.test:444/catalog?sort=price",
            "https://example.test/catalog?sort=price#fragment",
            "https://example.test/catalog?access_token=private-value",
        ] {
            let effective = ContextNavigationTarget::parse(changed).unwrap();
            assert!(!policy.admits_final_document(&requested, &effective));
        }
    }
    #[test]
    fn startup_relation_is_opaque_but_never_a_url_or_navigation_substitution() {
        let source = ContextNavigationTarget::parse("https://example.test/product").unwrap();
        for policy in [
            WorkBrowserDocumentPolicy::InitialQueryFinalization,
            WorkBrowserDocumentPolicy::DocumentQueryFinalization,
        ] {
            for query in ["a=one", "arbitrary=two&another=three"] {
                let target = ContextNavigationTarget::parse(&format!(
                    "https://example.test/product?{query}"
                ))
                .unwrap();
                assert!(policy.admits_final_document(&source, &target));
                assert!(!WorkBrowserDocumentPolicy::Exact.admits_final_document(&source, &target));
                assert!(!policy.admits_request(&target));
            }
            for changed in [
                "https://example.test/product?",
                "https://example.test/product?x=1#f",
                "https://example.test/other?x=1",
                "https://else.test/product?x=1",
                "http://example.test/product?x=1",
                "https://example.test:444/product?x=1",
            ] {
                let target = ContextNavigationTarget::parse(changed).unwrap();
                assert!(!policy.admits_final_document(&source, &target));
            }
            assert!(policy.admits_final_document(&source, &source));
            let prefix = "https://example.test/product?";
            let at_bound = ContextNavigationTarget::parse(&format!(
                "{prefix}{}",
                "x".repeat(8_192 - prefix.len())
            ))
            .unwrap();
            assert!(policy.admits_final_document(&source, &at_bound));
            // The existing target type enforces the same byte ceiling before a
            // native receipt can carry a page-derived URL into this relation.
            assert!(ContextNavigationTarget::parse(&format!("{}x", at_bound.as_url())).is_err());
            assert!(
                ContextNavigationTarget::parse("https://user:secret@example.test/product?x=1")
                    .is_err()
            );
        }
    }

    #[test]
    fn a_results_page_settles_anywhere_on_its_site_over_safe_https() {
        let policy = WorkBrowserDocumentPolicy::PublicResultsPage;
        let search =
            ContextNavigationTarget::parse("https://www.google.com/search?q=rust").unwrap();
        assert!(policy.admits_request(&search));
        assert!(policy.is_public() && policy.follows_site_loads());
        for settled in [
            "https://www.google.com/search?q=rust&sei=abc",
            "https://consent.google.com/ml?continue=x",
            "https://google.com/sorry/index",
        ] {
            let settled = ContextNavigationTarget::parse(settled).unwrap();
            assert!(
                policy.admits_final_document(&search, &settled),
                "{settled:?}"
            );
        }
        for foreign in [
            "https://www.bing.com/search?q=rust",
            "http://www.google.com/search?q=rust",
            "https://www.google.com/search?q=rust#frag",
            "https://www.google.com/search?access_token=private-value",
        ] {
            let foreign = ContextNavigationTarget::parse(foreign).unwrap();
            assert!(
                !policy.admits_final_document(&search, &foreign),
                "{foreign:?}"
            );
        }
        for policy in [
            WorkBrowserDocumentPolicy::PublicQueryFinalization,
            WorkBrowserDocumentPolicy::PublicSameDocumentQuery,
        ] {
            assert!(policy.is_public() && !policy.follows_site_loads());
        }
        assert!(WorkBrowserDocumentPolicy::SiteSession.follows_site_loads());
        assert!(!WorkBrowserDocumentPolicy::SiteSession.is_public());
    }
    #[test]
    fn a_redirect_slot_keeps_its_first_destination_and_redacts_it() {
        let slot = WorkBrowserRedirect::default();
        assert_eq!(slot.destination(), None);
        let first = ContextNavigationTarget::parse("https://www.example.com/a").unwrap();
        slot.clone().record(first.clone());
        slot.record(ContextNavigationTarget::parse("https://www.example.com/b").unwrap());
        assert_eq!(slot.destination(), Some(first));
        assert!(!format!("{slot:?}").contains("example"));
    }

    #[test]
    fn an_unreachable_slot_is_shared_by_its_clones() {
        let slot = WorkBrowserUnreachable::default();
        assert!(!slot.unreachable());
        slot.clone().record();
        assert!(slot.unreachable());
    }

    #[test]
    fn a_site_is_its_registrable_domain() {
        for (url, site) in [
            ("https://app.slack.com/client", "slack.com"),
            ("https://www.bbc.co.uk/news", "bbc.co.uk"),
            ("https://alice.github.io/x", "alice.github.io"),
            ("http://127.0.0.1:4100/", "127.0.0.1"),
            ("https://intranet.invalid/", "intranet.invalid"),
        ] {
            let target = ContextNavigationTarget::parse(url).unwrap();
            assert_eq!(registrable_site(&target).as_deref(), Some(site), "{url}");
        }
    }
    #[test]
    fn site_session_moves_within_the_registrable_domain_only() {
        let policy = WorkBrowserDocumentPolicy::SiteSession;
        let start = ContextNavigationTarget::parse("https://app.slack.com/client").unwrap();
        assert!(policy.admits_request(&start));
        for moved in [
            "https://app.slack.com/client/T1/C2",
            "https://files.slack.com/files/x?y=1",
            "https://slack.com/signin#done",
        ] {
            let moved = ContextNavigationTarget::parse(moved).unwrap();
            assert!(policy.admits_final_document(&start, &moved), "{moved:?}");
        }
        for foreign in [
            "https://slack.com.evil.test/",
            "https://accounts.google.com/",
            "http://app.slack.com/client",
            "https://app.slack.com:8443/client",
        ] {
            let foreign = ContextNavigationTarget::parse(foreign).unwrap();
            assert!(
                !policy.admits_final_document(&start, &foreign),
                "{foreign:?}"
            );
        }
        let tenant = ContextNavigationTarget::parse("https://one.github.io/a").unwrap();
        let other = ContextNavigationTarget::parse("https://two.github.io/a").unwrap();
        assert!(!policy.admits_final_document(&tenant, &other));
        let loopback = ContextNavigationTarget::parse("http://127.0.0.1:4100/inbox").unwrap();
        assert!(policy.admits_request(&loopback));
        assert!(policy.admits_final_document(
            &loopback,
            &ContextNavigationTarget::parse("http://127.0.0.1:4100/next").unwrap()
        ));
        assert!(!policy.admits_final_document(
            &loopback,
            &ContextNavigationTarget::parse("http://127.0.0.1:4101/next").unwrap()
        ));
        assert!(!policy
            .admits_request(&ContextNavigationTarget::parse("http://example.test/").unwrap()));
        // An in-page anchor is never a navigation request of a session.
        assert!(!policy.admits_request(
            &ContextNavigationTarget::parse("https://linear.app/acme/agent#skip-nav").unwrap()
        ));
        // Notion's workspace moved from notion.so to app.notion.com.
        assert!(policy.admits_final_document(
            &ContextNavigationTarget::parse("https://www.notion.so/").unwrap(),
            &ContextNavigationTarget::parse("https://app.notion.com/acme").unwrap()
        ));
    }
}
