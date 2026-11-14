#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

//! Pinned public extensions on the WebKitGTK runtime: Dark Reader darkens
//! every frame of a light page and its popup toggles the site; Vimium's link
//! hints follow a link in a cross-origin frame and its vomnibar opens.
//! Packages come from `scripts/webext/fetch-pinned.sh --extract`; point
//! `ZEPHIUM_WEBEXT_PINNED_DIR` at the cache directory.

#[path = "shell_support/mod.rs"]
mod shell;
#[path = "runtime_support/mod.rs"]
mod support;

use std::cell::{Cell, RefCell};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use serde_json::{json, Value};
use shell::{emit, Engine, Host, Shell, INSTALL, PROFILE};
use support::{pump_for, pump_until, ModuleDirectory};
use webkit2gtk::{WebContextExt, WebViewExt};
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_linux::runtime::{
    InstallSpec, ProfileRuntime, ProfileRuntimeConfig, RuntimeEngine,
};
use zephium_webext_linux::views::PageKind;
use zephium_webext_runtime::context::ExtensionId;
use zephium_webext_runtime::i18n::LocaleEnvironment;
use zephium_webext_runtime::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

const DARK_READER: (&str, &str) = ("eimadpbcbfnmbkopoojfekhnkhdbieeh", "4.9.135");
const VIMIUM: (&str, &str) = ("dbepggeogbaibhgnhhndojpepiihcmeb", "2.4.2");

struct Fixture {
    port: u16,
    reports: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

const REPORT: &str = "<script>setTimeout(()=>{const s=getComputedStyle(document.body);\
fetch('/report?'+encodeURIComponent(JSON.stringify({where:document.title,bg:s.backgroundColor,color:s.color})));},2500)</script>";

impl Fixture {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let reports: Arc<Mutex<Vec<Value>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (collected, stopping) = (reports.clone(), stop.clone());
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 2048];
                while request.len() < 65536 && !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => request.extend_from_slice(&buffer[..count]),
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let path = text
                    .split_ascii_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_owned();
                let light = "style=\"background:#ffffff;color:#000000\"";
                let body = if let Some(query) = path.strip_prefix("/report?") {
                    let decoded = url::form_urlencoded::parse(format!("q={query}").as_bytes())
                        .next()
                        .map(|(_, value)| value.into_owned())
                        .unwrap_or_default();
                    if let Ok(value) = serde_json::from_str(&decoded) {
                        collected.lock().unwrap().push(value);
                    }
                    String::new()
                } else if path.starts_with("/clicked") {
                    collected.lock().unwrap().push(json!({"where": "clicked"}));
                    "<!doctype html><title>clicked</title><p>clicked".to_owned()
                } else if path.starts_with("/linkframe") {
                    "<!doctype html><title>linkframe</title><body style=\"margin:0\"><a href=\"/clicked\" style=\"font-size:40px\">follow me</a>".to_owned()
                } else if path.starts_with("/vimium") {
                    format!("<!doctype html><title>vimium</title><body style=\"margin:0\"><p>no links here</p><iframe src=\"http://localhost:{port}/linkframe\" style=\"width:600px;height:200px\"></iframe>")
                } else if path.starts_with("/frame") {
                    format!("<!doctype html><title>frame</title><body {light}><p>frame text</p>{REPORT}")
                } else {
                    format!(
                        "<!doctype html><title>top</title><body {light}><p>light page</p>\
                         <iframe src=\"/frame\" title=\"same\"></iframe>\
                         <iframe id=\"blank\"></iframe>\
                         <iframe src=\"http://localhost:{port}/frame\"></iframe>{REPORT}\
                         <script>const d=document.getElementById('blank').contentDocument;\
                         d.body.setAttribute('style','background:#ffffff;color:#000000');d.body.innerHTML='<p>blank text</p>';\
                         setTimeout(()=>{{const s=getComputedStyle(d.body);fetch('/report?'+encodeURIComponent(JSON.stringify({{where:'blank',bg:s.backgroundColor,color:s.color}})));}},2500)</script>"
                    )
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            port,
            reports,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn take(&self) -> Vec<Value> {
        std::mem::take(&mut self.reports.lock().unwrap())
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

fn package((id, version): (&str, &str)) -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_PINNED_DIR")?);
    let directory = root.join(format!("{id}-{version}"));
    directory
        .join("manifest.json")
        .is_file()
        .then_some(directory)
}

fn host(
    context: webkit2gtk::WebContext,
    module: &ModuleDirectory,
    state: &Path,
    id: &str,
) -> (Host, Rc<dyn RuntimeEngine>) {
    let shell = Rc::new(RefCell::new(Shell::default()));
    let engine: Rc<dyn RuntimeEngine> = Rc::new(Engine {
        extension: id.to_owned(),
        shell: shell.clone(),
    });
    let runtime = ProfileRuntime::new(
        &context,
        ProfileRuntimeConfig {
            profile: ProfileId::from(PROFILE),
            incognito: false,
            module_directory: module.path().to_owned(),
            state_directory: Some(state.to_owned()),
            locale: LocaleEnvironment {
                ui_language: "en-US".into(),
                accept_languages: vec!["en-US".into()],
                right_to_left: false,
            },
            time_scale: 1,
        },
        engine.clone(),
    )
    .expect("runtime");
    (
        Host {
            context,
            runtime,
            shell,
            dirty: Rc::new(Cell::new(false)),
        },
        engine,
    )
}

fn spec(directory: &Path, id: &str, generation: u64) -> InstallSpec {
    let manifest = Manifest::load(directory).expect("manifest");
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    InstallSpec {
        install: ExtensionInstallId::from(INSTALL),
        extension_id: ExtensionId::parse(id).expect("id"),
        generation: ExtensionRuntimeGeneration::new(generation).unwrap(),
        directory: directory.to_owned(),
        manifest,
        permissions: InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        ),
        allowed_in_incognito: false,
        externally_connectable: None,
        unpacked: false,
    }
}

fn context() -> webkit2gtk::WebContext {
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    context
}

fn pump(host: &mut Host, duration: Duration) {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        pump_for(Duration::from_millis(10));
        host.work();
    }
}

