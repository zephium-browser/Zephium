use super::*;
use crate::browser::fake::FakeBrowser;
use crate::context::tests::{background_sender, profile};
use crate::context::DocumentGeneration;
use crate::dispatch::Dispatcher;
use crate::permissions::{PermissionDeclaration, PermissionRequest, SiteAccess};
use proptest::prelude::*;
use zephium_webext::manifest::Manifest;

fn permissions() -> InstallPermissions {
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"permissions":["activeTab"],"action":{}}),
    ));
    InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    )
}

fn setup() -> (
    FakeBrowser,
    Dispatcher<FakeBrowser>,
    Sender,
    InstallPermissions,
    i32,
) {
    let sender = background_sender();
    let mut host = FakeBrowser::new(profile());
    host.info.manifest = json!({"manifest_version":3,"name":"Example","action":{"default_title":"Default","default_popup":"popup.html"}});
    host.action = ActionStore::new(&host.info.manifest, &sender).unwrap();
    let window = host.add_window(false);
    let tab = host.add_tab(window, "https://example.com/", true);
    let mut table = ApiTable::new();
    register(&mut table).unwrap();
    (host, Dispatcher::new(table), sender, permissions(), tab)
}

#[test]
fn absent_action_manifest_refuses_dispatch_even_when_adapter_info_contains_action() {
    let (mut host, mut dispatcher, sender, _, _) = setup();
    let (declaration, _) = PermissionDeclaration::from_manifest(&Manifest::from_value(
        json!({"permissions":["activeTab"]}),
    ));
    let permissions = InstallPermissions::new(
        declaration,
        &PermissionRequest::default(),
        SiteAccess::OnAllSites,
        false,
    );
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getTitle",
            json!([{}])
        )
        .unwrap_err(),
        ApiError::missing_permission().message()
    );
    assert!(host.requests.is_empty());
}

#[test]
fn native_defaults_preserve_unset_color_sentinels_and_manifest_fallbacks() {
    let sender = background_sender();
    let mut store = ActionStore::new(
        &json!({"name":"Manifest name","icons":{"32":"product.png"},
        "action":{"default_title":"","default_state":"disabled"}}),
        &sender,
    )
    .unwrap();
    assert_eq!(store.get(None, Property::Title), "Manifest name");
    assert_eq!(store.get(None, Property::Enabled), false);
    assert_eq!(
        store.get(None, Property::BadgeBackgroundColor),
        json!([0, 0, 0, 0])
    );
    assert_eq!(
        store.get(None, Property::BadgeTextColor),
        json!([0, 0, 0, 0])
    );
    assert_eq!(
        store.state(None).icon,
        Some(Icon::Paths(BTreeMap::from([(
            32,
            "product.png".to_owned()
        )])))
    );
    store
        .apply(None, &Update::BadgeBackgroundColor([255, 255, 255, 255]))
        .unwrap();
    assert_eq!(
        store.get(None, Property::BadgeTextColor),
        json!([0, 0, 0, 0])
    );
    store
        .apply(None, &Update::BadgeTextColor([255, 0, 0, 255]))
        .unwrap();
    assert_eq!(
        store.get(None, Property::BadgeTextColor),
        json!([255, 0, 0, 255])
    );
}

