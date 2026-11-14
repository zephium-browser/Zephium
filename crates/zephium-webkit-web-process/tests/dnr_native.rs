#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![deny(unsafe_op_in_unsafe_fn)]

#[path = "native/dnr_filter.rs"]
mod filter;
mod support;

use gio::prelude::UnixFDListExtManual;
use gtk::glib::{self, prelude::ObjectExt, variant::ToVariant};
use serde_json::{json, Value};
use std::{
    cell::RefCell,
    fs::File,
    io::Write,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};
use support::{evaluate, load, pump_for, pump_until, send_raw, Fixture, ModuleDirectory};
use webkit2gtk::{
    SecurityManagerExt, URISchemeRequestExt, UserMessageExt, WebContextExt, WebViewExt,
    WebsiteDataManagerExt, WebsiteDataManagerExtManual, WebsiteDataTypes,
};
use zephium_webext_dnr::{parse_rules_json, CompiledRuleset, RulesetKind};
use zephium_webkit_web_process_protocol::{
    dnr::{DnrExtensionPolicy, DnrPolicySnapshot, DnrResourceAccess},
    protocol::{
        decode_event, encode_ui, Attach, Event, PolicyUpdate, UiMessage, EVENT_MESSAGE,
        PROTOCOL_VERSION,
    },
};

const TOKEN: &str = "isolated-dnr-native-policy";
const EXTENSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECOND: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
type Events = Rc<RefCell<Vec<Event>>>;

fn send(page: &webkit2gtk::WebView, message: &UiMessage, fds: Option<&gio::UnixFDList>) -> bool {
    send_raw(
        page,
        message.name(),
        Some(&encode_ui(message).unwrap().to_variant()),
        fds,
    )
    .ok()
    .and_then(|reply| reply.parameters())
    .and_then(|value| value.get::<bool>())
    .unwrap_or(false)
}

fn publish(page: &webkit2gtk::WebView, generation: u64, extensions: &[DnrExtensionPolicy]) {
    use rustix::fs::{fcntl_add_seals, memfd_create, MemfdFlags, SealFlags};
    let bytes = DnrPolicySnapshot::encode(extensions).unwrap();
    let mut file = File::from(
        memfd_create(
            "dnr-native",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap(),
    );
    file.write_all(&bytes).unwrap();
    fcntl_add_seals(
        &file,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    )
    .unwrap();
    let fds = gio::UnixFDList::new();
    let fd_index = fds.append(file).unwrap().try_into().unwrap();
    assert!(
        send(
            page,
            &UiMessage::Policy(PolicyUpdate {
                token: TOKEN.into(),
                generation: 1,
                policy_generation: generation,
                fd_index,
                length: bytes.len().try_into().unwrap(),
            }),
            Some(&fds)
        ),
        "policy generation {generation}"
    );
}

fn rules(id: &str, kind: RulesetKind, values: Value) -> Vec<u8> {
    let parsed = parse_rules_json(&serde_json::to_vec(&values).unwrap(), kind).unwrap();
    assert!(parsed.rejected.is_empty(), "{:?}", parsed.rejected);
    CompiledRuleset::compile(id, kind, parsed.rules)
        .unwrap()
        .to_bytes()
}

fn extension(id: &str, order: u64, rulesets: Vec<Vec<u8>>) -> DnrExtensionPolicy {
    DnrExtensionPolicy {
        extension_id: id.into(),
        install_order: order,
        host_permissions: vec!["<all_urls>".into()],
        require_host_access: false,
        web_accessible_resources: vec![DnrResourceAccess {
            resources: vec!["public.js".into()],
            matches: vec!["<all_urls>".into()],
            extension_ids: vec![],
        }],
        rulesets,
    }
}

fn observe(page: &webkit2gtk::WebView) -> Events {
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        let event = decode_event(&message.parameters().unwrap().get::<Vec<u8>>().unwrap()).unwrap();
        assert!(
            captured.borrow().len() < 4096,
            "bounded DNR test event capture"
        );
        captured.borrow_mut().push(event);
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    events
}

fn attach(page: &webkit2gtk::WebView, tab_id: i32) {
    assert!(send(
        page,
        &UiMessage::Attach(Attach {
            token: TOKEN.into(),
            generation: 1,
            tab_id,
            observe_requests: true,
            ..Attach::default()
        }),
        None
    ));
}

fn fetch(page: &webkit2gtk::WebView, url: &str) -> Value {
    serde_json::from_str(&evaluate(page, &format!(
        "try {{ const r=await fetch({}); return JSON.stringify({{url:r.url,body:await r.text()}}); }} catch(e) {{ return JSON.stringify({{error:String(e)}}); }}",
        serde_json::to_string(url).unwrap()), None)).unwrap()
}

fn hit(fixture: &Fixture, suffix: &str) -> bool {
    fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.path.contains(suffix))
}

