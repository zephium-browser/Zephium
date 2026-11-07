//! A native explanation for a startup that cannot continue. Without it a
//! Finder or Start-menu launch has no visible stderr and simply disappears.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use zephium_core::webview2::AdmissionError;

const TITLE: &str = "Zephium couldn't start";
const ISSUES_URL: &str = "https://github.com/zephium-browser/Zephium/issues";
const WEBVIEW2_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
const SHOW_LOG: &str = "Show log folder";
const CLOSE: &str = "Close";

/// SQLite's own messages, and the Windows errors for access denied, sharing
/// violation, handle disk full and disk full.
const UNWRITABLE: [&str; 7] = [
    "database or disk is full",
    "unable to open database file",
    "attempt to write a readonly database",
    "(os error 5)",
    "(os error 32)",
    "(os error 39)",
    "(os error 112)",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StartupProblem {
    NewerProfile,
    DamagedProfile,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    UnsupportedSystem,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    RendererFailed,
    /// No WebView2 Runtime is installed.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WebViewMissing,
    /// The runtime in use is a Beta, Dev or Canary build.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WebViewPreview,
    /// The runtime lacks something Zephium requires.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WebViewOutdated,
    /// A WebView2 environment variable redirects the runtime.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    WebViewOverride,
    /// The profile's folder cannot be read or written.
    DataFolderUnavailable,
    /// Another Zephium process still holds the profile.
    ProfileInUse,
    Other,
}

impl StartupProblem {
    fn title(self) -> &'static str {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
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
        } else if error.contains("database is locked") {
            Self::ProfileInUse
        } else if UNWRITABLE.iter().any(|stable| error.contains(stable)) {
            Self::DataFolderUnavailable
        } else {
            Self::Other
        }
    }

    /// Whether the person gets the log folder and the path to it: the cases
    /// where they cannot act and a report has to carry the details.
    fn offers_log(self) -> bool {
        matches!(
            self,
            Self::DamagedProfile | Self::UnsupportedSystem | Self::Other
        )
    }

    fn message(self, detail: &str) -> String {
        self.message_with_log(detail, crate::diagnostics::log_dir())
    }

    fn message_with_log(self, detail: &str, log: Option<&Path>) -> String {
        let mut message = match self {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
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
            Self::WebViewMissing => format!(
                "Zephium needs the Microsoft Edge WebView2 Runtime, and this PC doesn't have it.\n\n\
                 Install the Evergreen WebView2 Runtime from {WEBVIEW2_URL} \
                 and open Zephium again."
            ),
            Self::WebViewPreview => format!(
                "Zephium can't run on {}. It needs the stable Evergreen WebView2 Runtime \
                 from Microsoft.\n\n\
                 Install it from {WEBVIEW2_URL} and open Zephium again. If the installer says \
                 WebView2 is already installed, remove \"Microsoft Edge WebView2 Runtime\" in \
                 Settings > Apps and run the installer again.",
                describe_preview(detail)
            ),
            Self::WebViewOutdated => format!(
                "The Microsoft Edge WebView2 Runtime on this PC is older than Zephium needs.\n\n\
                 Run Windows Update, or install the latest Evergreen WebView2 Runtime from \
                 {WEBVIEW2_URL}, then open Zephium again.\n\nDetails: {detail}"
            ),
            Self::WebViewOverride => format!(
                "A WebView2 setting on this PC ({detail}) changes which browser engine Zephium \
                 would run on, so Zephium won't start with it.\n\n\
                 Remove that environment variable (search Windows for \"Edit environment \
                 variables\") and open Zephium again."
            ),
            Self::DataFolderUnavailable => format!(
                "Zephium couldn't read or write its data folder. Check that the disk isn't full \
                 and that the folder isn't read-only or blocked by security software, then open \
                 Zephium again. Nothing was deleted.\n\nDetails: {detail}"
            ),
            Self::ProfileInUse => "Zephium's data is still in use by another copy of Zephium that \
                 hasn't finished closing. Wait a few seconds and open it again; if it keeps \
                 happening, end Zephium in Task Manager or restart your computer."
                .to_owned(),
            Self::Other => format!(
                "Something went wrong while starting Zephium. Try opening it again.\n\n\
                 If it keeps happening, please report it at {ISSUES_URL}\n\nDetails: {detail}"
            ),
        };
        if let Some(log) = log.filter(|_| self.offers_log()) {
            message.push_str(&format!(
                "\n\nA log of what happened is kept in:\n{}",
                log.display()
            ));
        }
        message
    }
}

