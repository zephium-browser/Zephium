#![cfg(all(target_os = "linux", feature = "native-tests"))]
#![forbid(unsafe_code)]

mod support;

use gio::prelude::UnixFDListExtManual;
use gtk::glib::{self, prelude::ObjectExt, variant::ToVariant};
use serde_json::{json, Value};
use std::{cell::RefCell, fs::File, io::Write, rc::Rc, time::Duration};
use support::{evaluate, load, pump_for, pump_until, send_raw, Fixture, ModuleDirectory};
use webkit2gtk::{
    SecurityManagerExt, URISchemeRequestExt, UserMessageExt, WebContextExt, WebViewExt,
};
use zephium_webkit_web_process::{
    policy::{Decision, Header, PolicySnapshot, Rule},
    protocol::{
        decode_event, encode_ui, Attach, Deliver, Detach, Event, FrameEvent, Limits, PageRole,
        PolicyUpdate, Post, UiMessage, WorldRule, ATTACH_MESSAGE, EVENT_MESSAGE, PROTOCOL_VERSION,
    },
};

const TOKEN: &str = "isolated-native-test-process-token";
const WORLD: &str = "zephium-webext-native-test";
const GENERATION: u64 = 1;
const BOOTSTRAP: &str = r#"(() => {
    const post = globalThis.__zephiumPost;
    const register = globalThis.__zephiumRegisterReceiver;
    const encode = value => new TextEncoder().encode(JSON.stringify(value));
    globalThis.nativeTestSend = post;
    globalThis.nativeTestReceive = bytes => post(encode({phase:'ack', value:new TextDecoder().decode(bytes)}));
    globalThis.nativeTestRegistered = register(globalThis.nativeTestReceive);
    delete globalThis.__zephiumPost;
    delete globalThis.__zephiumRegisterReceiver;
    const hello = () => post(encode({phase:'hello', top:window === top, registered:globalThis.nativeTestRegistered}));
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', hello, {once:true});
    else hello();
})()"#;

