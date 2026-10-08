//! The browser window follows a page that fills it in fullscreen, and gives
//! back exactly the window the person had: one they had made fullscreen
//! themselves stays fullscreen afterwards.

use std::sync::Mutex;

use tauri::{AppHandle, Manager};

/// Whether the window was already fullscreen when the page took it, while a
/// page holds it.
static TAKEN: Mutex<Option<bool>> = Mutex::new(None);

pub(crate) fn follow(app: &AppHandle, active: bool) {
    let Some(window) = app.get_webview_window(crate::MAIN_LABEL) else {
        return;
    };
    // Window state is read and changed on the UI thread, in order, so a quick
    // enter/leave pair cannot interleave.
    let _ = app.run_on_main_thread(move || {
        let mut taken = TAKEN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active {
            if taken.is_some() {
                return;
            }
            let already = window.is_fullscreen().unwrap_or(false);
            *taken = Some(already);
            #[cfg(target_os = "windows")]
            crate::platform::imp::set_caption_suppressed(&window, true);
            if !already && window.set_fullscreen(true).is_err() {
                crate::write_diagnostic(format_args!(
                    "fullscreen: the window could not follow its page"
                ));
            }
        } else if let Some(already) = taken.take() {
            if !already && window.set_fullscreen(false).is_err() {
                crate::write_diagnostic(format_args!(
                    "fullscreen: the window could not leave fullscreen"
                ));
            }
            #[cfg(target_os = "windows")]
            crate::platform::imp::set_caption_suppressed(&window, false);
        }
    });
}
