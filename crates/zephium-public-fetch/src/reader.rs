use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    net::ToSocketAddrs,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, PoisonError,
    },
    task::Poll,
    time::Instant,
};

use reqwest::header::{
    HeaderMap, ACCEPT, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, LOCATION,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::content::{self, Admission};
use crate::decode::{Coding, Pipeline};
use crate::guard::{self, ResolverSlots};
use crate::limits::{
    ReaderLimits, MAX_LOCATION_BYTES, MAX_REDIRECTS, RESOLVER_SLOTS_PER_READ,
    SESSION_IDLE_PER_HOST, SESSION_IDLE_TIMEOUT, SNIFF_BYTES,
};
use crate::probe::{ConnectTiming, ReadProbe};
use crate::route::Route;
use crate::source::RouteSource;
use crate::types::{
    Expect, PublicBody, ReadRefusal, ReadRequest, SizeLimit, StatusClass, StatusRefusal,
    TimeoutPhase, Timings,
};

/// The one identity every anonymous Zephium request presents.
pub const USER_AGENT: &str = "Zephium-Agent-Browser/0.1";
const ACCEPTED_CODINGS: &str = "gzip, br";

pub(crate) enum Policy {
    Product,
    #[cfg(test)]
    Fixture(Arc<crate::fixture::Fixture>),
}

impl Policy {
    fn admits(&self, url: &url::Url) -> bool {
        match self {
            Policy::Product => guard::public_target(url),
            #[cfg(test)]
            Policy::Fixture(fixture) => fixture.admits(url),
        }
    }

    fn https_only(&self) -> bool {
        match self {
            Policy::Product => true,
            #[cfg(test)]
            Policy::Fixture(_) => false,
        }
    }

    fn resolver(&self, slots: &Arc<ResolverSlots>) -> Arc<dyn reqwest::dns::Resolve> {
        match self {
            Policy::Product => guard::vetting_resolver(slots.clone()),
            #[cfg(test)]
            Policy::Fixture(fixture) => fixture.resolver(),
        }
    }
}

#[derive(Default)]
struct Counters {
    reads: AtomicU64,
    completed: AtomicU64,
    truncated: AtomicU64,
    wire_bytes: AtomicU64,
    decoded_bytes: AtomicU64,
    connections: AtomicU64,
    refusals: [AtomicU64; ReadRefusal::ALL.len()],
}

/// Aggregate counts for a bench; classes, counts and sizes only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CounterSnapshot {
    pub reads: u64,
    pub completed: u64,
    pub truncated: u64,
    pub wire_bytes: u64,
    pub decoded_bytes: u64,
    pub connections: u64,
    refusals: [u64; ReadRefusal::ALL.len()],
}

impl CounterSnapshot {
    pub fn refused(&self, class: ReadRefusal) -> u64 {
        self.refusals[class.index()]
    }

    pub fn refusals(&self) -> u64 {
        self.refusals.iter().sum()
    }
}

struct Inner {
    limits: ReaderLimits,
    policy: Policy,
    global: Arc<Semaphore>,
    hosts: Mutex<HashMap<String, Arc<Semaphore>>>,
    resolver_slots: Arc<ResolverSlots>,
    counters: Counters,
    routes: RouteSource,
}

/// The only way the map touches the network: anonymous, bounded reads of
/// public HTTPS pages. Cheap to clone; clones share limits and counters.
#[derive(Clone)]
pub struct PublicReader {
    inner: Arc<Inner>,
}

impl PublicReader {
    /// Reads take `routes`' route; a change refuses reads in flight.
    pub fn new(limits: ReaderLimits, routes: RouteSource) -> Self {
        Self::with_policy(limits, Policy::Product, routes)
    }

    #[cfg(test)]
    pub(crate) fn fixture(limits: ReaderLimits, fixture: Arc<crate::fixture::Fixture>) -> Self {
        let system = fixture.clone();
        let routes = RouteSource::primed(
            Arc::new(move || system.system_route()),
            RouteSource::FIXTURE,
        );
        Self::fixture_with(limits, fixture, routes)
    }

    #[cfg(test)]
    pub(crate) fn fixture_with(
        limits: ReaderLimits,
        fixture: Arc<crate::fixture::Fixture>,
        routes: RouteSource,
    ) -> Self {
        Self::with_policy(limits, Policy::Fixture(fixture), routes)
    }

