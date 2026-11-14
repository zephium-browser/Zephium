use super::*;
use crate::api::scripting::tests::{call, fixture, permissions};
use serde_json::json;

#[test]
fn default_search_carries_query_and_exact_native_destination() {
    let mut host = fixture();
    let granted = permissions(&["search"], &["<all_urls>"]);
    for (details, expected) in [
        (json!({"text":"fixture","tabId":1}), Disposition::CurrentTab),
        (
            json!({"text":"fixture","disposition":"NEW_TAB"}),
            Disposition::NewTab,
        ),
        (
            json!({"text":"fixture","disposition":"NEW_WINDOW"}),
            Disposition::NewWindow,
        ),
    ] {
        call(&mut host, &granted, "search.query", json!([details])).unwrap();
        let Some(BrowserRequest::Service(ServiceRequest::Search(request))) = host.requests.last()
        else {
            panic!("search");
        };
        assert_eq!(request.text, "fixture");
        assert_eq!(request.disposition, expected);
        assert_eq!(request.tab, Some(host.snapshot.tabs[0].tab.id()));
        assert_eq!(request.document, Some(host.snapshot.tabs[0].document));
    }
}

#[test]
fn search_rejects_ambiguous_missing_or_inaccessible_destinations() {
    let mut host = fixture();
    let granted = permissions(&["search"], &["<all_urls>"]);
    for details in [
        json!({"text":""}),
        json!({"text":"fixture","tabId":1,"disposition":"CURRENT_TAB"}),
        json!({"text":"fixture","tabId":99}),
        json!({"text":"x".repeat(MAX_QUERY_BYTES+1)}),
    ] {
        assert!(call(&mut host, &granted, "search.query", json!([details])).is_err());
    }
    assert!(call(
        &mut host,
        &permissions(&["search"], &[]),
        "search.query",
        json!([{"text":"fixture","tabId":1}])
    )
    .is_err());
    assert!(host.requests.is_empty());
}
