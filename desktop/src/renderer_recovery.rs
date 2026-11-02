//! A renderer restart must settle before another can begin, and must never
//! become an endless crash/reload loop over a long-running browser session.
//! The privileged documents recover the same way on every platform; only the
//! native signal that their content process ended differs.

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::Mutex;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{AppHandle, Manager, WebviewWindow};

#[derive(Default)]
pub(crate) struct Recovery {
    attempts: u8,
    pending: Option<u8>,
    terminal: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Reload(u8),
    Ignore,
    Stop,
}

impl Recovery {
    #[cfg(all(test, target_os = "windows"))]
    pub(crate) fn settled_attempts(&self) -> u8 {
        if self.pending.is_none() && !self.terminal {
            self.attempts
        } else {
            0
        }
    }

    pub(crate) fn crashed(&mut self) -> Action {
        if self.terminal || self.pending.is_some() {
            return Action::Ignore;
        }
        if self.attempts == 2 {
            self.terminal = true;
            return Action::Stop;
        }
        self.attempts += 1;
        self.pending = Some(self.attempts);
        Action::Reload(self.attempts)
    }

    pub(crate) fn ready(&mut self) -> bool {
        !self.terminal && self.pending.take().is_some()
    }

    pub(crate) fn timed_out(&mut self, attempt: u8) -> bool {
        if self.pending != Some(attempt) {
            return false;
        }
        self.stop()
    }

    pub(crate) fn stop(&mut self) -> bool {
        self.pending = None;
        !std::mem::replace(&mut self.terminal, true)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
pub(crate) struct Renderers {
    pub(crate) main: Mutex<Recovery>,
    panel: Mutex<Recovery>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Renderers {
    fn surface(&self, label: &str) -> Option<&Mutex<Recovery>> {
        match label {
            crate::MAIN_LABEL => Some(&self.main),
            crate::overlay::PANEL_LABEL => Some(&self.panel),
            _ => None,
        }
    }
}

/// Whether a privileged document by this label recovers here.
#[cfg(target_os = "macos")]
pub(crate) fn owns(label: &str) -> bool {
    Renderers::default().surface(label).is_some()
}

/// The content process of the document labelled `label` ended. With
/// `browser_exited` the whole engine is gone and nothing can be reloaded.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn failed(app: &AppHandle, label: &str, browser_exited: bool) {
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

/// The recovered document initialized; true once per recovery.
#[cfg(any(target_os = "macos", target_os = "windows"))]
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

#[cfg(any(target_os = "macos", target_os = "windows"))]
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
mod tests {
    use super::*;

    #[test]
    fn repeated_events_do_not_overlap_reloads_or_reset_the_lifetime_budget() {
        let mut recovery = Recovery::default();
        assert!(!recovery.ready());
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert_eq!(recovery.crashed(), Action::Ignore);
        assert!(recovery.ready());
        assert_eq!(recovery.crashed(), Action::Reload(2));
        assert!(recovery.ready());
        assert_eq!(recovery.crashed(), Action::Stop);
        assert_eq!(recovery.crashed(), Action::Ignore);
        assert!(!recovery.ready());
    }

    #[test]
    fn an_old_deadline_cannot_stop_a_recovered_or_newer_renderer() {
        let mut recovery = Recovery::default();
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert!(recovery.ready());
        assert!(!recovery.timed_out(1));
        assert_eq!(recovery.crashed(), Action::Reload(2));
        assert!(!recovery.timed_out(1));
        assert!(recovery.timed_out(2));
        assert!(!recovery.timed_out(2));
        assert!(!recovery.ready());
        assert_eq!(recovery.crashed(), Action::Ignore);
    }

    #[test]
    fn browser_exit_stops_even_an_in_flight_recovery_once() {
        let mut recovery = Recovery::default();
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert!(recovery.stop());
        assert!(!recovery.stop());
        assert!(!recovery.ready());
        assert!(!recovery.timed_out(1));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn only_the_privileged_documents_recover_through_the_shared_policy() {
        let renderers = Renderers::default();
        assert!(renderers.surface(crate::MAIN_LABEL).is_some());
        assert!(renderers.surface(crate::overlay::PANEL_LABEL).is_some());
        assert!(renderers.surface("page").is_none());
        assert!(renderers.surface("").is_none());
    }
}
