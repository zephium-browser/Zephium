use super::*;
use gtk::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use webkit2gtk::WebContextExt;
use wry::WebViewBuilderExtUnix;
use zephium_core::blocker::{ContentRuleCoverage, ContentRuleDigest};

struct Fixture {
    address: String,
    blocked_requests: Arc<AtomicUsize>,
    allowed_requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_websocket(false)
    }

    fn with_websocket(websocket: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let blocked_requests = Arc::new(AtomicUsize::new(0));
        let requests = blocked_requests.clone();
        let allowed_requests = Arc::new(AtomicUsize::new(0));
        let allowed = allowed_requests.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("local fixture accept failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0; 1024];
                while request.len() < 8192 && !request.windows(4).any(|v| v == b"\r\n\r\n") {
                    match stream.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&bytes[..count]),
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let path = text.split_ascii_whitespace().nth(1).unwrap_or("");
                let body = if path.starts_with("/blocked.js") {
                    requests.fetch_add(1, Ordering::Relaxed);
                    "allowed"
                } else if path.starts_with("/allowed.js") {
                    allowed.fetch_add(1, Ordering::Relaxed);
                    "allowed"
                } else if websocket {
                    "<!doctype html><script>Promise.all(['/blocked.js','/allowed.js'].map(path=>\
                     new Promise(resolve=>{const ws=new WebSocket(location.origin.replace(/^http/,'ws')+path);\
                     ws.onopen=()=>{ws.close();resolve()};ws.onerror=()=>resolve()})))\
                     .then(()=>document.title='settled');</script>"
                } else {
                    "<!doctype html><script>Promise.all([\
                     fetch('/blocked.js').then(()=> 'allowed',()=> 'blocked'),\
                     fetch('/allowed.js').then(()=> 'allowed',()=> 'blocked')\
                     ]).then(v=>document.title=v.join(':'));</script>"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            address,
            blocked_requests,
            allowed_requests,
            stop,
            worker: Some(worker),
        }
    }

    fn load(&self, view: &wry::WebView, sequence: u8, expected: &str) {
        let page = view.webview();
        let uri = format!("{}/page-{sequence}", self.address);
        let finished = Rc::new(std::cell::Cell::new(false));
        let ready = finished.clone();
        let token = page.connect_load_changed(move |_, event| {
            if event == webkit2gtk::LoadEvent::Finished {
                ready.set(true);
            }
        });
        page.load_uri(&uri);
        let deadline = Instant::now() + Duration::from_secs(15);
        while !finished.get() || page.title().as_deref() != Some(expected) {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            assert!(
                Instant::now() < deadline,
                "request outcome: {:?}, expected {expected}",
                page.title()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        page.disconnect(token);
    }
}

#[test]
#[ignore = "requires an isolated display and admitted sandboxed WebKitGTK"]
fn native_filter_websocket_handshakes_obey_pause_and_retirement() {
    crate::platform::linux::enforce_runtime_security_floor().expect("admitted WebKitGTK");
    gtk::init().expect("isolated GTK display");
    let context = glib::MainContext::default();
    let _owner = context.acquire().expect("test owns GTK main context");
    let fixture = Fixture::with_websocket(true);
    let cache = tempfile::tempdir().unwrap();
    let rules = ContentRules::declarative(
        ContentRuleDigest::from_bytes([0x5c; 32]),
        ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            blocking_rule_entries: 1,
            ..Default::default()
        },
        DeclarativeRuleFormat::WebKitContentBlockerV1,
        Arc::from(r#"[{"trigger":{"url-filter":"/blocked[.]js","resource-type":["websocket"]},"action":{"type":"block"}}]"#),
    )
    .unwrap();
    let ContentRulesPayload::Declarative {
        artifact_digest, ..
    } = rules.payload()
    else {
        panic!("declarative fixture");
    };
    let policy = super::tests::wait_for_native_compilation(
        &context,
        cache.path(),
        rules.clone(),
        *artifact_digest.as_bytes(),
    )
    .unwrap();
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let view = wry::WebViewBuilder::new()
        .with_incognito(true)
        .build_gtk(&container)
        .unwrap();
    assert!(view.webview().context().unwrap().is_sandbox_enabled());
    let pause = crate::platform::content_pause::ContentPause::default();
    let registration = install_scoped_on_view(
        &view,
        &policy,
        &pause,
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    fixture.load(&view, 1, "settled");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 0);
    assert_eq!(fixture.allowed_requests.load(Ordering::Relaxed), 1);
    pause.set(true);
    fixture.load(&view, 2, "settled");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.allowed_requests.load(Ordering::Relaxed), 2);
    pause.set(false);
    fixture.load(&view, 3, "settled");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 1);
    registration.retire().unwrap();
    fixture.load(&view, 4, "settled");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 2);
    assert_eq!(fixture.allowed_requests.load(Ordering::Relaxed), 4);
    view.webview().terminate_web_process();
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
#[ignore = "requires an isolated display and admitted sandboxed WebKitGTK"]
fn native_filter_blocks_requests_and_pause_and_retirement_are_scoped() {
    crate::platform::linux::enforce_runtime_security_floor().expect("admitted WebKitGTK");
    gtk::init().expect("isolated GTK display");
    let context = glib::MainContext::default();
    let _owner = context.acquire().expect("test owns GTK main context");
    let fixture = Fixture::new();
    let cache = tempfile::tempdir().unwrap();
    let rules = ContentRules::declarative(
        ContentRuleDigest::from_bytes([0x5b; 32]),
        ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            blocking_rule_entries: 1,
            ..Default::default()
        },
        DeclarativeRuleFormat::WebKitContentBlockerV1,
        Arc::from(r#"[{"trigger":{"url-filter":"/blocked[.]js"},"action":{"type":"block"}}]"#),
    )
    .unwrap();
    let ContentRulesPayload::Declarative {
        artifact_digest, ..
    } = rules.payload()
    else {
        panic!("declarative fixture");
    };
    let policy = super::tests::wait_for_native_compilation(
        &context,
        cache.path(),
        rules.clone(),
        *artifact_digest.as_bytes(),
    )
    .unwrap();
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let view = wry::WebViewBuilder::new()
        .with_incognito(true)
        .build_gtk(&container)
        .unwrap();
    let other = wry::WebViewBuilder::new()
        .with_incognito(true)
        .build_gtk(&container)
        .unwrap();
    for view in [&view, &other] {
        let page = view.webview();
        assert!(page.is_ephemeral());
        let context = page.context().unwrap();
        assert!(context.is_ephemeral());
        assert!(context.is_sandbox_enabled());
    }
    let pause = crate::platform::content_pause::ContentPause::default();
    let registration = install_scoped_on_view(
        &view,
        &policy,
        &pause,
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    let observed_blocked = Rc::new(std::cell::Cell::new(0));
    let observed = observed_blocked.clone();
    view.webview()
        .connect_resource_load_started(move |_, resource, request| {
            use webkit2gtk::URIRequestExt;
            if request
                .uri()
                .is_some_and(|uri| uri.ends_with("/blocked.js"))
            {
                observed.set(observed.get() + 1);
            }
            use webkit2gtk::WebResourceExt;
            resource.connect_failed(|_, error| {
                eprintln!("resource failure: {error:?}");
            });
        });
    fixture.load(&view, 1, "blocked:allowed");
    assert_eq!(
        observed_blocked.get(),
        0,
        "content-filter hits have no resource object"
    );
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 0);
    fixture.load(&other, 2, "allowed:allowed");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 1);
    pause.set(true);
    fixture.load(&view, 3, "allowed:allowed");
    assert_eq!(observed_blocked.get(), 1);
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 2);
    pause.set(false);
    fixture.load(&view, 4, "blocked:allowed");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 2);
    registration.retire().unwrap();
    pause.set(true);
    pause.set(false);
    fixture.load(&view, 5, "allowed:allowed");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 3);
}

