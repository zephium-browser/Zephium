#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

//! Pinned public extensions on the WebKitGTK runtime: Dark Reader darkens
//! every frame of a light page and its popup toggles the site; Vimium's link
//! hints follow a link in a cross-origin frame and its vomnibar opens;
//! uBlock Origin Lite's default rulesets block a tracker host and its popup opens.
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
use webkit2gtk::{WebContextExt, WebViewExt, WebsiteDataManagerExt};
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
const UBOL: (&str, &str) = ("ddkjiahejlhfcafbddmgiahcphecmpfh", "2026.1110.1816");

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
                // Requests for tracker hosts arrive here through the test proxy.
                if path.contains("ubol-") {
                    collected.lock().unwrap().push(json!({"hit": path.clone()}));
                }
                if path.starts_with("/request-error") {
                    continue;
                }
                if path == "/delayed.js" {
                    std::thread::sleep(Duration::from_secs(2));
                    let body = "globalThis.fixtureParsed=true;";
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                let body = if path == "/delayed-form" {
                    let user_agent = text.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("user-agent")
                            .then(|| value.trim())
                    });
                    collected
                        .lock()
                        .unwrap()
                        .push(json!({"requestUserAgent":user_agent}));
                    "<!doctype html><html><head><script src='/delayed.js'></script></head><body><form><input autocomplete='username'></form></body></html>".into()
                } else if let Some(query) = path.strip_prefix("/report?") {
                    let decoded = url::form_urlencoded::parse(format!("q={query}").as_bytes())
                        .next()
                        .map(|(_, value)| value.into_owned())
                        .unwrap_or_default();
                    if let Ok(value) = serde_json::from_str(&decoded) {
                        collected.lock().unwrap().push(value);
                    }
                    String::new()
                } else if path.starts_with("/password-form") {
                    "<!doctype html><title>Password manager fixture</title><form action='/submitted' method='get'><label>Username<input name='username' autocomplete='username'></label><label>Password<input name='password' type='password' autocomplete='current-password'></label><button type='submit'>Sign in</button></form>".into()
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

#[test]
#[ignore = "pinned packages and sandboxed native WebKitGTK; see the module documentation"]
fn ubol() {
    let Some(directory) = package(UBOL) else {
        panic!("set ZEPHIUM_WEBEXT_PINNED_DIR after scripts/webext/fetch-pinned.sh --extract");
    };
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let id = UBOL.0;
    let context = context();
    // Tracker hosts resolve to the local fixture; loopback stays direct.
    let mut proxy =
        webkit2gtk::NetworkProxySettings::new(Some(&fixture.url("")), &["127.0.0.1", "localhost"]);
    context
        .website_data_manager()
        .unwrap()
        .set_network_proxy_settings(webkit2gtk::NetworkProxyMode::Custom, Some(&mut proxy));
    let (mut host, _engine) = host(context, &module, state.path(), id);
    let install = ExtensionInstallId::from(INSTALL);
    host.runtime.set_install_order(install, 1).expect("order");
    host.runtime
        .install_admitted(spec(&directory, id, 1))
        .expect("install");
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(60)
        && host
            .runtime
            .network_status(install)
            .is_none_or(|status| status.loading || status.active_rule_count == 0)
    {
        pump(&mut host, Duration::from_millis(100));
    }
    let status = host.runtime.network_status(install).expect("status");
    println!(
        "WEBEXT ubol rules={} unsupported={} error={:?}",
        status.active_rule_count, status.unsupported_rule_count, status.error
    );
    assert!(status.active_rule_count > 0, "default rulesets loaded");
    let tab = host.open(&fixture.url("/page"), true);
    pump(&mut host, Duration::from_secs(2));
    fixture.take();
    let view = host.shell.borrow().tabs[&tab].view.clone();
    let probe = |url: &str| {
        format!("try{{await fetch('{url}',{{mode:'no-cors',cache:'no-store'}});return 'loaded'}}catch(e){{return 'blocked'}}")
    };
    let tracker = evaluate(
        &view,
        &probe("http://doubleclick.net/dnr-resource?ubol-block"),
    );
    let control = evaluate(&view, &probe(&fixture.url("/ubol-control")));
    pump(&mut host, Duration::from_millis(300));
    let hits = fixture.take();
    println!("WEBEXT ubol tracker={tracker} control={control} hits={hits:?}");
    assert_eq!(tracker, "blocked", "default rules block the tracker");
    assert_eq!(control, "loaded", "the control request passes");
    assert!(
        !hits
            .iter()
            .any(|hit| hit["hit"].as_str().unwrap_or("").contains("ubol-block")),
        "the blocked request never reached the network: {hits:?}"
    );

    let popup = host
        .runtime
        .open_page(install, PageKind::Popup, None)
        .expect("popup");
    let started = Instant::now();
    let mut text = String::new();
    while started.elapsed() < Duration::from_secs(15) {
        pump(&mut host, Duration::from_millis(200));
        text = evaluate(
            popup.view(),
            "return (document.body && document.body.innerText || '').length;",
        );
        if text.parse::<u64>().unwrap_or(0) > 10 {
            break;
        }
    }
    println!("WEBEXT ubol popup_text_length={text}");
    assert!(text.parse::<u64>().unwrap_or(0) > 10, "popup rendered");
    drop(popup);
    host.close(tab);
    let mut drain = host.runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
}

fn password_package(id: &str, reference: &Value) -> PathBuf {
    let output = std::process::Command::new("python3")
        .args(["-I", "-c", r#"
import hashlib, io, json, os, pathlib, sys, tomllib, zipfile
pin = next(e for e in tomllib.load(open(sys.argv[1], 'rb'))['extension'] if e['id'] == sys.argv[2])
reference = json.loads(sys.argv[3])['package']
assert all(reference[k] == pin[k] for k in ['id','version','sha256']), 'Chromium reference pin is stale'
assert pin['version'] and pin['sha256'], 'unresolved package pin: run fetch-pinned.sh --pin-missing --extract'
root = pathlib.Path(os.environ['ZEPHIUM_WEBEXT_PINNED_DIR']) / (pin['id'] + '-' + pin['version'])
data = root.with_name(root.name + '.crx').read_bytes()
assert hashlib.sha256(data).hexdigest() == pin['sha256'], 'CRX digest mismatch'
assert data[:4] == b'Cr24' and int.from_bytes(data[4:8], 'little') == 3, 'expected CRX3'
archive = zipfile.ZipFile(io.BytesIO(data[12 + int.from_bytes(data[8:12], 'little'):]))
for name in archive.namelist():
    assert not name.startswith(('/', '\\')) and '..' not in name.replace('\\', '/').split('/'), 'unsafe package path'
    if not name.endswith('/'):
        file = root / name
        assert not file.is_symlink() and root.resolve() in file.resolve().parents, 'unsafe extracted path'
        assert file.read_bytes() == archive.read(name), 'extracted file differs from pin: ' + name
assert json.loads((root / 'manifest.json').read_text())['version'] == pin['version']
print(root)
"#])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/webext/pinned-crx.toml"))
        .arg(id)
        .arg(reference.to_string())
        .output()
        .expect("python3 pin verification");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
}

struct MappedPage(gtk::Window);

impl MappedPage {
    fn new(view: &webkit2gtk::WebView) -> Self {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_default_size(480, 640);
        window.add(view);
        window.show_all();
        window.present();
        Self(window)
    }
}

impl Drop for MappedPage {
    fn drop(&mut self) {
        if let Some(child) = self.0.child() {
            self.0.remove(&child);
        }
        self.0.close();
    }
}

struct DiagnosticDump(PathBuf);

impl Drop for DiagnosticDump {
    fn drop(&mut self) {
        if std::thread::panicking() {
            if let Ok(text) = std::fs::read_to_string(&self.0) {
                let lines: Vec<_> = text.lines().collect();
                eprintln!(
                    "diagnostics: {} (last 128 records)\n{}",
                    self.0.display(),
                    lines[lines.len().saturating_sub(128)..].join("\n")
                );
            }
        }
    }
}

fn diagnostic_rows(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn password_reference(id: &str) -> Value {
    [
        include_str!("../../../eval/webext-conformance/reference/password-managers/bitwarden.json"),
        include_str!(
            "../../../eval/webext-conformance/reference/password-managers/onepassword.json"
        ),
        include_str!(
            "../../../eval/webext-conformance/reference/password-managers/proton_pass.json"
        ),
    ]
    .into_iter()
    .map(|source| serde_json::from_str::<Value>(source).unwrap())
    .find(|reference| reference["package"]["id"] == id)
    .expect("pinned Chromium package reference")
}

fn reference_rejection(row: &Value, rows: &[Value], reference: &Value, used: &mut usize) -> bool {
    // bitwarden.json cites instrumented lines 73/145/153 and passive line 32.
    let Some(expected) = reference["backgroundUnhandled"]
        .as_array()
        .and_then(|items| items.first())
    else {
        return false;
    };
    let detail = &row["event"]["detail"];
    let matches = row["context"] == "Background"
        && detail["kind"] == "unhandled-rejection"
        && detail["message"]
            .as_str()
            .and_then(|text| text.lines().next())
            == expected["message"].as_str()
        && *used < expected["maximum"].as_u64().unwrap() as usize
        && rows
            .iter()
            .rev()
            .find(|call| {
                call["context"] == "Background"
                    && call["event"]["sender"] == row["event"]["sender"]
                    && call["sequence"].as_u64() < row["sequence"].as_u64()
                    && call["event"]["detail"]["kind"] == "api-failure"
                    && call["event"]["detail"]["message"] == expected["message"]
            })
            .is_some_and(|call| {
                let failure = &call["event"]["detail"];
                call["context"] == "Background"
                    && call["event"]["sender"] == row["event"]["sender"]
                    && call["sequence"].as_u64() < row["sequence"].as_u64()
                    && failure["kind"] == "api-failure"
                    && failure["namespace"] == "runtime"
                    && failure["method"] == "sendMessage"
                    && failure["command"] == expected["command"]
                    && failure["message"] == expected["message"]
            });
    if matches {
        *used += 1;
    }
    matches
}

fn console_message(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(|word| {
            if word.starts_with("https://") || word.starts_with("http://") {
                "<network-url>"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .replace("[network-url]", "<network-url>")
        .replace("TypeError: Load failed", "Load failed")
}

fn proxy_refusals(row: &Value, rows: &[Value], origin: &Value) -> usize {
    let context = if row["context"] == "Background" {
        "BackgroundOwner"
    } else {
        row["context"].as_str().unwrap_or_default()
    };
    rows.iter()
        .filter(|failure| {
            failure["context"] == context
                && failure["sequence"].as_u64() < row["sequence"].as_u64()
                && failure["event"]["kind"] == "resource-failed"
                && failure["event"]["resource"] == *origin
                && failure["event"]["domain"] == "g-io-error-quark"
                && failure["event"]["message"]
                    == "Could not connect to proxy server 127.0.0.1: Connection refused"
        })
        .count()
}

fn reference_offline(row: &Value, rows: &[Value], reference: &Value, used: &mut [usize]) -> bool {
    let event = &row["event"];
    let detail = if event["kind"] == "javascript" {
        &event["detail"]
    } else {
        event
    };
    for (index, expected) in reference["offlineErrors"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        if row["context"] != expected["context"]
            || detail["kind"] != expected["kind"]
            || used[index] >= expected["maximum"].as_u64().unwrap() as usize
        {
            continue;
        }
        // Each rule cites a real blocked request/console line in the reference.
        // Local package failures, other origins, other errors and extra calls fail.
        let matches = match detail["kind"].as_str() {
            Some("resource-failed" | "load-failed") => {
                detail["resource"] == expected["origin"]
                    && detail["domain"] == "g-io-error-quark"
                    && detail["message"]
                        == "Could not connect to proxy server 127.0.0.1: Connection refused"
            }
            Some("fetch-failed") => {
                detail["origin"] == expected["origin"]
                    && detail["message"] == "Load failed"
                    && proxy_refusals(row, rows, &expected["origin"]) > used[index]
            }
            Some("console-error") => {
                console_message(detail["message"].as_str().unwrap_or_default())
                    == expected["message"].as_str().unwrap()
                    && rows.iter().any(|failure| {
                        failure["context"] == row["context"]
                            && event["sender"].is_string()
                            && failure["event"]["sender"] == event["sender"]
                            && proxy_refusals(failure, rows, &expected["origin"]) > 0
                            && failure["sequence"].as_u64() < row["sequence"].as_u64()
                            && failure["event"]["detail"]["kind"] == "fetch-failed"
                            && failure["event"]["detail"]["origin"] == expected["origin"]
                            && failure["event"]["detail"]["message"] == "Load failed"
                    })
            }
            _ => false,
        };
        if matches {
            used[index] += 1;
            return true;
        }
    }
    false
}

#[test]
fn reference_offline_requires_the_recorded_origin_error_context_and_count() {
    let reference = password_reference("nngceckbapebfimnlniiiahkandclblb");
    let mut used = vec![0; reference["offlineErrors"].as_array().unwrap().len()];
    let row = json!({"context":"BackgroundOwner","event":{"kind":"resource-failed",
        "resource":"https://api.bitwarden.com","domain":"g-io-error-quark",
        "message":"Could not connect to proxy server 127.0.0.1: Connection refused"}});
    assert!(reference_offline(&row, &[], &reference, &mut used));
    assert!(!reference_offline(&row, &[], &reference, &mut used));
    for (key, value) in [
        ("resource", "https://unexpected.test"),
        ("resource", "chrome-extension://id/missing.js"),
        ("message", "Forbidden"),
        ("domain", "WebKitPolicyError"),
    ] {
        let mut changed = row.clone();
        changed["event"][key] = json!(value);
        assert!(!reference_offline(
            &changed,
            &[],
            &reference,
            &mut vec![0; used.len()]
        ));
    }
    let fetch = json!({"sequence":2,"context":"Background","event":{"kind":"javascript","sender":"worker-1","detail":{"kind":"fetch-failed","origin":"https://api.bitwarden.com","message":"Load failed"}}});
    assert!(!reference_offline(
        &fetch,
        &[],
        &reference,
        &mut vec![0; used.len()]
    ));
    let mut proxy = row.clone();
    proxy["sequence"] = json!(1);
    assert!(reference_offline(
        &fetch,
        &[proxy.clone()],
        &reference,
        &mut vec![0; used.len()]
    ));
    let mut console = json!({"sequence":3,"context":"Background","event":{"kind":"javascript","sender":"worker-2","detail":{"kind":"console-error","message":"Unable to fetch ServerConfig from <network-url> Load failed"}}});
    assert!(!reference_offline(
        &console,
        &[proxy.clone(), fetch.clone()],
        &reference,
        &mut vec![0; used.len()]
    ));
    console["event"]["sender"] = json!("worker-1");
    assert!(reference_offline(
        &console,
        &[proxy, fetch],
        &reference,
        &mut vec![0; used.len()]
    ));
    assert_eq!(console_message("Unable to fetch ServerConfig from https://api.bitwarden.com/config TypeError: Load failed"), "Unable to fetch ServerConfig from <network-url> Load failed");
    let mut changed = row;
    changed["context"] = json!("ContentScript");
    assert!(!reference_offline(
        &changed,
        &[],
        &reference,
        &mut vec![0; used.len()]
    ));
}

#[test]
fn reference_rejection_requires_the_same_command_context_and_single_error() {
    let reference = password_reference("nngceckbapebfimnlniiiahkandclblb");
    let message = reference["backgroundUnhandled"][0]["message"]
        .as_str()
        .unwrap();
    let call = json!({"sequence":1,"context":"Background","event":{"sender":"worker","detail":{
        "kind":"api-failure","namespace":"runtime","method":"sendMessage",
        "command":"getUserPremiumStatus","message":message}}});
    let rejection = json!({"sequence":2,"context":"Background","event":{"sender":"worker","detail":{
        "kind":"unhandled-rejection","message":format!("{message}\nworker.js:1:1")}}});
    let mut used = 0;
    assert!(reference_rejection(
        &rejection,
        std::slice::from_ref(&call),
        &reference,
        &mut used
    ));
    assert!(!reference_rejection(
        &rejection,
        std::slice::from_ref(&call),
        &reference,
        &mut used
    ));
    for (field, value) in [
        ("command", "syncStarted"),
        ("namespace", "tabs"),
        ("method", "connect"),
    ] {
        let mut other = call.clone();
        other["event"]["detail"][field] = json!(value);
        assert!(!reference_rejection(
            &rejection,
            &[other],
            &reference,
            &mut 0
        ));
    }
    let mut other = rejection.clone();
    other["context"] = json!("Popup");
    assert!(!reference_rejection(
        &other,
        std::slice::from_ref(&call),
        &reference,
        &mut 0
    ));
    other = rejection.clone();
    other["event"]["sender"] = json!("another worker");
    assert!(!reference_rejection(
        &other,
        std::slice::from_ref(&call),
        &reference,
        &mut 0
    ));
    let mut intervening = call.clone();
    intervening["sequence"] = json!(2);
    intervening["event"]["detail"]["command"] = json!("syncStarted");
    other = rejection.clone();
    other["sequence"] = json!(3);
    assert!(!reference_rejection(
        &other,
        &[call, intervening],
        &reference,
        &mut 0
    ));
}

fn password_manager(id: &str, name: &str, signal: &str) {
    use zephium_webext_linux::diagnostics;
    let evidence = PathBuf::from(
        std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").expect("set evidence directory"),
    );
    let diagnostic =
        DiagnosticDump(diagnostics::begin(&evidence, id).expect("fresh diagnostic file"));
    diagnostics::record(
        id,
        "host",
        json!({"kind":"popup-signal", "extensionName":name, "signal":signal}),
    );
    let reference = password_reference(id);
    let directory = password_package(id, &reference);
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let web_context = context();
    let mut proxy = webkit2gtk::NetworkProxySettings::new(
        Some("http://127.0.0.1:9"),
        &["127.0.0.1", "localhost"],
    );
    web_context
        .website_data_manager()
        .unwrap()
        .set_network_proxy_settings(webkit2gtk::NetworkProxyMode::Custom, Some(&mut proxy));
    let (mut host, _engine) = host(web_context, &module, state.path(), id);
    let install = ExtensionInstallId::from(INSTALL);
    let spec = spec(&directory, id, 1);
    let options_declared = spec.manifest.options_page().is_some();
    let fixture_url = url::Url::parse(&fixture.url("/password-form")).unwrap();
    let origin = zephium_webext_runtime::pattern::DocumentOrigin::of_url(&fixture_url);
    let registry = zephium_webext_runtime::scripting_registry::ScriptRegistry::from_manifest(
        &spec.manifest,
        |_, _| Ok(()),
    )
    .expect("manifest script registry");
    let expected_content: Vec<(String, usize)> = registry
        .injection_plan(
            &zephium_webext_runtime::pattern::FrameDocument {
                url: &fixture_url,
                origin: &origin,
                creator_url: None,
            },
            true,
            false,
            true,
        )
        .into_iter()
        .flat_map(|entry| {
            (0..entry.script.js.len())
                .map(|index| (entry.script.id.clone(), index))
                .collect::<Vec<_>>()
        })
        .collect();
    diagnostics::record(
        id,
        "ContentScript",
        json!({"kind":"expected-content", "files":expected_content}),
    );
    let mut failures = Vec::new();
    let observing = Instant::now();
    match host.runtime.install_admitted(spec) {
        Err(error) => failures.push(format!("install: {error}")),
        Ok(()) => {
            let tab = host.open("about:blank", true);
            let fixture_view = host.shell.borrow().tabs[&tab].view.clone();
            let fixture_window = MappedPage::new(&fixture_view);
            fixture_window.0.resize(1000, 700);
            fixture_view.load_uri(&fixture.url("/password-form"));
            pump(&mut host, Duration::from_secs(5));
            assert_eq!(
                evaluate(&fixture_view, "return document.visibilityState;"),
                "visible"
            );
            // Match the reference's settled installation page before clicking
            // an action that can navigate that same page again.
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let views: Vec<_> = host
                    .shell
                    .borrow()
                    .tabs
                    .values()
                    .filter(|tab| {
                        tab.url.as_ref().is_some_and(|url| {
                            url.scheme() == "chrome-extension" && url.host_str() == Some(id)
                        })
                    })
                    .map(|tab| tab.view.clone())
                    .collect();
                if views.iter().all(|view| {
                    !view.is_loading()
                        && evaluate(view, "return document.readyState === 'complete';") == "true"
                }) {
                    break;
                }
                if Instant::now() >= deadline {
                    failures.push("installation page did not finish loading before action".into());
                    break;
                }
                pump(&mut host, Duration::from_millis(200));
            }
            host.shell.borrow_mut().active = Some(tab);
            host.publish();
            match host.runtime.invoke_action(install, tab) {
                Ok(zephium_webext_linux::runtime::ActionActivation::Click) => {
                    assert_eq!(reference["action"], "click");
                    diagnostics::record(
                        id,
                        "host",
                        json!({"kind":"action-invoked","activation":"click"}),
                    );
                    pump(&mut host, Duration::from_secs(15));
                    let active = host.shell.borrow().active;
                    let views: Vec<_> = host
                        .shell
                        .borrow()
                        .tabs
                        .iter()
                        .filter(|(tab_id, tab)| {
                            Some(**tab_id) == active
                                && tab.url.as_ref().is_some_and(|url| {
                                    url.scheme() == "chrome-extension" && url.host_str() == Some(id)
                                })
                        })
                        .map(|(_, tab)| tab.view.clone())
                        .collect();
                    let mut interactive = false;
                    for view in views {
                        let _mapped = MappedPage::new(&view);
                        pump(&mut host, Duration::from_secs(5));
                        let state = evaluate(&view, "return JSON.stringify({ready:document.readyState,text:document.body?.innerText.slice(0,2048),path:location.pathname,controls:[...document.querySelectorAll('button,[role=button],a[href]')].filter(e=>e.getClientRects().length && !e.disabled).slice(0,24).map(e=>(e.getAttribute('aria-label')||e.textContent||'').trim().slice(0,120))});");
                        let ready = evaluate(&view, signal) == "true";
                        interactive |= ready;
                        diagnostics::record(
                            id,
                            "ExtensionTab",
                            json!({"kind":"action-tab-state","state":state,"interactive":ready}),
                        );
                    }
                    if !interactive {
                        failures.push("action did not open its interactive extension tab".into());
                    }
                }
                Ok(zephium_webext_linux::runtime::ActionActivation::Popup(path)) => {
                    assert_eq!(reference["action"], "popup");
                    diagnostics::record(
                        id,
                        "host",
                        json!({"kind":"action-invoked","activation":"popup","path":path}),
                    );
                }
                Err(error) => failures.push(format!("action: {error}")),
            }
            host.shell.borrow_mut().active = Some(tab);
            host.publish();
            match host.runtime.open_page(install, PageKind::Popup, None) {
                Err(error) => failures.push(format!("popup: {error:?}")),
                Ok(popup) => {
                    let _mapped = MappedPage::new(popup.view());
                    diagnostics::record(
                        id,
                        "Popup",
                        json!({"kind":"view-mapped","mapped":popup.view().is_mapped()}),
                    );
                    let reference_empty = reference["forcedPopup"]["interactive"] == false;
                    let deadline =
                        Instant::now() + Duration::from_secs(if reference_empty { 5 } else { 45 });
                    let mut ready = false;
                    while Instant::now() < deadline {
                        pump(&mut host, Duration::from_millis(200));
                        let result = evaluate(popup.view(), signal);
                        if result.starts_with("error:") {
                            failures.push(format!("popup signal: {result}"));
                            break;
                        }
                        if result == "true" {
                            ready = true;
                            break;
                        }
                    }
                    diagnostics::record(
                        id,
                        "Popup",
                        json!({"kind":"popup-result", "interactive":ready}),
                    );
                    let controls = evaluate(
                        popup.view(),
                        r#"return JSON.stringify([...document.querySelectorAll('button,[role="button"]')].filter(e=>e.getClientRects().length && !e.disabled).slice(0,24).map(e=>(e.getAttribute('aria-label')||e.textContent||'').trim().slice(0,120)));"#,
                    );
                    diagnostics::record(
                        id,
                        "Popup",
                        json!({"kind":"visible-controls", "labels":controls}),
                    );
                    if !ready {
                        let state = evaluate(popup.view(), "return JSON.stringify({ready:document.readyState,text:document.body?.innerText.slice(0,2048),children:document.body?.children.length,html:document.body?.innerHTML.slice(0,4096),visibility:document.visibilityState,path:location.pathname});");
                        diagnostics::record(
                            id,
                            "Popup",
                            json!({"kind":"popup-state","state":state}),
                        );
                        // onepassword.json: passive lines 349/505 prove welcome-tab
                        // action and a complete, empty forced manifest popup.
                        let state: Value = serde_json::from_str(&state).unwrap_or(Value::Null);
                        let matches_reference = reference_empty
                            && state["ready"] == "complete"
                            && state["text"] == reference["forcedPopup"]["text"]
                            && serde_json::from_str::<Value>(&controls).ok().as_ref()
                                == Some(&reference["forcedPopup"]["controls"]);
                        if !matches_reference {
                            failures.push(
                                "popup did not match its Chromium first-screen reference".into(),
                            );
                        }
                    }
                    pump(&mut host, Duration::from_secs(2));
                }
            }
            if options_declared {
                match host.runtime.open_page(install, PageKind::Options, None) {
                    Err(error) => failures.push(format!("options: {error:?}")),
                    Ok(options) => {
                        let _mapped = MappedPage::new(options.view());
                        let deadline = Instant::now() + Duration::from_secs(15);
                        let mut ready = false;
                        while Instant::now() < deadline {
                            pump(&mut host, Duration::from_millis(200));
                            if evaluate(options.view(), "return document.readyState === 'complete' && !!document.body && document.body.children.length > 0;") == "true" {
                                ready = true;
                                break;
                            }
                        }
                        diagnostics::record(
                            id,
                            "Options",
                            json!({"kind":"options-result", "loaded":ready}),
                        );
                        if !ready {
                            failures.push("declared options page did not load".into());
                        }
                    }
                }
            } else {
                diagnostics::record(id, "Options", json!({"kind":"options-not-declared"}));
            }
            pump(
                &mut host,
                Duration::from_secs(22).saturating_sub(observing.elapsed()),
            );
            host.close(tab);
        }
    }
    let runtime_errors = host.runtime.errors(install);
    let rows = diagnostic_rows(&diagnostic.0);
    let api_rows = diagnostic_rows(&diagnostic.0.with_extension("api.jsonl"));
    if !api_rows.iter().any(|row| {
        row["context"] == "Background"
            && row["event"]["detail"]["complete"] == true
            && row["event"]["detail"]["durationMs"] == 20000
    }) {
        failures.push("first 20 seconds of background APIs were not fully captured".into());
    }
    if !rows
        .iter()
        .any(|row| row["event"]["kind"] == "background-started")
    {
        failures.push("background never reached started".into());
    }
    if !rows.iter().any(|row| {
        row["event"]["kind"] == "content-evaluation"
            && row["event"]["accepted"] == true
            && row["event"]["success"] == true
            && row["event"]["fileIndex"]
                .as_u64()
                .is_some_and(|index| index < u64::from(u16::MAX))
    }) {
        failures.push("no content-script file successfully evaluated on the localhost form".into());
    }
    for (script, index) in &expected_content {
        if !rows.iter().any(|row| {
            row["event"]["kind"] == "content-evaluation"
                && row["event"]["accepted"] == true
                && row["event"]["success"] == true
                && row["event"]["script"] == *script
                && row["event"]["fileIndex"].as_u64() == Some(*index as u64)
        }) {
            failures.push(format!(
                "matching content script {script} file {index} did not evaluate"
            ));
        }
    }
    let mut reference_rejections = 0;
    let mut offline_used = vec![0; reference["offlineErrors"].as_array().unwrap().len()];
    let mut reference_console = Vec::new();
    for row in &rows {
        if reference_rejection(row, &rows, &reference, &mut reference_rejections) {
            continue;
        }
        if reference_offline(row, &rows, &reference, &mut offline_used) {
            if row["event"]["detail"]["kind"] == "console-error" {
                reference_console.push((
                    row["context"].as_str().unwrap().to_owned(),
                    console_message(row["event"]["detail"]["message"].as_str().unwrap()),
                ));
            }
            continue;
        }
        let event = &row["event"];
        let kind = if event["kind"] == "javascript" {
            event["detail"]["kind"].as_str()
        } else {
            event["kind"].as_str()
        };
        if matches!(
            kind,
            Some(
                "dispatcher-refusal"
                    | "delivery-failed"
                    | "bridge-refusal"
                    | "bridge-delivery-refused"
                    | "unknown-method"
                    | "invalid-diagnostic"
                    | "uncaught"
                    | "unhandled-rejection"
                    | "console-error"
                    | "api-refusal"
                    | "csp"
                    | "resource-error"
                    | "resource-failed"
                    | "resource-http-error"
                    | "load-failed"
                    | "fetch-failed"
                    | "fetch-http-error"
                    | "capture-truncated"
                    | "context-truncated"
                    | "record-too-large"
            )
        ) || (kind == Some("api-failure") && event["detail"]["method"] == "setIcon")
            || (kind == Some("content-evaluation") && event["success"] == false)
        {
            failures.push(format!(
                "diagnostic {} ({})",
                kind.unwrap_or("unknown"),
                row["context"]
            ));
        }
    }
    let mut remaining_reference_errors = reference_rejections;
    for error in runtime_errors {
        if error.kind == zephium_webext_linux::runtime::ExtensionErrorKind::Console {
            if let Some(index) = reference_console.iter().position(|(context, message)| {
                *context == format!("{:?}", error.context)
                    && *message == console_message(&error.message)
            }) {
                reference_console.remove(index);
                continue;
            }
        }
        let matches = remaining_reference_errors > 0
            && error.kind == zephium_webext_linux::runtime::ExtensionErrorKind::UnhandledRejection
            && error.context == zephium_webext_runtime::context::ContextKind::Background
            && reference["backgroundUnhandled"]
                .as_array()
                .is_some_and(|items| {
                    items.iter().any(|expected| {
                        error.message.strip_prefix("Uncaught (in promise) Error: ")
                            == expected["message"].as_str()
                    })
                });
        if matches {
            remaining_reference_errors -= 1;
        } else {
            failures.push(format!("runtime {:?} in {:?}", error.kind, error.context));
        }
    }
    let api = std::fs::read_to_string(diagnostic.0.with_extension("api.jsonl")).unwrap();
    if !api
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|row| row["event"]["detail"]["complete"] == true)
    {
        failures.push("first 20 seconds of background API trace incomplete".into());
    }
    diagnostics::record(
        id,
        "host",
        json!({"kind":"acceptance", "failures":failures}),
    );
    let tabs = host.shell.borrow().order.clone();
    for tab in tabs {
        host.close(tab);
    }
    let mut drain = host.runtime.retire();
    let drained = drain.run(Duration::from_secs(10));
    assert!(drained, "runtime teardown did not drain");
    assert!(
        failures.is_empty(),
        "{name}: {}\ndiagnostics: {}",
        failures.join("; "),
        diagnostic.0.display()
    );
}

// Signals require visible, enabled signed-out controls, not a spinner/root or error link.
const VISIBLE_CONTROLS: &str = r#"
const visible = e => !!e && e.getClientRects().length > 0 && getComputedStyle(e).visibility !== 'hidden' && !e.disabled;
const buttons = [...document.querySelectorAll('button,[role="button"],a[href]')].filter(visible);
const labels = buttons.map(e => (e.getAttribute('aria-label') || e.textContent || '').trim());
const text = document.body?.innerText || '';
if (/something went wrong|browser (?:is )?not supported|webassembly.*(?:failed|unavailable)|unexpected error/i.test(text)) return false;
"#;

#[test]
#[ignore = "pinned package, diagnostic module and private WebKitGTK session"]
fn bitwarden() {
    password_manager("nngceckbapebfimnlniiiahkandclblb", "Bitwarden", &format!("{VISIBLE_CONTROLS} return (visible(document.querySelector('autofill-default-password-manager-prompt #default-password-manager-prompt_button_continue')) && visible(document.querySelector('autofill-default-password-manager-prompt #default-password-manager-prompt_button_skip-for-now'))) || (visible(document.querySelector('[data-testid=login-email-input]')) && visible(document.querySelector('[data-testid=login-continue-button]'))) || (/bitwarden/i.test(text) && labels.some(s => /^(get started|log in|unlock)$/i.test(s)));"));
}

#[test]
#[ignore = "pinned package, diagnostic module and private WebKitGTK session"]
fn onepassword() {
    password_manager("aeblfdkhhhdcdjpifhhbdiojplfjncoa", "1Password", &format!("{VISIBLE_CONTROLS} return (/1password/i.test(text) && labels.some(s => /^(sign in|sign in to 1password|add an account|create an account|unlock 1password|get started)$/i.test(s))) || (visible(document.querySelector('h1')) && /^Welcome to 1Password!?$/.test(document.querySelector('h1').textContent.trim()) && labels.includes('Continue'));"));
}

#[test]
#[ignore = "pinned package, diagnostic module and private WebKitGTK session"]
fn proton_pass() {
    password_manager("ghmbeldphafepmbegfdlkpapadhbakde", "Proton Pass", &format!("{VISIBLE_CONTROLS} return visible(document.querySelector('.app-root .pass-lobby--heading')) && labels.includes('Sign in with Proton') && labels.includes('Create a Proton account');"));
}

#[test]
#[ignore = "diagnostic module and private WebKitGTK session"]
fn diagnostic_capture() {
    use zephium_webext_linux::diagnostics;
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let evidence = PathBuf::from(
        std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").expect("evidence directory"),
    );
    let diagnostic = DiagnosticDump(diagnostics::begin(&evidence, id).unwrap());
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    std::fs::write(package.path().join("manifest.json"), json!({
        "manifest_version":3,"name":"Diagnostic fixture","version":"1.0",
        "permissions":["bookmarks"], "host_permissions":["http://127.0.0.1/*"],
        "background":{"service_worker":"worker.js"},"action":{"default_popup":"popup.html"},
        "options_ui":{"page":"options.html"},
        "content_scripts":[{"matches":["http://127.0.0.1/*"],"js":["content.js"],"run_at":"document_start"}]
    }).to_string()).unwrap();
    let probe = "console.warn('diagnostic warning');void chrome.diagnosticMissing;void browser.runtime.diagnosticMissing;setTimeout(()=>{Promise.reject(new Error('diagnostic rejection'));},10);setTimeout(()=>{throw new Error('diagnostic async exception');},20);";
    for file in ["worker.js", "page.js"] {
        std::fs::write(
            package.path().join(file),
            format!("{probe}chrome.runtime.getPackageDirectoryEntry(()=>{{void chrome.runtime.lastError;}});"),
        ).unwrap();
    }
    std::fs::write(
        package.path().join("content.js"),
        format!("{probe}\nthrow new Error('diagnostic content exception');"),
    )
    .unwrap();
    for file in ["popup.html", "options.html"] {
        std::fs::write(package.path().join(file), "<!doctype html><title>Diagnostic</title><script src='page.js'></script><script>void 0;</script><img src='missing.png'><button>Ready</button>").unwrap();
    }
    let (mut host, _engine) = host(context(), &module, state.path(), id);
    let install = ExtensionInstallId::from(INSTALL);
    host.runtime
        .install_admitted(spec(package.path(), id, 1))
        .unwrap();
    let tab = host.open(&fixture.url("/password-form"), true);
    let popup = host
        .runtime
        .open_page(install, PageKind::Popup, None)
        .unwrap();
    let options = host
        .runtime
        .open_page(install, PageKind::Options, None)
        .unwrap();
    pump(&mut host, Duration::from_secs(5));
    let rows = diagnostic_rows(&diagnostic.0);
    drop(popup);
    drop(options);
    host.close(tab);
    let mut drain = host.runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    for context in ["Background", "Popup", "Options", "ContentScript"] {
        for kind in [
            "context-ready",
            "undefined-api",
            "console-warn",
            "unhandled-rejection",
        ] {
            assert!(
                rows.iter()
                    .any(|row| row["context"] == context && row["event"]["detail"]["kind"] == kind),
                "missing {context} {kind}"
            );
        }
    }
    assert!(rows
        .iter()
        .any(|row| row["event"]["kind"] == "background-started"));
    assert!(rows
        .iter()
        .any(|row| row["event"]["kind"] == "background-wake"));
    assert!(rows
        .iter()
        .any(|row| row["event"]["detail"]["kind"] == "api-refusal"));
    assert!(rows
        .iter()
        .any(|row| row["event"]["detail"]["kind"] == "csp"));
    assert!(rows
        .iter()
        .any(|row| row["event"]["detail"]["kind"] == "uncaught"
            && row["event"]["detail"]["stack"]
                .as_str()
                .is_some_and(|stack| stack.contains("page.js"))));
    assert!(rows.iter().any(
        |row| row["event"]["kind"] == "content-evaluation" && row["event"]["success"] == false
    ));
    assert!(rows
        .iter()
        .any(|row| row["event"]["kind"] == "resource-failed"
            || row["event"]["kind"] == "resource-http-error"
            || row["event"]["detail"]["kind"] == "resource-error"));
}

#[test]
#[ignore = "diagnostic module and private WebKitGTK session"]
fn runtime_compatibility() {
    use zephium_webext_linux::diagnostics;
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let evidence = PathBuf::from(
        std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").expect("evidence directory"),
    );
    let diagnostic = DiagnosticDump(diagnostics::begin(&evidence, id).unwrap());
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    std::fs::write(package.path().join("manifest.json"), json!({
        "manifest_version":3,"name":"Runtime compatibility fixture","version":"1.0",
        "permissions":["scripting","sidePanel","contextMenus","nativeMessaging"], "host_permissions":["http://127.0.0.1/*"],
        "background":{"service_worker":"worker.js"},"action":{"default_popup":"popup.html"},
        "content_security_policy":{"extension_pages":"script-src 'self'; object-src 'self'; connect-src 'none'"},
        "content_scripts":[{"matches":["http://127.0.0.1/*"],"js":["content.js"],"run_at":"document_start"}]
    }).to_string()).unwrap();
    std::fs::write(package.path().join("build.info"), "package-only").unwrap();
    std::fs::write(package.path().join("worker.js"), r#"
const packageReady = fetch(chrome.runtime.getURL('build.info')).then(r=>r.text());
chrome.contextMenus.onClicked.addListener(()=>{});
const configurationReady = Promise.all([chrome.contextMenus.removeAll(), chrome.sidePanel.setOptions({enabled:false})]);
const nativeReady = chrome.runtime.sendNativeMessage('org.example.absent', {}).then(()=> 'unexpected success', e=>e.message);
const portReady = new Promise(resolve=>{ const port=chrome.runtime.connectNative('org.example.absent'); port.onDisconnect.addListener(()=>resolve(chrome.runtime.lastError?.message)); });
chrome.runtime.onMessage.addListener((message,sender,respond)=>{
  (async()=>{
    const target={tabId:sender.tab.id,frameIds:[sender.frameId]};
    const immediate=await chrome.scripting.executeScript({target,injectImmediately:true,func:()=>document.readyState});
    const idle=await chrome.scripting.executeScript({target,func:()=>document.readyState});
    const main=await chrome.scripting.executeScript({target,world:'MAIN',func:()=>({
      userAgent:navigator.userAgent,chrome:typeof globalThis.chrome,browser:typeof globalThis.browser,
      nativePost:typeof globalThis.__zephiumPost,nativeReceiver:typeof globalThis.__zephiumRegisterReceiver
    })});
    await configurationReady;
    respond({main:main[0].result,immediate:immediate[0].result,idle:idle[0].result,package:await packageReady,workerUA:navigator.userAgent,
      panel:await chrome.sidePanel.getOptions({}),native:await nativeReady,port:await portReady});
  })().catch(e=>respond({error:String(e)}));
  return true;
});
"#).unwrap();
    std::fs::write(package.path().join("content.js"), r#"
chrome.runtime.sendMessage({probe:true}, result=>{
 document.documentElement.dataset.compatibility=JSON.stringify({...result,contentUA:navigator.userAgent});
});
"#).unwrap();
    std::fs::write(
        package.path().join("popup.html"),
        "<!doctype html><button>Ready</button>",
    )
    .unwrap();
    let (mut host, _engine) = host(context(), &module, state.path(), id);
    let install = ExtensionInstallId::from(INSTALL);
    host.runtime
        .install_admitted(spec(package.path(), id, 1))
        .unwrap();
    pump(&mut host, Duration::from_secs(2));
    let tab = host.open(&fixture.url("/delayed-form"), true);
    let view = host.shell.borrow().tabs[&tab].view.clone();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut result = Value::Null;
    while Instant::now() < deadline {
        pump(&mut host, Duration::from_millis(100));
        if let Ok(value) = serde_json::from_str::<Value>(&evaluate(
            &view,
            "return document.documentElement.dataset.compatibility || 'null';",
        )) {
            if !value.is_null() {
                result = value;
                break;
            }
        }
    }
    let popup = host
        .runtime
        .open_page(install, PageKind::Popup, None)
        .unwrap();
    pump(&mut host, Duration::from_secs(1));
    let popup_ua = evaluate(popup.view(), "return navigator.userAgent;");
    let main_ua = evaluate(&view, "return navigator.userAgent;");
    let remote = evaluate(
        popup.view(),
        &format!(
            "try{{await fetch('{}');return 'allowed'}}catch(e){{return 'blocked'}}",
            fixture.url("/password-form")
        ),
    );
    diagnostics::record(
        id,
        "host",
        json!({"kind":"compatibility-result","result":result,"popupUA":popup_ua,"mainUA":main_ua,"remoteFetch":remote}),
    );
    drop(popup);
    host.close(tab);
    let mut drain = host.runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    assert_eq!(result["package"], "package-only");
    assert_eq!(result["panel"]["enabled"], false);
    for key in ["native", "port"] {
        assert_eq!(result[key], "Specified native messaging host not found.");
    }
    assert_eq!(result["immediate"], "loading");
    assert!(
        matches!(result["idle"].as_str(), Some("interactive" | "complete")),
        "{result}"
    );
    for key in ["workerUA", "contentUA"] {
        assert!(
            result[key]
                .as_str()
                .is_some_and(|ua| ua.contains(" Chrome/") && ua.contains("AppleWebKit")),
            "{result}"
        );
    }
    assert!(popup_ua.contains(" Chrome/"));
    assert!(!main_ua.contains("ZephiumWebExtensions/"));
    assert_eq!(result["main"]["userAgent"], main_ua);
    for key in ["chrome", "browser", "nativePost", "nativeReceiver"] {
        assert_eq!(result["main"][key], "undefined", "{key}");
    }
    let requests = fixture.take();
    assert!(requests
        .iter()
        .any(|row| row["requestUserAgent"] == main_ua));
    for row in diagnostic_rows(&diagnostic.0) {
        let detail = &row["event"]["detail"];
        if detail["kind"] == "compatibility-identity" {
            for key in ["appVersion", "vendor", "platform", "userAgentData"] {
                assert_eq!(detail["before"][key], detail["after"][key], "{row}");
            }
        }
    }
    assert_eq!(remote, "blocked");
    assert!(diagnostic_rows(&diagnostic.0)
        .iter()
        .any(|row| row["event"]["kind"] == "background-started"));
}

#[test]
#[ignore = "diagnostic module and private WebKitGTK session"]
fn platform_capabilities() {
    use zephium_webext_linux::diagnostics;
    use zephium_webext_runtime::api::context_menus::MenuContext;
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let evidence = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").unwrap());
    let _diagnostic = DiagnosticDump(diagnostics::begin(&evidence, id).unwrap());
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    std::fs::write(package.path().join("manifest.json"), json!({
        "manifest_version":3,"name":"Platform capabilities","version":"1.0",
        "permissions":["tabs","webRequest","contextMenus","notifications"],"host_permissions":["<all_urls>"],
        "background":{"service_worker":"worker.js"},"action":{"default_popup":"popup.html"},
        "content_scripts":[{"matches":["http://127.0.0.1/*"],"js":["empty.js"]},{"matches":["http://127.0.0.1/*"],"js":["main.js"],"world":"MAIN"}]
    }).to_string()).unwrap();
    std::fs::write(
        package.path().join("main.js"),
        "document.documentElement.dataset.mainIdle=typeof requestIdleCallback;",
    )
    .unwrap();
    std::fs::write(package.path().join("empty.js"), r#"
(async()=>{
 let cancelled=false;
 const handle=requestIdleCallback(()=>cancelled=true);
 cancelIdleCallback(handle);
 const idle=await new Promise(resolve=>requestIdleCallback(deadline=>resolve({didTimeout:deadline.didTimeout,remaining:deadline.timeRemaining()})));
 const timed=await new Promise(resolve=>{
   requestIdleCallback(deadline=>resolve({didTimeout:deadline.didTimeout,remaining:deadline.timeRemaining()}),{timeout:1});
   const until=performance.now()+20;while(performance.now()<until){}
 });
 document.documentElement.dataset.isolatedIdle=JSON.stringify({idle,timed,cancelled});
})().catch(error=>document.documentElement.dataset.isolatedIdle=JSON.stringify({error:String(error)}));
"#).unwrap();
    let icon =
        gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 2, 2).unwrap();
    icon.fill(0xff00ffff);
    icon.savev(package.path().join("icon.png"), "png", &[])
        .unwrap();
    std::fs::write(
        package.path().join("popup.html"),
        "<!doctype html><title>Own package page</title><button>Ready</button>",
    )
    .unwrap();
    std::fs::write(package.path().join("worker.js"), r#"
const seen=[]; let clicks=0, matchingWindow=0, foreignWindow=0, iconResult=null;
(async()=>{
 await chrome.action.setIcon({path:'./icon.png'});
 let missing=false, foreign=false;
 try{await chrome.action.setIcon({path:'./missing.png'});}catch{missing=true;}
 try{await chrome.action.setIcon({path:'chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/icon.png'});}catch{foreign=true;}
 iconResult={loaded:true,missing,foreign};
})().catch(error=>iconResult={error:error.message});
chrome.windows.getAll().then(windows=>{
 chrome.webRequest.onBeforeRequest.addListener(()=>matchingWindow++,{urls:['http://127.0.0.1/*'],windowId:windows[0].id});
 chrome.webRequest.onBeforeRequest.addListener(()=>foreignWindow++,{urls:['http://127.0.0.1/*'],windowId:windows[0].id+1000});
});
for(const event of ['onBeforeRequest','onBeforeSendHeaders','onHeadersReceived','onCompleted','onErrorOccurred']) {
 const extra=event==='onBeforeSendHeaders'?['requestHeaders']:event==='onHeadersReceived'?['responseHeaders']:[];
 chrome.webRequest[event].addListener(details=>seen.push({event,id:details.requestId,type:details.type,headers:details.responseHeaders}),{urls:['http://127.0.0.1/*']},extra);
}
chrome.contextMenus.create({id:'fill',title:'Fill',contexts:['editable']});
chrome.contextMenus.onClicked.addListener(info=>{if(info.menuItemId==='fill')clicks++;});
chrome.runtime.onMessage.addListener((message,sender,respond)=>{ if(message.report)respond({seen,clicks,matchingWindow,foreignWindow,iconResult}); });
"#).unwrap();
    let plain = webkit2gtk::WebView::with_context(&context());
    plain.load_uri(&fixture.url("/password-form"));
    pump_for(Duration::from_secs(1));
    let default_idle = evaluate(&plain, "return typeof requestIdleCallback;");
    assert_eq!(
        default_idle, "undefined",
        "update this probe when WebKit enables idle callbacks by default"
    );
    drop(plain);
    let (mut host, _engine) = host(context(), &module, state.path(), id);
    let _initial_tab = host.open("about:blank", true);
    let install = ExtensionInstallId::from(INSTALL);
    host.runtime
        .install_admitted(spec(package.path(), id, 1))
        .unwrap();
    pump(&mut host, Duration::from_secs(2));
    let tab = host.open(&fixture.url("/password-form"), true);
    pump(&mut host, Duration::from_secs(2));
    let tab_view = host.shell.borrow().tabs[&tab].view.clone();
    assert_eq!(
        evaluate(&tab_view, "return typeof requestIdleCallback;"),
        default_idle
    );
    assert_eq!(
        evaluate(&tab_view, "return typeof cancelIdleCallback;"),
        "undefined"
    );
    assert_eq!(
        evaluate(
            &tab_view,
            "return document.documentElement.dataset.mainIdle;"
        ),
        default_idle
    );
    let isolated: Value = serde_json::from_str(&evaluate(
        &tab_view,
        "return document.documentElement.dataset.isolatedIdle;",
    ))
    .unwrap();
    assert_eq!(isolated["cancelled"], false, "{isolated}");
    assert_eq!(isolated["idle"]["didTimeout"], false, "{isolated}");
    assert!(
        isolated["idle"]["remaining"]
            .as_f64()
            .is_some_and(|v| (0.0..=50.0).contains(&v)),
        "{isolated}"
    );
    assert_eq!(
        isolated["timed"],
        json!({"didTimeout":true,"remaining":0}),
        "{isolated}"
    );
    drop(tab_view);
    let menu_context = MenuContext {
        menu_generation: host.runtime.context_menu_generation(),
        document_id: host.runtime.context_menu_document(tab),
        page_url: fixture.url("/password-form"),
        editable: true,
        ..Default::default()
    };
    assert_eq!(host.runtime.context_menu_items(tab, &menu_context).len(), 1);
    host.runtime
        .context_menu_clicked(install, tab, "fill", &menu_context)
        .unwrap();
    let mut stale = menu_context.clone();
    stale.document_id = Some("stale-document".into());
    assert!(host
        .runtime
        .context_menu_clicked(install, tab, "fill", &stale)
        .is_err());
    let failed_tab = host.open(&fixture.url("/request-error"), false);
    pump(&mut host, Duration::from_secs(2));
    host.close(failed_tab);
    let popup = host
        .runtime
        .open_page(install, PageKind::Popup, None)
        .unwrap();
    pump(&mut host, Duration::from_secs(1));
    assert_eq!(
        evaluate(
            popup.view(),
            r#"globalThis.result=null;(async()=>{
 const nativeIdle=Function.prototype.toString.call(requestIdleCallback).includes('[native code]');
 const created=await chrome.tabs.create({url:'popup.html#created'});
 await chrome.tabs.update(created.id,{url:'popup.html#updated'});
 const window=await chrome.windows.create({url:'popup.html#window',type:'popup'});
 let cancelledIdle=false;
 const cancelled=requestIdleCallback(()=>cancelledIdle=true);
 cancelIdleCallback(cancelled);
 const idle=await new Promise(resolve=>requestIdleCallback(deadline=>resolve({didTimeout:deadline.didTimeout,remaining:deadline.timeRemaining(),cancelled:cancelledIdle}),{timeout:1000}));
 const image=new Image();
 const inlineImage=new Promise(resolve=>{image.onload=()=>resolve(image.naturalWidth===1);image.onerror=()=>resolve(false);});
 image.src='data:image/svg+xml;base64,'+btoa('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><!--'+'x'.repeat(20000)+'--><rect width="1" height="1"/></svg>');
 let foreign=false;try{await chrome.tabs.create({url:'chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/private.html'});}catch{foreign=true;}
 globalThis.result={created,window,foreign,idle,nativeIdle,inlineImage:await inlineImage,clear:await chrome.notifications.clear('missing'),all:await chrome.notifications.getAll(),report:await chrome.runtime.sendMessage({report:true})};
})().catch(e=>globalThis.result={error:e.message});return 'started';"#
        ),
        "started"
    );
    pump(&mut host, Duration::from_secs(4));
    let result: Value = serde_json::from_str(&evaluate(
        popup.view(),
        "return JSON.stringify(globalThis.result);",
    ))
    .unwrap();
    diagnostics::record(
        id,
        "host",
        json!({"kind":"platform-capabilities","result":result}),
    );
    assert!(result.get("error").is_none(), "{result}");
    assert_eq!(result["nativeIdle"], true);
    assert_eq!(result["foreign"], true);
    assert_eq!(result["inlineImage"], true);
    assert_eq!(result["idle"]["cancelled"], false);
    assert!(result["idle"]["didTimeout"].is_boolean());
    assert!(result["idle"]["remaining"]
        .as_f64()
        .is_some_and(|value| (0.0..=50.0).contains(&value)));
    assert_eq!(result["window"]["type"], "normal");
    assert_eq!(result["clear"], false);
    assert_eq!(result["all"], json!({}));
    assert_eq!(result["report"]["clicks"], 1);
    assert_eq!(
        result["report"]["iconResult"],
        json!({"loaded":true,"missing":true,"foreign":true})
    );
    assert!(result["report"]["matchingWindow"].as_u64().unwrap() >= 2);
    assert_eq!(result["report"]["foreignWindow"], 0);
    for event in [
        "onBeforeRequest",
        "onBeforeSendHeaders",
        "onHeadersReceived",
        "onCompleted",
        "onErrorOccurred",
    ] {
        assert!(
            result["report"]["seen"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["event"] == event),
            "missing {event}: {result}"
        );
    }
    // An extension document in an ordinary tab keeps that view's defaults,
    // including after it navigates back to a web page.
    let package_tab = host.open(&format!("chrome-extension://{id}/popup.html"), true);
    pump(&mut host, Duration::from_secs(1));
    let package_view = host.shell.borrow().tabs[&package_tab].view.clone();
    assert_eq!(
        evaluate(&package_view, "return location.protocol;"),
        "chrome-extension:"
    );
    assert_eq!(
        evaluate(&package_view, "return typeof requestIdleCallback;"),
        default_idle
    );
    package_view.load_uri(&fixture.url("/password-form"));
    pump(&mut host, Duration::from_secs(1));
    assert_eq!(
        evaluate(&package_view, "return typeof requestIdleCallback;"),
        default_idle
    );
    drop(package_view);
    diagnostics::record(
        id,
        "host",
        json!({"kind":"idle-worlds","default":default_idle,"isolated":isolated,"ownedNative":result["nativeIdle"]}),
    );
    let tabs: Vec<_> = host.shell.borrow().tabs.keys().copied().collect();
    for tab in tabs {
        host.close(tab);
    }
    drop(popup);
    assert!(host.runtime.retire().run(Duration::from_secs(10)));
}

#[test]
#[ignore = "diagnostic module and private WebKitGTK session"]
fn request_cost() {
    use zephium_webext_linux::diagnostics;
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let evidence = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").unwrap());
    let diagnostic = DiagnosticDump(diagnostics::begin(&evidence, id).unwrap());
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    std::fs::write(package.path().join("manifest.json"),json!({"manifest_version":3,"name":"Zero request listeners","version":"1.0","content_scripts":[{"matches":["http://127.0.0.1/*"],"js":["empty.js"]}],"host_permissions":["http://127.0.0.1/*"]}).to_string()).unwrap();
    std::fs::write(package.path().join("empty.js"), "void 0;").unwrap();
    let (mut host, _engine) = host(context(), &module, state.path(), id);
    host.runtime
        .install_admitted(spec(package.path(), id, 1))
        .unwrap();
    let tab = host.open(&fixture.url("/password-form"), true);
    pump(&mut host, Duration::from_secs(1));
    let view = host.shell.borrow().tabs[&tab].view.clone();
    evaluate(&view,"globalThis.costDone=false;(async()=>{for(let i=0;i<256;i++)await fetch('/password-form?cost='+i,{cache:'no-store'});globalThis.costDone=true;})();return 'started';");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline
        && evaluate(&view, "return globalThis.costDone===true;") != "true"
    {
        pump(&mut host, Duration::from_millis(100));
    }
    assert_eq!(
        evaluate(&view, "return globalThis.costDone===true;"),
        "true"
    );
    pump(&mut host, Duration::from_secs(1));
    let rows = diagnostic_rows(&diagnostic.0);
    let mut values: Vec<u64> = rows
        .iter()
        .filter(|row| row["event"]["kind"] == "request-cost" && row["event"]["listeners"] == false)
        .filter_map(|row| row["event"]["nanoseconds"].as_u64())
        .filter(|value| *value > 0)
        .collect();
    values.sort_unstable();
    assert!(values.len() >= 256, "{} samples", values.len());
    diagnostics::record(
        id,
        "host",
        json!({"kind":"request-cost-summary","samples":values.len(),"p50_ns":values[values.len()/2],"p99_ns":values[(values.len()-1)*99/100],"mean_ns":values.iter().sum::<u64>()/values.len() as u64}),
    );
    host.close(tab);
    assert!(host.runtime.retire().run(Duration::from_secs(10)));
}

#[test]
#[ignore = "private native runtime session"]
fn window_commands_wake_and_effective_inventory() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let package = tempfile::tempdir().unwrap();
    let id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    std::fs::write(package.path().join("manifest.json"),json!({"manifest_version":3,"name":"Commands fixture","version":"1","permissions":["storage","activeTab"],"commands":{"go":{"description":"Go","suggested_key":{"linux":"Alt+Shift+Y"}},"conflict":{"description":"Conflict","suggested_key":{"linux":"Ctrl+T"}},"_execute_action":{"suggested_key":{"linux":"Alt+Shift+J"}}},"background":{"service_worker":"worker.js"},"action":{"default_popup":"popup.html"}}).to_string()).unwrap();
    std::fs::write(package.path().join("worker.js"),"chrome.commands.onCommand.addListener(async (name,tab)=>{await chrome.storage.local.set({command:name,tab:tab.id});});").unwrap();
    std::fs::write(
        package.path().join("popup.html"),
        "<!doctype html><button>Command fixture</button>",
    )
    .unwrap();
    let (mut host, _engine) = host(context(), &module, state.path(), id);
    let tab = host.open(&fixture.url("/password-form"), true);
    host.runtime
        .reserve_commands(vec![zephium_core::ports::engine::Shortcut {
            id: "tab.new".into(),
            key: 84,
            ctrl: true,
            shift: false,
            alt: false,
        }]);
    let install = ExtensionInstallId::from(INSTALL);
    host.runtime
        .install_admitted(spec(package.path(), id, 1))
        .unwrap();
    pump(&mut host, Duration::from_secs(2));
    let popup = host
        .runtime
        .open_page(install, PageKind::Popup, None)
        .unwrap();
    pump(&mut host, Duration::from_secs(1));
    let inventory: Value = serde_json::from_str(&evaluate(
        popup.view(),
        "return JSON.stringify(await chrome.commands.getAll());",
    ))
    .unwrap();
    assert!(inventory
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["name"] == "conflict" && row["shortcut"] == ""));
    assert!(!host.runtime.has_command(84, true, false, false));
    drop(popup);
    host.runtime
        .memory_pressure(zephium_webext_linux::runtime::MemoryPressure::Critical);
    pump(&mut host, Duration::from_secs(2));
    assert_eq!(
        host.runtime.background(install).unwrap().state,
        zephium_webext_runtime::lifecycle::BackgroundState::Stopped
    );
    assert!(host.runtime.perform_command(89, false, true, true));
    pump(&mut host, Duration::from_secs(2));
    let popup = host
        .runtime
        .open_page(install, PageKind::Popup, None)
        .unwrap();
    pump(&mut host, Duration::from_secs(1));
    let result: Value = serde_json::from_str(&evaluate(
        popup.view(),
        "return JSON.stringify(await chrome.storage.local.get(null));",
    ))
    .unwrap();
    assert_eq!(result["command"], "go");
    assert_eq!(result["tab"], host.runtime.tab_id(tab).unwrap());
    assert!(host.runtime.perform_command(74, false, true, true));
    pump(&mut host, Duration::from_millis(100));
    assert_eq!(host.shell.borrow().action_shortcuts, vec![(install, tab)]);
    let after: Value = serde_json::from_str(&evaluate(
        popup.view(),
        "return JSON.stringify(await chrome.storage.local.get(null));",
    ))
    .unwrap();
    assert_eq!(after, result, "_execute_action must not emit onCommand");
    println!("COMMANDS native wake, inventory and action route {result}");
    drop(popup);
    host.close(tab);
    assert!(host.runtime.retire().run(Duration::from_secs(10)));
}
