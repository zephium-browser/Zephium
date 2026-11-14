#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

mod support;

use std::{cell::RefCell, collections::BTreeSet, rc::Rc, sync::Arc, time::Duration};

use gtk::glib::{self, prelude::ObjectExt, variant::ToVariant};
use serde_json::Value;
use support::{evaluate, load, pump_for, pump_until, Fixture, ModuleDirectory};
use webkit2gtk::{
    SecurityManagerExt, URISchemeRequestExt, UserMessageExt, WebContextExt, WebViewExt,
};
use zephium_core::extensions::ExtensionRuntimeGeneration;
use zephium_core::ids::{ExtensionInstallId, ItemId, ProfileId};
use zephium_webext_linux::bridge::{
    world_name, AttachPlan, BridgeConfig, CloseReason, ExtensionPages, FrameLifecycle, Outbox,
    ProfileBridge, Refusal, Runtime, Transport, WorldGrant, BUFFER_TIMEOUT_MS,
};
use zephium_webext_runtime::context::{
    ContextKey, ContextKind, ExtensionId, ExtensionScheme, FrameId, InjectedWorld,
    InstallAdmission, Sender, ViewKind,
};
use zephium_webkit_web_process_protocol::{
    policy::{Decision, PolicySnapshot, Rule},
    protocol::{decode_event, encode_event, Event, Post, EVENT_MESSAGE},
};

const EXTENSION: &str = "abcdefghijklmnopabcdefghijklmnop";

const GLUE: &str = r#"(() => {
    const post = globalThis.__zephiumPost;
    const register = globalThis.__zephiumRegisterReceiver;
    delete globalThis.__zephiumPost;
    delete globalThis.__zephiumRegisterReceiver;
    if (typeof post !== 'function') return;
    const encode = value => new TextEncoder().encode(JSON.stringify(value));
    const receive = bytes => post(encode({phase:'ack', value:new TextDecoder().decode(bytes), href:location.href}));
    globalThis.__bridgeReceiver = receive;
    register(receive);
    globalThis.__bridgeSend = value => post(encode(value));
    const hello = () => post(encode({phase:'hello', href:location.href}));
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', hello, {once:true});
    else hello();
})()"#;

const FALLBACK_PAGE: &str = r#"<!doctype html><script>
    const queue = [btoa(JSON.stringify({phase:'hello', href:location.href}))];
    globalThis.__zephiumPullOutbox = () => queue.shift() || '';
    globalThis.__zephiumPullInbox = text => queue.push(btoa(JSON.stringify({phase:'ack', value:atob(text), href:location.href})));
</script><p>fallback background</p>"#;

#[derive(Default)]
struct Record {
    received: Vec<(Sender, Value)>,
    closed: Vec<(ContextKey, CloseReason)>,
    lifecycle: Vec<FrameLifecycle>,
}

#[derive(Clone, Default)]
struct TestRuntime(Rc<RefCell<Record>>);

impl Runtime for TestRuntime {
    fn receive(&mut self, sender: &Sender, envelope: &[u8], _: &mut Outbox) {
        let value = serde_json::from_slice(envelope).unwrap_or(Value::Null);
        let mut record = self.0.borrow_mut();
        assert!(record.received.len() < 512, "bounded capture");
        record.received.push((sender.clone(), value));
    }

    fn context_closed(&mut self, context: &ContextKey, reason: CloseReason, _: &mut Outbox) {
        self.0.borrow_mut().closed.push((context.clone(), reason));
    }

    fn frame_lifecycle(&mut self, event: &FrameLifecycle, _: &mut Outbox) {
        let mut record = self.0.borrow_mut();
        if record.lifecycle.len() < 512 {
            record.lifecycle.push(event.clone());
        }
    }
}

impl TestRuntime {
    fn hellos(&self, tab: Option<ItemId>) -> Vec<(Sender, Value)> {
        self.0
            .borrow()
            .received
            .iter()
            .filter(|(sender, value)| value["phase"] == "hello" && sender.tab() == tab)
            .cloned()
            .collect()
    }

