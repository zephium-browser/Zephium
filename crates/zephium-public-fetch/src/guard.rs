use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
};

pub const MAX_URL_BYTES: usize = 4096;
pub const MAX_RESOLVED_ADDRESSES: usize = 32;
pub const SHARED_RESOLVER_SLOTS: usize = 4;
pub const MAX_HOST_BYTES: usize = 253;
const UNREACHABLE_SUFFIXES: [&str; 3] = ["onion", "arpa", "alt"];

/// Only public HTTPS origins: no loopback, private, link-local, or `.local`.
pub fn public_https(url: &url::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > MAX_URL_BYTES
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.').to_ascii_lowercase();
            !domain.is_empty()
                && domain != "localhost"
                && !domain.ends_with(".localhost")
                && !domain.ends_with(".local")
                && !domain.ends_with(".internal")
                && domain.contains('.')
        }
        Some(url::Host::Ipv4(ip)) => public_address(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => public_address(IpAddr::V6(ip)),
        None => false,
    }
}

/// The reader's own admission: [`public_https`], plus a named host under a
/// known public suffix on the default port. Never an address literal, so a
/// proxy that resolves the name itself is never handed an inward address.
pub fn public_target(url: &url::Url) -> bool {
    if !public_https(url) || url.port().is_some() {
        return false;
    }
    let Some(url::Host::Domain(domain)) = url.host() else {
        return false;
    };
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    if domain.len() > MAX_HOST_BYTES
        || domain
            .rsplit('.')
            .next()
            .is_some_and(|last| UNREACHABLE_SUFFIXES.contains(&last))
    {
        return false;
    }
    psl::domain(domain.as_bytes()).is_some_and(|registrable| registrable.suffix().is_known())
}

/// Conservatively admits public unicast addresses, excluding private, loopback,
/// link-local, documentation, transition/tunnel and reserved special-purpose blocks.
/// Callers must apply this to the exact addresses used by their connector.
pub fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || a >= 224
                || a == 100 && (64..=127).contains(&b)
                || a == 192 && (b == 0 && c == 0 || b == 88 && c == 99)
                || a == 198 && (18..=19).contains(&b))
        }
        IpAddr::V6(ip) => {
            let parts = ip.segments();
            // Public unicast only, excluding transition/tunnel and reserved
            // special-purpose blocks rather than interpreting embedded hosts.
            (parts[0] & 0xe000) == 0x2000
                && parts[0] != 0x2002
                && !(parts[0] == 0x2001 && (parts[1] < 0x0200 || parts[1] == 0x0db8))
                && !(parts[0] == 0x3fff && parts[1] < 0x1000)
        }
    }
}

#[derive(Debug)]
pub(crate) struct NonPublicAnswer;

impl std::fmt::Display for NonPublicAnswer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("non-public address answer or resolution limit")
    }
}

impl std::error::Error for NonPublicAnswer {}

/// The whole answer is refused when any address in it is not public, so a
/// name cannot mix an inward address in with an outward one.
pub fn public_addresses(
    addresses: impl Iterator<Item = SocketAddr>,
) -> Result<reqwest::dns::Addrs, std::io::Error> {
    let mut kept = Vec::new();
    for address in addresses.take(MAX_RESOLVED_ADDRESSES + 1) {
        if kept.len() == MAX_RESOLVED_ADDRESSES || !public_address(address.ip()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                NonPublicAnswer,
            ));
        }
        kept.push(address);
    }
    if kept.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no public address",
        ));
    }
    Ok(Box::new(kept.into_iter()))
}

/// Whether an error chain carries the resolver's refusal of a non-public answer.
pub(crate) fn refused_answer(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.is::<NonPublicAnswer>()
            || error
                .downcast_ref::<std::io::Error>()
                .and_then(std::io::Error::get_ref)
                .is_some_and(|inner| inner.is::<NonPublicAnswer>())
        {
            return true;
        }
        current = error.source();
    }
    false
}