#[test]
fn effective_overrides_inherit_dynamic_defaults_and_navigation_resets_all_fields() {
    let (mut host, mut dispatcher, sender, permissions, tab) = setup();
    let methods = [
        ("action.setTitle", json!({"title":"local","tabId":tab})),
        ("action.setBadgeText", json!({"text":"1","tabId":tab})),
        (
            "action.setBadgeBackgroundColor",
            json!({"color":"#1234","tabId":tab}),
        ),
        (
            "action.setBadgeTextColor",
            json!({"color":"rgb(100% 0% 0% / 50%)","tabId":tab}),
        ),
        ("action.setPopup", json!({"popup":"","tabId":tab})),
    ];
    for (method, details) in methods {
        assert!(
            host.call(
                &mut dispatcher,
                &sender,
                &permissions,
                method,
                json!([details])
            )
            .is_ok(),
            "{method}"
        );
    }
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getTitle",
            json!([{"tabId":tab}])
        )
        .unwrap(),
        "local"
    );
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getBadgeBackgroundColor",
            json!([{"tabId":tab}])
        )
        .unwrap(),
        json!([17, 34, 51, 68])
    );
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getBadgeTextColor",
            json!([{"tabId":tab}])
        )
        .unwrap(),
        json!([255, 0, 0, 128])
    );
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getPopup",
            json!([{"tabId":tab}])
        )
        .unwrap(),
        ""
    );
    host.call(
        &mut dispatcher,
        &sender,
        &permissions,
        "action.setBadgeText",
        json!([{"text":"global"}]),
    )
    .unwrap();
    host.call(
        &mut dispatcher,
        &sender,
        &permissions,
        "action.setBadgeText",
        json!([{"tabId":tab}]),
    )
    .unwrap();
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getBadgeText",
            json!([{"tabId":tab}])
        )
        .unwrap(),
        "global"
    );
    host.call(
        &mut dispatcher,
        &sender,
        &permissions,
        "action.disable",
        json!([tab]),
    )
    .unwrap();
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.isEnabled",
            json!([tab])
        )
        .unwrap(),
        false
    );
    host.action.reset_tab(tab).unwrap();
    assert_eq!(host.action.get(Some(tab), Property::Title), "Default");
    assert_eq!(host.action.get(Some(tab), Property::Enabled), true);
    assert_eq!(host.action.get(Some(tab), Property::BadgeText), "global");
    assert!(host
        .action
        .get(Some(tab), Property::Popup)
        .as_str()
        .unwrap()
        .ends_with("/popup.html"));
}

#[test]
fn unavailable_tabs_icons_and_colors_fail_before_adapter_mutation() {
    let (mut host, mut dispatcher, sender, permissions, _) = setup();
    for (method, args) in [
        ("action.setTitle", json!([{"tabId":999,"title":"x"}])),
        (
            "action.setIcon",
            json!([{"imageData":{"width":257,"height":1,"data":[]}}]),
        ),
        (
            "action.setIcon",
            json!([{"imageData":{"width":1,"height":1,"data":[1,2,3,256]}}]),
        ),
        ("action.setIcon", json!([{"path":{"32":12}}])),
        ("action.setIcon", json!([{"path":"missing.png"}])),
        ("action.setIcon", json!([{"path":"ok.png","imageData":{}}])),
        ("action.setBadgeTextColor", json!([{"color":"transparent"}])),
        (
            "action.setBadgeBackgroundColor",
            json!([{"color":"url(secret)"}]),
        ),
        ("action.setBadgeText", json!([{"text":"a".repeat(101)}])),
    ] {
        let before = host.requests.len();
        assert!(
            host.call(&mut dispatcher, &sender, &permissions, method, args)
                .is_err(),
            "{method}"
        );
        assert_eq!(host.requests.len(), before, "{method}");
    }
    host.resources.insert("ok.png".into());
    host.call(
        &mut dispatcher,
        &sender,
        &permissions,
        "action.setIcon",
        json!([{"path":{"32":"ok.png"}}]),
    )
    .unwrap();
    host.call(
        &mut dispatcher,
        &sender,
        &permissions,
        "action.setIcon",
        json!([{"imageData":{"width":1,"height":1,"data":[1,2,3,255]}}]),
    )
    .unwrap();
    let raster = host.action.state(None).raster.unwrap();
    assert_eq!(raster.rgba().len(), 32 * 32 * 4);
    assert!(raster
        .rgba()
        .chunks_exact(4)
        .all(|pixel| pixel == [1, 2, 3, 255]));
}