    #[cfg(test)]
    pub(crate) fn routes(&self) -> &RouteSource {
        &self.inner.routes
    }

    fn with_policy(limits: ReaderLimits, policy: Policy, routes: RouteSource) -> Self {
        let limits = limits.clamped();
        Self {
            inner: Arc::new(Inner {
                limits,
                policy,
                global: Arc::new(Semaphore::new(limits.max_concurrent)),
                hosts: Mutex::new(HashMap::new()),
                resolver_slots: Arc::new(ResolverSlots::new(
                    limits.max_concurrent * RESOLVER_SLOTS_PER_READ,
                )),
                counters: Counters::default(),
                routes,
            }),
        }
    }

    pub fn limits(&self) -> ReaderLimits {
        self.inner.limits
    }

    /// A short-lived session that may reuse connections between its own
    /// reads; nothing is shared with any other session.
    pub fn session(&self) -> ReadSession {
        ReadSession {
            reader: self.clone(),
            client: Mutex::new(None),
        }
    }

    pub async fn read(&self, request: ReadRequest) -> Result<PublicBody, ReadRefusal> {
        self.session().read(request).await
    }

    pub fn counters(&self) -> CounterSnapshot {
        let counters = &self.inner.counters;
        let load = |counter: &AtomicU64| counter.load(Ordering::Acquire);
        let mut refusals = [0; ReadRefusal::ALL.len()];
        for (slot, counter) in refusals.iter_mut().zip(&counters.refusals) {
            *slot = load(counter);
        }
        CounterSnapshot {
            reads: load(&counters.reads),
            completed: load(&counters.completed),
            truncated: load(&counters.truncated),
            wire_bytes: load(&counters.wire_bytes),
            decoded_bytes: load(&counters.decoded_bytes),
            connections: load(&counters.connections),
            refusals,
        }
    }
}

struct SessionClient {
    client: reqwest::Client,
    generation: u64,
}

pub struct ReadSession {
    reader: PublicReader,
    client: Mutex<Option<SessionClient>>,
}

struct HostPermit {
    inner: Arc<Inner>,
    key: String,
    semaphore: Option<Arc<Semaphore>>,
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for HostPermit {
    fn drop(&mut self) {
        drop(self.permit.take());
        drop(self.semaphore.take());
        let mut hosts = self
            .inner
            .hosts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if hosts
            .get(&self.key)
            .is_some_and(|semaphore| Arc::strong_count(semaphore) == 1)
        {
            hosts.remove(&self.key);
        }
    }
}

fn host_key(url: &url::Url) -> String {
    format!(
        "{}:{}",
        url.host_str().unwrap_or_default().trim_end_matches('.'),
        url.port_or_known_default().unwrap_or_default()
    )
}

async fn host_permit(inner: &Arc<Inner>, url: &url::Url) -> Result<HostPermit, ReadRefusal> {
    let key = host_key(url);
    let semaphore = inner
        .hosts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Semaphore::new(inner.limits.max_per_host)))
        .clone();
    let mut permit = HostPermit {
        inner: inner.clone(),
        key,
        semaphore: Some(semaphore.clone()),
        permit: None,
    };
    permit.permit = Some(
        semaphore
            .acquire_owned()
            .await
            .map_err(|_| ReadRefusal::Transport)?,
    );
    Ok(permit)
}

struct ProxyOnly {
    host: String,
    slots: Arc<ResolverSlots>,
}

impl reqwest::dns::Resolve for ProxyOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        // Through a proxy the only name this client may look up is the
        // proxy's own; a destination resolved here would mean a direct path.
        let own = name.as_str().eq_ignore_ascii_case(&self.host);
        let host = self.host.clone();
        let slots = self.slots.clone();
        Box::pin(async move {
            if !own {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    guard::NonPublicAnswer,
                )
                .into());
            }
            Ok(guard::resolve_with(slots, move || {
                let addresses: Vec<_> = (host.as_str(), 0)
                    .to_socket_addrs()?
                    .take(guard::MAX_RESOLVED_ADDRESSES)
                    .collect();
                Ok(Box::new(addresses.into_iter()))
            })
            .await?)
        })
    }
}