// getaddrinfo runs on a blocking worker and cannot be cancelled by dropping
// its async waiter. Keep a physical bound until that worker returns.
pub(crate) struct ResolverSlots {
    used: AtomicUsize,
    maximum: usize,
}

impl ResolverSlots {
    pub(crate) fn new(maximum: usize) -> Self {
        Self {
            used: AtomicUsize::new(0),
            maximum,
        }
    }

    fn reserve(self: &Arc<Self>) -> Result<ResolverPermit, std::io::Error> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.maximum).then(|| used + 1)
            })
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::WouldBlock, "public resolver capacity")
            })?;
        Ok(ResolverPermit {
            slots: self.clone(),
            returned: false,
        })
    }
}

struct ResolverPermit {
    slots: Arc<ResolverSlots>,
    returned: bool,
}

impl ResolverPermit {
    fn returned(mut self) {
        self.returned = true;
    }
}

impl Drop for ResolverPermit {
    fn drop(&mut self) {
        // A panic or a dropped queued worker is not a confirmed resolver return.
        // Conservatively quarantine its slot instead of advertising false capacity.
        if self.returned {
            self.slots.used.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

pub(crate) async fn resolve_with(
    slots: Arc<ResolverSlots>,
    resolve: impl FnOnce() -> std::io::Result<reqwest::dns::Addrs> + Send + 'static,
) -> std::io::Result<reqwest::dns::Addrs> {
    let permit = slots.reserve()?;
    tokio::task::spawn_blocking(move || {
        let result = resolve();
        permit.returned();
        result
    })
    .await
    .map_err(|_| std::io::Error::other("public resolver did not return"))?
}

struct VettingResolver {
    slots: Arc<ResolverSlots>,
}

impl reqwest::dns::Resolve for VettingResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let slots = self.slots.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            Ok(resolve_with(slots, move || {
                // These exact checked addresses reach the connector; there is no
                // second resolution between validation and socket creation.
                public_addresses((host.as_str(), 0).to_socket_addrs()?)
            })
            .await?)
        })
    }
}

/// A resolver that answers only public unicast addresses, checked once and
/// handed to the connector as-is, so a name that points inward cannot reach a
/// private host however it is spelled or redirected.
pub fn public_resolver() -> Arc<dyn reqwest::dns::Resolve> {
    static SLOTS: OnceLock<Arc<ResolverSlots>> = OnceLock::new();
    let slots = SLOTS
        .get_or_init(|| Arc::new(ResolverSlots::new(SHARED_RESOLVER_SLOTS)))
        .clone();
    Arc::new(VettingResolver { slots })
}

