//! `chrome.downloads` over the browser's download manager: a download an
//! extension starts goes through the same admission, destination choice and
//! dangerous-type hold as one the page in front starts. Zephium keeps a
//! download's origin, never its full address, and doesn't hand extensions
//! file paths, so `url` is that origin and `filename` the file's name.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{json, Map, Value};
use zephium_core::downloads::{DownloadError, DownloadState, DownloadView};

use crate::runtime::Shared;
use crate::{api, data, events};

pub(crate) const PERMISSION: &str = "downloads";
pub(crate) const OPEN_PERMISSION: &str = "downloads.open";
pub(crate) const ON_CREATED: &str = "downloads.onCreated";
pub(crate) const ON_CHANGED: &str = "downloads.onChanged";
pub(crate) const ON_ERASED: &str = "downloads.onErased";
pub(crate) const EVENTS: &[&str] = &[ON_CREATED, ON_CHANGED, ON_ERASED];
/// Results one search returns.
const MAX_RESULTS: usize = 100;
/// A `data:` address an extension may hand over, its own blobs included.
pub(crate) const MAX_DATA_URL_BYTES: usize = 32 * 1024 * 1024;

/// What `chrome.downloads` asks of the browser's download manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadRequest {
    /// Downloads `url` from the focused window's tab, as that page would.
    Start {
        url: String,
        filename: Option<String>,
    },
    /// The downloads in progress and the most recent ones, newest first.
    List,
    Pause(u64),
    Resume(u64),
    Cancel(u64),
    /// Forgets a finished download; its file stays.
    Erase(u64),
    Open(u64),
    Show(u64),
}

#[derive(Clone, Debug)]
pub struct DownloadItem {
    /// Numbered by the browser for this run.
    pub id: u64,
    pub view: DownloadView,
}

#[derive(Clone, Debug)]
pub enum DownloadAnswer {
    Started(u64),
    Items(Vec<DownloadItem>),
    Done,
}

/// The last downloads listeners heard about; `None` while nobody listens.
#[derive(Default)]
pub(crate) struct Seen(std::cell::RefCell<Option<HashMap<u64, Value>>>);

impl Seen {
    pub(crate) fn listening(&self) -> bool {
        self.0.borrow().is_some()
    }
}

fn interrupt(error: DownloadError) -> &'static str {
    match error {
        DownloadError::Network => "NETWORK_FAILED",
        DownloadError::ConnectionLost => "NETWORK_DISCONNECTED",
        DownloadError::Timeout => "NETWORK_TIMEOUT",
        DownloadError::Server | DownloadError::Source => "SERVER_FAILED",
        DownloadError::Authentication => "SERVER_UNAUTHORIZED",
        DownloadError::Certificate => "SERVER_CERT_PROBLEM",
        DownloadError::DiskFull => "FILE_NO_SPACE",
        DownloadError::Permission => "FILE_ACCESS_DENIED",
        DownloadError::FileTooLarge => "FILE_TOO_LARGE",
        DownloadError::Integrity => "FILE_HASH_MISMATCH",
        DownloadError::Protection => "FILE_BLOCKED",
        DownloadError::Cancelled => "USER_CANCELED",
        _ => "FILE_FAILED",
    }
}

fn iso(seconds: i64) -> String {
    // Days from the civil calendar, as Chrome's ISO 8601 times read.
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        secs / 3_600,
        secs % 3_600 / 60,
        secs % 60
    )
}

