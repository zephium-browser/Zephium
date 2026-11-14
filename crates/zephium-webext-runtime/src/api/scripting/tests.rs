use super::*;
use crate::browser::fake::FakeBrowser;
use crate::context::tests::{background_sender, doc, profile};
use crate::context::{FrameId, Sender};
use crate::dispatch::{Dispatcher, Inbound};
use crate::envelope::{ApiName, CallId, Direction, Envelope};
use crate::pattern::DocumentOrigin;
use crate::permissions::{
    InstallPermissions, PermissionDeclaration, PermissionRequest, SiteAccess,
};
use zephium_webext::manifest::Manifest;

pub(crate) fn permissions(names: &[&str], hosts: &[&str]) -> InstallPermissions {
    let manifest = Manifest::from_value(json!({"permissions": names, "host_permissions": hosts}));
    let (declaration, _) = PermissionDeclaration::from_manifest(&manifest);
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

pub(crate) fn fixture() -> FakeBrowser {
    let mut host = FakeBrowser::new(profile());
    let window = host.add_window(false);
    let tab = host.add_tab(window, "https://example.com/", true);
    let tab = host.snapshot.tab(tab).unwrap();
    host.frames.push(BrowserFrame {
        tab: tab.tab.id(),
        frame: FrameId::MAIN,
        document: tab.document,
        document_id: tab.document_id.clone(),
        url: tab.url().unwrap(),
        origin: DocumentOrigin::of_url(&tab.url().unwrap()),
        parent_frame_id: None,
        parent_document_id: None,
        document_lifecycle: "active".into(),
        frame_type: "outermost_frame".into(),
        error_occurred: false,
    });
    let mut child = host.frames[0].clone();
    child.frame = FrameId::new(3);
    child.document = doc(3);
    child.document_id = "child-document".into();
    child.url = url::Url::parse("https://other.test/frame").unwrap();
    child.origin = DocumentOrigin::of_url(&child.url);
    child.parent_frame_id = Some(FrameId::MAIN);
    child.parent_document_id = Some(host.frames[0].document_id.clone());
    child.frame_type = "sub_frame".into();
    host.frames.push(child);
    host.resources
        .extend(["script.js".into(), "style.css".into()]);
    host
}

pub(crate) fn call(
    host: &mut FakeBrowser,
    permissions: &InstallPermissions,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    call_as(host, &background_sender(), permissions, name, args)
}

pub(crate) fn call_as(
    host: &mut FakeBrowser,
    sender: &Sender,
    permissions: &InstallPermissions,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    let mut table = ApiTable::new();
    super::register(&mut table).unwrap();
    super::super::web_navigation::register(&mut table).unwrap();
    super::super::history::register(&mut table).unwrap();
    super::super::bookmarks::register(&mut table).unwrap();
    super::super::sessions::register(&mut table).unwrap();
    super::super::top_sites::register(&mut table).unwrap();
    super::super::search::register(&mut table).unwrap();
    let mut dispatcher = Dispatcher::new(table);
    let args = serde_json::to_vec(&args).unwrap();
    let bytes = Envelope::Call {
        id: CallId::new(1).unwrap(),
        name: ApiName::parse(name).unwrap(),
        args: &args,
    }
    .encode(Direction::FromContext, u64::MAX)
    .unwrap();
    let inbound = dispatcher.receive(host, permissions, sender, &bytes, 0);
    let Inbound::Reply(reply) = inbound else {
        panic!("expected reply: {inbound:?}");
    };
    let Envelope::Reply { result, .. } =
        Envelope::decode(&reply, Direction::ToContext, u64::MAX).unwrap()
    else {
        panic!("reply");
    };
    result
        .map(|bytes| serde_json::from_slice(bytes).unwrap())
        .map_err(str::to_owned)
}

#[test]
fn script_targets_have_exact_documents_world_and_json_arguments() {
    let mut host = fixture();
    let granted = permissions(&["scripting"], &["<all_urls>"]);
    call(&mut host, &granted, "scripting.executeScript", json!([{"target":{"tabId":1,"documentIds":["child-document"]},"func":"(value) => value","args":[{"value":null}],"world":"MAIN","injectImmediately":true}])).unwrap();
    let BrowserRequest::Scripting(Request::Execute {
        target,
        source: ScriptSource::Function { source, args },
        world,
        inject_immediately,
    }) = host.requests.last().unwrap()
    else {
        panic!("script request");
    };
    assert_eq!(target.frames.len(), 1);
    assert_eq!(target.frames[0].frame, FrameId::new(3));
    assert_eq!(source, "(value) => value");
    assert_eq!(args, &[json!({"value":null})]);
    assert_eq!(*world, ExecutionWorld::Main);
    assert!(*inject_immediately);
}

#[test]
fn every_selected_frame_requires_host_access_and_active_tab_is_origin_scoped() {
    let mut host = fixture();
    let mut granted = permissions(&["scripting", "activeTab"], &[]);
    assert!(call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1},"files":["script.js"]}])
    )
    .is_err());
    let tab = host.snapshot.tabs[0].clone();
    assert!(granted.grant_active_tab(tab.tab.id(), tab.document, &tab.url().unwrap()));
    call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1},"files":["script.js"]}]),
    )
    .unwrap();
    let count = host.requests.len();
    call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1,"allFrames":true},"files":["script.js"]}]),
    )
    .unwrap();
    assert_eq!(host.requests.len(), count + 1);
    let Some(BrowserRequest::Scripting(request)) = host.requests.last() else {
        panic!("script request");
    };
    assert_eq!(request.target().frames.len(), 1);
    assert!(call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1,"frameIds":[3]},"files":["script.js"]}])
    )
    .is_err());
    host.snapshot.tabs[0].document = doc(99);
    host.frames[0].document = doc(99);
    assert!(call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1},"files":["script.js"]}])
    )
    .is_err());
}