fn luminance(color: &str) -> Option<f64> {
    let inner = color.split_once('(')?.1.trim_end_matches(')');
    let parts: Vec<f64> = inner
        .split(',')
        .take(3)
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    (parts.len() == 3).then(|| (0.2126 * parts[0] + 0.7152 * parts[1] + 0.0722 * parts[2]) / 255.0)
}

/// Loads the fixture and returns each frame's background luminance.
fn frames(host: &mut Host, fixture: &Fixture, tab: ItemId) -> Vec<(String, f64)> {
    fixture.take();
    let view = host.shell.borrow().tabs[&tab].view.clone();
    view.load_uri(&fixture.url("/page"));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut seen: Vec<Value> = Vec::new();
    while Instant::now() < deadline && seen.len() < 4 {
        pump(host, Duration::from_millis(50));
        seen.extend(fixture.take());
    }
    let mut result: Vec<(String, f64)> = seen
        .iter()
        .map(|report| {
            (
                report["where"].as_str().unwrap_or("?").to_owned(),
                luminance(report["bg"].as_str().unwrap_or("")).unwrap_or(1.0),
            )
        })
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn evaluate(view: &webkit2gtk::WebView, body: &str) -> String {
    let result = Rc::new(RefCell::new(None));
    let observed = result.clone();
    view.call_async_javascript_function(
        body,
        None,
        None,
        None,
        None::<&gtk::gio::Cancellable>,
        move |value| {
            *observed.borrow_mut() = Some(
                value
                    .map(|value| value.to_string())
                    .unwrap_or_else(|error| format!("error: {error}")),
            );
        },
    );
    pump_until(|| result.borrow().is_some());
    let value = result.borrow_mut().take().unwrap();
    value
}

#[test]
#[ignore = "pinned packages and sandboxed native WebKitGTK; see the module documentation"]
fn dark_reader() {
    let Some(directory) = package(DARK_READER) else {
        panic!("set ZEPHIUM_WEBEXT_PINNED_DIR after scripts/webext/fetch-pinned.sh --extract");
    };
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let id = DARK_READER.0;
    {
        let (mut host, _engine) = host(context(), &module, state.path(), id);
        host.runtime
            .install_admitted(spec(&directory, id, 1))
            .expect("install");
        pump(&mut host, Duration::from_secs(2));
        let tab = host.open("about:blank", true);
        pump(&mut host, Duration::from_millis(300));
        let dark = frames(&mut host, &fixture, tab);
        emit(&json!({"darkReader": "installed", "frames": format!("{dark:?}")}));
        println!("WEBEXT dark_reader installed frames={dark:?}");
        println!(
            "WEBEXT dark_reader unhandled={:?} errors={:?} background={:?}",
            host.runtime
                .unhandled_calls(ExtensionInstallId::from(INSTALL)),
            host.runtime
                .errors(ExtensionInstallId::from(INSTALL))
                .iter()
                .map(|error| format!("{error:?}").chars().take(400).collect::<String>())
                .collect::<Vec<_>>(),
            host.runtime.background(ExtensionInstallId::from(INSTALL))
        );
        assert_eq!(dark.len(), 4, "every frame reported: {dark:?}");
        assert!(
            dark.iter().all(|(_, l)| *l < 0.3),
            "every frame is dark: {dark:?}"
        );

        // The popup opens and toggles the site through its own channel.
        let popup = host
            .runtime
            .open_page(ExtensionInstallId::from(INSTALL), PageKind::Popup, None)
            .expect("popup");
        let started = Instant::now();
        let mut text = String::new();
        while started.elapsed() < Duration::from_secs(15) {
            pump(&mut host, Duration::from_millis(200));
            text = evaluate(
                popup.view(),
                "return (document.body && document.body.innerText || '').length;",
            );
            if text.parse::<u64>().unwrap_or(0) > 20 {
                break;
            }
        }
        println!(
            "WEBEXT dark_reader popup={text} errors={:?}",
            host.runtime
                .errors(ExtensionInstallId::from(INSTALL))
                .iter()
                .map(|error| format!("{:?} {:?} {}", error.kind, error.context, error.message))
                .collect::<Vec<_>>()
        );
        assert!(
            text.split(',')
                .next()
                .and_then(|n| n.trim_start_matches('[').parse::<u64>().ok())
                .unwrap_or(0)
                > 30,
            "popup rendered"
        );
        let toggled = evaluate(
            popup.view(),
            "return chrome.runtime.sendMessage({type:'ui-bg-toggle-active-tab'}).then(()=> 'sent', e => 'error: ' + e.message);",
        );
        println!("WEBEXT dark_reader toggle={toggled}");
        drop(popup);
        pump(&mut host, Duration::from_secs(1));
        let toggled_frames = frames(&mut host, &fixture, tab);
        println!("WEBEXT dark_reader site_toggled frames={toggled_frames:?}");
        assert!(
            toggled_frames
                .iter()
                .any(|(where_, l)| where_ == "top" && *l > 0.7),
            "the toggled site is light"
        );
        let popup = host
            .runtime
            .open_page(ExtensionInstallId::from(INSTALL), PageKind::Popup, None)
            .expect("popup");
        pump(&mut host, Duration::from_secs(2));
        let _ = evaluate(popup.view(), "return chrome.runtime.sendMessage({type:'ui-bg-toggle-active-tab'}).then(()=> 'sent');");
        drop(popup);
        pump(&mut host, Duration::from_secs(1));

        host.close(tab);
        let mut drain = host.runtime.retire();
        assert!(drain.run(Duration::from_secs(10)));
    }
    // After a restart it is still dark; disabled, the page is light again.
    {
        let (mut host, _engine) = host(context(), &module, state.path(), id);
        host.runtime
            .install_enabled(spec(&directory, id, 2))
            .expect("restore");
        let tab = host.open("about:blank", true);
        pump(&mut host, Duration::from_millis(500));
        let restored = frames(&mut host, &fixture, tab);
        println!("WEBEXT dark_reader restored frames={restored:?}");
        assert!(
            restored
                .iter()
                .any(|(where_, l)| where_ == "top" && *l < 0.3),
            "dark after restart"
        );
        let mut stop = host
            .runtime
            .install_disabled(ExtensionInstallId::from(INSTALL));
        assert!(stop.run(Duration::from_secs(10)));
        let light = frames(&mut host, &fixture, tab);
        println!("WEBEXT dark_reader disabled frames={light:?}");
        assert!(
            light.iter().all(|(_, l)| *l > 0.7),
            "light when disabled: {light:?}"
        );
        println!(
            "WEBEXT dark_reader unhandled={:?} errors={}",
            host.runtime
                .unhandled_calls(ExtensionInstallId::from(INSTALL)),
            host.runtime.errors(ExtensionInstallId::from(INSTALL)).len()
        );
        host.close(tab);
        let mut drain = host.runtime.retire();
        assert!(drain.run(Duration::from_secs(10)));
    }
}

fn keys(sequence: &str) {
    let script = "import ctypes as c,ctypes.util,sys,os\n\
assert os.environ.get('DISPLAY') not in (None,':0',':0.0')\n\
x=c.CDLL(ctypes.util.find_library('xdo'))\n\
x.xdo_new.restype=c.c_void_p\n\
x.xdo_send_keysequence_window.argtypes=[c.c_void_p,c.c_ulong,c.c_char_p,c.c_uint]\n\
d=x.xdo_new(None)\n\
for k in sys.argv[1].split():\n    assert x.xdo_send_keysequence_window(d,0,k.encode(),12000)==0\n";
    let status = std::process::Command::new("python3")
        .args(["-I", "-c", script, sequence])
        .status()
        .expect("python3");
    assert!(status.success(), "key input");
}

#[test]
#[ignore = "pinned packages and sandboxed native WebKitGTK; see the module documentation"]
fn vimium() {
    let Some(directory) = package(VIMIUM) else {
        panic!("set ZEPHIUM_WEBEXT_PINNED_DIR after scripts/webext/fetch-pinned.sh --extract");
    };
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let id = VIMIUM.0;
    let (mut host, _engine) = host(context(), &module, state.path(), id);
    host.runtime
        .install_admitted(spec(&directory, id, 1))
        .expect("install");
    pump(&mut host, Duration::from_secs(2));
    let tab = host.open(&fixture.url("/vimium"), true);
    let view = host.shell.borrow().tabs[&tab].view.clone();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_default_size(900, 600);
    window.add(&view);
    window.show_all();
    window.present();
    view.grab_focus();
    pump(&mut host, Duration::from_secs(4));
    fixture.take();

    // One link in the page, inside a cross-origin frame: `f` shows one hint.
    keys("f");
    pump(&mut host, Duration::from_secs(1));
    keys("s");
    let started = Instant::now();
    let mut clicked = false;
    while started.elapsed() < Duration::from_secs(8) && !clicked {
        pump(&mut host, Duration::from_millis(100));
        clicked = fixture
            .take()
            .iter()
            .any(|report| report["where"] == "clicked");
    }
    println!("WEBEXT vimium hint_followed_cross_origin={clicked}");

    keys("Escape");
    host.shell.borrow().tabs[&tab]
        .view
        .load_uri(&fixture.url("/vimium"));
    pump(&mut host, Duration::from_secs(3));
    view.grab_focus();
    keys("o");
    let started = Instant::now();
    let mut visible = String::new();
    while started.elapsed() < Duration::from_secs(8) {
        pump(&mut host, Duration::from_millis(200));
        visible = evaluate(
            &view,
            "const w=[...document.querySelectorAll('.vimium-reset')].map(e=>e.shadowRoot&&e.shadowRoot.querySelector('iframe.vomnibar-frame')).find(Boolean);return w?w.className:'none';",
        );
        if visible.contains("vimium-ui-component-visible") {
            break;
        }
    }
    println!("WEBEXT vimium vomnibar={visible}");
    println!(
        "WEBEXT vimium unhandled={:?} errors={:?}",
        host.runtime
            .unhandled_calls(ExtensionInstallId::from(INSTALL)),
        host.runtime
            .errors(ExtensionInstallId::from(INSTALL))
            .iter()
            .map(|error| error.message.chars().take(160).collect::<String>())
            .collect::<Vec<_>>()
    );
    window.remove(&view);
    window.close();
    host.close(tab);
    let mut drain = host.runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    assert!(clicked, "the link hint followed the cross-origin link");
    assert!(
        visible.contains("vimium-ui-component-visible"),
        "the vomnibar opened"
    );
}
