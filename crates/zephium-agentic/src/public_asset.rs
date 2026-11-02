//! One bounded, cookie-free fetch of a public image for media admission.
//! The bytes are data for the store's own sniffing and decoding; nothing
//! here trusts a server's declared type beyond a coarse `image/*` gate.
use reqwest::{redirect::Policy, Client};
use std::{
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

/// Largest public image body accepted before decoding; admission scales a
/// large photo down to display size before it is stored.
pub const MAX_PUBLIC_IMAGE_BYTES: usize = 24 * 1024 * 1024;
/// The long edge a display-sized variant is asked for.
pub const DISPLAY_EDGE: u32 = 1600;
const MAX_REDIRECTS: usize = 3;

/// Why a public image was not fetched. Never carries response content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicAssetError {
    /// Not a public HTTPS URL.
    InvalidUrl,
    /// Transport failure, non-200 status, or a refused redirect.
    Unavailable,
    /// The body exceeds [`MAX_PUBLIC_IMAGE_BYTES`].
    TooLarge,
    /// The server did not answer with an image body.
    NotAnImage,
}

/// Only public HTTPS origins: no loopback, private, link-local, or `.local`.
pub fn public_https(url: &url::Url) -> bool {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.as_str().len() > 4096
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
fn public_addresses(
    addresses: impl Iterator<Item = SocketAddr>,
) -> Result<reqwest::dns::Addrs, std::io::Error> {
    let mut kept = Vec::new();
    for address in addresses.take(33) {
        if kept.len() == 32 || !public_address(address.ip()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "non-public image address or resolution limit",
            ));
        }
        kept.push(address);
    }
    if kept.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no public image address",
        ));
    }
    Ok(Box::new(kept.into_iter()))
}
// getaddrinfo runs on a blocking worker and cannot be cancelled by dropping
// its async waiter. Keep a global physical bound until that worker returns.
struct ResolverSlots {
    used: AtomicUsize,
    maximum: usize,
}
impl ResolverSlots {
    fn new(maximum: usize) -> Self {
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
                std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "public image resolver capacity",
                )
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
async fn resolve_with(
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
    .map_err(|_| std::io::Error::other("public image resolver did not return"))?
}
struct PublicResolver;
impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        static SLOTS: OnceLock<Arc<ResolverSlots>> = OnceLock::new();
        let slots = SLOTS
            .get_or_init(|| Arc::new(ResolverSlots::new(4)))
            .clone();
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
    Arc::new(PublicResolver)
}

/// The same picture at display size, when its CDN takes size parameters in
/// the address: LEGO, Shopify, Cloudinary, imgix, Contentful, Sanity and
/// Airbnb. None when the host is not one of them or the address already asks
/// for a display size.
pub fn display_variant(url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let path = parsed.path().to_owned();
    let edge = DISPLAY_EDGE.to_string();
    let sized = |parsed: &url::Url, keys: &[&str]| {
        parsed.query_pairs().any(|(key, value)| {
            keys.contains(&key.as_ref())
                && value
                    .parse::<u32>()
                    .is_ok_and(|size| size > 0 && size <= DISPLAY_EDGE)
        })
    };
    let set = |parsed: &mut url::Url, pairs: &[(&str, &str)]| {
        let kept: Vec<(String, String)> = parsed
            .query_pairs()
            .filter(|(key, _)| !pairs.iter().any(|(own, _)| own == key))
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let mut query = parsed.query_pairs_mut();
        query.clear();
        for (key, value) in &kept {
            query.append_pair(key, value);
        }
        for (key, value) in pairs {
            query.append_pair(key, value);
        }
    };
    if (host == "www.lego.com" || host == "lego.com") && path.starts_with("/cdn/") {
        if sized(&parsed, &["width", "height"]) {
            return None;
        }
        set(
            &mut parsed,
            &[
                ("format", "jpg"),
                ("fit", "bounds"),
                ("quality", "80"),
                ("width", &edge),
                ("height", &edge),
                ("dpr", "1"),
            ],
        );
    } else if host == "cdn.shopify.com" || path.starts_with("/cdn/shop/") {
        if sized(&parsed, &["width", "height"]) {
            return None;
        }
        set(&mut parsed, &[("width", &edge)]);
    } else if host.ends_with(".imgix.net") {
        if sized(&parsed, &["w", "h"]) {
            return None;
        }
        set(
            &mut parsed,
            &[("w", &edge), ("fit", "max"), ("auto", "format,compress")],
        );
    } else if host == "images.ctfassets.net" {
        if sized(&parsed, &["w", "h"]) {
            return None;
        }
        set(&mut parsed, &[("w", &edge), ("fm", "jpg"), ("q", "80")]);
    } else if host == "cdn.sanity.io" {
        if sized(&parsed, &["w", "h"]) {
            return None;
        }
        set(
            &mut parsed,
            &[("w", &edge), ("auto", "format"), ("fit", "max")],
        );
    } else if host.ends_with("muscache.com") && path.starts_with("/im/") {
        if sized(&parsed, &["im_w"]) {
            return None;
        }
        set(&mut parsed, &[("im_w", "1200")]);
    } else if host == "res.cloudinary.com" {
        let (head, tail) = path.split_once("/image/upload/")?;
        let first = tail.split('/').next().unwrap_or_default();
        if first
            .split(',')
            .any(|t| t.starts_with("w_") || t.starts_with("h_"))
        {
            return None;
        }
        parsed.set_path(&format!(
            "{head}/image/upload/c_limit,w_{edge},f_auto,q_auto/{tail}"
        ));
    } else {
        return None;
    }
    let variant = parsed.to_string();
    (variant != url && public_https(&parsed)).then_some(variant)
}

