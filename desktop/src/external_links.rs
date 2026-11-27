//! Links other applications hand to Zephium: a clicked link once Zephium is
//! the default browser, or addresses on the command line of a launch. Core
//! admits every address before the shell sees it.

use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use zephium_core::navigation::{external_target, MAX_EXTERNAL_TARGETS};

/// Hand-offs that arrive before the shell exists. macOS delivers a link that
/// launched the app through the run loop, which can precede setup.
static WAITING: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A running instance received links and wants the window that shows them
/// in front. The shell says which window that is once it has opened them.
static FORWARD: AtomicBool = AtomicBool::new(false);

/// Admits `arguments` and opens them. `forward` brings the window to the
/// front, which a running instance wants and a launch leaves to its own
/// startup reveal.
pub(crate) fn hand_off(
    app: &tauri::AppHandle,
    arguments: impl IntoIterator<Item = String>,
    forward: bool,
) -> bool {
    let mut arguments: Vec<String> = arguments.into_iter().collect();
    let mini_app_id = take_mini_app_argument(&mut arguments);
    let mini_app_opened = mini_app_id
        .as_deref()
        .is_some_and(|id| super::miniapps::open_from_launch(app, id));
    let urls: Vec<String> = arguments
        .into_iter()
        .filter_map(|argument| external_target(&argument))
        .map(|url| url.to_string())
        .take(MAX_EXTERNAL_TARGETS)
        .collect();
    if urls.is_empty() || shutdown_started(app) {
        return mini_app_opened;
    }
    let Some(shell) = app.try_state::<Handle>() else {
        let mut waiting = WAITING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let room = MAX_EXTERNAL_TARGETS.saturating_sub(waiting.len());
        waiting.extend(urls.into_iter().take(room));
        return mini_app_opened;
    };
    // Each batch says afresh whether it wants the main window forward, so a
    // batch that opened only mini windows leaves nothing behind for the next.
    FORWARD.store(forward, Ordering::Release);
    if !shell.dispatch(Command::OpenExternal(urls)) {
        FORWARD.store(false, Ordering::Release);
    }
    mini_app_opened
}

fn take_mini_app_argument(arguments: &mut Vec<String>) -> Option<String> {
    let index = arguments
        .iter()
        .position(|argument| argument == "--mini-app")?;
    let id = arguments
        .get(index + 1)
        .filter(|id| valid_mini_app_id(id))
        .cloned();
    arguments.remove(index);
    if id.is_some() {
        arguments.remove(index);
    }
    id
}

fn valid_mini_app_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Links opened as tabs: the main window comes forward to show them. A
/// link that opened a mini window leaves the request in place, since that
/// window comes forward on its own and a later link of the same batch may
/// still open as a tab; the shell reports tabs once per batch.
pub(crate) fn forward_pending(app: &tauri::AppHandle) {
    if FORWARD.swap(false, Ordering::AcqRel) {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || bring_forward(&handle));
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
    let _ = hand_off(
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

#[cfg(test)]
mod tests {
    use super::take_mini_app_argument;

    #[test]
    fn mini_app_switch_and_id_are_removed_without_dropping_other_arguments() {
        let mut arguments = vec![
            "--mini-app".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
            "https://example.com".to_owned(),
        ];
        assert_eq!(
            take_mini_app_argument(&mut arguments).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert_eq!(arguments, ["https://example.com"]);
    }

    #[test]
    fn mini_app_switch_without_id_is_removed_safely() {
        let mut arguments = vec!["--mini-app".to_owned()];
        assert_eq!(take_mini_app_argument(&mut arguments), None);
        assert!(arguments.is_empty());
    }

    #[test]
    fn malformed_id_does_not_consume_following_url() {
        let mut arguments = vec!["--mini-app".to_owned(), "https://example.com".to_owned()];
        assert_eq!(take_mini_app_argument(&mut arguments), None);
        assert_eq!(arguments, ["https://example.com"]);
    }

    #[test]
    fn valid_id_is_consumed_even_if_launch_later_fails() {
        let mut arguments = vec![
            "--mini-app".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
        ];
        assert_eq!(
            take_mini_app_argument(&mut arguments).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
        assert!(arguments.is_empty());
    }
}
