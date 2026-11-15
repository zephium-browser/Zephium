use super::*;
use zephium_core::extensions::{
    ExtensionBrowserSurfaceGeneration, ExtensionBrowserTab, ExtensionBrowserWindow,
};

fn tab(id: u128, url: &str, loading: bool) -> ExtensionBrowserTab {
    ExtensionBrowserTab::from_snapshot(
        None,
        ItemId::from(id),
        true,
        "title",
        Some(&Url::parse(url).unwrap()),
        loading,
        false,
    )
    .expect("tab")
}

fn surface(
    generation: u64,
    tabs: Vec<ExtensionBrowserTab>,
    active: u128,
) -> ExtensionBrowserSurface {
    let window = ExtensionBrowserWindow::new(7, false, Some(ItemId::from(active)), tabs).unwrap();
    ExtensionBrowserSurface::new(
        ProfileId::from(1),
        ExtensionBrowserSurfaceGeneration::new(generation).unwrap(),
        Some(7),
        vec![window],
    )
    .unwrap()
}

fn shared() -> Shared {
    let shared = Shared::default();
    shared.set_profile(ProfileId::from(1));
    shared
}

#[test]
fn surface_changes_become_tab_events_with_stable_ids() {
    let shared = shared();
    assert!(shared
        .publish_surface(surface(1, vec![tab(10, "https://a.test/", false)], 10))
        .is_empty());
    let first = shared.tab_id(ItemId::from(10)).unwrap();
    let events = shared.publish_surface(surface(
        2,
        vec![
            tab(10, "https://a.test/next", true),
            tab(11, "https://b.test/", false),
        ],
        11,
    ));
    assert_eq!(shared.tab_id(ItemId::from(10)), Some(first));
    let second = shared.tab_id(ItemId::from(11)).unwrap();
    assert_ne!(first, second);
    assert!(events
        .iter()
        .any(|event| matches!(event, TabEvent::Created(tab) if tab.id == second)));
    assert!(events
        .iter()
        .any(|event| matches!(event, TabEvent::Activated { tab, .. } if *tab == second)));
    assert!(events.iter().any(|event| matches!(event,
        TabEvent::Updated { previous, current } if current.id == first && previous.tab.url() != current.tab.url())));

    let events = shared.publish_surface(surface(3, vec![tab(11, "https://b.test/", false)], 11));
    assert!(events
        .iter()
        .any(|event| matches!(event, TabEvent::Removed { tab, .. } if *tab == first)));
    assert_eq!(shared.tab_id(ItemId::from(10)), None);
    assert_eq!(shared.tab_item(i64::from(first)), None);
}

#[test]
fn a_stale_surface_cannot_roll_the_browser_back() {
    let shared = shared();
    let _ = shared.publish_surface(surface(5, vec![tab(10, "https://a.test/", false)], 10));
    let events = shared.publish_surface(surface(4, vec![tab(12, "https://c.test/", false)], 12));
    assert!(events.is_empty());
    assert!(shared.tab_id(ItemId::from(12)).is_none());
}

#[test]
fn language_results_cannot_cross_same_url_reload_or_tab_removal() {
    let shared = shared();
    shared.publish_surface(surface(1, vec![tab(10, "https://a.test/", false)], 10));
    let id = shared.tab_id(ItemId::from(10)).unwrap();
    let mut state = shared.state.borrow_mut();
    let document = DocumentGeneration::new(41).unwrap();
    state.frames.entry(ItemId::from(10)).or_default().insert(
        FrameId::MAIN,
        Frame {
            document,
            url: Url::parse("https://a.test/").unwrap(),
            parent: None,
            opaque: false,
            creator: None,
        },
    );
    let document_id = state
        .snapshot()
        .unwrap()
        .tab(id)
        .unwrap()
        .document_id
        .clone();
    assert_eq!(
        state
            .language_reply(id, document, &document_id, Ok("fr".into()))
            .unwrap(),
        json!("fr")
    );
    state
        .frames
        .get_mut(&ItemId::from(10))
        .unwrap()
        .get_mut(&FrameId::MAIN)
        .unwrap()
        .document = DocumentGeneration::new(42).unwrap();
    assert!(state
        .language_reply(id, document, &document_id, Ok("fr".into()))
        .is_err());
    drop(state);
    shared.publish_surface(surface(2, vec![tab(11, "https://a.test/", false)], 11));
    assert!(shared
        .state
        .borrow()
        .language_reply(id, document, &document_id, Ok("fr".into()))
        .is_err());
}

