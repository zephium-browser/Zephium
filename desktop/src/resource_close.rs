//! Shutdown waits for bounded renderer draft flushes. System shutdown joins
//! an existing round at its earliest deadline and never waits on a dialog.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

use crate::menu_text;

#[derive(Default)]
struct Gate {
    touched: BTreeSet<String>,
    pending: BTreeMap<String, (String, tokio::sync::oneshot::Sender<bool>)>,
    round: Option<FlushRound>,
    system_closing: bool,
    system_result: Option<bool>,
    prepared: bool,
    flushed: Vec<(String, String)>,
}
type FlushDone = Box<dyn FnOnce(bool) + Send>;

struct FlushRound {
    deadline: tokio::sync::watch::Sender<std::time::Instant>,
    system: bool,
    participants_available: bool,
    waiters: Vec<FlushDone>,
}
impl FlushRound {
    fn new(
        deadline: std::time::Instant,
    ) -> (Self, tokio::sync::watch::Receiver<std::time::Instant>) {
        let (send, receive) = tokio::sync::watch::channel(deadline);
        (
            Self {
                deadline: send,
                system: false,
                participants_available: true,
                waiters: Vec::new(),
            },
            receive,
        )
    }

    fn join_system(&mut self, deadline: std::time::Instant, done: FlushDone) {
        self.system = true;
        self.deadline
            .send_modify(|current| *current = (*current).min(deadline));
        self.waiters.push(done);
    }

    fn finish(self, saved: bool) {
        for done in self.waiters {
            done(saved);
        }
    }
}

async fn wait_for_flush(
    receivers: Vec<tokio::sync::oneshot::Receiver<bool>>,
    mut deadline: tokio::sync::watch::Receiver<std::time::Instant>,
) -> bool {
    let replies = async move {
        let mut okay = true;
        for receive in receivers {
            okay = receive.await.unwrap_or(false) && okay;
        }
        okay
    };
    tokio::pin!(replies);
    loop {
        let until = *deadline.borrow_and_update();
        // Even a queued successful reply cannot extend an expired OS budget.
        if std::time::Instant::now() >= until {
            return false;
        }
        tokio::select! {
            saved = &mut replies => {
                return saved && std::time::Instant::now() < *deadline.borrow();
            },
            _ = tokio::time::sleep_until(until.into()) => return false,
            changed = deadline.changed() => if changed.is_err() { return false; },
        }
    }
}

static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
fn gate() -> &'static Mutex<Gate> {
    GATE.get_or_init(|| Mutex::new(Gate::default()))
}
pub(super) fn touch(app: &tauri::AppHandle, label: &str) -> bool {
    let mut gate = gate().lock().unwrap_or_else(|e| e.into_inner());
    gate.touch(label, || super::shutdown_started(app))
}
impl Gate {
    fn touch(&mut self, label: &str, terminal_started: impl FnOnce() -> bool) -> bool {
        // Evaluate the authoritative shutdown check under the same mutex as
        // final close admission. A caller's earlier check may predate that lock.
        if self.prepared || terminal_started() {
            return false;
        }
        self.touched.insert(label.into());
        true
    }

