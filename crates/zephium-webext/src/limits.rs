//! What this browser cannot do for a given extension, in plain words, so the
//! install review can say so. Only features the extension declares or asks
//! for up front count; optional permissions are the extension's own to handle.

use serde_json::Value;

use crate::manifest::Manifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

const MAC: u8 = 1;
const WIN: u8 = 2;
const LIN: u8 = 4;
const ALL: u8 = MAC | WIN | LIN;

enum Uses {
    /// An API permission; a trailing `.` matches the whole family.
    Permission(&'static str),
    /// A dotted path into the manifest.
    Key(&'static str),
}
use Uses::{Key, Permission};

/// What each platform's runtime lacks today, in the order shown. A platform
/// is left out of a row's mask when it supports the feature or has not been
/// verified; unverified features stay silent rather than guess.
const GAPS: &[(&[Uses], &str, u8)] = &[
    (
        &[Permission("sidePanel"), Key("side_panel")],
        "Can't show its side panel",
        ALL,
    ),
    (
        &[Key("devtools_page")],
        "Developer tools panels aren't available",
        ALL,
    ),
    (
        &[Key("chrome_url_overrides.newtab")],
        "Can't replace the new tab page",
        ALL,
    ),
    (
        &[Key("chrome_url_overrides.history")],
        "Can't replace the history page",
        ALL,
    ),
    (
        &[Key("chrome_url_overrides.bookmarks")],
        "Can't replace the bookmarks page",
        ALL,
    ),
    (
        &[Permission("nativeMessaging")],
        "Can't talk to apps on your computer (native messaging)",
        WIN | LIN,
    ),
    (
        &[Permission("webRequestBlocking")],
        "Can't block requests the old way (webRequestBlocking)",
        MAC | LIN,
    ),
    (
        &[Permission("tabGroups")],
        "Can't manage tab groups",
        MAC | LIN,
    ),
    (
        &[Permission("readingList")],
        "Can't use the reading list",
        MAC | LIN,
    ),
    (
        &[Permission("userScripts")],
        "Can't run user scripts",
        MAC | LIN,
    ),
    (
        &[Permission("debugger")],
        "Can't use the page debugger",
        MAC | LIN,
    ),
    (
        &[Permission("proxy")],
        "Can't change proxy settings",
        MAC | LIN,
    ),
    (
        &[Permission("tts"), Permission("ttsEngine")],
        "Can't use text to speech",
        MAC | LIN,
    ),
    (
        &[Permission("fontSettings")],
        "Can't change font settings",
        MAC | LIN,
    ),
    (
        &[Permission("search")],
        "Can't search with the browser's search engine",
        MAC | LIN,
    ),
    (
        &[Permission("pageCapture")],
        "Can't save pages as MHTML",
        MAC | LIN,
    ),
    (
        &[Permission("desktopCapture")],
        "Can't capture your screen",
        MAC | LIN,
    ),
    (
        &[Permission("geolocation")],
        "Can't use your location",
        MAC | LIN,
    ),
    (
        &[Permission("gcm")],
        "Can't receive push messages",
        MAC | LIN,
    ),
    (
        &[Permission("management")],
        "Can't see or manage other extensions",
        MAC | LIN,
    ),
    (
        &[Permission("privacy")],
        "Can only change password and autofill privacy settings",
        MAC,
    ),
    (
        &[Permission("privacy")],
        "Can't change privacy settings",
        LIN,
    ),
    (
        &[
            Permission("declarativeNetRequest"),
            Permission("declarativeNetRequestWithHostAccess"),
        ],
        "Can't block, redirect or change requests with rules",
        LIN,
    ),
    (
        &[Permission("webRequest")],
        "Can't watch the requests pages make",
        LIN,
    ),
    (
        &[Permission("webRequestAuthProvider")],
        "Can't answer websites' sign-in prompts",
        LIN,
    ),
    (
        &[Permission("contextMenus"), Permission("menus")],
        "Can't add items to the right-click menu",
        LIN,
    ),
    (
        &[Permission("notifications")],
        "Can't show notifications",
        LIN,
    ),
    (
        &[Permission("cookies")],
        "Can't read or change cookies",
        LIN,
    ),
    (&[Permission("downloads")], "Can't manage downloads", LIN),
    (
        &[Permission("identity")],
        "Can't sign in to services through the browser",
        LIN,
    ),
    (
        &[Key("commands")],
        "Its keyboard shortcuts aren't available yet",
        LIN,
    ),
    (
        &[Key("externally_connectable")],
        "Websites can't message it directly",
        LIN,
    ),
    (
        &[Key("oauth2")],
        "Can't sign in with a Google account (identity.getAuthToken)",
        MAC | LIN,
    ),
    (
        &[Permission("enterprise.")],
        "Enterprise features aren't available",
        MAC | LIN,
    ),
    (
        &[Permission("system.")],
        "Can't read system information",
        MAC | LIN,
    ),
];

/// The limitations of running `manifest` on `platform`. `permissions` are
/// the API permissions it asks for up front.
pub fn limitations(manifest: &Manifest, permissions: &[String], platform: Platform) -> Vec<String> {
    let bit = match platform {
        Platform::Macos => MAC,
        Platform::Windows => WIN,
        Platform::Linux => LIN,
    };
    GAPS.iter()
        .filter(|(uses, _, platforms)| {
            platforms & bit != 0
                && uses.iter().any(|used| match used {
                    Permission(name) => permissions.iter().any(|permission| {
                        permission == name || (name.ends_with('.') && permission.starts_with(name))
                    }),
                    Key(path) => declares(manifest.raw(), path),
                })
        })
        .map(|(_, text, _)| (*text).to_owned())
        .collect()
}

fn declares(raw: &Value, path: &str) -> bool {
    let mut value = raw;
    for part in path.split('.') {
        match value.get(part) {
            Some(next) => value = next,
            None => return false,
        }
    }
    !value.is_null()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn assess(raw: Value, platform: Platform) -> Vec<String> {
        let manifest = Manifest::from_value(raw);
        limitations(&manifest, &manifest.permissions(), platform)
    }

    #[test]
    fn a_content_blocker_on_declarative_rules_is_unaffected() {
        let raw = json!({
            "manifest_version": 3,
            "permissions": ["declarativeNetRequest", "storage", "tabs", "scripting", "alarms"],
            "host_permissions": ["<all_urls>"],
            "background": {"service_worker": "sw.js"}
        });
        assert!(assess(raw.clone(), Platform::Macos).is_empty());
        assert!(assess(raw, Platform::Windows).is_empty());
    }

    #[test]
    fn a_password_manager_keeps_its_sign_in_answers_and_password_settings() {
        let raw = json!({
            "manifest_version": 3,
            "permissions": ["storage", "privacy", "webRequest", "webRequestAuthProvider",
                "nativeMessaging", "clipboardWrite", "notifications"],
        });
        assert_eq!(
            assess(raw.clone(), Platform::Macos),
            ["Can only change password and autofill privacy settings"]
        );
        assert_eq!(
            assess(raw, Platform::Windows),
            ["Can't talk to apps on your computer (native messaging)"]
        );
    }

    #[test]
    fn a_side_panel_is_missing_on_both_however_it_is_declared() {
        let both = json!({
            "manifest_version": 3,
            "permissions": ["sidePanel"],
            "side_panel": {"default_path": "panel.html"}
        });
        let key_only = json!({"manifest_version": 3, "side_panel": {"default_path": "p.html"}});
        for platform in [Platform::Macos, Platform::Windows] {
            assert_eq!(
                assess(both.clone(), platform),
                ["Can't show its side panel"]
            );
            assert_eq!(
                assess(key_only.clone(), platform),
                ["Can't show its side panel"]
            );
        }
    }

    #[test]
    fn optional_permissions_are_left_to_the_extension() {
        let raw = json!({
            "manifest_version": 3,
            "permissions": ["storage"],
            "optional_permissions": ["history", "downloads", "bookmarks"]
        });
        assert!(assess(raw, Platform::Macos).is_empty());
    }

    #[test]
    fn declared_pages_and_old_style_blocking_are_named() {
        let raw = json!({
            "manifest_version": 2,
            "permissions": ["webRequest", "webRequestBlocking", "tabGroups", "system.cpu", "<all_urls>"],
            "devtools_page": "devtools.html",
            "chrome_url_overrides": {"newtab": "tab.html"}
        });
        assert_eq!(
            assess(raw.clone(), Platform::Macos),
            [
                "Developer tools panels aren't available",
                "Can't replace the new tab page",
                "Can't block requests the old way (webRequestBlocking)",
                "Can't manage tab groups",
                "Can't read system information",
            ]
        );
        assert_eq!(
            assess(raw, Platform::Windows),
            [
                "Developer tools panels aren't available",
                "Can't replace the new tab page"
            ]
        );
    }

    #[test]
    fn browser_data_the_browser_keeps_is_not_named() {
        let raw = json!({"manifest_version": 3, "permissions": ["history", "bookmarks", "topSites", "sessions", "downloads"]});
        assert!(assess(raw, Platform::Macos).is_empty());
    }

    #[test]
    fn google_sign_in_is_flagged_by_its_manifest_key() {
        let raw = json!({"manifest_version": 3, "permissions": ["identity"], "oauth2": {"client_id": "x"}});
        assert_eq!(
            assess(raw.clone(), Platform::Macos),
            ["Can't sign in with a Google account (identity.getAuthToken)"]
        );
        assert!(assess(json!({"permissions": ["identity"]}), Platform::Macos).is_empty());
        assert!(assess(raw, Platform::Windows).is_empty());
    }

    #[test]
    fn linux_names_its_own_gaps_and_no_others() {
        let raw = json!({
            "manifest_version": 3,
            "permissions": ["declarativeNetRequest", "storage", "tabs", "scripting", "notifications", "privacy"],
            "host_permissions": ["<all_urls>"],
        });
        assert_eq!(
            assess(raw.clone(), Platform::Linux),
            [
                "Can't change privacy settings",
                "Can't block, redirect or change requests with rules",
                "Can't show notifications",
            ]
        );
        assert_eq!(
            assess(raw, Platform::Macos),
            ["Can only change password and autofill privacy settings"]
        );
        let reader = json!({"manifest_version": 3, "permissions": ["storage", "alarms", "scripting", "fontSettings"]});
        assert_eq!(
            assess(reader, Platform::Linux),
            ["Can't change font settings"]
        );
    }

    #[test]
    fn explicit_permissions_replace_the_manifests() {
        let manifest = Manifest::from_value(json!({"permissions": ["nativeMessaging"]}));
        assert!(limitations(&manifest, &[], Platform::Windows).is_empty());
    }
}
