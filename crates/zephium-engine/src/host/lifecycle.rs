#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "windows")]
use std::sync::Arc;

use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::EngineEvent;

use super::permits::EventPermit;
#[cfg(target_os = "windows")]
use super::profiles::{
    windows_erasure_provenance_is_consistent, windows_profile_provenance_presence_is_consistent,
};
use super::EngineHost;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RendererCrashTarget {
    Spare,
    Live,
    Retired,
}

fn renderer_crash_target(spare: Option<ItemId>, live: bool, id: ItemId) -> RendererCrashTarget {
    if spare == Some(id) {
        RendererCrashTarget::Spare
    } else if live {
        RendererCrashTarget::Live
    } else {
        RendererCrashTarget::Retired
    }
}

impl EngineHost {
    pub(super) fn has_live_profile_view(&self, profile: ProfileId) -> bool {
        self.partitions
            .iter()
            .any(|(id, partition)| partition.profile() == profile && self.views.contains_key(id))
    }

    fn close_idle_spare(&mut self, profile: ProfileId) {
        if self.has_live_profile_view(profile) {
            return;
        }
        let Some(spare) = self
            .spare
            .take_if(|spare| spare.partition.profile() == profile)
        else {
            return;
        };

        #[cfg(target_os = "windows")]
        {
            // Controller::Close is the documented trigger for normal
            // BrowserProcessExited once no same-environment controls remain.
            // Keep the Environment5 observer and exact process HANDLE until
            // that event independently proves the process group released its
            // UDF; only the path-only Wry context can be retired immediately.
            self.web_contexts.remove(&profile);
            let (debt, policy_cleanup_failed) = spare.view.close_explicit();
            if policy_cleanup_failed {
                self.fail_content_policy_retirement();
            }
            if let Some(debt) = debt {
                self.retain_windows_cleanup_debt(profile, debt);
            }
        }
        #[cfg(not(target_os = "windows"))]
        drop(spare);
    }