#[test]
#[ignore = "sandboxed WebKitGTK DNR; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_dnr_enforcement_and_resource_coverage() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let favicon_directory = tempfile::tempdir().unwrap();
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    context.set_favicon_database_directory(favicon_directory.path().to_str());
    let security = context.security_manager().unwrap();
    security.register_uri_scheme_as_secure("chrome-extension");
    security.register_uri_scheme_as_cors_enabled("chrome-extension");
    context.register_uri_scheme("chrome-extension", |request| {
        let path = request.path().unwrap_or_default();
        // uBOL redirect targets get an inert body: corpus files are never executed.
        let bytes = if path.starts_with("/web_accessible_resources/")
            && std::env::var_os("ZEPHIUM_DNR_UBOL_ROOT").is_some()
        {
            glib::Bytes::from_static(b"/* uBOL web-accessible resource placeholder */")
        } else {
            assert_eq!(path, "/public.js");
            glib::Bytes::from_static(b"globalThis.dnrExtensionBody='public-extension-resource';")
        };
        request.finish(
            &gio::MemoryInputStream::from_bytes(&bytes),
            bytes.len().try_into().unwrap(),
            Some("text/javascript"),
        );
    });
    let page = webkit2gtk::WebView::with_context(&context);
    let events = observe(&page);
    load(&page, &format!("{}/frame?inert", fixture.origin));
    attach(&page, 41);
    load(&page, &format!("{}/frame?attached", fixture.origin));
    support::assert_sandbox(&module);

    let base = json!([
        {"id":1,"action":{"type":"block"},"condition":{"urlFilter":"/dnr-block"}},
        {"id":2,"priority":2,"action":{"type":"allow"},"condition":{"urlFilter":"/dnr-block-allowed"}},
        {"id":3,"action":{"type":"redirect","redirect":{"transform":{"path":"/dnr-actual","queryTransform":{"removeParams":["remove"],"addOrReplaceParams":[{"key":"added","value":"yes"}]}}}},"condition":{"urlFilter":"/dnr-transform"}},
        {"id":4,"action":{"type":"modifyHeaders","requestHeaders":[{"header":"x-native-test","operation":"set","value":"dnr-native"}]},"condition":{"urlFilter":"/dnr-headers"}},
        {"id":5,"action":{"type":"redirect","redirect":{"extensionPath":"/public.js"}},"condition":{"urlFilter":"/dnr-extension-public"}},
        {"id":6,"action":{"type":"redirect","redirect":{"extensionPath":"/private.js"}},"condition":{"urlFilter":"/dnr-extension-private"}},
        {"id":7,"action":{"type":"upgradeScheme"},"condition":{"urlFilter":"/dnr-upgrade","resourceTypes":["main_frame"]}},
        {"id":8,"action":{"type":"allow"},"condition":{"urlFilter":"/dnr-builtin"}},
        {"id":9,"action":{"type":"redirect","redirect":{"url":format!("{}/dnr-actual?direct", fixture.origin)}},"condition":{"urlFilter":"/dnr-url"}},
        {"id":10,"action":{"type":"redirect","redirect":{"regexSubstitution":format!("{}/dnr-actual?captured=\\1", fixture.origin)}},"condition":{"regexFilter":"^http://[^/]+/dnr-regex/([a-z]+)$"}}
    ]);
    let first = extension(
        EXTENSION,
        1,
        vec![rules("_dynamic", RulesetKind::Dynamic, base.clone())],
    );
    publish(&page, 1, std::slice::from_ref(&first));
    assert!(fetch(&page, "/dnr-block")["error"].is_string());
    assert!(!hit(&fixture, "/dnr-block"));
    assert!(fetch(&page, "/dnr-block-allowed")["body"].is_string());
    assert!(hit(&fixture, "/dnr-block-allowed"));
    let transformed = fetch(&page, "/dnr-transform?remove=no&keep=yes");
    assert_eq!(transformed["body"], "/dnr-actual?keep=yes&added=yes");
    assert_eq!(fetch(&page, "/dnr-url")["body"], "/dnr-actual?direct");
    assert_eq!(
        fetch(&page, "/dnr-regex/native")["body"],
        "/dnr-actual?captured=native"
    );
    assert_eq!(fetch(&page, "/dnr-headers")["body"], "dnr-native");
    assert!(fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.header == "dnr-native"));
    assert_eq!(evaluate(&page, "await new Promise((resolve,reject)=>{const s=document.createElement('script');s.src='/dnr-extension-public';s.onload=resolve;s.onerror=reject;document.head.append(s)});return dnrExtensionBody;", None), "public-extension-resource");
    assert!(
        fetch(&page, "/dnr-extension-private")["error"].is_string(),
        "private extension redirect must be refused"
    );
    assert!(!hit(&fixture, "/dnr-extension-private"));

    let second = extension(
        SECOND,
        2,
        vec![rules(
            "second",
            RulesetKind::Static,
            json!([
                {"id":1,"action":{"type":"block"},"condition":{"urlFilter":"/dnr-block-allowed"}}
            ]),
        )],
    );
    publish(&page, 2, &[first.clone(), second]);
    assert!(fetch(&page, "/dnr-block-allowed?two-extensions")["error"].is_string());
    assert!(!hit(&fixture, "two-extensions"));

    let mut session = first.clone();
    session.rulesets.push(rules(
        "_session",
        RulesetKind::Session,
        json!([
            {"id":1,"action":{"type":"block"},"condition":{"urlFilter":"/dnr-tab","tabIds":[41]}}
        ]),
    ));
    publish(&page, 3, std::slice::from_ref(&session));
    assert!(fetch(&page, "/dnr-tab")["error"].is_string());
    let other = webkit2gtk::WebView::with_context(&context);
    let other_events = observe(&other);
    load(&other, &format!("{}/frame?second-tab", fixture.origin));
    attach(&other, 42);
    publish(&other, 1, &[session]);
    assert!(fetch(&other, &format!("{}/dnr-tab?other-tab", fixture.origin))["body"].is_string());
    publish(&page, 4, &[]);
    assert!(fetch(&page, "/dnr-block?after-update")["body"].is_string());
    assert!(fetch(&page, "/dnr-tab?after-update")["body"].is_string());
    pump_until(|| !other_events.borrow().is_empty());
    other.stop_loading();
    drop(other);

    publish(&page, 5, std::slice::from_ref(&first));
    filter::install(
        &page,
        r#"[{"trigger":{"url-filter":"/dnr-builtin"},"action":{"type":"block"}}]"#,
    );
    assert!(fetch(&page, "/dnr-builtin")["error"].is_string());
    assert!(!hit(&fixture, "/dnr-builtin"));
    assert!(fetch(&page, "/dnr-block?with-builtin")["error"].is_string());
    assert!(!hit(&fixture, "with-builtin"));

    let document_rules = extension(
        EXTENSION,
        1,
        vec![rules(
            "_dynamic",
            RulesetKind::Dynamic,
            json!([
                {"id":1,"action":{"type":"block"},"condition":{"urlFilter":"/dnr-resource"}},
                {"id":2,"priority":2,"action":{"type":"allowAllRequests"},"condition":{"urlFilter":"/dnr-allowed-document","resourceTypes":["main_frame"]}},
                {"id":3,"action":{"type":"block"},"condition":{"urlFilter":"/dnr-denied-document","resourceTypes":["main_frame"]}}
            ]),
        )],
    );
    publish(&page, 6, &[document_rules]);
    load(&page, &format!("{}/dnr-denied-document", fixture.origin));
    assert!(
        !hit(&fixture, "/dnr-denied-document"),
        "main document requires native send-request enforcement"
    );
    load(&page, &format!("{}/dnr-allowed-document", fixture.origin));
    assert!(fetch(&page, "/dnr-resource?allow-tree")["body"].is_string());
    evaluate(&page, "await new Promise(resolve=>{const f=document.createElement('iframe');f.src='/dnr-frame?allow-tree-child';f.onload=resolve;document.body.append(f)});return true;", None);
    pump_until(|| hit(&fixture, "probe=frame_fetch"));
    load(&page, &format!("{}/frame?reset-tree", fixture.origin));
    assert!(fetch(&page, "/dnr-resource?reset-tree")["error"].is_string());

    publish(&page, 7, std::slice::from_ref(&first));
    let failure = Rc::new(RefCell::new(String::new()));
    let captured = failure.clone();
    let signal = page.connect_load_failed(move |_, _, uri, _| {
        *captured.borrow_mut() = uri.to_owned();
        false
    });
    load(&page, &format!("{}/dnr-upgrade", fixture.origin));
    page.disconnect(signal);
    assert!(
        failure.borrow().starts_with("https://"),
        "upgrade failure URI: {}",
        failure.borrow()
    );
    assert!(!hit(&fixture, "/dnr-upgrade"));
    load(&page, &format!("{}/frame?feedback", fixture.origin));
    pump_for(Duration::from_millis(350));
    assert!(events.borrow().iter().any(|event| matches!(event, Event::Requests { observations, .. } if observations.iter().any(|row| !row.matched_rules.is_empty()))));
    println!("DNR native actions block=true priority_allow=true transform=true extension_redirect=true private_war_refused=true request_headers=true two_extensions=true atomic_next_request=true per_tab=true builtin_union=true main_frame=true allow_tree=true upgrade_failure_https=true feedback=true");

    publish(&page, 8, &[]);
    resource_coverage(&page, &fixture, &events, &fixture.origin, true);
    let mut proxy = webkit2gtk::NetworkProxySettings::new(Some(&fixture.origin), &[]);
    context
        .website_data_manager()
        .unwrap()
        .set_network_proxy_settings(webkit2gtk::NetworkProxyMode::Custom, Some(&mut proxy));
    // Public HTTP hosts are not potentially trustworthy: WebKit sends no fetch metadata.
    resource_coverage(&page, &fixture, &events, "http://dnr-insecure.test", false);
    if let Some(root) = std::env::var_os("ZEPHIUM_DNR_UBOL_ROOT") {
        corpus_smoke(&page, &fixture, &events, Path::new(&root));
    }
    load(&page, "about:blank");
    page.stop_loading();
    drop(page);
    drop(context);
}

