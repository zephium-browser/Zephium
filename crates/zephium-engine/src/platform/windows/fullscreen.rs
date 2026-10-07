//! WebView2 element fullscreen for ordinary tabs. WebView2 only fills its own
//! controller bounds; the browser makes its window fullscreen and lays this
//! one view over the whole of it, and restores both itself on exit.
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, ContainsFullScreenElementChangedEventHandler,
    ExecuteScriptCompletedHandler,
};
use windows_core::{BOOL, HSTRING};
use wry::WebViewExtWindows;

use crate::fullscreen::NativeFullscreen;

// Element fullscreen first, then a legacy video presentation.
const EXIT_SCRIPT: &str = "(async () => { if (document.fullscreenElement) { \
     await document.exitFullscreen(); } \
     else if (document.webkitFullscreenElement) { document.webkitExitFullscreen(); } \
     return !document.fullscreenElement && !document.webkitFullscreenElement; })()";
const EXIT_WORLD: &str = "zephium-fullscreen";

pub struct FullscreenObserver {
    core: ICoreWebView2,
    token: i64,
}

impl Drop for FullscreenObserver {
    fn drop(&mut self) {
        // SAFETY: the exact registration on the STA-owned core.
        let _ = unsafe {
            self.core
                .remove_ContainsFullScreenElementChanged(self.token)
        };
    }
}

pub fn install_fullscreen_observer(
    view: &wry::WebView,
    changed: impl Fn() + 'static,
) -> windows_core::Result<FullscreenObserver> {
    let core = view.webview();
    let handler = ContainsFullScreenElementChangedEventHandler::create(Box::new(move |_, _| {
        changed();
        Ok(())
    }));
    let mut token = 0_i64;
    // SAFETY: STA-owned core; the handler captures neither the core nor the
    // WebView and is removed before the controller drops.
    unsafe { core.add_ContainsFullScreenElementChanged(&handler, &mut token)? };
    Ok(FullscreenObserver { core, token })
}

fn contains_fullscreen_element(core: &ICoreWebView2) -> bool {
    let mut contains = BOOL(0);
    // SAFETY: STA-owned core and an initialized out value.
    unsafe { core.ContainsFullScreenElement(&mut contains) }.is_ok() && contains.as_bool()
}

pub(crate) fn fullscreen_state(view: &wry::WebView) -> NativeFullscreen {
    if contains_fullscreen_element(&view.webview()) {
        NativeFullscreen::Active
    } else {
        NativeFullscreen::Inactive
    }
}

pub(crate) fn exit_fullscreen(view: &wry::WebView) {
    let _ = exit_core(&view.webview());
}

/// Asks the page to leave fullscreen from an isolated world, so the page
/// cannot replace `exitFullscreen` to keep the screen; the page's own world
/// is the fallback when the protocol is unavailable. False when the page was
/// not fullscreen.
pub(crate) fn exit_core(core: &ICoreWebView2) -> bool {
    if !contains_fullscreen_element(core) {
        return false;
    }
    let next = core.clone();
    let frame_tree = protocol(core, "Page.getFrameTree", "{}", move |response| {
        let Some(frame) = response.as_deref().and_then(main_frame_id) else {
            return fallback(&next);
        };
        let parameters = serde_json::json!({ "frameId": frame, "worldName": EXIT_WORLD });
        let last = next.clone();
        let world = protocol(
            &next,
            "Page.createIsolatedWorld",
            &parameters.to_string(),
            move |response| {
                let Some(context) = response.as_deref().and_then(execution_context) else {
                    return fallback(&last);
                };
                let parameters = serde_json::json!({
                    "expression": EXIT_SCRIPT,
                    "contextId": context,
                    "awaitPromise": true,
                    "returnByValue": true,
                });
                let failed = last.clone();
                let evaluated = protocol(
                    &last,
                    "Runtime.evaluate",
                    &parameters.to_string(),
                    move |response| {
                        if !response.as_deref().is_some_and(exit_completed) {
                            fallback(&failed);
                        }
                    },
                );
                if !evaluated {
                    fallback(&last);
                }
            },
        );
        if !world {
            fallback(&next);
        }
    });
    if !frame_tree {
        fallback(core);
    }
    true
}

fn fallback(core: &ICoreWebView2) {
    let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
    // SAFETY: a fixed script on the STA-owned core.
    let script = format!("{EXIT_SCRIPT}.catch(() => false)");
    let _ = unsafe { core.ExecuteScript(&HSTRING::from(script), &done) };
}

fn protocol(
    core: &ICoreWebView2,
    method: &str,
    parameters: &str,
    completion: impl FnOnce(Option<String>) + 'static,
) -> bool {
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, response| {
            completion(result.is_ok().then_some(response));
            Ok(())
        }));
    // SAFETY: STA-owned core and immutable argument buffers; COM retains the
    // completion owner until it returns.
    unsafe {
        core.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(parameters),
            &handler,
        )
    }
    .is_ok()
}

fn main_frame_id(response: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(response).ok()?;
    value
        .pointer("/frameTree/frame/id")?
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .map(str::to_owned)
}

fn execution_context(response: &str) -> Option<i64> {
    let value: serde_json::Value = serde_json::from_str(response).ok()?;
    value
        .get("executionContextId")?
        .as_i64()
        .filter(|id| *id > 0)
}

fn exit_completed(response: &str) -> bool {
    // A successful COM call can still carry a JavaScript exception (including
    // a rejected exitFullscreen promise). Only its settled result proves exit.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(response) else {
        return false;
    };
    value.get("exceptionDetails").is_none()
        && value
            .pointer("/result/type")
            .and_then(|value| value.as_str())
            == Some("boolean")
        && value
            .pointer("/result/value")
            .and_then(|value| value.as_bool())
            == Some(true)
}

#[cfg(test)]
mod tests {
    use super::{execution_context, exit_completed, main_frame_id};

    #[test]
    fn protocol_replies_are_read_strictly() {
        assert_eq!(
            main_frame_id(r#"{"frameTree":{"frame":{"id":"A1B2","url":"x"},"childFrames":[]}}"#),
            Some("A1B2".to_owned())
        );
        assert_eq!(main_frame_id(r#"{"frameTree":{"frame":{"id":""}}}"#), None);
        assert_eq!(main_frame_id(r#"{"frameTree":{}}"#), None);
        assert_eq!(main_frame_id("not json"), None);
        assert_eq!(execution_context(r#"{"executionContextId":7}"#), Some(7));
        assert_eq!(execution_context(r#"{"executionContextId":"7"}"#), None);
        assert_eq!(execution_context(r#"{"executionContextId":0}"#), None);
        assert_eq!(execution_context(r#"{"executionContextId":-1}"#), None);
    }

    #[test]
    fn exit_requires_a_settled_success_without_a_script_exception() {
        assert!(exit_completed(
            r#"{"result":{"type":"boolean","value":true}}"#
        ));
        for reply in [
            r#"{"result":{"type":"boolean","value":false}}"#,
            r#"{"result":{"type":"boolean","value":true},"exceptionDetails":{}}"#,
            r#"{"result":{"type":"object","subtype":"error"},"exceptionDetails":{}}"#,
            r#"{"result":{"type":"object","className":"Promise"}}"#,
            r#"{"result":{"type":"boolean","value":"true"}}"#,
            r#"{"result":{"type":"undefined"}}"#,
            "{}",
            "not json",
        ] {
            assert!(!exit_completed(reply), "must fall back for {reply}");
        }
    }
}