/// "the Beta version of WebView2 (157.0.4341.16)" from the runtime's own
/// "157.0.4341.16 beta".
fn describe_preview(reported: &str) -> String {
    match reported.split_once(' ') {
        Some((version, channel)) if !channel.is_empty() => {
            let mut letters = channel.chars();
            let name = letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default();
            format!("the {name} version of WebView2 ({version})")
        }
        _ => format!("the preview version of WebView2 ({reported})"),
    }
}

/// A startup failure that knows what the person can do about it. It crosses
/// the setup hook's boxed error, where the alert reads it back; any other
/// error is classified from its text.
#[derive(Debug)]
pub(crate) struct StartupFailure {
    problem: StartupProblem,
    detail: String,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl StartupFailure {
    pub(crate) fn new(problem: StartupProblem, detail: impl Into<String>) -> Self {
        Self {
            problem,
            detail: detail.into(),
        }
    }

    pub(crate) fn problem(&self) -> StartupProblem {
        self.problem
    }

    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }

    /// Adds the runtime the failure was found on, for the log and the report.
    #[cfg(target_os = "windows")]
    pub(crate) fn on_runtime(mut self, reported: &str) -> Self {
        if self.problem != StartupProblem::WebViewPreview {
            self.detail = format!("{}; WebView2 runtime {reported}", self.detail);
        }
        self
    }
}

/// The log's account of the failure, which the alert's wording leaves out.
impl fmt::Display for StartupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.problem {
            StartupProblem::WebViewPreview => write!(
                formatter,
                "the WebView2 runtime in use ({}) is a preview channel, not the Evergreen WebView2 Runtime",
                self.detail
            ),
            StartupProblem::WebViewOverride => write!(
                formatter,
                "the WebView2 environment override {} is set",
                self.detail
            ),
            _ => formatter.write_str(&self.detail),
        }
    }
}

impl std::error::Error for StartupFailure {}

/// What a refused WebView2 runtime means for the person. The launch-time
/// check and the one made on the environment that was actually created
/// answer to this alone, so they cannot disagree about what to say.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn runtime_failure(reported: &str, error: AdmissionError) -> StartupFailure {
    match error {
        AdmissionError::PreviewChannel(_) => {
            StartupFailure::new(StartupProblem::WebViewPreview, reported)
        }
        AdmissionError::InvalidVersion(_) => StartupFailure::new(
            StartupProblem::Other,
            format!("the WebView2 runtime reported a version Zephium can't read ({reported:?}): {error}"),
        ),
    }
}

