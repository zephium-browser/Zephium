//! `chrome.bookmarks` over Zephium's one tree of folders and links. Chrome's
//! root "0" stands above Zephium's top level; Zephium has no bookmarks bar
//! or other permanent folders, so its ids are shifted past the ones Chrome
//! reserves for them, and an extension naming one hears it doesn't exist
//! instead of writing into an unrelated folder.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use zephium_core::bookmarks::{
    BookmarkChange, BookmarkEntry, BookmarkFailure, BookmarkReply, BookmarkRequest, MAX_ENTRIES,
};
use zephium_core::extension_data::{ExtensionDataCall, ExtensionDataReply};

use crate::data::{self, Call};

pub(crate) const PERMISSION: &str = "bookmarks";
pub(crate) const EVENTS: &[&str] = &[
    "bookmarks.onCreated",
    "bookmarks.onChanged",
    "bookmarks.onMoved",
    "bookmarks.onRemoved",
    "bookmarks.onImportBegan",
    "bookmarks.onImportEnded",
];
const ROOT: &str = "0";
/// Chrome's root and permanent folders take 0 to 3.
const RESERVED: i64 = 3;

fn chrome_id(id: i64) -> String {
    (id + RESERVED).to_string()
}

fn parent_id(parent: Option<i64>) -> String {
    parent.map_or_else(|| ROOT.to_owned(), chrome_id)
}

/// Zephium's id for a Chrome id; `Ok(None)` is the root.
fn zephium_id(id: &str) -> Result<Option<i64>, String> {
    if id == ROOT {
        return Ok(None);
    }
    match id.parse::<i64>() {
        Ok(id) if id > RESERVED => Ok(Some(id - RESERVED)),
        _ => Err("Can't find bookmark for id.".into()),
    }
}

fn not_root(id: &str) -> Result<i64, String> {
    zephium_id(id)?.ok_or_else(|| "Can't modify the root bookmark folders.".into())
}

fn node(entry: &BookmarkEntry) -> Map<String, Value> {
    let mut node = Map::new();
    node.insert("id".into(), chrome_id(entry.id).into());
    node.insert("parentId".into(), parent_id(entry.parent).into());
    node.insert("index".into(), entry.index.into());
    node.insert("title".into(), entry.title.clone().into());
    node.insert("dateAdded".into(), (entry.added_at as f64 * 1000.0).into());
    node.insert("syncing".into(), false.into());
    if let Some(url) = &entry.url {
        node.insert("url".into(), url.clone().into());
    }
    node
}

fn root() -> Map<String, Value> {
    let mut node = Map::new();
    node.insert("id".into(), ROOT.into());
    node.insert("title".into(), "".into());
    node.insert("syncing".into(), false.into());
    node
}

/// Nests entries listed folder before contents under `top`, which is the
/// root when absent.
fn tree(top: Option<&BookmarkEntry>, entries: &[BookmarkEntry]) -> Value {
    let mut children: HashMap<Option<i64>, Vec<&BookmarkEntry>> = HashMap::new();
    for entry in entries {
        children.entry(entry.parent).or_default().push(entry);
    }
    fn build(
        mut node: Map<String, Value>,
        id: Option<i64>,
        folder: bool,
        children: &HashMap<Option<i64>, Vec<&BookmarkEntry>>,
    ) -> Value {
        if folder {
            let mut below: Vec<&&BookmarkEntry> = children.get(&id).into_iter().flatten().collect();
            below.sort_by_key(|entry| entry.index);
            let below: Vec<Value> = below
                .into_iter()
                .map(|entry| {
                    build(
                        self::node(entry),
                        Some(entry.id),
                        entry.url.is_none(),
                        children,
                    )
                })
                .collect();
            node.insert("children".into(), below.into());
        }
        Value::Object(node)
    }
    match top {
        Some(top) => build(node(top), Some(top.id), top.url.is_none(), &children),
        None => build(root(), None, true, &children),
    }
}

