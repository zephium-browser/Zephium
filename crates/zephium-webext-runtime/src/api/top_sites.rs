//! Most-visited pages through the profile's native history data service.

use crate::browser::{self, BrowserPort, BrowserRequest};
use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;
use serde_json::{json, Value};
use zephium_core::extension_data::{
    ExtensionDataCall, ExtensionDataReply, HistoryReply, HistoryRequest, MAX_TOP_SITES,
};

pub(crate) const METHODS: &[&str] = &["topSites.get"];

/// Shapes a settled top-sites reply.
pub fn reply(reply: &ExtensionDataReply) -> Result<Value, ApiError> {
    match reply {
        ExtensionDataReply::History(HistoryReply::Sites(sites)) => Ok(sites
            .iter()
            .take(MAX_TOP_SITES as usize)
            .map(|site| json!({"url": site.url, "title": site.title}))
            .collect()),
        ExtensionDataReply::History(HistoryReply::Failed) => {
            Err(ApiError::other("Most visited sites could not be read."))
        }
        _ => Err(super::history::data_error(reply)),
    }
}

/// Installs the generated top-sites contract.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    table.register_generated(METHODS[0], |host: &mut S, call| {
        if let Err(error) = browser::snapshot(host, call.sender()) {
            return call.reply(Err(error));
        }
        if call.sender().incognito() {
            return call.reply(Ok(json!([])));
        }
        host.request(
            BrowserRequest::Data(ExtensionDataCall::History(HistoryRequest::MostVisited {
                limit: MAX_TOP_SITES,
            })),
            call,
        )
    })
}

#[cfg(test)]
mod tests;
