//! A native explanation for a startup that cannot continue. Without it a
//! Finder or Start-menu launch has no visible stderr and simply disappears.

use std::sync::atomic::{AtomicU8, Ordering};

const TITLE: &str = "Zephium couldn't start";
const ISSUES_URL: &str = "https://github.com/zephium-browser/Zephium/issues";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupProblem {
    NewerProfile,
    DamagedProfile,
    UnsupportedSystem,
    #[cfg(target_os = "windows")]
    RendererFailed,
    Other,
}

impl StartupProblem {
    fn title(self) -> &'static str {
        #[cfg(target_os = "windows")]
        if self == Self::RendererFailed {
            return "Zephium needs to restart";
        }
        TITLE
    }
    /// Store errors cross several boxed layers before they reach the setup
    /// boundary, so the stable SQLite and migration messages are the contract.
    pub(crate) fn classify(error: &str) -> Self {
        if error.contains("is newer than supported version") {
            Self::NewerProfile
        } else if error.contains("database disk image is malformed")
            || error.contains("file is not a database")
        {
            Self::DamagedProfile
        } else {
            Self::Other
        }
    }

    fn message(self, detail: &str) -> String {
        match self {
            #[cfg(target_os = "windows")]
            Self::RendererFailed => "Zephium couldn't restore its browser controls. \
                Close this message and open Zephium again. Your saved browsing data is kept, \
                but edits that hadn't been saved may be lost."
                .to_owned(),
            Self::NewerProfile => "Your profile was last opened by a newer version of Zephium. \
                 Install the latest version from zephium.app to open it. \
                 Nothing in your profile was changed."
                .to_owned(),
            Self::DamagedProfile => format!(
                "Zephium couldn't read your profile; it may be damaged. Nothing was deleted.\n\n\
                 Please report this at {ISSUES_URL}\n\nDetails: {detail}"
            ),
            Self::UnsupportedSystem => detail.to_owned(),
            Self::Other => format!(
                "Something went wrong while starting Zephium. Try opening it again.\n\n\
                 If it keeps happening, please report it at {ISSUES_URL}\n\nDetails: {detail}"
            ),
        }
    }
}

/// Before Tauri's event loop exists, a modal system alert is the only UI.
#[cfg(not(target_os = "linux"))]
pub(crate) fn show_blocking(problem: StartupProblem, detail: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(problem.title())
        .set_description(problem.message(detail))
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// Inside the running event loop a nested modal can re-enter it, so the alert
/// is asynchronous and `then` runs once it is dismissed. Only the first
/// failure is explained: later ones that arrive while it is open (dependent
/// components timing out) wait for it, and any after it proceed directly.
pub(crate) fn show_then(
    app: &tauri::AppHandle,
    problem: StartupProblem,
    detail: &str,
    then: impl FnOnce(&tauri::AppHandle) + Send + 'static,
) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

    const NONE: u8 = 0;
    const OPEN: u8 = 1;
    const DISMISSED: u8 = 2;
    static ALERT: AtomicU8 = AtomicU8::new(NONE);
    match ALERT.compare_exchange(NONE, OPEN, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(OPEN) => return,
        Err(_) => {
            then(app);
            return;
        }
    }
    let handle = app.clone();
    app.dialog()
        .message(problem.message(detail))
        .title(problem.title())
        .kind(MessageDialogKind::Error)
        .show(move |_| {
            ALERT.store(DISMISSED, Ordering::Release);
            let main = handle.clone();
            if handle.run_on_main_thread(move || then(&main)).is_err() {
                crate::write_diagnostic(format_args!(
                    "startup: cannot return to the main thread after the startup alert"
                ));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::StartupProblem;

    #[test]
    fn classifies_the_store_errors_users_can_act_on() {
        assert_eq!(
            StartupProblem::classify(
                "database schema version 40 is newer than supported version 34"
            ),
            StartupProblem::NewerProfile
        );
        for damaged in ["database disk image is malformed", "file is not a database"] {
            assert_eq!(
                StartupProblem::classify(damaged),
                StartupProblem::DamagedProfile
            );
        }
        assert_eq!(
            StartupProblem::classify("cannot create the data directory"),
            StartupProblem::Other
        );
    }
}
