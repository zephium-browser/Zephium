use super::*;
use std::time::Instant;
use zephium_webkit_web_process::protocol::{
    InjectRequest, InjectionKind, InjectionStatus, InjectionTarget, MatchRule, ScriptUpdate,
};
use zephium_webkit_web_process_protocol::{
    matching::{ContentScriptMatcher, MatchOptions},
    scripts::{RunAt, ScriptEntry, ScriptFile, ScriptTable},
};

const INSTALL: &str = "00000000000000000000000000000001";
const ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn rule(all_frames: bool) -> MatchRule {
    MatchRule {
        matcher: ContentScriptMatcher::new(
            &["<all_urls>"],
            &[],
            &[],
            &[],
            MatchOptions {
                match_about_blank: true,
                match_origin_as_fallback: true,
            },
        )
        .unwrap(),
        all_frames,
        file_access: false,
    }
}

fn file(name: &str, source: &str) -> ScriptFile {
    ScriptFile {
        source: source.into(),
        url: format!("chrome-extension://{ID}/{name}.js"),
    }
}

fn entry(id: &str, run_at: RunAt, world: &str, source: &str) -> ScriptEntry {
    ScriptEntry {
        install_id: INSTALL.into(),
        script_id: id.into(),
        world: world.into(),
        run_at,
        rule: rule(true),
        files: vec![file(id, source)],
        css: vec![],
        bootstrap: String::new(),
    }
}

fn publish(page: &webkit2gtk::WebView, table: &ScriptTable, sealed: bool) -> bool {
    use rustix::fs::{fcntl_add_seals, memfd_create, MemfdFlags, SealFlags};
    let bytes = table.encode().unwrap();
    let mut file = File::from(
        memfd_create(
            "zephium-native-scripts",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )
        .unwrap(),
    );
    file.write_all(&bytes).unwrap();
    if sealed {
        fcntl_add_seals(
            &file,
            SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
        )
        .unwrap();
    }
    let fds = gio::UnixFDList::new();
    let fd_index = fds.append(file).unwrap().try_into().unwrap();
    send(
        page,
        &UiMessage::Scripts(ScriptUpdate {
            token: TOKEN.into(),
            generation: 1,
            table_generation: table.generation,
            fd_index,
            length: bytes.len().try_into().unwrap(),
        }),
        Some(&fds),
    )
}