    pub(crate) fn close(&mut self, id: ItemId) {
        #[cfg(target_os = "macos")]
        {
            self.discarded_states.remove(&id);
            self.prepared_discard_states.remove(&id);
            self.restore_snapshots.remove(&id);
        }
        #[cfg(target_os = "macos")]
        self.webext.close_page(id);
        #[cfg(target_os = "macos")]
        self.revoke_page_permission_requests_for_close(id);
        let profile = self
            .partitions
            .get(&id)
            .map(|partition| partition.profile());
        #[cfg(target_os = "macos")]
        if let Some(profile) = profile {
            if !self.unbind_extension_browser_surface_view(profile, id) {
                self.native_resource_accounting_failed = true;
                (self.native_terminal_failure)(
                    "macOS extension browser surface could not unbind a retiring view",
                );
            }
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        self.forget_fullscreen(id);
        let removed = self.views.remove(&id);
        self.navigation_snapshots.remove(&id);
        self.partitions.remove(&id);
        #[cfg(target_os = "windows")]
        {
            self.hidden.remove(&id);
            self.dormant.remove(&id);
            self.desired_dormant.remove(&id);
            self.suspending.remove(&id);
            self.suspend_failed.remove(&id);
            self.suspend_uncertain.remove(&id);
            self.styles_missed.remove(&id);
        }
        #[cfg(target_os = "macos")]
        {
            self.dormant.remove(&id);
            self.styles_missed.remove(&id);
        }
        for stage in self.stages.values() {
            stage.remove_view(id);
        }
        #[cfg(target_os = "windows")]
        if let (Some(profile), Some(view)) = (profile, removed) {
            let view = if let Some(downloads) = &self.downloads {
                match downloads.retain_closed_view(view) {
                    None => {
                        self.close_idle_spare(profile);
                        return;
                    }
                    Some(view) => view,
                }
            } else {
                view
            };
            let (debt, policy_cleanup_failed) = view.close_explicit();
            if policy_cleanup_failed {
                self.fail_content_policy_retirement();
            }
            if let Some(debt) = debt {
                self.retain_windows_cleanup_debt(profile, debt);
            }
        }
        #[cfg(target_os = "macos")]
        match removed {
            Some(view)
                if crate::platform::macos::fullscreen::in_transition(
                    &crate::platform::imp::native_webview(&view.view),
                ) =>
            {
                self.retire_fullscreen_view(view)
            }
            removed => drop(removed),
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        drop(removed);
        if let Some(profile) = profile {
            self.close_idle_spare(profile);
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub(super) fn shutdown(&mut self, done: Box<dyn FnOnce(bool) + Send>) {
        if self.shutdown_completion.is_some() {
            done(false);
            return;
        }
        #[cfg(target_os = "macos")]
        let done = if let Some(downloads) = &self.downloads {
            let (native_done, download_done) = join_download_shutdown(done);
            downloads.quiesce(None, download_done);
            native_done
        } else {
            done
        };
        self.shutdown_completion = Some(done);
        self.shutdown_common();
        self.finish_content_policy_shutdown_if_quiescent();
    }

    #[cfg(target_os = "windows")]
    pub(super) fn shutdown(
        &mut self,
    ) -> (
        Vec<crate::platform::imp::BrowserProcessShutdownObligation>,
        bool,
    ) {
        self.shutdown_common();
        self.retry_windows_cleanup_debts(3);

        let mut provenance_valid = self.unverifiable_browser_processes.is_empty()
            && self.construction_unproven.is_empty()
            && self.unproven_browser_processes.is_empty()
            && self.unproven_environments.is_empty()
            && self.windows_cleanup_debts.is_empty()
            && !self.windows_cleanup_invariant_failed
            && !self.native_resource_accounting_failed
            && self.native_resources.is_quiescent()
            && self
                .environments
                .keys()
                .chain(self.browser_processes.keys())
                .chain(self.browser_process_exit_observers.keys())
                .chain(self.browser_version_observers.keys())
                .all(|profile| {
                    windows_profile_provenance_presence_is_consistent(
                        self.environments.contains_key(profile),
                        self.browser_processes.contains_key(profile),
                        self.browser_process_exit_observers.contains_key(profile),
                        self.browser_version_observers.contains_key(profile),
                    ) || windows_erasure_provenance_is_consistent(
                        self.environments.contains_key(profile),
                        self.browser_version_observers.contains_key(profile),
                        self.erasure_tombstones.contains(profile),
                        self.browser_processes
                            .get(profile)
                            .map(crate::platform::imp::BrowserProcess::id),
                        self.browser_process_exit_observers
                            .get(profile)
                            .map(|observer| {
                                (observer.expected_process_id(), observer.is_invalid())
                            }),
                    )
                });
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        {
            use super::resources::NativeResourceClass as Class;
            crate::platform::imp::diagnose_shutdown_admission(
                [
                    self.unverifiable_browser_processes.is_empty(),
                    self.construction_unproven.is_empty(),
                    self.unproven_browser_processes.is_empty(),
                    self.unproven_environments.is_empty(),
                    self.windows_cleanup_debts.is_empty(),
                    !self.windows_cleanup_invariant_failed,
                    !self.native_resource_accounting_failed,
                    self.native_resources.is_quiescent(),
                    provenance_valid,
                    self.browser_processes.len() == self.browser_process_exit_observers.len(),
                ],
                [
                    Class::Tab,
                    Class::WarmSpare,
                    Class::TeardownDebt,
                    Class::AgentContext,
                    Class::Extension,
                    Class::TransientConstruction,
                ]
                .map(|class| self.native_resources.count_for_audit(class)),
            );
        }
        self.windows_process_shutdown_started = true;
        let mut obligations = Vec::with_capacity(self.browser_processes.len());
        for (profile, process) in self.browser_processes.drain() {
            let Some(proof) = self
                .browser_process_exit_observers
                .get(&profile)
                .map(crate::platform::imp::BrowserProcessExitObserver::proof)
            else {
                provenance_valid = false;
                continue;
            };
            match crate::platform::imp::BrowserProcessShutdownObligation::new(process, proof) {
                Some(obligation) => obligations.push(obligation),
                None => provenance_valid = false,
            }
        }
        // Releasing controllers and these ordinary environment references
        // initiates normal runtime shutdown. Observer guards intentionally
        // remain UI-thread-owned until process exit signals their proofs.
        self.browser_version_observers.clear();
        self.environments.clear();
        self.exiting_browser_processes.clear();
        self.pending_profile_recovery.clear();
        self.hidden.clear();
        self.dormant.clear();
        self.desired_dormant.clear();
        self.suspending.clear();
        self.suspend_failed.clear();
        self.suspend_uncertain.clear();
        (obligations, provenance_valid)
    }

    fn shutdown_common(&mut self) {
        #[cfg(target_os = "macos")]
        {
            self.webext.cancel_auth_flows();
            self.discarded_states.clear();
            self.prepared_discard_states.clear();
        }
        #[cfg(target_os = "windows")]
        self.shutdown_windows_extensions();
        #[cfg(all(
            feature = "agentic-browser",
            any(target_os = "macos", target_os = "windows")
        ))]
        if !self.force_shutdown_agent_contexts() {
            // Physical teardown still completes, but clean shutdown requires
            // the shell to have settled every exact Close before this barrier.
            self.native_resource_accounting_failed = true;
        }
        #[cfg(target_os = "windows")]
        if let Some(downloads) = &self.downloads {
            for view in downloads.take_retained_views() {
                let profile = view.cleanup_profile;
                let (debt, failed) = view.close_explicit();
                if failed {
                    self.fail_content_policy_retirement();
                }
                if let Some(debt) = debt {
                    self.retain_windows_cleanup_debt(profile, debt);
                }
            }
        }
        let ids: Vec<ItemId> = self.views.keys().copied().collect();
        for id in ids {
            self.close(id);
        }
        // Shutdown cannot wait for WebKit to animate a fullscreen page home.
        #[cfg(target_os = "macos")]
        self.fullscreen_retiring.clear();
        self.spare = None;
        self.begin_content_policy_shutdown();
        self.navigation_snapshots.clear();
        self.partitions.clear();
        self.extension_browser_surfaces.clear();
        // Release native composition roots as part of the shutdown barrier.
        // Popups are separate native windows and macOS' parent view retains
        // subviews, so clearing only the Rust map is not sufficient.
        #[cfg(target_os = "macos")]
        for stage in self.stages.values() {
            stage.set_drop_indicator(None);
            stage.removeFromSuperview();
        }
        #[cfg(not(target_os = "macos"))]
        for stage in self.stages.values() {
            stage.set_drop_indicator(None);
        }
        self.stages.clear();
        #[cfg(target_os = "macos")]
        self.macos_ephemeral_data_stores.clear();
        #[cfg(all(target_os = "macos", feature = "agentic-browser"))]
        self.anonymous_work_stores.clear();
        #[cfg(all(target_os = "windows", feature = "agentic-browser"))]
        self.anonymous_work_environments.clear();
        #[cfg(all(target_os = "windows", feature = "agentic-browser"))]
        self.work_site_stores.clear();
        #[cfg(not(target_os = "macos"))]
        self.web_contexts.clear();
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.linux_data_managers.clear();
            self.linux_unverifiable_data_managers.clear();
        }
    }

