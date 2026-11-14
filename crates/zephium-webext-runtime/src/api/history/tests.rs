use super::*;
use crate::api::scripting::tests::{call, fixture, permissions};

#[test]
fn search_uses_native_contract_with_bounded_results_and_exact_millisecond_ranges() {
    let mut host = fixture();
    host.time_ms = 200_000_000;
    let granted = permissions(&["history"], &[]);
    call(
        &mut host,
        &granted,
        "history.search",
        json!([{"text":"fixture","startTime":1000.5,"endTime":2000.5,"maxResults":0}]),
    )
    .unwrap();
    assert!(
        matches!(host.requests.last(), Some(BrowserRequest::Data(ExtensionDataCall::History(HistoryRequest::Search { text, start: 2, end: 3, limit: MAX_HISTORY_RESULTS }))) if text == "fixture")
    );
    call(&mut host, &granted, "history.search", json!([{"text":""}])).unwrap();
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::History(
            HistoryRequest::Search {
                start: 113_600,
                end: 200_000,
                limit: 100,
                ..
            }
        )))
    ));
    assert_eq!(seconds(1000.0), 1);
    assert_eq!(seconds(1000.1), 2);
}

#[test]
fn history_writes_and_queries_are_typed_and_invalid_urls_are_refused() {
    let mut host = fixture();
    let granted = permissions(&["history"], &[]);
    for method in ["history.addUrl", "history.deleteUrl", "history.getVisits"] {
        call(
            &mut host,
            &granted,
            method,
            json!([{"url":"https://example.com/"}]),
        )
        .unwrap();
    }
    call(
        &mut host,
        &granted,
        "history.deleteRange",
        json!([{"startTime":1000.1,"endTime":1001}]),
    )
    .unwrap();
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::History(
            HistoryRequest::RemoveRange { start: 2, end: 2 }
        )))
    ));
    call(&mut host, &granted, "history.deleteAll", json!([])).unwrap();
    let count = host.requests.len();
    assert!(call(
        &mut host,
        &granted,
        "history.addUrl",
        json!([{"url":"javascript:alert(1)"}])
    )
    .is_err());
    assert!(call(
        &mut host,
        &granted,
        "history.getVisits",
        json!([{"url":"not-url"}])
    )
    .is_err());
    assert!(call(
        &mut host,
        &permissions(&[], &[]),
        "history.deleteAll",
        json!([])
    )
    .is_err());
    assert!(call(
        &mut host,
        &granted,
        "history.search",
        json!([{"text":"x".repeat(4097)}])
    )
    .is_err());
    assert_eq!(host.requests.len(), count);
}

#[test]
fn settled_items_visits_and_removals_keep_ids_and_native_time_units() {
    let entry = HistoryEntry {
        id: 7,
        url: "https://example.com/".into(),
        title: "Fixture".into(),
        last_visit: 2,
        visits: 3,
    };
    assert_eq!(item(&entry)["lastVisitTime"], 2000.0);
    assert_eq!(item(&entry)["id"], "7");
    let result = reply(
        "history.getVisits",
        &ExtensionDataReply::History(HistoryReply::Visits(vec![VisitEntry {
            id: 9,
            entry: 7,
            visited_at: 4,
        }])),
    )
    .unwrap();
    assert_eq!(result[0]["visitId"], "9");
    assert_eq!(result[0]["visitTime"], 4000.0);
    assert_eq!(
        removed(&["https://example.com/".into()], true),
        json!([{"allHistory":true,"urls":[]}])
    );
    assert!(reply("history.search", &ExtensionDataReply::Busy).is_err());
    assert!(reply(
        "history.deleteAll",
        &ExtensionDataReply::History(HistoryReply::Entries(vec![entry]))
    )
    .is_err());
}
