use gtk::glib::{self, prelude::ObjectExt};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    fs::Permissions,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use webkit2gtk::{WebContextExt, WebViewExt};

thread_local! {
    static SEEN_RENDERERS: RefCell<BTreeSet<u32>> = const { RefCell::new(BTreeSet::new()) };
}

pub fn init() -> NativeSession {
    assert!(
        std::env::var_os("ZEPHIUM_WEBEXT_PROBE_PRIVATE_SESSION").is_some(),
        "run with timeout -k 10 420 scripts/ci/linux-test.sh"
    );
    for name in [
        "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS",
        "WEBKIT_FORCE_SANDBOX",
    ] {
        assert!(std::env::var_os(name).is_none(), "sandbox override: {name}");
    }
    gtk::init().expect("isolated GTK display");
    SEEN_RENDERERS.with_borrow_mut(BTreeSet::clear);
    NativeSession
}

#[must_use = "declare before all native owners so shutdown drains last"]
pub struct NativeSession;

impl Drop for NativeSession {
    fn drop(&mut self) {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(10);
        let mut quiet = None;
        let mut drained = false;
        let mut busy_iterations = 0u64;
        let mut renderer_iterations = 0u64;
        let mut exits: Vec<(u32, Option<u128>)> =
            web_processes().into_iter().map(|pid| (pid, None)).collect();
        while Instant::now() < deadline {
            let live = web_processes();
            for (pid, exited) in &mut exits {
                if exited.is_none() && !live.contains(pid) {
                    *exited = Some(start.elapsed().as_millis());
                }
            }
            while gtk::events_pending() && Instant::now() < deadline {
                gtk::main_iteration_do(false);
                busy_iterations += 1;
            }
            if !web_processes().is_empty() {
                renderer_iterations += 1;
            }
            if web_processes().is_empty() && !gtk::events_pending() {
                let since = quiet.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(100) {
                    drained = true;
                    break;
                }
            } else {
                quiet = None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        println!(
            "WEBEXT native_shutdown drained={drained} elapsed_ms={:.3} remaining={:?} busy_iterations={busy_iterations} renderer_iterations={renderer_iterations} renderer_exit_ms={exits:?}",
            start.elapsed().as_secs_f64() * 1000.0,
            web_processes()
        );
        if !std::thread::panicking() {
            assert!(drained, "native shutdown did not settle before thread exit");
        }
    }
}

pub fn pump_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        while gtk::events_pending() {
            assert!(Instant::now() < deadline, "native queue did not drain");
            gtk::main_iteration_do(false);
        }
        assert!(Instant::now() < deadline, "native test timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn pump_for(duration: Duration) {
    let until = Instant::now() + duration;
    pump_until(|| Instant::now() >= until);
}

pub struct ModuleDirectory(tempfile::TempDir);

impl ModuleDirectory {
    pub fn new() -> Self {
        let artifact = std::env::var_os("ZEPHIUM_WEBEXT_MODULE_PATH")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("ZEPHIUM_WEBEXT_MODULE_DIR").map(|directory| {
                    PathBuf::from(directory).join("libzephium_webkit_web_process.so")
                })
            })
            .expect("set ZEPHIUM_WEBEXT_MODULE_PATH to the built cdylib");
        let artifact = artifact.canonicalize().expect("built web-process cdylib");
        let directory = tempfile::tempdir().expect("isolated module directory");
        let module = directory.path().join("libzephium_webkit_web_process.so");
        std::fs::copy(artifact, &module).expect("copy only the intended module");
        std::fs::set_permissions(module, Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(directory.path(), Permissions::from_mode(0o555)).unwrap();
        Self(directory)
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }

    pub fn context(&self, initialization: &glib::Variant) -> webkit2gtk::WebContext {
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        assert!(context.is_sandbox_enabled());
        context.add_path_to_sandbox(self.path(), true);
        context.set_web_extensions_directory(self.path().to_str().unwrap());
        context.set_web_extensions_initialization_user_data(initialization);
        context
    }
}

impl Drop for ModuleDirectory {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(self.path(), Permissions::from_mode(0o700));
    }
}

pub fn load(page: &webkit2gtk::WebView, url: &str) {
    let finished = Rc::new(Cell::new(false));
    let observed = finished.clone();
    let signal = page.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Finished {
            observed.set(true);
        }
    });
    page.load_uri(url);
    pump_until(|| finished.get());
    page.disconnect(signal);
    let _ = web_processes();
}