#[test]
#[ignore = "sandboxed WebKitGTK DNR; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_dnr_service_worker_coverage() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = observe(&page);
    load(&page, &format!("{}/frame?inert", fixture.origin));
    attach(&page, 43);
    load(&page, &format!("{}/dnr-sw-page", fixture.origin));
    pump_until(|| {
        evaluate(&page, "return String(globalThis.dnrProbes?.serviceWorker === true || 'serviceWorkerError' in globalThis.dnrProbes);", None) == "true"
    });
    pump_until(|| hit(&fixture, "probe=service_worker_fetch"));
    pump_for(Duration::from_millis(350));
    for kind in ["service_worker_script", "service_worker_fetch"] {
        let native: Vec<_> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Requests { observations, .. } => Some(observations),
                _ => None,
            })
            .flatten()
            .filter(|row| row.url.split("probe=").nth(1) == Some(kind))
            .map(|row| row.resource_type.clone())
            .collect();
        println!(
            "DNR resource {}",
            json!({"kind":kind,"trustworthy":true,"server":hit(&fixture, &format!("probe={kind}")),"module_types":native})
        );
        // The registration script fetch reaches the page hook only when WebKit
        // runs the job in the client's page context; worker fetches never do.
        let allowed: &[&str] = if kind == "service_worker_script" {
            &["Worker"]
        } else {
            &[]
        };
        assert!(
            native
                .iter()
                .all(|observed| allowed.contains(&observed.as_str())),
            "update the documented {kind} limitation: {native:?}"
        );
    }
    evaluate(&page, "await Promise.all((await navigator.serviceWorker.getRegistrations()).map(r=>r.unregister())); return true;", None);
    let stopped = clear_service_workers(&page);
    // A cross-site navigation leaves the worker's host process without pages.
    load(
        &page,
        &format!("{}/frame?after-service-worker", fixture.cross_origin),
    );
    // WebKit keeps a process that hosted a service worker alive for a delay
    // after the worker stops; await exactly that, so the generic teardown check
    // still proves nothing else outlives the test.
    support::pump_until(|| support::web_processes().len() <= 1);
    println!(
        "DNR service_worker_host_exit_after_stop_ms={}",
        stopped.elapsed().as_millis()
    );
    page.stop_loading();
    drop(page);
    drop(context);
}

