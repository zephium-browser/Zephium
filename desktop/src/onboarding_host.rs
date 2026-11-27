//! Windows serves onboarding from a host of its own. The browser that replaces
//! it in the same window is then a cross-site navigation, for which WebView2
//! starts a new renderer process instead of the one onboarding grew. That is
//! Chromium's choice, seen in every measured run but not guaranteed, and only
//! memory depends on it; there is no API to ask WebView2 for a fresh renderer.
//!
//! The host is the bundled frame, narrowed to onboarding: its own document and
//! the bundle's non-page files, which the main window already reads from its
//! app origin, never another page, only for the main window, and
//! only until onboarding hands over. It is a custom protocol, which Tauri
//! treats as a local origin, so onboarding keeps exactly the main window's
//! capabilities.

use std::borrow::Cow;

use tauri::http::{header, HeaderValue, Method, Request, Response};

pub(crate) const SCHEME: &str = "zephium-onboarding";
pub(crate) const URL: &str = "http://zephium-onboarding.localhost/onboarding.html";
const DOCUMENT: &str = "/onboarding.html";

pub(crate) fn serve(
    ctx: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    use tauri::Manager;
    let handed_over = ctx
        .app_handle()
        .try_state::<super::UiStartupGate>()
        .is_some_and(|gate| gate.handed_over.load(std::sync::atomic::Ordering::Acquire));
    let path = request.uri().path();
    if let Some(status) = refusal(ctx.webview_label(), request.method(), path, handed_over) {
        return responder.respond(refused(status));
    }
    let app = ctx.app_handle().clone();
    let path = path.to_owned();
    tauri::async_runtime::spawn_blocking(move || responder.respond(resolve(&app, &path)));
}

/// Why a request is refused, if it is. `label` is the requesting webview's:
/// Tauri gives each webview its own handler and passes that webview's label,
/// so nothing a request carries can choose it. Once onboarding has handed
/// over, nothing on this host is needed and nothing is served.
fn refusal(label: &str, method: &Method, path: &str, handed_over: bool) -> Option<u16> {
    if label != super::MAIN_LABEL || method != Method::GET {
        return Some(403);
    }
    if handed_over || !admitted(path) {
        return Some(404);
    }
    None
}

/// Onboarding's document, or a file that is not a page at all.
fn admitted(path: &str) -> bool {
    path == DOCUMENT || (path.starts_with('/') && !path.ends_with('/') && !path.ends_with(".html"))
}

fn resolve(app: &tauri::AppHandle, path: &str) -> Response<Cow<'static, [u8]>> {
    if !admitted(path) {
        return refused(404);
    }
    let Some(asset) = app.asset_resolver().get(path.to_owned()) else {
        return refused(404);
    };
    if path != DOCUMENT && asset.mime_type.starts_with("text/html") {
        return refused(404);
    }
    let mut response = Response::new(Cow::Owned(asset.bytes));
    let headers = response.headers_mut();
    if let Ok(mime) = HeaderValue::from_str(&asset.mime_type) {
        headers.insert(header::CONTENT_TYPE, mime);
    }
    if let Some(csp) = asset
        .csp_header
        .as_deref()
        .and_then(|csp| HeaderValue::from_str(csp).ok())
    {
        headers.insert(header::CONTENT_SECURITY_POLICY, csp);
    }
    super::harden_privileged_headers(headers);
    if path == DOCUMENT {
        // Onboarding is left for good: never kept in the back-forward cache.
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn refused(status: u16) -> Response<Cow<'static, [u8]>> {
    let mut response = Response::new(Cow::Borrowed(&[][..]));
    *response.status_mut() =
        tauri::http::StatusCode::from_u16(status).unwrap_or(tauri::http::StatusCode::NOT_FOUND);
    super::harden_privileged_headers(response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_onboarding_and_the_files_it_loads_are_served() {
        for path in [
            DOCUMENT,
            "/assets/onboarding-BEEq6a-I.js",
            "/assets/ui-core.css",
            "/zephium-icon.png",
        ] {
            assert!(admitted(path), "{path}");
        }
        for path in [
            "/",
            "",
            "/browser.html",
            "/panel.html",
            "/index.html",
            "/onboarding.html/",
            "assets/onboarding.js",
        ] {
            assert!(!admitted(path), "{path}");
        }
    }

    #[test]
    fn only_the_main_window_reaches_onboarding_and_only_until_handover() {
        let get = Method::GET;
        assert_eq!(
            refusal(super::super::MAIN_LABEL, &get, DOCUMENT, false),
            None
        );
        assert_eq!(
            refusal(super::super::MAIN_LABEL, &get, "/assets/a.js", false),
            None
        );
        assert_eq!(refusal("panel", &get, DOCUMENT, false), Some(403));
        assert_eq!(refusal("", &get, DOCUMENT, false), Some(403));
        assert_eq!(
            refusal(super::super::MAIN_LABEL, &Method::POST, DOCUMENT, false),
            Some(403)
        );
        assert_eq!(
            refusal(super::super::MAIN_LABEL, &get, "/browser.html", false),
            Some(404)
        );
        for path in [DOCUMENT, "/assets/a.js"] {
            assert_eq!(
                refusal(super::super::MAIN_LABEL, &get, path, true),
                Some(404),
                "{path}"
            );
        }
    }

    #[test]
    fn the_onboarding_url_is_its_own_site() {
        let url = tauri::Url::parse(URL).unwrap();
        assert_eq!(url.scheme(), "http");
        assert_eq!(url.host_str(), Some(&*format!("{SCHEME}.localhost")));
        assert_eq!(url.path(), DOCUMENT);
        assert!(!super::super::ui_navigation_allowed(&url));
    }
}
