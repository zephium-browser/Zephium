//! QA only: what a benchmark harness runs in pages. Evaluation answers one
//! script; a preload runs at document start in every page loaded afterwards.

use std::sync::mpsc::Sender;
use std::sync::Mutex;

use zephium_core::ids::ItemId;

use super::EngineHost;

/// Accepted preloads, wrapped, for views built after they were accepted.
static PRELOADS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub(super) fn preloads() -> Vec<String> {
    PRELOADS
        .lock()
        .map(|preloads| preloads.clone())
        .unwrap_or_default()
}

/// WebView2 injects document-start scripts into subframes too.
fn main_frame_only(source: &str) -> String {
    format!("if (window.top === window.self) {{\n{source}\n}}")
}

#[cfg(target_os = "macos")]
fn add_to_view(view: &wry::WebView, source: &str) -> bool {
    use objc2::MainThreadOnly as _;
    use objc2_foundation::{MainThreadMarker, NSString};
    use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime};
    use wry::WebViewExtMacOS;
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    // SAFETY: main-thread WebKit calls on a live view; the script is a fixed
    // page-world, main-frame, document-start user script.
    unsafe {
        let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
            WKUserScript::alloc(mtm),
            &NSString::from_str(source),
            WKUserScriptInjectionTime::AtDocumentStart,
            true,
        );
        view.webview()
            .configuration()
            .userContentController()
            .addUserScript(&script);
    }
    true
}

#[cfg(target_os = "windows")]
fn add_to_view(view: &wry::WebView, source: &str) -> bool {
    use webview2_com::AddScriptToExecuteOnDocumentCreatedCompletedHandler;
    use windows_core::HSTRING;
    use wry::WebViewExtWindows;
    let handler = AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(|e, _| e));
    // SAFETY: the view's core stays on its creating STA, where this runs.
    unsafe {
        view.webview()
            .AddScriptToExecuteOnDocumentCreated(&HSTRING::from(source), &handler)
    }
    .is_ok()
}

impl EngineHost {
    /// Answers `None` when the page has no view or gives no string result.
    pub(crate) fn blocker_bench_evaluate(
        &self,
        id: ItemId,
        script: &str,
        reply: Sender<Option<String>>,
    ) {
        let Some(view) = self.views.get(&id) else {
            let _ = reply.send(None);
            return;
        };
        let answer = reply.clone();
        if !crate::platform::cosmetic_pull::evaluate(&view.view, script, move |result| {
            let _ = answer.send(result);
        }) {
            let _ = reply.send(None);
        }
    }

    /// Adds `source` to every open view and, through `preloads`, to every view
    /// built later. Whether every open view took it.
    pub(crate) fn blocker_bench_preload(&self, source: &str) -> bool {
        let source = main_frame_only(source);
        let Ok(mut preloads) = PRELOADS.lock() else {
            return false;
        };
        preloads.push(source.clone());
        drop(preloads);
        let mut all = true;
        for view in self.views.values() {
            all &= add_to_view(&view.view, &source);
        }
        all
    }
}
