use gtk::glib::prelude::ObjectExt;
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
use webkit2gtk::WebViewExt;

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
        while Instant::now() < deadline {
            while gtk::events_pending() && Instant::now() < deadline {
                gtk::main_iteration_do(false);
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
            "WEBEXT native_shutdown drained={drained} elapsed_ms={:.3} remaining={:?}",
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

    /// A read-only directory with no module, as when it is missing.
    pub fn empty() -> Self {
        let directory = tempfile::tempdir().expect("isolated module directory");
        std::fs::set_permissions(directory.path(), Permissions::from_mode(0o555)).unwrap();
        Self(directory)
    }

    pub fn path(&self) -> &Path {
        self.0.path()
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
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
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
                let header = request
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("x-native-test")
                            .then(|| value.trim())
                    })
                    .unwrap_or("no-native-header");
                {
                    let mut records = recorded.lock().unwrap();
                    assert!(records.len() < 512, "bounded fixture request capture");
                    records.push(Request { path: path.into() });
                }
                let (mime, body) = response(path, header, port);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes());
                let _ = socket.write_all(body.as_bytes());
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

fn response(path: &str, _header: &str, port: u16) -> (&'static str, String) {
    let route = path.split('?').next().unwrap_or(path);
    match route {
        "/frames" => ("text/html", format!(
            "<!doctype html><iframe id='same' src='/frame?same'></iframe><iframe id='cross' src='http://localhost:{port}/frame?cross'></iframe>"
        )),
        "/blocked" | "/allowed" => ("text/plain", "fixture-body".into()),
        _ => ("text/html", "<!doctype html><p>bridge fixture</p>".into()),
    }
}