    fn acks(&self) -> Vec<(Sender, Value)> {
        self.0
            .borrow()
            .received
            .iter()
            .filter(|(_, value)| value["phase"] == "ack")
            .cloned()
            .collect()
    }

    fn count(&self) -> usize {
        self.0.borrow().received.len()
    }
}

fn install() -> ExtensionInstallId {
    ExtensionInstallId::from(0x5a)
}

fn admission() -> InstallAdmission {
    InstallAdmission {
        install: install(),
        extension_id: ExtensionId::parse(EXTENSION).unwrap(),
        generation: ExtensionRuntimeGeneration::INITIAL,
        allowed_in_incognito: false,
        externally_connectable: None,
    }
}

fn context() -> webkit2gtk::WebContext {
    let context = webkit2gtk::WebContext::new_ephemeral();
    context.set_sandbox_enabled(true);
    assert!(context.is_sandbox_enabled());
    context
}

fn serve_extension(context: &webkit2gtk::WebContext, body: &'static str) {
    let security = context.security_manager().unwrap();
    security.register_uri_scheme_as_secure("chrome-extension");
    security.register_uri_scheme_as_cors_enabled("chrome-extension");
    context.register_uri_scheme("chrome-extension", move |request| {
        let bytes = glib::Bytes::from_static(body.as_bytes());
        let stream = gio::MemoryInputStream::from_bytes(&bytes);
        request.finish(&stream, bytes.len().try_into().unwrap(), Some("text/html"));
    });
}

fn bridge(
    context: &webkit2gtk::WebContext,
    module: &ModuleDirectory,
    profile: u128,
    runtime: TestRuntime,
) -> ProfileBridge<TestRuntime> {
    let bridge = ProfileBridge::new(
        context,
        BridgeConfig {
            profile: ProfileId::from(profile),
            incognito: false,
            scheme: ExtensionScheme::CHROME_EXTENSION,
            module_directory: module.path().to_path_buf(),
        },
        runtime,
    )
    .expect("bridge");
    bridge.admit(admission()).expect("admit");
    bridge
}

fn tab_plan(fixture: &Fixture) -> AttachPlan {
    AttachPlan {
        worlds: vec![WorldGrant {
            install: install(),
            world: InjectedWorld::Isolated,
            origins: vec![fixture.origin.clone(), fixture.cross_origin.clone()],
            bootstrap: Arc::from(GLUE),
        }],
        apply_policy: true,
        ..AttachPlan::default()
    }
}

fn own_plan() -> AttachPlan {
    AttachPlan {
        extension_pages: vec![ExtensionPages {
            extension: ExtensionId::parse(EXTENSION).unwrap(),
            sandboxed_paths: Vec::new(),
        }],
        extension_bootstrap: Arc::from(GLUE),
        ..AttachPlan::default()
    }
}

/// Records the module's raw posts on a view, as a compromised renderer
/// would see its own frame identities. Connected before the bridge.
fn observe_posts(view: &webkit2gtk::WebView) -> Rc<RefCell<Vec<Post>>> {
    let posts = Rc::new(RefCell::new(Vec::new()));
    let observed = posts.clone();
    view.connect_user_message_received(move |_, message| {
        if message.name().as_deref() == Some(EVENT_MESSAGE) {
            if let Some(Ok(Event::Post(post))) = message
                .parameters()
                .and_then(|parameters| parameters.get::<Vec<u8>>())
                .map(|bytes| decode_event(&bytes))
            {
                let mut posts = observed.borrow_mut();
                if posts.len() < 256 {
                    posts.push(post);
                }
            }
        }
        false
    });
    posts
}

