//! Crash-resumable profile-deletion authorization and native erasure.

use super::*;

pub(super) const PROFILE_DELETION_STORE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(2);
const NOTES_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const PROFILE_DELETION_RETRY_BASE: std::time::Duration = std::time::Duration::from_millis(250);
const PROFILE_DELETION_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) enum ProfileDeletionPhase {
    /// No Store deletion authority exists yet. The next action builds a
    /// fresh post-removal snapshot and authorizes it.
    AwaitingAuthorization,
    /// Authorization RPC entered the storage actor but its result missed the
    /// caller deadline. An ordered journal read must resolve that attempt
    /// before a retry builds a fresh post-removal snapshot from the current
    /// aggregates. Retaining the original snapshot here would let a later
    /// retry overwrite survivor mutations accepted in the meantime.
    Authorizing {
        may_reauthorize: bool,
    },
    /// `AlreadyAuthorized` proved the barrier, but an exact journal read is
    /// still needed to choose native erasure versus local-only finalization.
    ResolveAuthorizedJournal,
    NotesReady {
        native_erasure_verified: bool,
    },
    NotesInFlight {
        attempt: u64,
        deadline: std::time::Instant,
        native_erasure_verified: bool,
    },
    NativeReady,
    NativeInFlight {
        attempt: u64,
    },
    FinalizeReady,
    /// A Store/application invariant failed closed. The durable journal and
    /// native data remain untouched while the composition root terminates
    /// the process.
    FailedClosed,
}

pub(super) struct ProfileDeletionState {
    pub(super) phase: ProfileDeletionPhase,
    pub(super) operation_id: Option<String>,
    /// Session revision represented by the snapshot used for the latest
    /// authorization attempt. If journal proof arrives after this changes, the
    /// authorization barrier is still valid but the newer survivor state needs
    /// a fresh ordinary persistence pass after the logical tombstone lands.
    pub(super) authorization_revision: u128,
    pub(super) attempt_generation: u64,
    pub(super) retry_generation: u64,
    pub(super) retry_exponent: u8,
}

impl ProfileDeletionState {
    pub(super) fn new(
        phase: ProfileDeletionPhase,
        operation_id: Option<String>,
        authorization_revision: u128,
    ) -> Self {
        Self {
            phase,
            operation_id,
            authorization_revision,
            attempt_generation: 0,
            retry_generation: 0,
            retry_exponent: 0,
        }
    }
}

pub(super) enum ProfileDeletionProof {
    NotesReleased,
    NativeErased(ProfileDataErasureOutcome),
}
pub(super) type ProfileDeletionInbox =
    Arc<Mutex<std::collections::HashMap<ProfileId, (u64, ProfileDeletionProof)>>>;

pub(super) struct ProfileDeletionCoordinator {
    pub(super) states: std::collections::HashMap<ProfileId, ProfileDeletionState>,
    pub(super) inbox: ProfileDeletionInbox,
    pub(super) batch_deadline: Option<std::time::Instant>,
    /// A failed invariant permanently closes deletion and bootstrap progress
    /// for this process while the composition root converges on shutdown.
    pub(super) failed_closed: bool,
}

impl Default for ProfileDeletionCoordinator {
    fn default() -> Self {
        Self {
            states: std::collections::HashMap::new(),
            inbox: Arc::new(Mutex::new(std::collections::HashMap::new())),
            batch_deadline: None,
            failed_closed: false,
        }
    }
}

