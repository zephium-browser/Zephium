use std::{
    fmt,
    future::{poll_fn, Future},
    sync::{Arc, Weak},
    task::Poll,
    time::Duration,
};

use tokio::sync::{mpsc, watch};
use tokio::time::Instant;
use zephium_core::proxy::ProxyServer;

use crate::limits::{SYSTEM_ROUTE_MIN_GAP, SYSTEM_ROUTE_REFRESH, SYSTEM_ROUTE_TIMEOUT};
use crate::route::{self, PublicRoute, Route};

pub(crate) type SystemRead = Arc<dyn Fn() -> Route + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cadence {
    pub(crate) refresh: Duration,
    pub(crate) timeout: Duration,
    pub(crate) min_gap: Duration,
}

impl Cadence {
    pub(crate) const PRODUCT: Self = Self {
        refresh: SYSTEM_ROUTE_REFRESH,
        timeout: SYSTEM_ROUTE_TIMEOUT,
        min_gap: SYSTEM_ROUTE_MIN_GAP,
    };
}

struct State {
    /// `None` until the app's setting is reported.
    app: Option<Option<ProxyServer>>,
    /// `None` until the first system read returns or times out.
    system: Option<Route>,
    reads: u64,
    generation: u64,
    effective: Route,
}

impl State {
    fn effective(&self) -> Route {
        match &self.app {
            None => Route::Unsupported,
            Some(manual) => route::route(manual.as_ref(), || {
                self.system.clone().unwrap_or(Route::Unsupported)
            }),
        }
    }

    fn known(&self) -> bool {
        match &self.app {
            None => false,
            Some(Some(_)) => true,
            Some(None) => self.system.is_some(),
        }
    }
}

struct Shared {
    state: watch::Sender<State>,
    class: watch::Sender<PublicRoute>,
    wake: mpsc::Sender<()>,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut State)) {
        self.state.send_modify(|state| {
            change(state);
            let effective = state.effective();
            if effective != state.effective {
                state.effective = effective;
                state.generation = state.generation.wrapping_add(1);
                self.class.send_replace(PublicRoute::of(&state.effective));
            }
        });
    }
}

/// The one route every anonymous reader takes: the proxy chosen in Zephium,
/// else the operating system's, read on a blocking thread and refreshed.
/// Unsupported until the app's setting is reported and, in system mode, the
/// first system read has settled. Cheap to clone; the refresh task stops
/// once every clone is dropped.
#[derive(Clone)]
pub struct RouteSource {
    shared: Arc<Shared>,
}

impl RouteSource {
    /// Schedules the first system read at once, on `runtime`'s blocking pool.
    pub fn new(runtime: &tokio::runtime::Handle) -> Self {
        Self::start(
            runtime,
            Arc::new(crate::platform::system_route),
            Cadence::PRODUCT,
            None,
        )
    }

    fn start(
        runtime: &tokio::runtime::Handle,
        read: SystemRead,
        cadence: Cadence,
        primed: Option<Route>,
    ) -> Self {
        let read_now = primed.is_none();
        let (wake, woken) = mpsc::channel(1);
        let shared = Arc::new(Shared {
            state: watch::Sender::new(State {
                app: None,
                system: primed,
                reads: 0,
                generation: 0,
                effective: Route::Unsupported,
            }),
            class: watch::Sender::new(PublicRoute::Unsupported),
            wake,
        });
        runtime.spawn(refresh(
            Arc::downgrade(&shared),
            woken,
            read,
            cadence,
            read_now,
        ));
        Self { shared }
    }

    /// The proxy the person chose in Zephium, or `None` for the system's.
    pub fn set_app_proxy(&self, proxy: Option<ProxyServer>) {
        self.shared.update(|state| state.app = Some(proxy));
    }

    /// The cached route; never blocks and never asks the operating system.
    pub fn route(&self) -> PublicRoute {
        PublicRoute::of(&self.shared.state.borrow().effective)
    }

    /// Marked changed whenever the route changes, a different proxy of the
    /// same class included.
    pub fn changed(&self) -> watch::Receiver<PublicRoute> {
        self.shared.class.subscribe()
    }

    /// Resolves once the route is known, for a caller that reads at once.
    pub async fn settled(&self) {
        let mut state = self.shared.state.subscribe();
        let _ = state.wait_for(State::known).await;
    }

    pub(crate) fn current(&self) -> (u64, Route) {
        let state = self.shared.state.borrow();
        (state.generation, state.effective.clone())
    }