pub(crate) fn item(item: &DownloadItem) -> Value {
    let view = &item.view;
    let number = |text: &str| text.parse::<u64>().unwrap_or(0);
    let received = number(&view.received);
    let total = view.total.as_deref().map(number);
    let (state, error) = match view.state {
        DownloadState::Completed => ("complete", None),
        DownloadState::Cancelled => ("interrupted", Some("USER_CANCELED")),
        DownloadState::Interrupted | DownloadState::Failed => (
            "interrupted",
            Some(view.error.map_or("FILE_FAILED", interrupt)),
        ),
        _ => ("in_progress", None),
    };
    let mut out = Map::new();
    out.insert("id".into(), item.id.into());
    out.insert("url".into(), view.source.clone().into());
    out.insert("finalUrl".into(), view.source.clone().into());
    out.insert("referrer".into(), "".into());
    out.insert("filename".into(), view.filename.clone().into());
    out.insert("incognito".into(), false.into());
    out.insert(
        "danger".into(),
        if view.state == DownloadState::Held {
            "file"
        } else {
            "safe"
        }
        .into(),
    );
    out.insert("mime".into(), "".into());
    out.insert(
        "startTime".into(),
        iso(view.created_at.parse().unwrap_or(0)).into(),
    );
    out.insert("state".into(), state.into());
    out.insert(
        "paused".into(),
        (view.state == DownloadState::Paused).into(),
    );
    out.insert(
        "canResume".into(),
        (view.state == DownloadState::Paused).into(),
    );
    if let Some(error) = error {
        out.insert("error".into(), error.into());
    }
    out.insert("bytesReceived".into(), received.into());
    out.insert(
        "totalBytes".into(),
        total.map_or(json!(-1), |total| total.into()),
    );
    out.insert(
        "fileSize".into(),
        match view.state {
            DownloadState::Completed => received.into(),
            _ => total.map_or(json!(-1), |total| total.into()),
        },
    );
    out.insert(
        "exists".into(),
        (view.state == DownloadState::Completed).into(),
    );
    Value::Object(out)
}

/// The fields Chrome reports changes of; bytes received are not among them.
const WATCHED: &[&str] = &[
    "url",
    "finalUrl",
    "filename",
    "danger",
    "mime",
    "startTime",
    "state",
    "paused",
    "canResume",
    "error",
    "totalBytes",
    "fileSize",
    "exists",
];

fn delta(id: u64, before: &Value, now: &Value) -> Option<Value> {
    let mut delta = Map::new();
    for key in WATCHED {
        let (old, new) = (before.get(*key), now.get(*key));
        if old != new {
            let mut change = Map::new();
            if let Some(old) = old {
                change.insert("previous".into(), old.clone());
            }
            if let Some(new) = new {
                change.insert("current".into(), new.clone());
            }
            delta.insert((*key).into(), Value::Object(change));
        }
    }
    (!delta.is_empty()).then(|| {
        delta.insert("id".into(), id.into());
        Value::Object(delta)
    })
}

pub(crate) fn hears(shared: &Shared) -> bool {
    shared.loaded.borrow().keys().any(|id| {
        api::granted(shared, id, PERMISSION)
            && EVENTS.iter().any(|name| events::listened(shared, id, name))
    })
}

/// Starts or stops keeping what listeners last heard; a fresh start takes
/// the downloads as they are, so nothing already there reads as new.
pub(crate) fn refresh(shared: &Rc<Shared>) {
    let listening = hears(shared);
    if listening == shared.downloads_seen.0.borrow().is_some() {
        return;
    }
    if !listening {
        *shared.downloads_seen.0.borrow_mut() = None;
        return;
    }
    *shared.downloads_seen.0.borrow_mut() = Some(HashMap::new());
    let weak = Rc::downgrade(shared);
    shared.host().downloads(
        DownloadRequest::List,
        Box::new(move |answer| {
            let (Some(shared), Ok(DownloadAnswer::Items(items))) = (weak.upgrade(), answer) else {
                return;
            };
            let mut seen = shared.downloads_seen.0.borrow_mut();
            if let Some(seen) = seen.as_mut() {
                for found in &items {
                    seen.entry(found.id).or_insert_with(|| item(found));
                }
            }
        }),
    );
}

/// The browser's downloads now, and those it forgot: listeners hear what
/// was created, changed or erased since they last heard.
pub(crate) fn changed(shared: &Rc<Shared>, items: Vec<DownloadItem>, erased: Vec<u64>) {
    let mut raised = Vec::new();
    {
        let mut seen = shared.downloads_seen.0.borrow_mut();
        let Some(seen) = seen.as_mut() else { return };
        for found in &items {
            let now = item(found);
            match seen.insert(found.id, now.clone()) {
                None => raised.push((ON_CREATED, json!([now]))),
                Some(before) => {
                    if let Some(delta) = delta(found.id, &before, &now) {
                        raised.push((ON_CHANGED, json!([delta])));
                    }
                }
            }
        }
        for id in erased {
            if seen.remove(&id).is_some() {
                raised.push((ON_ERASED, json!([id])));
            }
        }
    }
    let listening: Vec<String> = shared
        .loaded
        .borrow()
        .keys()
        .filter(|id| api::granted(shared, id, PERMISSION))
        .cloned()
        .collect();
    for id in listening {
        for (name, args) in &raised {
            if events::listened(shared, &id, name) {
                events::emit(shared, &id, name, args.clone(), None);
            }
        }
    }
}