impl Shell {
    pub(super) fn begin_profile_deletion(
        &mut self,
        profile: ProfileId,
        operation_id: Option<String>,
    ) -> OperationDisposition {
        if self.profile_deletion.failed_closed {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        }
        // One foreground deletion at a time keeps durable/native ownership
        // unambiguous. Crash-recovered journal rows may coexist, but a new
        // deletion waits until those obligations are resolved.
        if !self.profile_deletion.states.is_empty() {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ProfileDeletionInProgress,
            );
        }
        let Some(filtered) = self.filtered_session_for_profile_deletion(profile) else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ProfileDeletionPolicyRejected,
            );
        };
        // Do not begin deletion while a native page can still execute, and do
        // not rely on a later best-effort close as proof. The actor's view bit
        // is the exact native lifecycle obligation: it remains set through
        // construction and discard until the corresponding native terminal
        // event.
        if self
            .items
            .view_ids()
            .into_iter()
            .any(|id| self.profile_of_item(id) == Some(profile))
        {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ProfileDeletionPolicyRejected,
            );
        }
        // Validate policy and canonical snapshot construction up front. The
        // snapshot is rebuilt at authorization so this preflight never becomes
        // authority.
        drop(filtered);
        let authorization_revision = self.persistence.session_revision;
        self.profile_deletion.states.insert(
            profile,
            ProfileDeletionState::new(
                ProfileDeletionPhase::AwaitingAuthorization,
                operation_id,
                authorization_revision,
            ),
        );
        if self.user_content_status.retire_profile(profile) {
            self.project_runtime_status();
        }
        // From this point through durable tombstoning (or process restart),
        // the state row is a process-local quarantine and is never removed on
        // a retryable result.
        self.cancel_pending_blocker_mutation(profile, OperationReason::ProfileDeletionInProgress);
        self.retire_blocker_statistics(profile);
        self.blocker.retire_profile(profile);
        self.finish_terminalized_blocker_native_operations();
        operation_result(
            OperationOutcome::Deferred,
            OperationReason::StoreWorkPending,
        )
    }

    /// Builds the exact canonical post-removal state without mutating any
    /// aggregate. Only an inactive, non-default, non-private profile may pass.
    pub(super) fn filtered_session_for_profile_deletion(
        &self,
        profile: ProfileId,
    ) -> Option<session::SessionState> {
        let candidate = self.profiles.get(profile)?;
        if !self.bootstrapped
            || candidate.kind != ProfileKind::Named
            || self
                .windows
                .focused()
                .is_some_and(|window| window.profile == profile)
            || self
                .profiles
                .iter()
                .filter(|profile| profile.kind != ProfileKind::Incognito)
                .count()
                <= 1
        {
            return None;
        }

        let window = self.windows.focused();
        let mut filtered = session::snapshot_with_recently_closed(
            &self.profiles,
            &self.spaces,
            &self.items,
            window.map(|window| window.space),
            window.and_then(|window| window.active),
            window.and_then(|window| window.splits.as_ref()),
            &self.recently_closed,
        );
        if !filtered
            .profiles
            .iter()
            .any(|candidate| candidate.id == profile)
        {
            return None;
        }
        let removed_spaces: std::collections::HashSet<SpaceId> = filtered
            .spaces
            .iter()
            .filter(|space| space.profile == profile)
            .map(|space| space.id)
            .collect();
        filtered
            .profiles
            .retain(|candidate| candidate.id != profile);
        filtered
            .recently_closed
            .retain(|entry| entry.profile != profile);
        filtered.spaces.retain(|space| space.profile != profile);
        filtered.items.retain(|item| {
            let (Placement::Favorites { space } | Placement::Space { space, .. }) = item.placement;
            !removed_spaces.contains(&space)
        });

        (session::canonicalize(filtered.clone()) == filtered).then_some(filtered)
    }

    /// Applies only the logical half of an already-durable authorization.
    /// Native `Close` effects are deliberately suppressed: the engine's
    /// profile erasure owns closure and exact retirement of every view.
    pub(super) fn apply_profile_tombstone(&mut self, profile: ProfileId) {
        self.retire_web_extensions(profile);
        self.engine.forget_discarded_state(profile, None);
        self.continued_certificates
            .retain(|(owner, _)| *owner != profile);
        self.recently_closed
            .retain(|entry| entry.profile != profile);
        self.zoom.forget_profile(profile);
        self.extension_browser_surfaces.retire_profile(profile);
        self.extension_actions.retire_profile(profile);
        // Retire compiler admission and erase any callback result before the
        // native erasure boundary. A late list build can never reinstall
        // policy state for a journal-authorized profile.
        self.cancel_pending_blocker_mutation(profile, OperationReason::ProfileDeletionInProgress);
        self.retire_blocker_statistics(profile);
        self.blocker.retire_profile(profile);
        self.finish_terminalized_blocker_native_operations();
        let mut favicon_items: std::collections::HashSet<ItemId> = self
            .favicons
            .icon_attempts
            .iter()
            .filter_map(|(id, attempt)| (attempt.target.profile == profile).then_some(*id))
            .collect();
        favicon_items.extend(
            self.favicons
                .store_reads
                .iter()
                .filter_map(|(id, pending)| (pending.target.profile == profile).then_some(*id)),
        );
        favicon_items.extend(
            self.favicons
                .settled
                .iter()
                .filter_map(|(id, target)| (target.profile == profile).then_some(*id)),
        );
        for id in favicon_items {
            self.cancel_favicon_attempt(id);
        }
        let discard_items: Vec<ItemId> = self
            .residency
            .discard_probes
            .keys()
            .copied()
            .filter(|id| self.profile_of_item(*id) == Some(profile))
            .collect();
        for id in discard_items {
            self.residency.discard_probes.remove(&id);
            if let Some(queue) = &self.self_queue {
                queue.cancel_discard_probe(id);
            }
        }

        let removed_spaces = self.spaces.remove_for_profile(profile);
        let _native_erasure_owns_close = self.items.remove_for_profile(&removed_spaces);
        self.profiles.remove(profile);
        self.forget_profile_roster(profile);

        self.residency
            .recent
            .retain(|id| self.items.tab(*id).is_some());
        self.residency
            .last_focus
            .retain(|id, _| self.items.tab(*id).is_some());
        self.last_visits
            .retain(|id, _| self.items.tab(*id).is_some());
        self.residency
            .dormant_sent
            .retain(|id| self.items.tab(*id).is_some());
        self.residency
            .discard_protected_until
            .retain(|id, _| self.items.tab(*id).is_some());
        self.crash
            .crashes
            .retain(|id, _| self.items.tab(*id).is_some());
        self.crash
            .presentations
            .retain(|id| self.items.tab(*id).is_some());
        self.favicons
            .icons_checked
            .retain(|(item_profile, _)| *item_profile != profile);
        self.favicons
            .icon_sources
            .retain(|(item_profile, _), _| *item_profile != profile);
        self.favicons.failed.forget_profile(profile);
        self.favicons.unfetched.forget_profile(profile);
        self.favicons
            .icon_values
            .retain(|(item_profile, _), _| *item_profile != profile);
        self.favicons
            .icon_cache_order
            .retain(|(item_profile, _)| *item_profile != profile);
        if self
            .favicons
            .pending_batch
            .as_ref()
            .is_some_and(|pending| pending.profile == profile)
        {
            self.favicons.pending_batch = None;
        }
        if self
            .search
            .pending
            .as_ref()
            .is_some_and(|pending| pending.profile == profile)
        {
            self.search.pending = None;
        }

        // The authorization transaction already published the exact session.
        // An older debounce must never later overwrite that barrier.
        self.persistence.persist_first_dirty = None;
        if let Some(queue) = &self.self_queue {
            queue.cancel_persist();
        }
        self.project_items();
    }

    pub(super) fn drive_profile_deletion(&mut self, profile: ProfileId) {
        if self.profile_deletion.failed_closed {
            return;
        }
        enum Action {
            Authorize,
            ReconcileAuthorization(bool),
            ResolveAuthorizedJournal,
            ReleaseNotes,
            WaitForNotes(std::time::Instant),
            RetryNotes,
            EraseNative,
            Finalize,
            None,
        }

        let action = {
            let Some(state) = self.profile_deletion.states.get_mut(&profile) else {
                return;
            };
            match state.phase {
                ProfileDeletionPhase::AwaitingAuthorization => Action::Authorize,
                ProfileDeletionPhase::Authorizing {
                    may_reauthorize, ..
                } => Action::ReconcileAuthorization(may_reauthorize),
                ProfileDeletionPhase::ResolveAuthorizedJournal => Action::ResolveAuthorizedJournal,
                ProfileDeletionPhase::NotesReady { .. } => Action::ReleaseNotes,
                ProfileDeletionPhase::NotesInFlight {
                    deadline,
                    native_erasure_verified,
                    ..
                } => {
                    if std::time::Instant::now() >= deadline {
                        state.phase = ProfileDeletionPhase::NotesReady {
                            native_erasure_verified,
                        };
                        Action::RetryNotes
                    } else {
                        Action::WaitForNotes(deadline)
                    }
                }
                ProfileDeletionPhase::NativeReady => Action::EraseNative,
                ProfileDeletionPhase::NativeInFlight { .. } => Action::None,
                ProfileDeletionPhase::FinalizeReady => Action::Finalize,
                ProfileDeletionPhase::FailedClosed => Action::None,
            }
        };
        if let Some(queue) = &self.self_queue {
            queue.cancel_profile_deletion(profile);
        }
        match action {
            Action::Authorize => {
                let deadline = self.profile_deletion_deadline();
                self.authorize_profile_deletion(profile, deadline);
                if !self.profile_deletion.failed_closed
                    && !self
                        .profile_deletion
                        .states
                        .get(&profile)
                        .is_some_and(|state| {
                            matches!(
                                state.phase,
                                ProfileDeletionPhase::AwaitingAuthorization
                                    | ProfileDeletionPhase::FailedClosed
                            )
                        })
                {
                    self.drive_profile_deletion(profile);
                }
            }
            Action::ReconcileAuthorization(may_reauthorize) => {
                self.reconcile_profile_authorization(profile, may_reauthorize)
            }
            Action::ResolveAuthorizedJournal => self.resolve_authorized_journal(profile),
            Action::ReleaseNotes => {
                self.release_profile_notes(profile);
                self.consume_profile_deletion_outcome(profile);
            }
            Action::WaitForNotes(deadline) => self.schedule_notes_drain_deadline(profile, deadline),
            Action::RetryNotes => self.schedule_profile_deletion_retry(profile),
            Action::EraseNative => {
                self.start_native_profile_erasure(profile);
                // A native adapter may have completed synchronously into the
                // bounded inbox.
                self.consume_profile_deletion_outcome(profile);
            }
            Action::Finalize => {
                let deadline = self.profile_deletion_deadline();
                self.finalize_profile_deletion(profile, deadline);
            }
            Action::None => {}
        }
    }

    /// Process-local quarantine established when deletion begins. Restart may
    /// clear a row that never reached Store authorization; this process may
    /// not.
    pub(super) fn profile_deletion_quarantines(&self, profile: ProfileId) -> bool {
        self.profile_deletion.states.contains_key(&profile)
    }

    pub(super) fn profile_deletion_quarantines_item(&self, id: ItemId) -> bool {
        self.profile_of_item(id)
            .is_some_and(|profile| self.profile_deletion_quarantines(profile))
    }

    fn profile_deletion_deadline(&self) -> std::time::Instant {
        self.profile_deletion.batch_deadline.unwrap_or_else(|| {
            let now = std::time::Instant::now();
            now.checked_add(PROFILE_DELETION_STORE_TIMEOUT)
                .unwrap_or(now)
        })
    }

    fn fail_profile_deletion_invariant(&mut self, profile: ProfileId) {
        crate::diagnostic!(
            "profile deletion: Store invariant failed; preserving durable profile data"
        );
        if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
            state.phase = ProfileDeletionPhase::FailedClosed;
        }
        if let Some(queue) = &self.self_queue {
            queue.cancel_profile_deletion(profile);
        }
        self.profile_deletion.failed_closed = true;
        self.report_terminal_failure(ShellTerminalFailure::ProfileDeletionInvariant);
    }

    fn authorize_profile_deletion(&mut self, profile: ProfileId, deadline: std::time::Instant) {
        if !matches!(
            self.profile_deletion
                .states
                .get(&profile)
                .map(|state| &state.phase),
            Some(ProfileDeletionPhase::AwaitingAuthorization)
        ) {
            self.fail_profile_deletion_invariant(profile);
            return;
        }
        let Some(filtered) = self.filtered_session_for_profile_deletion(profile) else {
            self.fail_profile_deletion_invariant(profile);
            return;
        };
        if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
            state.authorization_revision = self.persistence.session_revision;
        }
        let outcome = self
            .store
            .authorize_profile_deletion(profile, filtered, deadline);
        self.handle_profile_authorization_outcome(profile, outcome);
    }

    fn reconcile_profile_authorization(&mut self, profile: ProfileId, may_reauthorize: bool) {
        match self.store.pending_profile_deletions() {
            ProfileDeletionLoad::Loaded(pending) => {
                if let Some(deletion) = pending
                    .into_iter()
                    .find(|deletion| deletion.profile == profile)
                {
                    self.authorization_is_durable(profile, deletion);
                    self.drive_profile_deletion(profile);
                    return;
                }
                if !may_reauthorize {
                    self.schedule_profile_deletion_retry(profile);
                    return;
                }
                // The ordered absence proves the previous attempt did not
                // publish an authorization. Re-run policy now; the fresh
                // snapshot is built again at authorization.
                if self
                    .filtered_session_for_profile_deletion(profile)
                    .is_none()
                {
                    // Authorizing is reachable only after policy passed once;
                    // a profile that no longer qualifies means the Store and
                    // application authorities disagree.
                    self.fail_profile_deletion_invariant(profile);
                    return;
                }
                if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                    state.authorization_revision = self.persistence.session_revision;
                    state.phase = ProfileDeletionPhase::AwaitingAuthorization;
                }
                self.drive_profile_deletion(profile);
            }
            ProfileDeletionLoad::Failed => self.schedule_profile_deletion_retry(profile),
        }
    }

    fn handle_profile_authorization_outcome(
        &mut self,
        profile: ProfileId,
        outcome: ProfileDeletionAuthorizeOutcome,
    ) {
        match outcome {
            ProfileDeletionAuthorizeOutcome::Authorized
            | ProfileDeletionAuthorizeOutcome::AlreadyAuthorized => {
                // The authorization result proves the session/journal
                // transaction, but it does not carry the durable native
                // namespace marker. Resolve the exact journal row before any
                // Engine erasure call; guessing `None` would lose authority.
                self.apply_profile_tombstone(profile);
                let Some(state) = self.profile_deletion.states.get_mut(&profile) else {
                    self.fail_profile_deletion_invariant(profile);
                    return;
                };
                state.phase = ProfileDeletionPhase::ResolveAuthorizedJournal;
                state.retry_exponent = 0;
            }
            ProfileDeletionAuthorizeOutcome::OutcomeUnknown => {
                // The ordered journal query is the only legal arbiter after a
                // deadline. Absence is not used to guess success; the next
                // bounded retry may re-run the idempotent authorization.
                if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                    state.phase = ProfileDeletionPhase::Authorizing {
                        may_reauthorize: false,
                    };
                    state.retry_exponent = 0;
                }
            }
            ProfileDeletionAuthorizeOutcome::NotRegistered
            | ProfileDeletionAuthorizeOutcome::SessionConflict
            | ProfileDeletionAuthorizeOutcome::InvalidSession => {
                // Policy/canonical validation already passed. These outcomes
                // can only mean the Store and application authorities
                // disagree; reactivating the quarantined profile would be
                // unsafe.
                self.fail_profile_deletion_invariant(profile)
            }
            ProfileDeletionAuthorizeOutcome::NotAdmitted
            | ProfileDeletionAuthorizeOutcome::Failed => {
                // No terminal Store contradiction was proven. Retain the
                // quarantined state and retry without touching aggregate/native
                // data.
                if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                    state.phase = ProfileDeletionPhase::AwaitingAuthorization;
                }
                self.schedule_profile_deletion_retry(profile);
            }
        }
    }

    fn resolve_authorized_journal(&mut self, profile: ProfileId) {
        match self.store.pending_profile_deletions() {
            ProfileDeletionLoad::Loaded(pending) => {
                if let Some(deletion) = pending
                    .into_iter()
                    .find(|deletion| deletion.profile == profile)
                {
                    self.authorization_is_durable(profile, deletion);
                    self.drive_profile_deletion(profile);
                } else {
                    // `AlreadyAuthorized` and a missing journal row conflict.
                    // Keep the logical tombstone and retry; never recreate the
                    // profile or infer that local deletion completed without
                    // an in-process native proof.
                    self.schedule_profile_deletion_retry(profile);
                }
            }
            ProfileDeletionLoad::Failed => self.schedule_profile_deletion_retry(profile),
        }
    }

    fn authorization_is_durable(&mut self, profile: ProfileId, deletion: PendingProfileDeletion) {
        if deletion.profile != profile {
            self.fail_profile_deletion_invariant(profile);
            return;
        }
        let survivor_state_changed = self
            .profile_deletion
            .states
            .get(&profile)
            .is_some_and(|state| state.authorization_revision != self.persistence.session_revision);
        self.apply_profile_tombstone(profile);
        if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
            state.phase = ProfileDeletionPhase::NotesReady {
                native_erasure_verified: deletion.native_erasure_verified,
            };
            state.retry_exponent = 0;
        }
        if survivor_state_changed {
            // The deletion authorization durably represents its own exact
            // snapshot, but newer survivor state was accepted while the reply
            // was uncertain. `apply_profile_tombstone` cancels the pre-barrier
            // debounce; establish a new post-barrier durability deadline
            // without pretending this scheduling pass is another mutation.
            self.schedule_current_session_persist();
        }
    }

    fn release_profile_notes(&mut self, profile: ProfileId) {
        let Some(state) = self.profile_deletion.states.get_mut(&profile) else {
            return;
        };
        let ProfileDeletionPhase::NotesReady {
            native_erasure_verified,
        } = state.phase
        else {
            return;
        };
        let Some(notes) = &self.notes else {
            state.phase = if native_erasure_verified {
                ProfileDeletionPhase::FinalizeReady
            } else {
                ProfileDeletionPhase::NativeReady
            };
            self.drive_profile_deletion(profile);
            return;
        };
        state.attempt_generation = state.attempt_generation.wrapping_add(1).max(1);
        let attempt = state.attempt_generation;
        let deadline = std::time::Instant::now() + NOTES_DRAIN_TIMEOUT;
        state.phase = ProfileDeletionPhase::NotesInFlight {
            attempt,
            deadline,
            native_erasure_verified,
        };
        let inbox = self.profile_deletion.inbox.clone();
        let wake = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        if !notes.release(
            profile,
            Box::new(move || {
                let mut pending = inbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if pending.len() < zephium_core::session::MAX_SESSION_PROFILES
                    || pending.contains_key(&profile)
                {
                    // A timed-out release may acknowledge after its retry.
                    // Never let that older proof replace the current one.
                    if pending
                        .get(&profile)
                        .is_none_or(|(generation, _)| *generation <= attempt)
                    {
                        pending.insert(profile, (attempt, ProfileDeletionProof::NotesReleased));
                    }
                }
                drop(pending);
                if let Some(wake) = wake {
                    let _ = wake.dispatch(Command::ProfileDeletionReady(profile));
                }
            }),
        ) {
            if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                state.phase = ProfileDeletionPhase::NotesReady {
                    native_erasure_verified,
                };
            }
            self.schedule_profile_deletion_retry(profile);
        } else {
            self.schedule_notes_drain_deadline(profile, deadline);
        }
    }

    fn schedule_notes_drain_deadline(&mut self, profile: ProfileId, deadline: std::time::Instant) {
        let Some(state) = self.profile_deletion.states.get_mut(&profile) else {
            return;
        };
        state.retry_generation = state.retry_generation.wrapping_add(1).max(1);
        if let Some(queue) = &self.self_queue {
            queue.schedule_profile_deletion(profile, state.retry_generation, deadline);
        }
    }

    fn start_native_profile_erasure(&mut self, profile: ProfileId) {
        if !self
            .profile_deletion
            .states
            .get(&profile)
            .is_some_and(|state| matches!(state.phase, ProfileDeletionPhase::NativeReady))
        {
            self.fail_profile_deletion_invariant(profile);
            return;
        }
        let attempt = {
            let state = self
                .profile_deletion
                .states
                .get_mut(&profile)
                .expect("phase validation retained exact deletion state");
            state.attempt_generation = state.attempt_generation.wrapping_add(1);
            if state.attempt_generation == 0 {
                state.attempt_generation = 1;
            }
            let attempt = state.attempt_generation;
            state.phase = ProfileDeletionPhase::NativeInFlight { attempt };
            state.retry_exponent = 0;
            attempt
        };
        let inbox = self.profile_deletion.inbox.clone();
        let wake = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        });
        self.engine.erase_profile_data(
            profile,
            Box::new(move |outcome| {
                let mut pending = inbox
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if pending.len() < zephium_core::session::MAX_SESSION_PROFILES
                    || pending.contains_key(&profile)
                {
                    pending.insert(
                        profile,
                        (attempt, ProfileDeletionProof::NativeErased(outcome)),
                    );
                }
                drop(pending);
                if let Some(wake) = wake {
                    let _ = wake.dispatch(Command::ProfileDeletionReady(profile));
                }
            }),
        );
    }

    pub(super) fn drain_profile_deletion_inbox(&mut self) {
        let profiles: Vec<ProfileId> = self
            .profile_deletion
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .copied()
            .collect();
        for profile in profiles {
            self.consume_profile_deletion_outcome(profile);
        }
    }

    pub(super) fn consume_profile_deletion_outcome(&mut self, profile: ProfileId) {
        let pending = self
            .profile_deletion
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&profile);
        let Some((attempt, outcome)) = pending else {
            return;
        };
        let Some(state) = self.profile_deletion.states.get_mut(&profile) else {
            return;
        };
        let outcome = match (outcome, &state.phase) {
            (
                ProfileDeletionProof::NotesReleased,
                ProfileDeletionPhase::NotesInFlight {
                    attempt: expected,
                    native_erasure_verified,
                    ..
                },
            ) if attempt == *expected => {
                state.phase = if *native_erasure_verified {
                    ProfileDeletionPhase::FinalizeReady
                } else {
                    ProfileDeletionPhase::NativeReady
                };
                state.retry_exponent = 0;
                self.drive_profile_deletion(profile);
                return;
            }
            (
                ProfileDeletionProof::NativeErased(outcome),
                ProfileDeletionPhase::NativeInFlight { attempt: expected },
            ) if attempt == *expected => outcome,
            _ => return,
        };
        match outcome {
            ProfileDataErasureOutcome::Verified => {
                if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                    state.phase = ProfileDeletionPhase::FinalizeReady;
                    state.retry_exponent = 0;
                }
                self.drive_profile_deletion(profile);
            }
            ProfileDataErasureOutcome::Failed | ProfileDataErasureOutcome::TimedOut => {
                if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                    state.phase = ProfileDeletionPhase::NativeReady;
                }
                self.schedule_profile_deletion_retry(profile);
            }
        }
    }

    fn finalize_profile_deletion(&mut self, profile: ProfileId, deadline: std::time::Instant) {
        if !self
            .profile_deletion
            .states
            .get(&profile)
            .is_some_and(|state| matches!(state.phase, ProfileDeletionPhase::FinalizeReady))
        {
            self.fail_profile_deletion_invariant(profile);
            return;
        }
        if std::time::Instant::now() >= deadline {
            self.schedule_profile_deletion_retry(profile);
            return;
        }
        let outcome = self.store.finalize_profile_deletion(profile, deadline);
        match outcome {
            ProfileDeletionFinalizeOutcome::Completed => self.finish_profile_deletion_operation(
                profile,
                OperationOutcome::Applied,
                OperationReason::ProfileDeletionCompleted,
            ),
            ProfileDeletionFinalizeOutcome::NotAuthorized => {
                // FinalizeReady is created only from a durable journal row
                // after native proof. Losing that authorization is a terminal
                // Store/application conflict.
                self.fail_profile_deletion_invariant(profile)
            }
            ProfileDeletionFinalizeOutcome::NotAdmitted
            | ProfileDeletionFinalizeOutcome::OutcomeUnknown
            | ProfileDeletionFinalizeOutcome::Failed => {
                if std::time::Instant::now() >= deadline {
                    self.schedule_profile_deletion_retry(profile);
                } else {
                    self.reconcile_profile_finalization(profile);
                }
            }
        }
    }

    fn reconcile_profile_finalization(&mut self, profile: ProfileId) {
        match self.store.pending_profile_deletions() {
            ProfileDeletionLoad::Loaded(pending) => {
                if pending.iter().any(|deletion| deletion.profile == profile) {
                    if let Some(state) = self.profile_deletion.states.get_mut(&profile) {
                        state.phase = ProfileDeletionPhase::FinalizeReady;
                    }
                    self.schedule_profile_deletion_retry(profile);
                } else {
                    // Journal removal is the store's last ordered step, so an
                    // authoritative absence after native proof establishes
                    // both phases complete even if the RPC reply was lost.
                    self.finish_profile_deletion_operation(
                        profile,
                        OperationOutcome::Applied,
                        OperationReason::ProfileDeletionCompleted,
                    );
                }
            }
            ProfileDeletionLoad::Failed => {
                self.schedule_profile_deletion_retry(profile);
            }
        }
    }

    fn finish_profile_deletion_operation(
        &mut self,
        profile: ProfileId,
        outcome: OperationOutcome,
        reason: OperationReason,
    ) {
        if outcome == OperationOutcome::Applied
            && reason == OperationReason::ProfileDeletionCompleted
        {
            self.degraded_storage_profiles.remove(&profile);
        }
        let operation_id = self
            .profile_deletion
            .states
            .remove(&profile)
            .and_then(|state| state.operation_id);
        if let Some(queue) = &self.self_queue {
            queue.cancel_profile_deletion(profile);
        }
        self.profile_deletion
            .inbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&profile);
        if let Some(operation_id) = operation_id {
            (self.emit)(Projection::OperationProcessed(OperationDisposition {
                operation_id,
                outcome,
                reason,
            }));
        }
    }

    pub(super) fn schedule_profile_deletion_retry(&mut self, profile: ProfileId) {
        let Some(state) = self.profile_deletion.states.get_mut(&profile) else {
            return;
        };
        if let ProfileDeletionPhase::Authorizing {
            may_reauthorize, ..
        } = &mut state.phase
        {
            *may_reauthorize = true;
        }
        state.retry_generation = state.retry_generation.wrapping_add(1);
        if state.retry_generation == 0 {
            state.retry_generation = 1;
        }
        let generation = state.retry_generation;
        let multiplier = 1_u32 << u32::from(state.retry_exponent.min(7));
        let delay = PROFILE_DELETION_RETRY_BASE
            .saturating_mul(multiplier)
            .min(PROFILE_DELETION_RETRY_MAX);
        state.retry_exponent = state.retry_exponent.saturating_add(1).min(7);
        if let Some(queue) = &self.self_queue {
            queue.schedule_profile_deletion(profile, generation, std::time::Instant::now() + delay);
        }
    }
}
