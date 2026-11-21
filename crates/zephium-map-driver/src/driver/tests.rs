use super::*;
use crate::{
    io::{Clock, Fetcher, ResultsPage, ResultsSource, SystemClock},
    memo::{Memo, MemoryMemo},
    wire::{Failure as WireFailure, Network, Outcome, Target},
    Reading,
};
use std::sync::atomic::AtomicUsize;
use zephium_map::{
    session::{FetchResult, FETCH_TIMEOUT, MAX_ELAPSED},
    PlaceKey,
};

const RESULT: &str =
    "<main><article><h2><a href='https://acme.com/'>Acme workshop</a></h2></article></main>";
const BOTH: &str = "<main><article><h2><a href='https://acme.com/'>Acme workshop</a></h2></article><article><h2><a href='https://other.com/'>Other works</a></h2></article></main>";
const NAV: &str =
    "<title>Acme Workshop</title><nav><a href='/pricing'>Pricing</a><a href='/news'>News</a></nav>";

#[derive(Clone)]
enum Plan {
    Page(&'static str, String),
    Hang,
    Fail(Failure),
}

#[derive(Default)]
struct Seen {
    active: BTreeMap<String, usize>,
    max_active: usize,
    max_host: usize,
    fetches: Vec<String>,
}

struct Fake {
    results: Plan,
    pages: BTreeMap<String, Plan>,
    delay: Duration,
    seen: Mutex<Seen>,
    calls: AtomicUsize,
    memo_calls: AtomicUsize,
    memo: MemoryMemo,
    clock: SystemClock,
}

impl Fake {
    fn new(results: Plan) -> Self {
        Self {
            results,
            pages: BTreeMap::new(),
            delay: Duration::from_millis(5),
            seen: Mutex::default(),
            calls: AtomicUsize::new(0),
            memo_calls: AtomicUsize::new(0),
            memo: MemoryMemo::new(),
            clock: SystemClock::new(),
        }
    }

    fn site(mut self, host: &str, pages: &[(&str, Plan)]) -> Self {
        for (path, plan) in pages {
            self.pages.insert(format!("{host}{path}"), plan.clone());
        }
        self
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap()
    }
}

fn html(body: &str) -> Plan {
    Plan::Page("text/html", body.to_owned())
}

fn acme(fake: Fake) -> Fake {
    fake.site(
        "acme.com",
        &[
            ("/", html(NAV)),
            (
                "/pricing",
                html("<h2 id='tiers'>Tiers</h2><h2 id='teams'>Teams</h2>"),
            ),
            ("/news", html("<h2 id='launch'>Launch</h2>")),
        ],
    )
}

struct Active(Arc<Fake>, String);

impl Drop for Active {
    fn drop(&mut self) {
        if let Some(n) = self.0.seen().active.get_mut(&self.1) {
            *n -= 1;
        }
    }
}

impl ResultsSource for Fake {
    fn load(&self, url: Url) -> Reading<Result<ResultsPage, Failure>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let plan = self.results.clone();
        Box::pin(async move {
            match plan {
                Plan::Page(_, html) => Ok(ResultsPage {
                    html,
                    url,
                    truncated: false,
                }),
                Plan::Hang => std::future::pending().await,
                Plan::Fail(failure) => Err(failure),
            }
        })
    }
}

struct Reader(Arc<Fake>);

impl Fetcher for Reader {
    fn fetch(&self, request: FetchRequest) -> Reading<Result<FetchResult, Failure>> {
        let fake = Arc::clone(&self.0);
        fake.calls.fetch_add(1, Ordering::SeqCst);
        let host = request.url.host_str().unwrap_or_default().to_owned();
        let page = format!("{host}{}", request.url.path());
        let plan = fake
            .pages
            .get(&page)
            .cloned()
            .unwrap_or(Plan::Fail(Failure::Missing));
        Box::pin(async move {
            {
                let mut seen = fake.seen();
                seen.fetches.push(page);
                *seen.active.entry(host.clone()).or_default() += 1;
                let total = seen.active.values().sum();
                let here = seen.active[&host];
                seen.max_host = seen.max_host.max(here);
                seen.max_active = seen.max_active.max(total);
            }
            let _active = Active(Arc::clone(&fake), host);
            tokio::time::sleep(fake.delay).await;
            match plan {
                Plan::Page(media, body) => Ok(FetchResult {
                    final_url: request.url,
                    content_type: media.into(),
                    charset: None,
                    bytes: body.into_bytes(),
                    truncated: false,
                }),
                Plan::Hang => std::future::pending().await,
                Plan::Fail(failure) => Err(failure),
            }
        })
    }
}