fn clear_service_workers(page: &webkit2gtk::WebView) -> Instant {
    let cleared = std::sync::Arc::new(std::sync::Mutex::new(None));
    let completed = cleared.clone();
    page.context()
        .unwrap()
        .website_data_manager()
        .unwrap()
        .clear(
            WebsiteDataTypes::SERVICE_WORKER_REGISTRATIONS | WebsiteDataTypes::DOM_CACHE,
            glib::TimeSpan(0),
            None::<&gio::Cancellable>,
            move |result| *completed.lock().unwrap() = Some(result),
        );
    pump_until(|| cleared.lock().unwrap().is_some());
    cleared.lock().unwrap().take().unwrap().unwrap();
    Instant::now()
}

fn resource_coverage(
    page: &webkit2gtk::WebView,
    fixture: &Fixture,
    events: &Events,
    origin: &str,
    trustworthy: bool,
) {
    events.borrow_mut().clear();
    fixture.requests.lock().unwrap().clear();
    load(page, &format!("{origin}/dnr-types?probe=main_frame"));
    pump_for(Duration::from_secs(2));
    println!("DNR resource_script {}", evaluate(page, "const audio=document.querySelector('audio');return JSON.stringify({...dnrProbes,origin:location.origin,mediaSupported:audio.canPlayType('audio/wav'),mediaNetworkState:audio.networkState,mediaReadyState:audio.readyState,mediaError:audio.error?.message,webTransport:typeof WebTransport,webBundle:HTMLScriptElement.supports?.('webbundle')});", None));
    for kind in [
        "main_frame",
        "sub_frame",
        "script",
        "module",
        "module_import",
        "stylesheet",
        "image",
        "font",
        "media",
        "object",
        "fetch",
        "xhr",
        "worker",
        "worker_import",
        "worker_fetch",
        "beacon",
        "ping",
        "csp_report",
        "preload",
        "prefetch",
        "favicon",
        "websocket",
    ] {
        let suffix = format!("probe={kind}");
        let native: Vec<_> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Requests { observations, .. } => Some(observations),
                _ => None,
            })
            .flatten()
            .filter(|row| row.url.split("probe=").nth(1) == Some(kind))
            .map(|row| row.resource_type.clone())
            .collect();
        let served: Vec<_> = fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path.contains(&suffix))
            .map(|request| (request.destination.clone(), request.accept.clone()))
            .collect();
        println!(
            "DNR resource {}",
            json!({"kind":kind,"trustworthy":trustworthy,"server":!served.is_empty(),"module_types":native,"fetch_dest_accept":served})
        );
        let expected = match kind {
            "main_frame" => Some("MainFrame"),
            "sub_frame" => Some("SubFrame"),
            "object" => Some("Object"),
            "stylesheet" => Some("Stylesheet"),
            "image" => Some("Image"),
            "script" | "preload" if trustworthy => Some("Script"),
            "fetch" | "xhr" | "worker_fetch" if trustworthy => Some("Fetch"),
            "beacon" if trustworthy => Some("Ping"),
            _ => None,
        };
        if let Some(expected) = expected {
            assert!(!served.is_empty(), "fixture must issue {kind}");
            assert!(
                native.iter().all(|observed| observed == expected) && !native.is_empty(),
                "{kind} must be observed as {expected}: {native:?}"
            );
        }
        if kind == "websocket" && trustworthy {
            assert!(!served.is_empty());
            assert!(
                native.is_empty(),
                "update documented limitation if WebKit gains WebSocket interception"
            );
        }
    }
    load(
        page,
        &format!("{}/frame?coverage-finished", fixture.cross_origin),
    );
}

