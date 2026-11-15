use super::*;
use zephium_webext_linux::diagnostics;

const SNAPSHOT: &str = "return JSON.stringify({ready:document.readyState,text:document.body?.innerText.slice(0,2048),path:location.pathname,controls:[...document.querySelectorAll('button,[role=button],a[href],input,select,textarea')].filter(e=>e.getClientRects().length&&!e.disabled).slice(0,40).map(e=>(e.getAttribute('aria-label')||e.textContent||e.getAttribute('placeholder')||'').trim().slice(0,120)),dark:getComputedStyle(document.body).backgroundColor,cookieBanner:(()=>{const e=document.getElementById('cookieBanner'),s=document.getElementById('fixtureSentinel');return e?{exists:true,display:getComputedStyle(e).display,visibility:getComputedStyle(e).visibility,rects:e.getClientRects().length,sentinelVisible:!!s?.getClientRects().length}:null})(),videoSpeed:document.querySelector('video')?.playbackRate});";

#[test]
#[ignore = "release-only pinned package capacity measurement in a private native session"]
fn capacity20() {
    if cfg!(debug_assertions) {
        panic!("capacity measurements require a release host");
    }
    let id = std::env::var("ZEPHIUM_WEBEXT_DIAGNOSTIC_EXTENSION").unwrap();
    let evidence = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").unwrap());
    let directory = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_BREADTH_PACKAGE").unwrap());
    let _diagnostic = DiagnosticDump(diagnostics::begin(&evidence, &id).unwrap());
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
    let (mut host, _engine) = host(web_context, &module, state.path(), &id);
    let tabs: Vec<_> = (0..20)
        .map(|_| host.open(&fixture.url("/breadth"), true))
        .collect();
    let views: Vec<_> = tabs
        .iter()
        .map(|tab| host.shell.borrow().tabs[tab].view.clone())
        .collect();
    let mapped: Vec<_> = views.iter().map(MappedPage::new).collect();
    pump(&mut host, Duration::from_secs(6));
    let wait_ready = |host: &mut Host| {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if views.iter().all(|view| !view.is_loading()) {
                break;
            }
            pump(host, Duration::from_millis(100));
        }
        pump(host, Duration::from_secs(2));
    };
    wait_ready(&mut host);
    let sample = |phase: &str| {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
        let ticks: u64 = stat
            .rsplit_once(") ")
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse()
            .unwrap();
        let namespace = std::fs::read_link("/proc/self/ns/pid").unwrap();
        let ready: Vec<_> = views
            .iter()
            .map(|view| evaluate_bounded(view, "return document.readyState;"))
            .collect();
        println!(
            "CAPACITY_SAMPLE {}",
            json!({"extension":id,"phase":phase,"pid":std::process::id(),"startTicks":ticks,"pidNamespace":namespace.to_string_lossy(),"tabs":20,"ready":ready})
        );
    };
    sample("without-extension");
    pump(&mut host, Duration::from_secs(1));
    match host.runtime.install_admitted(spec(&directory, &id, 1)) {
        Ok(()) => {
            pump(&mut host, Duration::from_secs(3));
            for view in &views {
                view.reload();
            }
            pump(&mut host, Duration::from_secs(10));
            wait_ready(&mut host);
            if let Ok(files) = std::env::var("ZEPHIUM_WEBEXT_CAPACITY_FILES") {
                use zephium_webkit_web_process_protocol::protocol::{
                    InjectRequest, InjectionKind, InjectionTarget, MAX_SCRIPT_SOURCE_BYTES,
                };
                let install = ExtensionInstallId::from(INSTALL);
                for file in files.split_whitespace() {
                    let path = zephium_webext::manifest::normalize_resource(file)
                        .expect("package source path");
                    let source = std::fs::read_to_string(directory.join(&path))
                        .expect("verified package file");
                    assert!(source.len() <= MAX_SCRIPT_SOURCE_BYTES);
                    let source: Arc<str> = source.into();
                    println!(
                        "CAPACITY_FORCED_SOURCE {}",
                        json!({"path":path,"bytes":source.len(),"tabs":tabs.len(),"scope":"synthetic localhost evaluation, not extension feature acceptance"})
                    );
                    for tab in &tabs {
                        let attached = host.shell.borrow().tabs[tab].attached;
                        host.runtime
                            .bridge()
                            .inject(
                                attached,
                                install,
                                InjectRequest {
                                    install_id: String::new(),
                                    token: String::new(),
                                    generation: 0,
                                    request_id: 0,
                                    target: InjectionTarget::AllFrames,
                                    world: zephium_webext_linux::bridge::world_name(
                                        install,
                                        &zephium_webext_runtime::context::InjectedWorld::Isolated,
                                    ),
                                    admission_world: zephium_webext_linux::bridge::world_name(
                                        install,
                                        &zephium_webext_runtime::context::InjectedWorld::Isolated,
                                    ),
                                    kind: InjectionKind::Script,
                                    source: source.clone(),
                                    source_fd: None,
                                    source_url: format!("chrome-extension://{id}/{path}"),
                                    args_json: "[]".into(),
                                    timeout_ms: 30000,
                                },
                            )
                            .expect("admitted native document source");
                    }
                    pump(&mut host, Duration::from_secs(30));
                }
                wait_ready(&mut host);
            }
            sample("with-extension");
            pump(&mut host, Duration::from_secs(1));
            snapshot(&id, "CapacityFirstTab", &views[0]);
            snapshot(&id, "CapacityLastTab", &views[19]);
        }
        Err(error) => panic!(
            "CAPACITY_INSTALL_ERROR {}",
            json!({"extension":id,"error":error.to_string()})
        ),
    }
    drop(mapped);
    drop(views);
    for tab in tabs {
        host.close(tab);
    }
}