impl Memo for Fake {
    fn lookup(&self, profile: &str, site: &PlaceKey) -> Option<Vec<u8>> {
        self.memo_calls.fetch_add(1, Ordering::SeqCst);
        self.memo.lookup(profile, site)
    }

    fn remember(&self, profile: &str, site: &PlaceKey, memo: Vec<u8>) {
        self.memo_calls.fetch_add(1, Ordering::SeqCst);
        self.memo.remember(profile, site, memo)
    }
}

impl Clock for Fake {
    fn now(&self) -> Duration {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.clock.now()
    }

    fn unix(&self) -> u64 {
        self.calls.fetch_add(1, Ordering::SeqCst);
        1_000_000
    }
}

fn io(fake: &Arc<Fake>) -> Io {
    Io {
        results: fake.clone(),
        fetch: Arc::new(Reader(Arc::clone(fake))),
        memo: fake.clone(),
        clock: fake.clone(),
    }
}

fn request(query: &str) -> MapRequest {
    MapRequest {
        query: query.into(),
        engine: SearchEngine::DuckDuckGo,
        session_id: 7,
        profile: "profile".into(),
        private: false,
    }
}

fn start(fake: &Arc<Fake>, request: MapRequest) -> (Driver, mpsc::Receiver<Change>) {
    Driver::start(request, io(fake), &Handle::current()).unwrap()
}

async fn rest(rx: &mut mpsc::Receiver<Change>) -> Vec<Change> {
    let mut out = Vec::new();
    while let Some(change) = rx.recv().await {
        out.push(change);
    }
    out
}

fn checked(changes: &[Change], first: u32) {
    for (n, change) in changes.iter().enumerate() {
        assert_eq!(change.seq(), first + n as u32);
    }
    let ends = changes
        .iter()
        .filter(|c| matches!(c, Change::End { .. }))
        .count();
    assert_eq!(ends, 1);
    assert!(matches!(changes.last(), Some(Change::End { .. })));
}

fn outcome(changes: &[Change]) -> (Outcome, u32) {
    match changes.last() {
        Some(Change::End {
            outcome,
            elapsed_ms,
            ..
        }) => (*outcome, *elapsed_ms),
        _ => panic!("no end"),
    }
}

fn last_for(changes: &[Change], slot: u8) -> Option<&Change> {
    changes.iter().rev().find(|c| {
        matches!(c, Change::Place { slot: s, .. } | Change::PlaceFailed { slot: s, .. } if *s == slot)
    })
}

fn noisy_results() -> Plan {
    html(&format!(
        "<html><head><script>{}</script><style>{}</style></head><body>{RESULT}</body></html>",
        "var s = '</div>';".repeat(page::MAX_RESULTS_PAGE_BYTES / 8),
        "p{}".repeat(1000)
    ))
}