    pub(super) fn on_renderer_process_exit(&mut self, id: ItemId, source_permit: &EventPermit) {
        let spare = self.spare.as_ref().map(|spare| spare.id.get());
        match renderer_crash_target(spare, self.views.contains_key(&id), id) {
            // A spare has no shell item. Drop the dead native object here so
            // it can never be adopted under a future real item id.
            RendererCrashTarget::Spare => {
                if self
                    .spare
                    .as_ref()
                    .is_some_and(|spare| spare.view.event_permit.same_generation(source_permit))
                {
                    self.spare = None;
                }
            }
            RendererCrashTarget::Live => {
                let token = self.views.get(&id).and_then(|view| {
                    view.event_permit
                        .same_generation(source_permit)
                        .then(|| view.event_permit.active_token())
                        .flatten()
                });
                let Some(token) = token else {
                    return;
                };
                // A crash event authorizes the shell to rebuild this logical
                // id. Remove and revoke the exact dead native generation
                // before that event can reach the shell.
                self.close(id);
                self.sink.emit_for(token, EngineEvent::Crashed { id });
            }
            // A callback can race an explicit close. The shell already owns
            // the resulting state transition, so a retired id is a no-op.
            RendererCrashTarget::Retired => {}
        }
    }

    #[cfg(target_os = "windows")]
    pub(super) fn on_stage_placement_failure(&mut self, id: ItemId, generation: &Arc<AtomicBool>) {
        let token = self.views.get(&id).and_then(|view| {
            view.event_permit
                .matches_token(generation)
                .then(|| view.event_permit.active_token())
                .flatten()
        });
        let Some(token) = token else {
            return;
        };
        eprintln!("engine: WebView2 stage placement failed after bounded retries");
        // A controller whose HWND/bounds/visibility contract cannot be
        // established must not remain logically live behind a permanent
        // placeholder. Retire the exact generation before reporting failure.
        self.close(id);
        self.sink
            .emit_for(token, EngineEvent::ViewCreationFailed { id });
    }
}

#[cfg(test)]
mod tests;

#[cfg(any(target_os = "macos", target_os = "windows"))]
type ShutdownPart = Box<dyn FnOnce(bool) + Send>;

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn join_download_shutdown(
    done: Box<dyn FnOnce(bool) + Send>,
) -> (ShutdownPart, ShutdownPart) {
    struct Join {
        left: usize,
        clean: bool,
        done: Option<Box<dyn FnOnce(bool) + Send>>,
    }
    let state = std::sync::Arc::new(std::sync::Mutex::new(Join {
        left: 2,
        clean: true,
        done: Some(done),
    }));
    let part = |state: std::sync::Arc<std::sync::Mutex<Join>>| -> Box<dyn FnOnce(bool) + Send> {
        Box::new(move |clean| {
            let ready = {
                let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
                state.clean &= clean;
                state.left -= 1;
                if state.left == 0 {
                    let clean = state.clean;
                    state.done.take().map(|done| (done, clean))
                } else {
                    None
                }
            };
            if let Some((done, clean)) = ready {
                done(clean);
            }
        })
    };
    (part(state.clone()), part(state))
}
