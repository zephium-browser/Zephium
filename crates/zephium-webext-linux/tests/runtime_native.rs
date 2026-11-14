#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

#[path = "runtime_support/network.rs"]
mod network_fixture;
#[path = "runtime_support/mod.rs"]
mod support;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib::prelude::ObjectExt;
use serde_json::Value;
use support::{pump_for, web_processes, ModuleDirectory};
use webkit2gtk::{LoadEvent, WebContextExt, WebViewExt};
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_linux::runtime::{
    ExtensionErrorKind, InstallSpec, MemoryPressure, Notice, ProfileRuntime, ProfileRuntimeConfig,
    RuntimeEngine,
};
use zephium_webext_linux::scheme::Requester;
use zephium_webext_linux::views::{ExtensionPage, PageKind};
use zephium_webext_runtime::context::ExtensionId;
use zephium_webext_runtime::i18n::LocaleEnvironment;
use zephium_webext_runtime::lifecycle::BackgroundState;
use zephium_webext_runtime::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

const SCALE: u32 = 10;

#[derive(Default)]
struct Engine {
    notices: RefCell<Vec<Notice>>,
}

impl RuntimeEngine for Engine {
    fn requester(&self, _: &webkit2gtk::WebView) -> Option<Requester> {
        None
    }

    fn notice(&self, notice: &Notice) {
        let mut notices = self.notices.borrow_mut();
        if notices.len() < 512 {
            notices.push(notice.clone());
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name)
}

fn extension_id(index: u8) -> ExtensionId {
    let mut id = String::from("abcdefghijklmnopabcdefghijklmno");
    id.push(char::from(b'a' + index));
    ExtensionId::parse(&id).unwrap()
}

fn spec(install: u128, index: u8, directory: &Path) -> InstallSpec {
    let manifest = Manifest::load(directory).unwrap();
    let (_, issues) = PermissionDeclaration::from_manifest(&manifest);
    assert!(issues.is_empty(), "{issues:?}");
    lenient_spec(install, index, directory)
}

fn lenient_spec(install: u128, index: u8, directory: &Path) -> InstallSpec {
    let manifest = Manifest::load(directory).unwrap();
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    InstallSpec {
        install: ExtensionInstallId::from(install),
        extension_id: extension_id(index),
        generation: ExtensionRuntimeGeneration::INITIAL,
        directory: directory.to_owned(),
        manifest,
        permissions: InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::default(),
            false,
        ),
        allowed_in_incognito: false,
        externally_connectable: None,
        unpacked: true,
    }
}

fn context() -> webkit2gtk::WebContext {
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    assert!(context.is_sandbox_enabled());
    context
}

fn runtime(
    context: &webkit2gtk::WebContext,
    module: &ModuleDirectory,
    state: Option<&Path>,
    engine: &Rc<Engine>,
    scale: u32,
) -> ProfileRuntime {
    let engine: Rc<dyn RuntimeEngine> = engine.clone();
    ProfileRuntime::new(
        context,
        ProfileRuntimeConfig {
            profile: ProfileId::from(7),
            incognito: false,
            module_directory: module.path().to_owned(),
            state_directory: state.map(Path::to_owned),
            locale: LocaleEnvironment {
                ui_language: "en-US".into(),
                accept_languages: vec!["en-US".into(), "en".into()],
                right_to_left: false,
            },
            time_scale: scale,
        },
        engine,
    )
    .unwrap()
}

fn wait(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) -> Duration {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < timeout, "timed out waiting for {what}");
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    start.elapsed()
}

fn state(runtime: &ProfileRuntime, install: ExtensionInstallId) -> Option<BackgroundState> {
    runtime.background(install).map(|status| status.state)
}

fn ready(runtime: &ProfileRuntime, install: ExtensionInstallId) -> bool {
    runtime
        .background(install)
        .is_some_and(|status| status.ready && status.state == BackgroundState::Running)
}

