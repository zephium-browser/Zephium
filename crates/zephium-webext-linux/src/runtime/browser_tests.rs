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