fn build_client(inner: &Inner, route: &Route) -> Result<reqwest::Client, ReadRefusal> {
    let limits = &inner.limits;
    let builder = reqwest::Client::builder()
        .no_proxy()
        .https_only(inner.policy.https_only())
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(limits.connect_timeout)
        .read_timeout(limits.first_byte_timeout)
        .timeout(limits.max_duration)
        .pool_max_idle_per_host(SESSION_IDLE_PER_HOST)
        .pool_idle_timeout(SESSION_IDLE_TIMEOUT)
        .http2_max_header_list_size(u32::try_from(limits.max_header_bytes).unwrap_or(u32::MAX))
        .user_agent(USER_AGENT)
        .connector_layer(ConnectTiming);
    let builder = match route {
        Route::Direct => builder.dns_resolver(inner.policy.resolver(&inner.resolver_slots)),
        Route::Http { host, .. } => {
            let proxy = route
                .proxy_url()
                .and_then(|url| reqwest::Proxy::all(url).ok())
                .ok_or(ReadRefusal::ProxyUnsupported)?;
            builder.proxy(proxy).dns_resolver(Arc::new(ProxyOnly {
                host: host.clone(),
                slots: inner.resolver_slots.clone(),
            }))
        }
        Route::Unsupported => return Err(ReadRefusal::ProxyUnsupported),
    };
    builder.build().map_err(|_| ReadRefusal::Transport)
}

fn accept(expect: Expect) -> &'static str {
    match expect {
        Expect::Html => "text/html,application/xhtml+xml;q=0.9",
        Expect::Xml => "application/xml,text/xml;q=0.9,application/gzip;q=0.8",
        Expect::Text => "text/plain,text/markdown;q=0.9",
    }
}

fn header_bytes(headers: &HeaderMap) -> usize {
    headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.len() + 4)
        .sum()
}

fn single(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Result<Option<&[u8]>, ()> {
    let mut values = headers.get_all(name).iter();
    match (values.next(), values.next()) {
        (None, _) => Ok(None),
        (Some(value), None) => Ok(Some(value.as_bytes())),
        (Some(_), Some(_)) => Err(()),
    }
}

fn coding(headers: &HeaderMap) -> Result<Coding, ReadRefusal> {
    let value = single(headers, CONTENT_ENCODING).map_err(|_| ReadRefusal::Encoding)?;
    let Some(value) = value else {
        return Ok(Coding::Identity);
    };
    let value = std::str::from_utf8(value).map_err(|_| ReadRefusal::Encoding)?;
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "identity" => Ok(Coding::Identity),
        "gzip" | "x-gzip" => Ok(Coding::Gzip),
        "br" => Ok(Coding::Brotli),
        _ => Err(ReadRefusal::Encoding),
    }
}

fn challenged(headers: &HeaderMap) -> bool {
    let marked = |name: &str, values: &[&[u8]]| {
        headers.get(name).is_some_and(|value| {
            values
                .iter()
                .any(|v| value.as_bytes().eq_ignore_ascii_case(v))
        })
    };
    marked("cf-mitigated", &[b"challenge"])
        || marked("x-amzn-waf-action", &[b"challenge", b"captcha"])
}

fn status_refusal(status: u16) -> StatusRefusal {
    match status {
        401 | 403 => StatusRefusal::Denied,
        429 => StatusRefusal::RateLimited,
        400..=499 => StatusRefusal::Client,
        500..=599 => StatusRefusal::Server,
        _ => StatusRefusal::Unexpected,
    }
}

fn location(headers: &HeaderMap, current: &url::Url) -> Result<url::Url, ReadRefusal> {
    let value = single(headers, LOCATION)
        .ok()
        .flatten()
        .filter(|value| value.len() <= MAX_LOCATION_BYTES)
        .ok_or(ReadRefusal::RedirectRefused)?;
    let text = std::str::from_utf8(value).map_err(|_| ReadRefusal::RedirectRefused)?;
    let mut next = current
        .join(text)
        .map_err(|_| ReadRefusal::RedirectRefused)?;
    next.set_fragment(None);
    Ok(next)
}

fn send_refusal(error: &reqwest::Error, probe: &ReadProbe) -> ReadRefusal {
    if guard::refused_answer(error) {
        ReadRefusal::DnsRefused
    } else if error.is_timeout() && error.is_connect() {
        ReadRefusal::Timeout(TimeoutPhase::Connect)
    } else if error.is_timeout() {
        probe.timeout()
    } else {
        ReadRefusal::Transport
    }
}

