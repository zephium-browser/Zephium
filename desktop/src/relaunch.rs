//! Restarting Zephium when the person asks: the orderly close saves every
//! profile and retires the engine as a quit does, and the final exit starts
//! the app again only when that close was clean.

use super::*;

static AFTER_EXIT: AtomicBool = AtomicBool::new(false);

/// Closes Zephium as a quit does, then starts it again.
#[tauri::command]
#[specta::specta]
pub(crate) fn app_relaunch(caller: WebviewWindow, app: tauri::AppHandle) -> bool {
    if !authorize(&caller, CallerPolicy::Main, "app_relaunch") {
        return false;
    }
    if shutdown_started(&app) || resource_close::is_closing() {
        return false;
    }
    let exit_app = app.clone();
    resource_close::request_with_result(app, move |saved| {
        // Unsaved work the person kept open cancels the restart, as it
        // cancels a quit.
        if !saved || shutdown_started(&exit_app) {
            resource_close::cancel_prepared(&exit_app);
            return;
        }
        AFTER_EXIT.store(true, Ordering::Release);
        exit_app.exit(0);
    });
    true
}

/// Called once the event loop has finished.
pub(crate) fn finish_on_exit(app: &tauri::AppHandle) {
    if !AFTER_EXIT.swap(false, Ordering::AcqRel) {
        return;
    }
    let clean = app
        .try_state::<ShutdownCoordinator>()
        .is_some_and(|shutdown| {
            shutdown.authorized_exit_code.load(Ordering::Acquire) == 0
                && !shutdown.terminal_failure.load(Ordering::Acquire)
        });
    if clean {
        tauri::process::restart(&app.env());
    }
}
