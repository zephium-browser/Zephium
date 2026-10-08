use crate::navigation_epoch::{NavigationEpoch, NavigationEpochTracker};
use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::ports::engine::{DiscardProbeId, EngineEvent};

use super::dispatch::with_discard_observation;
#[cfg(target_os = "windows")]
use super::dispatch::{with_suspend_deadline, with_suspend_result};
use super::permits::EventPermit;
#[cfg(any(not(target_os = "windows"), test))]
use super::scripts::DISCARD_SAFETY_QUERY_JS;
use super::EngineHost;

/// Native navigation provenance only. A non-GET/unknown main document in
/// this native history permanently prevents serialization/replay of it.
#[derive(Default)]
pub(super) struct ReplaySafety {
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    seen_get: std::cell::Cell<bool>,
    unsafe_history: std::cell::Cell<bool>,
    // The displayed document came from a non-GET request: reloading its URL
    // would not reproduce it, so it is never discarded. A request's method
    // applies only once it commits; a failed attempt leaves the page as it was.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    current_non_get: std::cell::Cell<bool>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pending_non_get: std::cell::Cell<Option<bool>>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    owned_bootstrap: bool,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    bootstrap_seen: std::cell::Cell<bool>,
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    engine_get: bool,
}
impl ReplaySafety {
    pub(super) fn new(owned_bootstrap: bool, engine_get: bool) -> Self {
        Self {
            owned_bootstrap,
            engine_get,
            ..Self::default()
        }
    }
    #[cfg(target_os = "macos")]
    pub(super) fn observe(&self, target: &str, action: wry::AppleNavigationAction) {
        if action.target_is_main_frame == Some(true) {
            if self.owned_bootstrap
                && target == "about:blank"
                && !self.seen_get.get()
                && !self.unsafe_history.get()
                && !self.bootstrap_seen.replace(true)
            {
                return;
            }
            self.seen_get.set(self.seen_get.get() || action.is_get);
            self.unsafe_history
                .set(self.unsafe_history.get() || !action.is_get);
            self.pending_non_get.set(Some(!action.is_get));
        }
    }
    #[cfg(target_os = "macos")]
    pub(super) fn commit(&self) {
        if let Some(non_get) = self.pending_non_get.take() {
            self.current_non_get.set(non_get);
        }
    }
    #[cfg(target_os = "macos")]
    pub(super) fn abandon(&self) {
        self.pending_non_get.set(None);
    }
    #[cfg(target_os = "macos")]
    pub(super) fn current_reloadable(&self) -> bool {
        self.seen_get.get() && !self.current_non_get.get()
    }
    #[cfg(target_os = "macos")]
    fn replayable(&self) -> bool {
        self.seen_get.get() && !self.unsafe_history.get()
    }
    pub(super) fn taint(&self) {
        self.unsafe_history.set(true);
    }
}

#[cfg(target_os = "macos")]
pub(super) struct DiscardedState {
    partition: zephium_core::ports::engine::Partition,
    url: String,
    state: crate::platform::imp::SessionState,
    // Shown over the tab while this history reloads; dropped first when the
    // snapshot budget is spent, never at the cost of the history itself.
    snapshot: Option<crate::platform::imp::PageSnapshot>,
}

#[cfg(target_os = "macos")]
const MAX_NATIVE_RESTORE_BYTES: usize = 16 * 1024 * 1024;
#[cfg(target_os = "macos")]
const MAX_RESTORE_SNAPSHOT_BYTES: usize = 24 * 1024 * 1024;

pub(super) type FinalDiscardDone = Box<dyn FnOnce(&mut EngineHost, bool) + Send>;

pub(super) struct ProbeLease {
    probe: DiscardProbeId,
    epoch: NavigationEpoch,
    deadline: std::time::Instant,
    settled: bool,
}

#[cfg(target_os = "windows")]
pub(super) struct WindowsFinalDiscard {
    id: ItemId,
    probe: DiscardProbeId,
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    epoch: NavigationEpoch,
    url: String,
    deadline: std::time::Instant,
    done: std::sync::Mutex<Option<FinalDiscardDone>>,
}

