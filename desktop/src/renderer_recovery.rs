//! A renderer restart must settle before another can begin, and must never
//! become an endless crash/reload loop over a long-running browser session.
//! The privileged documents recover the same way on every platform; only the
//! native signal that their content process ended differs.

use std::time::{Duration, Instant};

const STABLE_RECOVERY: Duration = Duration::from_secs(10 * 60);

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::Mutex;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{AppHandle, Manager, WebviewWindow};

#[derive(Default)]
pub(crate) struct Recovery {
    attempts: u8,
    generation: u64,
    pending: Option<u64>,
    healthy_since: Option<Instant>,
    terminal: bool,
    restarting: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Reload(u64),
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
        self.crashed_at(Instant::now())
    }

    fn crashed_at(&mut self, now: Instant) -> Action {
        if std::mem::take(&mut self.restarting) || self.terminal || self.pending.is_some() {
            return Action::Ignore;
        }
        if self
            .healthy_since
            .take()
            .is_some_and(|since| now.saturating_duration_since(since) >= STABLE_RECOVERY)
        {
            self.attempts = 0;
        }
        if self.attempts == 2 {
            self.terminal = true;
            return Action::Stop;
        }
        let Some(generation) = self.generation.checked_add(1) else {
            self.terminal = true;
            return Action::Stop;
        };
        self.attempts += 1;
        self.generation = generation;
        self.pending = Some(generation);
        Action::Reload(generation)
    }

    /// The next end of the content process is one the app asked for.
    #[cfg(any(test, target_os = "macos"))]
    pub(crate) fn expect_restart(&mut self) {
        self.restarting = true;
    }

    /// Whatever restart was asked for is over, ended or not.
    #[cfg(any(test, target_os = "macos"))]
    pub(crate) fn restart_settled(&mut self) {
        self.restarting = false;
    }

    pub(crate) fn ready(&mut self) -> bool {
        self.ready_at(Instant::now())
    }

    fn ready_at(&mut self, now: Instant) -> bool {
        if self.terminal || self.pending.take().is_none() {
            return false;
        }
        self.healthy_since = Some(now);
        true
    }

    pub(crate) fn timed_out(&mut self, attempt: u64) -> bool {
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

/// Marks the main document's next process end as asked for, not a crash.
#[cfg(target_os = "macos")]
pub(crate) fn expect_restart(app: &AppHandle) {
    if let Some(renderers) = app.try_state::<Renderers>() {
        renderers
            .main
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expect_restart();
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn restart_settled(app: &AppHandle) {
    if let Some(renderers) = app.try_state::<Renderers>() {
        renderers
            .main
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .restart_settled();
    }
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
    fn repeated_events_do_not_overlap_reloads_or_reset_the_rapid_crash_budget() {
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
    fn stable_recovery_replenishes_budget_without_reusing_deadline_identity() {
        let now = Instant::now();
        let mut recovery = Recovery::default();
        assert_eq!(recovery.crashed_at(now), Action::Reload(1));
        assert!(recovery.ready_at(now));
        assert_eq!(recovery.crashed_at(now), Action::Reload(2));
        assert!(recovery.ready_at(now));
        assert_eq!(
            recovery.crashed_at(now + STABLE_RECOVERY),
            Action::Reload(3)
        );
        assert!(!recovery.timed_out(1));
        assert!(!recovery.timed_out(2));
        assert!(recovery.timed_out(3));
    }

    #[test]
    fn exhausted_generation_space_stops_instead_of_reusing_a_deadline() {
        let mut recovery = Recovery {
            generation: u64::MAX,
            ..Recovery::default()
        };
        assert_eq!(recovery.crashed(), Action::Stop);
        assert_eq!(recovery.crashed(), Action::Ignore);
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
    fn an_asked_for_restart_is_not_a_crash_and_never_hides_a_later_one() {
        let mut recovery = Recovery::default();
        recovery.expect_restart();
        assert_eq!(recovery.crashed(), Action::Ignore);
        assert_eq!(recovery.crashed(), Action::Reload(1));
        assert!(recovery.ready());

        recovery.expect_restart();
        recovery.restart_settled();
        assert_eq!(recovery.crashed(), Action::Reload(2));
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
