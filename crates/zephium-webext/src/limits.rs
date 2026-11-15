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
    /// A boolean capability on an entry in a manifest array.
    ArrayFlag(&'static str, &'static str),
    /// An exact string capability in a manifest array.
    ArrayValue(&'static str, &'static str),
}
use Uses::{ArrayFlag, ArrayValue, Key, Permission};

/// What each platform's runtime lacks today, in the order shown. A platform
/// is left out of a row's mask when it supports the feature or has not been
/// verified; unverified features stay silent rather than guess.
const GAPS: &[(&[Uses], &str, u8)] = &[
    (
        &[Key("cross_origin_embedder_policy")],
        "SharedArrayBuffer isn't exposed in extension pages, even when cross-origin isolation is enabled",
        LIN,
    ),
    (
        &[ArrayFlag("content_scripts", "match_origin_as_fallback")],
        "Content scripts can't use origin fallback in opaque data documents because their initiating origin is unavailable",
        LIN,
    ),
    (
        &[ArrayFlag("web_accessible_resources", "use_dynamic_url")],
        "Dynamic public resource URLs aren't returned by runtime.getURL, and rule redirects to those resources aren't available",
        LIN,
    ),
    (
        &[Permission("offscreen")],
        "Offscreen clipboard access requires both clipboardRead and clipboardWrite; separate read-only or write-only access isn't available",
        LIN,
    ),
    (
        &[Permission("offscreen")],
        "Offscreen audio documents don't close automatically after silence",
        LIN,
    ),
    (
        &[Permission("sessions")],
        "Can list closed tabs but can't restore sessions",
        LIN,
    ),
    (
        &[Permission("history"), Permission("bookmarks"), Permission("sessions")],
        "History, bookmark and session change events aren't delivered",
        LIN,
    ),
    (
        // Linux stores options; presentation, action-click opening and layout are unavailable.
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
        "Password saving and address/card autofill report inactive and not controllable; privacy mutations are unsupported",
        LIN,
    ),
    (
        &[
            Permission("declarativeNetRequest"),
            Permission("declarativeNetRequestWithHostAccess"),
        ],
        "Can't change response headers with rules",
        LIN,
    ),
    (
        &[
            Permission("declarativeNetRequest"),
            Permission("declarativeNetRequestWithHostAccess"),
        ],
        "Rules don't apply to WebSockets or service-worker requests",
        LIN,
    ),
    (
        &[
            Permission("declarativeNetRequest"),
            Permission("declarativeNetRequestWithHostAccess"),
        ],
        "Rules tell some request types apart less precisely on non-HTTPS sites",
        LIN,
    ),
    (
        &[Permission("webRequest")],
        "UI request observations use frameId 0 only for main resources, otherwise frameId/parentFrameId -1 and type other; unknown initiators are omitted; request and top-level host access are required",
        LIN,
    ),
    (
        &[Permission("tabs")],
        "Can read browser windows but can't create, resize, focus or close them through the extension API",
        LIN,
    ),
    (
        &[Permission("tabs")],
        "Can't detect the language of a tab",
        LIN,
    ),
    (
        &[Permission("activeTab"), ArrayValue("host_permissions", "<all_urls>")],
        "Can't capture visible tabs; document-bound full-resolution capture and requested image formats aren't available",
        LIN,
    ),
    (
        &[Permission("webRequest")],
        "Request observation is nonblocking, with WebKit-provided headers and generic net::ERR_FAILED errors; it omits workers, WebSockets, service-worker fetches, authentication, unreported redirects and requests outside initially matching filters",
        LIN,
    ),
    (
        &[Permission("webRequest")],
        "Handler changes clear profile memory, back-forward and preflight caches immediately and wait for completion, instead of deferring memory-cache clearing until navigation; at most 128 clears can wait",
        LIN,
    ),
    (
        &[Permission("webRequestAuthProvider")],
        "Can't answer websites' sign-in prompts",
        LIN,
    ),
    (
        // Menus survive worker restarts; full browser-session restoration is not implemented.
        &[Permission("contextMenus"), Permission("menus")],
        "Right-click menus lack frame details, selected text and browser-session restoration",
        LIN,
    ),
    (
        &[Permission("contextMenus"), Permission("menus")],
        "Right-click menus require explicit IDs and onClicked listeners",
        LIN,
    ),
    (
        &[Permission("notifications")],
        "Notifications need a running desktop notification service; only basic/list templates and packaged raster main icons are supported (no auxiliary icons)",
        LIN,
    ),
    (
        &[Permission("scripting")],
        "CSS can't target non-main frame/document IDs; all-frame CSS excludes inherited-origin documents and requires host grants covering every port (not activeTab-only grants)",
        LIN,
    ),
    (
        &[Key("omnibox")],
        "Can't add address-bar commands or suggestions",
        LIN,
    ),
    (
        &[Permission("tabCapture")],
        "Can't capture tab audio or video",
        LIN,
    ),
    (
        &[Permission("declarativeContent")],
        "Can't evaluate declarative page-content rules",
        LIN,
    ),
    (
        &[Key("background.service_worker")],
        "Background workers lack service-worker registrations, push, extension fetch interception and static routes",
        LIN,
    ),
    (
        &[Key("action"), Key("browser_action")],
        "Action ImageData is limited to 256 pixels per dimension and 16 representations; packaged icons to 1 MiB and 1024 pixels per dimension",
        LIN,
    ),
    (
        &[Key("content_scripts"), Permission("scripting")],
        "Injected JavaScript files are limited to 2 MiB; code and translation data share an 8 MiB document budget; each content match-pattern set is limited to 128 patterns",
        LIN,
    ),
    (
        &[Key("default_locale")],
        "Translation catalogs are limited to 8 MiB and 16384 messages",
        LIN,
    ),
    (&[Key("incognito")], "Can't run in private windows", LIN),
    (
        &[Permission("cookies")],
        "Can't read or change cookies or report cookie changes",
        LIN,
    ),
    (&[Permission("downloads")], "Can't manage downloads", LIN),
    (
        &[Permission("identity")],
        "Can't sign in to services through the browser",
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
                    ArrayValue(path, value) => manifest
                        .raw()
                        .get(path)
                        .and_then(Value::as_array)
                        .is_some_and(|entries| {
                            entries.iter().any(|entry| entry.as_str() == Some(value))
                        }),
                    ArrayFlag(path, flag) => manifest
                        .raw()
                        .get(path)
                        .and_then(Value::as_array)
                        .is_some_and(|entries| {
                            entries
                                .iter()
                                .any(|entry| entry.get(flag).and_then(Value::as_bool) == Some(true))
                        }),
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
    fn capture_warning_requires_a_declared_capture_capability() {
        let partial = json!({"manifest_version":3,"host_permissions":["https://example.test/*"]});
        assert!(assess(partial, Platform::Linux).is_empty());
        for raw in [
            json!({"manifest_version":3,"host_permissions":["<all_urls>"]}),
            json!({"manifest_version":3,"permissions":["activeTab"]}),
        ] {
            assert_eq!(assess(raw.clone(), Platform::Linux), ["Can't capture visible tabs; document-bound full-resolution capture and requested image formats aren't available"]);
            assert!(assess(raw.clone(), Platform::Macos).is_empty());
            assert!(assess(raw, Platform::Windows).is_empty());
        }
    }

    #[test]
    fn dynamic_resource_warning_requires_a_declared_dynamic_entry() {
        let dynamic = json!({"manifest_version":3,"web_accessible_resources":[{"resources":["public.js"],"matches":["https://example.test/*"],"use_dynamic_url":true}]});
        assert_eq!(assess(dynamic.clone(), Platform::Linux), ["Dynamic public resource URLs aren't returned by runtime.getURL, and rule redirects to those resources aren't available"]);
        assert!(assess(dynamic.clone(), Platform::Macos).is_empty());
        assert!(assess(dynamic.clone(), Platform::Windows).is_empty());
        let mut fixed = dynamic;
        fixed["web_accessible_resources"][0]["use_dynamic_url"] = false.into();
        assert!(assess(fixed, Platform::Linux).is_empty());
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
    fn window_mutation_limitation_is_linux_only() {
        let raw = json!({"manifest_version":3,"permissions":["tabs"]});
        assert_eq!(
            assess(raw.clone(), Platform::Linux),
            ["Can read browser windows but can't create, resize, focus or close them through the extension API", "Can't detect the language of a tab"]
        );
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
    fn linux_names_session_restore_and_data_event_limits() {
        assert_eq!(
            assess(
                json!({"manifest_version": 3, "permissions": ["sessions", "history", "bookmarks"]}),
                Platform::Linux
            ),
            [
                "Can list closed tabs but can't restore sessions",
                "History, bookmark and session change events aren't delivered",
            ]
        );
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
                "Password saving and address/card autofill report inactive and not controllable; privacy mutations are unsupported",
                "Can't change response headers with rules",
                "Rules don't apply to WebSockets or service-worker requests",
                "Rules tell some request types apart less precisely on non-HTTPS sites",
                "Can read browser windows but can't create, resize, focus or close them through the extension API",
                "Can't detect the language of a tab",
                "Can't capture visible tabs; document-bound full-resolution capture and requested image formats aren't available",
                "Notifications need a running desktop notification service; only basic/list templates and packaged raster main icons are supported (no auxiliary icons)",
                "CSS can't target non-main frame/document IDs; all-frame CSS excludes inherited-origin documents and requires host grants covering every port (not activeTab-only grants)",
                "Injected JavaScript files are limited to 2 MiB; code and translation data share an 8 MiB document budget; each content match-pattern set is limited to 128 patterns",
            ]
        );
        assert_eq!(
            assess(raw, Platform::Macos),
            ["Can only change password and autofill privacy settings"]
        );
        let reader = json!({"manifest_version": 3, "permissions": ["storage", "alarms", "scripting", "fontSettings"]});
        assert_eq!(
            assess(reader, Platform::Linux),
            [
                "Can't change font settings",
                "CSS can't target non-main frame/document IDs; all-frame CSS excludes inherited-origin documents and requires host grants covering every port (not activeTab-only grants)",
                "Injected JavaScript files are limited to 2 MiB; code and translation data share an 8 MiB document budget; each content match-pattern set is limited to 128 patterns"
            ]
        );
    }

    #[test]
    fn explicit_permissions_replace_the_manifests() {
        let manifest = Manifest::from_value(json!({"permissions": ["nativeMessaging"]}));
        assert!(limitations(&manifest, &[], Platform::Windows).is_empty());
    }
}