#[test]
fn data_replies_refuse_unavailable_profiles_and_unknown_methods() {
    assert!(data_reply("history.search", &[], &ExtensionDataReply::Unavailable).is_err());
    assert!(data_reply("history.search", &[], &ExtensionDataReply::Busy).is_err());
    assert!(data_reply("other.call", &[], &ExtensionDataReply::Closed(Vec::new())).is_err());
}

#[test]
fn the_icon_nearest_the_toolbar_size_is_chosen() {
    let icons = json!({"16": "a.png", "32": "b.png", "128": "c.png"});
    assert_eq!(best_icon_path(&icons).as_deref(), Some("b.png"));
    let icons = json!({"16": "a.png", "48": "c.png"});
    assert_eq!(best_icon_path(&icons).as_deref(), Some("c.png"));
    assert_eq!(best_icon_path(&json!("x.png")).as_deref(), Some("x.png"));
    assert_eq!(best_icon_path(&json!(3)), None);
}

#[test]
fn manifest_action_defaults_name_the_popup_by_extension_url() {
    let facts = InstallFacts {
        extension_id: "abcdefghijklmnopabcdefghijklmnop".into(),
        root: PathBuf::from("/nonexistent"),
        messages: test_messages(),
        manifest: json!({"name": "Fixture", "action": {"default_popup": "ui/popup.html"}}),
    };
    let store = action_defaults(&facts);
    assert_eq!(
        store.get(None, action::Property::Popup),
        json!("chrome-extension://abcdefghijklmnopabcdefghijklmnop/ui/popup.html")
    );
    assert_eq!(store.get(None, action::Property::Title), json!("Fixture"));
    let facts = InstallFacts {
        manifest: json!({"name": "Off", "action": {"default_popup": "../escape.html", "default_state": "disabled"}}),
        ..facts
    };
    let store = action_defaults(&facts);
    assert_eq!(store.get(None, action::Property::Popup), json!(""));
    assert_eq!(store.get(None, action::Property::Enabled), json!(false));
}

#[test]
fn package_files_outside_the_package_are_never_read() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("a.js"), "1").unwrap();
    let facts = InstallFacts {
        extension_id: "abcdefghijklmnopabcdefghijklmnop".into(),
        root: directory.path().to_path_buf(),
        messages: test_messages(),
        manifest: json!({}),
    };
    assert_eq!(
        read_package_file(&facts, "a.js").unwrap().1,
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/a.js"
    );
    assert!(read_package_file(&facts, "../a.js").is_err());
    assert!(read_package_file(&facts, "_metadata/x.js").is_err());
    assert!(read_package_file(&facts, "missing.js").is_err());
}

#[test]
fn action_icons_use_the_worker_or_calling_page_directory() {
    let facts = InstallFacts {
        extension_id: "abcdefghijklmnopabcdefghijklmnop".into(),
        root: PathBuf::from("/nonexistent"),
        messages: test_messages(),
        manifest: json!({"background":{"service_worker":"scripts/worker.js"}}),
    };
    let owner =
        Url::parse("chrome-extension://abcdefghijklmnopabcdefghijklmnop/_zephium/background.html")
            .unwrap();
    let popup = owner.join("/ui/popup.html").unwrap();
    assert_eq!(
        action_icon_path(&facts, ContextKind::Background, &owner, "./assets/icon.png").as_deref(),
        Some("scripts/assets/icon.png")
    );
    assert_eq!(
        action_icon_path(&facts, ContextKind::Popup, &popup, "./assets/icon.png").as_deref(),
        Some("ui/assets/icon.png")
    );
    assert_eq!(
        action_icon_path(&facts, ContextKind::Background, &owner, "/assets/icon.png").as_deref(),
        Some("assets/icon.png")
    );
    for path in [
        "https://example.test/icon.png",
        "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/icon.png",
        "data:image/png;base64,AA",
    ] {
        assert!(action_icon_path(&facts, ContextKind::Background, &owner, path).is_none());
    }
}