fn inject(
    page: &webkit2gtk::WebView,
    events: &Rc<RefCell<Vec<Event>>>,
    request: InjectRequest,
) -> Vec<zephium_webkit_web_process::protocol::InjectionResult> {
    let id = request.request_id;
    assert!(send(page, &UiMessage::Inject(request), None));
    pump_until(|| {
        events
            .borrow()
            .iter()
            .any(|event| matches!(event,Event::Injection(result) if result.request_id == id))
    });
    pump_for(Duration::from_millis(30));
    events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            Event::Injection(result) if result.request_id == id => Some(result.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn run(module: &ModuleDirectory, fixture: &Fixture) {
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        let event = decode_event(&message.parameters().unwrap().get::<Vec<u8>>().unwrap()).unwrap();
        assert!(observed.borrow().len() < 2048);
        observed.borrow_mut().push(event);
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{}/frame?content-inert", fixture.origin));
    let bootstrap = "globalThis.bootCount=(globalThis.bootCount||0)+1;globalThis.nativeSend=__zephiumPost;delete globalThis.__zephiumPost;delete globalThis.__zephiumRegisterReceiver;globalThis.order=['bootstrap'];";
    let attachment = Attach {
        token: TOKEN.into(),
        generation: 1,
        worlds: vec![WorldRule {
            install_id: INSTALL.into(),
            name: WORLD.into(),
            matches: vec![rule(true)],
            bootstrap: String::new(),
        }],
        ..Attach::default()
    };
    assert!(send(&page, &UiMessage::Attach(attachment), None));
    let mut top_only = entry(
        "top-only",
        RunAt::DocumentStart,
        WORLD,
        "globalThis.topOnly=true;",
    );
    top_only.rule = rule(false);
    let mut excluded = entry(
        "excluded",
        RunAt::DocumentStart,
        WORLD,
        "globalThis.excludedRan=true;",
    );
    excluded.rule.matcher = ContentScriptMatcher::new(
        &["<all_urls>"],
        &["*://*/excluded*"],
        &[],
        &[],
        MatchOptions::default(),
    )
    .unwrap();
    let mut table=ScriptTable {generation:1,entries:vec![
        entry("first",RunAt::DocumentStart,WORLD,"\"use strict\"; let sharedLexical=40; const sharedConstant=2; order.push('start'); globalThis.strictThis=(function(){return this;})()===undefined; globalThis.bootBefore=bootCount===1;"),
        entry("second",RunAt::DocumentStart,WORLD,"\"use strict\"; globalThis.sharedAnswer=sharedLexical+sharedConstant;"),
        entry("main",RunAt::DocumentStart,"","globalThis.mainOverride='native';"),
        top_only,excluded,
        entry("end",RunAt::DocumentEnd,WORLD,"order.push('end');document.documentElement.dataset.nativeEnd='yes';globalThis.endState=document.readyState;"),
        entry("idle",RunAt::DocumentIdle,WORLD,"order.push('idle');globalThis.idleState=document.readyState;nativeSend(new TextEncoder().encode(JSON.stringify({phase:'content',href:location.href,order,sharedAnswer,strictThis,bootBefore,topOnly:globalThis.topOnly===true,excludedRan:globalThis.excludedRan===true,pageSaw:document.documentElement.dataset.main,apiHidden:document.documentElement.dataset.hidden,earlyEnd:document.documentElement.dataset.earlyEnd})));"),
    ]};
    for script in &mut table.entries {
        if !script.world.is_empty() {
            script.bootstrap = bootstrap.into();
        }
    }
    assert!(!publish(&page, &table, false));
    assert!(publish(&page, &table, true));
    load(&page, &format!("{}/content-frames", fixture.origin));
    pump_until(|| {
        posts(&events)
            .iter()
            .filter(|p| payload(p)["phase"] == "content")
            .count()
            >= 6
    });
    assert_eq!(
        evaluate(
            &page,
            "return document.getElementById('blank').contentWindow.blankInlineSawMain;",
            None
        ),
        "true"
    );
    let reports: Vec<_> = posts(&events)
        .into_iter()
        .filter(|p| payload(p)["phase"] == "content")
        .collect();
    for post in &reports {
        let value = payload(post);
        assert_eq!(
            value["order"],
            json!(["bootstrap", "start", "end", "idle"]),
            "{value}"
        );
        assert_eq!(value["sharedAnswer"], 42, "{value}");
        assert_eq!(value["strictThis"], true, "{value}");
        assert_eq!(value["bootBefore"], true, "{value}");
        assert_eq!(
            value["topOnly"],
            post.uri.ends_with("/content-frames"),
            "{value}"
        );
        if post.uri != "about:blank" {
            assert_eq!(value["pageSaw"], "native", "{value}");
            assert_eq!(value["apiHidden"], "true", "{value}");
            assert_eq!(
                value["earlyEnd"], "false",
                "synthetic events must not run lifecycle: {value}"
            );
        }
        if post.uri.contains("/excluded") {
            assert_eq!(value["excludedRan"], false);
        }
    }
    assert!(
        reports
            .iter()
            .any(|p| p.uri.starts_with(&fixture.cross_origin)),
        "all_urls reaches previously unlisted origin"
    );
    assert!(reports.iter().any(|p| p.uri == "about:blank"));
    assert!(reports.iter().any(|p| p.uri == "about:srcdoc"));
    let target = reports
        .iter()
        .find(|p| p.uri.starts_with(&fixture.cross_origin))
        .unwrap();
    let request=InjectRequest {install_id:INSTALL.into(),token:TOKEN.into(),generation:1,request_id:100,
        target:InjectionTarget::Frame{frame_id:target.frame_id,document_seq:target.document_seq},
        world:WORLD.into(),kind:InjectionKind::Function,
        source:"async (a,b)=>{await Promise.resolve();return {answer:a+b,lexical:sharedLexical,href:location.href};}".into(),
        source_url:format!("chrome-extension://{ID}/execute.js"),args_json:"[20,22]".into(),timeout_ms:1000};
    let results = inject(&page, &events, request.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, InjectionStatus::Success);
    let value: Value = serde_json::from_str(&results[0].result_json).unwrap();
    assert_eq!(value["answer"], 42);
    assert_eq!(value["lexical"], 40);
    assert_eq!(value["href"], target.uri);
    let mut stale = request.clone();
    stale.request_id = 101;
    stale.target = InjectionTarget::Frame {
        frame_id: target.frame_id,
        document_seq: target.document_seq + 1,
    };
    assert_ne!(
        inject(&page, &events, stale)[0].status,
        InjectionStatus::Success
    );
    let mut timeout = request.clone();
    timeout.request_id = 102;
    timeout.source = "()=>new Promise(()=>{})".into();
    timeout.timeout_ms = 30;
    assert_eq!(
        inject(&page, &events, timeout)[0].status,
        InjectionStatus::TimedOut
    );
    for (request_id, kind) in [
        (104, InjectionKind::InsertCss),
        (105, InjectionKind::RemoveCss),
    ] {
        let mut css = request.clone();
        css.request_id = request_id;
        css.kind = kind;
        css.source = "body { color: red; }".into();
        assert_eq!(
            inject(&page, &events, css)[0].status,
            InjectionStatus::Unsupported
        );
    }
    for (request_id, source, status) in [
        (
            106,
            "()=>{let x=null;for(let i=0;i<256;i++)x=[x];return x;}",
            InjectionStatus::EvaluationFailed,
        ),
        (107, "()=> 'x'.repeat(1048577)", InjectionStatus::Oversized),
        (
            108,
            "()=>{throw new Error('fixture failure');}",
            InjectionStatus::EvaluationFailed,
        ),
    ] {
        let mut refused = request.clone();
        refused.request_id = request_id;
        refused.source = source.into();
        assert_eq!(inject(&page, &events, refused)[0].status, status);
    }
    let mut all = request;
    all.request_id = 103;
    all.target = InjectionTarget::AllFrames;
    all.source = "()=>42".into();
    let results = inject(&page, &events, all);
    assert!(results.len() >= 6);
    assert!(results
        .iter()
        .all(|r| r.status == InjectionStatus::Success && r.result_json == "42"));
    let mut later = table.clone();
    later.generation = 2;
    later.entries.push(entry(
        "later",
        RunAt::DocumentStart,
        WORLD,
        "globalThis.laterRegistered=true;",
    ));
    later.entries.last_mut().unwrap().bootstrap = bootstrap.into();
    assert!(publish(&page, &later, true));
    assert_eq!(
        evaluate(&page, "return typeof laterRegistered;", Some(WORLD)),
        "undefined"
    );
    load(&page, &format!("{}/content-frame?later", fixture.origin));
    assert_eq!(
        evaluate(&page, "return laterRegistered;", Some(WORLD)),
        "true"
    );
    binding(&page, &events, fixture);
    page.stop_loading();
    println!("WEBEXT native_content lexical=true strict=true frame_order=true main_before_inline=true bootstrap=true execute_promise_subframe=true stale=true timeout=true later_documents=true");
}

pub(super) fn timing(module: &ModuleDirectory, fixture: &Fixture) {
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        captured.borrow_mut().push((
            Instant::now(),
            decode_event(&message.parameters().unwrap().get::<Vec<u8>>().unwrap()).unwrap(),
        ));
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{}/frame?timing-inert", fixture.origin));
    assert!(send(
        &page,
        &UiMessage::Attach(Attach {
            token: TOKEN.into(),
            generation: 1,
            worlds: vec![WorldRule {
                install_id: INSTALL.into(),
                name: WORLD.into(),
                matches: vec![rule(true)],
                bootstrap: "__zephiumPost(new Uint8Array([7]));".into()
            }],
            ..Attach::default()
        }),
        None
    ));
    for sample in 0..8 {
        events.borrow_mut().clear();
        let start = Instant::now();
        load(&page, &format!("{}/frame?timing={sample}", fixture.origin));
        pump_until(|| {
            let events = events.borrow();
            events.iter().any(|(_,e)|matches!(e,Event::Post(_))) && events.iter().any(|(_,e)|matches!(e,Event::Frames{events,..} if events.iter().any(|e|matches!(e,FrameEvent::Created(_)|FrameEvent::Committed(_)))))
        });
        let events = events.borrow();
        let post = events
            .iter()
            .find_map(|(t, e)| matches!(e, Event::Post(_)).then_some(*t))
            .unwrap();
        let frame=events.iter().find_map(|(t,e)|matches!(e,Event::Frames{events,..} if events.iter().any(|e|matches!(e,FrameEvent::Created(_)|FrameEvent::Committed(_)))).then_some(*t)).unwrap();
        println!(
            "TIMING v={} sample={} post_ms={:.3} frame_ms={:.3} host_wait_ms={:.3}",
            PROTOCOL_VERSION,
            sample,
            post.duration_since(start).as_secs_f64() * 1000.,
            frame.duration_since(start).as_secs_f64() * 1000.,
            frame.saturating_duration_since(post).as_secs_f64() * 1000.
        );
        assert!(frame <= post, "frame facts must precede first post");
        drop(events);
        pump_for(Duration::from_millis(31));
    }
    page.stop_loading();
}

fn binding(page: &webkit2gtk::WebView, events: &Rc<RefCell<Vec<Event>>>, fixture: &Fixture) {
    let surface = include_str!("../../../zephium-webext-runtime/js/test/surface.fixture.json");
    let bootstrap = format!(
        "({})(\"native-content\",{surface},{});",
        zephium_webext_runtime::js::BUNDLE,
        json!({"extensionId":ID,"scheme":"chrome-extension","contextKind":"contentScript","manifest":{"name":"Native binding fixture","version":"1"},"permissions":[]})
    );
    assert!(send(
        page,
        &UiMessage::Attach(Attach {
            token: TOKEN.into(),
            generation: 2,
            worlds: vec![WorldRule {
                install_id: INSTALL.into(),
                name: WORLD.into(),
                matches: vec![rule(true)],
                bootstrap
            }],
            ..Attach::default()
        }),
        None
    ));
    load(page, &format!("{}/frame?real-binding", fixture.origin));
    assert_eq!(
        evaluate(
            page,
            "return typeof chrome.runtime.sendMessage;",
            Some(WORLD)
        ),
        "function"
    );
    assert_eq!(
        evaluate(
            page,
            "return chrome.runtime.getManifest().name;",
            Some(WORLD)
        ),
        "Native binding fixture"
    );
    assert_eq!(evaluate(page, "return typeof chrome;", None), "undefined");
    assert_eq!(
        evaluate(page, "return typeof __zephiumPost;", Some(WORLD)),
        "undefined"
    );
    pump_until(|| {
        events
            .borrow()
            .iter()
            .any(|e| matches!(e, Event::Attached { generation: 2 }))
    });
}

pub(super) fn external(module: &ModuleDirectory, fixture: &Fixture) {
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        captured
            .borrow_mut()
            .push(decode_event(&message.parameters().unwrap().get::<Vec<u8>>().unwrap()).unwrap());
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{}/frame?external-inert", fixture.origin));
    let matcher = ContentScriptMatcher::new(
        &[format!("{}/external-allowed*", fixture.origin)],
        &[],
        &[],
        &[],
        MatchOptions::default(),
    )
    .unwrap();
    let attachment=Attach {token:TOKEN.into(),generation:1,external:vec![zephium_webkit_web_process::protocol::ExternalRule {
        install_id:INSTALL.into(), matches:vec![MatchRule{matcher,all_frames:true,file_access:false}],
        bootstrap:"globalThis.externalSend=__zephiumPost;delete globalThis.__zephiumPost;delete globalThis.__zephiumRegisterReceiver;externalSend(new Uint8Array([8]));".into(),
    }],..Attach::default()};
    assert!(send(&page, &UiMessage::Attach(attachment), None));
    load(&page, &format!("{}/external-denied", fixture.origin));
    assert_eq!(
        evaluate(&page, "return typeof externalSend;", None),
        "undefined"
    );
    load(&page, &format!("{}/external-allowed", fixture.origin));
    pump_until(|| {
        posts(&events)
            .iter()
            .any(|p| p.external_install.as_deref() == Some(INSTALL))
    });
    assert_eq!(
        evaluate(&page, "return typeof externalSend;", None),
        "function"
    );
    assert!(events.borrow().iter().any(|e|matches!(e,Event::Frames{events,..} if events.iter().any(|e|matches!(e,FrameEvent::ExternalPage{install_id,..} if install_id==INSTALL)))));
    load(&page, &format!("{}/external-denied-again", fixture.origin));
    assert_eq!(
        evaluate(&page, "return typeof externalSend;", None),
        "undefined"
    );
    page.stop_loading();
    println!("WEBEXT native_external exact_path=true external_claim=true navigation_revokes=true");
}

pub(super) fn view_grants(module: &ModuleDirectory, fixture: &Fixture) {
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        captured
            .borrow_mut()
            .push(decode_event(&message.parameters().unwrap().get::<Vec<u8>>().unwrap()).unwrap());
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{}/frame?grant-inert", fixture.origin));
    let matcher = ContentScriptMatcher::new(
        &[format!("{}/allowed*", fixture.origin)],
        &[],
        &[],
        &[],
        MatchOptions::default(),
    )
    .unwrap();
    let mut attachment=Attach {token:TOKEN.into(),generation:1,worlds:vec![WorldRule {
        install_id:INSTALL.into(),name:WORLD.into(),matches:vec![MatchRule{matcher,all_frames:true,file_access:false}],
        bootstrap:"globalThis.livePost=__zephiumPost;globalThis.bootCount=(globalThis.bootCount||0)+1;".into(),
    }],..Attach::default()};
    assert!(send(&page, &UiMessage::Attach(attachment.clone()), None));
    let table = ScriptTable {
        generation: 1,
        entries: vec![
            entry(
                "main-grant",
                RunAt::DocumentStart,
                "",
                "globalThis.tableRan=true;",
            ),
            entry(
                "isolated-grant",
                RunAt::DocumentStart,
                WORLD,
                "globalThis.tableRan=true;",
            ),
        ],
    };
    assert!(publish(&page, &table, true));
    load(&page, &format!("{}/denied", fixture.origin));
    assert_eq!(
        evaluate(&page, "return typeof tableRan;", None),
        "undefined"
    );
    assert_eq!(
        evaluate(&page, "return typeof tableRan;", Some(WORLD)),
        "undefined"
    );
    load(&page, &format!("{}/allowed", fixture.origin));
    assert_eq!(evaluate(&page, "return tableRan;", None), "true");
    assert_eq!(evaluate(&page, "return tableRan;", Some(WORLD)), "true");
    assert_eq!(
        evaluate(&page, "return livePost(new Uint8Array([3]));", Some(WORLD)),
        "true"
    );
    pump_until(|| posts(&events).iter().any(|p| p.bytes == [3]));
    let original = posts(&events).into_iter().find(|p| p.bytes == [3]).unwrap();
    attachment.worlds[0].matches.push(rule(true));
    let added_world = "zephium-webext-added-world";
    attachment.worlds.push(WorldRule {
        install_id: INSTALL.into(),
        name: added_world.into(),
        matches: vec![rule(true)],
        bootstrap: "globalThis.newWorldReady=true;".into(),
    });
    assert!(send(&page, &UiMessage::Attach(attachment), None));
    assert_eq!(evaluate(&page, "return bootCount;", Some(WORLD)), "1");
    assert_eq!(
        evaluate(&page, "return livePost(new Uint8Array([4]));", Some(WORLD)),
        "true"
    );
    pump_until(|| posts(&events).iter().any(|p| p.bytes == [4]));
    let fresh = posts(&events).into_iter().find(|p| p.bytes == [4]).unwrap();
    assert_eq!(
        (fresh.generation, fresh.frame_id, fresh.document_seq),
        (
            original.generation,
            original.frame_id,
            original.document_seq
        )
    );
    let result = inject(
        &page,
        &events,
        InjectRequest {
            token: TOKEN.into(),
            generation: 1,
            install_id: INSTALL.into(),
            request_id: 300,
            target: InjectionTarget::Frame {
                frame_id: original.frame_id,
                document_seq: original.document_seq,
            },
            world: added_world.into(),
            kind: InjectionKind::Function,
            source: "()=>globalThis.newWorldReady===true".into(),
            source_url: format!("chrome-extension://{ID}/new-world.js"),
            args_json: "[]".into(),
            timeout_ms: 1000,
        },
    );
    assert_eq!(result[0].status, InjectionStatus::Success);
    assert_eq!(result[0].result_json, "true");
    load(&page, &format!("{}/denied?now-widened", fixture.origin));
    assert_eq!(evaluate(&page, "return tableRan;", None), "true");
    page.stop_loading();
    println!("WEBEXT native_view_grants main_denied=true isolated_denied=true widened_context_preserved=true");
}

pub(super) fn frame_facts(module: &ModuleDirectory, fixture: &Fixture) {
    let context = module.context(&(PROTOCOL_VERSION, TOKEN).to_variant());
    let page = webkit2gtk::WebView::with_context(&context);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    page.connect_user_message_received(move |_, message| {
        if message.name().as_deref() != Some(EVENT_MESSAGE) {
            return false;
        }
        captured
            .borrow_mut()
            .push(decode_event(&message.parameters().unwrap().get::<Vec<u8>>().unwrap()).unwrap());
        message.send_reply(&webkit2gtk::UserMessage::new("observed", None));
        true
    });
    load(&page, &format!("{}/frame?facts-inert", fixture.origin));
    assert!(send(
        &page,
        &UiMessage::Attach(Attach {
            token: TOKEN.into(),
            generation: 1,
            worlds: vec![WorldRule {
                install_id: INSTALL.into(),
                name: WORLD.into(),
                matches: vec![rule(true)],
                bootstrap: "globalThis.factApi=true;".into(),
            }],
            ..Attach::default()
        }),
        None
    ));
    load(&page, &format!("{}/facts", fixture.origin));
    let facts = || {
        events
            .borrow()
            .iter()
            .flat_map(|event| match event {
                Event::Frames { events, .. } => events
                    .iter()
                    .filter_map(|event| match event {
                        FrameEvent::Created(info) | FrameEvent::Committed(info) => {
                            Some(info.clone())
                        }
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>()
    };
    pump_until(|| facts().iter().any(|f| f.uri.starts_with("data:")));
    let facts = facts();
    let main = facts
        .iter()
        .find(|f| f.is_main && f.uri.ends_with("/facts"))
        .unwrap();
    for suffix in [
        "/frame?ordinary",
        "/frame?cross-facts",
        "/frame?sandbox",
        "/csp-sandbox",
    ] {
        let frame = facts.iter().find(|f| f.uri.ends_with(suffix)).unwrap();
        assert_eq!(frame.parent_frame_id, Some(main.frame_id), "{frame:?}");
        assert_eq!(
            frame.opaque,
            suffix == "/frame?sandbox" || suffix == "/csp-sandbox",
            "{frame:?}"
        );
    }
    assert!(
        facts
            .iter()
            .find(|f| f.uri.starts_with("data:"))
            .unwrap()
            .opaque
    );
    page.stop_loading();
    println!("WEBEXT native_frame_facts parent=true cross_parent=true sandbox_attribute=true sandbox_csp=true data_opaque=true");
}