async fn until(fake: &Fake, path: &str) {
    while !fake.seen().fetches.iter().any(|f| f == path) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn active(fake: &Fake) -> usize {
    fake.seen().active.values().sum()
}

#[tokio::test(start_paused = true)]
async fn a_place_streams_first_layer_then_places_then_one_end() {
    let fake = Arc::new(acme(Fake::new(noisy_results())));
    let (driver, mut rx) = start(&fake, request("acme"));
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    assert!(matches!(changes[0], Change::FirstLayer { organic: 1, .. }));
    let places = &changes[1..changes.len() - 1];
    assert!(!places.is_empty());
    assert!(places
        .iter()
        .all(|c| matches!(c, Change::Place { slot: 0, .. })));
    assert!(matches!(
        places.last(),
        Some(Change::Place { complete: true, .. })
    ));
    assert!(places[..places.len() - 1].iter().all(|c| matches!(
        c,
        Change::Place {
            complete: false,
            ..
        }
    )));
    assert_eq!(outcome(&changes).0, Outcome::Done);
    let Some(Change::Place { place, .. }) = places.last() else {
        unreachable!()
    };
    let pricing = place
        .sections
        .iter()
        .find(|e| e.label == "Pricing")
        .unwrap();
    let tiers = pricing
        .children
        .iter()
        .find(|e| e.label == "Tiers")
        .unwrap();
    assert_eq!(
        driver
            .target(
                0,
                &Target::Entry {
                    id: tiers.id.clone()
                }
            )
            .map(|u| u.to_string()),
        Some("https://acme.com/pricing#tiers".into())
    );
    assert_eq!(
        driver.target(0, &Target::Site).map(|u| u.to_string()),
        Some("https://acme.com/".into())
    );
    let unknown = Target::Entry {
        id: "0000000000000000".into(),
    };
    let social = Target::Social {
        network: Network::GitHub,
    };
    for (slot, target) in [
        (0, &unknown),
        (1, &Target::Site),
        (2, &Target::Site),
        (0, &social),
    ] {
        assert_eq!(driver.target(slot, target), None);
    }
    let counts = driver.counts();
    assert_eq!(
        counts,
        Counts {
            fetches: counts.fetches,
            bytes: counts.bytes,
            ..Counts::default()
        }
    );
    let fetched = fake.seen().fetches.len();
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(fake.seen().fetches.len(), fetched);
    assert_eq!(active(&fake), 0);
}

#[tokio::test(start_paused = true)]
async fn a_comparison_side_without_a_site_ends_failed() {
    let fake = Arc::new(acme(Fake::new(html(RESULT))));
    let (driver, mut rx) = start(&fake, request("acme vs other"));
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    assert!(matches!(
        last_for(&changes, 0),
        Some(Change::Place { complete: true, .. })
    ));
    assert!(matches!(
        last_for(&changes, 1),
        Some(Change::PlaceFailed {
            failure: WireFailure::Unavailable,
            ..
        })
    ));
    assert!(!changes.iter().any(|c| matches!(c, Change::Compare { .. })));
    assert_eq!(driver.counts().anonymous_unavailable, 1);
    assert_eq!(driver.target(1, &Target::Site), None);
}

#[tokio::test(start_paused = true)]
async fn both_sides_compare_and_reads_stay_under_the_ceilings() {
    let many = |prefix: &str| {
        let links: String = (0..12)
            .map(|n| format!("<a href='/{prefix}{n}'>Part {n}</a>"))
            .collect();
        html(&format!(
            "<title>T</title><nav>{links}</nav><footer><a href='/pricing'>Pricing</a></footer>"
        ))
    };
    let mut fake = Fake::new(html(BOTH))
        .site(
            "acme.com",
            &[("/", many("a")), ("/pricing", html("<h2>Plans</h2>"))],
        )
        .site(
            "other.com",
            &[("/", many("o")), ("/pricing", html("<h2>Plans</h2>"))],
        );
    fake.delay = Duration::from_millis(200);
    let fake = Arc::new(fake);
    let (driver, mut rx) = start(&fake, request("acme vs other"));
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    assert!(changes.iter().any(|c| matches!(c, Change::Compare { .. })));
    let seen = fake.seen();
    assert!(seen.fetches.len() > MAX_CONCURRENT);
    assert!(seen.max_active <= MAX_CONCURRENT && seen.max_active > 1);
    assert!(seen.max_host <= MAX_HOST_CONCURRENT);
    assert_eq!(driver.counts().over_ceiling, 0);
}

#[tokio::test(start_paused = true)]
async fn the_driver_refuses_reads_over_the_ceilings() {
    let fake = Arc::new(Fake::new(Plan::Hang).site("a.com", &[("/", Plan::Hang)]));
    let (session, _) = Session::start(Config::default(), "acme").unwrap();
    let (tx, _rx) = mpsc::channel(CHANGE_BUFFER);
    let mut run = Run {
        session: Some(session),
        io: io(&fake),
        profile: String::new(),
        private: false,
        tx,
        shared: Arc::default(),
        reads: JoinSet::new(),
        tasks: HashMap::new(),
        hosts: BTreeMap::new(),
        immediate: VecDeque::new(),
        seq: 0,
        started: Duration::ZERO,
        cancelled: watch::channel(false).1,
        cancelling: false,
        unsent: None,
    };
    for (n, host) in ["a.com", "a.com", "a.com", "b.com", "c.com", "d.com"]
        .into_iter()
        .enumerate()
    {
        run.fetch(
            Id {
                session: 0,
                sequence: n as u32,
            },
            FetchRequest {
                url: Url::parse(&format!("https://{host}/")).unwrap(),
                expect: zephium_map::session::Expect::Html,
                max_bytes: 1,
                timeout: Duration::from_secs(1),
            },
        );
    }
    assert_eq!(run.hosts.len(), MAX_CONCURRENT);
    assert_eq!(run.reads.len(), MAX_CONCURRENT);
    assert_eq!(run.shared.over_ceiling.load(Ordering::SeqCst), 2);
    assert_eq!(run.immediate.len(), 2);
    assert!(run.immediate.iter().all(|i| matches!(
        i,
        Input::Fetch {
            result: Err(Failure::Limit),
            ..
        }
    )));
    let done = run.reads.join_next_with_id().await.unwrap();
    assert!(matches!(
        run.completed(done),
        Some(Input::Fetch {
            result: Err(Failure::Missing),
            ..
        })
    ));
    assert_eq!(run.hosts.len(), MAX_CONCURRENT - 1);
}

#[tokio::test]
async fn two_fetch_answers_are_counted_and_the_counts_survive_cancel() {
    const ROBOTS: &str = "User-agent: *\nAllow: /\n";
    let fake = Arc::new(Fake::new(html(RESULT)).site(
        "acme.com",
        &[
            ("/robots.txt", Plan::Page("text/plain", ROBOTS.into())),
            ("/", html(NAV)),
        ],
    ));
    // The first layer fills the buffer so the first place blocks further effects.
    let (driver, mut rx) =
        Driver::start_buffered(request("acme"), io(&fake), &Handle::current(), 1).unwrap();
    let bytes = (ROBOTS.len() + NAV.len()) as u64;
    tokio::time::timeout(FETCH_TIMEOUT, async {
        while driver.counts().bytes < bytes {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(driver.counts().fetches, 2);
    assert_eq!(driver.counts().bytes, bytes);
    driver.cancel();
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    assert_eq!(outcome(&changes).0, Outcome::Cancelled);
    assert_eq!(driver.counts().fetches, 2);
    assert_eq!(driver.counts().bytes, bytes);
}

#[tokio::test(start_paused = true)]
async fn cancel_ends_once_and_starts_nothing_after() {
    let fake = Arc::new(Fake::new(html(RESULT)).site("acme.com", &[("/", Plan::Hang)]));
    let (driver, mut rx) = start(&fake, request("acme"));
    let first = rx.recv().await.unwrap();
    assert!(matches!(first, Change::FirstLayer { seq: 0, .. }));
    until(&fake, "acme.com/").await;
    assert_eq!(active(&fake), 1);
    let fetched = fake.seen().fetches.len();
    driver.cancel();
    driver.cancel();
    let changes = rest(&mut rx).await;
    checked(&changes, 1);
    assert_eq!(changes.len(), 1);
    assert_eq!(outcome(&changes).0, Outcome::Cancelled);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(active(&fake), 0);
    assert_eq!(fake.seen().fetches.len(), fetched);
    driver.cancel();
}

fn stalled(fake: &Arc<Fake>) -> (Driver, mpsc::Receiver<Change>) {
    Driver::start_buffered(request("acme"), io(fake), &Handle::current(), 1).unwrap()
}

#[tokio::test(start_paused = true)]
async fn cancel_does_not_wait_behind_a_stalled_receiver() {
    let fake = Arc::new(acme(Fake::new(html(RESULT))));
    let (driver, mut rx) = stalled(&fake);
    until(&fake, "acme.com/").await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let fetched = fake.seen().fetches.len();
    assert_eq!(fetched, 2);
    driver.cancel();
    tokio::time::sleep(Duration::from_secs(30)).await;
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    assert_eq!(changes.len(), 2);
    assert!(matches!(changes[0], Change::FirstLayer { .. }));
    assert_eq!(outcome(&changes).0, Outcome::Cancelled);
    assert_eq!(fake.seen().fetches.len(), fetched);
    assert_eq!(active(&fake), 0);
    assert_eq!(driver.target(0, &Target::Site), None);
}

#[tokio::test(start_paused = true)]
async fn a_receiver_that_never_reads_is_released_by_dropping_the_driver() {
    let fake = Arc::new(acme(Fake::new(html(RESULT))));
    let (driver, mut rx) = stalled(&fake);
    until(&fake, "acme.com/").await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    driver.cancel();
    tokio::time::sleep(Duration::from_secs(1)).await;
    drop(driver);
    let changes = rest(&mut rx).await;
    assert_eq!(changes.len(), 1);
    assert!(matches!(changes[0], Change::FirstLayer { .. }));
}

#[tokio::test(start_paused = true)]
async fn dropping_the_driver_or_the_receiver_stops_reads_silently() {
    for drop_driver in [true, false] {
        let fake = Arc::new(Fake::new(html(RESULT)).site("acme.com", &[("/", Plan::Hang)]));
        let (driver, mut rx) = start(&fake, request("acme"));
        rx.recv().await.unwrap();
        until(&fake, "acme.com/").await;
        let fetched = fake.seen().fetches.len();
        if drop_driver {
            drop(driver);
            assert!(rx.recv().await.is_none());
        } else {
            drop(rx);
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert_eq!(active(&fake), 0);
            drop(driver);
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
        assert_eq!(active(&fake), 0);
        assert_eq!(fake.seen().fetches.len(), fetched);
    }
}

#[tokio::test(start_paused = true)]
async fn a_hanging_results_page_is_ended_by_the_elapsed_budget() {
    let fake = Arc::new(Fake::new(Plan::Hang));
    let (_driver, mut rx) = start(&fake, request("acme"));
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    let (outcome, elapsed) = outcome(&changes);
    assert_eq!(
        outcome,
        Outcome::Failed {
            failure: WireFailure::Timeout
        }
    );
    let budget = MAX_ELAPSED.as_millis() as u32;
    assert!((budget..=budget + TICK.as_millis() as u32).contains(&elapsed));
    assert!(fake.seen().fetches.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_hanging_fetch_is_ended_by_its_own_timeout() {
    let fake =
        Arc::new(acme(Fake::new(html(RESULT))).site("acme.com", &[("/robots.txt", Plan::Hang)]));
    let (_driver, mut rx) = start(&fake, request("acme"));
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    let (_, elapsed) = outcome(&changes);
    assert!(elapsed >= FETCH_TIMEOUT.as_millis() as u32);
    assert!(elapsed < MAX_ELAPSED.as_millis() as u32);
    assert_eq!(active(&fake), 0);
}

#[tokio::test(start_paused = true)]
async fn a_results_failure_ends_the_map() {
    let fake = Arc::new(Fake::new(Plan::Fail(Failure::Consent)));
    let (_driver, mut rx) = start(&fake, request("acme"));
    let changes = rest(&mut rx).await;
    checked(&changes, 0);
    assert_eq!(
        outcome(&changes).0,
        Outcome::Failed {
            failure: WireFailure::Consent
        }
    );
    let over = "a".repeat(page::MAX_RESULTS_PAGE_BYTES + 1);
    let fake = Arc::new(Fake::new(html(&over)));
    let (_driver, mut rx) = start(&fake, request("acme"));
    assert_eq!(
        outcome(&rest(&mut rx).await).0,
        Outcome::Failed {
            failure: WireFailure::Limit
        }
    );
}

#[tokio::test(start_paused = true)]
async fn non_map_queries_are_refused_before_any_io() {
    let fake = Arc::new(acme(Fake::new(html(RESULT))));
    for (query, engine, refusal) in [
        ("", SearchEngine::DuckDuckGo, StartRefusal::Invalid),
        (
            "san francisco time",
            SearchEngine::DuckDuckGo,
            StartRefusal::NotAMap,
        ),
        (
            "how to reset the timer",
            SearchEngine::Google,
            StartRefusal::NotAMap,
        ),
        (
            "acme",
            SearchEngine::Custom,
            StartRefusal::UnsupportedEngine,
        ),
    ] {
        let result = Driver::start(
            MapRequest {
                engine,
                ..request(query)
            },
            io(&fake),
            &Handle::current(),
        );
        assert_eq!(result.err(), Some(refusal), "{query}");
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
    assert_eq!(fake.memo_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_private_map_never_reads_or_writes_the_memo() {
    let fake = Arc::new(acme(Fake::new(html(RESULT))));
    let (_driver, mut rx) = start(&fake, request("acme"));
    rest(&mut rx).await;
    let site = PlaceKey::from_url(&Url::parse("https://acme.com/").unwrap()).unwrap();
    assert!(fake.memo.lookup("profile", &site).is_some());
    let fetched = fake.seen().fetches.len();
    let (_driver, mut rx) = start(&fake, request("acme"));
    let remembered = rest(&mut rx).await;
    checked(&remembered, 0);
    assert!(matches!(
        last_for(&remembered, 0),
        Some(Change::Place {
            complete: true,
            stale: false,
            ..
        })
    ));
    assert_eq!(fake.seen().fetches.len(), fetched);
    let calls = fake.memo_calls.load(Ordering::SeqCst);
    let (_driver, mut rx) = start(
        &fake,
        MapRequest {
            private: true,
            ..request("acme")
        },
    );
    let private = rest(&mut rx).await;
    checked(&private, 0);
    assert!(fake.seen().fetches.len() > fetched);
    assert_eq!(fake.memo_calls.load(Ordering::SeqCst), calls);
}
