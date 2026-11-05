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
    // An update being swapped in holds the app open, as it holds a quit.
    if shutdown_started(&app) || resource_close::is_closing() || updates::blocks_exit(&app) {
        return false;
    }
    let exit_app = app.clone();
    resource_close::request_with_result(app, move |saved| {
        // Unsaved work the person kept open cancels the restart, as it
        // cancels a quit.
        if !saved || shutdown_started(&exit_app) || updates::blocks_exit(&exit_app) {
            resource_close::cancel_prepared(&exit_app);
            return;
        }
        AFTER_EXIT.store(true, Ordering::Release);
        exit_app.exit(0);
    });
    true
}

/// An exit the app refused is no restart; a later quit must not become one.
pub(crate) fn exit_refused() {
    AFTER_EXIT.store(false, Ordering::Release);
}

/// Called once the event loop has finished.
#[cfg(not(target_os = "windows"))]
pub(crate) fn finish_on_exit(app: &tauri::AppHandle) {
    if AFTER_EXIT.swap(false, Ordering::AcqRel) && closed_cleanly(app) {
        tauri::process::restart(&app.env());
    }
}

/// On Windows the successor starts only after the WebView2 runtime is proven
/// gone and its private data cleaned up, which happens after the event loop.
#[cfg(target_os = "windows")]
pub(crate) fn finish_after_cleanup(app: &tauri::AppHandle, cleanup_succeeded: bool) {
    if AFTER_EXIT.swap(false, Ordering::AcqRel) && cleanup_succeeded && closed_cleanly(app) {
        tauri::process::restart(&app.env());
    }
}

fn closed_cleanly(app: &tauri::AppHandle) -> bool {
    app.try_state::<ShutdownCoordinator>()
        .is_some_and(|shutdown| {
            shutdown.authorized_exit_code.load(Ordering::Acquire) == 0
                && !shutdown.terminal_failure.load(Ordering::Acquire)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_exit_leaves_no_restart_for_a_later_quit() {
        AFTER_EXIT.store(true, Ordering::Release);
        exit_refused();
        assert!(!AFTER_EXIT.load(Ordering::Acquire));
    }

    #[test]
    fn a_restart_waits_out_an_update_swap_and_on_windows_the_cleanup() {
        let relaunch = include_str!("relaunch.rs");
        let admission = relaunch.split("AFTER_EXIT.store(true").next().unwrap();
        assert_eq!(admission.matches("updates::blocks_exit(").count(), 2);
        let lib = include_str!("lib.rs");
        let refused = lib.split("relaunch::exit_refused()").next().unwrap();
        assert!(refused.trim_end().ends_with("api.prevent_exit();"));
        let windows = lib.split("let cleanup_succeeded =").nth(1).unwrap();
        assert!(windows.contains("relaunch::finish_after_cleanup(&handle, cleanup_succeeded"));
        let exit_hook = lib.split("relaunch::finish_on_exit(app)").next().unwrap();
        assert!(exit_hook.trim_end().ends_with(
            "#[cfg(not(target_os = \"windows\"))]\n    if matches!(&event, tauri::RunEvent::Exit) {"
        ));
    }
}