fn id(message: &Value) -> Result<u64, String> {
    message
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Invalid downloadId.".into())
}

/// The file name Chrome takes as a path below the downloads folder; the
/// browser keeps only its last part.
fn file_name(options: &Value) -> Result<Option<String>, String> {
    match options.get("filename") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) => {
            if name.starts_with('/') || name.split(['/', '\\']).any(|part| part == "..") {
                return Err("Invalid filename".into());
            }
            Ok(name
                .rsplit(['/', '\\'])
                .next()
                .filter(|part| !part.is_empty())
                .map(str::to_owned))
        }
        Some(_) => Err("'filename' must be a string.".into()),
    }
}

fn start(options: &Value) -> Result<DownloadRequest, String> {
    let url = data::text(options, "url")?;
    let parsed = url::Url::parse(url).map_err(|_| "Invalid URL".to_owned())?;
    match parsed.scheme() {
        "http" | "https" => {}
        "data" if url.len() <= MAX_DATA_URL_BYTES => {}
        "data" => return Err("The data is too large to download.".into()),
        _ => return Err("Invalid URL".into()),
    }
    if options
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|method| !method.eq_ignore_ascii_case("GET"))
        || options.get("body").is_some_and(|body| !body.is_null())
        || options
            .get("headers")
            .and_then(Value::as_array)
            .is_some_and(|headers| !headers.is_empty())
    {
        return Err("Only plain GET downloads are supported.".into());
    }
    if options
        .get("conflictAction")
        .and_then(Value::as_str)
        .is_some_and(|action| action != "uniquify")
    {
        return Err("Downloads never replace a file; a name already taken gets a number.".into());
    }
    Ok(DownloadRequest::Start {
        url: url.to_owned(),
        filename: file_name(options)?,
    })
}

pub(crate) fn request(
    shared: &Rc<Shared>,
    extension: &str,
    op: &str,
    message: &Value,
    done: data::Done,
) {
    if !api::granted(shared, extension, PERMISSION) {
        return done(Err(format!(
            "The extension doesn't have the '{PERMISSION}' permission."
        )));
    }
    let details = message.get("details").unwrap_or(&Value::Null);
    let request = match op {
        "download" => start(details),
        "search" => Ok(DownloadRequest::List),
        "pause" => id(message).map(DownloadRequest::Pause),
        "resume" => id(message).map(DownloadRequest::Resume),
        "cancel" => id(message).map(DownloadRequest::Cancel),
        "erase" => id(message).map(DownloadRequest::Erase),
        "show" => id(message).map(DownloadRequest::Show),
        "open" if !api::granted(shared, extension, OPEN_PERMISSION) => Err(format!(
            "The extension doesn't have the '{OPEN_PERMISSION}' permission."
        )),
        "open" => id(message).map(DownloadRequest::Open),
        _ => Err(format!("downloads.{op} is not supported")),
    };
    let request = match request {
        Ok(request) => request,
        Err(message) => return done(Err(message)),
    };
    let query = details.clone();
    shared.host().downloads(
        request,
        Box::new(move |answer| {
            done(answer.map(|answer| match answer {
                DownloadAnswer::Started(id) => id.into(),
                DownloadAnswer::Items(items) => search(&items, &query),
                DownloadAnswer::Done => Value::Null,
            }))
        }),
    );
}

