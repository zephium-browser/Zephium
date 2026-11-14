#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

#[path = "runtime_support/mod.rs"]
mod support;

use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

use serde_json::{json, Value};
use webkit2gtk::{LoadEvent, WebContextExt, WebViewExt};
use zephium_core::{extensions::ExtensionRuntimeGeneration, ids::ExtensionInstallId};
use zephium_webext::manifest::Manifest;
use zephium_webext_linux::{
    runtime::{InstallSpec, ProfileRuntime, ProfileRuntimeConfig, RuntimeEngine},
    scheme::Requester,
};
use zephium_webext_runtime::{
    context::ExtensionId,
    i18n::LocaleEnvironment,
    js::{background_surface, surface, BUNDLE, OWNER_BUNDLE, WORKER_BUNDLE},
    permissions::{InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess},
};

struct Engine;

impl RuntimeEngine for Engine {
    fn requester(&self, _: &webkit2gtk::WebView) -> Option<Requester> {
        None
    }
}

fn sample() -> Value {
    let processes: Vec<_> = support::web_processes()
        .into_iter()
        .map(|pid| {
            let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).unwrap();
            let field = |name: &str| -> u64 {
                text.lines()
                    .find_map(|line| line.strip_prefix(name))
                    .unwrap()
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            json!({"pid":pid,"pss_kib":field("Pss:"),"anonymous_kib":field("Anonymous:"),"private_dirty_kib":field("Private_Dirty:")})
        })
        .collect();
    assert_eq!(
        processes.len(),
        1,
        "one hidden owner and its worker share one renderer"
    );
    json!(processes)
}

#[test]
#[ignore = "release memory qualification; use the external runner in the task report"]
fn native_background_memory() -> Result<(), &'static str> {
    if cfg!(debug_assertions) {
        return Err("memory samples require --release");
    }
    let _session = support::init();
    let module = support::ModuleDirectory::new();
    let case = std::env::var("ZEPHIUM_WEBEXT_MEMORY_CASE").expect("memory case");
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    assert!(context.is_sandbox_enabled());
    let engine: Rc<dyn RuntimeEngine> = Rc::new(Engine);
    let runtime = ProfileRuntime::new(
        &context,
        ProfileRuntimeConfig {
            profile: 7u128.into(),
            incognito: false,
            module_directory: module.path().to_owned(),
            state_directory: None,
            locale: LocaleEnvironment {
                ui_language: "en-US".into(),
                accept_languages: vec!["en-US".into()],
                right_to_left: false,
            },
            time_scale: 1,
        },
        engine,
    )
    .unwrap();
    let mut owner = None;
    let mut projected_surface_bytes = None;
    if matches!(case.as_str(), "minimal" | "vimium") {
        let root = if case == "minimal" {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/tiny")
        } else {
            PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_CORPUS").expect("extension corpus"))
                .join("vimium")
        };
        let manifest = Manifest::load(&root).unwrap();
        projected_surface_bytes = Some(background_surface(&manifest).len());
        let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
        let install = ExtensionInstallId::from(1);
        runtime
            .install_admitted(InstallSpec {
                install,
                extension_id: ExtensionId::parse("abcdefghijklmnopabcdefghijklmnop").unwrap(),
                generation: ExtensionRuntimeGeneration::INITIAL,
                directory: root,
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
            })
            .unwrap();
        support::pump_until(|| {
            runtime
                .background(install)
                .is_some_and(|status| status.ready)
        });
    } else {
        let bootstrap = json!({
            "extensionId":"abcdefghijklmnopabcdefghijklmnop", "contextKind":"background",
            "manifest":{"manifest_version":3}, "permissions":[],
            "worker":{"install":false}
        });
        let payload = match case.as_str() {
            "bare" => String::new(),
            "owner-api" => format!("({BUNDLE})('page',{}, {bootstrap});", surface()),
            "owner-relay" => format!("({OWNER_BUNDLE})({{}});"),
            "blank-worker" => "__notify('ready');".into(),
            "worker-modules" => {
                format!("try{{({BUNDLE})('invalid')}}catch(e){{}}__notify('ready');")
            }
            "worker-api" => format!(
                "({BUNDLE})('worker',{}, {bootstrap});__notify('ready');",
                surface()
            ),
            "worker-trimmed" => format!(
                "({WORKER_BUNDLE})({}, {bootstrap});__notify('ready');",
                background_surface(&Manifest::from_value(bootstrap["manifest"].clone()))
            ),
            _ => panic!("unknown memory case {case}"),
        };
        let view = webkit2gtk::WebView::with_context(&context);
        let loaded = Rc::new(Cell::new(false));
        let seen = loaded.clone();
        view.connect_load_changed(move |_, event| {
            if event == LoadEvent::Finished {
                seen.set(true);
            }
        });
        let javascript = if case.contains("worker") {
            format!(
                "const worker=new Worker(URL.createObjectURL(new Blob([{}],{{type:'text/javascript'}})));worker.onmessage=()=>{{document.title='ready'}};worker.onerror=(e)=>{{document.title='error:'+e.message}};globalThis.worker=worker;",
                serde_json::to_string(&format!("const __notify=self.postMessage.bind(self);{payload}")).unwrap()
            )
        } else {
            format!("{payload}document.title='ready';")
        };
        view.load_html(
            &format!("<!doctype html><title>loading</title><script>{javascript}</script>"),
            Some("https://memory.invalid/"),
        );
        support::pump_until(|| {
            loaded.get()
                && view
                    .title()
                    .is_some_and(|title| title == "ready" || title.starts_with("error:"))
        });
        assert_eq!(view.title().as_deref(), Some("ready"));
        owner = Some(view);
    }
    support::pump_for(Duration::from_secs(3));
    println!(
        "WEBEXT_MEMORY {}",
        json!({"case":case,"renderer":sample(),"bundle_bytes":BUNDLE.len(),"surface_bytes":surface().len(),"owner_bytes":OWNER_BUNDLE.len(),"worker_bytes":WORKER_BUNDLE.len(),"projected_surface_bytes":projected_surface_bytes})
    );
    drop(owner);
    let mut drain = runtime.retire();
    assert!(drain.run(Duration::from_secs(10)));
    drop(runtime);
    drop(context);
    support::pump_until(|| support::web_processes().is_empty());
    Ok(())
}