#[test]
fn trusted_clicks_and_popups_grant_active_tab_and_refuse_disabled_actions() {
    let (mut host, mut dispatcher, sender, mut permissions, tab) = setup();
    let browser_tab = host.snapshot.tab(tab).unwrap().clone();
    assert!(browser_tab.check_access(&sender, &permissions).is_err());
    assert!(matches!(
        host.action
            .trusted_invoke(&browser_tab, &mut permissions)
            .unwrap(),
        Activation::Popup(_)
    ));
    assert!(browser_tab.check_access(&sender, &permissions).is_ok());
    let revoked = permissions.main_document_changed(
        browser_tab.tab.id(),
        DocumentGeneration::new(browser_tab.document.get() + 1).unwrap(),
    );
    assert_eq!(revoked.ended_active_tabs, vec![browser_tab.tab.id()]);
    assert!(browser_tab.check_access(&sender, &permissions).is_err());
    host.action
        .apply(Some(tab), &Update::Popup(String::new()))
        .unwrap();
    assert_eq!(
        host.action
            .trusted_invoke(&browser_tab, &mut permissions)
            .unwrap(),
        Activation::Click
    );
    host.action
        .apply(Some(tab), &Update::Enabled(false))
        .unwrap();
    assert!(host
        .action
        .trusted_invoke(&browser_tab, &mut permissions)
        .is_err());
    assert!(host
        .call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.openPopup",
            json!([])
        )
        .is_err());
    host.action.reset_tab(tab).unwrap();
    assert!(host
        .call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.openPopup",
            json!([])
        )
        .is_ok());
    host.snapshot.windows[0].focused = false;
    let count = host.requests.len();
    assert!(host
        .call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.openPopup",
            json!([])
        )
        .unwrap_err()
        .contains("inactive window"));
    assert_eq!(host.requests.len(), count);
    host.action.is_on_toolbar = true;
    assert_eq!(
        host.call(
            &mut dispatcher,
            &sender,
            &permissions,
            "action.getUserSettings",
            json!([])
        )
        .unwrap(),
        json!({"isOnToolbar":true})
    );
}

#[test]
fn shell_projection_sanitizes_bounded_text_without_altering_getters() {
    let (mut host, _, sender, _, tab) = setup();
    let title = format!("line\n{}", "ą".repeat(200));
    host.action
        .apply(None, &Update::Title(title.clone()))
        .unwrap();
    host.action
        .apply(None, &Update::BadgeText(Some("a".repeat(100))))
        .unwrap();
    let projected = host
        .action
        .shell_state(
            tab,
            ItemId::from(7),
            ExtensionRuntimeInstance::new(sender.profile(), sender.install(), sender.generation()),
        )
        .unwrap();
    assert!(projected.label().len() <= MAX_EXTENSION_ACTION_LABEL_BYTES);
    assert!(!projected.label().contains('\n'));
    assert_eq!(projected.badge().len(), MAX_EXTENSION_ACTION_BADGE_BYTES);
    assert_eq!(host.action.get(None, Property::Title), title);
}

#[test]
fn css_colors_cover_named_hex_alpha_rgb_hsl_and_refuse_nonfinite_values() {
    assert_eq!(color(&json!("rebeccapurple")).unwrap(), [102, 51, 153, 255]);
    assert_eq!(color(&json!("rgba(1, 2, 3, 0.5)")).unwrap(), [1, 2, 3, 128]);
    assert_eq!(
        color(&json!("hsl(120, 100%, 25%)")).unwrap(),
        [0, 128, 0, 255]
    );
    assert_eq!(color(&json!("#11223344")).unwrap(), [17, 34, 51, 68]);
    for value in [
        json!("hsl(NaN, 100%, 25%)"),
        json!("rgb(inf,0,0)"),
        json!("#abcde"),
        json!("rgb(1,,2,3)"),
        json!("rgb(1 2 3 .5)"),
        json!([1, 2, 3]),
    ] {
        assert!(color(&value).is_err());
    }
}

proptest! {
    #[test]
    fn arbitrary_default_override_and_reset_sequences_match_inheritance_model(steps in prop::collection::vec((0u8..4,0i32..8,"[a-z]{0,16}"),0..200)) {
        let mut store = ActionStore::default();
        let mut default = String::new();
        let mut local = BTreeMap::new();
        for (operation,tab,text) in steps {
            match operation {
                0 => { store.apply(None,&Update::Title(text.clone())).unwrap(); default = text; },
                1 => { store.apply(Some(tab),&Update::Title(text.clone())).unwrap(); local.insert(tab,text); },
                2 => { store.reset_tab(tab).unwrap(); local.remove(&tab); },
                _ => { store.apply(Some(tab),&Update::BadgeText(None)).unwrap(); },
            }
            for id in 0..8 { prop_assert_eq!(store.get(Some(id),Property::Title),json!(local.get(&id).unwrap_or(&default))); }
        }
    }
}