#[test]
#[ignore = "sandboxed native WebKitGTK; use timeout -k 10 420 scripts/ci/linux-test.sh"]
fn native_web_process_module() {
    let _session = support::init();
    let module = ModuleDirectory::new();
    let fixture = Fixture::new();

    assert_invalid_initialization(&module, &fixture);
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let held_replies = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let held = held_replies.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        let bytes = message.parameters().unwrap().get::<Vec<u8>>().unwrap();
        let event = decode_event(&bytes).expect("bounded module event");
        let hold = matches!(&event, Event::Post(post) if post.bytes == [240]);
        let mut events = captured.borrow_mut();
        assert!(events.len() < 512, "bounded native event capture");
        events.push(event);
        if hold {
            held.borrow_mut().push(message.clone());
        } else {
            message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        }
        true
    });
    load(&page, &format!("{}/frame?inert", fixture.origin));
    assert_inert(&page, &events);

    let attach = attach(&fixture);
    let malformed = vec![255u8; 16].to_variant();
    assert!(!accepted(send_raw(
        &page,
        ATTACH_MESSAGE,
        Some(&malformed),
        None
    )));
    let wrong_type = "not-a-byte-array".to_variant();
    assert!(!accepted(send_raw(
        &page,
        ATTACH_MESSAGE,
        Some(&wrong_type),
        None
    )));
    let mut wrong_version = attach.clone();
    wrong_version.protocol_version = PROTOCOL_VERSION + 1;
    assert!(!send(&page, &UiMessage::Attach(wrong_version), None));
    let mut wrong_token = attach.clone();
    wrong_token.token = "other-process-token".into();
    assert!(!send(&page, &UiMessage::Attach(wrong_token), None));
    assert_inert(&page, &events);

    assert!(send(&page, &UiMessage::Attach(attach), None));
    load(&page, &format!("{}/frames?attached", fixture.origin));
    pump_until(|| hello_posts(&events).len() == 3);
    let targets = hello_posts(&events);
    assert_eq!(
        targets
            .iter()
            .map(|post| post.frame_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    assert!(targets
        .iter()
        .any(|post| post.uri.starts_with(&fixture.cross_origin)));
    assert!(targets
        .iter()
        .all(|post| post.world == WORLD && post.document_seq > 0));
    assert!(targets
        .iter()
        .all(|post| payload(post)["registered"] == true));
    assert_eq!(
        targets
            .iter()
            .filter(|post| payload(post)["top"] == true)
            .count(),
        1
    );
    pump_until(|| {
        events.borrow().iter().any(|event| {
            matches!(
                event,
                Event::Attached {
                    generation: GENERATION
                }
            )
        })
    });
    pump_until(|| {
        events.borrow().iter().any(|event| matches!(event, Event::Frames { events, .. } if events.iter().any(|event| matches!(event, FrameEvent::Created(_) | FrameEvent::Committed(_)))))
    });
    support::assert_sandbox(&module);

    assert_hostile_posts(&page, &events);
    assert_directed_delivery(&page, &events, &targets);
    assert_stale_delivery(&page, &fixture, &events, &targets);
    assert_request_policy(&page, &fixture, &events);
    assert_top_documents(&page, &fixture, &targets);
    assert_form(&page, &fixture, &events, false);

    let before = posts(&events).len();
    assert!(send(
        &page,
        &UiMessage::Detach(Detach {
            token: TOKEN.into(),
            generation: GENERATION
        }),
        None
    ));
    assert_eq!(
        evaluate(
            &page,
            "return nativeTestSend(new Uint8Array([9]));",
            Some(WORLD)
        ),
        "false"
    );
    pump_for(Duration::from_millis(100));
    assert_eq!(posts(&events).len(), before);
    assert_repeated_lifecycle(&page, &fixture, &events);
    assert_inflight_lifecycle(&page, &fixture, &held_replies);
    assert_extension_owned_default_world(&module, &fixture);
    page.stop_loading();
    println!("WEBEXT native_module inert=true hostile_posts=true frame_roundtrip=true stale_drop=true request_policy=true");
}

fn assert_extension_owned_default_world(module: &ModuleDirectory, fixture: &Fixture) {
    const ORIGIN: &str = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let security = context.security_manager().unwrap();
    security.register_uri_scheme_as_secure("chrome-extension");
    security.register_uri_scheme_as_cors_enabled("chrome-extension");
    context.register_uri_scheme("chrome-extension", |request| {
        let bytes = glib::Bytes::from_static(
            br#"<!doctype html><script>globalThis.nativeInlineSawBridge=typeof nativeTestSend==='function';</script><p>extension-owned native fixture</p>"#,
        );
        let stream = gio::MemoryInputStream::from_bytes(&bytes);
        request.finish(&stream, bytes.len().try_into().unwrap(), Some("text/html"));
    });
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        let bytes = message.parameters().unwrap().get::<Vec<u8>>().unwrap();
        let event = decode_event(&bytes).unwrap();
        let mut events = observed.borrow_mut();
        assert!(events.len() < 64, "bounded extension-owned event capture");
        events.push(event);
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{ORIGIN}/inert.html"));
    assert_inert(&page, &events);
    assert_eq!(
        evaluate(&page, "return nativeInlineSawBridge;", None),
        "false"
    );
    let mut attachment = Attach {
        token: TOKEN.into(),
        generation: 1,
        role: PageRole::ExtensionOwned,
        limits: Limits {
            max_message_bytes: 4096,
            ..Limits::default()
        },
        extension_origins: vec![ORIGIN.into()],
        extension_bootstrap: BOOTSTRAP.into(),
        ..Attach::default()
    };
    assert!(send(&page, &UiMessage::Attach(attachment.clone()), None));
    load(&page, &format!("{ORIGIN}/active.html"));
    pump_until(|| hello_posts(&events).len() == 1);
    let target = hello_posts(&events).remove(0);
    assert_eq!(target.uri, format!("{ORIGIN}/active.html"));
    assert!(
        target.world.is_empty(),
        "extension-owned bridge belongs in the default world"
    );
    assert_eq!(
        payload(&target),
        json!({"phase":"hello", "top":true, "registered":true})
    );
    assert_eq!(
        evaluate(&page, "return nativeInlineSawBridge;", None),
        "true"
    );
    assert_directed_delivery(&page, &events, std::slice::from_ref(&target));

    let before = posts(&events).len();
    let malformed = "invalid-attach-parameters".to_variant();
    assert!(!accepted(send_raw(
        &page,
        ATTACH_MESSAGE,
        Some(&malformed),
        None
    )));
    assert_eq!(
        evaluate(&page, "return nativeTestSend(new Uint8Array([9]));", None),
        "false"
    );
    assert!(!send(
        &page,
        &UiMessage::Deliver(delivery(&target, vec![1])),
        None
    ));
    pump_for(Duration::from_millis(100));
    assert_eq!(
        posts(&events).len(),
        before,
        "malformed attach left a captured bridge active"
    );

    attachment.generation = 2;
    assert!(send(&page, &UiMessage::Attach(attachment), None));
    load(&page, &format!("{ORIGIN}/rearmed.html"));
    pump_until(|| posts(&events).iter().any(|post| post.generation == 2));
    assert_eq!(
        evaluate(&page, "return nativeInlineSawBridge;", None),
        "true"
    );
    let latest = posts(&events)
        .into_iter()
        .find(|post| post.generation == 2)
        .unwrap();
    let before = posts(&events).len();
    load(
        &page,
        &format!("{}/frame?left-extension-origin", fixture.origin),
    );
    assert_eq!(
        evaluate(&page, "return typeof nativeTestSend;", None),
        "undefined"
    );
    assert_eq!(
        evaluate(&page, "return typeof __zephiumPost;", None),
        "undefined"
    );
    let mut stale = delivery(&latest, vec![1]);
    stale.generation = 2;
    assert!(!send(&page, &UiMessage::Deliver(stale), None));
    pump_for(Duration::from_millis(100));
    assert_eq!(
        posts(&events).len(),
        before,
        "HTTP document received an extension-owned bridge"
    );
    page.stop_loading();
    println!("WEBEXT native_extension_owned default_world=true document_start=true roundtrip=true http_bridge_absent=true malformed_attach_inert=true");
}

fn attach(fixture: &Fixture) -> Attach {
    Attach {
        token: TOKEN.into(),
        generation: GENERATION,
        limits: Limits {
            max_message_bytes: 4096,
            ..Limits::default()
        },
        worlds: vec![WorldRule {
            name: WORLD.into(),
            origins: vec![fixture.origin.clone(), fixture.cross_origin.clone()],
            bootstrap: BOOTSTRAP.into(),
        }],
        observe_requests: true,
        ..Attach::default()
    }
}

fn accepted(reply: Result<webkit2gtk::UserMessage, glib::Error>) -> bool {
    reply
        .ok()
        .and_then(|reply| reply.parameters())
        .and_then(|parameters| parameters.get::<bool>())
        .unwrap_or(false)
}

fn send(
    page: &webkit2gtk::WebView,
    message: &UiMessage,
    descriptors: Option<&gio::UnixFDList>,
) -> bool {
    let parameters = encode_ui(message).unwrap().to_variant();
    accepted(send_raw(
        page,
        message.name(),
        Some(&parameters),
        descriptors,
    ))
}

fn posts(events: &Rc<RefCell<Vec<Event>>>) -> Vec<Post> {
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Post(post) => Some(post.clone()),
            _ => None,
        })
        .collect()
}