#[test]
#[ignore = "isolated GTK; native persistent filter reuse and damaged-cache refusal"]
fn native_filter_cache_reuses_validated_bytecode_without_loading_rule_text() {
    crate::platform::linux::enforce_runtime_security_floor().unwrap();
    gtk::init().unwrap();
    let context = glib::MainContext::default();
    let _owner = context.acquire().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let text: Arc<str> =
        Arc::from(r#"[{"trigger":{"url-filter":"/blocked[.]js"},"action":{"type":"block"}}]"#);
    let digest = [0x63; 32];
    let run = |text: Arc<dyn zephium_core::blocker::DeclarativeRuleText>| {
        let result = Rc::new(std::cell::RefCell::new(None));
        let done = result.clone();
        let _cancellation = compile(cache.path(), text, digest, move |value| {
            *done.borrow_mut() = Some(value);
        });
        let deadline = Instant::now() + Duration::from_secs(15);
        while result.borrow().is_none() {
            context.iteration(false);
            assert!(
                Instant::now() < deadline,
                "native cache completion deadline"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        result.take().unwrap()
    };
    drop(run(Arc::new(text.clone())).unwrap());
    struct UnavailableText(usize, Arc<AtomicUsize>);
    impl zephium_core::blocker::DeclarativeRuleText for UnavailableText {
        fn len(&self) -> usize {
            self.0
        }
        fn load(&self) -> Option<Arc<str>> {
            self.1.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let unavailable = Arc::new(UnavailableText(text.len(), calls.clone()));
    let policy = run(unavailable.clone()).expect("warm native hit requires no source text");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let fixture = Fixture::new();
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let view = wry::WebViewBuilder::new()
        .with_incognito(true)
        .build_gtk(&container)
        .unwrap();
    let registration = install_scoped_on_view(
        &view,
        &policy,
        &Default::default(),
        &Default::default(),
        &Default::default(),
    )
    .unwrap();
    fixture.load(&view, 1, "blocked:allowed");
    assert_eq!(fixture.blocked_requests.load(Ordering::Relaxed), 0);
    registration.retire().unwrap();
    view.webview().terminate_web_process();
    drop(policy);
    let mut damaged = 0;
    for entry in std::fs::read_dir(cache.path()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::write(entry.path(), b"damaged cache").unwrap();
            damaged += 1;
        }
    }
    assert!(damaged > 0);
    assert!(
        run(unavailable).is_err(),
        "corrupt bytecode cannot become protection"
    );
}