    /// Runs one anonymous direct operation while its admitted route stays current.
    /// The operation must not access or change this route source during a poll.
    pub async fn while_direct<F: Future>(
        &self,
        operation: F,
    ) -> Result<F::Output, crate::ReadRefusal> {
        let mut changed = self.changed();
        let (generation, route) = self.current();
        if route != Route::Direct {
            return Err(crate::ReadRefusal::ProxyUnsupported);
        }
        let mut operation = std::pin::pin!(operation);
        let admitted =
            poll_fn(
                |cx| match self.at_generation(generation, || operation.as_mut().poll(cx)) {
                    Ok(polled) => polled.map(Ok),
                    Err(refusal) => Poll::Ready(Err(refusal)),
                },
            );
        let output = tokio::select! {
            biased;
            Ok(()) = changed.changed() => return Err(crate::ReadRefusal::RouteChanged),
            result = admitted => result?,
        };
        self.at_generation(generation, || output)
    }

    pub(crate) fn at_generation<T>(
        &self,
        generation: u64,
        action: impl FnOnce() -> T,
    ) -> Result<T, crate::ReadRefusal> {
        let state = self.shared.state.borrow();
        if state.generation != generation {
            return Err(crate::ReadRefusal::RouteChanged);
        }
        // The watch borrow holds route publication until this synchronous admission returns.
        Ok(action())
    }

    /// Asks for a system read soon; requests while one is due coalesce.
    pub(crate) fn request_refresh(&self) {
        let _ = self.shared.wake.try_send(());
    }
}

impl fmt::Debug for RouteSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RouteSource")
            .field(&self.route())
            .finish()
    }
}

fn apply(shared: &Weak<Shared>, change: impl FnOnce(&mut State)) -> bool {
    match shared.upgrade() {
        Some(shared) => {
            shared.update(change);
            true
        }
        None => false,
    }
}

async fn system_read(shared: &Weak<Shared>, read: &SystemRead, timeout: Duration) -> bool {
    let read = read.clone();
    let mut job = tokio::task::spawn_blocking(move || read());
    let outcome = match tokio::time::timeout(timeout, &mut job).await {
        Ok(outcome) => outcome,
        Err(_) => {
            if !apply(shared, |state| state.system = Some(Route::Unsupported)) {
                return false;
            }
            job.await
        }
    };
    let route = outcome.unwrap_or(Route::Unsupported);
    apply(shared, |state| {
        state.system = Some(route);
        state.reads = state.reads.wrapping_add(1);
    })
}

async fn refresh(
    shared: Weak<Shared>,
    mut woken: mpsc::Receiver<()>,
    read: SystemRead,
    cadence: Cadence,
    mut read_now: bool,
) {
    let mut started = Instant::now();
    loop {
        if read_now {
            started = Instant::now();
            if !system_read(&shared, &read, cadence.timeout).await {
                return;
            }
        }
        read_now = true;
        tokio::select! {
            () = tokio::time::sleep_until(started + cadence.refresh) => {}
            woke = woken.recv() => {
                if woke.is_none() {
                    return;
                }
                let gap = started + cadence.min_gap;
                loop {
                    tokio::select! {
                        () = tokio::time::sleep_until(gap) => break,
                        woke = woken.recv() => if woke.is_none() {
                            return;
                        },
                    }
                }
            }
        }
    }
}

#[cfg(test)]
impl RouteSource {
    pub(crate) const FIXTURE: Cadence = Cadence {
        refresh: Duration::from_secs(3600),
        timeout: Duration::from_secs(5),
        min_gap: Duration::ZERO,
    };

    /// Read once on the caller's thread, then refreshed as in the product.
    pub(crate) fn primed(read: SystemRead, cadence: Cadence) -> Self {
        let route = read();
        let source = Self::start(
            &tokio::runtime::Handle::current(),
            read,
            cadence,
            Some(route),
        );
        source.set_app_proxy(None);
        source
    }

    pub(crate) fn unread(read: SystemRead, cadence: Cadence) -> Self {
        Self::start(&tokio::runtime::Handle::current(), read, cadence, None)
    }

    pub(crate) fn reads(&self) -> u64 {
        self.shared.state.borrow().reads
    }