fn payload(post: &Post) -> Value {
    serde_json::from_slice(&post.bytes).unwrap_or(Value::Null)
}

fn hello_posts(events: &Rc<RefCell<Vec<Event>>>) -> Vec<Post> {
    posts(events)
        .into_iter()
        .filter(|post| payload(post)["phase"] == "hello")
        .collect()
}

fn assert_inert(page: &webkit2gtk::WebView, events: &Rc<RefCell<Vec<Event>>>) {
    assert_eq!(
        evaluate(page, "return typeof globalThis.__zephiumPost;", None),
        "undefined"
    );
    assert_eq!(
        evaluate(page, "return typeof globalThis.__zephiumPost;", Some(WORLD)),
        "undefined"
    );
    assert_eq!(
        evaluate(
            page,
            "return typeof globalThis.nativeTestSend;",
            Some(WORLD)
        ),
        "undefined"
    );
    pump_for(Duration::from_millis(100));
    assert!(events.borrow().is_empty(), "unattached page emitted events");
}

fn assert_invalid_initialization(module: &ModuleDirectory, fixture: &Fixture) {
    for initialization in [
        (PROTOCOL_VERSION + 1, TOKEN).to_variant(),
        "malformed-init".to_variant(),
    ] {
        let context = module.context(&initialization);
        let page = webkit2gtk::WebView::with_context(&context);
        load(&page, &format!("{}/frame?bad-init", fixture.origin));
        assert!(!send(&page, &UiMessage::Attach(attach(fixture)), None));
        assert_eq!(
            evaluate(
                &page,
                "return typeof globalThis.__zephiumPost;",
                Some(WORLD)
            ),
            "undefined"
        );
        page.stop_loading();
    }
}