/// Fetches one public image without cookies, following at most three public
/// HTTPS redirects, and returns at most [`MAX_PUBLIC_IMAGE_BYTES`]. A CDN
/// that serves display sizes is asked for one first; the address as given is
/// the fallback.
pub async fn fetch_public_image(url: &str) -> Result<Vec<u8>, PublicAssetError> {
    let parsed = url::Url::parse(url).map_err(|_| PublicAssetError::InvalidUrl)?;
    if !public_https(&parsed) {
        return Err(PublicAssetError::InvalidUrl);
    }
    let client = public_client(Arc::new(PublicResolver))?;
    if let Some(variant) = display_variant(url).and_then(|v| url::Url::parse(&v).ok()) {
        if let Ok(bytes) = fetch_with(&client, variant).await {
            return Ok(bytes);
        }
    }
    fetch_with(&client, parsed).await
}

/// Exact anonymous record-source retrieval: no URL variants, credentials or redirects.
/// The caller owns the original short deadline and must drop this future on cancellation.
pub async fn fetch_public_record_image(raw: &str) -> Result<Vec<u8>, PublicAssetError> {
    let target =
        crate::ContextNavigationTarget::parse(raw).map_err(|_| PublicAssetError::InvalidUrl)?;
    if !public_https(target.as_url()) || !crate::semantic_wire::model_safe_public_url(&target) {
        return Err(PublicAssetError::InvalidUrl);
    }
    let client = public_client_builder(Arc::new(PublicResolver))
        .redirect(Policy::none())
        .build()
        .map_err(|_| PublicAssetError::Unavailable)?;
    fetch_bounded(
        &client,
        target.as_url().clone(),
        crate::MAX_PUBLIC_RECORD_IMAGE_BYTES,
    )
    .await
}

