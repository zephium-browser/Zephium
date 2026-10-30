use gtk::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Manager, WebviewWindow};

use crate::linux_shortcut::LinuxLauncherShortcut;
use crate::linux_shortcut_portal::{ActivationContext, ActivationTarget, GlobalRegistration};
use crate::linux_x11_shortcut::X11Shortcut;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayBackend {
    Wayland,
    X11,
    Unsupported,
}

pub(crate) struct LinuxGlobalShortcuts {
    backend: DisplayBackend,
    shortcut: Option<LinuxLauncherShortcut>,
    portal_started: AtomicBool,
    registration: GlobalRegistration,
    target: Arc<ActivationTarget>,
    x11_shortcut: Mutex<Option<X11Shortcut>>,
}

impl LinuxGlobalShortcuts {
    pub(crate) fn install(
        window: &WebviewWindow,
        shortcut: Option<LinuxLauncherShortcut>,
        registration: GlobalRegistration,
        activation: impl Fn(ActivationContext) + Send + Sync + 'static,
    ) -> Self {
        let state = Self {
            backend: detect_display_backend(window),
            shortcut,
            portal_started: AtomicBool::new(false),
            registration,
            target: Arc::new(ActivationTarget::new(activation)),
            x11_shortcut: Mutex::new(None),
        };
        state.install_x11_backend();
        state
    }

    pub(crate) fn is_registered(&self) -> bool {
        self.registration.is_live()
    }

    pub(crate) fn start_after_main_mapped(&self) {
        if self.backend != DisplayBackend::Wayland
            || self.shortcut.is_none()
            || !self.target.is_active()
            || self
                .portal_started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }

        let target = Arc::downgrade(&self.target);
        let registration = self.registration.clone();
        let shortcut = self.shortcut;
        let spawn = std::thread::Builder::new()
            .name("zephium-wayland-shortcut".to_owned())
            .spawn(move || {
                let Some(shortcut) = shortcut else {
                    return;
                };
                if let Err(error) = crate::linux_shortcut_portal::run(target, registration, shortcut)
                {
                    if !error.is_stopped() {
                        crate::write_diagnostic(format_args!(
                            "global shortcut: Wayland portal unavailable; focused-window fallback remains active: {error}"
                        ));
                    }
                }
            });
        if let Err(error) = spawn {
            crate::write_diagnostic(format_args!(
                "global shortcut: could not start Wayland portal worker; focused-window fallback remains active: {error}"
            ));
        }
    }

    pub(crate) fn shutdown(&self) {
        self.registration.set_live(false);
        self.target.stop();
        let shortcut = lock_recover(&self.x11_shortcut).take();
        if let Some(shortcut) = shortcut {
            shortcut.shutdown();
        }
    }

    fn install_x11_backend(&self) {
        if self.backend != DisplayBackend::X11 {
            if self.backend == DisplayBackend::Unsupported {
                crate::write_diagnostic(format_args!(
                    "global shortcut: unsupported GDK display backend; focused-window fallback remains active"
                ));
            }
            return;
        }
        let Some(shortcut) = self.shortcut else {
            return;
        };
        let shortcut = match X11Shortcut::spawn(
            shortcut,
            Arc::downgrade(&self.target),
            self.registration.clone(),
        ) {
            Ok(shortcut) => shortcut,
            Err(error) => {
                crate::write_diagnostic(format_args!(
                    "global shortcut unavailable on X11: {error}"
                ));
                return;
            }
        };
        *lock_recover(&self.x11_shortcut) = Some(shortcut);
    }
}

pub(crate) fn main_window_mapped(window: &WebviewWindow) {
    if let Some(state) = window.app_handle().try_state::<LinuxGlobalShortcuts>() {
        state.start_after_main_mapped();
    }
}

pub(crate) fn shutdown(app: &tauri::AppHandle) {
    if let Some(state) = app.try_state::<LinuxGlobalShortcuts>() {
        state.shutdown();
    }
}

fn detect_display_backend(window: &WebviewWindow) -> DisplayBackend {
    let Ok(gtk_window) = window.gtk_window() else {
        return DisplayBackend::Unsupported;
    };
    let display = gtk_window.display();
    classify_display_type(display.type_().name())
}

fn classify_display_type(type_name: &str) -> DisplayBackend {
    match type_name {
        "GdkWaylandDisplay" => DisplayBackend::Wayland,
        "GdkX11Display" => DisplayBackend::X11,
        _ => DisplayBackend::Unsupported,
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdk_backend_detection_is_type_exact() {
        assert_eq!(
            classify_display_type("GdkWaylandDisplay"),
            DisplayBackend::Wayland
        );
        assert_eq!(classify_display_type("GdkX11Display"), DisplayBackend::X11);
        assert_eq!(
            classify_display_type("wayland-0"),
            DisplayBackend::Unsupported
        );
        assert_eq!(classify_display_type(""), DisplayBackend::Unsupported);
    }

    #[test]
    fn production_adapter_has_no_panicking_diagnostics() {
        let production = include_str!("linux_global_shortcuts.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production Linux global-shortcut adapter");
        for forbidden in [
            "eprintln!",
            ".unwrap(",
            ".expect(",
            "panic!",
            "unreachable!",
        ] {
            assert!(!production.contains(forbidden));
        }
    }
}
