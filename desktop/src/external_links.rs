//! Links other applications hand to Zephium: a clicked link once Zephium is
//! the default browser, or addresses on the command line of a launch. Core
//! admits every address before the shell sees it.

use super::*;
use std::sync::Mutex;
use zephium_core::navigation::{external_document, MAX_EXTERNAL_TARGETS};

/// Hand-offs that arrive before the shell exists. macOS delivers a link that
/// launched the app through the run loop, which can precede setup.
static WAITING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Admits `arguments` and opens them. `forward` brings the window to the
/// front, which a running instance wants and a launch leaves to its own
/// startup reveal.
pub(crate) fn hand_off(
    app: &tauri::AppHandle,
    arguments: impl IntoIterator<Item = String>,
    forward: bool,
) {
    let urls: Vec<String> = arguments
        .into_iter()
        .filter_map(|argument| external_document(&argument))
        .map(|url| url.to_string())
        .take(MAX_EXTERNAL_TARGETS)
        .collect();
    if urls.is_empty() || shutdown_started(app) {
        return;
    }
    let Some(shell) = app.try_state::<Handle>() else {
        let mut waiting = WAITING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let room = MAX_EXTERNAL_TARGETS.saturating_sub(waiting.len());
        waiting.extend(urls.into_iter().take(room));
        return;
    };
    if shell.dispatch(Command::OpenExternal(urls)) && forward {
        bring_forward(app);
    }
}

/// Runs once the shell is managed: what arrived early, then this launch's own
/// command line (Windows and Linux pass a clicked link as an argument).
pub(crate) fn adopt(app: &tauri::AppHandle) {
    let waiting = std::mem::take(
        &mut *WAITING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    hand_off(
        app,
        waiting.into_iter().chain(std::env::args().skip(1)),
        false,
    );
}

pub(crate) fn bring_forward(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