fn open(runtime: &ProfileRuntime, install: ExtensionInstallId) -> ExtensionPage {
    let page = runtime.open_page(install, PageKind::Options, None).unwrap();
    let finished = Rc::new(Cell::new(false));
    let seen = finished.clone();
    let handler = page.view().connect_load_changed(move |_, event| {
        if event == LoadEvent::Finished {
            seen.set(true);
        }
    });
    wait("options page load", Duration::from_secs(20), || {
        finished.get()
    });
    page.view().disconnect(handler);
    page
}

fn script(page: &ExtensionPage, body: &str) -> Value {
    let wrapped = format!(
        "try {{ return JSON.stringify({{ok: await (async () => {{ {body} }})()}}); }} \
         catch (e) {{ return JSON.stringify({{error: String(e && e.message || e)}}); }}"
    );
    let result = Rc::new(RefCell::new(None));
    let observed = result.clone();
    page.view().call_async_javascript_function(
        &wrapped,
        None,
        None,
        None,
        None::<&gtk::gio::Cancellable>,
        move |value| {
            *observed.borrow_mut() = Some(value.map(|value| value.to_string()));
        },
    );
    wait("page script", Duration::from_secs(30), || {
        result.borrow().is_some()
    });
    let text = result
        .borrow_mut()
        .take()
        .unwrap()
        .expect("page evaluation");
    serde_json::from_str(&text).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

fn ask(page: &ExtensionPage, message: &str) -> Value {
    let reply = script(
        page,
        &format!("return await chrome.runtime.sendMessage({message});"),
    );
    reply
        .get("ok")
        .cloned()
        .unwrap_or_else(|| panic!("message {message} failed: {reply}"))
}

/// Waits for the asynchronous onInstalled handler's write.
fn installed(page: &ExtensionPage) -> Value {
    let mut stored = Value::Null;
    wait("onInstalled write", Duration::from_secs(20), || {
        stored = ask(page, "{type: 'storage'}");
        !stored["installs"].is_null()
    });
    assert_eq!(stored["installs"], 1, "{stored}");
    stored
}

fn pss_kib(pid: u32) -> Option<u64> {
    let rollup = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    rollup
        .lines()
        .find_map(|line| line.strip_prefix("Pss:"))
        .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
}

fn report(name: &str, value: impl std::fmt::Display) {
    println!("WEBEXT runtime {name}={value}");
}

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_runtime_classic_background() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let directory = tempfile::tempdir().unwrap();
    let engine = Rc::new(Engine::default());
    let context = context();
    let runtime = runtime(&context, &module, Some(directory.path()), &engine, SCALE);
    let install = ExtensionInstallId::from(0x11);
    let started = Instant::now();
    runtime
        .install_admitted(spec(0x11, 0, &fixture("classic")))
        .unwrap();
    wait("background ready", Duration::from_secs(30), || {
        ready(&runtime, install)
    });
    report("classic_start_to_ready_ms", started.elapsed().as_millis());
    report(
        "classic_start_to_listeners_ms",
        format!(
            "{:.1}",
            runtime.background(install).unwrap().startup_ms.unwrap()
        ),
    );

    let page = open(&runtime, install);
    let probe = ask(&page, "{type: 'probe'}");
    assert_eq!(probe["serviceWorker"], true, "{probe}");
    assert_eq!(probe["window"], "undefined");
    assert_eq!(probe["document"], "undefined");
    assert_eq!(probe["helper"], 42);
    assert_eq!(probe["greeting"], "Hello fixture, now");
    assert_eq!(probe["id"], extension_id(0).as_str());
    assert_eq!(
        probe["lifecycle"],
        serde_json::json!([
            "install",
            "install settled",
            "runtime.onInstalled:install",
            "activate",
            "activate settled"
        ]),
        "{probe}"
    );
    assert!(
        probe["sender"].as_str().unwrap().ends_with("/options.html"),
        "{probe}"
    );
    let first_start = probe["startedAt"].as_f64().unwrap();
    let stored = installed(&page);
    assert_eq!(stored["lastReason"], "install");

    // Idle termination, then a wake by message from the options page.
    let generation = runtime.background(install).unwrap().generation;
    let idle = wait("idle termination", Duration::from_secs(20), || {
        state(&runtime, install) == Some(BackgroundState::Stopped)
    });
    report("classic_idle_out_ms", idle.as_millis());
    let woken = Instant::now();
    let probe = ask(&page, "{type: 'probe'}");
    report("classic_message_wake_ms", woken.elapsed().as_millis());
    assert!(probe["startedAt"].as_f64().unwrap() > first_start);
    assert_eq!(
        probe["helper"], 42,
        "install-time imports survive a restart"
    );
    assert!(runtime.background(install).unwrap().generation > generation);
    let stored = ask(&page, "{type: 'storage'}");
    assert_eq!(stored["installs"], 1, "onInstalled fired again: {stored}");

    // An alarm wakes a background that idled out.
    assert_eq!(
        ask(&page, "{type: 'alarm', name: 'wake', minutes: 1}"),
        true
    );
    let armed = Instant::now();
    wait("idle before alarm", Duration::from_secs(20), || {
        state(&runtime, install) == Some(BackgroundState::Stopped)
    });
    let generation = runtime.background(install).unwrap().generation;
    wait("alarm wake", Duration::from_secs(20), || {
        runtime
            .background(install)
            .is_some_and(|status| status.generation > generation)
    });
    report("classic_alarm_wake_after_ms", armed.elapsed().as_millis());
    wait("alarm stored", Duration::from_secs(20), || {
        script(
            &page,
            "return (await chrome.storage.local.get('alarmCount')).alarmCount;",
        )["ok"]
            == 1
    });

    // Offscreen document: one per extension, keeps the background alive.
    assert_eq!(ask(&page, "{type: 'offscreenOpen'}"), true);
    assert!(runtime.has_offscreen_document(install));
    assert_eq!(ask(&page, "{type: 'offscreenHas'}"), true);
    assert_eq!(
        ask(&page, "{type: 'offscreenSecond'}"),
        "Only a single offscreen document may be created."
    );
    let ping = ask(&page, "{type: 'offscreenPing'}");
    assert_eq!(ping["offscreen"], true, "{ping}");
    assert_eq!(ping["window"], "object");
    let generation = runtime.background(install).unwrap().generation;
    pump_for(Duration::from_millis(30_000 / u64::from(SCALE) + 1500));
    assert_eq!(state(&runtime, install), Some(BackgroundState::Running));
    assert_eq!(runtime.background(install).unwrap().generation, generation);
    assert_eq!(ask(&page, "{type: 'offscreenClose'}"), true);
    assert!(!runtime.has_offscreen_document(install));
    assert_eq!(
        ask(&page, "{type: 'offscreenClose'}"),
        "No current offscreen document."
    );

    // Errors reach the host with the extension id.
    assert_eq!(ask(&page, "{type: 'fail'}"), true);
    wait("errors", Duration::from_secs(20), || {
        let kinds: Vec<_> = runtime.errors(install).iter().map(|e| e.kind).collect();
        [
            ExtensionErrorKind::Console,
            ExtensionErrorKind::UnhandledRejection,
            ExtensionErrorKind::Uncaught,
        ]
        .iter()
        .all(|kind| kinds.contains(kind))
    });
    for error in runtime.errors(install) {
        assert_eq!(error.extension_id, extension_id(0).as_str());
        report(
            "classic_error",
            format!("{:?}:{}", error.kind, error.message),
        );
    }

    // A crash: the background restarts on the next event after backoff.
    drop(page);
    wait(
        "only the background renderer",
        Duration::from_secs(20),
        || web_processes().len() == 1,
    );
    let renderer = *web_processes().iter().next().unwrap();
    if let Some(pss) = pss_kib(renderer) {
        report("classic_background_pss_kib", pss);
    }
    let crashed_generation = runtime.background(install).unwrap().generation;
    assert!(std::process::Command::new("kill")
        .args(["-KILL", &renderer.to_string()])
        .status()
        .unwrap()
        .success());
    wait("crash observed", Duration::from_secs(20), || {
        state(&runtime, install) == Some(BackgroundState::Crashed)
    });
    assert!(runtime
        .errors(install)
        .iter()
        .any(|error| error.kind == ExtensionErrorKind::Crash));
    let page = open(&runtime, install);
    let recovered = Instant::now();
    let probe = ask(&page, "{type: 'probe'}");
    report("classic_crash_recovery_ms", recovered.elapsed().as_millis());
    assert_eq!(probe["serviceWorker"], true);
    assert!(runtime.background(install).unwrap().generation > crashed_generation);
    let stored = ask(&page, "{type: 'storage'}");
    assert_eq!(stored["installs"], 1, "{stored}");
    assert_eq!(stored["alarmCount"], 1);
    assert!(
        runtime.unhandled_calls(install).is_empty(),
        "{:?}",
        runtime.unhandled_calls(install)
    );

    drop(page);
    let mut drain = runtime.retire();
    assert!(drain.run(Duration::from_secs(10)), "drain did not settle");
    drop(runtime);
    drop(context);
    assert!(engine
        .notices
        .borrow()
        .iter()
        .all(|notice| !matches!(notice, Notice::Disabled(_))));
    wait("no renderers", Duration::from_secs(10), || {
        web_processes().is_empty()
    });
    report("classic_remaining_renderers", web_processes().len());
}

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_runtime_module_background() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let engine = Rc::new(Engine::default());
    let context = context();
    let runtime = runtime(&context, &module, None, &engine, SCALE);
    let install = ExtensionInstallId::from(0x12);
    let started = Instant::now();
    runtime
        .install_admitted(spec(0x12, 1, &fixture("module")))
        .unwrap();
    wait("module background ready", Duration::from_secs(30), || {
        ready(&runtime, install)
    });
    report("module_start_to_ready_ms", started.elapsed().as_millis());
    let page = open(&runtime, install);
    let probe = ask(&page, "{type: 'probe'}");
    assert_eq!(probe["serviceWorker"], true, "{probe}");
    assert_eq!(probe["window"], "undefined");
    assert_eq!(probe["helper"], 42);
    assert_eq!(probe["module"], true);
    assert_eq!(probe["awaited"], "top-level await");
    assert_eq!(probe["importScriptsThrows"], true);
    let stored = installed(&page);
    assert_eq!(stored["lastReason"], "install");
    drop(page);
    let mut drain = runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    drop(runtime);
    drop(context);
    wait("no renderers", Duration::from_secs(10), || {
        web_processes().is_empty()
    });
}

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_runtime_capacity_and_pressure() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let engine = Rc::new(Engine::default());
    let context = context();
    // Real time: eight starts must finish well inside one idle timeout.
    let runtime = runtime(&context, &module, None, &engine, 1);
    let tiny = fixture("tiny");
    let installs: Vec<ExtensionInstallId> = (0..9u8)
        .map(|index| ExtensionInstallId::from(0x100 + u128::from(index)))
        .collect();
    for (index, install) in installs.iter().take(8).enumerate() {
        runtime
            .install_admitted(spec(0x100 + index as u128, index as u8, &tiny))
            .unwrap();
        wait("tiny ready", Duration::from_secs(30), || {
            ready(&runtime, *install)
        });
        pump_for(Duration::from_millis(20));
    }
    let renderers = web_processes().len();
    report("capacity_renderers_at_cap", renderers);
    let pss: u64 = web_processes().into_iter().filter_map(pss_kib).sum();
    report("capacity_mean_pss_kib", pss / renderers.max(1) as u64);
    runtime.install_admitted(spec(0x108, 8, &tiny)).unwrap();
    wait("ninth ready", Duration::from_secs(30), || {
        ready(&runtime, installs[8])
    });
    assert_eq!(state(&runtime, installs[0]), Some(BackgroundState::Stopped));
    assert!(installs[1..]
        .iter()
        .all(|install| ready(&runtime, *install)));
    // Busy backgrounds are never reclaimed: let onInstalled settle.
    pump_for(Duration::from_millis(1000));
    runtime.memory_pressure(MemoryPressure::Critical);
    wait("pressure", Duration::from_secs(20), || {
        installs
            .iter()
            .all(|install| state(&runtime, *install) == Some(BackgroundState::Stopped))
    });
    wait("renderers gone", Duration::from_secs(20), || {
        web_processes().is_empty()
    });
    let mut drain = runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    drop(runtime);
    drop(context);
}

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_runtime_browser_restart() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let directory = tempfile::tempdir().unwrap();
    let install = ExtensionInstallId::from(0x13);
    {
        let engine = Rc::new(Engine::default());
        let context = context();
        let runtime = runtime(&context, &module, Some(directory.path()), &engine, SCALE);
        runtime
            .install_admitted(spec(0x13, 2, &fixture("classic")))
            .unwrap();
        wait("first run ready", Duration::from_secs(30), || {
            ready(&runtime, install)
        });
        let page = open(&runtime, install);
        installed(&page);
        assert_eq!(
            script(
                &page,
                r#"
            await chrome.scripting.registerContentScripts([
                {id:'persistent',matches:['https://example.com/*'],js:['lib/helper.js']},
                {id:'session',matches:['https://example.com/*'],js:['lib/helper.js'],persistAcrossSessions:false}
            ]);
            return (await chrome.scripting.getRegisteredContentScripts()).map(s=>s.id);
        "#
            )["ok"],
            serde_json::json!(["persistent", "session"])
        );
        drop(page);
        let mut drain = runtime.retire();
        assert!(drain.run(Duration::from_secs(10)));
        drop(runtime);
        drop(context);
        wait("first run renderers", Duration::from_secs(10), || {
            web_processes().is_empty()
        });
    }
    let engine = Rc::new(Engine::default());
    let context = context();
    let runtime = runtime(&context, &module, Some(directory.path()), &engine, SCALE);
    runtime
        .install_enabled(spec(0x13, 2, &fixture("classic")))
        .unwrap();
    runtime.browser_started();
    assert_eq!(state(&runtime, install), Some(BackgroundState::Stopped));
    let page = open(&runtime, install);
    assert_eq!(
        script(
            &page,
            "return (await chrome.scripting.getRegisteredContentScripts()).map(s=>s.id);"
        )["ok"],
        serde_json::json!(["persistent"])
    );
    let woken = Instant::now();
    let probe = ask(&page, "{type: 'probe'}");
    report("restart_message_wake_ms", woken.elapsed().as_millis());
    assert_eq!(probe["helper"], 42, "persisted import set: {probe}");
    let stored = ask(&page, "{type: 'storage'}");
    assert_eq!(stored["installs"], 1, "{stored}");
    assert!(
        runtime.errors(install).is_empty(),
        "{:?}",
        runtime.errors(install)
    );
    drop(page);
    let mut drain = runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    drop(runtime);
    drop(context);
    wait("no renderers", Duration::from_secs(10), || {
        web_processes().is_empty()
    });
}

