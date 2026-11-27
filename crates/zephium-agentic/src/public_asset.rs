//! One bounded, cookie-free fetch of a public image for media admission.
//! The bytes are data for the store's own sniffing and decoding; nothing
//! here trusts a server's declared type beyond a coarse `image/*` gate.
//! Every fetch connects directly, and only while the shared route is direct:
//! it never goes around the person's proxy.
use reqwest::{dns::Resolve, redirect::Policy, Client};
use std::{future::Future, sync::Arc, time::Duration};
pub use zephium_public_fetch::guard::{public_address, public_https, public_resolver};
use zephium_public_fetch::SYSTEM_ROUTE_TIMEOUT;
pub use zephium_public_fetch::{PublicRoute, RouteSource};

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
    /// The route is proxied or not known: a direct fetch would go around it.
    RouteRefused,
}

/// How long a fetch started before the route is known waits for it: one
/// system read's timeout and a margin.
pub const ROUTE_SETTLE_WAIT: Duration =
    SYSTEM_ROUTE_TIMEOUT.saturating_add(Duration::from_millis(500));

async fn within(
    settled: impl Future<Output = ()>,
    route: impl FnOnce() -> PublicRoute,
    bound: Duration,
) -> PublicRoute {
    let _ = tokio::time::timeout(bound, settled).await;
    route()
}

pub(crate) async fn settled_route(routes: &RouteSource) -> PublicRoute {
    within(routes.settled(), || routes.route(), ROUTE_SETTLE_WAIT).await
}

pub(crate) async fn with_direct_route<T>(
    routes: &RouteSource,
    operation: impl Future<Output = Result<T, PublicAssetError>>,
) -> Result<T, PublicAssetError> {
    direct(settled_route(routes).await)?;
    routes
        .while_direct(operation)
        .await
        .map_err(|_| PublicAssetError::RouteRefused)?
}

