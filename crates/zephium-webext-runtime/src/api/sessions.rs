//! Recently closed ordinary tabs; the browser has no cross-device session sync.

use crate::browser::{self, BrowserPort, BrowserRequest};
use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;
use serde_json::{json, Value};
use zephium_core::extension_data::{
    ClosedTab, ExtensionDataCall, ExtensionDataReply, MAX_CLOSED_TABS,
};

pub(crate) const METHODS: &[&str] = &[
    "sessions.getRecentlyClosed",
    "sessions.getDevices",
    "sessions.restore",
];

/// Describes an ordinary closed tab; a closed record has no live tab identity.
pub fn session(tab: &ClosedTab) -> Value {
    json!({"lastModified": tab.closed_at_ms.map_or(0, |ms| ms / 1000), "tab": {"sessionId": tab.session, "url": tab.url, "title": tab.title, "index": 0, "windowId": -1, "active": false, "highlighted": false, "pinned": false, "selected": false, "discarded": false, "autoDiscardable": true, "incognito": false}})
}

/// Shapes settled data replies. A restore adapter may supply its authoritative reopened tab projection.
pub fn reply(
    method: &str,
    reply: &ExtensionDataReply,
    restored_tab: Option<Value>,
) -> Result<Value, ApiError> {
    match (method, reply) {
        ("sessions.getRecentlyClosed", ExtensionDataReply::Closed(tabs)) => Ok(tabs
            .iter()
            .take(MAX_CLOSED_TABS as usize)
            .map(session)
            .collect()),
        ("sessions.restore", ExtensionDataReply::Restored(tab)) => {
            let restored_tab = restored_tab.ok_or_else(ApiError::context_invalidated)?;
            if restored_tab
                .get("id")
                .and_then(Value::as_i64)
                .is_none_or(|id| id <= 0)
            {
                return Err(ApiError::context_invalidated());
            }
            Ok(
                json!({"lastModified": tab.closed_at_ms.map_or(0, |ms| ms / 1000), "tab": restored_tab}),
            )
        }
        ("sessions.restore", ExtensionDataReply::Unavailable) => {
            Err(ApiError::other("Could not restore session."))
        }
        _ => Err(super::history::data_error(reply)),
    }
}

/// Installs generated sessions contracts; getDevices returns the empty unsynced device list.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, |host: &mut S, call| {
            if let Err(error) = browser::snapshot(host, call.sender()) {
                return call.reply(Err(error));
            }
            if call.name() == "sessions.getDevices" {
                return call.reply(Ok(json!([])));
            }
            if call.sender().incognito() {
                return if call.name() == "sessions.getRecentlyClosed" {
                    call.reply(Ok(json!([])))
                } else {
                    call.reply(Err(ApiError::other("Could not restore session.")))
                };
            }
            let value = call.args().first().unwrap_or(&Value::Null);
            let request = if call.name() == "sessions.getRecentlyClosed" {
                ExtensionDataCall::RecentlyClosed {
                    limit: value["maxResults"]
                        .as_u64()
                        .unwrap_or(u64::from(MAX_CLOSED_TABS)) as u32,
                }
            } else {
                ExtensionDataCall::Restore {
                    session: value.as_str().map(str::to_owned),
                }
            };
            if !request.validate() {
                return call.reply(Err(ApiError::other("Invalid session id.")));
            }
            host.request(BrowserRequest::Data(request), call)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