    fn participants(&self) -> Vec<&'static str> {
        // The launcher may not have been created or loaded yet.
        // A window that never used a resource API cannot own a resource draft
        // and may not have a renderer close listener yet. Touched hidden hosts
        // still participate: hiding a view does not discard its pending edits.
        [super::MAIN_LABEL, super::overlay::PANEL_LABEL]
            .into_iter()
            .filter(|label| self.touched.contains(*label))
            .collect()
    }

    fn requires_another_flush(&self, participants: &[&str]) -> bool {
        self.participants()
            .iter()
            .any(|label| !participants.contains(label))
    }

    /// The person chose to leave what could not be saved. Admission is closed
    /// exactly as after a successful flush.
    fn leave_unsaved(&mut self, terminal: bool, requests: &[(&'static str, String)]) {
        self.prepared = !terminal;
        self.flushed = requests
            .iter()
            .map(|(label, token)| ((*label).to_owned(), token.clone()))
            .collect();
    }

    fn enlist_new(
        &mut self,
        participants: &mut Vec<&'static str>,
        available: impl Fn(&str) -> bool,
        mut token: impl FnMut() -> String,
    ) -> (
        Vec<(&'static str, String)>,
        Vec<tokio::sync::oneshot::Receiver<bool>>,
    ) {
        let additional: Vec<_> = self
            .participants()
            .into_iter()
            .filter(|label| !participants.contains(label))
            .collect();
        let mut requests = Vec::new();
        let mut receivers = Vec::new();
        for label in additional {
            participants.push(label);
            if !available(label) {
                if let Some(round) = self.round.as_mut() {
                    round.participants_available = false;
                }
                continue;
            }
            let token = token();
            let (send, receive) = tokio::sync::oneshot::channel();
            self.pending.insert(label.into(), (token.clone(), send));
            receivers.push(receive);
            requests.push((label, token));
        }
        (requests, receivers)
    }

    fn needs_system_wave(
        &self,
        completed: bool,
        participants: &[&str],
        now: std::time::Instant,
    ) -> bool {
        completed
            && self.requires_another_flush(participants)
            && self
                .round
                .as_ref()
                .is_some_and(|round| round.system && now < *round.deadline.borrow())
    }

    fn admit_system(
        &mut self,
        deadline: std::time::Instant,
        terminal_started: bool,
        done: FlushDone,
    ) -> Option<FlushDone> {
        self.system_closing = true;
        if let Some(saved) = self.system_result {
            done(saved);
        } else if terminal_started {
            self.system_result = Some(false);
            done(false);
        } else if let Some(round) = self.round.as_mut() {
            round.join_system(deadline, done);
        } else {
            return Some(done);
        }
        None
    }

    fn finish_system(
        &mut self,
        round: FlushRound,
        completed: bool,
        participants: &[&str],
        requests: &[(&'static str, String)],
    ) -> bool {
        let saved = completed
            && round.participants_available
            && std::time::Instant::now() < *round.deadline.borrow()
            && !self.requires_another_flush(participants);
        self.pending.clear();
        self.prepared = false;
        if saved {
            self.flushed = requests
                .iter()
                .map(|(label, token)| ((*label).to_owned(), token.clone()))
                .collect();
        }
        self.system_result = Some(saved);
        round.finish(saved);
        saved
    }

    fn resolve_user_choice(
        &mut self,
        quit: bool,
        terminal: bool,
        requests: &[(&'static str, String)],
    ) -> bool {
        if !quit || self.system_closing {
            return false;
        }
        self.leave_unsaved(terminal, requests);
        true
    }

    fn complete(&mut self, label: &str, token: &str, success: bool) -> bool {
        if self
            .pending
            .get(label)
            .is_none_or(|(expected, _)| expected != token)
        {
            return false;
        }
        let Some((_, send)) = self.pending.remove(label) else {
            return false;
        };
        send.send(success).is_ok()
    }
}
pub(super) fn complete(label: &str, token: &str, success: bool) -> bool {
    gate()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .complete(label, token, success)
}
pub(super) fn request(app: tauri::AppHandle, done: impl FnOnce() + Send + 'static) {
    if super::updates::blocks_exit(&app) {
        return;
    }
    request_inner(app, true, move |saved| {
        if saved {
            done();
        }
    });
}

/// Updates must learn that a draft flush was refused before changing the bundle.
pub(super) fn request_with_result(app: tauri::AppHandle, done: impl FnOnce(bool) + Send + 'static) {
    request_inner(app, false, done);
}

async fn update_download_activity(
    dispatch: impl FnOnce(tokio::sync::oneshot::Sender<bool>) -> Result<(), tauri::Error>,
    timeout: std::time::Duration,
) -> Result<bool, super::updates::UpdateRetryReason> {
    let (send, receive) = tokio::sync::oneshot::channel();
    dispatch(send).map_err(|_| super::updates::UpdateRetryReason::ActivityCheckFailed)?;
    tokio::time::timeout(timeout, receive)
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(super::updates::UpdateRetryReason::ActivityCheckFailed)
}

pub(super) fn confirm_update_relaunch(
    app: tauri::AppHandle,
    done: impl FnOnce(Result<bool, super::updates::UpdateRetryReason>) + Send + 'static,
) {
    tauri::async_runtime::spawn(async move {
        let downloads = update_download_activity(
            |send| {
                app.run_on_main_thread(move || {
                    #[cfg(any(target_os = "macos", target_os = "windows"))]
                    zephium_engine::downloads_in_progress(move |active| {
                        let _ = send.send(active);
                    });
                    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                    let _ = send.send(false);
                })
            },
            std::time::Duration::from_secs(15),
        )
        .await;
        let downloads = match downloads {
            Ok(downloads) => downloads,
            Err(reason) => {
                done(Err(reason));
                return;
            }
        };
        #[cfg(feature = "work-product")]
        let work = app
            .try_state::<super::work_product::WorkProductState>()
            .is_some_and(|owner| owner.operations.running());
        #[cfg(not(feature = "work-product"))]
        let work = false;
        confirm_update_activity(
            downloads,
            work,
            move |message, answer| {
                use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogKind};
                app.dialog()
                    .message(message)
                    .title(menu_text::label("Relaunch to update?"))
                    .kind(MessageDialogKind::Warning)
                    .buttons(MessageDialogButtons::OkCancelCustom(
                        menu_text::label("Relaunch"),
                        menu_text::label("Cancel"),
                    ))
                    .show(answer);
            },
            move |confirmed| done(Ok(confirmed)),
        );
    });
}

fn confirm_update_activity(
    downloads: bool,
    work: bool,
    ask: impl FnOnce(String, Box<dyn FnOnce(bool) + Send>),
    done: impl FnOnce(bool) + Send + 'static,
) {
    let (key, english) = match (downloads, work) {
        (false, false) => {
            done(true);
            return;
        }
        (true, true) => (
            "native_relaunch_stops_downloads_work",
            "Downloads in progress will stop, and running Work will be stopped.",
        ),
        (true, false) => (
            "native_relaunch_stops_downloads",
            "Downloads in progress will stop.",
        ),
        (false, true) => (
            "native_relaunch_stops_work",
            "Running Work will be stopped.",
        ),
    };
    ask(menu_text::message(key, english), Box::new(done));
}

pub(super) fn cancel_prepared(app: &tauri::AppHandle) {
    let flushed = {
        let mut gate = gate().lock().unwrap_or_else(|e| e.into_inner());
        gate.prepared = false;
        std::mem::take(&mut gate.flushed)
    };
    for (label, token) in flushed {
        super::emit_to_privileged(app, &label, "zephium:resource-close-cancelled", &token);
    }
}

/// The OS budget is absolute and shared with any renderer flush already in flight.
/// A failed/unproven flush still completes the callback, without a user dialog.
pub(super) fn request_system(
    app: tauri::AppHandle,
    flush_deadline: std::time::Instant,
    done: impl FnOnce(bool) + Send + 'static,
) {
    request_flush(app, true, Some(flush_deadline), Box::new(done));
}

fn request_inner(app: tauri::AppHandle, terminal: bool, done: impl FnOnce(bool) + Send + 'static) {
    request_flush(app, terminal, None, Box::new(done));
}

fn request_flush(
    app: tauri::AppHandle,
    terminal: bool,
    system_deadline: Option<std::time::Instant>,
    done: FlushDone,
) {
    let mut gate = gate().lock().unwrap_or_else(|e| e.into_inner());
    let done = if let Some(deadline) = system_deadline {
        let Some(done) = gate.admit_system(deadline, super::shutdown_started(&app), done) else {
            return;
        };
        done
    } else {
        if super::shutdown_started(&app) {
            drop(gate);
            done(true);
            return;
        }
        done
    };
    if gate.round.is_some() || (system_deadline.is_none() && gate.system_closing) {
        drop(gate);
        done(false);
        return;
    }
    if terminal && gate.prepared {
        // The update's successful flush froze all resource admission. Consume
        // that proof while publishing terminal shutdown, rather than asking
        // frozen renderers to save a second time after bundle replacement.
        gate.prepared = false;
        if system_deadline.is_some() {
            gate.system_result = Some(true);
        }
        done(true);
        return;
    }
    if gate.touched.is_empty() {
        gate.prepared = !terminal;
        if system_deadline.is_some() {
            gate.system_result = Some(true);
        }
        // Terminal callers publish shutdown while admission is still locked.
        if !terminal {
            drop(gate);
        }
        done(true);
        return;
    }
    let until = system_deadline
        .unwrap_or_else(|| std::time::Instant::now() + std::time::Duration::from_secs(15));
    if std::time::Instant::now() >= until {
        if system_deadline.is_some() {
            gate.system_result = Some(false);
            done(false);
            return;
        }
        drop(gate);
        done(false);
        return;
    }
    let (mut round, deadline) = FlushRound::new(until);
    let done = if system_deadline.is_some() {
        round.join_system(until, done);
        None
    } else {
        Some(done)
    };
    gate.round = Some(round);
    let mut participants = Vec::new();
    let (requests, receivers) = gate.enlist_new(
        &mut participants,
        |label| app.get_webview_window(label).is_some(),
        || zephium_core::ids::ResourceId::generate().to_string(),
    );
    drop(gate);
    if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
        panel.wake();
    }
    for (label, token) in &requests {
        super::emit_to_privileged(&app, label, "zephium:resource-close", token);
    }
    tauri::async_runtime::spawn(async move {
        let mut participants = participants;
        let mut requests = requests;
        let mut receivers = receivers;
        loop {
            let completed = wait_for_flush(receivers, deadline.clone()).await;
            let retry = {
                let mut gate = crate::resource_close::gate()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if gate.needs_system_wave(completed, &participants, std::time::Instant::now()) {
                    // Keep the same round, original replies and absolute budget.
                    // Only a newly owning host needs another renderer request.
                    let (additional, waiting) = gate.enlist_new(
                        &mut participants,
                        |label| app.get_webview_window(label).is_some(),
                        || zephium_core::ids::ResourceId::generate().to_string(),
                    );
                    drop(gate);
                    if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
                        panel.wake();
                    }
                    for (label, token) in &additional {
                        super::emit_to_privileged(&app, label, "zephium:resource-close", token);
                    }
                    requests.extend(additional);
                    receivers = waiting;
                    continue;
                }
                let round = gate.round.take().expect("flush worker owns its round");
                let retry = completed && gate.requires_another_flush(&participants);
                if round.system {
                    // OS callbacks publish terminal admission under this lock.
                    gate.finish_system(round, completed, &participants, &requests);
                    drop(gate);
                    if let Some(done) = done {
                        // OS takeover cannot authorize an update installation or
                        // a competing user-quit completion, even after a saved flush.
                        done(false);
                    }
                    return;
                }
                gate.pending.clear();
                if completed && !retry {
                    // Linearize the final participant check with first resource
                    // calls from a newly initialized launcher. Native shutdown's
                    // terminal marker is published synchronously by this callback.
                    gate.prepared = !terminal;
                    gate.flushed = requests
                        .iter()
                        .map(|(label, token)| ((*label).to_owned(), token.clone()))
                        .collect();
                    if !terminal {
                        drop(gate);
                    }
                    if let Some(done) = done {
                        done(true);
                    }
                    return;
                }
                retry
            };
            let Some(done) = done else {
                return;
            };
            if retry {
                // A cold host began using resources while the other host flushed.
                // A normal user close retains its existing retry behavior.
                request_inner(app, terminal, done);
            } else if !super::shutdown_started(&app) {
                if crate::resource_close::gate()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .system_closing
                {
                    done(false);
                    return;
                }
                for (label, token) in &requests {
                    super::emit_to_privileged(
                        &app,
                        label,
                        "zephium:resource-close-cancelled",
                        token,
                    );
                }
                if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
                    if !panel.snapshot().visible {
                        panel.hide();
                    }
                }
                // A failed user save still lets the person decide whether to leave.
                ask_to_quit_anyway(&app, move |quit| {
                    let mut gate = crate::resource_close::gate()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    if !gate.resolve_user_choice(quit, terminal, &requests) {
                        drop(gate);
                        done(false);
                        return;
                    }
                    if !terminal {
                        drop(gate);
                    }
                    done(true);
                });
            } else {
                done(false);
            }
            return;
        }
    });
}

fn ask_to_quit_anyway(app: &tauri::AppHandle, answer: impl FnOnce(bool) + Send + 'static) {
    use tauri_plugin_dialog::{MessageDialogButtons, MessageDialogKind};
    app.dialog()
        .message(menu_text::message(
            "native_unsaved_changes_message",
            "Some changes in Notes, Tasks or Work could not be saved. Go back to retry or resolve a conflict, or quit anyway and lose them. Changes in another profile can only be saved from that profile.",
        ))
        .title(menu_text::label("Unsaved changes"))
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            menu_text::label("Quit Anyway"),
            menu_text::label("Cancel"),
        ))
        .show(answer);
}

