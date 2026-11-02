//! Recover the privileged documents independently of ordinary page crashes.
use tauri::AppHandle;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
};
use webview2_com::ProcessFailedEventHandler;

#[cfg(test)]
use crate::renderer_recovery::{Action, Renderers};

pub(super) fn install(
    core: &ICoreWebView2,
    app: AppHandle,
    label: String,
) -> windows::core::Result<()> {
    let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let mut kind = Default::default();
        unsafe { args.ProcessFailedKind(&mut kind)? };
        let browser_exited = kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED;
        if !browser_exited && kind != COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED {
            // A busy machine can repeatedly report unresponsiveness. Do not
            // destroy live drafts for that, or restart for self-healing GPU and
            // utility-process failures.
            return Ok(());
        }
        let app = app.clone();
        let label = label.clone();
        // Tauri executes run_on_main_thread inline on the STA. Hop off it first
        // so navigation never re-enters WebView2 from its ProcessFailed callback.
        tauri::async_runtime::spawn(async move {
            let target = app.clone();
            let _ = app.run_on_main_thread(move || {
                crate::renderer_recovery::failed(&target, &label, browser_exited)
            });
        });
        Ok(())
    }));
    let mut token = 0;
    // The handler captures no core/controller. The core owns it until close,
    // like the adjacent privileged permission/navigation registrations.
    unsafe { core.add_ProcessFailed(&handler, &mut token) }
}

#[cfg(test)]
#[path = "windows_renderer_tests.rs"]
mod tests;
