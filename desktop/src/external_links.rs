//! Links other applications hand to Zephium: a clicked link once Zephium is
//! the default browser, or addresses on the command line of a launch. Core
//! admits every address before the shell sees it.

use super::*;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use zephium_core::navigation::{external_target, MAX_EXTERNAL_TARGETS};

#[derive(Debug, PartialEq, Eq)]
struct WaitingRequest {
    profile: Option<String>,
    urls: Vec<String>,
    forward: bool,
}

#[derive(Default)]
struct WaitingRequests(Vec<WaitingRequest>);

impl WaitingRequests {
    fn push(&mut self, request: WaitingRequest) -> bool {
        let waiting: usize = self
            .0
            .iter()
            .map(|request| {
                request
                    .urls
                    .len()
                    .max(usize::from(request.profile.is_some()))
            })
            .sum();
        let room = MAX_EXTERNAL_TARGETS.saturating_sub(waiting);
        let cost = request
            .urls
            .len()
            .max(usize::from(request.profile.is_some()));
        if cost > 0 && cost <= room {
            self.0.push(request);
            true
        } else {
            false
        }
    }

    fn take(&mut self) -> Vec<WaitingRequest> {
        std::mem::take(&mut self.0)
    }
}

/// Hand-offs that arrive before the shell exists. macOS delivers a link that
/// launched the app through the run loop, which can precede setup.
static WAITING: Mutex<WaitingRequests> = Mutex::new(WaitingRequests(Vec::new()));
static PROFILE_ADMISSION_FAILED: AtomicBool = AtomicBool::new(false);

/// A running instance received links and wants the window that shows them
/// in front. The shell says which window that is once it has opened them.
static FORWARD: AtomicBool = AtomicBool::new(false);

fn without_startup_options(
    arguments: impl IntoIterator<Item = String>,
) -> impl Iterator<Item = String> {
    let mut skip_value = false;
    arguments.into_iter().filter(move |argument| {
        if skip_value {
            skip_value = false;
            return false;
        }
        if argument == "--profile" || argument == "--user-data-dir" {
            skip_value = true;
            return false;
        }
        !argument.starts_with("--profile=") && !argument.starts_with("--user-data-dir=")
    })
}

fn requested_profile(arguments: &[String]) -> Option<String> {
    let mut requested = None;
    let mut args = arguments.iter();
    while let Some(argument) = args.next() {
        let value = if argument == "--profile" {
            let value = args.next()?;
            if value.starts_with("--") {
                return None;
            }
            Some(value.as_str())
        } else {
            argument.strip_prefix("--profile=")
        };
        let Some(value) = value else { continue };
        if requested.is_some() {
            return None;
        }
        let name = zephium_core::profiles::profile_name(value).filter(|name| name == value)?;
        requested = Some(name);
    }
    requested
}

pub(crate) fn cancel_forward() {
    FORWARD.store(false, Ordering::Release);
}

/// Admits `arguments` and opens them. `forward` brings the window to the
/// front, which a running instance wants and a launch leaves to its own
/// startup reveal.
pub(crate) fn hand_off(
    app: &tauri::AppHandle,
    arguments: impl IntoIterator<Item = String>,
    forward: bool,
) {
    let arguments: Vec<String> = arguments.into_iter().collect();
    let has_profile_option = arguments
        .iter()
        .any(|argument| argument == "--profile" || argument.starts_with("--profile="));
    let profile = requested_profile(&arguments);
    if has_profile_option && profile.is_none() {
        cancel_forward();
        crate::emit_ui_command(app, "profile.open-failed");
        return;
    }
    let urls: Vec<String> = without_startup_options(arguments)
        .filter_map(|argument| external_target(&argument))
        .map(|url| url.to_string())
        .take(MAX_EXTERNAL_TARGETS)
        .collect();
    if shutdown_started(app) || (urls.is_empty() && profile.is_none()) {
        return;
    }
    let Some(shell) = app.try_state::<Handle>() else {
        let mut waiting = WAITING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let accepted = waiting.push(WaitingRequest {
            profile,
            urls,
            forward,
        });
        if !accepted && has_profile_option {
            PROFILE_ADMISSION_FAILED.store(true, Ordering::Release);
        }
        return;
    };
    // Each batch says afresh whether it wants the main window forward, so a
    // batch that opened only mini windows leaves nothing behind for the next.
    if let Some(name) = profile {
        let has_urls = !urls.is_empty();
        let command = if !has_urls {
            Command::OpenProfileByName(name)
        } else {
            Command::OpenProfileAndExternal { name, urls }
        };
        FORWARD.store(forward && has_urls, Ordering::Release);
        let admission = crate::dispatch_operation(app, &shell, command);
        if !admission.accepted {
            cancel_forward();
            crate::emit_ui_command(app, "profile.open-failed");
        }
    } else {
        FORWARD.store(forward, Ordering::Release);
        if !shell.dispatch(Command::OpenExternal(urls)) {
            cancel_forward();
        }
    }
}