pub(super) fn is_closing() -> bool {
    let gate = gate().lock().unwrap_or_else(|e| e.into_inner());
    gate.round.is_some() || gate.prepared || gate.system_closing
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn update_activity_query_failures_are_retryable_and_distinct_from_cancel() {
        use super::super::updates::UpdateRetryReason::ActivityCheckFailed;
        let timeout = std::time::Duration::from_millis(1);
        assert_eq!(
            update_download_activity(
                |_| Err(tauri::Error::Io(std::io::Error::other("dispatch failed"))),
                timeout
            )
            .await,
            Err(ActivityCheckFailed),
            "dispatch failure is not a user cancel",
        );
        assert_eq!(
            update_download_activity(|_| Ok(()), timeout).await,
            Err(ActivityCheckFailed),
            "a dropped reply is not a user cancel",
        );
        let mut pending = None;
        assert_eq!(
            update_download_activity(
                |send| {
                    pending = Some(send);
                    Ok(())
                },
                timeout
            )
            .await,
            Err(ActivityCheckFailed),
            "timeout is not a user cancel",
        );
        for active in [true, false] {
            assert_eq!(
                update_download_activity(
                    |send| {
                        send.send(active).unwrap();
                        Ok(())
                    },
                    timeout
                )
                .await,
                Ok(active),
            );
        }
    }

    #[test]
    fn update_relaunch_requires_confirmation_only_for_active_downloads_or_work() {
        for (downloads, work, expected) in [
            (false, false, None),
            (true, false, Some("Downloads in progress will stop.")),
            (false, true, Some("Running Work will be stopped.")),
            (
                true,
                true,
                Some("Downloads in progress will stop, and running Work will be stopped."),
            ),
        ] {
            for confirmed in [true, false] {
                let (send, receive) = std::sync::mpsc::channel();
                confirm_update_activity(
                    downloads,
                    work,
                    |message, answer| {
                        assert_eq!(Some(message.as_str()), expected);
                        answer(confirmed);
                    },
                    move |proceed| {
                        send.send(proceed).unwrap();
                    },
                );
                assert_eq!(receive.recv().unwrap(), expected.is_none() || confirmed);
            }
        }
        let (send, receive) = std::sync::mpsc::channel();
        let mut pending = None;
        confirm_update_activity(
            true,
            true,
            |_, answer| pending = Some(answer),
            move |proceed| {
                send.send(proceed).unwrap();
            },
        );
        assert!(receive.try_recv().is_err(), "wait for the person's choice");
        pending.unwrap()(false);
        assert!(!receive.recv().unwrap());
    }

    #[test]
    fn prepared_install_refuses_new_drafts_until_cancelled() {
        let mut gate = Gate {
            prepared: true,
            ..Gate::default()
        };
        assert!(!gate.touch(super::super::overlay::PANEL_LABEL, || false));
        assert!(gate.participants().is_empty());
        gate.prepared = false;
        assert!(gate.touch(super::super::overlay::PANEL_LABEL, || false));
    }

    #[test]
    fn leaving_unsaved_work_closes_admission_like_a_successful_flush() {
        let requests = [("main", "token".to_owned())];
        for terminal in [true, false] {
            let mut gate = Gate::default();
            gate.leave_unsaved(terminal, &requests);
            assert_eq!(gate.prepared, !terminal);
            assert_eq!(gate.flushed, [("main".to_owned(), "token".to_owned())]);
        }
    }

    #[test]
    fn main_resources_do_not_enlist_the_unloaded_launcher() {
        let mut gate = Gate::default();
        assert!(gate.participants().is_empty());
        gate.touched.insert(super::super::MAIN_LABEL.into());
        assert_eq!(gate.participants(), [super::super::MAIN_LABEL]);
    }

    #[test]
    fn a_touched_launcher_still_participates_after_it_is_hidden() {
        let mut gate = Gate::default();
        gate.touched.insert(super::super::MAIN_LABEL.into());
        gate.touched
            .insert(super::super::overlay::PANEL_LABEL.into());
        assert_eq!(
            gate.participants(),
            [super::super::MAIN_LABEL, super::super::overlay::PANEL_LABEL]
        );
    }

    #[test]
    fn a_launcher_first_touched_during_close_requires_its_own_flush() {
        let mut gate = Gate::default();
        assert!(gate.touch(super::super::MAIN_LABEL, || false));
        let participants = gate.participants();
        assert!(!gate.requires_another_flush(&participants));
        assert!(gate.touch(super::super::overlay::PANEL_LABEL, || false));
        assert!(gate.requires_another_flush(&participants));
        // After the retry includes both hosts, ordinary save calls from those
        // same hosts do not invalidate a successfully completed flush.
        assert!(!gate.requires_another_flush(&gate.participants()));
    }

    #[test]
    fn a_caller_waiting_on_final_close_rechecks_terminal_admission() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        };
        let gate = Arc::new(Mutex::new(Gate::default()));
        let terminal = Arc::new(AtomicBool::new(false));
        let closing = gate.lock().expect("final close owns the gate");
        let (checked, receive) = mpsc::sync_channel(1);
        let caller_gate = gate.clone();
        let caller_terminal = terminal.clone();
        let caller = std::thread::spawn(move || {
            // Match a native caller that passed its early shutdown check,
            // then waited for final close's participant/shutdown transition.
            assert!(!caller_terminal.load(Ordering::Acquire));
            checked.send(()).expect("report stale early check");
            let mut admission = caller_gate.lock().expect("resource call owns the gate");
            admission.touch(super::super::overlay::PANEL_LABEL, || {
                caller_terminal.load(Ordering::Acquire)
            })
        });
        receive
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("early check completed");
        assert!(closing.participants().is_empty());
        // The native close callback publishes this marker while holding Gate.
        terminal.store(true, Ordering::Release);
        drop(closing);
        assert!(!caller.join().expect("resource caller completed"));
        assert!(gate
            .lock()
            .expect("inspect participants")
            .participants()
            .is_empty());
    }

    #[test]
    fn a_failed_launcher_flush_cannot_be_overridden_by_the_main_reply() {
        let mut gate = Gate::default();
        let (main_send, mut main_receive) = tokio::sync::oneshot::channel();
        let (panel_send, mut panel_receive) = tokio::sync::oneshot::channel();
        gate.pending
            .insert("main".into(), ("main-token".into(), main_send));
        gate.pending
            .insert("panel".into(), ("panel-token".into(), panel_send));
        assert!(gate.complete("panel", "panel-token", false));
        assert!(gate.complete("main", "main-token", true));
        assert_eq!(main_receive.try_recv(), Ok(true));
        assert_eq!(panel_receive.try_recv(), Ok(false));
    }

    #[test]
    fn replies_are_bound_to_the_requesting_window_and_current_token() {
        let mut gate = Gate::default();
        let (send, mut receive) = tokio::sync::oneshot::channel();
        gate.pending.insert("main".into(), ("current".into(), send));
        assert!(!gate.complete("panel", "current", true));
        assert!(!gate.complete("main", "old", true));
        assert!(receive.try_recv().is_err());
        assert!(gate.complete("main", "current", true));
        assert_eq!(receive.try_recv(), Ok(true));
        assert!(!gate.complete("main", "current", true));
    }

    #[tokio::test]
    async fn system_join_keeps_current_tokens_earliest_deadline_and_all_callbacks_once() {
        let now = std::time::Instant::now();
        let mut gate = Gate::default();
        gate.touched.insert(super::super::MAIN_LABEL.into());
        let participants = gate.participants();
        let (round, deadline) = FlushRound::new(now + std::time::Duration::from_secs(15));
        gate.round = Some(round);
        let (send, receive) = tokio::sync::oneshot::channel();
        gate.pending.insert("main".into(), ("current".into(), send));
        let (callbacks, answers) = std::sync::mpsc::channel();
        let earliest = now + std::time::Duration::from_secs(2);
        for until in [earliest, now + std::time::Duration::from_secs(5)] {
            let callbacks = callbacks.clone();
            assert!(gate
                .admit_system(
                    until,
                    false,
                    Box::new(move |saved| {
                        callbacks.send(saved).unwrap();
                    })
                )
                .is_none());
        }
        assert_eq!(*deadline.borrow(), earliest);
        assert_eq!(gate.pending.len(), 1);
        assert_eq!(gate.pending["main"].0, "current");
        assert!(
            answers.try_recv().is_err(),
            "join waits for the original replies"
        );
        assert!(gate.complete("main", "current", true));
        assert!(wait_for_flush(vec![receive], deadline).await);
        let round = gate.round.take().unwrap();
        assert!(gate.finish_system(round, true, &participants, &[("main", "current".into())]));
        assert_eq!(answers.try_iter().collect::<Vec<_>>(), [true, true]);
        assert!(
            !gate.complete("main", "current", true),
            "late reply is retired"
        );
        let callbacks = callbacks.clone();
        assert!(gate
            .admit_system(
                now + std::time::Duration::from_secs(30),
                false,
                Box::new(move |saved| callbacks.send(saved).unwrap())
            )
            .is_none());
        assert_eq!(answers.try_iter().collect::<Vec<_>>(), [true]);
        assert!(
            gate.round.is_none(),
            "settled OS shutdown cannot restart a flush"
        );
    }

    #[tokio::test]
    async fn system_join_wakes_an_already_waiting_normal_deadline() {
        let now = std::time::Instant::now();
        let (mut round, deadline) = FlushRound::new(now + std::time::Duration::from_secs(60));
        let (_hold, receive) = tokio::sync::oneshot::channel();
        let mut waiting = Box::pin(wait_for_flush(vec![receive], deadline));
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        round.join_system(now - std::time::Duration::from_secs(1), Box::new(|_| {}));
        assert!(
            !tokio::time::timeout(std::time::Duration::from_millis(100), waiting)
                .await
                .expect("the shortened absolute deadline wakes the existing wait")
        );
    }

    #[tokio::test]
    async fn system_flush_needs_every_reply_before_the_absolute_deadline() {
        for results in [vec![true, true], vec![false, true], vec![true, false]] {
            let (round, deadline) =
                FlushRound::new(std::time::Instant::now() + std::time::Duration::from_secs(60));
            let mut receivers = Vec::new();
            for saved in &results {
                let (send, receive) = tokio::sync::oneshot::channel();
                send.send(*saved).unwrap();
                receivers.push(receive);
            }
            assert_eq!(
                wait_for_flush(receivers, deadline).await,
                results.iter().all(|saved| *saved)
            );
            drop(round);
        }
        let (_round, deadline) = FlushRound::new(std::time::Instant::now());
        let (send, receive) = tokio::sync::oneshot::channel();
        send.send(true).unwrap();
        assert!(
            !wait_for_flush(vec![receive], deadline).await,
            "late success is unproven"
        );
        let (_round, deadline) =
            FlushRound::new(std::time::Instant::now() + std::time::Duration::from_secs(60));
        let (send, receive) = tokio::sync::oneshot::channel();
        drop(send);
        assert!(
            !wait_for_flush(vec![receive], deadline).await,
            "closed renderer cannot attest success"
        );
    }

    #[test]
    fn system_deadline_caps_a_newly_touched_host_without_restarting() {
        let now = std::time::Instant::now();
        let mut gate = Gate::default();
        assert!(gate.touch(super::super::MAIN_LABEL, || false));
        let participants = gate.participants();
        let (round, deadline) = FlushRound::new(now + std::time::Duration::from_secs(15));
        gate.round = Some(round);
        let (send, receive) = std::sync::mpsc::channel();
        let earliest = now + std::time::Duration::from_secs(2);
        assert!(gate
            .admit_system(
                earliest,
                false,
                Box::new(move |saved| send.send(saved).unwrap())
            )
            .is_none());
        assert!(
            gate.touch(super::super::overlay::PANEL_LABEL, || false),
            "close-task IPC mutations still enter during renderer flush"
        );
        assert!(gate.needs_system_wave(true, &participants, now));
        assert!(!gate.needs_system_wave(true, &participants, earliest));
        let round = gate.round.take().unwrap();
        assert!(
            !gate.finish_system(round, true, &participants, &[]),
            "a newly owning host has not supplied flush proof"
        );
        assert_eq!(receive.try_recv(), Ok(false));
        assert_eq!(*deadline.borrow(), earliest);
        let (send, receive) = std::sync::mpsc::channel();
        assert!(gate
            .admit_system(
                now + std::time::Duration::from_secs(15),
                false,
                Box::new(move |saved| send.send(saved).unwrap())
            )
            .is_none());
        assert_eq!(receive.try_recv(), Ok(false));
        assert!(gate.round.is_none());
        assert_eq!(*deadline.borrow(), earliest);
        assert!(
            !gate.touch(super::super::MAIN_LABEL, || true),
            "native terminal admission closes writes after flush settlement"
        );
    }

    #[test]
    fn late_unsaved_prompt_answer_cannot_override_system_shutdown() {
        let mut gate = Gate::default();
        let requests = [("main", "old-user-token".to_owned())];
        let (normal, answer) = std::sync::mpsc::channel();
        let late_prompt = move |gate: &mut Gate| {
            normal
                .send(gate.resolve_user_choice(true, false, &requests))
                .unwrap();
        };
        let (system, result) = std::sync::mpsc::channel();
        let deadline = std::time::Instant::now();
        let initial = gate
            .admit_system(
                deadline,
                false,
                Box::new(move |saved| system.send(saved).unwrap()),
            )
            .unwrap();
        let (mut round, _) = FlushRound::new(deadline);
        round.join_system(deadline, initial);
        assert!(!gate.finish_system(round, false, &[], &[]));
        assert_eq!(result.try_recv(), Ok(false));
        late_prompt(&mut gate);
        assert_eq!(answer.try_recv(), Ok(false));
        assert!(!gate.prepared, "late answer cannot freeze/admit an update");
        assert!(gate.flushed.is_empty());
        assert!(gate.round.is_none());
        assert_eq!(gate.system_result, Some(false));
    }

    #[test]
    fn terminal_teardown_is_not_proof_of_a_renderer_flush() {
        let mut gate = Gate::default();
        let (send, result) = std::sync::mpsc::channel();
        assert!(gate
            .admit_system(
                std::time::Instant::now(),
                true,
                Box::new(move |saved| send.send(saved).unwrap())
            )
            .is_none());
        assert_eq!(result.try_recv(), Ok(false));
        assert_eq!(gate.system_result, Some(false));
        assert!(gate.round.is_none());
    }

    #[tokio::test]
    async fn system_round_flushes_a_late_launcher_with_its_original_budget() {
        let now = std::time::Instant::now();
        let mut gate = Gate::default();
        gate.touch(super::super::MAIN_LABEL, || false);
        let (round, deadline) = FlushRound::new(now + std::time::Duration::from_secs(15));
        gate.round = Some(round);
        let mut participants = Vec::new();
        let (mut requests, waiting) =
            gate.enlist_new(&mut participants, |_| true, || "main-token".into());
        let (send, answer) = std::sync::mpsc::channel();
        let earliest = now + std::time::Duration::from_secs(2);
        assert!(gate
            .admit_system(
                earliest,
                false,
                Box::new(move |saved| send.send(saved).unwrap())
            )
            .is_none());
        assert!(gate.touch(super::super::overlay::PANEL_LABEL, || false));
        assert!(gate.complete(super::super::MAIN_LABEL, "main-token", true));
        assert!(wait_for_flush(waiting, deadline.clone()).await);
        assert!(gate.needs_system_wave(true, &participants, now));
        let (additional, waiting) =
            gate.enlist_new(&mut participants, |_| true, || "launcher-token".into());
        assert_eq!(
            additional,
            [(
                super::super::overlay::PANEL_LABEL,
                "launcher-token".to_owned()
            )]
        );
        requests.extend(additional);
        assert_eq!(
            requests[0],
            (super::super::MAIN_LABEL, "main-token".to_owned())
        );
        assert!(!gate.pending.contains_key(super::super::MAIN_LABEL));
        assert_eq!(gate.pending.len(), 1);
        assert_eq!(*deadline.borrow(), earliest);
        assert!(
            answer.try_recv().is_err(),
            "main success alone is not whole-round proof"
        );
        assert!(gate.complete(super::super::overlay::PANEL_LABEL, "launcher-token", true));
        assert!(wait_for_flush(waiting, deadline.clone()).await);
        assert!(!gate.needs_system_wave(true, &participants, now));
        let round = gate.round.take().unwrap();
        assert!(gate.finish_system(round, true, &participants, &requests));
        assert_eq!(answer.try_recv(), Ok(true));
        assert_eq!(gate.flushed.len(), 2);
        assert_eq!(*deadline.borrow(), earliest);
    }

    #[tokio::test]
    async fn an_os_deadline_joining_after_reply_collection_still_caps_success() {
        let now = std::time::Instant::now();
        let mut gate = Gate::default();
        let (round, deadline) = FlushRound::new(now + std::time::Duration::from_secs(60));
        gate.round = Some(round);
        let (send, receive) = tokio::sync::oneshot::channel();
        send.send(true).unwrap();
        assert!(wait_for_flush(vec![receive], deadline).await);
        let (send, answer) = std::sync::mpsc::channel();
        assert!(gate
            .admit_system(
                now - std::time::Duration::from_secs(1),
                false,
                Box::new(move |saved| send.send(saved).unwrap())
            )
            .is_none());
        let round = gate.round.take().unwrap();
        assert!(
            !gate.finish_system(round, true, &[], &[]),
            "an earlier deadline admitted before final settlement remains authoritative"
        );
        assert_eq!(answer.try_recv(), Ok(false));
    }

    #[tokio::test]
    async fn a_touched_missing_renderer_cannot_imply_successful_system_flush() {
        let now = std::time::Instant::now();
        let mut gate = Gate::default();
        gate.touch(super::super::MAIN_LABEL, || false);
        let (round, deadline) = FlushRound::new(now + std::time::Duration::from_secs(15));
        gate.round = Some(round);
        let mut participants = Vec::new();
        let (requests, waiting) = gate.enlist_new(
            &mut participants,
            |_| false,
            || panic!("missing native host cannot receive a token"),
        );
        assert_eq!(participants, [super::super::MAIN_LABEL]);
        assert!(requests.is_empty());
        assert!(waiting.is_empty());
        let (send, answer) = std::sync::mpsc::channel();
        assert!(gate
            .admit_system(
                now + std::time::Duration::from_secs(2),
                false,
                Box::new(move |saved| send.send(saved).unwrap())
            )
            .is_none());
        assert!(
            wait_for_flush(waiting, deadline).await,
            "no nonexistent renderer callback is awaited"
        );
        let round = gate.round.take().unwrap();
        assert!(
            !gate.finish_system(round, true, &participants, &requests),
            "a touched host without a live renderer has no save attestation"
        );
        assert_eq!(answer.try_recv(), Ok(false));
        assert!(gate.flushed.is_empty());
    }
}
