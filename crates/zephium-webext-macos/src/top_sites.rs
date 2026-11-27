//! `chrome.topSites` from the addresses visited most over recent weeks of
//! the profile's history, as the new tab page would list them.

use serde_json::{json, Value};
use zephium_core::extension_data::{
    ExtensionDataCall, ExtensionDataReply, HistoryReply, HistoryRequest, MAX_TOP_SITES,
};

use crate::data::{self, Call};

pub(crate) fn call(op: &str, _message: &Value) -> Result<Call, String> {
    if op != "get" {
        return Err(format!("topSites.{op} is not supported"));
    }
    Ok(Call {
        permission: "topSites",
        request: ExtensionDataCall::History(HistoryRequest::MostVisited {
            limit: MAX_TOP_SITES,
        }),
        answer: Box::new(|reply| match reply {
            ExtensionDataReply::History(HistoryReply::Sites(sites)) => Ok(sites
                .iter()
                .map(|site| json!({ "url": site.url, "title": site.title }))
                .collect()),
            ExtensionDataReply::History(HistoryReply::Failed) => {
                Err("Most visited sites could not be read.".into())
            }
            reply => data::unexpected(reply),
        }),
    })
}