pub fn evaluate(page: &webkit2gtk::WebView, body: &str, world: Option<&str>) -> String {
    let result = Rc::new(RefCell::new(None));
    let observed = result.clone();
    page.call_async_javascript_function(
        body,
        None,
        world,
        None,
        None::<&gtk::gio::Cancellable>,
        move |value| {
            *observed.borrow_mut() = Some(value.map(|value| value.to_string()));
        },
    );
    pump_until(|| result.borrow().is_some());
    let value = result.borrow_mut().take().unwrap();
    value.expect("native JavaScript evaluation")
}

pub fn send_raw(
    page: &webkit2gtk::WebView,
    name: &str,
    parameters: Option<&glib::Variant>,
    descriptors: Option<&gio::UnixFDList>,
) -> Result<webkit2gtk::UserMessage, glib::Error> {
    let result = Rc::new(RefCell::new(None));
    let observed = result.clone();
    page.send_message_to_page(
        &webkit2gtk::UserMessage::with_fd_list(name, parameters, descriptors),
        None::<&gio::Cancellable>,
        move |reply| *observed.borrow_mut() = Some(reply),
    );
    pump_until(|| result.borrow().is_some());
    let reply = result.borrow_mut().take().unwrap();
    reply
}

pub fn web_processes() -> BTreeSet<u32> {
    let mut pending = vec![std::process::id()];
    let mut renderers = BTreeSet::new();
    let mut visited = BTreeSet::new();
    while let Some(pid) = pending.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for task in tasks.flatten() {
                if let Ok(children) = std::fs::read_to_string(task.path().join("children")) {
                    pending.extend(
                        children
                            .split_whitespace()
                            .filter_map(|pid| pid.parse::<u32>().ok()),
                    );
                }
            }
        }
        if is_renderer(pid) {
            renderers.insert(pid);
        }
    }
    SEEN_RENDERERS.with_borrow_mut(|seen| {
        seen.extend(renderers);
        seen.retain(|pid| is_renderer(*pid));
        seen.clone()
    })
}

fn is_renderer(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .unwrap_or_default()
        .split(|byte| *byte == 0)
        .next()
        .is_some_and(|arg| arg == b"WebKitWebProcess" || arg.ends_with(b"/WebKitWebProcess"))
}

pub fn assert_sandbox(module: &ModuleDirectory) {
    let renderers = web_processes();
    assert!(!renderers.is_empty(), "module must load in a live renderer");
    for pid in renderers {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let field = |name: &str| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .map(str::trim)
        };
        assert_eq!(field("Seccomp:"), Some("2"));
        assert_eq!(field("NoNewPrivs:"), Some("1"));
        for namespace in ["mnt", "user", "pid"] {
            assert_ne!(
                std::fs::read_link(format!("/proc/{pid}/ns/{namespace}")).unwrap(),
                std::fs::read_link(format!("/proc/self/ns/{namespace}")).unwrap(),
                "separate renderer {namespace} namespace"
            );
        }
        let mounts = std::fs::read_to_string(format!("/proc/{pid}/mountinfo")).unwrap();
        assert!(
            mounts.lines().any(|line| {
                let fields: Vec<_> = line.split_ascii_whitespace().collect();
                fields
                    .get(4)
                    .is_some_and(|path| *path == module.path().to_str().unwrap())
                    && fields
                        .get(5)
                        .is_some_and(|options| options.split(',').any(|option| option == "ro"))
            }),
            "module directory must be mounted read-only in renderer {pid}"
        );
    }
    println!("WEBEXT native_sandbox sandbox=true read_only_module_mount=true");
}

#[derive(Clone, Debug)]
pub struct Request {
    pub path: String,
    pub header: String,
    pub destination: String,
    pub accept: String,
}

pub struct Fixture {
    pub origin: String,
    pub cross_origin: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    pub fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                // Connections are served concurrently, as a browser expects of a site.
                let recorded = recorded.clone();
                std::thread::spawn(move || serve(socket, &recorded, port));
            }
        });
        Self {
            origin: format!("http://127.0.0.1:{port}"),
            cross_origin: format!("http://localhost:{port}"),
            requests,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn serve(mut socket: std::net::TcpStream, recorded: &Mutex<Vec<Request>>, port: u16) {
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0; 1024];
    while bytes.len() < 8192 && !bytes.windows(4).any(|part| part == b"\r\n\r\n") {
        match socket.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(count) => bytes.extend_from_slice(&chunk[..count]),
        }
    }
    let request = String::from_utf8_lossy(&bytes);
    let path = request.split_ascii_whitespace().nth(1).unwrap_or("");
    let field = |wanted: &str| {
        request.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case(wanted).then(|| value.trim())
        })
    };
    let header = field("x-native-test").unwrap_or("no-native-header");
    {
        let mut records = recorded.lock().unwrap();
        assert!(records.len() < 4096, "bounded fixture request capture");
        records.push(Request {
            path: path.into(),
            header: header.into(),
            destination: field("sec-fetch-dest").unwrap_or("").into(),
            accept: field("accept").unwrap_or("").into(),
        });
    }
    let (mime, body) = response(path, header, port);
    let csp = if path.starts_with("/dnr-csp") {
        "Content-Security-Policy: script-src 'none'; report-uri /dnr-resource?probe=csp_report\r\n"
    } else if path.starts_with("/csp-sandbox") {
        "Content-Security-Policy: sandbox allow-scripts\r\n"
    } else {
        ""
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\n{csp}Content-Type: {mime}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes());
    let _ = socket.write_all(body.as_bytes());
}

