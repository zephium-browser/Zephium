//! Bookmark CRUD over the native data contract, with a synthetic immutable root.

use crate::browser::{self, BrowserPort, BrowserRequest};
use crate::dispatch::{ApiTable, Invocation, RegistrationError};
use crate::ApiError;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use zephium_core::bookmarks::{
    BookmarkChange, BookmarkEntry, BookmarkFailure, BookmarkReply, BookmarkRequest, MAX_ENTRIES,
};
use zephium_core::extension_data::{ExtensionDataCall, ExtensionDataReply};

pub(crate) const METHODS: &[&str] = &[
    "bookmarks.get",
    "bookmarks.getChildren",
    "bookmarks.getRecent",
    "bookmarks.getTree",
    "bookmarks.getSubTree",
    "bookmarks.search",
    "bookmarks.create",
    "bookmarks.move",
    "bookmarks.update",
    "bookmarks.remove",
    "bookmarks.removeTree",
];

const ROOT: &str = "0";
/// Chrome's root and permanent folders take 0 to 3.
const RESERVED: i64 = 3;

fn chrome_id(id: i64) -> String {
    (i128::from(id) + i128::from(RESERVED)).to_string()
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
fn tree(top: Option<&BookmarkEntry>, entries: &[BookmarkEntry]) -> Result<Value, ApiError> {
    let mut children: HashMap<Option<i64>, Vec<&BookmarkEntry>> = HashMap::new();
    for entry in entries {
        children.entry(entry.parent).or_default().push(entry);
    }
    fn build(
        mut node: Map<String, Value>,
        id: Option<i64>,
        folder: bool,
        children: &HashMap<Option<i64>, Vec<&BookmarkEntry>>,
        depth: usize,
    ) -> Result<Value, ApiError> {
        if depth > zephium_core::bookmarks::MAX_DEPTH {
            return Err(ApiError::other("Invalid bookmark tree."));
        }
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
                        depth + 1,
                    )
                })
                .collect::<Result<_, _>>()?;
            node.insert("children".into(), below.into());
        }
        Ok(Value::Object(node))
    }
    match top {
        Some(top) => build(node(top), Some(top.id), top.url.is_none(), &children, 0),
        None => build(root(), None, true, &children, 0),
    }
}

