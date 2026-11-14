#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

//! The WebKitGTK engine for `eval/webext-conformance/run-linux.mjs`: one
//! unpacked case extension in a disposable profile, its runner page in an
//! ordinary tab, and a minimal shell that owns tabs as the browser does.
//! Line protocol on stdin/stdout, prefixed `ZCH `.

#[path = "shell_support/mod.rs"]
mod shell;
#[path = "runtime_support/mod.rs"]
mod support;

use shell::{emit, Engine, Host, Shell, INSTALL, PROFILE};

use std::cell::{Cell, RefCell};
use std::io::BufRead;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::glib;
use serde_json::{json, Value};
use support::{pump_for, ModuleDirectory};
use webkit2gtk::WebContextExt;
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext::manifest::Manifest;
use zephium_webext_linux::runtime::{
    ActionActivation, InstallSpec, ProfileRuntime, ProfileRuntimeConfig, RuntimeEngine,
};
use zephium_webext_linux::views::{ExtensionPage, PageKind};
use zephium_webext_runtime::context::ExtensionId;
use zephium_webext_runtime::i18n::LocaleEnvironment;
use zephium_webext_runtime::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};

fn control(
    host: &mut Host,
    popup: &mut Option<ExtensionPage>,
    operation: &str,
    args: &Value,
) -> Result<Value, String> {
    match operation {
        "consoleMessages" => Ok(Value::Array(
            host.runtime
                .errors(ExtensionInstallId::from(INSTALL))
                .into_iter()
                .map(|error| json!({"type": "error", "text": error.message}))
                .collect(),
        )),
        "invoke-action" => {
            let tab_id = args["tabId"].as_i64().ok_or("tabId required")?;
            let tab = host
                .shell
                .borrow()
                .order
                .iter()
                .copied()
                .find(|tab| host.runtime.tab_id(*tab).map(i64::from) == Some(tab_id))
                .ok_or("Action target page missing")?;
            host.shell.borrow_mut().active = Some(tab);
            host.publish();
            match host
                .runtime
                .invoke_action(ExtensionInstallId::from(INSTALL), tab)?
            {
                ActionActivation::Popup(_) => {
                    *popup = host
                        .runtime
                        .open_page(ExtensionInstallId::from(INSTALL), PageKind::Popup, None)
                        .ok();
                }
                ActionActivation::Click => {}
            }
            Ok(json!({"supported": true, "method": "native toolbar invocation"}))
        }
        other => Err(format!("Unsupported host operation: {other}")),
    }
}

#[test]
#[ignore = "driven by eval/webext-conformance/run-linux.mjs"]
fn conformance_host() {
    let _session = support::init();
    let extension_dir = PathBuf::from(std::env::var("ZEPHIUM_CONFORMANCE_EXTENSION").unwrap());
    let extension = std::env::var("ZEPHIUM_CONFORMANCE_ID").unwrap();
    let module = ModuleDirectory::new();
    let state = tempfile::tempdir().unwrap();
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    let shell = Rc::new(RefCell::new(Shell::default()));
    let engine: Rc<dyn RuntimeEngine> = Rc::new(Engine {
        extension: extension.clone(),
        shell: shell.clone(),
    });
    let runtime = ProfileRuntime::new(
        &context,
        ProfileRuntimeConfig {
            profile: ProfileId::from(PROFILE),
            incognito: false,
            module_directory: module.path().to_owned(),
            state_directory: Some(state.path().to_owned()),
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
    let manifest = Manifest::load(&extension_dir).expect("manifest");
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    let installed = runtime.install_admitted(InstallSpec {
        install: ExtensionInstallId::from(INSTALL),
        extension_id: ExtensionId::parse(&extension).expect("id"),
        generation: ExtensionRuntimeGeneration::INITIAL,
        directory: extension_dir,
        manifest,
        permissions: InstallPermissions::new(
            declaration,
            &PermissionRequest::default(),
            SiteAccess::OnAllSites,
            false,
        ),
        allowed_in_incognito: false,
        externally_connectable: None,
        unpacked: true,
    });
    if let Err(error) = installed {
        emit(&json!({"event": "install-failed", "error": error.to_string()}));
        return;
    }
    let mut host = Host {
        context,
        runtime,
        shell,
        dirty: Rc::new(Cell::new(false)),
    };
    let mut popup: Option<ExtensionPage> = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while host
        .runtime
        .background(ExtensionInstallId::from(INSTALL))
        .is_none_or(|status| !status.ready)
        && Instant::now() < deadline
    {
        pump_for(Duration::from_millis(10));
        host.work();
    }
    host.open(&format!("chrome-extension://{extension}/runner.html"), true);
    let (lines, input) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    emit(&json!({"event": "ready"}));
    loop {
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        host.work();
        match input.try_recv() {
            Ok(line) => {
                let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                if request["op"] == "quit" {
                    break;
                }
                let result = control(
                    &mut host,
                    &mut popup,
                    request["op"].as_str().unwrap_or(""),
                    &request["args"],
                );
                emit(&match result {
                    Ok(result) => json!({"id": request["id"], "result": result}),
                    Err(error) => json!({"id": request["id"], "error": error}),
                });
            }
            Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(2)),
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
    if std::env::var_os("ZEPHIUM_CONFORMANCE_DEBUG").is_some() {
        let install = ExtensionInstallId::from(INSTALL);
        eprintln!(
            "conformance-host errors={:?} unhandled={:?} tabs={:?}",
            host.runtime.errors(install),
            host.runtime.unhandled_calls(install),
            host.shell
                .borrow()
                .tabs
                .values()
                .map(|tab| tab.url.as_ref().map(url::Url::to_string))
                .collect::<Vec<_>>()
        );
    }
    popup.take();
    let ids: Vec<ItemId> = host.shell.borrow().order.clone();
    for id in ids {
        host.close(id);
    }
    let mut drain = host.runtime.retire();
    let drained = drain.run(Duration::from_secs(10));
    emit(&json!({"event": "closed", "drained": drained}));
    glib::MainContext::default().iteration(false);
    drop(host);
}
