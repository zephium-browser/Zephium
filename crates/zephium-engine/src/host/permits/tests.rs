use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::navigation_epoch::NavigationEpochTracker;

use super::{navigation_callback_matches, EventPermit};

#[cfg(target_os = "windows")]
#[test]
fn extension_navigation_requires_live_profile_grant_and_view_token() {
    let token = Arc::new(AtomicBool::new(true));
    let grants = super::ExtensionNavigationGrants::default();
    let allowed = EventPermit::bound(&token).with_extensions(grants.clone());
    let other_profile =
        EventPermit::bound(&token).with_extensions(super::ExtensionNavigationGrants::default());
    let target = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/options.html";
    assert!(!allowed.allows_navigation(target));
    grants
        .lock()
        .unwrap()
        .insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    assert!(allowed.allows_navigation(target));
    assert!(!other_profile.allows_navigation(target));
    assert!(!allowed.allows_navigation(
        "chrome-extension://user@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/options.html"
    ));
    let stale = allowed.clone();
    grants.lock().unwrap().clear();
    assert!(!stale.allows_navigation(target));
    grants
        .lock()
        .unwrap()
        .insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    allowed.revoke();
    assert!(!stale.allows_navigation(target));
}

#[test]
fn native_event_permit_is_one_shot_and_generation_exact() {
    let first = Arc::new(AtomicBool::new(true));
    let replacement = Arc::new(AtomicBool::new(true));
    let permit = EventPermit::inactive();
    let callback_copy = permit.clone();

    assert!(permit.bind_once(&first));
    assert!(permit.matches_token(&first));
    assert!(callback_copy.same_generation(&permit));
    assert!(!callback_copy.bind_once(&replacement));
    assert!(!permit.matches_token(&replacement));

    permit.revoke();
    assert!(callback_copy.active_token().is_none());
    assert!(!permit.bind_once(&replacement));

    let replacement_permit = EventPermit::bound(&replacement);
    assert!(!replacement_permit.same_generation(&permit));
    assert!(replacement_permit.matches_token(&replacement));
}

#[test]
fn revoking_an_inactive_spare_is_terminal() {
    let token = Arc::new(AtomicBool::new(true));
    let permit = EventPermit::inactive();
    let callback_copy = permit.clone();

    assert!(permit.allows_navigation("about:blank"));
    permit.revoke();

    assert!(!callback_copy.allows_navigation("about:blank"));
    assert!(!callback_copy.bind_once(&token));
    assert!(permit.active_token().is_none());
}

#[test]
fn navigation_callbacks_require_generation_tracker_and_epoch_identity() {
    let token = Arc::new(AtomicBool::new(true));
    let permit = EventPermit::bound(&token);
    let callback_permit = permit.clone();
    let navigation = NavigationEpochTracker::new();
    let callback_navigation = navigation.clone();
    let first = navigation
        .begin("https://example.com/first")
        .expect("first navigation epoch");

    assert!(navigation_callback_matches(
        &permit,
        &navigation,
        &callback_permit,
        &callback_navigation,
        first,
    ));

    let second = navigation
        .begin("https://example.com/second")
        .expect("second navigation epoch");
    assert!(!navigation_callback_matches(
        &permit,
        &navigation,
        &callback_permit,
        &callback_navigation,
        first,
    ));
    assert!(navigation_callback_matches(
        &permit,
        &navigation,
        &callback_permit,
        &callback_navigation,
        second,
    ));

    // The same outer token does not make a replacement native generation
    // or a distinct epoch tracker equivalent to the callback owner.
    let replacement_permit = EventPermit::bound(&token);
    assert!(!navigation_callback_matches(
        &replacement_permit,
        &navigation,
        &callback_permit,
        &callback_navigation,
        second,
    ));
    let replacement_navigation = NavigationEpochTracker::new();
    let coincident = replacement_navigation
        .begin("https://example.com/second")
        .expect("replacement navigation epoch");
    assert!(!navigation_callback_matches(
        &permit,
        &replacement_navigation,
        &callback_permit,
        &callback_navigation,
        coincident,
    ));
}

#[test]
fn a_download_only_view_cannot_regain_page_authority() {
    let first = Arc::new(AtomicBool::new(true));
    let second = Arc::new(AtomicBool::new(true));
    let permit = EventPermit::bound(&first);
    assert!(permit.retire_for_download());
    assert!(permit.active_token().is_none());
    assert!(permit.allows_navigation("about:blank"));
    assert!(!permit.allows_navigation("https://example.com"));
    assert!(!permit.bind_once(&second));
    assert!(!permit.retire_for_download());
    permit.revoke();
    assert!(!permit.allows_navigation("about:blank"));
}

#[cfg(target_os = "windows")]
#[test]
fn extension_close_requires_own_granted_document_and_live_view() {
    let token = Arc::new(AtomicBool::new(true));
    let grants = super::ExtensionNavigationGrants::default();
    grants
        .lock()
        .unwrap()
        .insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    let permit = EventPermit::bound(&token).with_extensions(grants.clone());
    let own = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/options.html";
    assert!(permit.allows_extension_document_close(own));
    for url in [
        "https://example.com",
        "about:blank",
        "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/options.html",
    ] {
        assert!(!permit.allows_extension_document_close(url));
    }
    grants.lock().unwrap().clear();
    assert!(!permit.allows_extension_document_close(own));
    grants
        .lock()
        .unwrap()
        .insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    permit.revoke();
    assert!(!permit.allows_extension_document_close(own));
}

#[cfg(target_os = "windows")]
#[test]
fn extension_frame_requires_granted_extension_document_and_live_view() {
    let token = Arc::new(AtomicBool::new(true));
    let grants = super::ExtensionNavigationGrants::default();
    let permit = EventPermit::bound(&token).with_extensions(grants.clone());
    let menu = "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/inline/menu/menu.html";
    assert!(!permit.allows_extension_frame(menu));
    grants
        .lock()
        .unwrap()
        .insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    assert!(permit.allows_extension_frame(menu));
    for url in [
        // Ordinary frames are the security handler's, never an extension grant.
        "https://example.com/frame",
        "about:blank",
        "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/inline/menu/menu.html",
        "chrome-extension://user@aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/inline/menu/menu.html",
        "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:8080/inline/menu/menu.html",
        "chrome://settings",
        "edge://settings",
    ] {
        assert!(!permit.allows_extension_frame(url), "{url}");
    }
    let without_grants = EventPermit::bound(&token);
    assert!(!without_grants.allows_extension_frame(menu));
    let spare = EventPermit::inactive().with_extensions(grants.clone());
    assert!(!spare.allows_extension_frame(menu));
    assert!(spare.bind_once(&token));
    assert!(spare.allows_extension_frame(menu));
    grants.lock().unwrap().clear();
    assert!(!permit.allows_extension_frame(menu));
    grants
        .lock()
        .unwrap()
        .insert("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into());
    permit.revoke();
    assert!(!permit.allows_extension_frame(menu));
}
