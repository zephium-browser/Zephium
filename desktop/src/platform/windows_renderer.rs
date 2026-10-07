//! Recover the privileged documents independently of ordinary page crashes.
use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
    COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
};
use webview2_com::ProcessFailedEventHandler;

use crate::renderer_recovery::{Action, Recovery};

#[derive(Default)]
pub(crate) struct Renderers {
    main: Mutex<Recovery>,
    panel: Mutex<Recovery>,
}

impl Renderers {
    fn surface(&self, label: &str) -> Option<&Mutex<Recovery>> {
        match label {
            crate::MAIN_LABEL => Some(&self.main),
            crate::overlay::PANEL_LABEL => Some(&self.panel),
            _ => None,
        }
    }
}

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
            let _ = app.run_on_main_thread(move || failed(&target, &label, browser_exited));
        });
        Ok(())
    }));
    let mut token = 0;
    // The handler captures no core/controller. The core owns it until close,
    // like the adjacent privileged permission/navigation registrations.
    unsafe { core.add_ProcessFailed(&handler, &mut token) }
}

fn failed(app: &AppHandle, label: &str, browser_exited: bool) {
    if crate::shutdown_started(app) {
        return;
    }
    let Some(renderers) = app.try_state::<Renderers>() else {
        return;
    };
    let Some(surface) = renderers.surface(label) else {
        return;
    };
    let action = {
        let mut recovery = surface.lock().unwrap_or_else(|e| e.into_inner());
        if browser_exited {
            if recovery.stop() {
                Action::Stop
            } else {
                Action::Ignore
            }
        } else {
            recovery.crashed()
        }
    };
    let attempt = match action {
        Action::Ignore => return,
        Action::Stop => return explain_failure(app),
        Action::Reload(attempt) => attempt,
    };
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    crate::write_diagnostic(format_args!(
        "chrome: recovering {label} renderer, attempt {attempt}"
    ));
    let target = if label == crate::MAIN_LABEL {
        app.try_state::<crate::UiStartupGate>()
            .and_then(|gate| gate.prepare_recovery(&window))
    } else {
        app.try_state::<crate::overlay::Overlay>()
            .and_then(|overlay| overlay.prepare_recovery())
    };
    if target.is_none_or(|url| window.navigate(url).is_err()) {
        surface.lock().unwrap_or_else(|e| e.into_inner()).stop();
        explain_failure(app);
        return;
    }
    let app = app.clone();
    let label = label.to_owned();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(crate::UI_INITIALIZATION_TIMEOUT).await;
        let target = app.clone();
        let _ = app.run_on_main_thread(move || {
            if crate::shutdown_started(&target) {
                return;
            }
            let expired = target.try_state::<Renderers>().is_some_and(|renderers| {
                renderers.surface(&label).is_some_and(|surface| {
                    surface
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .timed_out(attempt)
                })
            });
            if expired {
                explain_failure(&target);
            }
        });
    });
}

pub(crate) fn ready(window: &WebviewWindow) -> bool {
    window
        .app_handle()
        .try_state::<Renderers>()
        .is_some_and(|renderers| {
            renderers
                .surface(window.label())
                .is_some_and(|surface| surface.lock().unwrap_or_else(|e| e.into_inner()).ready())
        })
}

fn explain_failure(app: &AppHandle) {
    crate::write_diagnostic(format_args!("chrome: privileged renderer recovery failed"));
    crate::startup_alert::show_then(
        app,
        crate::startup_alert::StartupProblem::RendererFailed,
        "",
        crate::request_orderly_terminal_failure,
    );
}

#[cfg(test)]
#[path = "windows_renderer_tests.rs"]
mod tests;
