//! Default-provider search intent; the browser owns provider choice and URL construction.

use crate::browser::{self, BrowserPort, BrowserRequest, ServiceRequest};
use crate::dispatch::{ApiTable, RegistrationError};
use crate::ApiError;
use zephium_core::ids::ItemId;

pub(crate) const METHODS: &[&str] = &["search.query"];
/// Maximum UTF-8 query bytes.
pub const MAX_QUERY_BYTES: usize = 4 * 1024;

/// Default search destination.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    /// Navigate the resolved caller/active ordinary tab.
    CurrentTab,
    /// Create an ordinary tab.
    NewTab,
    /// Create an ordinary window.
    NewWindow,
}

/// Validated search intent; adapters use the selected profile's default provider.
#[derive(Clone, Debug)]
pub struct Request {
    /// Bounded query text, never interpreted as a navigation URL.
    pub text: String,
    /// Destination mode.
    pub disposition: Disposition,
    /// Exact native destination/anchor tab when one exists.
    pub tab: Option<ItemId>,
    /// Main-document fence of the destination/anchor tab.
    pub document: Option<crate::context::DocumentGeneration>,
}

/// Installs the generated search contract.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    table.register_generated(METHODS[0], |host: &mut S, call| {
        let result = (|| {
            let details = &call.args()[0];
            let text = details["text"]
                .as_str()
                .ok_or_else(ApiError::malformed_arguments)?;
            if text.is_empty() {
                return Err(ApiError::other("Empty text parameter."));
            }
            if text.len() > MAX_QUERY_BYTES {
                return Err(ApiError::message_too_large());
            }
            let id = details["tabId"].as_i64();
            if id.is_some() && details.get("disposition").is_some() {
                return Err(ApiError::other(
                    "Cannot set both 'disposition' and 'tabId'.",
                ));
            }
            let snapshot = browser::snapshot(host, call.sender())?;
            let disposition = match details["disposition"].as_str() {
                Some("NEW_TAB") => Disposition::NewTab,
                Some("NEW_WINDOW") => Disposition::NewWindow,
                _ => Disposition::CurrentTab,
            };
            let tab = if let Some(id) = id {
                Some(snapshot.tab(id as i32)?)
            } else {
                call.sender()
                    .tab()
                    .and_then(|id| snapshot.tabs.iter().find(|tab| tab.tab.id() == id))
                    .or_else(|| {
                        snapshot.tabs.iter().find(|tab| {
                            tab.active && Some(tab.window_id) == snapshot.current_window
                        })
                    })
            };
            if disposition == Disposition::CurrentTab && tab.is_none() {
                return Err(ApiError::other("No active browser."));
            }
            if let Some(tab) = tab {
                tab.check_access(call.sender(), call.permissions())?;
            }
            Ok(Request {
                text: text.into(),
                disposition,
                tab: tab.map(|tab| tab.tab.id()),
                document: tab.map(|tab| tab.document),
            })
        })();
        match result {
            Ok(request) => host.request(
                BrowserRequest::Service(ServiceRequest::Search(request)),
                call,
            ),
            Err(error) => call.reply(Err(error)),
        }
    })
}

#[cfg(test)]
mod tests;