/// A client for one small anonymous JSON lookup: no redirects, a short
/// deadline, the same public-address resolver as image fetches.
pub(crate) fn lookup_client(timeout: Duration) -> Result<Client, PublicAssetError> {
    public_client_builder(Arc::new(PublicResolver))
        .redirect(Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
        .map_err(|_| PublicAssetError::Unavailable)
}

fn public_client(resolver: Arc<dyn reqwest::dns::Resolve>) -> Result<Client, PublicAssetError> {
    public_client_builder(resolver)
        .build()
        .map_err(|_| PublicAssetError::Unavailable)
}
fn public_client_builder(resolver: Arc<dyn reqwest::dns::Resolve>) -> reqwest::ClientBuilder {
    Client::builder()
        .no_proxy()
        .dns_resolver(resolver)
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS || !public_https(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .referer(false)
        .retry(reqwest::retry::never())
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .pool_max_idle_per_host(0)
        .user_agent(crate::provider_transport::PRODUCT_USER_AGENT)
}

async fn fetch_with(client: &Client, url: url::Url) -> Result<Vec<u8>, PublicAssetError> {
    fetch_bounded(client, url, MAX_PUBLIC_IMAGE_BYTES).await
}
async fn fetch_bounded(
    client: &Client,
    url: url::Url,
    limit: usize,
) -> Result<Vec<u8>, PublicAssetError> {
    let response = client
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "image/png,image/jpeg,image/webp,image/gif",
        )
        .send()
        .await
        .map_err(|_| PublicAssetError::Unavailable)?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(PublicAssetError::Unavailable);
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.starts_with("image/") {
        return Err(PublicAssetError::NotAnImage);
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(PublicAssetError::TooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|_| PublicAssetError::Unavailable)?
    {
        if bytes.len() + chunk.len() > limit {
            return Err(PublicAssetError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Err(PublicAssetError::NotAnImage);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn exact_record_fetch_refuses_private_and_secret_destinations_before_transport() {
        for url in [
            "http://images.example.com/a.png",
            "https://localhost/a.png",
            "https://127.0.0.1/a.png",
            "https://[::1]/a.png",
            "https://user:secret@images.example.com/a.png",
            "https://images.example.com/a.png?access_token=secret",
        ] {
            assert_eq!(
                fetch_public_record_image(url).await,
                Err(PublicAssetError::InvalidUrl)
            );
        }
    }

    use super::*;

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

    #[test]
    fn cdns_are_asked_for_a_display_sized_picture() {
        let lego =
            display_variant("https://www.lego.com/cdn/cs/set/assets/blt1/21066_Prod_en-gb.png")
                .unwrap();
        assert!(
            lego.contains("width=1600") && lego.contains("format=jpg"),
            "{lego}"
        );
        let resized = display_variant(
            "https://www.lego.com/cdn/cs/set/assets/blt1/21064.png?fit=bounds&format=jpg&quality=80&width=3000&height=3000&dpr=1",
        )
        .unwrap();
        assert!(
            resized.contains("width=1600") && !resized.contains("3000"),
            "{resized}"
        );
        assert_eq!(
            display_variant("https://www.lego.com/cdn/cs/set/assets/a.png?width=800&height=800"),
            None
        );
        assert!(
            display_variant("https://cdn.shopify.com/s/files/1/a.jpg?v=12")
                .unwrap()
                .ends_with("v=12&width=1600")
        );
        assert_eq!(
            display_variant("https://res.cloudinary.com/demo/image/upload/v1/sample.jpg").as_deref(),
            Some("https://res.cloudinary.com/demo/image/upload/c_limit,w_1600,f_auto,q_auto/v1/sample.jpg")
        );
        assert_eq!(
            display_variant("https://res.cloudinary.com/demo/image/upload/w_400/v1/sample.jpg"),
            None
        );
        assert!(
            display_variant("https://a0.muscache.com/im/pictures/x.jpeg")
                .unwrap()
                .ends_with("im_w=1200")
        );
        assert!(display_variant("https://example.imgix.net/a.jpg")
            .unwrap()
            .contains("w=1600"));
        assert_eq!(display_variant("https://example.com/a.jpg"), None);
    }

    #[test]
    fn only_public_https_origins_are_fetchable() {
        for ok in [
            "https://cdn.example/a.png",
            "https://93.184.216.34/logo.webp",
            "https://images.example.co.uk/x.jpg?w=200",
        ] {
            assert!(public_https(&url::Url::parse(ok).unwrap()), "{ok}");
        }
        for bad in [
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
        ] {
            assert!(!public_https(&url::Url::parse(bad).unwrap()), "{bad}");
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
            assert!(public_addresses([good, SocketAddr::new(ip, 443)].into_iter()).is_err());
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
        assert!(public_addresses(std::iter::empty()).is_err());
        assert_eq!(
            public_addresses(std::iter::repeat_n(good, 32))
                .unwrap()
                .count(),
            32
        );
        assert!(public_addresses(std::iter::repeat_n(good, 33)).is_err());
    }

    struct RebindingAnswer(SocketAddr);
    impl reqwest::dns::Resolve for RebindingAnswer {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            let address = self.0;
            Box::pin(async move { Ok(public_addresses([address].into_iter())?) })
        }
    }
    #[tokio::test]
    async fn a_public_hostname_resolving_to_loopback_never_connects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let client = public_client(Arc::new(RebindingAnswer(address))).unwrap();
        let url = url::Url::parse(&format!(
            "https://cdn.example.test:{}/photo.png",
            address.port()
        ))
        .unwrap();
        assert!(public_https(&url));
        assert_eq!(
            fetch_with(&client, url).await,
            Err(PublicAssetError::Unavailable)
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn inherited_proxy_cannot_bypass_public_address_validation() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.env_clear();
        // Winsock needs the system directory to load its service providers.
        #[cfg(windows)]
        child.env("SystemRoot", std::env::var_os("SystemRoot").unwrap());
        let status = child
            .env("HTTPS_PROXY", &proxy)
            .env("ALL_PROXY", &proxy)
            .args([
                "--exact",
                "public_asset::tests::a_public_hostname_resolving_to_loopback_never_connects",
                "--test-threads=1",
            ])
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