fn response(path: &str, header: &str, port: u16) -> (&'static str, String) {
    let path = path
        .strip_prefix("http://")
        .and_then(|value| value.find('/').map(|at| &value[at..]))
        .unwrap_or(path);
    let route = path.split('?').next().unwrap_or(path);
    if route == "/news" {
        return ("text/html", news_page());
    }
    if let Some(kind) = path
        .split("nt=")
        .nth(1)
        .map(|rest| rest.split('&').next().unwrap())
    {
        return match kind {
            "js" => ("text/javascript", "void 0;".into()),
            "css" => ("text/css", "p{margin:0}".into()),
            "img" => (
                "image/svg+xml",
                "<svg xmlns='http://www.w3.org/2000/svg' width='2' height='2'/>".into(),
            ),
            "json" => ("application/json", "{}".into()),
            _ => ("text/html", "<!doctype html><p>embed</p>".into()),
        };
    }
    if route == "/dnr-types" {
        return ("text/html", include_str!("../dnr-resources.html").into());
    }
    if route == "/dnr-csp" {
        return (
            "text/html",
            "<!doctype html><script>globalThis.shouldBeBlocked=true;</script>".into(),
        );
    }
    if route == "/dnr-sw-page" {
        return ("text/html", "<!doctype html><script>globalThis.dnrProbes={};navigator.serviceWorker.addEventListener('message',()=>dnrProbes.serviceWorker=true);navigator.serviceWorker.register('/dnr-sw.js?probe=service_worker_script').then(()=>navigator.serviceWorker.ready).then(r=>r.active.postMessage('fetch')).catch(e=>dnrProbes.serviceWorkerError=String(e));</script>".into());
    }
    if route == "/dnr-sw.js" {
        return ("text/javascript", "self.addEventListener('install',e=>e.waitUntil(self.skipWaiting()));self.addEventListener('activate',e=>e.waitUntil(self.clients.claim()));self.addEventListener('message',e=>e.waitUntil(fetch('/dnr-resource?probe=service_worker_fetch').then(()=>e.source.postMessage('sw-fetched'))));".into());
    }
    if route == "/dnr-worker.js" {
        return ("text/javascript", "importScripts('/dnr-script.js?probe=worker_import');fetch('/dnr-resource?probe=worker_fetch').finally(()=>postMessage('done'));".into());
    }
    if route == "/dnr-script.js" {
        return ("text/javascript", "globalThis.dnrScript=true;".into());
    }
    if route == "/dnr-module.js" {
        return (
            "text/javascript",
            "import '/dnr-script.js?probe=module_import';".into(),
        );
    }
    if route == "/dnr-style.css" {
        return (
            "text/css",
            "@font-face{font-family:dnr;src:url('/dnr-font.woff?probe=font')}body{font-family:dnr}"
                .into(),
        );
    }
    if route == "/dnr-image.svg" {
        return (
            "image/svg+xml",
            "<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>".into(),
        );
    }
    if route == "/dnr-audio.wav" {
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&8000u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data\0\0\0\0");
        return ("audio/wav", String::from_utf8(wav).unwrap());
    }
    if route == "/dnr-headers" {
        return ("text/plain", header.into());
    }
    if route == "/dnr-actual" {
        return ("text/plain", path.into());
    }
    if route == "/dnr-frame" {
        return ("text/html", "<!doctype html><script>fetch('/dnr-resource?probe=frame_fetch').catch(()=>{});</script>".into());
    }
    let kind = path
        .split("kind=")
        .nth(1)
        .unwrap_or("")
        .split('&')
        .next()
        .unwrap();
    if route.starts_with("/policy-") && kind == "script" {
        let value = if route == "/policy-actual" {
            "redirected-resource"
        } else {
            header
        };
        return (
            "text/javascript",
            format!("globalThis.nativeScriptResult={value:?};"),
        );
    }
    if route.starts_with("/policy-") && kind == "image" {
        return (
            "image/svg+xml",
            "<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'/>".into(),
        );
    }
    let inline = "<script>document.documentElement.dataset.main=String(globalThis.mainOverride);document.documentElement.dataset.hidden=String(typeof nativeSend==='undefined');document.dispatchEvent(new Event('DOMContentLoaded',{bubbles:true}));window.dispatchEvent(new Event('load'));document.documentElement.dataset.earlyEnd=String(document.documentElement.dataset.nativeEnd==='yes');window.addEventListener('DOMContentLoaded',e=>e.stopImmediatePropagation(),true);window.addEventListener('load',e=>e.stopImmediatePropagation(),true);</script>";
    match route {
        "/facts" => ("text/html", format!("<!doctype html><iframe src='/frame?ordinary'></iframe><iframe src='http://localhost:{port}/frame?cross-facts'></iframe><iframe sandbox='allow-scripts' src='/frame?sandbox'></iframe><iframe src='/csp-sandbox'></iframe><iframe src='data:text/html,opaque'></iframe>")),
        "/content-frames" => ("text/html", format!(
            "<!doctype html>{inline}<iframe src='/content-frame?same'></iframe><iframe src='http://localhost:{port}/content-frame?cross'></iframe><iframe id='blank' src='about:blank'></iframe><iframe srcdoc=\"{inline}\"></iframe><iframe src='/excluded'></iframe><script>const blankDocument=document.getElementById('blank').contentDocument;const firstInline=blankDocument.createElement('script');firstInline.textContent=\"globalThis.blankInlineSawMain=globalThis.mainOverride==='native'\";blankDocument.documentElement.append(firstInline);</script>"
        )),
        "/content-frame" | "/excluded" => ("text/html", format!("<!doctype html>{inline}<p>content fixture</p>")),
        "/frames" => ("text/html", format!(
            "<!doctype html><iframe id='same' src='/frame?same'></iframe><iframe id='cross' src='http://localhost:{port}/frame?cross'></iframe>"
        )),
        "/worker.js" => ("text/javascript", format!(
            "{};onmessage=async event=>postMessage(await nativeRequests(event.data));",
            include_str!("../requests.js")
        )),
        "/policy-actual" => ("text/plain", "redirected-resource".into()),
        "/policy-headers" => ("text/plain", header.into()),
        _ => ("text/html", "<!doctype html><p>native fixture</p>".into()),
    }
}