/// Chrome's onCreated, onChanged, onMoved, onRemoved and import events.
pub(crate) fn events(change: &BookmarkChange) -> Vec<(&'static str, Value)> {
    match change {
        BookmarkChange::Created(entry) => vec![(
            "bookmarks.onCreated",
            json!([chrome_id(entry.id), node(entry)]),
        )],
        BookmarkChange::Changed { id, title, url } => {
            let mut info = Map::new();
            info.insert("title".into(), title.clone().into());
            if let Some(url) = url {
                info.insert("url".into(), url.clone().into());
            }
            vec![("bookmarks.onChanged", json!([chrome_id(*id), info]))]
        }
        BookmarkChange::Moved {
            id,
            parent,
            index,
            old_parent,
            old_index,
        } => vec![(
            "bookmarks.onMoved",
            json!([chrome_id(*id), {
                "parentId": parent_id(*parent),
                "index": index,
                "oldParentId": parent_id(*old_parent),
                "oldIndex": old_index,
            }]),
        )],
        BookmarkChange::Removed(entry) => vec![(
            "bookmarks.onRemoved",
            json!([chrome_id(entry.id), {
                "parentId": parent_id(entry.parent),
                "index": entry.index,
                "node": node(entry),
            }]),
        )],
        BookmarkChange::Imported => vec![
            ("bookmarks.onImportBegan", json!([])),
            ("bookmarks.onImportEnded", json!([])),
        ],
    }
}

fn failure(failure: BookmarkFailure, op: &str) -> String {
    match (failure, op) {
        (BookmarkFailure::Missing, "create") => "Can't find parent bookmark for id.".into(),
        (BookmarkFailure::Missing, _) => "Can't find bookmark for id.".into(),
        (BookmarkFailure::Full, "getTree" | "getSubTree") => {
            "The bookmark tree is too large to read at once; read it folder by folder.".into()
        }
        (BookmarkFailure::Full, _) => {
            "Bookmarks are full, or folders would nest too deeply.".into()
        }
        (BookmarkFailure::Cycle, _) => "Can't move a folder into itself or its contents.".into(),
        (BookmarkFailure::Invalid, "remove") => {
            "Can't remove non-empty folder (use recursive to force).".into()
        }
        (BookmarkFailure::Invalid, "update") => {
            "Only a bookmark's address can change, and only to a web address.".into()
        }
        (BookmarkFailure::Invalid, "create") => {
            "A bookmark needs a web address, and a folder a title.".into()
        }
        (BookmarkFailure::Invalid, _) => "Invalid bookmark.".into(),
        (BookmarkFailure::Unavailable, _) => "Bookmarks could not be read or changed.".into(),
    }
}

fn entries(reply: ExtensionDataReply, op: &'static str) -> Result<Vec<BookmarkEntry>, String> {
    match reply {
        ExtensionDataReply::Bookmarks(BookmarkReply::Entries(entries)) => Ok(entries),
        ExtensionDataReply::Bookmarks(BookmarkReply::Failed(why)) => Err(failure(why, op)),
        reply => data::unexpected(reply).map(|_| Vec::new()),
    }
}

fn nodes(op: &'static str) -> data::Answer {
    Box::new(move |reply| Ok(entries(reply, op)?.iter().map(node).collect()))
}

fn one(op: &'static str) -> data::Answer {
    Box::new(move |reply| {
        entries(reply, op)?
            .first()
            .map(|entry| Value::Object(node(entry)))
            .ok_or_else(|| failure(BookmarkFailure::Missing, op))
    })
}

fn id_arg(message: &Value) -> Result<&str, String> {
    data::text(message, "id")
}

fn optional_text(details: &Value, key: &str) -> Result<Option<String>, String> {
    match details.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(format!("'{key}' must be a string.")),
    }
}

fn index(details: &Value) -> Result<Option<u32>, String> {
    match details.get("index") {
        None | Some(Value::Null) => Ok(None),
        value => data::number(value)
            .map(|n| Some(n.min(u32::MAX as f64) as u32))
            .ok_or_else(|| "'index' must be a non-negative number.".into()),
    }
}

fn url(details: &Value) -> Result<Option<String>, String> {
    optional_text(details, "url")?
        .map(|url| {
            url::Url::parse(&url)
                .map(|_| url)
                .map_err(|_| "Invalid URL.".to_owned())
        })
        .transpose()
}