#[test]
fn conflicting_selectors_sources_and_invalid_resources_do_not_reach_browser() {
    let mut host = fixture();
    let granted = permissions(&["scripting"], &["<all_urls>"]);
    for details in [
        json!({"target":{"tabId":1,"allFrames":true,"frameIds":[0]},"files":["script.js"]}),
        json!({"target":{"tabId":1,"frameIds":[0],"documentIds":["child-document"]},"files":["script.js"]}),
        json!({"target":{"tabId":1},"files":["../script.js"]}),
        json!({"target":{"tabId":1},"files":["missing.js"]}),
        json!({"target":{"tabId":1},"files":[]}),
        json!({"target":{"tabId":1},"files":["script.js", "./script.js"]}),
        json!({"target":{"tabId":1},"files":["script.js"],"func":"()=>1"}),
        json!({"target":{"tabId":1},"files":["script.js"],"args":[]}),
        json!({"target":{"tabId":1,"frameIds":[9]},"files":["script.js"]}),
        json!({"target":{"tabId":99},"files":["script.js"]}),
    ] {
        assert!(call(
            &mut host,
            &granted,
            "scripting.executeScript",
            json!([details])
        )
        .is_err());
    }
    assert!(host.requests.is_empty());
}

#[test]
fn css_identity_retains_source_origin_and_removal() {
    let mut host = fixture();
    let granted = permissions(&["scripting"], &["<all_urls>"]);
    for method in ["scripting.insertCSS", "scripting.removeCSS"] {
        call(&mut host, &granted, method, json!([{"target":{"tabId":1,"frameIds":[3]},"css":"body { color: red }","origin":"USER"}])).unwrap();
        let BrowserRequest::Scripting(Request::Css {
            target,
            source,
            origin,
            remove,
        }) = host.requests.last().unwrap()
        else {
            panic!("css");
        };
        assert_eq!(target.frames[0].frame, FrameId::new(3));
        assert_eq!(source, &CssSource::Code("body { color: red }".into()));
        assert_eq!(*origin, StyleOrigin::User);
        assert_eq!(*remove, method == "scripting.removeCSS");
    }
}

#[test]
fn settled_script_results_fence_generation_and_order_main_first() {
    let host = fixture();
    let request = Request::Execute {
        target: InjectionTarget {
            tab_id: 1,
            main_document: host.frames[0].document,
            frames: host.frames.clone(),
        },
        source: ScriptSource::Files(vec!["script.js".into()]),
        world: ExecutionWorld::Isolated,
        inject_immediately: false,
    };
    let results: Vec<_> = host
        .frames
        .iter()
        .rev()
        .map(|frame| FrameResult {
            frame: frame.clone(),
            result: Ok(ExecutionResult::Value(json!(frame.frame.get()))),
        })
        .collect();
    let result = reply(&request, results).unwrap();
    assert_eq!(result[0]["frameId"], 0);
    assert_eq!(result[1]["frameId"], 3);
    assert!(reply(&request, vec![]).is_err());
    let mut stale = host.frames[1].clone();
    stale.document = doc(99);
    assert!(reply(
        &request,
        vec![
            FrameResult {
                frame: host.frames[0].clone(),
                result: Ok(ExecutionResult::Value(Value::Null))
            },
            FrameResult {
                frame: stale,
                result: Ok(ExecutionResult::Value(Value::Null))
            }
        ]
    )
    .is_err());
    let partial = reply(
        &request,
        vec![
            FrameResult {
                frame: host.frames[0].clone(),
                result: Ok(ExecutionResult::Value(Value::Null)),
            },
            FrameResult {
                frame: host.frames[1].clone(),
                result: Err(ApiError::other("Error: failed")),
            },
        ],
    )
    .unwrap();
    assert_eq!(partial.as_array().unwrap().len(), 1);
}

