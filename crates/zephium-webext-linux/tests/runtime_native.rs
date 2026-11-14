#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

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
    let (declaration, issues) = PermissionDeclaration::from_manifest(&manifest);
    assert!(issues.is_empty(), "{issues:?}");
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
    assert!(
        probe["sender"].as_str().unwrap().ends_with("/options.html"),
        "{probe}"
    );
    let first_start = probe["startedAt"].as_f64().unwrap();
    let stored = ask(&page, "{type: 'storage'}");
    assert_eq!(stored["installs"], 1, "{stored}");
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
    let stored = ask(&page, "{type: 'storage'}");
    assert_eq!(stored["installs"], 1);
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