fn assert_hostile_posts(page: &webkit2gtk::WebView, events: &Rc<RefCell<Vec<Event>>>) {
    let before = posts(events).len();
    let result: Value = serde_json::from_str(&evaluate(page, r#"
        let coerced = false;
        const hostile = {get length(){coerced=true;throw Error('length')}, toString(){coerced=true;throw Error('string')}};
        const results = [nativeTestSend(new Uint8Array(4097)), nativeTestSend('x'.repeat(1048576)),
            nativeTestSend(hostile), nativeTestSend(new Float64Array(1)),
            nativeTestSend(new Proxy(new Uint8Array(1), {get(){coerced=true;throw Error('proxy')}}))];
        const boundary = nativeTestSend(new Uint8Array(4096).fill(65));
        return JSON.stringify({results, coerced, boundary,
            globalsDeleted:typeof __zephiumPost === 'undefined' && typeof __zephiumRegisterReceiver === 'undefined'});
    "#, Some(WORLD))).unwrap();
    assert_eq!(
        result["results"],
        json!([false, false, false, false, false])
    );
    assert_eq!(result["coerced"], false);
    assert_eq!(result["boundary"], true);
    assert_eq!(result["globalsDeleted"], true);
    pump_until(|| posts(events).len() > before);
    pump_for(Duration::from_millis(100));
    let forwarded = posts(events);
    assert_eq!(forwarded.len(), before + 1, "refused values reached IPC");
    assert_eq!(forwarded.last().unwrap().bytes, vec![65; 4096]);
    assert_eq!(
        evaluate(page, "return typeof nativeTestSend;", None),
        "undefined"
    );
}

fn delivery(post: &Post, bytes: Vec<u8>) -> Deliver {
    Deliver {
        token: TOKEN.into(),
        generation: GENERATION,
        frame_id: post.frame_id,
        document_seq: post.document_seq,
        world: post.world.clone(),
        bytes,
    }
}

fn assert_directed_delivery(
    page: &webkit2gtk::WebView,
    events: &Rc<RefCell<Vec<Event>>>,
    targets: &[Post],
) {
    for target in targets {
        let before = posts(events).len();
        let value = format!("target-{}", target.frame_id);
        assert!(send(
            page,
            &UiMessage::Deliver(delivery(target, value.as_bytes().to_vec())),
            None
        ));
        pump_until(|| posts(events).len() > before);
        pump_for(Duration::from_millis(30));
        let observed = posts(events);
        assert_eq!(observed.len(), before + 1);
        let reply = &observed[before];
        assert_eq!(reply.frame_id, target.frame_id);
        assert_eq!(payload(reply), json!({"phase":"ack", "value":value}));
    }
    let before = posts(events).len();
    let target = &targets[0];
    let mut invalid = vec![delivery(target, vec![0; 4097])];
    let mut unknown = delivery(target, vec![1]);
    unknown.frame_id = u64::MAX;
    invalid.push(unknown);
    let mut stale = delivery(target, vec![1]);
    stale.document_seq += 1;
    invalid.push(stale);
    let mut world = delivery(target, vec![1]);
    world.world = "unattached-world".into();
    invalid.push(world);
    let mut generation = delivery(target, vec![1]);
    generation.generation += 1;
    invalid.push(generation);
    for invalid in invalid {
        assert!(!send(page, &UiMessage::Deliver(invalid), None));
    }
    pump_for(Duration::from_millis(100));
    assert_eq!(posts(events).len(), before);
}

fn assert_stale_delivery(
    page: &webkit2gtk::WebView,
    fixture: &Fixture,
    events: &Rc<RefCell<Vec<Event>>>,
    targets: &[Post],
) {
    let old = targets
        .iter()
        .find(|post| post.uri == format!("{}/frame?same", fixture.origin))
        .unwrap();
    evaluate(
        page,
        "document.getElementById('same').src='/frame?navigated'; return true;",
        None,
    );
    pump_until(|| {
        hello_posts(events)
            .iter()
            .any(|post| post.uri.ends_with("/frame?navigated"))
    });
    let fresh = hello_posts(events)
        .into_iter()
        .find(|post| post.uri.ends_with("/frame?navigated"))
        .unwrap();
    assert_eq!(fresh.frame_id, old.frame_id);
    assert_ne!(fresh.document_seq, old.document_seq);
    assert!(!send(
        page,
        &UiMessage::Deliver(delivery(old, vec![1])),
        None
    ));
    assert!(send(
        page,
        &UiMessage::Deliver(delivery(&fresh, b"new-document".to_vec())),
        None
    ));
    evaluate(
        page,
        "document.getElementById('same').remove(); return true;",
        None,
    );
    pump_for(Duration::from_millis(100));
    assert!(!send(
        page,
        &UiMessage::Deliver(delivery(&fresh, vec![1])),
        None
    ));
}

fn assert_request_policy(
    page: &webkit2gtk::WebView,
    fixture: &Fixture,
    events: &Rc<RefCell<Vec<Event>>>,
) {
    let mut rules = Vec::new();
    for source in ["page", "classic", "module"] {
        let kinds: &[&str] = if source == "page" {
            &["fetch", "document", "main-document", "script", "image"]
        } else {
            &["fetch"]
        };
        for kind in kinds {
            for (action, decision) in [
                ("cancel", Decision::Cancel),
                (
                    "redirect",
                    Decision::Redirect(format!(
                        "{}/policy-actual?source={source}&kind={kind}",
                        fixture.origin
                    )),
                ),
                (
                    "headers",
                    Decision::SetRequestHeaders(vec![Header {
                        name: "X-Native-Test".into(),
                        value: "rewritten-in-web-process".into(),
                    }]),
                ),
            ] {
                rules.push(Rule {
                    id: u32::try_from(rules.len() + 1).unwrap(),
                    url: format!(
                        "{}/policy-{action}?source={source}&kind={kind}",
                        fixture.origin
                    ),
                    method: None,
                    resource_type: None,
                    decision,
                });
            }
        }
    }
    let snapshot = PolicySnapshot::new(rules).unwrap().encode().unwrap();
    assert!(!install_policy(page, &snapshot, false));
    assert!(install_policy(page, &snapshot, true));
    let script = format!(
        r#"{}
        return JSON.stringify({{page:await nativeRequests(), classic:await nativeWorker('classic'),
            module:await nativeWorker('module'), subresources:await nativeSubresources()}});"#,
        include_str!("requests.js")
    );
    let result: Value = serde_json::from_str(&evaluate(page, &script, None)).unwrap();
    println!("WEBEXT native_request_controls={result}");
    for source in ["page", "classic", "module"] {
        assert_eq!(
            result[source]["cancel"]["error"], "TypeError",
            "{source} fetch cancellation"
        );
        assert_eq!(
            result[source]["redirect"]["body"], "redirected-resource",
            "{source} redirect"
        );
        assert_eq!(
            result[source]["headers"]["body"], "rewritten-in-web-process",
            "{source} headers"
        );
    }
    for resource in result["subresources"].as_array().unwrap() {
        assert_ne!(resource["loaded"], "timeout", "{resource}");
        if resource["action"] != "cancel" {
            assert_eq!(resource["loaded"], true, "{resource}");
            if resource["kind"] != "image" {
                let expected = if resource["action"] == "redirect" {
                    "redirected-resource"
                } else {
                    "rewritten-in-web-process"
                };
                assert_eq!(resource["body"], expected, "{resource}");
            }
        } else if resource["kind"] != "document" {
            assert_eq!(resource["loaded"], false, "{resource}");
        }
    }
    let requests = fixture.requests.lock().unwrap();
    assert!(requests
        .iter()
        .all(|request| !request.path.starts_with("/policy-cancel")));
    for source in ["page", "classic", "module"] {
        assert!(requests.iter().any(|request| request.path
            == format!("/policy-headers?source={source}&kind=fetch")
            && request.header == "rewritten-in-web-process"));
    }
    for kind in ["document", "script", "image"] {
        assert!(requests.iter().any(|request| request.path
            == format!("/policy-headers?source=page&kind={kind}")
            && request.header == "rewritten-in-web-process"));
    }
    drop(requests);
    pump_for(Duration::from_millis(100));
    pump_until(|| {
        events.borrow().iter().any(|event| matches!(event, Event::Requests { observations, .. } if observations.iter().any(|observation| observation.matched)))
    });
}

fn assert_top_documents(page: &webkit2gtk::WebView, fixture: &Fixture, targets: &[Post]) {
    load(
        page,
        &format!(
            "{}/policy-redirect?source=page&kind=main-document",
            fixture.origin
        ),
    );
    assert_eq!(
        evaluate(page, "return document.body.textContent.trim();", None),
        "redirected-resource"
    );
    assert_eq!(
        page.uri().unwrap().as_str(),
        format!(
            "{}/policy-actual?source=page&kind=main-document",
            fixture.origin
        )
    );
    for target in targets {
        assert!(
            !send(page, &UiMessage::Deliver(delivery(target, vec![1])), None),
            "retired document accepted delivery"
        );
    }
    load(
        page,
        &format!(
            "{}/policy-headers?source=page&kind=main-document",
            fixture.origin
        ),
    );
    assert_eq!(
        evaluate(page, "return document.body.textContent.trim();", None),
        "rewritten-in-web-process"
    );
    let failed = Rc::new(std::cell::Cell::new(false));
    let observed = failed.clone();
    let signal = page.connect_load_failed(move |_, _, _, _| {
        observed.set(true);
        false
    });
    load(
        page,
        &format!(
            "{}/policy-cancel?source=page&kind=main-document",
            fixture.origin
        ),
    );
    page.disconnect(signal);
    println!("WEBEXT main_cancel load_failed={}", failed.get());
    assert!(!page.is_loading(), "canceled main navigation must settle");
    load(page, &format!("{}/frame?restored", fixture.origin));
    let requests = fixture.requests.lock().unwrap();
    assert!(requests
        .iter()
        .all(|request| !request.path.starts_with("/policy-cancel")));
    assert!(requests.iter().any(|request| request.path
        == "/policy-headers?source=page&kind=main-document"
        && request.header == "rewritten-in-web-process"));
}

fn assert_form(
    page: &webkit2gtk::WebView,
    fixture: &Fixture,
    events: &Rc<RefCell<Vec<Event>>>,
    expected: bool,
) {
    let before = events
        .borrow()
        .iter()
        .filter(|event| matches!(event, Event::Form(_)))
        .count();
    let marker = if expected { "enabled" } else { "disabled" };
    let script = format!(
        r#"
        const target = document.createElement('iframe'); target.name = 'native-form-target';
        document.body.append(target);
        const form = document.createElement('form'); form.action = '/form-submit';
        form.target = target.name;
        const input = document.createElement('input'); input.name = 'probe'; input.value = '{marker}';
        form.append(input); document.body.append(form); form.submit(); return true;
    "#
    );
    evaluate(page, &script, None);
    pump_until(|| {
        fixture
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.path == format!("/form-submit?probe={marker}"))
    });
    if expected {
        pump_until(|| {
            events
                .borrow()
                .iter()
                .filter(|event| matches!(event, Event::Form(_)))
                .count()
                > before
        });
    } else {
        pump_for(Duration::from_millis(600));
    }
    let forms: Vec<_> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Form(form) => Some(form.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        forms.len(),
        before + usize::from(expected),
        "form observation flag"
    );
    if expected {
        assert!(forms
            .last()
            .unwrap()
            .fields
            .contains(&("probe".into(), marker.into())));
    }
}

fn assert_repeated_lifecycle(
    page: &webkit2gtk::WebView,
    fixture: &Fixture,
    events: &Rc<RefCell<Vec<Event>>>,
) {
    let mut second = attach(fixture);
    second.generation = 2;
    second.observe_forms = true;
    assert!(send(page, &UiMessage::Attach(second), None));
    load(page, &format!("{}/frame?second-attach", fixture.origin));
    pump_until(|| posts(events).iter().any(|post| post.generation == 2));
    assert_form(page, fixture, events, true);
    assert!(
        !send(page, &UiMessage::Attach(attach(fixture)), None),
        "stale attach must refuse"
    );
    assert_eq!(
        evaluate(
            page,
            "return nativeTestSend(new Uint8Array([9]));",
            Some(WORLD)
        ),
        "false"
    );
    let mut third = attach(fixture);
    third.generation = 3;
    assert!(send(page, &UiMessage::Attach(third), None));
    load(page, &format!("{}/frame?third-attach", fixture.origin));
    pump_until(|| posts(events).iter().any(|post| post.generation == 3));
    let current = posts(events)
        .into_iter()
        .find(|post| post.generation == 3)
        .unwrap();
    let mut target = delivery(&current, b"reattached".to_vec());
    target.generation = 3;
    assert!(send(page, &UiMessage::Deliver(target), None));
    assert!(send(
        page,
        &UiMessage::Detach(Detach {
            token: TOKEN.into(),
            generation: 3
        }),
        None
    ));
    let before = posts(events).len();
    assert_eq!(
        evaluate(
            page,
            "return nativeTestSend(new Uint8Array([9]));",
            Some(WORLD)
        ),
        "false"
    );
    pump_for(Duration::from_millis(100));
    assert_eq!(posts(events).len(), before);
}

fn assert_inflight_lifecycle(
    page: &webkit2gtk::WebView,
    fixture: &Fixture,
    held: &Rc<RefCell<Vec<webkit2gtk::UserMessage>>>,
) {
    let post = || {
        evaluate(
            page,
            "return nativeTestSend(new Uint8Array([240]));",
            Some(WORLD),
        )
    };
    for generation in [4, 5] {
        let mut message = attach(fixture);
        message.generation = generation;
        message.limits.frame_outstanding = 1;
        assert!(send(page, &UiMessage::Attach(message), None));
        load(
            page,
            &format!("{}/frame?inflight-{generation}", fixture.origin),
        );
        pump_for(Duration::from_millis(50));
        assert_eq!(post(), "true");
        pump_until(|| held.borrow().len() == usize::try_from(generation - 3).unwrap());
        assert_eq!(
            post(),
            "false",
            "held reply must consume the frame reservation"
        );
        if generation == 4 {
            assert!(send(
                page,
                &UiMessage::Detach(Detach {
                    token: TOKEN.into(),
                    generation
                }),
                None
            ));
        }
    }
    held.borrow_mut()
        .remove(0)
        .send_reply(&webkit2gtk::UserMessage::new(
            "old-generation-settled",
            None,
        ));
    pump_for(Duration::from_millis(50));
    assert_eq!(
        post(),
        "false",
        "old completion released a new-generation reservation"
    );
    held.borrow_mut()
        .pop()
        .unwrap()
        .send_reply(&webkit2gtk::UserMessage::new(
            "current-generation-settled",
            None,
        ));
    pump_for(Duration::from_millis(50));
    assert_eq!(post(), "true");
    pump_until(|| held.borrow().len() == 1);
    held.borrow_mut()
        .pop()
        .unwrap()
        .send_reply(&webkit2gtk::UserMessage::new("observed", None));
    assert!(send(
        page,
        &UiMessage::Detach(Detach {
            token: TOKEN.into(),
            generation: 5
        }),
        None
    ));
}

fn install_policy(page: &webkit2gtk::WebView, bytes: &[u8], sealed: bool) -> bool {
    use rustix::fs::{fcntl_add_seals, memfd_create, MemfdFlags, SealFlags};
    let descriptor = memfd_create(
        "zephium-native-policy",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .unwrap();
    let mut file = File::from(descriptor);
    file.write_all(bytes).unwrap();
    if sealed {
        fcntl_add_seals(
            &file,
            SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
        )
        .unwrap();
    }
    let descriptors = gio::UnixFDList::new();
    let index = descriptors.append(file).unwrap();
    send(
        page,
        &UiMessage::Policy(PolicyUpdate {
            token: TOKEN.into(),
            generation: GENERATION,
            policy_generation: 1,
            fd_index: index.try_into().unwrap(),
            length: bytes.len().try_into().unwrap(),
        }),
        Some(&descriptors),
    )
}
