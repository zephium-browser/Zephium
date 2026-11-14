use super::*;
use crate::api::scripting::tests::{call, fixture, permissions};

#[test]
fn recently_closed_restore_and_empty_sync_devices_use_settled_data_contract() {
    let mut host = fixture();
    let granted = permissions(&["sessions"], &[]);
    call(
        &mut host,
        &granted,
        "sessions.getRecentlyClosed",
        json!([{"maxResults":0}]),
    )
    .unwrap();
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::RecentlyClosed {
            limit: 0
        }))
    ));
    call(&mut host, &granted, "sessions.restore", json!(["closed-1"])).unwrap();
    let Some(BrowserRequest::Data(ExtensionDataCall::Restore { session: Some(id) })) =
        host.requests.last()
    else {
        panic!("restore");
    };
    assert_eq!(id, "closed-1");
    call(&mut host, &granted, "sessions.restore", json!([])).unwrap();
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::Restore {
            session: None
        }))
    ));
    let count = host.requests.len();
    assert_eq!(
        call(&mut host, &granted, "sessions.getDevices", json!([])).unwrap(),
        json!([])
    );
    assert_eq!(host.requests.len(), count);
    assert!(call(
        &mut host,
        &granted,
        "sessions.restore",
        json!(["x".repeat(65)])
    )
    .is_err());
    assert!(call(
        &mut host,
        &granted,
        "sessions.getRecentlyClosed",
        json!([{"maxResults":26}])
    )
    .is_err());
}

#[test]
fn restored_sessions_require_authoritative_live_tab_identity() {
    let closed = ClosedTab {
        session: "closed-1".into(),
        url: "https://example.com/".into(),
        title: "Fixture".into(),
        closed_at_ms: Some(2001),
    };
    let data = ExtensionDataReply::Closed(vec![closed.clone()]);
    let result = reply("sessions.getRecentlyClosed", &data, None).unwrap();
    assert_eq!(result[0]["lastModified"], 2);
    assert!(result[0]["tab"].get("id").is_none());
    let data = ExtensionDataReply::Restored(closed);
    assert!(reply("sessions.restore", &data, None).is_err());
    assert!(reply("sessions.restore", &data, Some(json!({"id":-1}))).is_err());
    assert_eq!(
        reply(
            "sessions.restore",
            &data,
            Some(json!({"id":7,"url":"https://example.com/"}))
        )
        .unwrap()["tab"]["id"],
        7
    );
    assert!(reply("sessions.restore", &ExtensionDataReply::Unavailable, None).is_err());
}

#[test]
fn private_instances_never_forward_durable_data_calls() {
    use crate::api::scripting::tests::call_as;
    use crate::browser::{fake::FakeBrowser, IncognitoMode};
    use crate::context::tests::{admission, committed, install, profile, EXTENSION};
    use crate::{Authority, ExtensionScheme, ViewKind};
    let mut authority = Authority::new(profile(), true, ExtensionScheme::CHROME_EXTENSION);
    let mut admitted = admission(install(), EXTENSION, 1);
    admitted.allowed_in_incognito = true;
    authority.admit(admitted).unwrap();
    let view = authority
        .open_view(install(), ViewKind::Background)
        .unwrap();
    let sender = authority
        .bind_view(
            &view,
            &committed(&format!("chrome-extension://{EXTENSION}/bg.html"), 1),
        )
        .unwrap();
    let mut host = FakeBrowser::new(profile());
    host.info.incognito_allowed = true;
    host.info.incognito_mode = IncognitoMode::Split;
    let window = host.add_window(true);
    host.add_tab(window, "https://example.com/", true);
    let granted = permissions(&["sessions", "history", "bookmarks", "topSites"], &[]);
    for (method, args) in [
        ("sessions.getRecentlyClosed", json!([])),
        ("sessions.getDevices", json!([])),
        ("topSites.get", json!([])),
    ] {
        assert_eq!(
            call_as(&mut host, &sender, &granted, method, args).unwrap(),
            json!([])
        );
    }
    for (method, args) in [
        ("sessions.restore", json!([])),
        ("history.search", json!([{"text":""}])),
        ("bookmarks.getTree", json!([])),
    ] {
        assert!(call_as(&mut host, &sender, &granted, method, args).is_err());
    }
    assert!(host.requests.is_empty());
}