/// Before Tauri's event loop exists, a modal system alert is the only UI.
#[cfg(not(target_os = "linux"))]
pub(crate) fn show_blocking(problem: StartupProblem, detail: &str) {
    let dialog = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title(problem.title())
        .set_description(problem.message(detail));
    if problem.offers_log() && crate::diagnostics::log_dir().is_some() {
        let answer = dialog
            .set_buttons(rfd::MessageButtons::OkCancelCustom(
                SHOW_LOG.to_owned(),
                CLOSE.to_owned(),
            ))
            .show();
        if answer == rfd::MessageDialogResult::Custom(SHOW_LOG.to_owned()) {
            crate::diagnostics::reveal();
        }
    } else {
        dialog.set_buttons(rfd::MessageButtons::Ok).show();
    }
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
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

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
    let dialog = app
        .dialog()
        .message(problem.message(detail))
        .title(problem.title())
        .kind(MessageDialogKind::Error);
    let dialog = if problem.offers_log() && crate::diagnostics::log_dir().is_some() {
        dialog.buttons(MessageDialogButtons::OkCancelCustom(
            SHOW_LOG.to_owned(),
            CLOSE.to_owned(),
        ))
    } else {
        dialog
    };
    dialog.show(move |show_log| {
        if show_log {
            crate::diagnostics::reveal();
        }
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
    use std::path::Path;

    use zephium_core::webview2::{admit_runtime, assess_runtime};

    use super::{runtime_failure, StartupFailure, StartupProblem};

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

    #[test]
    fn classifies_a_held_or_unwritable_profile() {
        assert_eq!(
            StartupProblem::classify("database is locked"),
            StartupProblem::ProfileInUse
        );
        for unwritable in [
            "database or disk is full",
            "unable to open database file",
            "attempt to write a readonly database",
            "Access is denied. (os error 5)",
            "The process cannot access the file because it is being used by another process. (os error 32)",
            "There is not enough space on the disk. (os error 112)",
        ] {
            assert_eq!(
                StartupProblem::classify(unwritable),
                StartupProblem::DataFolderUnavailable,
                "{unwritable}"
            );
        }
        // A longer code is a different error.
        assert_eq!(
            StartupProblem::classify("The system cannot find the path specified. (os error 50)"),
            StartupProblem::Other
        );
    }

    #[test]
    fn a_preview_runtime_is_named_with_its_channel_and_how_to_leave_it() {
        for (reported, channel) in [
            ("157.0.4341.16 beta", "Beta"),
            ("158.0.4400.0 dev", "Dev"),
            ("159.0.4410.1 canary", "Canary"),
        ] {
            let failure = runtime_failure(
                reported,
                admit_runtime(reported).expect_err("preview runtimes are refused"),
            );
            assert_eq!(failure.problem(), StartupProblem::WebViewPreview);
            let message = failure.problem().message_with_log(failure.detail(), None);
            let version = reported.split(' ').next().unwrap();
            assert!(
                message.contains(&format!("the {channel} version of WebView2 ({version})")),
                "{message}"
            );
            assert!(message.contains("Evergreen WebView2 Runtime"));
            assert!(message.contains("https://developer.microsoft.com/microsoft-edge/webview2/"));
            assert!(!message.contains("hardening"));
            assert!(!message.contains("composition"));
        }
    }

    #[test]
    fn the_launch_check_and_the_environment_check_say_the_same_thing() {
        // Both refuse through `admit_runtime`; whichever runs, the alert is
        // the same for the same runtime string.
        for reported in [
            "157.0.4341.16 beta",
            "157.0.4341.16",
            "garbage",
            "",
            "1.2.3",
        ] {
            let early = assess_runtime(reported, 0).map(|_| ());
            let environment = admit_runtime(reported).map(|_| ());
            assert_eq!(early.is_ok(), environment.is_ok(), "{reported:?}");
            if let (Err(early), Err(environment)) = (early, environment) {
                let early = runtime_failure(reported, early);
                let environment = runtime_failure(reported, environment);
                assert_eq!(early.problem(), environment.problem());
                assert_eq!(early.detail(), environment.detail());
            }
        }
        let unreadable = runtime_failure(
            "garbage",
            admit_runtime("garbage").expect_err("not a version"),
        );
        assert_eq!(unreadable.problem(), StartupProblem::Other);
        assert!(unreadable.detail().contains("garbage"));
    }

    #[test]
    fn actionable_problems_say_what_to_do_without_internal_terms() {
        for problem in [
            StartupProblem::WebViewMissing,
            StartupProblem::WebViewOutdated,
            StartupProblem::WebViewOverride,
            StartupProblem::DataFolderUnavailable,
            StartupProblem::ProfileInUse,
        ] {
            let message = problem.message_with_log("detail", Some(Path::new("/logs")));
            for jargon in ["privileged", "hardening", "composition", "admission"] {
                assert!(!message.contains(jargon), "{problem:?}: {message}");
            }
            assert!(
                !message.contains("/logs"),
                "{problem:?} needs no log, the person can fix it"
            );
        }
        assert!(StartupProblem::WebViewOverride
            .message_with_log("WEBVIEW2_USER_DATA_FOLDER", None)
            .contains("WEBVIEW2_USER_DATA_FOLDER"));
    }

    #[test]
    fn what_cannot_be_acted_on_names_the_failure_and_the_log() {
        for problem in [
            StartupProblem::Other,
            StartupProblem::DamagedProfile,
            StartupProblem::UnsupportedSystem,
        ] {
            assert!(problem.offers_log());
            let message = problem.message_with_log("the reason", Some(Path::new("/logs")));
            assert!(message.contains("the reason") || problem == StartupProblem::NewerProfile);
            assert!(message.contains("/logs"), "{problem:?}: {message}");
            assert!(!problem
                .message_with_log("the reason", None)
                .contains("/logs"));
        }
        assert!(!StartupProblem::NewerProfile.offers_log());
    }

    #[test]
    fn a_failure_reads_back_as_its_detail() {
        let failure = StartupFailure::new(StartupProblem::WebViewOutdated, "missing interface");
        let boxed: Box<dyn std::error::Error> = Box::new(failure);
        assert_eq!(boxed.to_string(), "missing interface");
        let preview = StartupFailure::new(StartupProblem::WebViewPreview, "157.0.4341.16 beta");
        assert!(preview.to_string().contains("157.0.4341.16 beta"));
        assert!(preview.to_string().contains("preview channel"));
        assert_eq!(preview.detail(), "157.0.4341.16 beta");
        let back = boxed
            .downcast::<StartupFailure>()
            .expect("the typed failure survives the boxed setup error");
        assert_eq!(back.problem(), StartupProblem::WebViewOutdated);
    }
}