#[test]
fn injection_results_preserve_chrome_single_and_multiple_frame_failure_shapes() {
    let host = fixture();
    let target = InjectionTarget {
        tab_id: 1,
        main_document: host.frames[0].document,
        frames: host.frames.clone(),
    };
    let script = Request::Execute {
        target: target.clone(),
        source: ScriptSource::Files(vec!["script.js".into()]),
        world: ExecutionWorld::Isolated,
        inject_immediately: false,
    };
    let failed = || {
        host.frames
            .iter()
            .map(|frame| FrameResult {
                frame: frame.clone(),
                result: Err(ApiError::other("Error: failed")),
            })
            .collect()
    };
    assert_eq!(reply(&script, failed()).unwrap(), json!([]));
    let css = Request::Css {
        target,
        source: CssSource::Code("body {}".into()),
        origin: StyleOrigin::Author,
        remove: false,
    };
    assert_eq!(reply(&css, failed()).unwrap(), Value::Null);
    let single = Request::Execute {
        target: InjectionTarget {
            tab_id: 1,
            main_document: host.frames[0].document,
            frames: vec![host.frames[0].clone()],
        },
        source: ScriptSource::Files(vec!["script.js".into()]),
        world: ExecutionWorld::Isolated,
        inject_immediately: false,
    };
    assert_eq!(
        reply(
            &single,
            vec![FrameResult {
                frame: host.frames[0].clone(),
                result: Err(ApiError::other("Error: failed"))
            }]
        )
        .unwrap_err()
        .message(),
        "Error: failed"
    );
}

#[test]
fn script_exceptions_resolve_null_without_masking_adapter_failures() {
    let host = fixture();
    for frames in [&host.frames[..1], &host.frames[..]] {
        let request = Request::Execute {
            target: InjectionTarget {
                tab_id: 1,
                main_document: host.frames[0].document,
                frames: frames.to_vec(),
            },
            source: ScriptSource::Function {
                source: "() => { throw new Error('boom'); }".into(),
                args: vec![],
            },
            world: ExecutionWorld::Isolated,
            inject_immediately: false,
        };
        let results = frames
            .iter()
            .map(|frame| FrameResult {
                frame: frame.clone(),
                result: Ok(ExecutionResult::Exception),
            })
            .collect();
        let expected: Vec<_> = frames
            .iter()
            .map(|frame| {
                json!({
                    "frameId": frame.frame.get(),
                    "documentId": frame.document_id,
                    "result": null,
                })
            })
            .collect();
        assert_eq!(reply(&request, results).unwrap(), json!(expected));
        let mut stale = frames.to_vec();
        stale[0].document = doc(99);
        assert!(reply(
            &request,
            stale
                .into_iter()
                .map(|frame| FrameResult {
                    frame,
                    result: Ok(ExecutionResult::Exception),
                })
                .collect()
        )
        .is_err());
    }
}

#[test]
fn source_budgets_and_foreign_profiles_fail_before_injection() {
    let mut host = fixture();
    let granted = permissions(&["scripting"], &["<all_urls>"]);
    assert!(call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1},"func":"x".repeat(MAX_SOURCE_BYTES+1)}])
    )
    .is_err());
    assert!(call(
        &mut host,
        &granted,
        "scripting.insertCSS",
        json!([{"target":{"tabId":1},"css":"x".repeat(MAX_SOURCE_BYTES+1)}])
    )
    .is_err());
    host.snapshot.profile = zephium_core::ids::ProfileId::from(99);
    assert!(call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1},"files":["script.js"]}])
    )
    .is_err());
    assert!(host.requests.is_empty());
}

#[test]
fn invalid_tab_and_frame_id_errors_match_chrome_scripting() {
    let mut host = fixture();
    let granted = permissions(&["scripting"], &["<all_urls>"]);
    assert_eq!(
        call(
            &mut host,
            &granted,
            "scripting.executeScript",
            json!([{"target":{"tabId":1,"frameIds":[-1]},"files":["script.js"]}])
        ),
        Err("No frame with id -1 in tab with id 1".into())
    );
    for id in [-1, 99] {
        assert_eq!(
            call(
                &mut host,
                &granted,
                "scripting.executeScript",
                json!([{"target":{"tabId":id},"files":["script.js"]}])
            ),
            Err(format!("No tab with id: {id}"))
        );
    }
    host.frames[0].document_lifecycle = "cached".into();
    assert!(call(
        &mut host,
        &granted,
        "scripting.executeScript",
        json!([{"target":{"tabId":1,"allFrames":true},"files":["script.js"]}])
    )
    .is_err());
    assert!(host.requests.is_empty());
}
