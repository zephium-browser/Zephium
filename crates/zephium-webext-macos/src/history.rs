//! `chrome.history` over Zephium's history, which keeps one row per visit:
//! an address is named by its oldest visit still kept, and a visit records
//! no transition or typing, so each reads as a link and nothing as typed.

use serde_json::{json, Value};
use zephium_core::extension_data::{
    ExtensionDataCall, ExtensionDataReply, HistoryEntry, HistoryReply, HistoryRequest, VisitEntry,
    MAX_HISTORY_RESULTS,
};

use crate::data::{self, Call};

pub(crate) const PERMISSION: &str = "history";
pub(crate) const ON_VISITED: &str = "history.onVisited";
pub(crate) const ON_VISIT_REMOVED: &str = "history.onVisitRemoved";
pub(crate) const EVENTS: &[&str] = &[ON_VISITED, ON_VISIT_REMOVED];
const DAY_MS: f64 = 86_400_000.0;
const DEFAULT_RESULTS: u32 = 100;

pub(crate) fn item(entry: &HistoryEntry) -> Value {
    json!({
        "id": entry.id.to_string(),
        "url": entry.url,
        "title": entry.title,
        "lastVisitTime": entry.last_visit as f64 * 1000.0,
        "visitCount": entry.visits,
        "typedCount": 0,
    })
}

fn visit(visit: &VisitEntry) -> Value {
    json!({
        "id": visit.entry.to_string(),
        "visitId": visit.id.to_string(),
        "visitTime": visit.visited_at as f64 * 1000.0,
        "referringVisitId": "0",
        "transition": "link",
        "isLocal": true,
    })
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_millis() as f64)
}

/// Visits are kept to the second: one at second `s` happened at `s * 1000`
/// milliseconds, so it falls in `[start, end)` exactly when `s` falls in
/// `[ceil(start / 1000), ceil(end / 1000))`.
fn seconds(ms: f64) -> i64 {
    (ms / 1000.0).ceil().min(i64::MAX as f64) as i64
}

fn time(details: &Value, key: &str) -> Result<Option<f64>, String> {
    match details.get(key) {
        None | Some(Value::Null) => Ok(None),
        value => data::number(value)
            .map(Some)
            .ok_or_else(|| format!("'{key}' must be a time in milliseconds.")),
    }
}

fn address(details: &Value) -> Result<String, String> {
    let url = data::text(details, "url")?;
    if url::Url::parse(url).is_err() {
        return Err("Invalid url.".into());
    }
    Ok(url.to_owned())
}

fn done(reply: ExtensionDataReply) -> Result<Value, String> {
    match reply {
        ExtensionDataReply::History(HistoryReply::Removed { .. } | HistoryReply::Entries(_)) => {
            Ok(Value::Null)
        }
        reply => failed(reply),
    }
}

fn failed(reply: ExtensionDataReply) -> Result<Value, String> {
    match reply {
        ExtensionDataReply::History(HistoryReply::Failed) => {
            Err("History could not be read or changed.".into())
        }
        reply => data::unexpected(reply),
    }
}