/// Chrome's search over what the browser lists: exact fields, every word of
/// `query` in the name or origin, and the order and limit asked.
fn search(items: &[DownloadItem], query: &Value) -> Value {
    let words: Vec<String> = query
        .get("query")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_lowercase)
        .collect();
    let limit = match query.get("limit").and_then(Value::as_u64) {
        Some(0) | None => MAX_RESULTS,
        Some(limit) => (limit as usize).min(MAX_RESULTS),
    };
    let mut found: Vec<Value> = items
        .iter()
        .map(item)
        .filter(|item| {
            let haystack = format!(
                "{} {}",
                item["filename"].as_str().unwrap_or_default(),
                item["url"].as_str().unwrap_or_default()
            )
            .to_lowercase();
            words.iter().all(|word| haystack.contains(word.as_str()))
                && [
                    "id",
                    "url",
                    "finalUrl",
                    "filename",
                    "danger",
                    "mime",
                    "state",
                    "paused",
                    "error",
                    "exists",
                    "totalBytes",
                    "fileSize",
                    "bytesReceived",
                    "startTime",
                ]
                .iter()
                .all(|key| {
                    query
                        .get(*key)
                        .is_none_or(|wanted| item.get(*key) == Some(wanted))
                })
        })
        .collect();
    let oldest_first = query
        .get("orderBy")
        .and_then(Value::as_array)
        .and_then(|order| order.first())
        .and_then(Value::as_str)
        == Some("startTime");
    if oldest_first {
        found.reverse();
    }
    found.truncate(limit);
    Value::Array(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(state: DownloadState, error: Option<DownloadError>) -> DownloadView {
        DownloadView {
            id: "01HZZZZZZZZZZZZZZZZZZZZZZZ".into(),
            revision: "00000001".into(),
            created_at: "1700000000".into(),
            filename: "report.pdf".into(),
            source: "https://files.example".into(),
            source_is_context: false,
            state,
            received: "10".into(),
            total: Some("20".into()),
            error,
        }
    }

    #[test]
    fn a_download_reads_as_a_chrome_item() {
        let read = item(&DownloadItem {
            id: 3,
            view: view(DownloadState::Paused, None),
        });
        assert_eq!(read["state"], "in_progress");
        assert_eq!(read["paused"], true);
        assert_eq!(read["canResume"], true);
        assert_eq!(read["startTime"], "2023-11-14T22:13:20.000Z");
        assert_eq!(read["totalBytes"], 20);
        assert_eq!(read["url"], "https://files.example");
        let failed = item(&DownloadItem {
            id: 3,
            view: view(DownloadState::Failed, Some(DownloadError::DiskFull)),
        });
        assert_eq!(
            (failed["state"].clone(), failed["error"].clone()),
            (json!("interrupted"), json!("FILE_NO_SPACE"))
        );
    }

    #[test]
    fn only_watched_fields_make_a_change() {
        let before = item(&DownloadItem {
            id: 1,
            view: view(DownloadState::Receiving, None),
        });
        let mut more = view(DownloadState::Receiving, None);
        more.received = "15".into();
        assert_eq!(
            delta(1, &before, &item(&DownloadItem { id: 1, view: more })),
            None
        );
        let done = item(&DownloadItem {
            id: 1,
            view: view(DownloadState::Completed, None),
        });
        let change = delta(1, &before, &done).unwrap();
        assert_eq!(
            change["state"],
            json!({ "previous": "in_progress", "current": "complete" })
        );
        assert_eq!(
            change["exists"],
            json!({ "previous": false, "current": true })
        );
        assert_eq!(change["id"], 1);
    }

    #[test]
    fn a_download_starts_only_as_a_plain_web_or_data_address() {
        assert_eq!(
            start(&json!({ "url": "https://a.example/f.zip", "filename": "dir/f.zip" })),
            Ok(DownloadRequest::Start {
                url: "https://a.example/f.zip".into(),
                filename: Some("f.zip".into())
            })
        );
        assert!(start(&json!({ "url": "data:text/plain,hi" })).is_ok());
        assert!(start(&json!({ "url": "file:///etc/passwd" })).is_err());
        assert!(start(&json!({ "url": "https://a.example/", "filename": "../x" })).is_err());
        assert!(start(&json!({ "url": "https://a.example/", "method": "POST" })).is_err());
        assert!(
            start(&json!({ "url": "https://a.example/", "conflictAction": "overwrite" })).is_err()
        );
    }

    #[test]
    fn a_search_filters_and_orders_as_asked() {
        let items: Vec<DownloadItem> = (1..=3)
            .map(|id| {
                let mut found = view(
                    if id == 2 {
                        DownloadState::Completed
                    } else {
                        DownloadState::Receiving
                    },
                    None,
                );
                found.filename = format!("file{id}.zip");
                DownloadItem { id, view: found }
            })
            .collect();
        let ids = |value: Value| {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_u64().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(search(&items, &json!({ "state": "complete" }))), [2]);
        assert_eq!(ids(search(&items, &json!({ "query": ["FILE3"] }))), [3]);
        assert_eq!(ids(search(&items, &json!({ "limit": 2 }))), [1, 2]);
        assert_eq!(
            ids(search(&items, &json!({ "orderBy": ["startTime"] }))),
            [3, 2, 1]
        );
    }
}