fn direct(route: PublicRoute) -> Result<(), PublicAssetError> {
    match route {
        PublicRoute::Direct => Ok(()),
        PublicRoute::Proxied | PublicRoute::Unsupported => Err(PublicAssetError::RouteRefused),
    }
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
pub async fn fetch_public_image(
    routes: &RouteSource,
    url: &str,
) -> Result<Vec<u8>, PublicAssetError> {
    let parsed = image_target(url)?;
    with_direct_route(routes, fetch_image(public_resolver(), url, parsed)).await
}

fn image_target(url: &str) -> Result<url::Url, PublicAssetError> {
    let parsed = url::Url::parse(url).map_err(|_| PublicAssetError::InvalidUrl)?;
    if !public_https(&parsed) {
        return Err(PublicAssetError::InvalidUrl);
    }
    Ok(parsed)
}

async fn fetch_image(
    resolver: Arc<dyn Resolve>,
    url: &str,
    parsed: url::Url,
) -> Result<Vec<u8>, PublicAssetError> {
    let client = public_client(resolver)?;
    if let Some(variant) = display_variant(url).and_then(|v| url::Url::parse(&v).ok()) {
        if let Ok(bytes) = fetch_with(&client, variant).await {
            return Ok(bytes);
        }
    }
    fetch_with(&client, parsed).await
}

/// Exact anonymous record-source retrieval: no URL variants, credentials or redirects.
/// The caller owns the original short deadline and must drop this future on cancellation.
pub async fn fetch_public_record_image(
    routes: &RouteSource,
    raw: &str,
) -> Result<Vec<u8>, PublicAssetError> {
    let target = record_target(raw)?;
    with_direct_route(routes, fetch_record_image(public_resolver(), target)).await
}

fn record_target(raw: &str) -> Result<crate::ContextNavigationTarget, PublicAssetError> {
    let target =
        crate::ContextNavigationTarget::parse(raw).map_err(|_| PublicAssetError::InvalidUrl)?;
    if !public_https(target.as_url()) || !crate::semantic_wire::model_safe_public_url(&target) {
        return Err(PublicAssetError::InvalidUrl);
    }
    Ok(target)
}

async fn fetch_record_image(
    resolver: Arc<dyn Resolve>,
    target: crate::ContextNavigationTarget,
) -> Result<Vec<u8>, PublicAssetError> {
    let client = public_client_builder(resolver)
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
pub(crate) fn lookup_client(
    route: PublicRoute,
    timeout: Duration,
) -> Result<Client, PublicAssetError> {
    direct(route)?;
    public_client_builder(public_resolver())
        .redirect(Policy::none())
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
        .map_err(|_| PublicAssetError::Unavailable)
}

fn public_client(resolver: Arc<dyn Resolve>) -> Result<Client, PublicAssetError> {
    public_client_builder(resolver)
        .build()
        .map_err(|_| PublicAssetError::Unavailable)
}
fn permits_redirect(previous: usize, target: &url::Url) -> bool {
    previous <= MAX_REDIRECTS && public_https(target)
}

fn public_client_builder(resolver: Arc<dyn Resolve>) -> reqwest::ClientBuilder {
    Client::builder()
        .no_proxy()
        .dns_resolver(resolver)
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            // `previous()` starts with the original request, not a redirect.
            if permits_redirect(attempt.previous().len(), attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
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
    async fn image(
        route: impl Future<Output = PublicRoute>,
        resolver: Arc<dyn Resolve>,
        url: &str,
    ) -> Result<Vec<u8>, PublicAssetError> {
        let parsed = image_target(url)?;
        direct(route.await)?;
        fetch_image(resolver, url, parsed).await
    }

    async fn record_image(
        route: impl Future<Output = PublicRoute>,
        resolver: Arc<dyn Resolve>,
        raw: &str,
    ) -> Result<Vec<u8>, PublicAssetError> {
        let target = record_target(raw)?;
        direct(route.await)?;
        fetch_record_image(resolver, target).await
    }

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
                record_image(std::future::pending(), public_resolver(), url).await,
                Err(PublicAssetError::InvalidUrl)
            );
        }
    }

    use super::*;
    use std::future::ready;
    use std::net::SocketAddr;
    use zephium_public_fetch::guard::public_addresses;

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

    struct RebindingAnswer(SocketAddr);
    impl reqwest::dns::Resolve for RebindingAnswer {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            let address = self.0;
            Box::pin(async move { Ok(public_addresses([address].into_iter())?) })
        }
    }

    #[derive(Default)]
    struct CountingResolver(std::sync::atomic::AtomicUsize);
    impl reqwest::dns::Resolve for CountingResolver {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let loopback = SocketAddr::from(([127, 0, 0, 1], 443));
            Box::pin(async move { Ok(public_addresses([loopback].into_iter())?) })
        }
    }

    #[tokio::test]
    async fn every_fetch_refuses_unless_the_route_is_direct_before_any_lookup() {
        let image_url = "https://images.example.com/a.png";
        for route in [PublicRoute::Proxied, PublicRoute::Unsupported] {
            let resolver = Arc::new(CountingResolver::default());
            assert_eq!(
                image(ready(route), resolver.clone(), image_url).await,
                Err(PublicAssetError::RouteRefused)
            );
            assert_eq!(
                record_image(ready(route), resolver.clone(), image_url).await,
                Err(PublicAssetError::RouteRefused)
            );
            assert_eq!(
                lookup_client(route, Duration::from_secs(1)).err(),
                Some(PublicAssetError::RouteRefused)
            );
            assert_eq!(resolver.0.load(std::sync::atomic::Ordering::Acquire), 0);
        }
        let resolver = Arc::new(CountingResolver::default());
        assert_eq!(
            image(ready(PublicRoute::Direct), resolver.clone(), image_url).await,
            Err(PublicAssetError::Unavailable)
        );
        assert_eq!(
            record_image(ready(PublicRoute::Direct), resolver.clone(), image_url).await,
            Err(PublicAssetError::Unavailable)
        );
        assert_eq!(resolver.0.load(std::sync::atomic::Ordering::Acquire), 2);
        assert!(lookup_client(PublicRoute::Direct, Duration::from_secs(1)).is_ok());
        assert_eq!(
            image(
                std::future::pending(),
                resolver,
                "http://images.example.com/a.png"
            )
            .await,
            Err(PublicAssetError::InvalidUrl)
        );
    }

    /// A route that becomes known when `settle` is sent, as the source's
    /// first system read does.
    fn launching() -> (
        tokio::sync::oneshot::Sender<PublicRoute>,
        impl Future<Output = PublicRoute>,
    ) {
        let (settle, settled) = tokio::sync::oneshot::channel::<PublicRoute>();
        let known = Arc::new(std::sync::Mutex::new(PublicRoute::Unsupported));
        let route = {
            let known = known.clone();
            async move {
                within(
                    async {
                        if let Ok(route) = settled.await {
                            *known.lock().unwrap() = route;
                        }
                    },
                    || *known.lock().unwrap(),
                    Duration::from_millis(150),
                )
                .await
            }
        };
        (settle, route)
    }

    #[tokio::test]
    async fn a_fetch_started_before_the_route_is_known_waits_for_it_within_the_bound() {
        let image_url = "https://images.example.com/a.png";
        for (settles, expected, lookups) in [
            (Some(PublicRoute::Direct), PublicAssetError::Unavailable, 1),
            (
                Some(PublicRoute::Proxied),
                PublicAssetError::RouteRefused,
                0,
            ),
            (
                Some(PublicRoute::Unsupported),
                PublicAssetError::RouteRefused,
                0,
            ),
            (None, PublicAssetError::RouteRefused, 0),
        ] {
            for record in [false, true] {
                let resolver = Arc::new(CountingResolver::default());
                let (settle, route) = launching();
                let started = std::time::Instant::now();
                let fetch = {
                    let resolver = resolver.clone();
                    tokio::spawn(async move {
                        if record {
                            record_image(route, resolver, image_url).await
                        } else {
                            image(route, resolver, image_url).await
                        }
                    })
                };
                tokio::time::sleep(Duration::from_millis(30)).await;
                assert_eq!(resolver.0.load(std::sync::atomic::Ordering::Acquire), 0);
                let unsent = match settles {
                    Some(route) => {
                        settle.send(route).unwrap();
                        None
                    }
                    None => Some(settle),
                };
                assert_eq!(fetch.await.unwrap(), Err(expected), "{settles:?} {record}");
                drop(unsent);
                assert_eq!(
                    resolver.0.load(std::sync::atomic::Ordering::Acquire),
                    lookups,
                    "{settles:?} {record}"
                );
                if settles.is_some() {
                    assert!(started.elapsed() < Duration::from_millis(150));
                } else {
                    assert!(started.elapsed() >= Duration::from_millis(150));
                }
            }
        }
    }

    #[tokio::test]
    async fn once_settled_the_wait_completes_on_its_first_poll() {
        use futures_util::FutureExt as _;
        assert!(ROUTE_SETTLE_WAIT > SYSTEM_ROUTE_TIMEOUT);
        let routes = RouteSource::new(&tokio::runtime::Handle::current());
        routes.set_app_proxy(zephium_core::proxy::from_settings(
            "http",
            "proxy.example.com:3128",
        ));
        assert_eq!(routes.settled().now_or_never(), Some(()));
        assert_eq!(
            settled_route(&routes).now_or_never(),
            Some(PublicRoute::Proxied)
        );
    }

    #[tokio::test]
    async fn the_shared_route_source_gates_the_public_entry_points() {
        let routes = RouteSource::new(&tokio::runtime::Handle::current());
        let url = "https://images.example.com/a.png";
        assert_eq!(
            within(
                routes.settled(),
                || routes.route(),
                Duration::from_millis(50)
            )
            .await,
            PublicRoute::Unsupported
        );
        routes.set_app_proxy(zephium_core::proxy::from_settings(
            "http",
            "proxy.example.com:3128",
        ));
        assert_eq!(routes.route(), PublicRoute::Proxied);
        assert_eq!(
            fetch_public_image(&routes, url).await,
            Err(PublicAssetError::RouteRefused)
        );
        assert_eq!(
            fetch_public_record_image(&routes, url).await,
            Err(PublicAssetError::RouteRefused)
        );
        assert_eq!(
            crate::youtube_oembed::fetch(&routes, "https://youtu.be/dQw4w9WgXcQ").await,
            Err(PublicAssetError::RouteRefused)
        );
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

#[cfg(test)]
#[test]
fn public_redirect_policy_allows_three_hops_and_refuses_the_fourth() {
    let target = url::Url::parse("https://example.com/asset.png").unwrap();
    assert!(permits_redirect(MAX_REDIRECTS, &target));
    assert!(!permits_redirect(MAX_REDIRECTS + 1, &target));
    let private = url::Url::parse("https://127.0.0.1/asset.png").unwrap();
    assert!(!permits_redirect(1, &private));
}