fn test_messages() -> std::sync::Arc<zephium_webext_runtime::i18n::Messages> {
    use zephium_webext_runtime::i18n::{LocaleEnvironment, Messages};
    Messages::new(
        zephium_webext_runtime::context::ExtensionId::parse("abcdefghijklmnopabcdefghijklmnop")
            .unwrap(),
        LocaleEnvironment {
            ui_language: "en".into(),
            accept_languages: vec![],
            right_to_left: false,
        },
        None,
        &std::collections::BTreeMap::new(),
    )
    .unwrap()
    .into()
}

#[test]
fn user_execution_wait_budget_covers_queued_sources_and_releases_on_drop() {
    let shared = shared();
    let request = scripting::Request::UserExecute {
        target: scripting::InjectionTarget {
            tab_id: 1,
            main_document: DocumentGeneration::new(1).unwrap(),
            frames: Vec::new(),
        },
        sources: vec![zephium_webext_runtime::user_scripts::Source {
            code: Some("1".into()),
            file: None,
        }],
        world: zephium_webext_runtime::user_scripts::World::UserScript,
        world_id: None,
        inject_immediately: false,
    };
    let mut permits = Vec::new();
    for _ in 0..64 {
        permits.push(shared.reserve_user_execution(&request).unwrap());
    }
    assert!(shared.reserve_user_execution(&request).is_err());
    permits.clear();
    assert_eq!(shared.execution_budget.get(), (0, 0));
    let mut large = request;
    if let scripting::Request::UserExecute { sources, .. } = &mut large {
        sources[0].code = Some("x".repeat(1024 * 1024));
    }
    for _ in 0..15 {
        permits.push(shared.reserve_user_execution(&large).unwrap());
    }
    assert!(shared.reserve_user_execution(&large).is_err());
    permits.clear();
    assert_eq!(shared.execution_budget.get(), (0, 0));
}

#[test]
fn frame_results_refuse_aggregate_growth_before_retention() {
    let url = Url::parse("https://a.test/").unwrap();
    let frame = BrowserFrame {
        tab: ItemId::from(10),
        frame: FrameId::MAIN,
        document: DocumentGeneration::new(1).unwrap(),
        document_id: "document".into(),
        origin: DocumentOrigin::of_url(&url),
        url,
        parent_frame_id: None,
        parent_document_id: None,
        document_lifecycle: "active".into(),
        frame_type: "outermost_frame".into(),
        error_occurred: false,
    };
    let result = Ok(ExecutionResult::Value(Value::String("\"".repeat(300_000))));
    let bytes = bounded_frame_result_bytes(&frame, &result, 1024 * 1024 - 2).unwrap();
    assert!(bytes > 600_000);
    assert_eq!(
        bounded_frame_result_bytes(&frame, &result, bytes).unwrap(),
        bytes
    );
    assert!(bounded_frame_result_bytes(&frame, &result, bytes - 1).is_err());
    assert!(bounded_frame_result_bytes(&frame, &result, 1024 * 1024 - 2 - bytes).is_err());
    assert_eq!(
        bounded_frame_result_bytes(&frame, &Err(ApiError::context_invalidated()), 0).unwrap(),
        0
    );
    let step = Step {
        world: "world".into(),
        admission_world: "world".into(),
        kind: InjectionKind::ScriptBatch,
        source: "1".into(),
        source_url: "chrome-extension://test/file.js".into(),
        args_json: "x".repeat(1024 * 1024).into(),
    };
    let other = step.clone();
    assert!(std::sync::Arc::ptr_eq(&step.args_json, &other.args_json));
}
