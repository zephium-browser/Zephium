use super::*;
use crate::context::tests::{
    admission, background_sender, committed, doc, install, profile, EXTENSION,
};
use crate::{Authority, ExtensionScheme, ViewKind};
use zephium_core::extensions::{ExtensionBrowserSurfaceGeneration, ExtensionBrowserWindow};

fn surface(private: bool) -> ExtensionBrowserSurface {
    let tabs = (1..=2)
        .map(|id| {
            ExtensionBrowserTab::from_snapshot(
                None,
                ItemId::from(id as u128),
                true,
                "Fixture",
                Some(&Url::parse("https://example.com/").unwrap()),
                false,
                false,
            )
            .unwrap()
        })
        .collect();
    ExtensionBrowserSurface::new(
        profile(),
        ExtensionBrowserSurfaceGeneration::INITIAL,
        Some(1),
        vec![ExtensionBrowserWindow::new(1, private, Some(ItemId::from(1)), tabs).unwrap()],
    )
    .unwrap()
}

fn projection(private: bool) -> BrowserSnapshot {
    BrowserSnapshot::from_surface(
        &surface(private),
        |id| Some(if id == ItemId::from(1) { 1 } else { 2 }),
        |_| Some(1),
        |id| Some((doc(1), id.to_string())),
    )
    .unwrap()
}

#[test]
fn stable_identity_mapping_is_required_and_aliases_are_refused() {
    assert!(BrowserSnapshot::from_surface(
        &surface(false),
        |_| None,
        |_| Some(1),
        |_| Some((doc(1), "document".into()))
    )
    .is_err());
    assert!(BrowserSnapshot::from_surface(
        &surface(false),
        |_| Some(1),
        |_| Some(1),
        |id| Some((doc(1), id.to_string()))
    )
    .is_err());
    assert!(BrowserSnapshot::from_surface(
        &surface(false),
        |id| Some(if id == ItemId::from(1) { 1 } else { 2 }),
        |_| Some(1),
        |_| Some((doc(1), "same-document".into()))
    )
    .is_err());
}

#[test]
fn profile_and_private_policy_are_checked_before_enumeration() {
    let sender = background_sender();
    let info = ExtensionInfo::default();
    let mut snapshot = projection(false);
    assert!(snapshot.authorize(&sender, &info).is_ok());
    snapshot.profile = ProfileId::from(999);
    assert!(snapshot.authorize(&sender, &info).is_err());
    assert!(projection(true).authorize(&sender, &info).is_err());

    let mut authority = Authority::new(profile(), true, ExtensionScheme::CHROME_EXTENSION);
    let mut admitted = admission(install(), EXTENSION, 1);
    admitted.allowed_in_incognito = true;
    authority.admit(admitted).unwrap();
    let view = authority
        .open_view(install(), ViewKind::Background)
        .unwrap();
    let private_sender = authority
        .bind_view(
            &view,
            &committed(&format!("chrome-extension://{EXTENSION}/bg.html"), 1),
        )
        .unwrap();
    for mode in [
        IncognitoMode::Spanning,
        IncognitoMode::Split,
        IncognitoMode::NotAllowed,
    ] {
        for allowed in [false, true] {
            let info = ExtensionInfo {
                incognito_mode: mode,
                incognito_allowed: allowed,
                ..ExtensionInfo::default()
            };
            assert_eq!(
                projection(true).authorize(&private_sender, &info).is_ok(),
                allowed && mode != IncognitoMode::NotAllowed
            );
        }
    }
}

#[test]
fn malformed_browser_facts_fail_closed() {
    let sender = background_sender();
    let mut snapshot = projection(false);
    snapshot.tabs[0].window_id = 99;
    assert!(snapshot
        .authorize(&sender, &ExtensionInfo::default())
        .is_err());
    let mut snapshot = projection(false);
    snapshot.tabs[0].zoom = f64::NAN;
    assert!(snapshot
        .authorize(&sender, &ExtensionInfo::default())
        .is_err());
}

#[test]
fn native_arm64_platform_reports_chromium_nacl_arch() {
    if std::env::consts::ARCH == "aarch64" {
        let info = super::ExtensionInfo::default().platform;
        assert_eq!(info["arch"], "arm64");
        assert_eq!(info["nacl_arch"], "arm");
    }
}
