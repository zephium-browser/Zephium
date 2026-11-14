#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

//! The runtime as the engine wires it: an ordinary tab attached with the
//! profile's content scripts, the shell's tabs published, the popup page,
//! a toolbar click, disable, restart on the same state and removal.

#[path = "runtime_support/mod.rs"]
mod support;

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{pump_for, pump_until, ModuleDirectory};
use webkit2gtk::{LoadEvent, WebContextExt, WebViewExt};
use zephium_core::extensions::{
    ExtensionBrowserSurface, ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab,
    ExtensionBrowserWindow, ExtensionRuntimeGeneration,
};
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_linux::runtime::{
    ActionActivation, BrowserCall, InstallSpec, ProfileRuntime, ProfileRuntimeConfig, RuntimeEngine,
};
use zephium_webext_linux::scheme::Requester;
use zephium_webext_linux::views::PageKind;
use zephium_webext_runtime::context::ExtensionId;
use zephium_webext_runtime::i18n::LocaleEnvironment;
use zephium_webext_runtime::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

const EXTENSION: &str = "abcdefghijklmnopabcdefghijklmnop";

#[derive(Default)]
struct Engine {
    calls: RefCell<Vec<String>>,
}

impl RuntimeEngine for Engine {
    fn requester(&self, _: &webkit2gtk::WebView) -> Option<Requester> {
        None
    }

    fn browser_call(&self, call: BrowserCall) {
        let mut calls = self.calls.borrow_mut();
        if calls.len() < 256 {
            calls.push(format!("{call:?}").chars().take(80).collect());
        }
    }
}

