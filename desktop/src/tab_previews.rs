use tauri::Manager;
use zephium_core::ids::{ItemId, WindowId};

pub(crate) const SCHEME: &str = "zephium-preview";

fn request_key(
    label: &str,
    method: &tauri::http::Method,
    path: &str,
    query: Option<&str>,
) -> Option<(WindowId, ItemId, u64)> {
    if label != super::overlay::PANEL_LABEL
        || *method != tauri::http::Method::GET
        || path.len() > 128
    {
        return None;
    }
    let (window, tab) = path.strip_prefix('/')?.split_once('/')?;
    let window_id = window.parse::<WindowId>().ok()?;
    let tab_id = ItemId::parse(tab)?;
    let query = query.filter(|value| value.len() <= 22)?;
    let version = query.strip_prefix("v=")?;
    let revision = version.parse::<u64>().ok()?;
    (window_id.to_string() == window
        && tab_id.to_string() == tab
        && revision.to_string() == version)
        .then_some((window_id, tab_id, revision))
}

fn response(bytes: Option<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(if bytes.is_some() { 200 } else { 404 })
        .header("Content-Type", "image/jpeg")
        .header("Cache-Control", "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Content-Security-Policy", "default-src 'none'; sandbox")
        .body(bytes.unwrap_or_default())
        .unwrap_or_default()
}

pub(crate) fn serve(
    context: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    let Some((window, tab, revision)) = request_key(
        context.webview_label(),
        request.method(),
        request.uri().path(),
        request.uri().query(),
    ) else {
        responder.respond(response(None));
        return;
    };
    let Some(shell) = context.app_handle().try_state::<zephium_app::Handle>() else {
        responder.respond(response(None));
        return;
    };
    let (done, reply) = std::sync::mpsc::sync_channel(1);
    if !shell.dispatch(zephium_app::Command::ReadTabPreview {
        window,
        tab,
        revision,
        done,
    }) {
        responder.respond(response(None));
        return;
    }
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(response(
            reply
                .recv_timeout(std::time::Duration::from_secs(2))
                .ok()
                .flatten(),
        ));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_panel_gets_canonical_preview_keys() {
        let window = 1_u64;
        let tab = ItemId::from(2);
        let path = format!("/{window}/{tab}");
        assert_eq!(
            request_key("panel", &tauri::http::Method::GET, &path, Some("v=3")),
            Some((window, tab, 3))
        );
        for label in ["main", "content", "panel-other", ""] {
            assert!(request_key(label, &tauri::http::Method::GET, &path, Some("v=3")).is_none());
        }
        for query in [None, Some("v=03"), Some("v=3&x=1"), Some("v=-1")] {
            assert!(request_key("panel", &tauri::http::Method::GET, &path, query).is_none());
        }
        assert!(request_key("panel", &tauri::http::Method::POST, &path, Some("v=3")).is_none());
        assert!(request_key("panel", &tauri::http::Method::GET, "/../2", Some("v=3")).is_none());
        assert_eq!(response(None).status(), 404);
        assert_eq!(
            response(Some(vec![1])).headers()["Cache-Control"],
            "no-store"
        );
    }
}