pub(crate) fn vetting_resolver(slots: Arc<ResolverSlots>) -> Arc<dyn reqwest::dns::Resolve> {
    Arc::new(VettingResolver { slots })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::time::Duration;

    #[tokio::test]
    async fn cancelled_dns_waiter_retains_capacity_until_physical_return() {
        let slots = Arc::new(ResolverSlots::new(1));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let owned = slots.clone();
        let caller = tokio::spawn(async move {
            resolve_with(owned, move || {
                let _ = started_tx.send(());
                release_rx.recv().unwrap();
                public_addresses(["93.184.216.34:443".parse().unwrap()].into_iter())
            })
            .await
        });
        started_rx.await.unwrap();
        caller.abort();
        assert!(matches!(caller.await, Err(error) if error.is_cancelled()));
        assert_eq!(slots.used.load(Ordering::Acquire), 1);
        let refused = resolve_with(slots.clone(), || {
            panic!("capacity must refuse before worker dispatch")
        })
        .await;
        assert!(matches!(refused, Err(error) if error.kind() == std::io::ErrorKind::WouldBlock));
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while slots.used.load(Ordering::Acquire) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let next = resolve_with(slots.clone(), || {
            public_addresses(["93.184.216.34:443".parse().unwrap()].into_iter())
        })
        .await
        .unwrap();
        assert_eq!(next.count(), 1);
        assert_eq!(slots.used.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn returned_dns_error_releases_capacity_but_panic_does_not() {
        let slots = Arc::new(ResolverSlots::new(1));
        let error = resolve_with(slots.clone(), || {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "injected",
            ))
        })
        .await;
        assert!(matches!(error, Err(error) if error.kind() == std::io::ErrorKind::NotFound));
        assert_eq!(slots.used.load(Ordering::Acquire), 0);
        let panic = resolve_with(slots.clone(), || panic!("injected resolver panic")).await;
        assert!(matches!(panic, Err(error) if error.kind() == std::io::ErrorKind::Other));
        assert_eq!(slots.used.load(Ordering::Acquire), 1);
        assert!(slots.reserve().is_err());
    }

    pub(crate) const PUBLIC_HTTPS_ADMITTED: [&str; 3] = [
        "https://cdn.example/a.png",
        "https://93.184.216.34/logo.webp",
        "https://images.example.co.uk/x.jpg?w=200",
    ];

    pub(crate) const PUBLIC_HTTPS_REFUSED: [&str; 14] = [
        "http://cdn.example/a.png",
        "https://localhost/a.png",
        "https://api.localhost/a.png",
        "https://printer.local/a.png",
        "https://metadata.internal/a.png",
        "https://127.0.0.1/a.png",
        "https://10.0.0.5/a.png",
        "https://169.254.169.254/latest",
        "https://100.64.0.1/a.png",
        "https://[::1]/a.png",
        "https://[fd00::1]/a.png",
        "https://[fe80::1]/a.png",
        "https://user:pw@cdn.example/a.png",
        "https://intranet/a.png",
    ];

    #[test]
    fn only_public_https_origins_are_fetchable() {
        for ok in PUBLIC_HTTPS_ADMITTED {
            assert!(public_https(&url::Url::parse(ok).unwrap()), "{ok}");
        }
        for bad in PUBLIC_HTTPS_REFUSED {
            assert!(!public_https(&url::Url::parse(bad).unwrap()), "{bad}");
        }
    }

    #[test]
    fn the_reader_admits_only_named_hosts_under_a_known_suffix_on_the_default_port() {
        for ok in [
            "https://docs.example.com/pricing",
            "https://example.com:443/",
            "https://www.bbc.co.uk/news",
            "https://user-pages.github.io/a",
            "https://xn--bcher-kva.de/",
            "https://example.com./sitemap.xml",
        ] {
            assert!(public_target(&url::Url::parse(ok).unwrap()), "{ok}");
        }
        for bad in [
            "https://93.184.216.34/",
            "https://[2606:4700::1111]/",
            "https://example.com:8443/",
            "https://example.com:80/",
            "https://cdn.example/a.png",
            "https://router.lan/",
            "https://wiki.corp/",
            "https://nas.home.arpa/",
            "https://example.onion/",
            "https://site.test/",
            "https://co.uk/",
            "https://com/",
            "http://example.com/",
            "https://user@example.com/",
            "https://example.com.local/",
        ] {
            assert!(!public_target(&url::Url::parse(bad).unwrap()), "{bad}");
        }
        let label = "a".repeat(63);
        let host = |last: usize| format!("{label}.{label}.{label}.{}.com", "d".repeat(last));
        assert_eq!(host(57).len(), MAX_HOST_BYTES);
        assert!(public_target(
            &url::Url::parse(&format!("https://{}/", host(57))).unwrap()
        ));
        assert!(!public_target(
            &url::Url::parse(&format!("https://{}/", host(58))).unwrap()
        ));
        let at = |len: usize| format!("https://example.com/{}", "a".repeat(len - 20));
        assert!(public_target(&url::Url::parse(&at(MAX_URL_BYTES)).unwrap()));
        assert!(!public_target(
            &url::Url::parse(&at(MAX_URL_BYTES + 1)).unwrap()
        ));
    }

    #[test]
    fn the_reader_guard_is_at_least_as_strict_on_every_existing_case() {
        for case in PUBLIC_HTTPS_ADMITTED
            .into_iter()
            .chain(PUBLIC_HTTPS_REFUSED)
        {
            let url = url::Url::parse(case).unwrap();
            assert!(!public_target(&url) || public_https(&url), "{case}");
        }
        for refused in PUBLIC_HTTPS_REFUSED {
            assert!(!public_target(&url::Url::parse(refused).unwrap()));
        }
    }

    #[test]
    fn resolved_addresses_cannot_cross_private_or_special_networks() {
        let denied = [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.8",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.1.1",
            "198.19.255.255",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
            "fc00::1",
            "fe80::1",
            "ff02::1",
            "2001::1",
            "2001:2::1",
            "2001:db8::1",
            "2002:7f00:1::1",
            "3fff::1",
        ];
        let good: SocketAddr = "93.184.216.34:443".parse().unwrap();
        for address in denied {
            let ip = address.parse().unwrap();
            assert!(!public_address(ip), "{address}");
            let mixed = public_addresses([good, SocketAddr::new(ip, 443)].into_iter());
            assert!(matches!(&mixed, Err(error) if refused_answer(error)));
            assert!(public_addresses([SocketAddr::new(ip, 443), good].into_iter()).is_err());
        }
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "198.17.255.255",
            "198.20.0.0",
            "2606:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(public_address(address.parse().unwrap()), "{address}");
        }
        assert!(
            matches!(public_addresses(std::iter::empty()), Err(error) if !refused_answer(&error))
        );
        assert_eq!(
            public_addresses(std::iter::repeat_n(good, MAX_RESOLVED_ADDRESSES))
                .unwrap()
                .count(),
            MAX_RESOLVED_ADDRESSES
        );
        assert!(public_addresses(std::iter::repeat_n(good, MAX_RESOLVED_ADDRESSES + 1)).is_err());
    }

    fn url_text() -> impl Strategy<Value = String> {
        let scheme = prop_oneof![
            Just("https"),
            Just("http"),
            Just("HTTPS"),
            Just("ftp"),
            Just("file")
        ];
        let userinfo = prop_oneof![
            Just(""),
            Just("user@"),
            Just("user:pw@"),
            Just(":@"),
            Just("@")
        ];
        let host = prop_oneof![
            "[a-z0-9-]{1,12}(\\.[a-z0-9-]{1,12}){0,3}\\.?",
            "[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}",
            "\\[[0-9a-f:]{2,20}\\]",
            "0x[0-9a-f]{1,8}",
            "[^/?#@:]{0,24}",
        ];
        let port = prop_oneof![
            Just(String::new()),
            (0u32..70000).prop_map(|p| format!(":{p}"))
        ];
        let tail = "[ -~]{0,40}";
        (scheme, userinfo, host, port, tail)
            .prop_map(|(s, u, h, p, t)| format!("{s}://{u}{h}{p}/{t}"))
    }

    proptest! {
        #[test]
        fn no_url_makes_the_guard_panic_or_admit_more_than_public_https(text in url_text()) {
            if let Ok(url) = url::Url::parse(&text) {
                let admitted = public_target(&url);
                prop_assert!(!admitted || public_https(&url));
                if admitted {
                    prop_assert!(matches!(url.host(), Some(url::Host::Domain(_))));
                    prop_assert!(url.port().is_none());
                }
            }
        }

        #[test]
        fn arbitrary_text_never_panics_the_guard(text in "\\PC{0,200}") {
            if let Ok(url) = url::Url::parse(&text) {
                let _ = public_target(&url);
                let _ = public_https(&url);
            }
        }
    }
}