struct Fixture {
    port: u16,
    reports: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

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
                let path = text.split_ascii_whitespace().nth(1).unwrap_or("/");
                let body = if let Some(query) = path.strip_prefix("/report?") {
                    let decoded = url::form_urlencoded::parse(format!("q={query}").as_bytes())
                        .next()
                        .map(|(_, value)| value.into_owned())
                        .unwrap_or_default();
                    if let Ok(value) = serde_json::from_str(&decoded) {
                        collected.lock().unwrap().push(value);
                    }
                    String::new()
                } else if path.starts_with("/frame") {
                    "<!doctype html><title>frame</title><p>frame".to_owned()
                } else {
                    format!(
                        "<!doctype html><title>page</title><p>page\
                         <iframe src=\"http://localhost:{port}/frame\"></iframe>"
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

    fn page(&self) -> String {
        format!("http://127.0.0.1:{}/page", self.port)
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

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/wiring")
}

fn spec(generation: u64) -> InstallSpec {
    let directory = fixture();
    let manifest = Manifest::load(&directory).unwrap();
    let (declaration, issues) = PermissionDeclaration::from_manifest(&manifest);
    assert!(issues.is_empty(), "{issues:?}");
    InstallSpec {
        install: ExtensionInstallId::from(41),
        extension_id: ExtensionId::parse(EXTENSION).unwrap(),
        generation: ExtensionRuntimeGeneration::new(generation).unwrap(),
        directory,
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

fn runtime(
    context: &webkit2gtk::WebContext,
    module: &ModuleDirectory,
    state: &Path,
    engine: &Rc<Engine>,
) -> ProfileRuntime {
    let engine: Rc<dyn RuntimeEngine> = engine.clone();
    ProfileRuntime::new(
        context,
        ProfileRuntimeConfig {
            profile: ProfileId::from(9),
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
        engine,
    )
    .expect("runtime")
}

fn surface(generation: u64, tab: ItemId, url: &str) -> ExtensionBrowserSurface {
    let record = ExtensionBrowserTab::from_snapshot(
        None,
        tab,
        true,
        "page",
        Some(&url::Url::parse(url).unwrap()),
        false,
        false,
    )
    .unwrap();
    ExtensionBrowserSurface::new(
        ProfileId::from(9),
        ExtensionBrowserSurfaceGeneration::new(generation).unwrap(),
        Some(3),
        vec![ExtensionBrowserWindow::new(3, false, Some(tab), vec![record]).unwrap()],
    )
    .unwrap()
}

fn context() -> webkit2gtk::WebContext {
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    context
}

fn load(view: &webkit2gtk::WebView, url: &str) {
    let finished = Rc::new(std::cell::Cell::new(false));
    let observed = finished.clone();
    let handler = view.connect_load_changed(move |_, event| {
        if event == LoadEvent::Finished {
            observed.set(true);
        }
    });
    view.load_uri(url);
    pump_until(|| finished.get());
    gtk::glib::prelude::ObjectExt::disconnect(view, handler);
}

/// Waits for one report from the top frame and one from the cross-origin frame.
fn reports(fixture: &Fixture) -> (Value, Value) {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(fixture.take());
        let top = seen.iter().find(|r| r["top"] == true).cloned();
        let frame = seen.iter().find(|r| r["top"] == false).cloned();
        if let (Some(top), Some(frame)) = (top, frame) {
            return (top, frame);
        }
        pump_for(Duration::from_millis(20));
    }
    panic!("content scripts did not report from both frames: {seen:?}");
}

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_wiring_acceptance() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let engine = Rc::new(Engine::default());
    let tab = ItemId::from(5);
    let started = Instant::now();

    {
        let context = context();
        let runtime = runtime(&context, &module, state.path(), &engine);
        runtime.install_admitted(spec(1)).expect("installed");
        let page = webkit2gtk::WebView::with_context(&context);
        let view = runtime.attach_tab(tab, &page).expect("attached");
        runtime.publish_surface(surface(1, tab, &fixture.page()));
        let chrome_tab = runtime.tab_id(tab).expect("chrome tab id");

        load(&page, &fixture.page());
        let (top, frame) = reports(&fixture);
        println!("WEBEXT wiring top={top} frame={frame}");
        assert_eq!(top["error"], Value::Null);
        assert_eq!(top["reply"]["tab"], chrome_tab);
        assert_eq!(top["reply"]["frame"], 0);
        assert_eq!(top["reply"]["tabs"], 1);
        assert_eq!(top["reply"]["url"], fixture.page());
        assert!(frame["href"]
            .as_str()
            .unwrap()
            .starts_with("http://localhost:"));
        assert!(frame["reply"]["frame"]
            .as_u64()
            .is_some_and(|frame| frame > 0));
        assert_eq!(frame["reply"]["tab"], chrome_tab);
        let seen = top["reply"]["seen"]
            .as_u64()
            .unwrap()
            .max(frame["reply"]["seen"].as_u64().unwrap());
        assert_eq!(seen, 2, "storage counted both frames");

        // The popup page talks to the background; a toolbar click names it.
        let popup = runtime
            .open_page(ExtensionInstallId::from(41), PageKind::Popup, None)
            .expect("popup");
        pump_until(|| {
            popup
                .view()
                .title()
                .is_some_and(|title| title.starts_with("reply:") || title.starts_with("error:"))
        });
        let title = popup.view().title().unwrap();
        println!("WEBEXT wiring popup={title}");
        assert!(title.starts_with("reply:"), "{title}");
        assert!(title.contains("\"popup\":true"));
        drop(popup);
        let activation = runtime
            .invoke_action(ExtensionInstallId::from(41), tab)
            .expect("action");
        assert!(
            matches!(activation, ActionActivation::Popup(url) if url == format!("chrome-extension://{EXTENSION}/popup.html"))
        );

        // Disabled: new documents get nothing.
        let mut stop = runtime.install_disabled(ExtensionInstallId::from(41));
        assert!(stop.run(Duration::from_secs(10)), "disable drained");
        load(&page, &fixture.page());
        pump_for(Duration::from_millis(1500));
        assert!(fixture.take().is_empty(), "a disabled extension injected");
        println!("WEBEXT wiring disabled_injections=0");

        // Enabled again: storage was kept.
        runtime.install_enabled(spec(2)).expect("enabled");
        load(&page, &fixture.page());
        let (top, _) = reports(&fixture);
        assert!(top["reply"]["seen"].as_u64().unwrap() >= 3);
        assert_eq!(top["reply"]["popup"], true);

        runtime.detach_tab(view);
        drop(page);
        let mut drain = runtime.retire();
        assert!(drain.run(Duration::from_secs(10)), "retire drained");
    }

    // A restarted browser restores the extension and its storage.
    {
        let context = context();
        let runtime = runtime(&context, &module, state.path(), &engine);
        runtime.install_enabled(spec(3)).expect("restored");
        let page = webkit2gtk::WebView::with_context(&context);
        let view = runtime.attach_tab(tab, &page).expect("attached");
        runtime.publish_surface(surface(1, tab, &fixture.page()));
        load(&page, &fixture.page());
        let (top, _) = reports(&fixture);
        println!("WEBEXT wiring restored={top}");
        assert!(
            top["reply"]["seen"].as_u64().unwrap() >= 5,
            "storage survived the restart"
        );

        // Removal erases storage.
        let mut stop = runtime.install_removed(ExtensionInstallId::from(41));
        assert!(stop.run(Duration::from_secs(10)), "removal drained");
        pump_for(Duration::from_millis(300));
        runtime.install_admitted(spec(4)).expect("reinstalled");
        load(&page, &fixture.page());
        let (top, frame) = reports(&fixture);
        let seen = top["reply"]["seen"]
            .as_u64()
            .unwrap()
            .max(frame["reply"]["seen"].as_u64().unwrap());
        assert_eq!(seen, 2, "a reinstall starts with empty storage");
        assert_eq!(top["reply"]["popup"], false);
        println!("WEBEXT wiring removal_erased=true");

        runtime.detach_tab(view);
        drop(page);
        let mut drain = runtime.retire();
        assert!(drain.run(Duration::from_secs(10)), "retire drained");
    }
    println!(
        "WEBEXT wiring elapsed_ms={:.0} calls={:?}",
        started.elapsed().as_secs_f64() * 1000.0,
        engine.calls.borrow().len()
    );
}