/// Links opened as tabs: the main window comes forward to show them. A
/// link that opened a mini window leaves the request in place, since that
/// window comes forward on its own and a later link of the same batch may
/// still open as a tab; the shell reports tabs once per batch.
pub(crate) fn forward_pending(app: &tauri::AppHandle) {
    if FORWARD.swap(false, Ordering::AcqRel) {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || bring_forward(&handle));
    }
}

/// Runs once the shell is managed: what arrived early, then this launch's own
/// command line (Windows and Linux pass a clicked link as an argument).
pub(crate) fn adopt(app: &tauri::AppHandle) {
    hand_off(app, std::env::args().skip(1), false);
    let waiting = WAITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    for request in waiting {
        let mut arguments = Vec::new();
        if let Some(profile) = request.profile {
            arguments.extend(["--profile".into(), profile]);
        }
        arguments.extend(request.urls);
        hand_off(app, arguments, request.forward);
    }
    if PROFILE_ADMISSION_FAILED.swap(false, Ordering::AcqRel) {
        crate::emit_ui_command(app, "profile.open-failed");
    }
}

pub(crate) fn bring_forward(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        requested_profile, without_startup_options, WaitingRequest, WaitingRequests,
        MAX_EXTERNAL_TARGETS,
    };

    #[test]
    fn profile_options_and_values_are_not_forwarded_as_urls() {
        let arguments = [
            "https://example.com".into(),
            "--profile".into(),
            "Work".into(),
            "--user-data-dir=/tmp/profile".into(),
            "https://example.org".into(),
        ];
        assert_eq!(
            without_startup_options(arguments).collect::<Vec<_>>(),
            ["https://example.com", "https://example.org"]
        );
    }

    #[test]
    fn second_launch_profile_arguments_are_validated() {
        assert_eq!(
            requested_profile(&["--profile".into(), "Work".into()]).as_deref(),
            Some("Work")
        );
        assert_eq!(
            requested_profile(&["--profile=Work".into()]).as_deref(),
            Some("Work")
        );
        assert!(
            requested_profile(&["--profile=Work".into(), "--profile".into(), "Home".into()])
                .is_none()
        );
        assert!(requested_profile(&["--profile=".into()]).is_none());
        assert!(requested_profile(&["--profile=Work".into(), "--profile".into()]).is_none());
    }

    #[test]
    fn startup_profile_request_waits_for_shell_adoption() {
        let request = WaitingRequest {
            profile: Some("Work".into()),
            urls: Vec::new(),
            forward: false,
        };
        let mut waiting = WaitingRequests::default();
        assert!(waiting.push(request));
        assert_eq!(
            waiting.take(),
            [WaitingRequest {
                profile: Some("Work".into()),
                urls: Vec::new(),
                forward: false,
            }]
        );
        assert!(waiting.take().is_empty());
    }

    #[test]
    fn startup_profile_request_is_rejected_when_waiting_capacity_is_full() {
        let mut waiting = WaitingRequests::default();
        for index in 0..MAX_EXTERNAL_TARGETS {
            assert!(waiting.push(WaitingRequest {
                profile: None,
                urls: vec![format!("https://{index}.example/")],
                forward: false,
            }));
        }
        assert!(!waiting.push(WaitingRequest {
            profile: Some("Work".into()),
            urls: Vec::new(),
            forward: false,
        }));
    }
}
