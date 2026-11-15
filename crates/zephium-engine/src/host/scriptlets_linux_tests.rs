use super::*;
use gtk::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use webkit2gtk::WebContextExt;
use zephium_core::blocker::{scriptlet_program_with_pauses, BlockerSite};

const RULES: &str = "\
localhost,127.0.0.1##+js(set-constant, fixtureFlag, true)\n\
localhost,127.0.0.1##+js(json-prune, adPlacements adSlots playerAds)\n\
localhost,127.0.0.1##+js(no-xhr-if, /ad-endpoint, emptyObj)";

#[test]
fn malformed_or_oversized_programs_are_refused_before_native_registration() {
    for source in [
        String::new(),
        "run();\0other();".into(),
        "x".repeat(MAX_PROGRAM_BYTES + 1),
    ] {
        assert!(Program::new(&source).is_none());
    }
}

fn compiled(rules: &str) -> String {
    let policy = zephium_blocker::CosmeticPolicy::compile([rules]).unwrap();
    assert_eq!(policy.report().unknown_scriptlet_rules, 0);
    assert_eq!(policy.report().unsupported_scriptlet_rules, 0);
    policy.scriptlet_program().unwrap().to_owned()
}

struct Fixture {
    port: u16,
    requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let received = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut input = Vec::new();
                let mut buffer = [0; 1024];
                while input.len() < 8192 && !input.windows(4).any(|part| part == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => input.extend_from_slice(&buffer[..count]),
                    }
                }
                let input = String::from_utf8_lossy(&input);
                let path = input.split_ascii_whitespace().nth(1).unwrap_or("");
                let body = if path.starts_with("/ad-endpoint") {
                    received.fetch_add(1, Ordering::Relaxed);
                    "network".to_owned()
                } else {
                    fixture_html(port)
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        Self {
            port,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self, host: &str) -> String {
        format!("http://{host}:{}/top", self.port)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn fixture_html(port: u16) -> String {
    let script = r#"const topFrame = self === top;
const label = topFrame ? 'top' : location.pathname.slice(1);
const records = [];
function record(value) {
    records.push(value);
    if (records.length === 5) document.title = records.sort().join('|');
}
if (topFrame) addEventListener('message', event => {
    if (typeof event.data === 'string' && event.data.startsWith('fixture:')) record(event.data.slice(8));
});
const constant = globalThis.fixtureFlag === true;
const player = JSON.parse('{"adPlacements":[1],"adSlots":[2],"playerAds":[3],"streamingData":{"formats":[1,2]},"playabilityStatus":{"status":"OK"}}');
const pruned = !('adPlacements' in player) && !('adSlots' in player) && !('playerAds' in player);
const intact = player.streamingData.formats.length === 2 && player.playabilityStatus.status === 'OK';
const xhr = new XMLHttpRequest();
xhr.open('GET', '/ad-endpoint');
xhr.onload = () => {
    const value = [label, constant, pruned, intact, xhr.responseText === '{}', globalThis.unrelated === 7].join(':');
    if (topFrame) record(value); else top.postMessage('fixture:' + value, '*');
};
xhr.send();
if (topFrame) addEventListener('DOMContentLoaded', () => {
    for (const label of ['same', 'cross']) {
        const frame = document.createElement('iframe');
        frame.src = label === 'same' ? '/same' : 'http://127.0.0.1:PORT/cross';
        document.body.append(frame);
    }
    const blank = document.createElement('iframe');
    document.body.append(blank);
    blank.contentDocument.write('<script>top.postMessage("fixture:blank:" + Boolean(globalThis.fixtureFlag), "*")<\/script>');
    blank.contentDocument.close();
    const srcdoc = document.createElement('iframe');
    srcdoc.srcdoc = '<script>top.postMessage("fixture:srcdoc:" + Boolean(globalThis.fixtureFlag), "*")<\/script>';
    document.body.append(srcdoc);
});"#;
    format!(
        "<!doctype html><html><head><script>{}</script></head><body></body></html>",
        script.replace("PORT", &port.to_string())
    )
}

fn pump_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
            assert!(Instant::now() < deadline, "native loop did not settle");
        }
        assert!(Instant::now() < deadline, "native fixture timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct Page {
    view: webkit2gtk::WebView,
    window: gtk::Window,
}

impl Page {
    fn new() -> Self {
        let manager = webkit2gtk::WebsiteDataManager::new_ephemeral();
        let context = webkit2gtk::WebContext::builder()
            .website_data_manager(&manager)
            .process_swap_on_cross_site_navigation_enabled(true)
            .build();
        context.set_sandbox_enabled(true);
        let view = webkit2gtk::WebView::with_context(&context);
        assert!(view.is_ephemeral() && context.is_sandbox_enabled());
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.add(&view);
        window.show_all();
        Self { view, window }
    }

    fn load(&self, url: &str, expected: &str) {
        let finished = Rc::new(std::cell::Cell::new(false));
        let observed = finished.clone();
        let handler = self.view.connect_load_changed(move |_, event| {
            if event == webkit2gtk::LoadEvent::Finished {
                observed.set(true);
            }
        });
        eprintln!("loading fixture, expecting {expected}");
        let title_handler = self
            .view
            .connect_title_notify(|view| eprintln!("fixture title: {:?}", view.title()));
        self.view.load_uri(url);
        pump_until(|| finished.get() && self.view.title().as_deref() == Some(expected));
        self.view.disconnect(handler);
        self.view.disconnect(title_handler);
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        self.view.terminate_web_process();
        unsafe { self.window.destroy() };
        let until = Instant::now() + Duration::from_millis(200);
        while Instant::now() < until {
            while gtk::events_pending() && Instant::now() < until {
                gtk::main_iteration_do(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn native_init() {
    assert!(
        std::env::var_os("ZEPHIUM_BLOCKER_NATIVE_ISOLATED").is_some(),
        "use the isolated blocker runner"
    );
    crate::platform::linux::enforce_runtime_security_floor().expect("admitted WebKitGTK");
    assert!(std::env::var_os("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS").is_none());
    gtk::init().expect("run through scripts/ci/linux-test.sh");
}

fn expected(constant: bool, pruned: bool, xhr: bool) -> String {
    format!("blank:{constant}|cross:{constant}:{pruned}:true:{xhr}:true|same:{constant}:{pruned}:true:{xhr}:true|srcdoc:false|top:{constant}:{pruned}:true:{xhr}:true")
}

#[test]
#[ignore = "requires isolated Xvfb, disposable profiles and admitted sandboxed WebKitGTK"]
fn native_scriptlets_precede_head_scripts_and_preserve_frames_pauses_and_profiles() {
    native_init();
    let fixture = Fixture::new();
    let first = Page::new();
    let second = Page::new();
    let first_manager = first.view.user_content_manager().unwrap();
    let second_manager = second.view.user_content_manager().unwrap();
    let unrelated = webkit2gtk::UserScript::new(
        "globalThis.unrelated = 7;",
        webkit2gtk::UserContentInjectedFrames::AllFrames,
        webkit2gtk::UserScriptInjectionTime::Start,
        &[],
        &[],
    );
    first_manager.add_script(&unrelated);
    second_manager.add_script(&unrelated);
    let first_slot = ViewScriptlet::default();
    let second_slot = ViewScriptlet::default();
    let mut profiles = Scriptlets::default();
    let a = ProfileId::generate();
    let b = ProfileId::generate();
    let program = compiled(RULES);
    profiles.prepare(a, Some(&program));
    profiles.prepare(b, Some(&program));
    assert!(Rc::ptr_eq(&profiles.0[&a], &profiles.0[&b]));
    first_slot.replace(&first_manager, profiles.0.get(&a).cloned());
    first.load(&fixture.url("localhost"), &expected(true, true, true));
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 0);

    // Cross-site navigation must carry the native registration into a new document.
    first.load(&fixture.url("127.0.0.1"), &expected(true, true, true));
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 0);
    second.load(&fixture.url("localhost"), &expected(false, false, false));
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 3);

    let pause = BlockerSite::try_from("localhost".to_owned()).unwrap();
    let paused = scriptlet_program_with_pauses(&program, [&pause]);
    profiles.prepare(a, Some(&paused));
    first_slot.replace(&first_manager, profiles.0.get(&a).cloned());
    first.load(&fixture.url("localhost"), &expected(false, false, false));
    // A paused top-level site also pauses its cross-origin child.
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 6);
    first.load(&fixture.url("127.0.0.1"), &expected(true, true, true));
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 6);

    profiles.prepare(b, Some(&program));
    second_slot.replace(&second_manager, profiles.0.get(&b).cloned());
    second.load(&fixture.url("localhost"), &expected(true, true, true));
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 6);

    let excepted = compiled(&format!(
        "{RULES}\nlocalhost,127.0.0.1#@#+js(set-constant, fixtureFlag, true)"
    ));
    profiles.prepare(b, Some(&excepted));
    second_slot.replace(&second_manager, profiles.0.get(&b).cloned());
    second.load(&fixture.url("localhost"), &expected(false, true, true));
    // Removing our exact registration preserves another owner's script.
    second_slot.replace(&second_manager, None);
    profiles.forget(b);
    second.load(&fixture.url("localhost"), &expected(false, false, false));
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 9);
}

#[test]
#[ignore = "requires isolated Xvfb, disposable profiles and admitted sandboxed WebKitGTK"]
fn native_scriptlets_refuse_privileged_documents_and_their_remote_children() {
    native_init();
    let fixture = Fixture::new();
    let page = Page::new();
    let manager = page.view.user_content_manager().unwrap();
    let slot = ViewScriptlet::default();
    slot.replace(
        &manager,
        Some(Rc::new(Program::new(&compiled(RULES)).unwrap())),
    );
    let html = format!(
        r#"<!doctype html><script>
        const own = Boolean(globalThis.fixtureFlag);
        addEventListener('message', e => {{ if (typeof e.data === 'string' && e.data.startsWith('fixture:')) document.title = own + '|' + e.data; }});
        </script><iframe src="http://localhost:{}/remote"></iframe>"#,
        fixture.port
    );
    page.view.load_html(
        &html,
        Some("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/fixture"),
    );
    pump_until(|| {
        page.view.title().as_deref() == Some("false|fixture:remote:false:false:true:false:false")
    });
    assert_eq!(fixture.requests.load(Ordering::Relaxed), 1);
}