/// A news-shaped page: 240 subresources over first-party, CDN, media and
/// common advertising/analytics hosts, all served by this fixture via proxy.
pub fn news_page() -> String {
    let trackers = [
        "www.googletagmanager.com/gtag/js?id=G-N",
        "www.google-analytics.com/analytics.js?v=",
        "securepubads.g.doubleclick.net/tag/js/gpt.js?v=",
        "sb.scorecardresearch.com/beacon.js?v=",
        "static.chartbeat.com/js/chartbeat.js?v=",
        "connect.facebook.net/en_US/fbevents.js?v=",
        "c.amazon-adsystem.com/aax2/apstag.js?v=",
        "cdn.taboola.com/libtrc/news/loader.js?v=",
        "widgets.outbrain.com/outbrain.js?v=",
        "script.hotjar.com/modules.js?v=",
    ];
    let mut page = String::from("<!doctype html><meta charset=utf-8><title>news</title>");
    for i in 0..3 {
        page += &format!("<link rel=stylesheet href='http://news.test/n/site{i}.css?nt=css'>");
        page +=
            &format!("<link rel=stylesheet href='http://cdn.newsstatic.test/n/lib{i}.css?nt=css'>");
    }
    for (i, tracker) in trackers.iter().enumerate() {
        page += &format!("<script src='http://news.test/n/app{i}.js?nt=js'></script>");
        page += &format!(
            "<script async src='http://cdn.newsstatic.test/n/vendor{i}.js?nt=js'></script>"
        );
        page += &format!("<script async src='http://{tracker}{i}&nt=js'></script>");
    }
    for i in 0..150 {
        page += &format!("<img alt='' src='http://img.newsmedia.test/n/photo{i}.svg?nt=img'>");
    }
    for i in 0..20 {
        page += &format!(
            "<img alt='' src='http://pagead2.googlesyndication.com/pagead/imgad?id={i}&nt=img'>"
        );
    }
    for i in 0..6 {
        page += &format!("<iframe src='http://tpc.googlesyndication.com/safeframe/1-0-40/html/container.html?n={i}&nt=html'></iframe>");
    }
    for i in 0..4 {
        page += &format!("<iframe src='http://news.test/n/embed{i}.html?nt=html'></iframe>");
    }
    page += "<script>for (let i = 0; i < 10; i++) { fetch('http://news.test/n/api' + i + '.json?nt=json').catch(() => {}); fetch('http://www.google-analytics.com/g/collect?v=2&n=' + i + '&nt=json').catch(() => {}); }</script>";
    page
}