/// Chrome's onCreated, onChanged, onMoved, onRemoved and import events.
pub fn events(change: &BookmarkChange) -> Vec<(&'static str, Value)> {
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

fn id(value: &Value) -> Result<Option<i64>, ApiError> {
    zephium_id(value.as_str().ok_or_else(ApiError::malformed_arguments)?).map_err(ApiError::other)
}
fn writable(value: &Value) -> Result<i64, ApiError> {
    not_root(value.as_str().ok_or_else(ApiError::malformed_arguments)?).map_err(ApiError::other)
}
fn text(details: &Value, key: &str) -> Option<String> {
    details.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn index(details: &Value) -> Option<u32> {
    details["index"].as_u64().map(|value| value as u32)
}
fn address(details: &Value) -> Result<Option<String>, ApiError> {
    text(details, "url")
        .map(|url| {
            url::Url::parse(&url)
                .map(|_| url)
                .map_err(|_| ApiError::other("Invalid URL."))
        })
        .transpose()
}
fn ids(value: &Value) -> Result<Vec<Option<i64>>, ApiError> {
    let result = if let Some(values) = value.as_array() {
        values.iter().map(id).collect::<Result<Vec<_>, _>>()?
    } else {
        vec![id(value)?]
    };
    if result.is_empty() {
        return Err(ApiError::other("At least one bookmark id is required."));
    }
    if result.len() > MAX_ENTRIES as usize {
        return Err(ApiError::too_many_pending());
    }
    Ok(result)
}

/// Shapes settled bookmark data using the original normalized invocation arguments.
pub fn reply(method: &str, args: &[Value], reply: &ExtensionDataReply) -> Result<Value, ApiError> {
    let op = method.strip_prefix("bookmarks.").unwrap_or(method);
    let entries = match reply {
        ExtensionDataReply::Bookmarks(BookmarkReply::Entries(entries)) => entries,
        ExtensionDataReply::Bookmarks(BookmarkReply::Done)
            if matches!(op, "remove" | "removeTree") =>
        {
            return Ok(Value::Null)
        }
        ExtensionDataReply::Bookmarks(BookmarkReply::Failed(error)) => {
            return Err(ApiError::other(failure(*error, op)))
        }
        _ => return Err(super::history::data_error(reply)),
    };
    if entries.len() > zephium_core::bookmarks::MAX_SUBTREE as usize {
        return Err(ApiError::message_too_large());
    }
    match op {
        "getTree" => Ok(json!([tree(None, entries)?])),
        "getSubTree" => {
            let root = id(args.first().unwrap_or(&Value::Null))?;
            let top = root
                .map(|root| {
                    entries
                        .iter()
                        .find(|entry| entry.id == root)
                        .ok_or_else(|| ApiError::other("Can't find bookmark for id."))
                })
                .transpose()?;
            Ok(json!([tree(top, entries)?]))
        }
        "get" => {
            let wanted = ids(args.first().unwrap_or(&Value::Null))?;
            wanted
                .iter()
                .map(|id| match id {
                    None => Ok(Value::Object(root())),
                    Some(id) => entries
                        .iter()
                        .find(|entry| entry.id == *id)
                        .map(|entry| Value::Object(node(entry)))
                        .ok_or_else(|| ApiError::other("Can't find bookmark for id.")),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        "create" | "update" | "move" => entries
            .first()
            .map(|entry| Value::Object(node(entry)))
            .ok_or_else(|| ApiError::other("Can't find bookmark for id.")),
        "getChildren" | "getRecent" | "search" => Ok(entries
            .iter()
            .map(|entry| Value::Object(node(entry)))
            .collect()),
        _ => Err(browser::unsupported()),
    }
}

/// Installs every generated bookmark method.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, |host: &mut S, call| match prepare(host, &call) {
            Ok(request) => host.request(BrowserRequest::Data(request), call),
            Err(error) => call.reply(Err(error)),
        })?;
    }
    Ok(())
}

fn prepare(host: &impl BrowserPort, call: &Invocation<'_>) -> Result<ExtensionDataCall, ApiError> {
    browser::snapshot(host, call.sender())?;
    if call.sender().incognito() {
        return Err(super::history::data_error(&ExtensionDataReply::Unavailable));
    }
    let first = call.args().first().unwrap_or(&Value::Null);
    let second = call.args().get(1).unwrap_or(&Value::Null);
    let request = match call.name() {
        "bookmarks.getTree" => BookmarkRequest::Subtree { root: None },
        "bookmarks.getSubTree" => BookmarkRequest::Subtree { root: id(first)? },
        "bookmarks.getChildren" => BookmarkRequest::ChildEntries { parent: id(first)? },
        "bookmarks.get" => {
            let ids: Vec<_> = ids(first)?.into_iter().flatten().collect();
            if ids.is_empty() {
                BookmarkRequest::ChildEntries { parent: None }
            } else {
                BookmarkRequest::Entries { ids }
            }
        }
        "bookmarks.getRecent" => {
            let count = first
                .as_u64()
                .filter(|count| *count > 0)
                .ok_or_else(|| ApiError::other("numberOfItems cannot be less than 1."))?;
            BookmarkRequest::Recent {
                limit: count.min(u64::from(MAX_ENTRIES)) as u32,
            }
        }
        "bookmarks.search" => {
            if let Some(query) = first.as_str() {
                BookmarkRequest::Find {
                    query: Some(query.into()),
                    url: None,
                    title: None,
                }
            } else {
                BookmarkRequest::Find {
                    query: text(first, "query"),
                    url: address(first)?,
                    title: text(first, "title"),
                }
            }
        }
        "bookmarks.create" => BookmarkRequest::Create {
            parent: first
                .get("parentId")
                .map(id)
                .transpose()
                .map_err(|_| ApiError::other("Can't find parent bookmark for id."))?
                .flatten(),
            index: index(first),
            title: text(first, "title").unwrap_or_default(),
            url: address(first)?,
        },
        "bookmarks.update" => BookmarkRequest::Update {
            id: writable(first)?,
            title: text(second, "title"),
            url: address(second)?,
        },
        "bookmarks.move" => BookmarkRequest::Relocate {
            id: writable(first)?,
            parent: second.get("parentId").map(id).transpose()?,
            index: index(second),
        },
        "bookmarks.remove" | "bookmarks.removeTree" => BookmarkRequest::Delete {
            id: writable(first)?,
            recursive: call.name() == "bookmarks.removeTree",
        },
        _ => return Err(browser::unsupported()),
    };
    let request = ExtensionDataCall::Bookmarks(request);
    if !request.validate() {
        return Err(ApiError::message_too_large());
    }
    Ok(request)
}

#[cfg(test)]
mod tests;
