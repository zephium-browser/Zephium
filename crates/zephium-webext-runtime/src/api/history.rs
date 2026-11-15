//! History calls over the existing profile-scoped deferred data contract.

use serde_json::{json, Value};
use zephium_core::extension_data::{
    ExtensionDataCall, ExtensionDataReply, HistoryEntry, HistoryReply, HistoryRequest, VisitEntry,
    MAX_HISTORY_RESULTS,
};

use crate::browser::{self, BrowserPort, BrowserRequest};
use crate::dispatch::{ApiTable, Invocation, RegistrationError};
use crate::ApiError;

pub(crate) const METHODS: &[&str] = &[
    "history.search",
    "history.getVisits",
    "history.addUrl",
    "history.deleteUrl",
    "history.deleteRange",
    "history.deleteAll",
];
const DAY_MS: f64 = 86_400_000.0;

/// Chrome history item; the store has no typing-count evidence.
pub fn item(entry: &HistoryEntry) -> Value {
    json!({"id": entry.id.to_string(), "url": entry.url, "title": entry.title, "lastVisitTime": entry.last_visit as f64 * 1000.0, "visitCount": entry.visits, "typedCount": 0})
}

fn visit(entry: &VisitEntry) -> Value {
    json!({"id": entry.entry.to_string(), "visitId": entry.id.to_string(), "visitTime": entry.visited_at as f64 * 1000.0, "referringVisitId": "0", "transition": "link", "isLocal": true})
}

/// Converts milliseconds to the inclusive start or exclusive end of a seconds range.
pub fn seconds(ms: f64) -> i64 {
    (ms / 1000.0).ceil() as i64
}

pub(crate) fn data_error(reply: &ExtensionDataReply) -> ApiError {
    ApiError::other(match reply {
        ExtensionDataReply::Unavailable => {
            "This profile keeps no such data, or it can't be reached now."
        }
        ExtensionDataReply::Busy => "Too many requests are waiting; try again.",
        _ => "The browser answered with something unexpected.",
    })
}

/// Shapes only settled native/store replies; admission is never success.
pub fn reply(method: &str, reply: &ExtensionDataReply) -> Result<Value, ApiError> {
    match (method, reply) {
        ("history.search", ExtensionDataReply::History(HistoryReply::Entries(entries))) => {
            Ok(entries
                .iter()
                .take(MAX_HISTORY_RESULTS as usize)
                .map(item)
                .collect())
        }
        ("history.getVisits", ExtensionDataReply::History(HistoryReply::Visits(entries))) => {
            Ok(entries
                .iter()
                .take(zephium_core::extension_data::MAX_VISITS as usize)
                .map(visit)
                .collect())
        }
        ("history.addUrl", ExtensionDataReply::History(HistoryReply::Entries(_)))
        | (
            "history.deleteUrl" | "history.deleteRange" | "history.deleteAll",
            ExtensionDataReply::History(HistoryReply::Removed { .. }),
        ) => Ok(Value::Null),
        (_, ExtensionDataReply::History(HistoryReply::Failed)) => {
            Err(ApiError::other("History could not be read or changed."))
        }
        _ => Err(data_error(reply)),
    }
}

/// Native history change as Chrome event arguments.
pub fn removed(urls: &[String], all: bool) -> Value {
    json!([{"allHistory": all, "urls": if all { &[][..] } else { urls }}])
}

/// Installs the generated history contracts.
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

fn address(details: &Value) -> Result<String, ApiError> {
    let value = details["url"]
        .as_str()
        .ok_or_else(ApiError::malformed_arguments)?;
    url::Url::parse(value).map_err(|_| ApiError::other("Url is invalid."))?;
    Ok(value.into())
}

fn prepare(host: &impl BrowserPort, call: &Invocation<'_>) -> Result<ExtensionDataCall, ApiError> {
    browser::snapshot(host, call.sender())?;
    if call.sender().incognito() {
        return Err(data_error(&ExtensionDataReply::Unavailable));
    }
    let details = call.args().first().unwrap_or(&Value::Null);
    let request = match call.name() {
        "history.search" => {
            let now = host.now_ms() as f64;
            let start = details["startTime"]
                .as_f64()
                .unwrap_or((now - DAY_MS).max(0.0));
            let end = details["endTime"].as_f64().unwrap_or(now);
            let limit = details["maxResults"].as_u64().unwrap_or(100) as u32;
            HistoryRequest::Search {
                text: details["text"]
                    .as_str()
                    .ok_or_else(ApiError::malformed_arguments)?
                    .into(),
                start: seconds(start),
                end: seconds(end).max(seconds(start)),
                limit: if limit == 0 {
                    MAX_HISTORY_RESULTS
                } else {
                    limit.min(MAX_HISTORY_RESULTS)
                },
            }
        }
        "history.getVisits" => HistoryRequest::Visits {
            url: address(details)?,
        },
        "history.addUrl" => HistoryRequest::Add {
            url: address(details)?,
        },
        "history.deleteUrl" => HistoryRequest::RemoveUrl {
            url: address(details)?,
        },
        "history.deleteRange" => {
            let start = seconds(
                details["startTime"]
                    .as_f64()
                    .ok_or_else(ApiError::malformed_arguments)?,
            );
            let end = seconds(
                details["endTime"]
                    .as_f64()
                    .ok_or_else(ApiError::malformed_arguments)?,
            );
            HistoryRequest::RemoveRange {
                start,
                end: end.max(start),
            }
        }
        "history.deleteAll" => HistoryRequest::RemoveAll,
        _ => return Err(browser::unsupported()),
    };
    let call = ExtensionDataCall::History(request);
    if !call.validate() {
        return Err(ApiError::other("Url is invalid."));
    }
    Ok(call)
}

#[cfg(test)]
mod tests;
