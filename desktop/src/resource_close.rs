//! User-initiated shutdown waits for bounded renderer draft flushes. Security and
//! forced teardown keep the existing native shutdown path and never wait on UI.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
struct Gate {
    touched: BTreeSet<String>,
    pending: BTreeMap<String, (String, tokio::sync::oneshot::Sender<bool>)>,
    requesting: bool,
    prepared: bool,
    flushed: Vec<(String, String)>,
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

fn request_inner(app: tauri::AppHandle, terminal: bool, done: impl FnOnce(bool) + Send + 'static) {
    if super::shutdown_started(&app) {
        done(true);
        return;
    }
    let mut gate = gate().lock().unwrap_or_else(|e| e.into_inner());
    if gate.requesting {
        drop(gate);
        done(false);
        return;
    }
    if terminal && gate.prepared {
        // The update's successful flush froze all resource admission. Consume
        // that proof while publishing terminal shutdown, rather than asking
        // frozen renderers to save a second time after bundle replacement.
        gate.prepared = false;
        done(true);
        return;
    }
    if gate.touched.is_empty() {
        gate.prepared = !terminal;
        // Terminal callers publish shutdown while admission is still locked.
        if !terminal {
            drop(gate);
        }
        done(true);
        return;
    }
    gate.requesting = true;
    let mut receivers = Vec::new();
    let mut requests = Vec::new();
    let participants = gate.participants();
    for label in participants.iter().copied() {
        if app.get_webview_window(label).is_none() {
            continue;
        }
        let token = zephium_core::ids::ResourceId::generate().to_string();
        let (send, receive) = tokio::sync::oneshot::channel();
        gate.pending.insert(label.into(), (token.clone(), send));
        receivers.push(receive);
        requests.push((label, token));
    }
    drop(gate);
    if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
        panel.wake();
    }
    for (label, token) in &requests {
        super::emit_to_privileged(&app, label, "zephium:resource-close", token);
    }
    tauri::async_runtime::spawn(async move {
        let completed = tokio::time::timeout(std::time::Duration::from_secs(15), async move {
            let mut okay = true;
            for receive in receivers {
                okay = receive.await.unwrap_or(false) && okay;
            }
            okay
        })
        .await
        .unwrap_or(false);
        let retry = {
            let mut gate = crate::resource_close::gate()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let retry = completed && gate.requires_another_flush(&participants);
            gate.pending.clear();
            gate.requesting = false;
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
                done(true);
                return;
            }
            retry
        };
        if retry {
            // A cold host began using resources while the other host flushed.
            // It may now hold a draft, so request a fresh flush from both.
            request_inner(app, terminal, done);
        } else if !super::shutdown_started(&app) {
            for (label, token) in requests {
                super::emit_to_privileged(&app, label, "zephium:resource-close-cancelled", &token);
            }
            if let Some(panel) = app.try_state::<super::overlay::Overlay>() {
                if !panel.snapshot().visible {
                    panel.hide();
                }
            }
            app.dialog().message("Some changes in Notes, Tasks or Work could not be saved. Return to them to retry or resolve a conflict. Drafts in another profile must be saved from that profile.").title("Unsaved changes").kind(tauri_plugin_dialog::MessageDialogKind::Warning).show(|_|{});
            done(false);
        } else {
            done(false);
        }
    });
}

pub(super) fn is_closing() -> bool {
    let gate = gate().lock().unwrap_or_else(|e| e.into_inner());
    gate.requesting || gate.prepared
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
