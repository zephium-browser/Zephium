use super::{MemoryResource, RequestHost, Requester, Scheme, SchemeError};
use gtk::prelude::*;
use serde_json::{json, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use webkit2gtk::{WebContextExt, WebViewExt};
use zephium_core::{ids::ExtensionInstallId, webkitgtk};
use zephium_webext::manifest::Manifest;

const ALPHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const BETA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FRAME_DOCUMENT: &[u8] =
    b"<!doctype html><title>package frame</title><script src='/frame.js'></script>";

#[test]
#[ignore = "requires private linux-test.sh session and admitted sandboxed WebKitGTK"]
fn native_package_scheme_acceptance() {
    let _session = NativeSession::new();
    let server = Server::new();
    let root = tempfile::tempdir().unwrap();
    let alpha_root = root.path().join("alpha");
    let beta_root = root.path().join("beta");
    let manifest = prepare_package(&alpha_root, "alpha");
    prepare_package(&beta_root, "beta");
    std::fs::write(root.path().join("outside.txt"), "outside-package").unwrap();
    std::os::unix::fs::symlink(
        root.path().join("outside.txt"),
        alpha_root.join("escape.txt"),
    )
    .unwrap();

    let host = Rc::new(TestHost::default());
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    assert!(context.is_sandbox_enabled());
    let scheme = Scheme::register(&context, host.clone(), None).unwrap();
    assert!(matches!(
        Scheme::register(&context, host.clone(), None),
        Err(SchemeError::AlreadyRegistered)
    ));
    let alpha = ExtensionInstallId::from(1);
    let beta = ExtensionInstallId::from(2);
    let alias = scheme
        .add_install(alpha, ALPHA, &alpha_root, &manifest)
        .unwrap();
    scheme
        .add_install(beta, BETA, &beta_root, &manifest)
        .unwrap();
    scheme
        .set_runtime_resource(
            alpha,
            "/_zephium/runtime.html",
            MemoryResource::new(FRAME_DOCUMENT.to_vec(), "text/html").unwrap(),
        )
        .unwrap();

    let alpha_view = NativeView::owned(&context, &host, alpha);
    let beta_view = NativeView::owned(&context, &host, beta);
    let tab = NativeView::tab(&context, &host);
    load(
        &alpha_view.page,
        &format!("chrome-extension://{ALPHA}/index.html"),
    );
    let capabilities = observe(
        &alpha_view.page,
        "package_secure_module_wasm_csp",
        "for(let i=0;i<200 && !globalThis.packageProbe;i++)await new Promise(resolve=>setTimeout(resolve,10));return await globalThis.packageProbe;",
    );
    assert_eq!(
        capabilities,
        json!({
            "origin": format!("chrome-extension://{ALPHA}"),
            "secure": true,
            "module": 42,
            "inline": false,
            "eval": false,
            "wasm": true,
            "body": "alpha"
        })
    );
    assert_eq!(
        observe(
            &alpha_view.page,
            "package_response_headers",
            r#"
            const response=await fetch('/data.txt',{cache:'no-store'});
            return {type:response.headers.get('content-type'),
                cache:response.headers.get('cache-control'),
                nosniff:response.headers.get('x-content-type-options'),
                policies:response.headers.get('content-security-policy').split(',').length};
        "#
        ),
        json!({"type":"text/plain","cache":"no-store","nosniff":"nosniff","policies":2})
    );
    assert_eq!(
        observe(&alpha_view.page, "large_package_stream", "const bytes=new Uint8Array(await (await fetch('/blob.bin')).arrayBuffer());return {length:bytes.length,intact:bytes.every(byte=>byte===90)};"),
        json!({"length":196_613,"intact":true})
    );
    assert_eq!(
        frames(
            &alpha_view.page,
            ALPHA,
            &["private.html", "_zephium/runtime.html"]
        ),
        json!([true, true]),
        "negative resource controls must execute in an owned view"
    );
    assert_eq!(
        observe(
            &alpha_view.page,
            "sandbox_opaque_origin",
            r#"
            return await new Promise(resolve=>{
                const frame=document.createElement('iframe');
                const finish=result=>{clearTimeout(timer);removeEventListener('message',received);frame.remove();resolve(result)};
                const received=e=>{if(e.source===frame.contentWindow && e.data?.fixture==='sandbox')
                    finish({origin:e.origin,storageDenied:e.data.storageDenied})};
                addEventListener('message',received);
                const timer=setTimeout(()=>finish({error:'sandbox script did not report'}),2500);
                frame.src='/sandbox.html';document.body.append(frame);
            });
        "#
        ),
        json!({"origin":"null","storageDenied":true})
    );

    let refused = observe(
        &alpha_view.page,
        "traversal_and_symlink_refused",
        r#"
        return await Promise.all(['/%2e%2e%2foutside.txt','/escape.txt'].map(async path=>{
            try { const r=await fetch(location.origin+path,{cache:'no-store'});
                return {ok:r.ok,body:await r.text()}; }
            catch { return {ok:false,body:''}; }
        }));
        "#,
    );
    for row in refused.as_array().expect("refusal observations") {
        assert_eq!(row["ok"], false);
        assert_ne!(row["body"], "outside-package");
    }

    let storage = "const before=localStorage.getItem('install');localStorage.setItem('install',location.hostname);return before;";
    assert_eq!(
        observe(&alpha_view.page, "alpha_storage_initial", storage),
        Value::Null
    );
    load(
        &beta_view.page,
        &format!("chrome-extension://{BETA}/index.html"),
    );
    assert_eq!(
        observe(&beta_view.page, "beta_storage_initial", storage),
        Value::Null
    );
    assert_eq!(
        observe(
            &beta_view.page,
            "beta_package_body",
            "for(let i=0;i<200 && !globalThis.packageProbe;i++)await new Promise(resolve=>setTimeout(resolve,10));return (await globalThis.packageProbe).body;"
        ),
        "beta"
    );
    assert_eq!(
        observe(&alpha_view.page, "alpha_storage_retained", storage),
        ALPHA
    );
    assert_eq!(
        observe(&beta_view.page, "beta_storage_retained", storage),
        BETA
    );
    let isolated = observe(
        &alpha_view.page,
        "cross_install_resource_refused",
        &format!(
            "try {{return (await fetch('chrome-extension://{BETA}/data.txt',{{cache:'no-store'}})).ok}}catch{{return false}}"
        ),
    );
    assert_eq!(isolated, false);

    load(&tab.page, &format!("{}/index.html", server.origin));
    assert_eq!(
        frames(
            &tab.page,
            ALPHA,
            &[
                "public.html",
                "private.html",
                "_zephium/runtime.html",
                "dynamic.html"
            ]
        ),
        json!([true, false, false, false])
    );
    assert_eq!(frames(&tab.page, &alias, &["dynamic.html"]), json!([true]));
    let private_fetch = format!(
        "try{{return (await fetch('chrome-extension://{ALPHA}/data.txt',{{cache:'no-store'}})).ok}}catch{{return false}}"
    );
    assert_eq!(
        observe(&tab.page, "unframed_tab_private_fetch", &private_fetch),
        false
    );
    // A live frame of the extension admits its non-public resources to the tab.
    *host.framed.borrow_mut() = vec![ALPHA.to_owned()];
    assert_eq!(
        frames(&tab.page, ALPHA, &["private.html", "_zephium/runtime.html"]),
        json!([true, false])
    );
    let private_script = format!(
        "return await new Promise(resolve=>{{const s=document.createElement('script');s.onload=()=>resolve(true);s.onerror=()=>resolve(false);s.src='chrome-extension://{ALPHA}/entry.js?'+Math.random();document.head.append(s);}});"
    );
    // The scheme cannot tell the frame from its embedder: this is the coarse layer.
    println!(
        "WEBEXT package_framed_embedding_page private_fetch={} private_script={}",
        observe(&tab.page, "framed_tab_private_fetch", &private_fetch),
        observe(&tab.page, "framed_tab_private_script", &private_script)
    );
    *host.framed.borrow_mut() = vec![BETA.to_owned()];
    assert_eq!(frames(&tab.page, ALPHA, &["private.html"]), json!([false]));
    host.framed.borrow_mut().clear();
    assert_eq!(frames(&tab.page, ALPHA, &["private.html"]), json!([false]));
    assert_eq!(
        observe(&tab.page, "unframed_again_private_fetch", &private_fetch),
        false
    );
    assert_eq!(
        observe(&tab.page, "unframed_private_script", &private_script),
        false
    );
    load(
        &tab.page,
        &format!(
            "{}/index.html",
            server.origin.replace("127.0.0.1", "localhost")
        ),
    );
    assert_eq!(frames(&tab.page, ALPHA, &["public.html"]), json!([false]));

    assert!(scheme.remove_install(alpha));
    assert_eq!(
        observe(
            &alpha_view.page,
            "removed_install_stops_serving",
            "try{return (await fetch('/data.txt',{cache:'no-store'})).ok}catch{return false}"
        ),
        false
    );
    assert_eq!(
        observe(
            &beta_view.page,
            "other_install_still_serves",
            "return await (await fetch('/data.txt',{cache:'no-store'})).text();"
        ),
        "beta"
    );
    scheme.retire();
    assert_eq!(
        observe(
            &beta_view.page,
            "retired_profile_stops_serving",
            "try{return (await fetch('/data.txt',{cache:'no-store'})).ok}catch{return false}"
        ),
        false
    );
}

fn prepare_package(root: &Path, label: &str) -> Manifest {
    std::fs::create_dir(root).unwrap();
    let manifest = json!({
        "manifest_version": 3,
        "name": "native package fixture",
        "version": "1.0",
        "content_security_policy": {
            "extension_pages": "script-src 'self' 'wasm-unsafe-eval'; object-src 'self'",
            "sandbox": "sandbox allow-scripts; script-src 'self'; object-src 'self'"
        },
        "sandbox": {"pages":["sandbox.html"]},
        "web_accessible_resources": [{
            "resources": ["public.html", "frame.js"],
            "matches": ["http://127.0.0.1/*"]
        }, {
            "resources": ["dynamic.html", "dynamic.js"],
            "matches": ["http://127.0.0.1/*"],
            "use_dynamic_url": true
        }]
    });
    std::fs::write(root.join("manifest.json"), manifest.to_string()).unwrap();
    for (path, bytes) in [
        ("index.html", b"<!doctype html><title>package fixture</title><script>globalThis.inlineRan=true</script><script type='module' src='/entry.js'></script>".as_slice()),
        ("entry.js", br#"
            import {answer} from './dependency.js';
            globalThis.packageProbe=(async()=>{
                let evaluated=false;
                try{new Function('return 1')();evaluated=true}catch{}
                const wasm=await WebAssembly.instantiateStreaming(fetch('/empty.wasm'));
                return {origin:location.origin,secure:isSecureContext,module:answer,
                    inline:!!globalThis.inlineRan,eval:evaluated,wasm:!!wasm.instance,
                    body:await (await fetch('/data.txt')).text()};
            })();
        "#.as_slice()),
        ("dependency.js", b"export const answer=42;".as_slice()),
        ("empty.wasm", b"\0asm\x01\0\0\0".as_slice()),
        ("public.html", FRAME_DOCUMENT),
        ("private.html", FRAME_DOCUMENT),
        ("frame.js", br#"parent.postMessage({fixture:'package-frame',token:location.hash.slice(1),origin:location.origin},'*');"#.as_slice()),
        ("dynamic.html", b"<!doctype html><script src='./dynamic.js'></script>".as_slice()),
        ("dynamic.js", br#"parent.postMessage({fixture:'package-frame',token:location.hash.slice(1),origin:location.origin},'*');"#.as_slice()),
        ("sandbox.html", b"<!doctype html><script src='/sandbox.js'></script>".as_slice()),
        ("sandbox.js", br#"let storageDenied=false;try{localStorage.getItem('sandbox')}catch{storageDenied=true}parent.postMessage({fixture:'sandbox',storageDenied},'*');"#.as_slice()),
    ] {
        std::fs::write(root.join(path), bytes).unwrap();
    }
    std::fs::write(root.join("data.txt"), label).unwrap();
    std::fs::write(root.join("blob.bin"), vec![90; 196_613]).unwrap();
    Manifest::from_value(manifest)
}

#[derive(Default)]
struct TestHost {
    requesters: RefCell<BTreeMap<usize, Requester>>,
    framed: RefCell<Vec<String>>,
}

impl RequestHost for TestHost {
    fn requester(&self, view: &webkit2gtk::WebView) -> Option<Requester> {
        self.requesters
            .borrow()
            .get(&(view.as_ptr() as usize))
            .cloned()
    }

    fn framed_extensions(&self, _: &webkit2gtk::WebView) -> Vec<String> {
        self.framed.borrow().clone()
    }
}

struct NativeView {
    page: webkit2gtk::WebView,
}

impl NativeView {
    fn owned(
        context: &webkit2gtk::WebContext,
        host: &TestHost,
        install: ExtensionInstallId,
    ) -> Self {
        let page = webkit2gtk::WebView::with_context(context);
        host.requesters.borrow_mut().insert(
            page.as_ptr() as usize,
            Requester::ExtensionOwned { install },
        );
        Self { page }
    }

    fn tab(context: &webkit2gtk::WebContext, host: &Rc<TestHost>) -> Self {
        let page = webkit2gtk::WebView::with_context(context);
        let host = host.clone();
        page.connect_load_changed(move |page, event| {
            if event == webkit2gtk::LoadEvent::Committed {
                let mut requesters = host.requesters.borrow_mut();
                let key = page.as_ptr() as usize;
                requesters.remove(&key);
                if let Some(url) = page.uri().and_then(|uri| url::Url::parse(&uri).ok()) {
                    requesters.insert(
                        key,
                        Requester::Tab {
                            committed_top_level: url,
                            extension_install: None,
                        },
                    );
                }
            }
        });
        Self { page }
    }
}

impl Drop for NativeView {
    fn drop(&mut self) {
        self.page.stop_loading();
        // Native widgets must close before the session drains WebKit's close work.
        unsafe { self.page.destroy() };
    }
}

fn load(page: &webkit2gtk::WebView, uri: &str) {
    let finished = Rc::new(Cell::new(false));
    let done = finished.clone();
    let signal = page.connect_load_changed(move |_, event| {
        if event == webkit2gtk::LoadEvent::Finished {
            done.set(true);
        }
    });
    page.load_uri(uri);
    pump_until(|| finished.get());
    page.disconnect(signal);
}

fn observe(page: &webkit2gtk::WebView, label: &str, body: &str) -> Value {
    let result = Rc::new(RefCell::new(None));
    let received = result.clone();
    let source = format!(
        r#"
        let timer;
        try {{
            const value=await Promise.race([(async()=>{{{body}}})(),
                new Promise((_,reject)=>{{timer=setTimeout(()=>reject(new Error('native probe timeout')),10000)}})]);
            const encoded=JSON.stringify(value ?? null);
            return encoded.length<=65536?encoded:'{{"error":"oversize"}}';
        }}catch(e){{return JSON.stringify({{error:String(e).slice(0,256)}})}}
        finally{{clearTimeout(timer)}}
    "#
    );
    page.call_async_javascript_function(
        &source,
        None,
        None,
        None,
        None::<&gio::Cancellable>,
        move |value| *received.borrow_mut() = Some(value.map(|value| value.to_string())),
    );
    pump_until(|| result.borrow().is_some());
    let encoded = result
        .borrow_mut()
        .take()
        .unwrap()
        .expect("native JavaScript evaluation");
    let value: Value = serde_json::from_str(&encoded).unwrap();
    println!("WEBEXT_SCHEME {label}={value}");
    value
}

fn frames(page: &webkit2gtk::WebView, extension: &str, paths: &[&str]) -> Value {
    let source = format!(
        r#"
        const base='chrome-extension://{extension}/', paths={paths};
        return await Promise.all(paths.map((path,index)=>new Promise(resolve=>{{
            const frame=document.createElement('iframe'), token='native-frame-'+index;
            const finish=loaded=>{{clearTimeout(timer);removeEventListener('message',received);frame.remove();resolve(loaded)}};
            const received=e=>{{if(e.source===frame.contentWindow && e.origin===base.slice(0,-1) &&
                e.data?.fixture==='package-frame' && e.data.token===token && e.data.origin===e.origin)finish(true)}};
            addEventListener('message',received);
            const timer=setTimeout(()=>finish(false),2500);
            frame.src=base+path+'#'+token;document.body.append(frame);
        }})));
    "#,
        paths = serde_json::to_string(paths).unwrap()
    );
    observe(page, "package_frames", &source)
}

fn pump_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        while gtk::events_pending() {
            assert!(
                Instant::now() < deadline,
                "native event queue did not drain"
            );
            gtk::main_iteration_do(false);
        }
        assert!(Instant::now() < deadline, "native test timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[must_use = "bind as the first local so the native loop outlives every WebKit owner"]
struct NativeSession;

impl NativeSession {
    fn new() -> Self {
        assert!(std::env::var_os("ZEPHIUM_WEBEXT_PROBE_PRIVATE_SESSION").is_some(),
            "use timeout -k 10 420 scripts/ci/linux-test.sh with ZEPHIUM_WEBEXT_PROBE_PRIVATE_SESSION=1");
        for (name, _) in std::env::vars_os() {
            assert!(
                !name
                    .to_str()
                    .is_some_and(webkitgtk::environment_override_is_security_relevant),
                "unset security-relevant WebKitGTK environment overrides"
            );
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(now >= webkitgtk::SECURITY_FLOOR_PUBLISHED_UNIX_SECONDS);
        let version = unsafe {
            (
                webkit2gtk::ffi::webkit_get_major_version(),
                webkit2gtk::ffi::webkit_get_minor_version(),
                webkit2gtk::ffi::webkit_get_micro_version(),
            )
        };
        webkitgtk::assess_runtime(version.0, version.1, version.2, now)
            .expect("admitted WebKitGTK security floor");
        gtk::init().expect("private GTK display");
        Self
    }
}

impl Drop for NativeSession {
    fn drop(&mut self) {
        // Run queued page-close work before WebKit destroys this thread's run loop.
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
        println!("WEBEXT_SCHEME native_shutdown drained={drained} elapsed_ms={:.3} remaining_web_processes={:?}",
            start.elapsed().as_secs_f64()*1000.0,web_processes());
        if !std::thread::panicking() {
            assert!(
                drained,
                "native shutdown did not settle before test-thread exit"
            );
        }
    }
}

fn web_processes() -> BTreeSet<u32> {
    let mut pending = vec![std::process::id()];
    let mut found = BTreeSet::new();
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
        let command = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        if command
            .split(|byte| *byte == 0)
            .next()
            .is_some_and(|arg| arg == b"WebKitWebProcess" || arg.ends_with(b"/WebKitWebProcess"))
        {
            found.insert(pid);
        }
    }
    found
}

struct Server {
    origin: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("local fixture accept: {error}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while request.len() < 8192 && !request.windows(4).any(|bytes| bytes == b"\r\n\r\n")
                {
                    match socket.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&buffer[..count]),
                    }
                }
                let body =
                    "<!doctype html><title>ordinary web fixture</title><body>Web page</body>";
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Self {
            origin,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