/// Boots unmodified public extension sources from `ZEPHIUM_WEBEXT_CORPUS`
/// (`<dir>/<name>` package roots) until their listeners are registered and
/// records the API they used that this runtime does not implement.
#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_runtime_real_extensions_smoke() {
    let _session = support::init();
    let Some(corpus) = std::env::var_os("ZEPHIUM_WEBEXT_CORPUS").map(PathBuf::from) else {
        report("smoke", "skipped: ZEPHIUM_WEBEXT_CORPUS unset");
        return;
    };
    let module = ModuleDirectory::new();
    let engine = Rc::new(Engine::default());
    let context = context();
    let runtime = runtime(&context, &module, None, &engine, 1);
    for (index, name) in ["vimium", "darkreader"].into_iter().enumerate() {
        let root = corpus.join(name);
        if !root.join("manifest.json").is_file() {
            report(&format!("smoke_{name}"), "skipped: no built package");
            continue;
        }
        let install = ExtensionInstallId::from(0x200 + index as u128);
        let started = Instant::now();
        if let Err(error) =
            runtime.install_admitted(lenient_spec(0x200 + index as u128, 10 + index as u8, &root))
        {
            report(&format!("smoke_{name}_refused"), error);
            continue;
        }
        let booted = (0..20_000).find(|_| {
            pump_for(Duration::from_millis(1));
            ready(&runtime, install)
                || runtime
                    .errors(install)
                    .iter()
                    .any(|error| error.kind == ExtensionErrorKind::Startup)
        });
        report(&format!("smoke_{name}_ready"), ready(&runtime, install));
        if booted.is_some() && ready(&runtime, install) {
            report(
                &format!("smoke_{name}_start_to_listeners_ms"),
                started.elapsed().as_millis(),
            );
        }
        pump_for(Duration::from_secs(2));
        for error in runtime.errors(install) {
            report(
                &format!("smoke_{name}_error"),
                format!("{:?}: {}", error.kind, error.message),
            );
        }
        for (call, count) in runtime.unhandled_calls(install) {
            report(
                &format!("smoke_{name}_unhandled"),
                format!("{call} x{count}"),
            );
        }
    }
    let mut drain = runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    drop(runtime);
    drop(context);
    wait("no renderers", Duration::from_secs(10), || {
        web_processes().is_empty()
    });
}