fn ack_for(runtime: &TestRuntime, key: &ContextKey, value: &str) -> usize {
    runtime
        .acks()
        .iter()
        .filter(|(sender, ack)| sender.context_key() == *key && ack["value"] == value)
        .count()
}

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_bridge() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();
    let context = context();
    serve_extension(&context, "<!doctype html><p>extension background</p>");
    let runtime = TestRuntime::default();
    let bridge = bridge(&context, &module, 1, runtime.clone());
    let tab = ItemId::from(5);
    let page = webkit2gtk::WebView::with_context(&context);
    let raw = observe_posts(&page);
    let view = bridge
        .attach_tab(tab, &page, &tab_plan(&fixture))
        .expect("attach");

    // Posts from an isolated world in top, same-origin and cross-origin frames.
    load(&page, &format!("{}/frames", fixture.origin));
    pump_until(|| runtime.hellos(Some(tab)).len() == 3);
    support::assert_sandbox(&module);
    assert_eq!(bridge.transport(view), Some(Transport::Module));
    let hellos = runtime.hellos(Some(tab));
    let frames: BTreeSet<FrameId> = hellos.iter().filter_map(|(s, _)| s.frame()).collect();
    assert_eq!(frames.len(), 3);
    assert!(frames.contains(&FrameId::MAIN));
    for (sender, value) in &hellos {
        assert_eq!(sender.kind(), ContextKind::ContentScript);
        assert_eq!(sender.install(), install());
        assert_eq!(sender.url().as_str(), value["href"].as_str().unwrap());
    }
    assert!(hellos
        .iter()
        .any(|(sender, _)| sender.url().as_str().starts_with(&fixture.cross_origin)));
    println!("WEBEXT bridge_frames senders=3 distinct_frames=3 cross_origin=true");

    // Replies reach exactly the bound frame and world.
    for (index, (sender, _)) in hellos.iter().enumerate() {
        let key = sender.context_key();
        let value = format!("target-{index}");
        let before = runtime.acks().len();
        bridge
            .deliver(&key, value.clone().into_bytes())
            .expect("deliver");
        pump_until(|| ack_for(&runtime, &key, &value) == 1);
        pump_for(Duration::from_millis(50));
        assert_eq!(
            runtime.acks().len(),
            before + 1,
            "delivered to one context only"
        );
    }
    assert_eq!(
        evaluate(&page, "return typeof globalThis.__bridgeReceiver;", None),
        "undefined",
        "the page world never sees the isolated bridge"
    );
    println!("WEBEXT bridge_directed_delivery contexts=3");

    // A second tab's renderer forging the first tab's claims binds nothing.
    let other_tab = ItemId::from(6);
    let other = webkit2gtk::WebView::with_context(&context);
    let other_raw = observe_posts(&other);
    bridge
        .attach_tab(other_tab, &other, &tab_plan(&fixture))
        .expect("attach");
    load(&other, &format!("{}/frame?other", fixture.cross_origin));
    pump_until(|| runtime.hellos(Some(other_tab)).len() == 1);
    let victim = raw
        .borrow()
        .iter()
        .find(|post| post.uri == format!("{}/frames", fixture.origin))
        .cloned()
        .expect("victim main post");
    let own = other_raw.borrow()[0].clone();
    let before = runtime.count();
    let refused_before: u64 = bridge.stats().refused.values().sum();
    let forgeries = [
        Post {
            generation: own.generation,
            ..victim.clone()
        },
        Post {
            generation: victim.generation,
            ..victim.clone()
        },
        Post {
            uri: victim.uri.clone(),
            ..own.clone()
        },
        Post {
            world: world_name(ExtensionInstallId::from(0x99), &InjectedWorld::Isolated),
            ..own.clone()
        },
        Post {
            document_seq: own.document_seq + 1000,
            ..own.clone()
        },
    ];
    for forged in &forgeries {
        let bytes = encode_event(&Event::Post(forged.clone())).unwrap();
        let message = webkit2gtk::UserMessage::new(EVENT_MESSAGE, Some(&bytes.to_variant()));
        let handled = other.emit_by_name::<bool>("user-message-received", &[&message]);
        assert!(handled);
    }
    pump_for(Duration::from_millis(BUFFER_TIMEOUT_MS + 600));
    assert_eq!(runtime.count(), before, "no forged claim produced a sender");
    let stats = bridge.stats();
    assert_eq!(
        stats.refused.values().sum::<u64>() - refused_before,
        forgeries.len() as u64
    );
    assert!(
        stats
            .refused
            .get(&Refusal::UnknownFrame)
            .copied()
            .unwrap_or(0)
            >= 1
    );
    println!(
        "WEBEXT bridge_forged_claims refused={} stats={:?}",
        forgeries.len(),
        stats.refused
    );

    // A navigation ends the old document's contexts.
    let main = hellos
        .iter()
        .find(|(sender, _)| sender.frame() == Some(FrameId::MAIN))
        .unwrap()
        .0
        .context_key();
    load(&page, &format!("{}/frame?next", fixture.origin));
    pump_until(|| {
        runtime
            .0
            .borrow()
            .closed
            .iter()
            .any(|(key, reason)| *key == main && *reason == CloseReason::DocumentGone)
    });
    assert!(bridge.deliver(&main, b"late".to_vec()).is_err());
    pump_until(|| runtime.hellos(Some(tab)).len() == 4);
    assert!(runtime.0.borrow().lifecycle.iter().any(|event| matches!(
        event,
        FrameLifecycle::MainDocumentChanged { tab: changed, .. } if *changed == tab
    )));
    println!("WEBEXT bridge_navigation old_context_closed=true");

    // A request-policy snapshot reaches the page and its cancel rule applies.
    let snapshot = PolicySnapshot::new(vec![Rule {
        id: 1,
        url: format!("{}/blocked", fixture.origin),
        method: None,
        resource_type: None,
        decision: Decision::Cancel,
    }])
    .unwrap()
    .encode()
    .unwrap();
    bridge.publish_policy(&snapshot).expect("publish");
    pump_for(Duration::from_millis(100));
    let fetch = "return await fetch(path).then(r => r.text(), e => e.name);";
    let blocked = evaluate_with(&page, fetch, "/blocked");
    let allowed = evaluate_with(&page, fetch, "/allowed");
    assert_eq!(
        (blocked.as_str(), allowed.as_str()),
        ("\"TypeError\"", "\"fixture-body\"")
    );
    assert!(fixture
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.path != "/blocked"));
    assert_eq!(bridge.stats().policy_refused, 0);
    println!("WEBEXT bridge_policy cancel=true control=true");

    // A page-initiated cross-site navigation keeps working.
    evaluate(
        &page,
        &format!(
            "location.href = '{}/frame?cross-site'; return true;",
            fixture.cross_origin
        ),
        None,
    );
    pump_until(|| {
        runtime
            .hellos(Some(tab))
            .iter()
            .any(|(sender, _)| sender.url().as_str().ends_with("/frame?cross-site"))
    });
    println!("WEBEXT bridge_cross_site page_id={}", page.page_id());

    // A crashed web process is recovered.
    let before_crash = runtime.hellos(Some(tab)).len();
    let crashed = runtime.hellos(Some(tab)).last().unwrap().0.context_key();
    page.terminate_web_process();
    pump_until(|| {
        runtime
            .0
            .borrow()
            .closed
            .iter()
            .any(|(key, reason)| *key == crashed && *reason == CloseReason::ProcessGone)
    });
    load(&page, &format!("{}/frame?after-crash", fixture.origin));
    pump_until(|| runtime.hellos(Some(tab)).len() > before_crash);
    let (recovered, _) = runtime.hellos(Some(tab)).last().cloned().unwrap();
    bridge
        .deliver(&recovered.context_key(), b"recovered".to_vec())
        .expect("deliver");
    pump_until(|| ack_for(&runtime, &recovered.context_key(), "recovered") == 1);
    assert_eq!(bridge.transport(view), Some(Transport::Module));
    println!("WEBEXT bridge_crash recovered=true");

    // An extension-owned view binds its own page world.
    let background = webkit2gtk::WebView::with_context(&context);
    let owned = bridge
        .attach_extension_view(
            install(),
            ViewKind::Background,
            &background,
            &own_plan(),
            true,
        )
        .expect("owned");
    load(
        &background,
        &format!("chrome-extension://{EXTENSION}/bg.html"),
    );
    pump_until(|| {
        runtime
            .hellos(None)
            .iter()
            .any(|(sender, _)| sender.kind() == ContextKind::Background)
    });
    assert_eq!(bridge.transport(owned), Some(Transport::Module));
    let (sender, _) = runtime.hellos(None).pop().unwrap();
    bridge
        .deliver(&sender.context_key(), b"to-background".to_vec())
        .expect("deliver");
    pump_until(|| ack_for(&runtime, &sender.context_key(), "to-background") == 1);
    println!("WEBEXT bridge_extension_owned kind=Background roundtrip=true");

    fallback(&fixture);

    // Teardown: detach, drop the views, drain until every renderer exited.
    page.stop_loading();
    let mut drain = bridge.shutdown();
    assert!(runtime
        .0
        .borrow()
        .closed
        .iter()
        .any(|(_, reason)| *reason == CloseReason::Detached));
    drop(page);
    drop(other);
    drop(background);
    drop(bridge);
    assert!(drain.run(Duration::from_secs(10)), "bridge drain settled");
    assert_eq!(drain.remaining_views(), 0);
    println!(
        "WEBEXT bridge_teardown drained=true pending={}",
        drain.pending_messages()
    );
    drop(context);
}