fn corpus_smoke(page: &webkit2gtk::WebView, fixture: &Fixture, events: &Events, root: &Path) {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let started = Instant::now();
    let mut compiled = Vec::new();
    let mut count = 0;
    for resource in manifest["declarative_net_request"]["rule_resources"]
        .as_array()
        .unwrap()
    {
        if resource["enabled"] != true {
            continue;
        }
        let path = resource["path"].as_str().unwrap().trim_start_matches('/');
        let values: Value =
            serde_json::from_slice(&std::fs::read(root.join(path)).unwrap()).unwrap();
        count += values.as_array().unwrap().len();
        compiled.push(rules(
            resource["id"].as_str().unwrap(),
            RulesetKind::Static,
            values,
        ));
    }
    println!(
        "DNR ubOL compile rules={count} bytes={} elapsed_ms={:.3}",
        compiled.iter().map(Vec::len).sum::<usize>(),
        started.elapsed().as_secs_f64() * 1000.0
    );
    let mut policy = extension(EXTENSION, 1, compiled);
    policy.web_accessible_resources =
        serde_json::from_value::<Vec<Value>>(manifest["web_accessible_resources"].clone())
            .unwrap()
            .into_iter()
            .map(|row| DnrResourceAccess {
                resources: row["resources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().into())
                    .collect(),
                matches: row["matches"]
                    .as_array()
                    .map(|a| a.iter().map(|s| s.as_str().unwrap().into()).collect())
                    .unwrap_or_default(),
                extension_ids: vec![],
            })
            .collect();
    let measured_url = "http://dnr-fixture.test/dnr-types";
    for (generation, enabled) in (9..).zip([false, true]) {
        publish(
            page,
            generation,
            if enabled {
                std::slice::from_ref(&policy)
            } else {
                &[]
            },
        );
        load(page, measured_url);
        let mut samples = Vec::new();
        events.borrow_mut().clear();
        for _ in 0..10 {
            let start = Instant::now();
            load(page, measured_url);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        pump_for(Duration::from_millis(350));
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let variance = samples
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / samples.len() as f64;
        println!(
            "DNR page_load {}",
            json!({"ubol":enabled,"build":if cfg!(debug_assertions){"debug"}else{"release"},"samples_ms":samples,"mean_ms":mean,"variance_ms2":variance})
        );
        let mut timing: Vec<_> = events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                Event::Requests { observations, .. } => Some(observations),
                _ => None,
            })
            .flatten()
            .map(|row| row.evaluation_time_ns)
            .filter(|value| *value > 0)
            .collect();
        timing.sort_unstable();
        if !timing.is_empty() {
            println!(
                "DNR module_cost {}",
                json!({"ubol":enabled,"samples":timing.len(),"p50_ns":timing[timing.len()/2],"p99_ns":timing[(timing.len()-1)*99/100]})
            );
        }
    }
    for (generation, enabled) in (11..).zip([false, true]) {
        publish(
            page,
            generation,
            if enabled {
                std::slice::from_ref(&policy)
            } else {
                &[]
            },
        );
        load(page, "http://news.test/news?warm");
        pump_for(Duration::from_millis(200));
        let mut samples = Vec::new();
        let mut timing = Vec::new();
        let mut blocked = 0usize;
        let mut redirected = 0usize;
        let mut observed = 0usize;
        for round in 0..20 {
            events.borrow_mut().clear();
            fixture.requests.lock().unwrap().clear();
            let start = Instant::now();
            load(page, &format!("http://news.test/news?round={round}"));
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            pump_for(Duration::from_millis(150));
            for row in events
                .borrow()
                .iter()
                .filter_map(|event| match event {
                    Event::Requests { observations, .. } => Some(observations),
                    _ => None,
                })
                .flatten()
            {
                observed += 1;
                blocked += usize::from(row.decision == "cancel");
                redirected += usize::from(row.decision == "redirect");
                if row.evaluation_time_ns > 0 {
                    timing.push(row.evaluation_time_ns);
                }
            }
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        timing.sort_unstable();
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        println!(
            "DNR news_load {}",
            json!({"ubol":enabled,"build":if cfg!(debug_assertions){"debug"}else{"release"},"loads":samples.len(),"mean_ms":mean,"p95_ms":sorted[(sorted.len()*95).div_ceil(100)-1],"samples_ms":samples,"observed_requests":observed,"blocked_requests":blocked,"redirected_requests":redirected,"module_p50_ns":timing.get(timing.len()/2),"module_p99_ns":timing.get(timing.len().saturating_sub(1)*99/100)})
        );
        if enabled {
            assert!(blocked > 0, "uBOL defaults must block news trackers");
        }
    }
    // Corpus sample rules are selected from the read-only package, never synthesized.
    for candidate in [
        "http://doubleclick.net/dnr-resource?ubol-block",
        "http://google-analytics.com/dnr-resource?ubol-block",
    ] {
        assert!(
            fetch(page, candidate)["error"].is_string(),
            "uBOL default block sample: {candidate}"
        );
        assert!(!hit(fixture, candidate));
    }
    load(
        page,
        "http://amazon.com/dnr-actual?asc_campaign=tracking&keep=yes",
    );
    assert_eq!(
        page.uri().as_deref(),
        Some("http://amazon.com/dnr-actual?keep=yes")
    );
    assert!(hit(fixture, "http://amazon.com/dnr-actual?keep=yes"));
    assert!(!hit(fixture, "asc_campaign=tracking"));
    println!("DNR ubOL block_samples=2 redirect_samples=1 proxy=localhost scripts_executed=false");
}