pub(crate) fn call(op: &str, message: &Value) -> Result<Call, String> {
    let details = message.get("details").unwrap_or(&Value::Null);
    let (request, answer): (BookmarkRequest, data::Answer) = match op {
        "getTree" => (
            BookmarkRequest::Subtree { root: None },
            Box::new(|reply| Ok(json!([tree(None, &entries(reply, "getTree")?)]))),
        ),
        "getSubTree" => {
            let root = zephium_id(id_arg(message)?)?;
            (
                BookmarkRequest::Subtree { root },
                Box::new(move |reply| {
                    let entries = entries(reply, "getSubTree")?;
                    Ok(json!([match root {
                        None => tree(None, &entries),
                        Some(_) => tree(entries.first(), &entries[1.min(entries.len())..]),
                    }]))
                }),
            )
        }
        "getChildren" => (
            BookmarkRequest::ChildEntries {
                parent: zephium_id(id_arg(message)?)?,
            },
            nodes("getChildren"),
        ),
        "get" => {
            let ids: Vec<&str> = match message.get("ids") {
                Some(Value::Array(ids)) if !ids.is_empty() => ids
                    .iter()
                    .map(|id| id.as_str().ok_or("Bookmark ids must be strings."))
                    .collect::<Result<_, _>>()?,
                _ => return Err("At least one bookmark id is required.".into()),
            };
            if ids.len() > MAX_ENTRIES as usize {
                return Err(format!(
                    "At most {MAX_ENTRIES} bookmarks can be read at once."
                ));
            }
            let order: Vec<Option<i64>> = ids
                .iter()
                .map(|id| zephium_id(id))
                .collect::<Result<_, _>>()?;
            let wanted: Vec<i64> = order.iter().flatten().copied().collect();
            if wanted.is_empty() {
                // Only the root, which the store doesn't hold.
                return Ok(Call {
                    permission: PERMISSION,
                    request: ExtensionDataCall::Bookmarks(BookmarkRequest::ChildEntries {
                        parent: None,
                    }),
                    answer: Box::new(move |reply| {
                        entries(reply, "get")?;
                        Ok(order.iter().map(|_| Value::Object(root())).collect())
                    }),
                });
            }
            (
                BookmarkRequest::Entries { ids: wanted },
                Box::new(move |reply| {
                    let found = entries(reply, "get")?;
                    let mut found = found.iter();
                    Ok(order
                        .iter()
                        .map(|id| match id {
                            None => Value::Object(root()),
                            Some(_) => found
                                .next()
                                .map_or(Value::Null, |entry| Value::Object(node(entry))),
                        })
                        .collect())
                }),
            )
        }
        "getRecent" => {
            let count = data::number(message.get("count"))
                .filter(|count| *count >= 1.0)
                .ok_or("numberOfItems cannot be less than 1.")?;
            (
                BookmarkRequest::Recent {
                    limit: count.min(MAX_ENTRIES as f64) as u32,
                },
                nodes("getRecent"),
            )
        }
        "search" => {
            let request = match details {
                Value::String(query) => BookmarkRequest::Find {
                    query: Some(query.clone()),
                    url: None,
                    title: None,
                },
                Value::Object(_) => BookmarkRequest::Find {
                    query: optional_text(details, "query")?,
                    url: url(details)?,
                    title: optional_text(details, "title")?,
                },
                _ => return Err("The search query must be a string or an object.".into()),
            };
            (request, nodes("search"))
        }
        "create" => {
            let parent = match optional_text(details, "parentId")? {
                Some(id) => {
                    zephium_id(&id).map_err(|_| failure(BookmarkFailure::Missing, "create"))?
                }
                None => None,
            };
            (
                BookmarkRequest::Create {
                    parent,
                    index: index(details)?,
                    title: optional_text(details, "title")?.unwrap_or_default(),
                    url: url(details)?,
                },
                one("create"),
            )
        }
        "update" => {
            let id = not_root(id_arg(message)?)?;
            (
                BookmarkRequest::Update {
                    id,
                    title: optional_text(details, "title")?,
                    url: url(details)?,
                },
                one("update"),
            )
        }
        "move" => {
            let id = not_root(id_arg(message)?)?;
            let parent = match optional_text(details, "parentId")? {
                Some(parent) => Some(zephium_id(&parent)?),
                None => None,
            };
            (
                BookmarkRequest::Relocate {
                    id,
                    parent,
                    index: index(details)?,
                },
                one("move"),
            )
        }
        "remove" | "removeTree" => (
            BookmarkRequest::Delete {
                id: not_root(id_arg(message)?)?,
                recursive: op == "removeTree",
            },
            Box::new(|reply| match reply {
                ExtensionDataReply::Bookmarks(BookmarkReply::Done) => Ok(Value::Null),
                reply => entries(reply, "remove").map(|_| Value::Null),
            }),
        ),
        _ => return Err(format!("bookmarks.{op} is not supported")),
    };
    Ok(Call {
        permission: PERMISSION,
        request: ExtensionDataCall::Bookmarks(request),
        answer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64, parent: Option<i64>, index: u32, url: Option<&str>) -> BookmarkEntry {
        BookmarkEntry {
            id,
            parent,
            index,
            url: url.map(str::to_owned),
            title: format!("n{id}"),
            added_at: 2,
        }
    }

    fn request(op: &str, message: Value) -> Result<BookmarkRequest, String> {
        call(op, &message).map(|call| match call.request {
            ExtensionDataCall::Bookmarks(request) => request,
            other => panic!("{other:?}"),
        })
    }

    #[test]
    fn ids_step_past_chromes_permanent_folders() {
        assert_eq!(
            request("getChildren", json!({ "id": "0" })),
            Ok(BookmarkRequest::ChildEntries { parent: None })
        );
        assert_eq!(
            request("getChildren", json!({ "id": "4" })),
            Ok(BookmarkRequest::ChildEntries { parent: Some(1) })
        );
        // "1" is Chrome's bookmarks bar, which Zephium doesn't have.
        assert_eq!(
            request(
                "create",
                json!({ "details": { "parentId": "1", "title": "x" } })
            ),
            Err("Can't find parent bookmark for id.".into())
        );
        assert_eq!(
            request("remove", json!({ "id": "0" })),
            Err("Can't modify the root bookmark folders.".into())
        );
        assert_eq!(
            request("move", json!({ "id": "5", "details": { "index": 2 } })),
            Ok(BookmarkRequest::Relocate {
                id: 2,
                parent: None,
                index: Some(2)
            })
        );
        assert_eq!(
            request("move", json!({ "id": "5", "details": { "parentId": "0" } })),
            Ok(BookmarkRequest::Relocate {
                id: 2,
                parent: Some(None),
                index: None
            })
        );
    }

    #[test]
    fn a_tree_nests_entries_in_their_order_under_the_root() {
        let entries = [
            entry(1, None, 0, None),
            entry(3, Some(1), 1, Some("https://b.example/")),
            entry(2, Some(1), 0, Some("https://a.example/")),
            entry(4, None, 1, Some("https://c.example/")),
        ];
        let tree = tree(None, &entries);
        assert_eq!(tree["id"], "0");
        let top = tree["children"].as_array().unwrap();
        assert_eq!(top[0]["id"], "4");
        assert_eq!(top[0]["parentId"], "0");
        assert_eq!(top[0]["dateAdded"], 2000.0);
        assert_eq!(
            top[0]["children"]
                .as_array()
                .unwrap()
                .iter()
                .map(|node| node["url"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["https://a.example/", "https://b.example/"]
        );
        assert!(top[1].get("children").is_none(), "links have no children");
    }

    #[test]
    fn changes_read_as_chromes_events() {
        let moved = events(&BookmarkChange::Moved {
            id: 2,
            parent: None,
            index: 0,
            old_parent: Some(1),
            old_index: 3,
        });
        assert_eq!(
            moved,
            vec![(
                "bookmarks.onMoved",
                json!(["5", { "parentId": "0", "index": 0, "oldParentId": "4", "oldIndex": 3 }])
            )]
        );
        let removed = events(&BookmarkChange::Removed(entry(2, Some(1), 1, None)));
        assert_eq!(removed[0].1[1]["parentId"], "4");
        assert_eq!(removed[0].1[1]["node"]["id"], "5");
        assert_eq!(events(&BookmarkChange::Imported).len(), 2);
    }
}
