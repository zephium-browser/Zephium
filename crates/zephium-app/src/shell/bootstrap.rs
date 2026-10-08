//! Authoritative session bootstrap and recovery admission.

use super::*;

impl Shell {
    pub(super) fn bootstrap(&mut self) {
        #[cfg(debug_assertions)]
        let started = *self
            .bootstrap_started
            .get_or_insert_with(std::time::Instant::now);
        if self.profile_deletion.failed_closed {
            return;
        }
        // Runtime update state is independent of session recovery and chrome
        // reloads. Querying the sticky engine state also repairs a callback
        // that arrived before the shell callback ingress was installed.
        if !self.reconcile_runtime_restart_requirement() {
            self.project_runtime_status();
        }
        // The chrome re-invokes bootstrap whenever its webview reloads (dev
        // HMR, crash recovery); state and native surfaces must not be rebuilt.
        if self.windows.focused().is_some() {
            let _ = self.relayout();
            self.forget_delivered_icons(zephium_ipc::IconSurface::Chrome);
            self.project_items();
            self.project_browser_page();
            self.resume_focus();
            return;
        }
        let pending_deletions = match self.store.pending_profile_deletions() {
            ProfileDeletionLoad::Loaded(pending) => pending,
            ProfileDeletionLoad::Failed => {
                crate::diagnostic!(
                    "bootstrap: profile deletion journal is unavailable; refusing initialization"
                );
                self.report_terminal_failure(ShellTerminalFailure::SessionUnavailable);
                return;
            }
        };
        let mut journal_profiles = std::collections::HashSet::new();
        if pending_deletions.len() > zephium_core::session::MAX_SESSION_PROFILES
            || pending_deletions
                .iter()
                .any(|deletion| !journal_profiles.insert(deletion.profile))
        {
            crate::diagnostic!(
                "bootstrap: profile deletion journal exceeds its unique bounded cohort"
            );
            self.report_terminal_failure(ShellTerminalFailure::SessionUnavailable);
            return;
        }
        let mut active_item = None;
        let mut active_space = None;
        let mut splits = None;
        let mut session_absent = false;
        let mut blocker_configs = None;
        let mut load = self.store.load_session();
        if let SessionLoad::RecoveryRequired { reason } = &load {
            // A session that cannot be restored must not keep the browser
            // from opening at every launch. The store keeps its bytes in a
            // file and restarts it from the profile registry, so each
            // profile keeps its history, bookmarks and settings.
            crate::diagnostic!(
                "bootstrap: session cannot be restored ({reason}); setting it aside"
            );
            if let Some((restarted, _file)) = self.store.set_aside_session() {
                if !matches!(
                    restarted,
                    SessionLoad::RecoveryRequired { .. } | SessionLoad::Failed
                ) {
                    self.session_set_aside = true;
                    load = restarted;
                }
            }
        }
        match load {
            SessionLoad::Loaded {
                state,
                blocker_configs: loaded_blocker_configs,
            } => {
                let restored = session::restore(state);
                self.recently_closed = restored.recently_closed;
                self.profiles = restored.profiles;
                self.spaces = restored.spaces;
                self.items = restored.items;
                active_item = restored.active_item;
                active_space = restored.active_space;
                splits = restored.splits;
                blocker_configs = Some(loaded_blocker_configs);
            }
            SessionLoad::LoadedWithDegradedProfiles {
                state,
                profiles,
                blocker_configs: loaded_blocker_configs,
            } => {
                let session_profiles: std::collections::HashSet<_> =
                    state.profiles.iter().map(|profile| profile.id).collect();
                let mut degraded = std::collections::HashSet::new();
                if profiles.is_empty()
                    || profiles.len() > zephium_core::session::MAX_SESSION_PROFILES
                    || profiles.iter().any(|profile| {
                        !session_profiles.contains(profile) || !degraded.insert(*profile)
                    })
                {
                    crate::diagnostic!(
                        "bootstrap: ancillary-profile degradation report is invalid; refusing initialization"
                    );
                    return;
                }
                let mut labels: Vec<_> = degraded.iter().map(ToString::to_string).collect();
                labels.sort_unstable();
                crate::diagnostic!(
                    "bootstrap: ancillary history/favicon storage is disabled for profiles: {}",
                    labels.join(",")
                );
                self.degraded_storage_profiles = degraded;

                let restored = session::restore(state);
                self.recently_closed = restored.recently_closed;
                self.profiles = restored.profiles;
                self.spaces = restored.spaces;
                self.items = restored.items;
                active_item = restored.active_item;
                active_space = restored.active_space;
                splits = restored.splits;
                blocker_configs = Some(loaded_blocker_configs);
            }
            SessionLoad::Absent => session_absent = true,
            SessionLoad::RecoveryRequired { reason } => {
                // Setting it aside failed: the exact bytes stay preserved in
                // read-only mode rather than being overwritten.
                crate::diagnostic!("bootstrap: explicit session recovery required: {reason}");
                self.report_terminal_failure(
                    if reason == zephium_core::ports::store::NEWER_SESSION_REASON {
                        ShellTerminalFailure::SessionFromNewerVersion
                    } else {
                        ShellTerminalFailure::SessionUnavailable
                    },
                );
                return;
            }
            SessionLoad::Failed => {
                // Never convert corruption or I/O failure into first-run
                // state. Since bootstrapped remains false, every persistence
                // path also refuses to overwrite the recoverable snapshot.
                crate::diagnostic!(
                    "bootstrap: session storage is unavailable; refusing initialization"
                );
                self.report_terminal_failure(ShellTerminalFailure::SessionUnavailable);
                return;
            }
        }
        // The authoritative Store must decode the earlier QA Settings-tab
        // snapshot byte-exactly before any mutation. Settings is window-scoped
        // again; retire only those inert typed rows before first projection.
        let retired_settings = self.items.retired_settings_tab_ids();
        for id in &retired_settings {
            if active_item == Some(*id) {
                active_item = None;
            }
            if splits.as_ref().is_some_and(|tree| tree.contains(*id)) {
                splits = None;
            }
            let effects = self.items.remove(*id);
            if !effects.is_empty() {
                crate::diagnostic!(
                    "bootstrap: retired Settings tab unexpectedly owned a native view"
                );
                return;
            }
        }
        if (!pending_deletions.is_empty() && session_absent)
            || pending_deletions
                .iter()
                .any(|deletion| self.profiles.get(deletion.profile).is_some())
        {
            // The store contract atomically removes every journaled profile
            // from a still-authoritative session. Any overlap/absence means
            // the two durable facts disagree; creating views would guess.
            crate::diagnostic!(
                "bootstrap: profile deletion journal conflicts with the authoritative session"
            );
            return;
        }
        if let Some(configs) = blocker_configs {
            if !self.initialize_blocker_cohort(configs) {
                crate::diagnostic!(
                    "bootstrap: blocker configuration cohort is not exact; refusing initialization"
                );
                return;
            }
        }

        for PendingProfileDeletion {
            profile,
            native_erasure_verified,
        } in pending_deletions
        {
            let phase = if native_erasure_verified {
                ProfileDeletionPhase::FinalizeReady
            } else {
                ProfileDeletionPhase::NativeReady
            };
            self.profile_deletion.states.insert(
                profile,
                ProfileDeletionState::new(phase, None, self.persistence.session_revision),
            );
            // Normally no aggregate row remains. This defensive cleanup also
            // cancels any runtime-only work restored by a future caller.
            self.apply_profile_tombstone(profile);
        }
        let recovered_deletions: Vec<ProfileId> =
            self.profile_deletion.states.keys().copied().collect();
        let recovery_deadline = std::time::Instant::now() + PROFILE_DELETION_STORE_TIMEOUT;
        self.profile_deletion.batch_deadline = Some(recovery_deadline);
        for profile in recovered_deletions {
            if std::time::Instant::now() < recovery_deadline {
                self.drive_profile_deletion(profile);
            } else {
                self.schedule_profile_deletion_retry(profile);
            }
            if self.profile_deletion.failed_closed {
                self.profile_deletion.batch_deadline = None;
                crate::diagnostic!(
                    "bootstrap: recovered profile deletion failed closed; refusing native view construction"
                );
                return;
            }
        }
        self.profile_deletion.batch_deadline = None;
        if splits
            .as_ref()
            .is_some_and(|tree| tree.tabs().len() > MAX_VISIBLE_PANES)
        {
            crate::diagnostic!("session: discarded oversized split layout");
            splits = None;
        }

        let Some(space) = active_space
            .or_else(|| {
                self.profiles
                    .default_profile()
                    .and_then(|p| self.spaces.first_for(p))
            })
            .or_else(|| self.create_default_space())
        else {
            crate::diagnostic!(
                "bootstrap: bounded profile/space aggregate cannot create first-run state"
            );
            return;
        };
        let Some(profile) = self.spaces.get(space).map(|space| space.profile) else {
            crate::diagnostic!("bootstrap: selected space has no authoritative profile ownership");
            return;
        };
        if !self.start_blocker_profile(profile) {
            crate::diagnostic!(
                "bootstrap: active profile policy compilation was not admitted; content views remain unavailable"
            );
        }

        // Restore is semi-trusted input. Recheck focus and every pane against
        // the actual window selected after fallback, before creating any
        // native view or assigning a renderer partition.
        active_item = active_item.filter(|id| self.item_in_scope(*id, profile, space));
        splits = splits.filter(|tree| self.pane_in_scope(tree, profile, space));

        let window = self
            .windows
            .create(WindowKind::Main, profile, space, self.pending_size);
        // Starting with a new tab keeps every restored tab where it was and
        // loads none of them. A link that launched the browser is its own
        // new tab, and a blank tab left in front already is one.
        let start_fresh = self.tab_preferences.start_with_new_tab
            && self.pending_external.is_empty()
            && active_item
                .is_some_and(|id| self.items.tab(id).is_some_and(|tab| tab.url.is_some()));
        let mut fx = Vec::new();
        if let Some(tree) = splits {
            if !start_fresh {
                for leaf in tree.tabs() {
                    fx.extend(self.items.ensure_view(leaf));
                    self.touch(leaf);
                }
            }
            if let Some(win) = self.windows.get_mut(window) {
                win.splits = Some(tree);
            }
        }
        let restored = if start_fresh {
            None
        } else {
            active_item.or_else(|| self.today_tabs(space).first().copied())
        };
        match restored {
            Some(item) => fx.extend(self.focus_tab(item)),
            None => fx.extend(self.open_tab()),
        }
        // Restored/hibernated tabs may not create a native view and therefore
        // have no URL/load callback to trigger favicon hydration. Load the
        // active space's already-decoded fixed rasters in one bounded store
        // request before the first sidebar projection.
        self.hydrate_favicon_cache(profile, space);
        self.apply(fx);
        let _ = self.relayout();
        self.project_items();
        self.project_browser_page();
        self.bootstrapped = true;
        self.resume_focus();
        self.apply_deferred_web_extensions();
        let waiting = std::mem::take(&mut self.pending_external);
        self.open_external(waiting);
        if !retired_settings.is_empty() {
            self.schedule_persist();
        }
        #[cfg(debug_assertions)]
        eprintln!(
            "bootstrap: native session ready at {}ms",
            started.elapsed().as_millis()
        );
        if session_absent {
            // Register the first-run profile with Store immediately. Extension
            // management and other profile-scoped actors must never observe a
            // chrome-visible profile that exists only in volatile Shell state.
            self.persist();
        }
    }

    pub(super) fn create_default_space(&mut self) -> Option<SpaceId> {
        let mut created_profile = None;
        let profile = if let Some(profile) = self.profiles.default_profile() {
            profile
        } else {
            let mut inserted = None;
            for _ in 0..8 {
                let id = ProfileId::generate();
                if self.profiles.insert(Profile {
                    id,
                    name: "Personal".into(),
                    kind: ProfileKind::Default,
                }) {
                    inserted = Some(id);
                    break;
                }
            }
            let id = inserted?;
            created_profile = Some(id);
            id
        };
        if !self.blocker.profiles.contains_key(&profile)
            && !self.initialize_new_blocker_profile(profile)
        {
            if created_profile == Some(profile) {
                self.profiles.remove(profile);
            }
            return None;
        }
        for _ in 0..8 {
            let id = SpaceId::generate();
            if self.spaces.insert(Space {
                id,
                profile,
                name: "Space".into(),
            }) {
                return Some(id);
            }
        }
        if let Some(profile) = created_profile {
            self.blocker.discard_uncommitted_profile(profile);
            self.finish_terminalized_blocker_native_operations();
            self.profiles.remove(profile);
        }
        None
    }
}
