use super::*;
use crate::api::scripting::tests::{call, fixture, permissions};
use crate::context::tests::{background_sender, doc};
use crate::events::ListenerFilter;

#[test]
fn frame_queries_filter_hosts_and_cross_check_document_and_frame_ids() {
    let mut host = fixture();
    let granted = permissions(&["webNavigation"], &["https://example.com/*"]);
    let result = call(
        &mut host,
        &granted,
        "webNavigation.getAllFrames",
        json!([{"tabId":1}]),
    )
    .unwrap();
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["frameId"], 0);
    let granted = permissions(&["webNavigation"], &["<all_urls>"]);
    let result = call(
        &mut host,
        &granted,
        "webNavigation.getFrame",
        json!([{"documentId":"child-document"}]),
    )
    .unwrap();
    assert_eq!(result["parentFrameId"], 0);
    assert_eq!(result["documentId"], "child-document");
    assert!(result.get("frameId").is_none());
    assert_eq!(
        call(
            &mut host,
            &granted,
            "webNavigation.getFrame",
            json!([{"tabId":1,"frameId":0,"documentId":"child-document"}])
        )
        .unwrap(),
        Value::Null
    );
    assert_eq!(
        call(
            &mut host,
            &granted,
            "webNavigation.getAllFrames",
            json!([{"tabId":99}])
        )
        .unwrap(),
        Value::Null
    );
    assert!(call(&mut host, &granted, "webNavigation.getFrame", json!([{}])).is_err());
}

#[test]
fn stale_or_duplicate_native_observations_fail_closed() {
    let mut host = fixture();
    let granted = permissions(&["webNavigation"], &["<all_urls>"]);
    host.frames.push(host.frames[0].clone());
    assert!(call(
        &mut host,
        &granted,
        "webNavigation.getAllFrames",
        json!([{"tabId":1}])
    )
    .is_err());
    host.frames.pop();
    host.frames[0].document = doc(99);
    assert!(call(
        &mut host,
        &granted,
        "webNavigation.getAllFrames",
        json!([{"tabId":1}])
    )
    .is_err());
}

#[test]
fn native_navigation_details_and_url_filters_are_preserved() {
    let host = fixture();
    let sender = background_sender();
    let granted = permissions(&["webNavigation"], &["<all_urls>"]);
    let mut event = NavigationObservation {
        tab_id: 1,
        frame: host.frames[1].clone(),
        event: NavigationEvent::Committed,
        time_stamp: 123.5,
        transition_type: Some("form_submit".into()),
        transition_qualifiers: vec!["server_redirect".into()],
        error: None,
    };
    let args = observation(&host, &sender, &granted, &event).unwrap();
    assert_eq!(args[0]["transitionType"], "form_submit");
    assert_eq!(args[0]["transitionQualifiers"], json!(["server_redirect"]));
    let filter = ListenerFilter::parse(
        event.event.name(),
        Some(&json!({"url":[{"hostEquals":"other.test","pathPrefix":"/frame"}]})),
    )
    .unwrap();
    assert!(filter.matches(&event_facts(&event.frame, 1)));
    assert!(!filter.matches(&event_facts(&host.frames[0], 1)));
    event.event = NavigationEvent::ErrorOccurred;
    event.error = Some("net::ERR_CONNECTION_REFUSED".into());
    let args = observation(&host, &sender, &granted, &event).unwrap();
    assert_eq!(args[0]["error"], "net::ERR_CONNECTION_REFUSED");
    event.frame.document = doc(99);
    assert!(observation(&host, &sender, &granted, &event).is_err());
}

#[test]
fn newly_created_targets_and_replacements_check_both_sides() {
    let mut host = fixture();
    let sender = background_sender();
    let granted = permissions(&["webNavigation"], &["<all_urls>"]);
    let created = CreatedNavigationTarget {
        source_tab: 1,
        source: host.frames[0].clone(),
        target_tab: 1,
        target: host.frames[1].clone(),
        time_stamp: 1.0,
    };
    assert_eq!(
        created_navigation_target(&host, &sender, &granted, &created).unwrap()[0]["sourceFrameId"],
        0
    );
    let restricted = permissions(&["webNavigation"], &["https://example.com/*"]);
    assert!(created_navigation_target(&host, &sender, &restricted, &created).is_err());
    let before = host.snapshot.clone();
    let window = host.snapshot.windows[0].id;
    let new_tab = host.add_tab(window, "https://other.test/", false);
    assert_eq!(
        tab_replaced(&host, &sender, &granted, &before, 1, new_tab, 1.0).unwrap()[0]
            ["replacedTabId"],
        1
    );
    assert!(tab_replaced(&host, &sender, &restricted, &before, 1, new_tab, 1.0).is_err());
}
