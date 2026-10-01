//! Opt-in physical WebView2 qualification. Entire module is test-only.
//! No CDP, WebMessage bridge, credentials, or ordinary browser profile.
use super::*;
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::num::NonZeroIsize;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use serde_json::{json, Value};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2WebResourceRequestedEventArgs2, COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use wry::{WebView, WebViewBuilder, WebViewBuilderExtWindows};
use zephium_blocker::{PolicySource, SourceFormat, SourceId, StaticPolicyCatalog, WorkerBlocker};
use zephium_core::blocker::{BlockerConfig, ContentPolicyGeneration};
use zephium_core::ports::blocker::{
    BlockerCompileOutcome, BlockerCompiler, BlockerDispatch, BlockerShutdownOutcome,
};

const SAMPLE_LIMIT: usize = 200_000;
thread_local! {
    static SAMPLES: RefCell<Option<Samples>> = const { RefCell::new(None) };
}
struct Samples {
    callback: Vec<(&'static str, u64)>,
    matcher: Vec<u64>,
    dropped: u64,
}
pub(super) struct CallbackSample {
    started: Option<Instant>,
    outcome: &'static str,
}
impl CallbackSample {
    pub(super) fn start() -> Self {
        Self {
            started: SAMPLES.with(|s| s.borrow().is_some().then(Instant::now)),
            outcome: "native_fail_open",
        }
    }
    pub(super) fn outcome(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
    pub(super) fn finish(self) {
        let Some(started) = self.started else { return };
        let ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        SAMPLES.with(|samples| {
            if let Some(samples) = samples.borrow_mut().as_mut() {
                if samples.callback.len() < SAMPLE_LIMIT {
                    samples.callback.push((self.outcome, ns));
                } else {
                    samples.dropped += 1;
                }
            }
        });
    }
}
pub(super) fn record_matcher(elapsed: Duration) {
    SAMPLES.with(|samples| {
        if let Some(samples) = samples.borrow_mut().as_mut() {
            if samples.matcher.len() < SAMPLE_LIMIT {
                samples
                    .matcher
                    .push(elapsed.as_nanos().min(u64::MAX as u128) as u64);
            }
        }
    });
}
fn summary(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    let percentile = |p: usize| {
        values
            .get((values.len() * p).div_ceil(100).saturating_sub(1))
            .copied()
    };
    json!({"count":values.len(), "p50_ns":percentile(50), "p95_ns":percentile(95),
        "p99_ns":percentile(99), "max_ns":values.last()})
}

struct Host(HWND);
impl Host {
    fn new() -> Self {
        // SAFETY: process-lifetime class and UI-apartment window procedure.
        unsafe {
            let module = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: windows_core::w!("ZephiumProtectionQualification"),
                ..Default::default()
            };
            RegisterClassW(&class);
            Self(
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class.lpszClassName,
                    windows_core::w!("Zephium Protection Qualification"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    1200,
                    800,
                    None,
                    None,
                    Some(module.into()),
                    None,
                )
                .unwrap(),
            )
        }
    }
}
impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: borrowed handle cannot outlive this owner.
        Ok(
            unsafe {
                WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(raw)))
            },
        )
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wp, lp) }
}
fn pump() {
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        // Bounded load/evaluation wait. Idle sampling below uses its entire
        // remaining deadline, without this timeout or a fixture-server poll.
        MsgWaitForMultipleObjectsEx(None, 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}
fn until(mut condition: impl FnMut() -> bool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !condition() {
        assert!(Instant::now() < deadline, "native qualification timed out");
        pump();
    }
}
fn evaluate(view: &WebView, script: &str) -> Value {
    let (tx, rx) = mpsc::channel();
    view.evaluate_script_with_callback(script, move |value| {
        let _ = tx.send(serde_json::from_str(&value).unwrap_or(Value::Null));
    })
    .unwrap();
    let mut result = None;
    until(
        || {
            result = rx.try_recv().ok();
            result.is_some()
        },
        10,
    );
    result.unwrap()
}

struct Fixture {
    origin: String,
    stop: Arc<AtomicBool>,
    blocked_hits: Arc<AtomicU64>,
    allowed_hits: Arc<AtomicU64>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Fixture {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let blocked_hits = Arc::new(AtomicU64::new(0));
        let allowed_hits = Arc::new(AtomicU64::new(0));
        let (signal, blocks, allows) = (stop.clone(), blocked_hits.clone(), allowed_hits.clone());
        let worker = std::thread::spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut input = [0; 8192];
                let count = stream.read(&mut input).unwrap_or(0);
                let request = String::from_utf8_lossy(&input[..count]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let (mime, body) = if path.starts_with("/ads/cbr.js") {
                    blocks.fetch_add(1, Ordering::Relaxed);
                    (
                        "application/javascript",
                        "globalThis.adExecuted = (globalThis.adExecuted || 0) + 1;".to_owned(),
                    )
                } else if path.starts_with("/useful.js") {
                    allows.fetch_add(1, Ordering::Relaxed);
                    (
                        "application/javascript",
                        "globalThis.usefulExecuted = (globalThis.usefulExecuted || 0) + 1;"
                            .to_owned(),
                    )
                } else if path.starts_with("/strict") {
                    ("text/html", "<!doctype html><title>Strict CSP fixture</title><h1>Useful content</h1><div id='first'>First target</div><div id='second'>Second target</div><script>globalThis.pageScriptRan=true</script>".to_owned())
                } else if path.starts_with("/worker.js") {
                    ("application/javascript", "fetch('/ads/cbr.js?dedicated').then(r=>postMessage(r.status)).catch(()=>postMessage(-1));".to_owned())
                } else if path.starts_with("/shared.js") {
                    ("application/javascript", "onconnect=e=>fetch('/ads/cbr.js?shared').then(r=>e.ports[0].postMessage(r.status));".to_owned())
                } else if path.starts_with("/service.js") {
                    ("application/javascript", "self.addEventListener('install',e=>e.waitUntil(fetch('/ads/cbr.js?service')));".to_owned())
                } else if path.starts_with("/subdocument") {
                    (
                        "text/html",
                        "<!doctype html><title>Child</title>Child".to_owned(),
                    )
                } else if path.starts_with("/coverage") {
                    ("text/html", r#"<!doctype html><title>Coverage fixture</title><iframe src='/subdocument'></iframe><script>
                        globalThis.workerResults = {};
                        new Worker('/worker.js').onmessage=e=>workerResults.dedicated=e.data;
                        const shared=new SharedWorker('/shared.js'); shared.port.onmessage=e=>workerResults.shared=e.data;
                        navigator.serviceWorker.register('/service.js').then(()=>workerResults.service='registered').catch(()=>workerResults.service='failed');
                        const socket=new WebSocket('ws://'+location.host+'/socket'); socket.onerror=()=>workerResults.socket='error';
                    </script>"#.to_owned())
                } else {
                    let mut page =
                        "<!doctype html><title>Protection fixture</title><h1>Useful content</h1>"
                            .to_owned();
                    for i in 0..100 {
                        page.push_str(&format!("<script src='/useful.js?n={i}'></script>"));
                        if !path.starts_with("/clean") {
                            page.push_str(&format!("<script src='/ads/cbr.js?n={i}'></script>"));
                        }
                    }
                    ("text/html", page)
                };
                let csp = if path.starts_with("/strict") {
                    "Content-Security-Policy: script-src 'none'; style-src 'none'; require-trusted-types-for 'script'\r\n"
                } else {
                    ""
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n{csp}Connection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            origin,
            stop,
            blocked_hits,
            allowed_hits,
            worker: Some(worker),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

fn policy() -> (Arc<WorkerBlocker>, Arc<ContentRules>) {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/blocker-seed/v1");
    let mut sources = Vec::new();
    for name in ["easylist", "easyprivacy"] {
        let mut text = String::new();
        flate2::read::GzDecoder::new(
            std::fs::File::open(root.join(format!("{name}.txt.gz"))).unwrap(),
        )
        .read_to_string(&mut text)
        .unwrap();
        sources.push(PolicySource::new(
            SourceId::new(name).unwrap(),
            SourceFormat::Standard,
            Arc::from(text),
        ));
    }
    let worker = WorkerBlocker::start(StaticPolicyCatalog::new(sources).unwrap()).unwrap();
    let (tx, rx) = mpsc::channel();
    assert_eq!(
        worker.compile(
            zephium_core::ids::ProfileId::generate(),
            ContentPolicyGeneration::new(1).unwrap(),
            BlockerConfig { enabled: true },
            Box::new(move |result| {
                tx.send(result).unwrap();
            })
        ),
        BlockerDispatch::Scheduled
    );
    let BlockerCompileOutcome::Compiled(rules) = rx.recv_timeout(Duration::from_secs(180)).unwrap()
    else {
        panic!("seed compilation failed")
    };
    (worker, rules)
}

#[test]
fn percentiles_use_nearest_rank_and_empty_samples_stay_empty() {
    assert_eq!(summary(vec![])["p99_ns"], Value::Null);
    let sample = summary((1..=100).rev().collect());
    assert_eq!(sample["p50_ns"], 50);
    assert_eq!(sample["p99_ns"], 99);
    assert_eq!(sample["max_ns"], 100);
}

#[test]
#[ignore = "physical WebView2 qualifier; run alone with --ignored --nocapture --test-threads=1"]
fn native_protection_qualification() {
    let mode = std::env::var("ZEPHIUM_PROTECTION_MODE").unwrap_or_else(|_| "on".into());
    assert!(["on", "off", "paused"].contains(&mode.as_str()));
    let tabs: usize = std::env::var("ZEPHIUM_PROTECTION_TABS")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .unwrap();
    assert!([1, 10, 30].contains(&tabs));
    let site = std::env::var("ZEPHIUM_PROTECTION_SITE").unwrap_or_else(|_| "fixture".into());
    let fixture = Fixture::start();
    let url = match site.as_str() {
        "fixture" => format!("{}/heavy", fixture.origin),
        "clean" => format!("{}/clean", fixture.origin),
        "csp" => format!("{}/strict", fixture.origin),
        "coverage" => format!("{}/coverage", fixture.origin),
        "yahoo" => "https://www.yahoo.com/".into(),
        "bloomberg" => "https://www.bloomberg.com/".into(),
        "news" => "https://www.theguardian.com/technology/2026/sep/02/google-defeats-justice-department-bid-ad-tech-sale".into(),
        "github" => "https://github.com/rust-lang/rust".into(),
        "google" => "https://www.google.com/search?q=rust+programming".into(),
        _ => panic!("unknown fixed qualification site"),
    };
    let (worker, rules) = policy();
    let policy = prepare(&rules).unwrap();
    let pause = crate::platform::content_pause::ContentPause::default();
    let counter: zephium_core::blocker::BlockedLoadCounter = Arc::default();
    let counting = std::env::var("ZEPHIUM_PROTECTION_COUNTING").unwrap_or_else(|_| "on".into());
    assert!(["on", "off"].contains(&counting.as_str()));
    if counting == "on" {
        pause.set_statistics(counter.clone());
    }
    pause.set(mode == "paused");
    let profile = tempfile::tempdir().unwrap();
    let host = Host::new();
    let mut context = wry::WebContext::new(Some(profile.path().to_owned()));
    let mut views = Vec::new();
    let mut registrations = Vec::new();
    let mut pages = Vec::new();
    let coverage = Rc::new(RefCell::new(Vec::new()));
    let mut observers = Vec::new();
    let public_page = !["fixture", "clean", "csp", "coverage"].contains(&site.as_str());
    let settle = Duration::from_secs(if public_page { 3 } else { 0 });
    let construction = Instant::now();
    SAMPLES.with(|samples| {
        *samples.borrow_mut() = Some(Samples {
            callback: Vec::with_capacity(SAMPLE_LIMIT),
            matcher: Vec::with_capacity(SAMPLE_LIMIT),
            dropped: 0,
        })
    });
    for _ in 0..tabs {
        let loaded = Rc::new(Cell::new(None::<Instant>));
        let event = loaded.clone();
        let mut builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_visible(true)
            .with_focused(false)
            .with_devtools(false)
            .with_on_page_load_handler(move |phase, _| {
                event.set(matches!(phase, wry::PageLoadEvent::Finished).then(Instant::now));
            });
        if site == "csp" {
            // Exercise the exact fixed production bootstrap in a native CSP
            // document, without introducing a page-to-host bridge.
            let discard = include_str!("../../host/scripts.rs")
                .split_once("const DISCARD_SAFETY_BOOTSTRAP_JS: &str = r#\"")
                .unwrap()
                .1
                .split_once("\"#;")
                .unwrap()
                .0;
            builder = builder.with_initialization_script_for_main_only(discard, true);
            builder = builder.with_initialization_script_for_main_only(
                include_str!("../../host/content_style.js"),
                true,
            );
        }
        if let Some(view) = views.first() {
            builder = builder.with_environment(WebViewExtWindows::environment(view));
        }
        let view = builder.build_as_child(&host).unwrap();
        if site == "coverage" {
            assert_eq!(tabs, 1, "worker observation has one environment owner");
            let observations = coverage.clone();
            let core = view.webview();
            let core22: ICoreWebView2_22 = core.cast().unwrap();
            let observer = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut context = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                let mut source = COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_DOCUMENT;
                unsafe {
                    args.ResourceContext(&mut context)?;
                    args.cast::<ICoreWebView2WebResourceRequestedEventArgs2>()?
                        .RequestedSourceKind(&mut source)?;
                    let request = args.Request()?;
                    let mut uri = PWSTR::null();
                    request.Uri(&mut uri)?;
                    let uri = webview2_com::take_pwstr(uri);
                    let mut destination = PWSTR::null();
                    let _ = request
                        .Headers()?
                        .GetHeader(windows_core::w!("Sec-Fetch-Dest"), &mut destination);
                    let destination = if destination.is_null() {
                        String::new()
                    } else {
                        webview2_com::take_pwstr(destination)
                    };
                    let mut entries = observations.borrow_mut();
                    if entries.len() < 1000 {
                        entries.push(json!({"url":uri, "context":context.0, "source":source.0, "destination":destination}));
                    }
                }
                Ok(())
            }));
            let mut token = 0;
            unsafe {
                core22
                    .AddWebResourceRequestedFilterWithRequestSourceKinds(
                        windows_core::w!("*"),
                        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                        COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                    )
                    .unwrap();
                core.add_WebResourceRequested(&observer, &mut token)
                    .unwrap();
            }
            observers.push((core, core22, token));
        }
        // An ALL-source observer broadens the event stream seen by every
        // handler on this view. Keep that diagnostic separate from policy
        // qualification, whose production source-kind filters stay exact.
        let selected = if mode == "off" || site == "coverage" {
            &NativeContentPolicy::AllowAll
        } else {
            &policy
        };
        registrations.push(install_scoped_on_view(&view, selected, &pause).unwrap());
        loaded.set(None);
        let started = Instant::now();
        view.load_url(&url).unwrap();
        // Consent and client redirects can follow an initial completion. Use
        // the last completed navigation after a bounded quiet interval, and
        // exclude that deliberate settle wait from the page-load metric.
        until(|| loaded.get().is_some_and(|at| at.elapsed() >= settle), 90);
        let elapsed = loaded.get().unwrap().duration_since(started).as_secs_f64() * 1000.0;
        if site == "coverage" {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                pump();
            }
            pages.push(json!({"worker_results":evaluate(&view, "globalThis.workerResults")}));
        }
        let navigation = evaluate(
            &view,
            "JSON.parse(JSON.stringify(performance.getEntriesByType('navigation')[0] || null))",
        );
        let document = evaluate(
            &view,
            "({url:location.href,title:document.title,readyState:document.readyState,bodyCharacters:document.body?.textContent?.length || 0})",
        );
        pages.push(json!({"host_load_ms":elapsed, "navigation":navigation, "document":document}));
        if site == "fixture" || site == "clean" {
            assert_eq!(evaluate(&view, "globalThis.usefulExecuted || 0"), 100);
            let ads = if mode == "on" || site == "clean" {
                0
            } else {
                100
            };
            assert_eq!(evaluate(&view, "globalThis.adExecuted || 0"), ads);
        }
        if site == "csp" {
            assert_eq!(
                evaluate(
                    &view,
                    r#"(() => {
                const report = globalThis.__zephium_discard_safety_v1__;
                if (report() !== 1) return 'clean document rejected';
                const listener = () => {};
                addEventListener('beforeunload', listener);
                if (!(report() & 2)) return 'bare unload handler missed';
                removeEventListener('beforeunload', listener);
                if (report() !== 1) return 'removed handler retained';
                const host = document.createElement('div'); document.body.append(host);
                const root = host.attachShadow({mode:'closed'});
                const input = document.createElement('input'); root.append(input); input.value = 'unsaved';
                if (!(report() & 4)) return 'closed shadow form missed';
                input.value = '';
                if (report() !== 1) return 'clean shadow form rejected';
                const fragment = document.createDocumentFragment();
                for (let i = 0; i < 25000; i++) fragment.append(document.createElement('span'));
                host.append(fragment);
                if (!(report() & 256)) return 'oversized document admitted';
                host.remove();
                return 'passed';
            })()"#
                ),
                "passed"
            );
            assert_eq!(
                evaluate(
                    &view,
                    r#"(() => {
                const a = globalThis.__zephium_content_style_v1__;
                const {token, url} = a.inspect();
                if (globalThis.pageScriptRan) return 'page CSP failed';
                const first = document.getElementById('first');
                first.style.setProperty('display', 'block', 'important');
                if (a.apply('personal', 'stale-token', url, '0000000000000001', '1'.repeat(64), '#first{display:none!important}')) return 'stale admission';
                if (!a.apply('personal', token, url, '0000000000000001', '1'.repeat(64), '#first{display:none!important}')) return 'apply failed';
                if (getComputedStyle(first).display !== 'none') return 'inline important survived';
                if (!a.apply('personal', token, url, '0000000000000002', '2'.repeat(64), '#first,#second{display:none!important}')) return 'second apply failed';
                if (getComputedStyle(first).display !== 'none' || getComputedStyle(document.getElementById('second')).display !== 'none') return 'multiple hide failed';
                if (!a.apply('personal', token, url, '0000000000000003', '3'.repeat(64), '')) return 'undo failed';
                if (first.style.getPropertyValue('display') !== 'block' || getComputedStyle(document.querySelector('h1')).display === 'none') return 'restore failed';
                if (!a.startPicker(token, url, '0000000000000004')) return 'picker CSP failed';
                if (!a.stopPicker(token, url)) return 'picker stop failed';
                return 'passed';
            })()"#
                ),
                "passed"
            );
        }
        views.push(view);
    }
    let elapsed = construction.elapsed().as_secs_f64();
    let samples = SAMPLES.with(|s| s.borrow_mut().take().unwrap());
    let mut callbacks = serde_json::Map::new();
    for outcome in [
        "allowed",
        "blocked",
        "paused",
        "native_fail_open",
        "response_failed",
    ] {
        callbacks.insert(
            outcome.into(),
            summary(
                samples
                    .callback
                    .iter()
                    .filter(|(kind, _)| *kind == outcome)
                    .map(|(_, ns)| *ns)
                    .collect(),
            ),
        );
    }
    let count = counter.0.load(Ordering::Relaxed);
    if site == "fixture" {
        assert_eq!(
            fixture.blocked_hits.load(Ordering::Relaxed),
            if mode == "on" { 0 } else { 100 * tabs as u64 }
        );
        assert_eq!(
            count,
            if mode == "on" && counting == "on" {
                100 * tabs as u64
            } else {
                0
            }
        );
    }
    let mut runtime = PWSTR::null();
    unsafe {
        views[0]
            .environment()
            .BrowserVersionString(&mut runtime)
            .unwrap();
    }
    let runtime = webview2_com::take_pwstr(runtime);
    let diagnostics = if let NativeContentPolicy::Runtime { policy, .. } = &policy {
        let d = policy.diagnostics();
        json!({"decisions":d.total_decisions, "budget_exhausted":d.candidate_budget_exhausted,
            "unavailable":d.matcher_unavailable, "unprepared":d.matcher_unprepared,
            "attribution_unavailable":d.attribution_unavailable, "errors":d.evaluation_errors})
    } else {
        Value::Null
    };
    println!(
        "PROTECTION_RESULT {}",
        json!({"mode":mode, "counting":counting, "site":site, "tabs":tabs, "runtime":runtime,
        "debug_assertions":cfg!(debug_assertions), "construction_and_load_seconds":elapsed,
        "public_settle_seconds":settle.as_secs(),
        "callback":callbacks, "matcher":summary(samples.matcher), "dropped_samples":samples.dropped,
        "installed_blocks":count, "fixture_allowed_hits":fixture.allowed_hits.load(Ordering::Relaxed),
        "diagnostics":diagnostics, "coverage":*coverage.borrow(), "pages":pages})
    );
    drop(fixture);
    let idle: u64 = std::env::var("ZEPHIUM_PROTECTION_IDLE_SECONDS")
        .unwrap_or_else(|_| "0".into())
        .parse()
        .unwrap();
    assert!(idle <= 600);
    println!("PROTECTION_IDLE_READY");
    std::io::stdout().flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(idle);
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            MsgWaitForMultipleObjectsEx(
                None,
                remaining.as_millis().min(u32::MAX as u128) as u32,
                QS_ALLINPUT,
                MWMO_INPUTAVAILABLE,
            );
        }
    }
    println!("PROTECTION_IDLE_COMPLETE");
    std::io::stdout().flush().unwrap();
    for registration in registrations {
        registration.retire().unwrap();
    }
    for (core, core22, token) in observers {
        unsafe {
            core.remove_WebResourceRequested(token).unwrap();
            core22
                .RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                    windows_core::w!("*"),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
                )
                .unwrap();
        }
    }
    drop(views);
    assert_eq!(
        worker.shutdown_until(Instant::now() + Duration::from_secs(5)),
        BlockerShutdownOutcome::Clean
    );
}
