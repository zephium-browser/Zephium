//! Offscreen documents (`chrome.offscreen`): at most one hidden extension
//! page per install. It keeps the background from idling out, and it closes
//! when its background stops for any reason other than idling (which it
//! prevents). The per-view clipboard setting is enabled only for a document
//! created with the `CLIPBOARD` reason by an extension holding
//! `clipboardRead` or `clipboardWrite`.

use serde_json::Value;
use url::Url;
use zephium_webext_runtime::context::ExtensionId;
use zephium_webext_runtime::errors::ApiError;

/// `offscreen.Reason`.
pub const REASONS: [&str; 15] = [
    "TESTING",
    "AUDIO_PLAYBACK",
    "IFRAME_SCRIPTING",
    "DOM_SCRAPING",
    "BLOBS",
    "DOM_PARSER",
    "USER_MEDIA",
    "DISPLAY_MEDIA",
    "WEB_RTC",
    "CLIPBOARD",
    "LOCAL_STORAGE",
    "WORKERS",
    "BATTERY_STATUS",
    "MATCH_MEDIA",
    "GEOLOCATION",
];

/// A validated `offscreen.createDocument` request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    /// The document URL, inside the extension.
    pub url: String,
    /// Whether the view may read and write the clipboard.
    pub clipboard: bool,
}

/// Checks `createDocument`'s parameters with Chrome's error texts.
pub(crate) fn check(
    parameters: &Value,
    extension: &ExtensionId,
    clipboard_permission: bool,
) -> Result<Request, ApiError> {
    let reasons: Vec<&str> = parameters["reasons"]
        .as_array()
        .map(|reasons| reasons.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if reasons.is_empty() {
        return Err(ApiError::other("A reason must be provided."));
    }
    if reasons.iter().any(|reason| !REASONS.contains(reason)) {
        return Err(ApiError::malformed_arguments());
    }
    if parameters["justification"]
        .as_str()
        .is_none_or(|text| text.trim().is_empty())
    {
        return Err(ApiError::other("A justification must be provided."));
    }
    let base = Url::parse(&format!("chrome-extension://{}/", extension.as_str()))
        .map_err(|_| ApiError::malformed_arguments())?;
    let url = parameters["url"]
        .as_str()
        .and_then(|url| base.join(url).ok())
        .filter(|url| {
            url.scheme() == base.scheme()
                && url.host_str() == base.host_str()
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && !url.path().starts_with("/_")
                && url.path() != "/"
        })
        .ok_or_else(|| ApiError::other("Invalid URL."))?;
    Ok(Request {
        url: url.to_string(),
        clipboard: clipboard_permission && reasons.contains(&"CLIPBOARD"),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn id() -> ExtensionId {
        ExtensionId::parse("abcdefghijklmnopabcdefghijklmnop").unwrap()
    }

    #[test]
    fn clipboard_needs_both_the_reason_and_a_permission() {
        let request = json!({"url": "off.html", "reasons": ["CLIPBOARD"], "justification": "copy"});
        assert!(check(&request, &id(), true).unwrap().clipboard);
        assert!(!check(&request, &id(), false).unwrap().clipboard);
        let other =
            json!({"url": "/off.html", "reasons": ["DOM_PARSER"], "justification": "parse"});
        let checked = check(&other, &id(), true).unwrap();
        assert!(!checked.clipboard);
        assert_eq!(
            checked.url,
            "chrome-extension://abcdefghijklmnopabcdefghijklmnop/off.html"
        );
    }

    #[test]
    fn refuses_foreign_reserved_and_malformed_documents() {
        for request in [
            json!({"url": "https://example.com/", "reasons": ["TESTING"], "justification": "x"}),
            json!({"url": "chrome-extension://bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/a.html", "reasons": ["TESTING"], "justification": "x"}),
            json!({"url": "/_zephium/background.html", "reasons": ["TESTING"], "justification": "x"}),
            json!({"url": "a.html", "reasons": [], "justification": "x"}),
            json!({"url": "a.html", "reasons": ["NOPE"], "justification": "x"}),
            json!({"url": "a.html", "reasons": ["TESTING"], "justification": " "}),
        ] {
            assert!(check(&request, &id(), true).is_err(), "{request}");
        }
    }
}
