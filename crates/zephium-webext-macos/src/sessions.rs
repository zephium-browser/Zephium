//! `chrome.sessions` over the browser's recently closed tabs. Zephium
//! reopens tabs, not whole windows, and syncs no devices.

use serde_json::{json, Value};
use zephium_core::extension_data::{
    ClosedTab, ExtensionDataCall, ExtensionDataReply, MAX_CLOSED_TABS,
};

use crate::data::{self, Call};

pub(crate) const PERMISSION: &str = "sessions";
pub(crate) const ON_CHANGED: &str = "sessions.onChanged";

/// A closed tab as Chrome describes one: it has no tab id until it is
/// reopened, and its time is in seconds.
fn session(tab: &ClosedTab) -> Value {
    json!({
        "lastModified": tab.closed_at_ms.map_or(0, |ms| ms / 1000),
        "tab": {
            "sessionId": tab.session,
            "url": tab.url,
            "title": tab.title,
            "index": 0,
            "windowId": -1,
            "active": false,
            "highlighted": false,
            "pinned": false,
            "selected": false,
            "discarded": false,
            "autoDiscardable": true,
            "incognito": false,
        },
    })
}

pub(crate) fn call(op: &str, message: &Value) -> Result<Call, String> {
    let (request, answer): (ExtensionDataCall, data::Answer) = match op {
        "getRecentlyClosed" => {
            let limit = match message
                .get("filter")
                .and_then(|filter| filter.get("maxResults"))
            {
                None | Some(Value::Null) => MAX_CLOSED_TABS,
                value => match data::number(value) {
                    Some(n) if n <= MAX_CLOSED_TABS as f64 => n as u32,
                    _ => {
                        return Err(format!(
                            "'maxResults' must be between 0 and {MAX_CLOSED_TABS}."
                        ))
                    }
                },
            };
            (
                ExtensionDataCall::RecentlyClosed { limit },
                Box::new(|reply| match reply {
                    ExtensionDataReply::Closed(tabs) => Ok(tabs.iter().map(session).collect()),
                    reply => data::unexpected(reply),
                }),
            )
        }
        "restore" => {
            let named = match message.get("sessionId") {
                None | Some(Value::Null) => None,
                Some(Value::String(id)) => Some(id.clone()),
                Some(_) => return Err("'sessionId' must be a string.".into()),
            };
            (
                ExtensionDataCall::Restore { session: named },
                Box::new(|reply| match reply {
                    ExtensionDataReply::Restored(tab) => Ok(session(&tab)),
                    reply => data::unexpected(reply),
                }),
            )
        }
        _ => return Err(format!("sessions.{op} is not supported")),
    };
    Ok(Call {
        permission: PERMISSION,
        request,
        answer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_tab_reads_as_a_chrome_session() {
        let read = session(&ClosedTab {
            session: "01J0".into(),
            url: "https://a.example/".into(),
            title: "A".into(),
            closed_at_ms: Some(1_700_000_000_500),
        });
        assert_eq!(read["lastModified"], 1_700_000_000);
        assert_eq!(read["tab"]["sessionId"], "01J0");
        assert!(read["tab"].get("id").is_none());
        assert!(call(
            "getRecentlyClosed",
            &json!({ "filter": { "maxResults": 26 } })
        )
        .is_err());
        assert!(matches!(
            call("restore", &json!({})).map(|call| call.request),
            Ok(ExtensionDataCall::Restore { session: None })
        ));
    }
}