#[test]
#[ignore = "sandboxed WebKitGTK DNR runtime; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_runtime_dnr_updates_and_persistence() {
    use serde_json::json;
    use zephium_core::ids::ItemId;

    let _session = support::init();
    let module = ModuleDirectory::new();
    let server = network_fixture::Server::new();
    let package = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    std::fs::write(package.path().join("manifest.json"), serde_json::to_vec(&json!({
        "manifest_version":3,"name":"DNR native fixture","version":"1.0",
        "permissions":["declarativeNetRequest","declarativeNetRequestFeedback"],
        "host_permissions":["<all_urls>"],
        "background":{"service_worker":"sw.js"},"options_ui":{"page":"options.html"},
        "action":{},"declarative_net_request":{"rule_resources":[{"id":"static","enabled":true,"path":"rules.json"}]}
    })).unwrap()).unwrap();
    std::fs::write(
        package.path().join("sw.js"),
        "chrome.runtime.onMessage.addListener(()=>false);",
    )
    .unwrap();
    std::fs::write(
        package.path().join("options.html"),
        "<!doctype html><p>DNR fixture</p>",
    )
    .unwrap();
    std::fs::write(package.path().join("rules.json"), r#"[{"id":1,"action":{"type":"block"},"condition":{"urlFilter":"/static-block"}},{"id":2,"action":{"type":"modifyHeaders","responseHeaders":[{"header":"content-security-policy","operation":"append","value":"script-src none"}]},"condition":{"urlFilter":"/unsupported-response"}}]"#).unwrap();
    let install = ExtensionInstallId::from(0x301);
    let engine = Rc::new(Engine::default());
    let native_context = context();
    let profile = runtime(&native_context, &module, Some(storage.path()), &engine, 1);
    profile.set_browser(Box::new(network_fixture::Browser::new(&server.origin)));
    profile
        .install_admitted(spec(0x301, 2, package.path()))
        .unwrap();
    wait("DNR background ready", Duration::from_secs(30), || {
        ready(&profile, install)
    });
    assert!(
        profile
            .network_status(install)
            .unwrap()
            .unsupported_rule_count
            > 0
    );
    let options = open(&profile, install);
    assert_eq!(
        script(
            &options,
            "return await chrome.declarativeNetRequest.getEnabledRulesets();"
        )["ok"],
        json!(["static"])
    );

    let tab = webkit2gtk::WebView::with_context(&native_context);
    let bridge_view = profile
        .attach_tab_with_id(ItemId::from(1), 1, &tab)
        .unwrap();
    tab.load_uri(&format!("{}/page", server.origin));
    wait("DNR tab load", Duration::from_secs(20), || {
        !tab.is_loading() && tab.uri().is_some()
    });
    // Attachment completes asynchronously; wait before testing first policy use.
    pump_for(Duration::from_millis(500));
    assert!(!network_fetch(&tab, "/static-block"));
    assert_eq!(script(&options, "globalThis.dnrDebug=[];chrome.declarativeNetRequest.onRuleMatchedDebug.addListener(info=>dnrDebug.push(info));return true;")["ok"], true);
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.setExtensionActionOptions({displayActionCountAsBadgeText:true});return true;")["ok"], true);
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.updateDynamicRules({addRules:[{id:7,action:{type:'block'},condition:{urlFilter:'/dynamic-block'}}]});return true;")["ok"], true);
    assert!(
        !network_fetch(&tab, "/dynamic-block"),
        "resolved API must enforce the next request without reload"
    );
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.updateSessionRules({addRules:[{id:8,action:{type:'block'},condition:{urlFilter:'/session-block',tabIds:[1]}}]});return true;")["ok"], true);
    assert!(!network_fetch(&tab, "/session-block"));
    let duplicate = script(&options, "return await chrome.declarativeNetRequest.updateDynamicRules({removeRuleIds:[7],addRules:[{id:9,action:{type:'block'},condition:{urlFilter:'/duplicate'}},{id:9,action:{type:'allow'},condition:{urlFilter:'/duplicate'}}]});");
    assert!(duplicate["error"].is_string());
    assert!(!network_fetch(&tab, "/dynamic-block?after-rejection"));
    assert_eq!(script(&options, "return (await chrome.declarativeNetRequest.getDynamicRules({ruleIds:[7]})).map(rule=>rule.id);")["ok"], json!([7]));
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.updateStaticRules({rulesetId:'static',disableRuleIds:[1]});return true;")["ok"], true);
    assert!(network_fetch(&tab, "/static-block?disabled"));
    assert_eq!(
        script(
            &options,
            "return await chrome.declarativeNetRequest.getDisabledRuleIds({rulesetId:'static'});"
        )["ok"],
        json!([1])
    );
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.updateEnabledRulesets({disableRulesetIds:['static']});return true;")["ok"], true);
    pump_for(Duration::from_millis(500));
    let feedback = script(
        &options,
        "return await chrome.declarativeNetRequest.getMatchedRules({tabId:1});",
    );
    assert!(
        feedback["ok"]["rulesMatchedInfo"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["rule"]["ruleId"] == 7)),
        "{feedback}"
    );
    assert_eq!(
        script(
            &options,
            "return dnrDebug.some(info=>info.rule.ruleId===7 && info.request.type==='xmlhttprequest' && info.request.requestId.length>0);"
        )["ok"],
        true
    );
    let badge = script(
        &options,
        "return await chrome.action.getBadgeText({tabId:1});",
    );
    assert!(
        badge["ok"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|count| count > 0),
        "{badge}"
    );
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.updateDynamicRules({removeRuleIds:[7]});return true;")["ok"], true);
    assert!(network_fetch(&tab, "/dynamic-block?removed"));
    assert_eq!(script(&options, "await chrome.declarativeNetRequest.updateDynamicRules({addRules:[{id:10,action:{type:'block'},condition:{urlFilter:'/persisted'}}]});return true;")["ok"], true);
    assert!(!server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|path| path == "/dynamic-block" || path == "/session-block"));
    profile.detach_tab(bridge_view);
    tab.stop_loading();
    drop(tab);
    drop(options);
    assert!(profile.retire().run(Duration::from_secs(10)));
    drop(profile);
    drop(native_context);
    wait("DNR first runtime drained", Duration::from_secs(10), || {
        web_processes().is_empty()
    });

    let native_context = context();
    let profile = runtime(&native_context, &module, Some(storage.path()), &engine, 1);
    profile.set_browser(Box::new(network_fixture::Browser::new(&server.origin)));
    profile
        .install_enabled(spec(0x301, 2, package.path()))
        .unwrap();
    // An enabled install starts its background lazily; rules restore without it.
    wait("DNR rules restored", Duration::from_secs(30), || {
        profile
            .network_status(install)
            .is_some_and(|status| !status.loading)
    });
    assert_eq!(state(&profile, install), Some(BackgroundState::Stopped));
    assert!(
        profile.errors(install).is_empty(),
        "{:?}",
        profile.errors(install)
    );
    let tab = webkit2gtk::WebView::with_context(&native_context);
    let bridge_view = profile
        .attach_tab_with_id(ItemId::from(2), 2, &tab)
        .unwrap();
    tab.load_uri(&format!("{}/page?restored", server.origin));
    wait("DNR restored tab load", Duration::from_secs(20), || {
        !tab.is_loading() && tab.uri().is_some()
    });
    pump_for(Duration::from_millis(500));
    assert!(
        !network_fetch(&tab, "/persisted"),
        "restored dynamic rule must be enforced"
    );
    assert!(network_fetch(&tab, "/static-block?ruleset-still-disabled"));
    profile.detach_tab(bridge_view);
    tab.stop_loading();
    drop(tab);
    let options = open(&profile, install);
    assert_eq!(
        script(
            &options,
            "await chrome.runtime.sendMessage({}).catch(()=>{});return true;"
        )["ok"],
        true
    );
    wait("DNR restored background", Duration::from_secs(30), || {
        ready(&profile, install)
    });
    assert_eq!(
        script(
            &options,
            "return (await chrome.declarativeNetRequest.getDynamicRules()).map(rule=>rule.id);"
        )["ok"],
        json!([10])
    );
    assert_eq!(
        script(
            &options,
            "return await chrome.declarativeNetRequest.getSessionRules();"
        )["ok"],
        json!([])
    );
    assert_eq!(
        script(
            &options,
            "return await chrome.declarativeNetRequest.getEnabledRulesets();"
        )["ok"],
        json!([])
    );
    assert_eq!(
        script(
            &options,
            "return await chrome.declarativeNetRequest.getDisabledRuleIds({rulesetId:'static'});"
        )["ok"],
        json!([1])
    );
    drop(options);
    assert!(profile.retire().run(Duration::from_secs(10)));
    drop(profile);
    drop(native_context);
    println!("DNR runtime next_request=true static_toggle=true duplicate_atomicity=true dynamic_persisted=true session_ephemeral=true feedback=true badge=true");
}

fn network_fetch(page: &webkit2gtk::WebView, path: &str) -> bool {
    use javascriptcore::ValueExt;
    let result = Rc::new(RefCell::new(None));
    let captured = result.clone();
    page.call_async_javascript_function(
        &format!(
            "try {{ await fetch({}); return true; }} catch {{ return false; }}",
            serde_json::to_string(path).unwrap()
        ),
        None,
        None,
        None,
        None::<&gtk::gio::Cancellable>,
        move |reply| *captured.borrow_mut() = Some(reply.unwrap().to_boolean()),
    );
    wait("native DNR fetch", Duration::from_secs(20), || {
        result.borrow().is_some()
    });
    let completed = result.borrow_mut().take().unwrap();
    completed
}