#[cfg(target_os = "windows")]
impl WindowsFinalDiscard {
    fn finish(self: &std::sync::Arc<Self>, host: &mut EngineHost, allowed: bool) {
        let Some(done) = self.done.lock().unwrap_or_else(|p| p.into_inner()).take() else {
            return;
        };
        let timer = host
            .views
            .get_mut(&self.id)
            .filter(|view| {
                view.windows_final_discard
                    .as_ref()
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, self))
            })
            .and_then(|view| view.discard_deadline.take());
        drop(timer);
        let allowed = allowed
            && std::time::Instant::now() < self.deadline
            && host.views.get(&self.id).is_some_and(|view| {
                discard_probe_identity_matches(
                    &view.event_permit,
                    &view.navigation,
                    &self.permit,
                    &self.navigation,
                    self.epoch,
                ) && view
                    .navigation
                    .matches_committed_snapshot(self.epoch, &self.url)
            })
            && host.windows_replay_allows(self.id, &self.url)
            && host.native_owned_state_allows_discard(self.id)
            && host.windows_audio_idle(self.id);
        done(host, allowed);
        if let Some(view) = host.views.get_mut(&self.id) {
            if view
                .windows_final_discard
                .as_ref()
                .is_some_and(|current| std::sync::Arc::ptr_eq(current, self))
            {
                view.windows_final_discard = None;
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(super) struct SuspendAttempt {
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    epoch: NavigationEpoch,
    deadline: std::time::Instant,
    started: std::sync::atomic::AtomicBool,
    expired: std::sync::atomic::AtomicBool,
}

#[cfg(target_os = "macos")]
pub(super) struct FinalDiscard {
    id: ItemId,
    probe: DiscardProbeId,
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    epoch: NavigationEpoch,
    url: String,
    revision: u64,
    deadline: std::time::Instant,
    // Native playback seen before the renderer report; only a report that
    // explains it as muted may still admit the discard.
    native_playback: std::cell::Cell<bool>,
    snapshot: std::cell::RefCell<Option<crate::platform::imp::PageSnapshot>>,
    done: std::cell::RefCell<Option<FinalDiscardDone>>,
    timer: std::cell::RefCell<Option<crate::platform::imp::ContentPolicyTimeout>>,
}

#[cfg(target_os = "macos")]
impl FinalDiscard {
    fn finish(self: &std::rc::Rc<Self>, host: &mut EngineHost, allowed: bool) {
        let Some(done) = self.done.borrow_mut().take() else {
            return;
        };
        self.timer.borrow_mut().take();
        let allowed =
            allowed && host.final_discard_matches(self) && host.prepare_native_discard_state(self);
        done(host, allowed);
        // Only physical close promotes this state. Cancellation can win in
        // the outer engine gate after these checks and before retirement.
        host.prepared_discard_states.remove(&self.id);
        if let Some(view) = host.views.get_mut(&self.id) {
            if view
                .final_discard
                .as_ref()
                .is_some_and(|current| std::rc::Rc::ptr_eq(current, self))
            {
                view.final_discard = None;
            }
        }
    }
}

#[cfg(target_os = "windows")]
const MAX_CONCURRENT_SUSPENDS: usize = 8;

#[cfg(target_os = "macos")]
fn queue_final_discard(
    state: std::rc::Rc<FinalDiscard>,
    allowed: bool,
    failure: std::sync::Arc<dyn Fn(&'static str) + Send + Sync>,
) {
    let id = state.id;
    if !super::dispatch::with_discard_terminal(id, move |host| state.finish(host, allowed)) {
        super::dispatch::seal_ingress();
        failure("final discard terminal completion was not admitted");
    }
}

#[cfg(any(target_os = "windows", test))]
fn suspend_result_matches(current: Option<&EventPermit>, requested: &EventPermit) -> bool {
    current.is_some_and(|current| {
        current.same_generation(requested) && current.active_token().is_some()
    })
}

#[cfg(target_os = "windows")]
fn queue_windows_final(
    state: std::sync::Arc<WindowsFinalDiscard>,
    allowed: bool,
    failure: std::sync::Arc<dyn Fn(&'static str) + Send + Sync>,
) {
    if !super::dispatch::with_discard_terminal(state.id, move |host| state.finish(host, allowed)) {
        super::dispatch::seal_ingress();
        failure("final discard terminal completion was not admitted");
    }
}

fn renderer_report_allows_discard(result: &str) -> bool {
    // Safe means the primitive mask contains only the ready bit, optionally
    // with the informational muted-playback bit. Reject every alternate
    // number/string/object representation without parsing.
    matches!(result, "1" | "1025")
}

/// The renderer's verdict on one probe of the current document.
#[derive(Clone, Copy)]
struct RendererReport {
    safe: bool,
    // Any native playback is muted media the renderer fully accounted for.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    playback_explained: bool,
}

/// Native playback in such a document is muted, so a reload loses nothing.
#[cfg(target_os = "macos")]
fn renderer_report_explains_playback(result: &str) -> bool {
    result == "1025"
}

fn discard_probe_identity_matches(
    current_permit: &EventPermit,
    current_navigation: &NavigationEpochTracker,
    requested_permit: &EventPermit,
    requested_navigation: &NavigationEpochTracker,
    requested_epoch: NavigationEpoch,
) -> bool {
    current_permit.same_generation(requested_permit)
        && current_navigation.same_generation(requested_navigation)
        && requested_navigation.is_current(requested_epoch)
}

impl EngineHost {
    /// One exact callback owns completion; renderer/native silence becomes a
    /// bounded refusal while the resident view and its authorities stay live.
    pub(crate) fn finalize_discard(
        &mut self,
        id: ItemId,
        probe: DiscardProbeId,
        done: FinalDiscardDone,
    ) {
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (id, probe);
            done(self, false);
        }
        #[cfg(target_os = "windows")]
        {
            let Some(view) = self.views.get(&id) else {
                done(self, false);
                return;
            };
            let Some((epoch, url)) = view.navigation.committed_snapshot() else {
                done(self, false);
                return;
            };
            if view.windows_final_discard.is_some()
                || !self.windows_replay_allows(id, &url)
                || !self.native_owned_state_allows_discard(id)
            {
                done(self, false);
                return;
            }
            let state = std::sync::Arc::new(WindowsFinalDiscard {
                id,
                probe,
                permit: view.event_permit.clone(),
                navigation: view.navigation.clone(),
                epoch,
                url,
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
                done: std::sync::Mutex::new(Some(done)),
            });
            let Some(view) = self.views.get_mut(&id) else {
                state.finish(self, false);
                return;
            };
            view.windows_final_discard = Some(state.clone());
            let weak = std::sync::Arc::downgrade(&state);
            let failure = self.native_terminal_failure.clone();
            let Some(timer) = crate::platform::imp::schedule_browser_timeout(
                std::time::Duration::from_secs(2),
                move || {
                    if let Some(state) = weak.upgrade() {
                        queue_windows_final(state, false, failure);
                    }
                },
            ) else {
                state.finish(self, false);
                return;
            };
            let current = self.views.get_mut(&id).filter(|view| {
                view.windows_final_discard
                    .as_ref()
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, &state))
                    && discard_probe_identity_matches(
                        &view.event_permit,
                        &view.navigation,
                        &state.permit,
                        &state.navigation,
                        state.epoch,
                    )
            });
            if let Some(view) = current {
                view.discard_deadline = Some(timer);
            } else {
                drop(timer);
                state.finish(self, false);
                return;
            }
            let script = self.windows_top_script(id);
            let weak = std::sync::Arc::downgrade(&state);
            let failure = self.native_terminal_failure.clone();
            let started = self.views.get(&id).is_some_and(|view| {
                view.evaluate_script_with_callback(script, move |result| {
                    if let Some(state) = weak.upgrade() {
                        queue_windows_final(
                            state,
                            renderer_report_allows_discard(&result),
                            failure.clone(),
                        );
                    }
                })
                .is_ok()
            });
            if !started {
                state.finish(self, false);
            }
        }
        #[cfg(target_os = "macos")]
        {
            let Some(view) = self.views.get(&id) else {
                done(self, false);
                return;
            };
            let Some((epoch, url)) = view.navigation.committed_snapshot() else {
                done(self, false);
                return;
            };
            if view.final_discard.is_some()
                || !view.replay_safety.current_reloadable()
                || !self.native_owned_state_allows_discard(id)
            {
                done(self, false);
                return;
            }
            let state = std::rc::Rc::new(FinalDiscard {
                id,
                probe,
                permit: view.event_permit.clone(),
                navigation: view.navigation.clone(),
                epoch,
                url,
                revision: self.discarded_state_revision,
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
                snapshot: std::cell::RefCell::new(None),
                native_playback: std::cell::Cell::new(false),
                done: std::cell::RefCell::new(Some(done)),
                timer: std::cell::RefCell::new(None),
            });
            self.views.get_mut(&id).unwrap().final_discard = Some(state.clone());
            let Some(mtm) = objc2_foundation::MainThreadMarker::new() else {
                state.finish(self, false);
                return;
            };
            let weak = dispatch2::MainThreadBound::new(std::rc::Rc::downgrade(&state), mtm);
            let failure = self.native_terminal_failure.clone();
            let Some(timer) = crate::platform::imp::schedule_content_policy_timeout(
                std::time::Duration::from_secs(2),
                move || {
                    if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                        if let Some(state) = weak.get(mtm).upgrade() {
                            queue_final_discard(state, false, failure);
                        }
                    }
                },
            ) else {
                state.finish(self, false);
                return;
            };
            *state.timer.borrow_mut() = Some(timer);
            let completion = std::rc::Rc::downgrade(&state);
            let activity_failure = self.native_terminal_failure.clone();
            let started = self.views.get(&id).is_some_and(|view| {
                crate::platform::imp::query_document_playback(view, move |playback| {
                    let Some(completion) = completion.upgrade() else {
                        return;
                    };
                    let Some(playing) = playback else {
                        queue_final_discard(completion, false, activity_failure);
                        return;
                    };
                    completion.native_playback.set(playing);
                    let id = completion.id;
                    let queued = completion.clone();
                    if !super::dispatch::with_discard_terminal(id, move |host| {
                        host.query_final_discard(queued)
                    }) {
                        super::dispatch::seal_ingress();
                        activity_failure("final discard activity completion was not admitted");
                    }
                })
            });
            if !started {
                state.finish(self, false);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn query_final_discard(&mut self, state: std::rc::Rc<FinalDiscard>) {
        if !self.final_discard_matches(&state) {
            state.finish(self, false);
            return;
        }
        let mtm = objc2_foundation::MainThreadMarker::new().unwrap();
        let weak = dispatch2::MainThreadBound::new(std::rc::Rc::downgrade(&state), mtm);
        let failure = self.native_terminal_failure.clone();
        let started = self.views.get(&state.id).is_some_and(|view| {
            view.evaluate_script_with_callback(DISCARD_SAFETY_QUERY_JS, move |result| {
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    if let Some(state) = weak.get(mtm).upgrade() {
                        let allowed = renderer_report_allows_discard(&result)
                            && (!state.native_playback.get()
                                || renderer_report_explains_playback(&result));
                        if !allowed {
                            queue_final_discard(state, false, failure.clone());
                            return;
                        }
                        let id = state.id;
                        let capture_failure = failure.clone();
                        if !super::dispatch::with_discard_terminal(id, move |host| {
                            host.capture_final_snapshot(state, capture_failure)
                        }) {
                            super::dispatch::seal_ingress();
                            failure("final discard snapshot was not admitted");
                        }
                    }
                }
            })
            .is_ok()
        });
        if !started {
            state.finish(self, false);
        }
    }

    /// The last step before retirement: the page's last frame, taken while
    /// it is still the exact checked document. No snapshot never blocks it.
    #[cfg(target_os = "macos")]
    fn capture_final_snapshot(
        &mut self,
        state: std::rc::Rc<FinalDiscard>,
        failure: std::sync::Arc<dyn Fn(&'static str) + Send + Sync>,
    ) {
        if !self.final_discard_matches(&state) {
            state.finish(self, false);
            return;
        }
        let Some(view) = self.views.get(&state.id) else {
            state.finish(self, false);
            return;
        };
        let mtm = objc2_foundation::MainThreadMarker::new().unwrap();
        let weak = dispatch2::MainThreadBound::new(std::rc::Rc::downgrade(&state), mtm);
        let started = crate::platform::imp::PageSnapshot::capture(&view.view, move |snapshot| {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                if let Some(state) = weak.get(mtm).upgrade() {
                    *state.snapshot.borrow_mut() = snapshot;
                    queue_final_discard(state, true, failure);
                }
            }
        });
        if !started {
            state.finish(self, true);
        }
    }

    #[cfg(target_os = "macos")]
    fn final_discard_matches(&self, state: &FinalDiscard) -> bool {
        self.discarded_state_revision != u64::MAX
            && self.discarded_state_revision == state.revision
            && std::time::Instant::now() < state.deadline
            && self.views.get(&state.id).is_some_and(|view| {
                view.final_discard
                    .as_ref()
                    .is_some_and(|current| current.probe == state.probe)
                    && discard_probe_identity_matches(
                        &view.event_permit,
                        &view.navigation,
                        &state.permit,
                        &state.navigation,
                        state.epoch,
                    )
                    && view
                        .navigation
                        .matches_committed_snapshot(state.epoch, &state.url)
                    && view.replay_safety.current_reloadable()
            })
            && self.native_owned_state_allows_discard(state.id)
    }

    /// Returns whether the discard may proceed. History that cannot be safely
    /// replayed (an earlier POST, erased history, a non-web entry) is dropped
    /// and the page later reloads its committed GET URL. History that is only
    /// too large to keep is never dropped silently: the page stays resident.
    #[cfg(target_os = "macos")]
    fn prepare_native_discard_state(&mut self, proof: &FinalDiscard) -> bool {
        let Some(partition) = self.partitions.get(&proof.id).copied() else {
            return false;
        };
        let Some(view) = self.views.get(&proof.id) else {
            return false;
        };
        if !view.replay_safety.replayable() {
            return true;
        }
        let state = match crate::platform::imp::SessionState::capture(
            &view.view,
            &proof.url,
            view.replay_safety.owned_bootstrap,
            || self.final_discard_matches(proof),
        ) {
            Ok(state) => state,
            Err(crate::platform::imp::SessionCaptureRefusal::Unreplayable) => {
                return self.final_discard_matches(proof);
            }
            Err(_) => return false,
        };
        let bytes = self
            .discarded_states
            .values()
            .chain(self.prepared_discard_states.values())
            .map(|entry| entry.state.bytes())
            .sum::<usize>();
        if bytes
            .checked_add(state.bytes())
            .is_none_or(|total| total > MAX_NATIVE_RESTORE_BYTES)
            || self.discarded_states.len() + self.prepared_discard_states.len()
                >= zephium_core::session::MAX_SESSION_ITEMS
        {
            return false;
        }
        let snapshot_bytes = self
            .discarded_states
            .values()
            .chain(self.prepared_discard_states.values())
            .filter_map(|entry| entry.snapshot.as_ref())
            .map(crate::platform::imp::PageSnapshot::bytes)
            .sum::<usize>();
        let snapshot = proof.snapshot.borrow_mut().take().filter(|snapshot| {
            snapshot_bytes
                .checked_add(snapshot.bytes())
                .is_some_and(|total| total <= MAX_RESTORE_SNAPSHOT_BYTES)
        });
        self.prepared_discard_states.insert(
            proof.id,
            DiscardedState {
                partition,
                url: proof.url.clone(),
                state,
                snapshot,
            },
        );
        true
    }

    pub(crate) fn close_for_discard(&mut self, id: ItemId) {
        #[cfg(target_os = "macos")]
        let prepared = self.prepared_discard_states.remove(&id);
        #[cfg(target_os = "macos")]
        let revision = self.discarded_state_revision;
        self.close(id);
        #[cfg(target_os = "macos")]
        if let Some(state) = prepared {
            if self.discarded_state_revision == revision
                && !self.erasure_tombstones.contains(&state.partition.profile())
            {
                self.discarded_states.insert(id, state);
            }
        }
    }

    pub(crate) fn forget_discarded_state(&mut self, profile: ProfileId, item: Option<ItemId>) {
        for (id, view) in &self.views {
            if self
                .partitions
                .get(id)
                .is_some_and(|partition| partition.profile() == profile)
                && item.is_none_or(|item| item == *id)
            {
                view.replay_safety.taint();
                #[cfg(target_os = "windows")]
                if let Some(witness) = &view.request_witness {
                    witness.taint();
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            // A single-tab erasure belongs to a tab that is being closed; its
            // own capture and promotion are synchronous with that close. Only
            // profile-wide erasure must invalidate other in-flight captures.
            if item.is_none() {
                self.discarded_state_revision = self.discarded_state_revision.saturating_add(1);
            }
            let retain = |id: &ItemId, state: &mut DiscardedState| {
                state.partition.profile() != profile || item.is_some_and(|item| item != *id)
            };
            self.discarded_states.retain(retain);
            self.prepared_discard_states.retain(retain);
            self.restore_snapshots.retain(|id, (partition, _, _)| {
                partition.profile() != profile || item.is_some_and(|item| item != *id)
            });
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn restore_discarded_state(
        &mut self,
        id: ItemId,
        partition: zephium_core::ports::engine::Partition,
        url: &str,
        view: &wry::WebView,
        permit: &EventPermit,
        navigation: &NavigationEpochTracker,
    ) -> Option<bool> {
        let saved = self.discarded_states.get(&id)?;
        if saved.partition != partition || saved.url != url {
            self.discarded_states.remove(&id);
            return None;
        }
        let revision = self.discarded_state_revision;
        let epoch = navigation.current()?;
        let restored = saved.state.restore(view, url, || {
            self.discarded_state_revision == revision
                && revision != u64::MAX
                && !self.erasure_tombstones.contains(&partition.profile())
                && permit.active_token().is_some()
                && navigation.is_current(epoch)
        });
        // A snapshot is used at most once. If WebKit declines it, the caller
        // loads the committed URL instead of failing the tab repeatedly. The
        // last frame is worth showing only over the same restored position.
        let saved = self.discarded_states.remove(&id);
        if restored {
            if let Some(saved) = saved {
                if let Some(snapshot) = saved.snapshot {
                    self.restore_snapshots
                        .insert(id, (partition, saved.url, snapshot));
                }
            }
        }
        restored.then_some(true)
    }

    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    fn native_owned_state_allows_discard(&self, id: ItemId) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        let Some(profile) = self
            .partitions
            .get(&id)
            .map(|partition| partition.profile())
        else {
            return false;
        };
        if self.erasure_tombstones.contains(&profile) || self.picker.is_some() {
            return false;
        }
        #[cfg(target_os = "macos")]
        if self
            .downloads
            .as_ref()
            .is_some_and(|downloads| downloads.has_pending_decision(profile))
        {
            return false;
        }
        #[cfg(target_os = "windows")]
        if self
            .downloads
            .as_ref()
            .is_some_and(|downloads| downloads.has_profile_activity(profile))
        {
            return false;
        }
        #[cfg(target_os = "macos")]
        if view.file_uploads.has_pending()
            || self.page_permissions.has_pending_for(id)
            || self
                .page_permissions
                .pending_presence()
                .load(std::sync::atomic::Ordering::Acquire)
            || !crate::platform::imp::native_discard_idle(&view.view)
        {
            return false;
        }
        #[cfg(target_os = "windows")]
        if !self.hidden.contains(&id) {
            return false;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = view;
        true
    }
    #[cfg(target_os = "windows")]
    fn windows_audio_idle(&self, id: ItemId) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        let idle = std::rc::Rc::new(std::cell::Cell::new(false));
        let captured = idle.clone();
        crate::platform::imp::query_document_activity(view, move |allowed| captured.set(allowed))
            && idle.get()
    }

    #[cfg(target_os = "windows")]
    fn windows_replay_allows(&self, id: ItemId, url: &str) -> bool {
        self.views.get(&id).is_some_and(|view| {
            view.replay_safety.engine_get
                && !view.replay_safety.unsafe_history.get()
                && view.request_witness.as_ref().is_some_and(|witness| {
                    witness.allows_replay(url)
                        && crate::platform::imp::discard_history_allows(
                            view,
                            witness.has_owned_blank_bootstrap(),
                        )
                })
        })
    }

    #[cfg(target_os = "windows")]
    fn windows_top_script(&self, id: ItemId) -> &'static str {
        if self
            .views
            .get(&id)
            .and_then(|view| view.request_witness.as_ref())
            .is_some_and(|witness| witness.has_owned_blank_bootstrap())
        {
            super::scripts::DISCARD_BOOTSTRAP_TOP_QUERY_JS
        } else {
            super::scripts::DISCARD_TOP_QUERY_JS
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn sample_page_memory(&self, ids: Vec<ItemId>) {
        for id in ids {
            let (Some(view), Some(partition)) = (self.views.get(&id), self.partitions.get(&id))
            else {
                continue;
            };
            if let Some(bytes) = crate::platform::imp::page_footprint(&view.view) {
                view.event_permit.emit(
                    &self.sink,
                    EngineEvent::PageMemory {
                        id,
                        profile: partition.profile(),
                        bytes,
                    },
                );
            }
        }
    }

    pub(crate) fn set_memory_pressure(
        &mut self,
        pressure: zephium_core::ports::engine::MemoryPressure,
    ) {
        self.memory_pressure = pressure;
        if pressure == zephium_core::ports::engine::MemoryPressure::Normal {
            return;
        }
        if let Some(spare) = self.spare.take() {
            #[cfg(target_os = "windows")]
            {
                let profile = spare.partition.profile();
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
    }

    pub(crate) fn probe_discard_safety(&self, id: ItemId, probe: DiscardProbeId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(epoch) = view.navigation.current_committed() else {
            return;
        };
        {
            let Ok(mut lease) = view.discard_probe_lease.try_borrow_mut() else {
                return;
            };
            if lease
                .as_ref()
                .is_none_or(|lease| lease.probe != probe || lease.epoch != epoch)
            {
                *lease = Some(ProbeLease {
                    probe,
                    epoch,
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
                    settled: false,
                });
            }
            let lease = lease.as_mut().unwrap();
            if lease.settled {
                return;
            }
            if std::time::Instant::now() >= lease.deadline {
                lease.settled = true;
                view.event_permit.emit(
                    &self.sink,
                    EngineEvent::DiscardSafety {
                        id,
                        probe,
                        can_discard: false,
                    },
                );
                return;
            }
        }
        if !cfg!(any(target_os = "macos", target_os = "windows")) {
            view.event_permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
            return;
        }
        #[cfg(target_os = "windows")]
        if !self.windows_replay_allows(id, &view.navigation.committed_snapshot().unwrap().1)
            || !self.native_owned_state_allows_discard(id)
        {
            view.event_permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
            return;
        }
        #[cfg(target_os = "macos")]
        if !view.replay_safety.current_reloadable() || !self.native_owned_state_allows_discard(id) {
            view.event_permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
            return;
        }
        let permit = view.event_permit.clone();
        let navigation = view.navigation.clone();
        let queued_permit = permit.clone();
        let queued_navigation = navigation.clone();
        #[cfg(target_os = "windows")]
        let script = self.windows_top_script(id);
        #[cfg(not(target_os = "windows"))]
        let script = DISCARD_SAFETY_QUERY_JS;
        let _ = view.evaluate_script_with_callback(script, move |result| {
            // Callback completion can race focus-driven navigation or close.
            // Do not reinterpret a report from the old document under a new
            // same-id view/navigation; the shell timeout is fail-closed.
            if !queued_navigation.is_current(epoch) {
                return;
            }
            let renderer_safe = renderer_report_allows_discard(&result);
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if matches!(result.as_str(), "513" | "1537") {
                let completion_permit = queued_permit.clone();
                let completion_navigation = queued_navigation.clone();
                with_discard_observation(id, move |host| {
                    host.defer_discard_probe(
                        id,
                        probe,
                        completion_permit,
                        completion_navigation,
                        epoch,
                    );
                });
                return;
            }
            let report = RendererReport {
                safe: renderer_safe,
                #[cfg(target_os = "macos")]
                playback_explained: renderer_report_explains_playback(&result),
                #[cfg(not(target_os = "macos"))]
                playback_explained: false,
            };
            let completion_permit = queued_permit.clone();
            let completion_navigation = queued_navigation.clone();
            with_discard_observation(id, move |host| {
                host.complete_discard_probe(
                    id,
                    probe,
                    &completion_permit,
                    &completion_navigation,
                    epoch,
                    report,
                )
            });
        });
    }

    fn complete_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        permit: &EventPermit,
        navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
        report: RendererReport,
    ) {
        let renderer_safe = report.safe;
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !discard_probe_identity_matches(
            &view.event_permit,
            &view.navigation,
            permit,
            navigation,
            epoch,
        ) {
            return;
        }

        if !renderer_safe {
            self.settle_discard_probe(id, probe, epoch);
            permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
            return;
        }

        // WebView2 and WebKitGTK expose native audio activity; WKWebView has
        // public asynchronous playback plus synchronous camera/microphone
        // capture state. Page JavaScript cannot spoof these cross-checks.
        let activity_permit = permit.clone();
        let activity_navigation = navigation.clone();
        #[cfg(target_os = "macos")]
        let started = crate::platform::imp::query_document_playback(view, move |playback| {
            let native_allows =
                playback.is_some_and(|playing| !playing || report.playback_explained);
            with_discard_observation(id, move |host| {
                host.finish_discard_probe(
                    id,
                    probe,
                    &activity_permit,
                    &activity_navigation,
                    epoch,
                    native_allows,
                )
            });
        });
        #[cfg(not(target_os = "macos"))]
        let started = crate::platform::imp::query_document_activity(view, move |native_allows| {
            with_discard_observation(id, move |host| {
                host.finish_discard_probe(
                    id,
                    probe,
                    &activity_permit,
                    &activity_navigation,
                    epoch,
                    native_allows,
                )
            });
        });
        if !started {
            self.settle_discard_probe(id, probe, epoch);
            // Missing native API/admission is uncertainty, never silence.
            permit.emit(
                &self.sink,
                EngineEvent::DiscardSafety {
                    id,
                    probe,
                    can_discard: false,
                },
            );
        }
    }

    #[cfg(target_os = "macos")]
    fn defer_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        permit: EventPermit,
        navigation: NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !discard_probe_identity_matches(
            &view.event_permit,
            &view.navigation,
            &permit,
            &navigation,
            epoch,
        ) {
            return;
        }
        let timer = crate::platform::imp::schedule_presentation_timeout(
            std::time::Duration::from_millis(200),
            move || {
                with_discard_observation(id, move |host| {
                    if host.views.get(&id).is_some_and(|view| {
                        discard_probe_identity_matches(
                            &view.event_permit,
                            &view.navigation,
                            &permit,
                            &navigation,
                            epoch,
                        )
                    }) {
                        host.probe_discard_safety(id, probe);
                    }
                });
            },
        );
        *view.discard_settle_timer.borrow_mut() = timer;
    }

    #[cfg(target_os = "windows")]
    fn defer_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        permit: EventPermit,
        navigation: NavigationEpochTracker,
        epoch: NavigationEpoch,
    ) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let timer = crate::platform::imp::schedule_browser_timeout(
            std::time::Duration::from_millis(200),
            move || {
                if permit.active_token().is_none() || !navigation.is_current(epoch) {
                    return;
                }
                with_discard_observation(id, move |host| {
                    if host.views.get(&id).is_some_and(|view| {
                        discard_probe_identity_matches(
                            &view.event_permit,
                            &view.navigation,
                            &permit,
                            &navigation,
                            epoch,
                        )
                    }) {
                        host.probe_discard_safety(id, probe);
                    }
                });
            },
        );
        let previous = view.discard_settle_timer.replace(timer);
        drop(previous);
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn cancel_final_discard(&mut self, id: ItemId, probe: DiscardProbeId) {
        let state = self
            .views
            .get(&id)
            .and_then(|view| view.windows_final_discard.as_ref())
            .filter(|state| state.probe == probe)
            .cloned();
        if let Some(state) = state {
            state.finish(self, false);
        }
        if let Some(view) = self.views.get(&id) {
            let matched = {
                let mut lease = view.discard_probe_lease.borrow_mut();
                if let Some(lease) = lease.as_mut().filter(|lease| lease.probe == probe) {
                    lease.settled = true;
                    true
                } else {
                    false
                }
            };
            if matched {
                let timer = view.discard_settle_timer.replace(None);
                drop(timer);
            }
        }
    }

    fn settle_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        epoch: NavigationEpoch,
    ) -> bool {
        let Some(view) = self.views.get(&id) else {
            return false;
        };
        let valid = {
            let Ok(mut lease) = view.discard_probe_lease.try_borrow_mut() else {
                return false;
            };
            let Some(lease) = lease
                .as_mut()
                .filter(|lease| lease.probe == probe && lease.epoch == epoch)
            else {
                return false;
            };
            let valid = !lease.settled && std::time::Instant::now() < lease.deadline;
            lease.settled = true;
            valid
        };
        #[cfg(target_os = "windows")]
        {
            let timer = view.discard_settle_timer.replace(None);
            drop(timer);
        }
        valid
    }

    fn finish_discard_probe(
        &self,
        id: ItemId,
        probe: DiscardProbeId,
        permit: &EventPermit,
        navigation: &NavigationEpochTracker,
        epoch: NavigationEpoch,
        native_allows: bool,
    ) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        if !discard_probe_identity_matches(
            &view.event_permit,
            &view.navigation,
            permit,
            navigation,
            epoch,
        ) {
            return;
        }
        // Admitted macOS downloads belong to the profile coordinator and
        // retain their WKDownload/delegate independently of this view. Pending
        // destination decisions remain view-bound and are revoked on teardown.
        // Windows/Linux continue denying downloads at their native boundary.
        permit.emit(
            &self.sink,
            EngineEvent::DiscardSafety {
                id,
                probe,
                can_discard: native_allows && self.settle_discard_probe(id, probe, epoch),
            },
        );
    }

    /// Suspends hidden views: WebView2 `TrySuspend` on Windows, WebKit's
    /// suspend scheduling policy on macOS. Neither is reported as proof that
    /// a renderer stopped; macOS still runs audible or capturing pages.
    pub(crate) fn set_dormant(&mut self, ids: Vec<ItemId>) {
        #[cfg(target_os = "windows")]
        {
            let next: std::collections::HashSet<ItemId> = ids
                .into_iter()
                .filter(|id| self.hidden.contains(id))
                .collect();
            let wake: Vec<ItemId> = self.dormant.difference(&next).copied().collect();
            for id in wake {
                if let Some(view) = self.views.get(&id) {
                    crate::platform::imp::resume(view);
                }
                self.dormant.remove(&id);
                self.refresh_missed_styles(id);
            }
            self.suspend_failed.retain(|id| next.contains(id));
            let cancelled: Vec<_> = self
                .views
                .iter()
                .filter(|(id, view)| !next.contains(id) && view.suspend_attempt.is_some())
                .map(|(id, _)| *id)
                .collect();
            for id in cancelled {
                self.cancel_suspend_guard(id);
            }
            self.desired_dormant = next;
            self.pump_suspends();
        }
        #[cfg(target_os = "macos")]
        {
            let next: std::collections::HashSet<ItemId> = ids.into_iter().collect();
            for (id, view) in &self.views {
                crate::platform::imp::set_background_suspension(&view.view, next.contains(id));
            }
            let woken: Vec<ItemId> = self.dormant.difference(&next).copied().collect();
            self.dormant = next;
            for id in woken {
                self.refresh_missed_styles(id);
            }
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        let _ = ids;
    }

    #[cfg(target_os = "windows")]
    pub(super) fn cancel_suspend_guard(&mut self, id: ItemId) {
        if let Some(attempt) = self
            .views
            .get(&id)
            .and_then(|view| view.suspend_attempt.as_ref())
        {
            attempt
                .expired
                .store(true, std::sync::atomic::Ordering::Release);
            self.suspending.remove(&id);
            self.suspend_uncertain.insert(id);
        }
        let timer = self
            .views
            .get_mut(&id)
            .and_then(|view| view.suspend_deadline.take());
        drop(timer);
    }

    #[cfg(target_os = "windows")]
    fn suspend_attempt_matches(
        &self,
        id: ItemId,
        attempt: &std::sync::Arc<SuspendAttempt>,
    ) -> bool {
        self.views.get(&id).is_some_and(|view| {
            suspend_result_matches(Some(&view.event_permit), &attempt.permit)
                && view
                    .suspend_attempt
                    .as_ref()
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, attempt))
        })
    }

    #[cfg(target_os = "windows")]
    fn on_suspend_timeout(&mut self, id: ItemId, attempt: std::sync::Arc<SuspendAttempt>) {
        if !self.suspend_attempt_matches(id, &attempt)
            || attempt
                .expired
                .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        self.suspending.remove(&id);
        self.suspend_failed.insert(id);
        let timer = self
            .views
            .get_mut(&id)
            .and_then(|view| view.suspend_deadline.take());
        drop(timer);
        // Keep one uncertain operation per native generation until its exact
        // callback arrives or the view closes. A timeout releases a batch slot
        // but cannot authorize overlapping TrySuspend/evaluation requests.
        self.suspend_uncertain.insert(id);
        self.refresh_missed_styles(id);
        self.pump_suspends();
    }

    #[cfg(target_os = "windows")]
    fn on_suspend_preflight(
        &mut self,
        id: ItemId,
        attempt: std::sync::Arc<SuspendAttempt>,
        safe: bool,
    ) {
        if !self.suspend_attempt_matches(id, &attempt) {
            return;
        }
        if attempt.started.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let current = safe
            && !attempt.expired.load(std::sync::atomic::Ordering::Acquire)
            && std::time::Instant::now() < attempt.deadline
            && self.desired_dormant.contains(&id)
            && self.hidden.contains(&id)
            && self.views.get(&id).is_some_and(|view| {
                discard_probe_identity_matches(
                    &view.event_permit,
                    &view.navigation,
                    &attempt.permit,
                    &attempt.navigation,
                    attempt.epoch,
                )
            })
            && self.native_owned_state_allows_discard(id)
            && self.windows_audio_idle(id);
        if !current {
            self.on_guarded_suspend_result(id, attempt, false);
            return;
        }
        if attempt
            .started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let weak = std::sync::Arc::downgrade(&attempt);
        let started = self.views.get(&id).is_some_and(|view| {
            crate::platform::imp::try_suspend(view, move |suspended| {
                if let Some(attempt) = weak.upgrade() {
                    // Best effort: WebView2 resumes a suspended view itself
                    // when it becomes visible, so a refused completion (queue
                    // overload or shutdown) cannot strand a frozen page.
                    let _ = with_suspend_result(id, move |host| {
                        host.on_guarded_suspend_result(id, attempt, suspended)
                    });
                }
            })
        });
        if !started {
            self.on_guarded_suspend_result(id, attempt, false);
        }
    }

    #[cfg(target_os = "windows")]
    fn on_guarded_suspend_result(
        &mut self,
        id: ItemId,
        attempt: std::sync::Arc<SuspendAttempt>,
        suspended: bool,
    ) {
        if !self.suspend_attempt_matches(id, &attempt) {
            return;
        }
        let current_document = self.views.get(&id).is_some_and(|view| {
            discard_probe_identity_matches(
                &view.event_permit,
                &view.navigation,
                &attempt.permit,
                &attempt.navigation,
                attempt.epoch,
            )
        });
        let still_desired = !attempt.expired.load(std::sync::atomic::Ordering::Acquire)
            && std::time::Instant::now() < attempt.deadline
            && current_document
            && self.desired_dormant.contains(&id)
            && self.hidden.contains(&id);
        self.suspending.remove(&id);
        self.suspend_uncertain.remove(&id);
        let timer = if let Some(view) = self.views.get_mut(&id) {
            view.suspend_attempt = None;
            view.suspend_deadline.take()
        } else {
            None
        };
        drop(timer);
        if suspended && still_desired {
            self.dormant.insert(id);
        } else {
            if suspended {
                // Even a late successful callback only belongs to this exact
                // native view. Resume it if focus, document or desired state moved.
                if let Some(view) = self.views.get(&id) {
                    crate::platform::imp::resume(view);
                }
            }
            if self.desired_dormant.contains(&id) {
                self.suspend_failed.insert(id);
            }
        }
        self.refresh_missed_styles(id);
        self.pump_suspends();
    }

    #[cfg(target_os = "windows")]
    fn pump_suspends(&mut self) {
        while self.suspending.len() < MAX_CONCURRENT_SUSPENDS {
            let candidate = self
                .desired_dormant
                .iter()
                .filter(|id| {
                    !self.dormant.contains(id)
                        && !self.suspending.contains(id)
                        && !self.suspend_failed.contains(id)
                        && !self.suspend_uncertain.contains(id)
                })
                .copied()
                .min();
            let Some(id) = candidate else { break };
            let Some(view) = self.views.get(&id) else {
                self.suspend_failed.insert(id);
                continue;
            };
            let Some(epoch) = view.navigation.current_committed() else {
                self.suspend_failed.insert(id);
                continue;
            };
            let attempt = std::sync::Arc::new(SuspendAttempt {
                permit: view.event_permit.clone(),
                navigation: view.navigation.clone(),
                epoch,
                started: std::sync::atomic::AtomicBool::new(false),
                expired: std::sync::atomic::AtomicBool::new(false),
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
            });
            let Some(view) = self.views.get_mut(&id) else {
                continue;
            };
            view.suspend_attempt = Some(attempt.clone());
            self.suspending.insert(id);
            let weak = std::sync::Arc::downgrade(&attempt);
            let timer = crate::platform::imp::schedule_browser_timeout(
                std::time::Duration::from_secs(2),
                move || {
                    let Some(attempt) = weak.upgrade() else {
                        return;
                    };
                    if attempt.expired.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }
                    let _ =
                        with_suspend_deadline(id, move |host| host.on_suspend_timeout(id, attempt));
                },
            );
            let Some(timer) = timer else {
                self.suspending.remove(&id);
                self.suspend_failed.insert(id);
                if let Some(view) = self.views.get_mut(&id) {
                    view.suspend_attempt = None;
                }
                continue;
            };
            let current = self.views.get_mut(&id).filter(|view| {
                view.suspend_attempt
                    .as_ref()
                    .is_some_and(|current| std::sync::Arc::ptr_eq(current, &attempt))
                    && discard_probe_identity_matches(
                        &view.event_permit,
                        &view.navigation,
                        &attempt.permit,
                        &attempt.navigation,
                        attempt.epoch,
                    )
            });
            if let Some(view) = current {
                view.suspend_deadline = Some(timer);
            } else {
                drop(timer);
                self.on_guarded_suspend_result(id, attempt, false);
                continue;
            }
            let weak = std::sync::Arc::downgrade(&attempt);
            let started = self.views.get(&id).is_some_and(|view| {
                view.evaluate_script_with_callback(
                    super::scripts::SUSPEND_ACTIVITY_QUERY_JS,
                    move |result| {
                        if let Some(attempt) = weak.upgrade() {
                            let safe = renderer_report_allows_discard(&result);
                            let _ = with_suspend_result(id, move |host| {
                                host.on_suspend_preflight(id, attempt, safe)
                            });
                        }
                    },
                )
                .is_ok()
            });
            if !started {
                self.suspending.remove(&id);
                self.suspend_failed.insert(id);
                let timer = self.views.get_mut(&id).and_then(|view| {
                    view.suspend_attempt = None;
                    view.suspend_deadline.take()
                });
                drop(timer);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use crate::navigation_epoch::NavigationTransition;

    use super::super::scripts::DISCARD_SAFETY_BOOTSTRAP_JS;
    use super::*;

    #[cfg(target_os = "macos")]
    fn action(get: bool, main: Option<bool>) -> wry::AppleNavigationAction {
        wry::AppleNavigationAction {
            navigation_type: wry::AppleNavigationType::Other,
            is_get: get,
            target_is_main_frame: main,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn get_redirect_cannot_make_prior_post_history_replayable() {
        let replay = ReplaySafety::new(false, true);
        replay.observe("https://example.com/form", action(true, Some(true)));
        assert!(replay.replayable());
        replay.observe("https://example.com/result", action(false, Some(true)));
        replay.observe("https://example.com/confirmation", action(true, Some(true)));
        assert!(
            !replay.replayable(),
            "a GET redirect cannot erase a POST/unknown native request from this history"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_post_result_page_is_never_reloaded_but_a_later_get_page_is() {
        let replay = ReplaySafety::new(false, true);
        replay.observe("https://example.com/login", action(true, Some(true)));
        replay.commit();
        replay.observe("https://example.com/session", action(false, Some(true)));
        replay.commit();
        assert!(
            !replay.current_reloadable(),
            "a GET of a POST result's URL would not reproduce the page"
        );
        replay.observe("https://example.com/next", action(true, Some(true)));
        replay.abandon();
        assert!(
            !replay.current_reloadable(),
            "a failed GET attempt leaves the POST result on screen"
        );
        replay.observe("https://example.com/inbox", action(true, Some(true)));
        replay.commit();
        assert!(replay.current_reloadable());
        assert!(
            !replay.replayable(),
            "the earlier POST still forbids replaying the native history"
        );
        replay.observe("https://example.com/frame", action(false, Some(false)));
        assert!(
            replay.current_reloadable(),
            "child-frame posts do not count"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_owned_initial_blank_is_exempt_from_unknown_method() {
        let replay = ReplaySafety::new(true, true);
        replay.observe("about:blank", action(false, Some(true)));
        replay.observe("https://example.com/", action(true, Some(true)));
        assert!(replay.replayable());
        replay.observe("about:blank", action(false, Some(true)));
        assert!(!replay.replayable());
        let unowned = ReplaySafety::new(false, false);
        unowned.observe("about:blank", action(false, Some(true)));
        unowned.observe("https://example.com/", action(true, Some(true)));
        assert!(!unowned.replayable());
        let missed_bootstrap = ReplaySafety::new(true, true);
        missed_bootstrap.observe("https://example.com/", action(true, Some(true)));
        missed_bootstrap.observe("about:blank", action(false, Some(true)));
        assert!(
            !missed_bootstrap.replayable(),
            "a missed initial callback cannot exempt a later user navigation"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn clearing_data_keeps_existing_native_history_nonrestorable() {
        let replay = ReplaySafety::new(false, true);
        replay.observe("https://example.com/", action(true, Some(true)));
        replay.taint();
        replay.observe("https://example.com/after-clear", action(true, Some(true)));
        assert!(!replay.replayable());
        assert!(
            !ReplaySafety::new(false, true).replayable(),
            "a new generation still needs authoritative native GET evidence"
        );
    }

    #[test]
    fn suspend_completion_cannot_settle_a_closed_or_replaced_view() {
        use std::sync::{atomic::AtomicBool, Arc};
        let token = Arc::new(AtomicBool::new(true));
        let current = EventPermit::bound(&token);
        let requested = current.clone();
        assert!(suspend_result_matches(Some(&current), &requested));
        assert!(!suspend_result_matches(None, &requested));
        let replacement_token = Arc::new(AtomicBool::new(true));
        let replacement = EventPermit::bound(&replacement_token);
        assert!(!suspend_result_matches(Some(&replacement), &requested));
        current.revoke();
        assert!(!suspend_result_matches(Some(&current), &requested));
    }

    #[test]
    fn discard_report_accepts_only_the_exact_safe_primitive_mask() {
        assert!(renderer_report_allows_discard("1"));
        assert!(renderer_report_allows_discard("1025"));
        for protected in [
            "0", "2", "3", "255", "256", "511", "1024", "1026", "1537", "null", "\"1\"", "{}", " 1",
        ] {
            assert!(
                !renderer_report_allows_discard(protected),
                "alternate renderer value must veto discard: {protected:?}"
            );
        }
        assert!(DISCARD_SAFETY_BOOTSTRAP_JS
            .contains("((localUncertain || (editedEver && localIncomplete)) ? 256 : 0)"));
        // A reload rebuilds ordinary scripted work, and patching these APIs
        // is visible to anti-bot scripts.
        for unpatched in [
            "'fetch'",
            "XMLHttpRequest",
            "'WebSocket'",
            "'Worker'",
            "pushState",
            "collect('canvas')",
            "childFrames",
        ] {
            assert!(
                !DISCARD_SAFETY_BOOTSTRAP_JS.contains(unpatched),
                "discard bootstrap must not veto or hook ordinary page work: {unpatched}"
            );
        }
        assert!(DISCARD_SAFETY_QUERY_JS.contains("return 256"));
        assert!(!DISCARD_SAFETY_QUERY_JS.contains("return {"));
    }

    #[test]
    fn discard_probe_requires_exact_generation_and_current_navigation_epoch() {
        let first_token = Arc::new(AtomicBool::new(true));
        let first_permit = EventPermit::bound(&first_token);
        let first_navigation = NavigationEpochTracker::new();
        let first_epoch = first_navigation.begin("https://first.example/").unwrap();
        assert_eq!(
            first_navigation.observe_navigation(&wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Started,
                url: "https://first.example/".into(),
            }),
            Some(NavigationTransition::Started(first_epoch))
        );
        assert_eq!(
            first_navigation.observe_navigation(&wry::NavigationEvent {
                id: wry::NavigationId::from_raw(1),
                phase: wry::NavigationEventPhase::Committed,
                url: "https://first.example/".into(),
            }),
            Some(NavigationTransition::Committed(first_epoch))
        );
        assert!(discard_probe_identity_matches(
            &first_permit,
            &first_navigation,
            &first_permit,
            &first_navigation,
            first_epoch,
        ));

        let second_epoch = first_navigation.begin("https://second.example/").unwrap();
        assert_ne!(first_epoch, second_epoch);
        assert!(!discard_probe_identity_matches(
            &first_permit,
            &first_navigation,
            &first_permit,
            &first_navigation,
            first_epoch,
        ));

        let replacement_token = Arc::new(AtomicBool::new(true));
        let replacement_permit = EventPermit::bound(&replacement_token);
        let replacement_navigation = NavigationEpochTracker::new();
        let replacement_epoch = replacement_navigation
            .begin("https://second.example/")
            .unwrap();
        assert!(!discard_probe_identity_matches(
            &replacement_permit,
            &replacement_navigation,
            &first_permit,
            &first_navigation,
            second_epoch,
        ));
        assert!(!discard_probe_identity_matches(
            &first_permit,
            &first_navigation,
            &replacement_permit,
            &replacement_navigation,
            replacement_epoch,
        ));
    }
}
