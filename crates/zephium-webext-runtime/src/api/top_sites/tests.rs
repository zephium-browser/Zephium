use super::*;
use crate::api::scripting::tests::{call, fixture, permissions};
use zephium_core::extension_data::TopSite;

#[test]
fn top_sites_is_permission_gated_and_uses_existing_native_ranked_query() {
    let mut host = fixture();
    assert!(call(&mut host, &permissions(&[], &[]), "topSites.get", json!([])).is_err());
    assert!(host.requests.is_empty());
    call(
        &mut host,
        &permissions(&["topSites"], &[]),
        "topSites.get",
        json!([]),
    )
    .unwrap();
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::History(
            HistoryRequest::MostVisited {
                limit: MAX_TOP_SITES
            }
        )))
    ));
    let result = reply(&ExtensionDataReply::History(HistoryReply::Sites(vec![
        TopSite {
            url: "https://example.com/".into(),
            title: "Fixture".into(),
        },
    ])))
    .unwrap();
    assert_eq!(
        result,
        json!([{"url":"https://example.com/","title":"Fixture"}])
    );
    assert!(reply(&ExtensionDataReply::Unavailable).is_err());
}