impl ReadSession {
    fn client(&self, generation: u64, route: &Route) -> Result<reqwest::Client, ReadRefusal> {
        let inner = &self.reader.inner;
        let mut slot = self.client.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(current) = slot
            .as_ref()
            .filter(|current| current.generation == generation)
        {
            return Ok(current.client.clone());
        }
        *slot = None;
        let client = build_client(inner, route)?;
        *slot = Some(SessionClient {
            client: client.clone(),
            generation,
        });
        Ok(client)
    }

    pub async fn read(&self, request: ReadRequest) -> Result<PublicBody, ReadRefusal> {
        let inner = &self.reader.inner;
        inner.counters.reads.fetch_add(1, Ordering::AcqRel);
        let started = Instant::now();
        let probe = Arc::new(ReadProbe::default());
        let mut changed = inner.routes.changed();
        let admission = inner.routes.current();
        let generation = admission.0;
        let deadline = request.deadline.min(started + inner.limits.max_duration);
        let outcome = if deadline <= started {
            Err(ReadRefusal::Timeout(TimeoutPhase::Queue))
        } else {
            tokio::select! {
                biased;
                Ok(()) = changed.changed() => Err(ReadRefusal::RouteChanged),
                result = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    self.attempt(&request, &probe, started, admission),
                ) => result.unwrap_or_else(|_| Err(probe.timeout())),
            }
        };
        let outcome = inner.routes.at_generation(generation, || outcome).flatten();
        if matches!(outcome, Err(ReadRefusal::Timeout(TimeoutPhase::Connect))) {
            inner.routes.request_refresh();
        }
        let counters = &inner.counters;
        counters
            .connections
            .fetch_add(probe.opened(), Ordering::AcqRel);
        counters
            .wire_bytes
            .fetch_add(probe.wire(), Ordering::AcqRel);
        match &outcome {
            Ok(body) => {
                counters.completed.fetch_add(1, Ordering::AcqRel);
                counters
                    .truncated
                    .fetch_add(u64::from(body.truncated), Ordering::AcqRel);
                counters
                    .decoded_bytes
                    .fetch_add(body.bytes.len() as u64, Ordering::AcqRel);
            }
            Err(refusal) => {
                counters.refusals[refusal.index()].fetch_add(1, Ordering::AcqRel);
            }
        }
        outcome
    }

    async fn attempt(
        &self,
        request: &ReadRequest,
        probe: &Arc<ReadProbe>,
        started: Instant,
        admission: (u64, Route),
    ) -> Result<PublicBody, ReadRefusal> {
        let inner = &self.reader.inner;
        let limits = inner.limits;
        let mut current = request.url.clone();
        current.set_fragment(None);
        if !inner.policy.admits(&current) {
            return Err(ReadRefusal::NotPublicHttps);
        }
        let (generation, route) = admission;
        let client = self.client(generation, &route)?;
        let cap = request.max_bytes.clamp(1, limits.max_decoded_bytes);
        let mut host = host_permit(inner, &current).await?;
        let mut global = inner
            .global
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ReadRefusal::Transport)?;
        let queued = started.elapsed();
        let mut hops = 0u8;
        let mut visited = vec![current.clone()];
        let (response, first_byte) = loop {
            probe.first_byte();
            let request = client
                .get(current.clone())
                .header(ACCEPT, accept(request.expect))
                .header(ACCEPT_ENCODING, ACCEPTED_CODINGS);
            let mut sending = std::pin::pin!(async { request.send().await });
            let admitted = poll_fn(|cx| {
                match inner
                    .routes
                    .at_generation(generation, || sending.as_mut().poll(cx))
                {
                    Ok(polled) => polled.map(Ok),
                    Err(refusal) => Poll::Ready(Err(refusal)),
                }
            });
            let response = probe
                .scope(tokio::time::timeout(
                    limits.connect_timeout + limits.first_byte_timeout,
                    admitted,
                ))
                .await
                .map_err(|_| probe.timeout())??
                .map_err(|error| {
                    let refusal = send_refusal(&error, probe);
                    if error.is_connect() && refusal == ReadRefusal::Transport {
                        inner.routes.request_refresh();
                    }
                    refusal
                })?;
            let first_byte = started.elapsed();
            let headers = response.headers();
            let size = header_bytes(headers);
            probe.add_wire(size as u64);
            if size > limits.max_header_bytes {
                return Err(ReadRefusal::TooLarge(SizeLimit::Headers));
            }
            if challenged(headers) {
                return Err(ReadRefusal::Status(StatusRefusal::Challenge));
            }
            match response.status().as_u16() {
                200 | 203 => break (response, first_byte),
                301 | 302 | 303 | 307 | 308 => {
                    let next = location(headers, &current)?;
                    if !inner.policy.admits(&next) {
                        return Err(ReadRefusal::RedirectRefused);
                    }
                    hops += 1;
                    if hops > MAX_REDIRECTS || visited.contains(&next) {
                        return Err(ReadRefusal::TooManyRedirects);
                    }
                    drop(response);
                    if host_key(&next) != host_key(&current) {
                        drop(global);
                        drop(host);
                        probe.queue();
                        host = host_permit(inner, &next).await?;
                        global = inner
                            .global
                            .clone()
                            .acquire_owned()
                            .await
                            .map_err(|_| ReadRefusal::Transport)?;
                    }
                    visited.push(next.clone());
                    current = next;
                }
                status => return Err(ReadRefusal::Status(status_refusal(status))),
            }
        };
        let status = if response.status().as_u16() == 200 {
            StatusClass::Ok
        } else {
            StatusClass::NonAuthoritative
        };
        let declared = content::parse_content_type(
            single(response.headers(), CONTENT_TYPE).map_err(|_| ReadRefusal::WrongContentType)?,
        )?;
        let admission: Admission = content::admit(request.expect, declared.as_ref())?;
        let mut pipeline = Pipeline::new(coding(response.headers())?, admission.wrapped, cap);
        probe.body();
        let mut response = response;
        let mut body_wire = 0u64;
        let mut checked = false;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            if error.is_timeout() {
                ReadRefusal::Timeout(TimeoutPhase::Body)
            } else {
                ReadRefusal::Transport
            }
        })? {
            body_wire = body_wire.saturating_add(chunk.len() as u64);
            probe.add_wire(chunk.len() as u64);
            if body_wire > limits.max_wire_bytes {
                return Err(ReadRefusal::TooLarge(SizeLimit::Wire));
            }
            let more = pipeline.push(&chunk).map_err(|_| ReadRefusal::Encoding)?;
            if !checked && pipeline.decoded().len() >= SNIFF_BYTES {
                content::check_body(request.expect, admission, pipeline.decoded())?;
                checked = true;
            }
            if !more {
                break;
            }
        }
        drop(response);
        let (mut bytes, mut truncated) = pipeline.finish().map_err(|_| ReadRefusal::Encoding)?;
        if !checked {
            content::check_body(request.expect, admission, &bytes)?;
        }
        if request.max_bytes == 0 && !bytes.is_empty() {
            bytes.clear();
            truncated = true;
        }
        let (media_type, charset) = declared
            .map(|declared| (Some(declared.essence), declared.charset))
            .unwrap_or_default();
        Ok(PublicBody {
            final_url: current,
            hops,
            status,
            media_type,
            charset,
            bytes,
            truncated,
            wire_bytes: probe.wire(),
            timings: Timings {
                queued,
                connect: probe.connect(),
                first_byte,
                total: started.elapsed(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::dns::Resolve;

    #[test]
    fn equivalent_dns_names_share_one_host_key() {
        let bare = url::Url::parse("https://example.com/").unwrap();
        let rooted = url::Url::parse("https://example.com./").unwrap();
        assert_eq!(host_key(&bare), host_key(&rooted));
    }

    #[tokio::test]
    async fn proxy_dns_refuses_when_the_physical_resolver_slot_is_busy() {
        let slots = Arc::new(ResolverSlots::new(1));
        let proxy = ProxyOnly {
            host: "127.0.0.1".into(),
            slots: slots.clone(),
        };
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let worker = tokio::spawn(guard::resolve_with(slots, move || {
            started.send(()).unwrap();
            released.recv().unwrap();
            Ok(Box::new(std::iter::empty()))
        }));
        ready.await.unwrap();
        let refused = proxy.resolve("127.0.0.1".parse().unwrap()).await;
        release.send(()).unwrap();
        assert_eq!(worker.await.unwrap().unwrap().count(), 0);
        assert!(
            matches!(refused, Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|io| io.kind() == std::io::ErrorKind::WouldBlock))
        );
    }
}