fn fallback(fixture: &Fixture) {
    let empty = ModuleDirectory::empty();
    let context = context();
    serve_extension(&context, FALLBACK_PAGE);
    let runtime = TestRuntime::default();
    let bridge = bridge(&context, &empty, 2, runtime.clone());
    let background = webkit2gtk::WebView::with_context(&context);
    let owned = bridge
        .attach_extension_view(
            install(),
            ViewKind::Background,
            &background,
            &own_plan(),
            true,
        )
        .expect("owned");
    let tab = webkit2gtk::WebView::with_context(&context);
    let tab_view = bridge
        .attach_tab(ItemId::from(9), &tab, &tab_plan(fixture))
        .expect("tab");
    load(
        &background,
        &format!("chrome-extension://{EXTENSION}/_zephium/background.html"),
    );
    pump_until(|| bridge.transport(owned) == Some(Transport::Fallback));
    pump_until(|| runtime.hellos(None).len() == 1);
    let (sender, _) = runtime.hellos(None).pop().unwrap();
    assert_eq!(sender.kind(), ContextKind::Background);
    bridge
        .deliver(&sender.context_key(), b"pulled".to_vec())
        .expect("deliver");
    bridge.ring(owned);
    pump_until(|| ack_for(&runtime, &sender.context_key(), "pulled") == 1);
    load(&tab, &format!("{}/frame?no-module", fixture.origin));
    assert_eq!(
        bridge.transport(tab_view),
        Some(Transport::Refused),
        "tabs never fall back"
    );
    assert!(runtime.hellos(Some(ItemId::from(9))).is_empty());
    println!("WEBEXT bridge_fallback pull_roundtrip=true tab_refused=true");
    let mut drain = bridge.shutdown();
    drop(background);
    drop(tab);
    drop(bridge);
    assert!(drain.run(Duration::from_secs(10)), "fallback drain settled");
    drop(context);
}

fn evaluate_with(page: &webkit2gtk::WebView, body: &str, path: &str) -> String {
    let result = Rc::new(RefCell::new(None));
    let observed = result.clone();
    let arguments = glib::VariantDict::new(None);
    arguments.insert_value("path", &path.to_variant());
    page.call_async_javascript_function(
        body,
        Some(&arguments.end()),
        None,
        None,
        None::<&gio::Cancellable>,
        move |value| {
            *observed.borrow_mut() = Some(value.map(|value| {
                use javascriptcore::ValueExt;
                value
                    .to_json(0)
                    .map(|json| json.to_string())
                    .unwrap_or_default()
            }));
        },
    );
    pump_until(|| result.borrow().is_some());
    let value = result.borrow_mut().take().unwrap();
    value.expect("native JavaScript evaluation")
}