fn evaluate_bounded(view: &webkit2gtk::WebView, source: &str) -> String {
    let result = Rc::new(RefCell::new(None));
    let observed = result.clone();
    let cancel = gtk::gio::Cancellable::new();
    view.call_async_javascript_function(source, None, None, None, Some(&cancel), move |value| {
        *observed.borrow_mut() = Some(
            value
                .map(|value| value.to_string())
                .unwrap_or_else(|error| format!("error: {error}")),
        );
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while result.borrow().is_none() && Instant::now() < deadline {
        gtk::main_iteration_do(false);
        std::thread::sleep(Duration::from_millis(1));
    }
    cancel.cancel();
    let value = result
        .borrow_mut()
        .take()
        .unwrap_or_else(|| "error: native page evaluation timed out after 5 seconds".into());
    value
}

fn snapshot(id: &str, kind: &str, view: &webkit2gtk::WebView) {
    let result = evaluate_bounded(view, SNAPSHOT);
    diagnostics::record(
        id,
        kind,
        json!({"kind":"breadth-page-state","state":serde_json::from_str::<Value>(&result).unwrap_or(json!({"evaluationError":result}))}),
    );
}

fn interaction(id: &str, kind: &str, view: &webkit2gtk::WebView, source: &str) {
    let result = evaluate_bounded(view, source);
    diagnostics::record(
        id,
        kind,
        json!({"kind":"breadth-interaction", "state":serde_json::from_str::<Value>(&result).unwrap_or(json!({"evaluationError":result}))}),
    );
}

#[test]
#[ignore = "pinned public package census in a private native session"]
fn capture() {
    let id = std::env::var("ZEPHIUM_WEBEXT_DIAGNOSTIC_EXTENSION").unwrap();
    let evidence = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_DIAGNOSTICS_DIR").unwrap());
    let directory = PathBuf::from(std::env::var_os("ZEPHIUM_WEBEXT_BREADTH_PACKAGE").unwrap());
    let _diagnostic = DiagnosticDump(diagnostics::begin(&evidence, &id).unwrap());
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let state = tempfile::tempdir().unwrap();
    let web_context = context();
    web_context.set_preferred_languages(&["en-US", "en"]);
    let mut proxy = webkit2gtk::NetworkProxySettings::new(
        Some("http://127.0.0.1:9"),
        &["127.0.0.1", "localhost"],
    );
    web_context
        .website_data_manager()
        .unwrap()
        .set_network_proxy_settings(webkit2gtk::NetworkProxyMode::Custom, Some(&mut proxy));
    let (mut host, _engine) = host(web_context, &module, state.path(), &id);
    let install = ExtensionInstallId::from(INSTALL);
    let spec = spec(&directory, &id, 1);
    let manifest = spec.manifest.raw().clone();
    let options = spec.manifest.options_page();
    diagnostics::record(
        &id,
        "host",
        json!({"kind":"breadth-package","manifest":manifest}),
    );
    if id == UBOL.0 {
        let tab = host.open(&fixture.url("/breadth"), true);
        let view = host.shell.borrow().tabs[&tab].view.clone();
        let mapped = MappedPage::new(&view);
        pump(&mut host, Duration::from_secs(2));
        interaction(
            &id,
            "FilteringAbsent",
            &view,
            include_str!("../../../../eval/webext-conformance/fixtures/breadth/filter.js"),
        );
        diagnostics::record(
            &id,
            "host",
            json!({"kind":"breadth-filter-absent-hits","hits":fixture.take()}),
        );
        drop(mapped);
        drop(view);
        host.close(tab);
    }
    if let Err(error) = host.runtime.install_admitted(spec) {
        diagnostics::record(
            &id,
            "host",
            json!({"kind":"breadth-install","error":error.to_string()}),
        );
        return;
    }
    diagnostics::record(
        &id,
        "host",
        json!({"kind":"breadth-install","installed":true}),
    );
    let tab = host.open("about:blank", true);
    let view = host.shell.borrow().tabs[&tab].view.clone();
    let mapped = MappedPage::new(&view);
    mapped.0.resize(1000, 700);
    view.load_uri(&fixture.url("/breadth"));
    pump(&mut host, Duration::from_secs(5));
    snapshot(&id, "ContentScript", &view);
    if id == "nkbihfbeogaeaoehlefnkodbefgpgknn" {
        interaction(
            &id,
            "WalletProvider",
            &view,
            include_str!("../../../../eval/webext-conformance/fixtures/breadth/provider.js"),
        );
    }
    interaction(
        &id,
        "Media",
        &view,
        include_str!("../../../../eval/webext-conformance/fixtures/breadth/media.js"),
    );
    if id == VIMIUM.0 {
        view.grab_focus();
        evaluate(
            &view,
            include_str!("../../../../eval/webext-conformance/fixtures/breadth/prepare-hints.js"),
        );
        keys("f");
        pump(&mut host, Duration::from_millis(500));
        interaction(
            &id,
            "VimiumHints",
            &view,
            include_str!("../../../../eval/webext-conformance/fixtures/breadth/hints.js"),
        );
        keys("Escape");
        pump(&mut host, Duration::from_millis(100));
        interaction(
            &id,
            "VimiumHintsClosed",
            &view,
            include_str!("../../../../eval/webext-conformance/fixtures/breadth/hints.js"),
        );
    }
    host.shell.borrow_mut().active = Some(tab);
    host.publish();
    let activation = host.runtime.invoke_action(install, tab);
    match activation {
        Ok(zephium_webext_linux::runtime::ActionActivation::Popup(path)) => {
            diagnostics::record(&id, "host", json!({"kind":"breadth-action","popup":path}));
            let prefix = format!("chrome-extension://{id}/");
            let resource = path.strip_prefix(&prefix).expect("own action URL");
            match host
                .runtime
                .open_page(install, PageKind::Popup, Some(resource))
            {
                Ok(page) => {
                    let _mapped = MappedPage::new(page.view());
                    pump(&mut host, Duration::from_secs(15));
                    snapshot(&id, "Popup", page.view());
                    if id == UBOL.0 {
                        interaction(&id, "FilteringConfiguration", page.view(), include_str!("../../../../eval/webext-conformance/fixtures/breadth/filter-config.js"));
                    }
                }
                Err(error) => diagnostics::record(
                    &id,
                    "Popup",
                    json!({"kind":"breadth-page-error","error":format!("{error:?}")}),
                ),
            }
        }
        Ok(zephium_webext_linux::runtime::ActionActivation::Click) => {
            diagnostics::record(&id, "host", json!({"kind":"breadth-action","click":true}));
            pump(&mut host, Duration::from_secs(15));
        }
        Err(error) => diagnostics::record(
            &id,
            "host",
            json!({"kind":"breadth-action","error":error.to_string()}),
        ),
    }
    let tabs: Vec<_> = host
        .shell
        .borrow()
        .tabs
        .values()
        .map(|tab| tab.view.clone())
        .collect();
    for page in tabs {
        if page
            .uri()
            .is_some_and(|uri| uri.starts_with("chrome-extension://"))
        {
            let _mapped = MappedPage::new(&page);
            pump(&mut host, Duration::from_secs(2));
            snapshot(&id, "ExtensionTab", &page);
        }
    }
    if let Some(path) = options {
        let options_tab = host.open(&format!("chrome-extension://{id}/{path}"), true);
        let options_view = host.shell.borrow().tabs[&options_tab].view.clone();
        let _mapped = MappedPage::new(&options_view);
        pump(&mut host, Duration::from_secs(5));
        snapshot(&id, "Options", &options_view);
        host.close(options_tab);
    } else {
        diagnostics::record(&id, "Options", json!({"kind":"options-not-declared"}));
    }
    if id == UBOL.0 {
        interaction(
            &id,
            "Filtering",
            &view,
            include_str!("../../../../eval/webext-conformance/fixtures/breadth/filter.js"),
        );
        diagnostics::record(
            &id,
            "host",
            json!({"kind":"breadth-filter-hits","hits":fixture.take()}),
        );
    }
    view.load_uri(&fixture.url("/breadth.json"));
    pump(&mut host, Duration::from_secs(3));
    snapshot(&id, "JsonDocument", &view);
    interaction(
        &id,
        "JsonControls",
        &view,
        include_str!("../../../../eval/webext-conformance/fixtures/breadth/json.js"),
    );
    drop(mapped);
    let tabs: Vec<_> = host.shell.borrow().tabs.keys().copied().collect();
    for tab in tabs {
        host.close(tab);
    }
    let before = host.runtime.background(install);
    pump(&mut host, Duration::from_secs(60));
    let after = host.runtime.background(install);
    if let Some(network) = host.runtime.network_status(install) {
        diagnostics::record(
            &id,
            "host",
            json!({"kind":"breadth-network-status", "loading":network.loading,"activeRules":network.active_rule_count,"unsupportedRules":network.unsupported_rule_count,"error":network.error,"limitations":network.limitations}),
        );
    }
    let record = json!({"kind":"breadth-idle","before":format!("{before:?}"),"after":format!("{after:?}"),"seconds":60,"unhandled":host.runtime.unhandled_calls(install),"errors":host.runtime.errors(install).iter().map(|error|format!("{error:?}")).collect::<Vec<_>>()});
    diagnostics::record(&id, "host", record.clone());
    println!("BREADTH_IDLE {record}");
    std::io::stdout().flush().unwrap();
    pump(&mut host, Duration::from_secs(2));
    let mut drain = host.runtime.retire();
    assert!(drain.run(Duration::from_secs(10)), "native teardown");
}
