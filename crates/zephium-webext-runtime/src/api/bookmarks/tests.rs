use super::*;
use crate::api::scripting::tests::{call, fixture, permissions};

fn entry(id: i64, parent: Option<i64>, index: u32, url: Option<&str>) -> BookmarkEntry {
    BookmarkEntry {
        id,
        parent,
        index,
        url: url.map(str::to_owned),
        title: format!("node{id}"),
        added_at: 2,
    }
}

#[test]
fn every_method_maps_to_the_existing_bookmark_contract() {
    let mut host = fixture();
    let granted = permissions(&["bookmarks"], &[]);
    for (method, args) in [
        ("bookmarks.get", json!(["4"])),
        ("bookmarks.get", json!([["0", "4"]])),
        ("bookmarks.getChildren", json!(["0"])),
        ("bookmarks.getRecent", json!([3])),
        ("bookmarks.getTree", json!([])),
        ("bookmarks.getSubTree", json!(["4"])),
        ("bookmarks.search", json!(["fixture"])),
        (
            "bookmarks.search",
            json!([{"url":"https://example.com/","title":"Fixture"}]),
        ),
        ("bookmarks.create", json!([{"title":"Folder"}])),
        ("bookmarks.update", json!(["4",{"title":"Renamed"}])),
        ("bookmarks.move", json!(["4",{"parentId":"0","index":2}])),
        ("bookmarks.remove", json!(["4"])),
        ("bookmarks.removeTree", json!(["4"])),
    ] {
        call(&mut host, &granted, method, args).unwrap();
        assert!(matches!(
            host.requests.last(),
            Some(BrowserRequest::Data(ExtensionDataCall::Bookmarks(_)))
        ));
    }
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::Bookmarks(
            BookmarkRequest::Delete {
                id: 1,
                recursive: true
            }
        )))
    ));
    call(&mut host, &granted, "bookmarks.move", json!(["4", {}])).unwrap();
    assert!(matches!(
        host.requests.last(),
        Some(BrowserRequest::Data(ExtensionDataCall::Bookmarks(
            BookmarkRequest::Relocate {
                id: 1,
                parent: None,
                index: None
            }
        )))
    ));
}

#[test]
fn reserved_roots_and_malformed_ids_cannot_mutate_other_nodes() {
    let mut host = fixture();
    let granted = permissions(&["bookmarks"], &[]);
    for (method, args) in [
        ("bookmarks.remove", json!(["0"])),
        ("bookmarks.remove", json!(["1"])),
        ("bookmarks.remove", json!(["-1"])),
        ("bookmarks.remove", json!(["99999999999999999999999999"])),
        (
            "bookmarks.create",
            json!([{"parentId":"1","title":"Folder"}]),
        ),
        ("bookmarks.create", json!([{"url":"not-url"}])),
        ("bookmarks.get", json!([[]])),
        ("bookmarks.getRecent", json!([0])),
    ] {
        assert!(call(&mut host, &granted, method, args).is_err());
    }
    assert!(host.requests.is_empty());
}

#[test]
fn settled_trees_nest_by_sibling_order_and_get_keeps_root_and_requested_order() {
    let entries = vec![
        entry(1, None, 0, None),
        entry(3, Some(1), 1, Some("https://b.test/")),
        entry(2, Some(1), 0, Some("https://a.test/")),
    ];
    let data = ExtensionDataReply::Bookmarks(BookmarkReply::Entries(entries));
    let tree = reply("bookmarks.getTree", &[], &data).unwrap();
    assert_eq!(tree[0]["id"], "0");
    assert_eq!(tree[0]["children"][0]["children"][0]["id"], "5");
    assert_eq!(tree[0]["children"][0]["children"][1]["id"], "6");
    assert_eq!(tree[0]["children"][0]["dateAdded"], 2000.0);
    let get = reply("bookmarks.get", &[json!(["6", "0", "4"])], &data).unwrap();
    assert_eq!(get[0]["id"], "6");
    assert_eq!(get[1]["id"], "0");
    assert_eq!(get[2]["id"], "4");
    assert!(reply("bookmarks.get", &[json!("99")], &data).is_err());
}

#[test]
fn bookmark_events_and_failed_settlement_keep_native_identity() {
    let change = BookmarkChange::Moved {
        id: 2,
        parent: None,
        index: 0,
        old_parent: Some(1),
        old_index: 3,
    };
    assert_eq!(
        events(&change),
        vec![(
            "bookmarks.onMoved",
            json!(["5",{"parentId":"0","index":0,"oldParentId":"4","oldIndex":3}])
        )]
    );
    assert_eq!(
        reply(
            "bookmarks.remove",
            &[json!("4")],
            &ExtensionDataReply::Bookmarks(BookmarkReply::Failed(BookmarkFailure::Invalid))
        )
        .unwrap_err()
        .message(),
        "Can't remove non-empty folder (use recursive to force)."
    );
    assert_eq!(
        reply(
            "bookmarks.removeTree",
            &[json!("4")],
            &ExtensionDataReply::Bookmarks(BookmarkReply::Done)
        )
        .unwrap(),
        Value::Null
    );
}