pub(crate) fn call(op: &str, message: &Value) -> Result<Call, String> {
    let details = message.get("details").unwrap_or(&Value::Null);
    let (request, answer): (HistoryRequest, data::Answer) = match op {
        "search" => {
            let text = data::text(details, "text")?.to_owned();
            let end = time(details, "endTime")?.unwrap_or_else(now_ms);
            let start = time(details, "startTime")?.unwrap_or(end - DAY_MS).max(0.0);
            let limit = match details.get("maxResults") {
                None | Some(Value::Null) => DEFAULT_RESULTS,
                value => match data::number(value) {
                    Some(0.0) => MAX_HISTORY_RESULTS,
                    Some(n) => (n as u32).min(MAX_HISTORY_RESULTS),
                    None => return Err("'maxResults' must be a non-negative number.".into()),
                },
            };
            (
                HistoryRequest::Search {
                    text,
                    start: seconds(start),
                    end: seconds(end).max(seconds(start)),
                    limit,
                },
                Box::new(|reply| match reply {
                    ExtensionDataReply::History(HistoryReply::Entries(entries)) => {
                        Ok(entries.iter().map(item).collect())
                    }
                    reply => failed(reply),
                }),
            )
        }
        "getVisits" => (
            HistoryRequest::Visits {
                url: address(details)?,
            },
            Box::new(|reply| match reply {
                ExtensionDataReply::History(HistoryReply::Visits(visits)) => {
                    Ok(visits.iter().map(visit).collect())
                }
                reply => failed(reply),
            }),
        ),
        "addUrl" => {
            let url = address(details)?;
            if !zephium_core::navigation::is_allowed_str(&url) {
                return Err("Invalid url.".into());
            }
            (HistoryRequest::Add { url }, Box::new(done))
        }
        "deleteUrl" => (
            HistoryRequest::RemoveUrl {
                url: address(details)?,
            },
            Box::new(done),
        ),
        "deleteRange" => {
            let (Some(start), Some(end)) = (time(details, "startTime")?, time(details, "endTime")?)
            else {
                return Err("'startTime' and 'endTime' are required.".into());
            };
            (
                HistoryRequest::RemoveRange {
                    start: seconds(start),
                    end: seconds(end).max(seconds(start)),
                },
                Box::new(done),
            )
        }
        "deleteAll" => (HistoryRequest::RemoveAll, Box::new(done)),
        _ => return Err(format!("history.{op} is not supported")),
    };
    Ok(Call {
        permission: PERMISSION,
        request: ExtensionDataCall::History(request),
        answer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(op: &str, details: Value) -> HistoryRequest {
        match call(op, &json!({ "details": details })).map(|call| call.request) {
            Ok(ExtensionDataCall::History(request)) => request,
            other => panic!("{op}: {:?}", other.err()),
        }
    }

    #[test]
    fn times_in_milliseconds_select_the_seconds_visits_are_kept_in() {
        assert_eq!(
            request(
                "search",
                json!({ "text": "", "startTime": 1_000.0, "endTime": 2_000.5, "maxResults": 0 })
            ),
            HistoryRequest::Search {
                text: String::new(),
                start: 1,
                end: 3,
                limit: MAX_HISTORY_RESULTS
            }
        );
        // A visit at second 1 is at 1000 ms, before 1001.
        assert_eq!(
            request(
                "deleteRange",
                json!({ "startTime": 1_001.0, "endTime": 1_001.0 })
            ),
            HistoryRequest::RemoveRange { start: 2, end: 2 }
        );
        let HistoryRequest::Search {
            start, end, limit, ..
        } = request("search", json!({ "text": "a" }))
        else {
            panic!("search");
        };
        assert_eq!(limit, DEFAULT_RESULTS);
        assert!((end - start - 86_400).abs() <= 1, "the last day by default");
        assert!(call("search", &json!({ "details": {} })).is_err());
        assert!(call("addUrl", &json!({ "details": { "url": "not a url" } })).is_err());
    }

    #[test]
    fn an_address_reads_as_chrome_reports_it() {
        let entry = HistoryEntry {
            id: 7,
            url: "https://a.example/".into(),
            title: "A".into(),
            last_visit: 1_700_000_000,
            visits: 3,
        };
        assert_eq!(
            item(&entry),
            json!({
                "id": "7", "url": "https://a.example/", "title": "A",
                "lastVisitTime": 1_700_000_000_000.0, "visitCount": 3, "typedCount": 0,
            })
        );
        assert_eq!(
            visit(&VisitEntry {
                id: 9,
                entry: 7,
                visited_at: 5
            }),
            json!({
                "id": "7", "visitId": "9", "visitTime": 5000.0, "referringVisitId": "0",
                "transition": "link", "isLocal": true,
            })
        );
    }
}
