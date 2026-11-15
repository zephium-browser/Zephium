//! Schema-validated capabilities with no native implementation yet.

use crate::browser::{BrowserPort, BrowserRequest};
use crate::dispatch::{ApiTable, RegistrationError};

pub(crate) fn methods() -> impl Iterator<Item = &'static str> {
    super::generated::METHOD_VALIDATORS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| {
            [
                "cookies.",
                "downloads.",
                "identity.",
                "readingList.",
                "tabGroups.",
                "tts.",
                "desktopCapture.",
                "tabCapture.",
                "omnibox.",
                "devtools.",
                "userScripts.",
                "management.",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
                && *name != "management.getSelf"
                && !name.ends_with(".addListener")
                && !name.ends_with(".removeListener")
                && !name.ends_with(".hasListener")
                && !name.ends_with(".hasListeners")
                // Frontend-only declarations have no native caller contract.
                && crate::schema::access(name).is_some()
        })
}

/// Preserve generated argument, caller and permission checks before refusal.
pub fn register<S: BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for name in methods() {
        table.register_generated(name, move |host: &mut S, call| {
            host.request(BrowserRequest::Unavailable { api: name }, call)
        })?;
    }
    Ok(())
}