    /// Asks for a system read and waits until one more has returned.
    pub(crate) async fn reread(&self) {
        let mut state = self.shared.state.subscribe();
        let target = state.borrow().reads.wrapping_add(1);
        self.request_refresh();
        let _ = state.wait_for(|state| state.reads >= target).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use zephium_core::proxy::from_settings;

    fn cadence(refresh: u64, timeout: u64, min_gap: u64) -> Cadence {
        Cadence {
            refresh: Duration::from_millis(refresh),
            timeout: Duration::from_millis(timeout),
            min_gap: Duration::from_millis(min_gap),
        }
    }

    fn proxy() -> Option<ProxyServer> {
        from_settings("http", "proxy.example.com:3128")
    }

    struct Gate(Mutex<Option<std::sync::mpsc::Receiver<Route>>>);

    impl Gate {
        fn read(&self) -> Route {
            self.0
                .lock()
                .unwrap()
                .take()
                .and_then(|answer| answer.recv().ok())
                .unwrap_or(Route::Direct)
        }
    }

    fn gated() -> (SystemRead, std::sync::mpsc::Sender<Route>) {
        let (answer, receive) = std::sync::mpsc::channel();
        let gate = Gate(Mutex::new(Some(receive)));
        (Arc::new(move || gate.read()), answer)
    }

    async fn eventually(mut condition: impl FnMut() -> bool) {
        for _ in 0..400 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition never held");
    }

    #[test]
    fn the_product_cadence_is_the_named_limits() {
        assert_eq!(
            Cadence::PRODUCT,
            Cadence {
                refresh: SYSTEM_ROUTE_REFRESH,
                timeout: SYSTEM_ROUTE_TIMEOUT,
                min_gap: SYSTEM_ROUTE_MIN_GAP,
            }
        );
        assert!(SYSTEM_ROUTE_TIMEOUT < SYSTEM_ROUTE_MIN_GAP);
        assert!(SYSTEM_ROUTE_MIN_GAP < SYSTEM_ROUTE_REFRESH);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_route_is_unsupported_until_the_setting_and_the_system_are_known() {
        let (read, answer) = gated();
        let source = RouteSource::unread(read, cadence(3_600_000, 5_000, 0));
        assert_eq!(source.route(), PublicRoute::Unsupported);
        source.set_app_proxy(None);
        assert_eq!(source.route(), PublicRoute::Unsupported);
        let settled = tokio::time::timeout(Duration::from_millis(50), source.settled()).await;
        assert!(settled.is_err());
        answer.send(Route::Direct).unwrap();
        source.settled().await;
        assert_eq!(source.route(), PublicRoute::Direct);

        let fresh = RouteSource::unread(Arc::new(|| Route::Direct), RouteSource::FIXTURE);
        fresh.reread().await;
        assert_eq!(fresh.route(), PublicRoute::Unsupported);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_apps_proxy_wins_without_waiting_for_a_system_read() {
        let (read, answer) = gated();
        let source = RouteSource::unread(read, cadence(3_600_000, 5_000, 0));
        source.set_app_proxy(proxy());
        assert_eq!(source.route(), PublicRoute::Proxied);
        tokio::time::timeout(Duration::from_millis(500), source.settled())
            .await
            .unwrap();
        assert_eq!(source.reads(), 0);
        source.set_app_proxy(from_settings("socks5", "127.0.0.1:9050"));
        assert_eq!(source.route(), PublicRoute::Unsupported);
        answer.send(Route::Direct).unwrap();
        eventually(|| source.reads() == 1).await;
        assert_eq!(source.route(), PublicRoute::Unsupported);
        source.set_app_proxy(None);
        assert_eq!(source.route(), PublicRoute::Direct);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_system_change_at_a_refresh_is_announced_once() {
        let system = Arc::new(Mutex::new(Route::Direct));
        let seen = system.clone();
        let source = RouteSource::primed(
            Arc::new(move || seen.lock().unwrap().clone()),
            RouteSource::FIXTURE,
        );
        let mut changed = source.changed();
        source.reread().await;
        assert!(!changed.has_changed().unwrap());
        *system.lock().unwrap() = Route::Http {
            host: "proxy.example.com".into(),
            port: 3128,
        };
        source.reread().await;
        assert!(changed.has_changed().unwrap());
        assert_eq!(*changed.borrow_and_update(), PublicRoute::Proxied);
        source.reread().await;
        assert!(!changed.has_changed().unwrap());
        *system.lock().unwrap() = Route::Http {
            host: "other.example.com".into(),
            port: 3128,
        };
        source.reread().await;
        assert!(changed.has_changed().unwrap());
        assert_eq!(*changed.borrow_and_update(), PublicRoute::Proxied);
        source.set_app_proxy(proxy());
        assert!(changed.has_changed().unwrap());
        changed.borrow_and_update();
        source.set_app_proxy(proxy());
        assert!(!changed.has_changed().unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_slow_system_read_never_blocks_the_route() {
        let calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        let source = RouteSource::primed(
            Arc::new(move || {
                if counted.fetch_add(1, Ordering::AcqRel) > 0 {
                    std::thread::sleep(Duration::from_millis(400));
                }
                Route::Direct
            }),
            RouteSource::FIXTURE,
        );
        source.request_refresh();
        eventually(|| calls.load(Ordering::Acquire) == 2).await;
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(source.route(), PublicRoute::Direct);
        }
        assert!(started.elapsed() < Duration::from_millis(100));
        assert_eq!(source.reads(), 0);
        eventually(|| source.reads() == 1).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_read_past_the_timeout_is_unsupported_until_it_returns() {
        let calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        let source = RouteSource::primed(
            Arc::new(move || {
                if counted.fetch_add(1, Ordering::AcqRel) > 0 {
                    std::thread::sleep(Duration::from_millis(300));
                }
                Route::Direct
            }),
            cadence(3_600_000, 50, 0),
        );
        let mut changed = source.changed();
        assert_eq!(source.route(), PublicRoute::Direct);
        source.request_refresh();
        eventually(|| source.route() == PublicRoute::Unsupported).await;
        assert!(changed.has_changed().unwrap());
        changed.borrow_and_update();
        assert_eq!(source.reads(), 0);
        eventually(|| source.route() == PublicRoute::Direct).await;
        assert_eq!(source.reads(), 1);
        assert!(changed.has_changed().unwrap());

        let (read, answer) = gated();
        let first = RouteSource::unread(read, cadence(3_600_000, 50, 0));
        first.set_app_proxy(None);
        first.settled().await;
        assert_eq!(first.route(), PublicRoute::Unsupported);
        answer.send(Route::Direct).unwrap();
        eventually(|| first.route() == PublicRoute::Direct).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn asked_reads_coalesce_and_wait_for_the_minimum_gap() {
        let calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        let source = RouteSource::primed(
            Arc::new(move || {
                counted.fetch_add(1, Ordering::AcqRel);
                Route::Direct
            }),
            cadence(3_600_000, 5_000, 300),
        );
        for _ in 0..5 {
            source.request_refresh();
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(source.reads(), 0);
        eventually(|| source.reads() == 1).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(source.reads(), 1);
        assert_eq!(calls.load(Ordering::Acquire), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_system_is_read_again_at_every_refresh() {
        let source = RouteSource::primed(Arc::new(|| Route::Direct), cadence(40, 5_000, 0));
        eventually(|| source.reads() >= 3).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_every_handle_stops_the_refresh_task() {
        let calls = Arc::new(AtomicU64::new(0));
        let counted = calls.clone();
        let read: SystemRead = Arc::new(move || {
            counted.fetch_add(1, Ordering::AcqRel);
            Route::Direct
        });
        let source = RouteSource::primed(read.clone(), cadence(20, 5_000, 0));
        let clone = source.clone();
        eventually(|| source.reads() >= 2).await;
        drop(source);
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(Arc::strong_count(&read) > 1);
        drop(clone);
        eventually(|| Arc::strong_count(&read) == 1).await;
        let after = calls.load(Ordering::Acquire);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(calls.load(Ordering::Acquire), after);
    }
}

#[cfg(test)]
#[tokio::test]
async fn an_old_generation_is_refused_before_admission_even_if_its_class_returns() {
    let routes = RouteSource::primed(Arc::new(|| Route::Direct), RouteSource::FIXTURE);
    let (direct, _) = routes.current();
    routes.set_app_proxy(zephium_core::proxy::from_settings(
        "http",
        "proxy.example.com:3128",
    ));
    assert_eq!(
        routes.at_generation(direct, || panic!("stale request must never dispatch")),
        Err(crate::ReadRefusal::RouteChanged)
    );
    routes.set_app_proxy(None);
    assert_eq!(routes.route(), PublicRoute::Direct);
    assert_eq!(
        routes.at_generation(direct, || panic!("stale request must never dispatch")),
        Err(crate::ReadRefusal::RouteChanged)
    );
    assert_eq!(routes.at_generation(routes.current().0, || 7), Ok(7));
}

#[cfg(test)]
#[tokio::test]
async fn a_direct_operation_cannot_start_its_fallback_after_the_route_changes() {
    let routes = RouteSource::primed(Arc::new(|| Route::Direct), RouteSource::FIXTURE);
    let (reached, first) = tokio::sync::oneshot::channel();
    let (resume, release) = tokio::sync::oneshot::channel();
    let operation = tokio::spawn({
        let routes = routes.clone();
        async move {
            routes
                .while_direct(async {
                    reached.send(()).unwrap();
                    assert!(release.await.is_ok());
                    panic!("the fallback must not run on a stale route");
                })
                .await
        }
    });
    first.await.unwrap();
    routes.set_app_proxy(zephium_core::proxy::from_settings(
        "http",
        "proxy.example.com:3128",
    ));
    let _ = resume.send(());
    assert_eq!(
        operation.await.unwrap(),
        Err(crate::ReadRefusal::RouteChanged)
    );
    routes.set_app_proxy(None);
    assert_eq!(routes.while_direct(async { 7 }).await, Ok(7));
}
