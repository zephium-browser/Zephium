//! Persistent profiles the person makes, names, draws and switches between.
//! A switch shows another profile in the window the way private browsing
//! shows its own: the profile left behind keeps its tabs but not its pages.

use super::*;
use zephium_core::profiles::{profile_name, ProfileAvatar, ProfileAvatarStyle};
use zephium_ipc::{ProfileAvatarStyleView, ProfileAvatarView, ProfileEntryView, ProfilesView};

#[derive(Default)]
pub(super) struct RosterState {
    avatars: std::collections::HashMap<ProfileId, ProfileAvatar>,
    /// The tab each profile had in front when the window last left it.
    left_at: std::collections::HashMap<ProfileId, ItemId>,
}

pub(super) struct ProfileCreationValidation {
    pub(super) source: ProfileId,
    pub(super) name: String,
    pub(super) avatar: ProfileAvatar,
    pub(super) photo: Option<Arc<Vec<u8>>>,
    pub(super) folder: i64,
    pub(super) switch: bool,
    pub(super) exists: Option<bool>,
    pub(super) generation: u64,
}

fn avatar_view(avatar: ProfileAvatar) -> ProfileAvatarView {
    ProfileAvatarView {
        style: match avatar.style {
            ProfileAvatarStyle::Initials => ProfileAvatarStyleView::Initials,
            ProfileAvatarStyle::Symbol => ProfileAvatarStyleView::Symbol,
            ProfileAvatarStyle::Photo => ProfileAvatarStyleView::Photo,
        },
        color: avatar.color,
        symbol: avatar.symbol,
        revision: avatar.revision,
    }
}

impl Shell {
    pub(super) fn load_profile_avatars(&mut self) {
        self.roster.avatars = self
            .store
            .load_profile_avatars()
            .into_iter()
            .filter(|(profile, _)| self.profiles.get(*profile).is_some())
            .collect();
    }

    pub(super) fn profile_avatar(&self, profile: ProfileId) -> ProfileAvatar {
        self.roster
            .avatars
            .get(&profile)
            .copied()
            .unwrap_or_else(|| ProfileAvatar::initial(profile))
    }

    pub(super) fn forget_profile_roster(&mut self, profile: ProfileId) {
        self.roster.avatars.remove(&profile);
        self.roster.left_at.remove(&profile);
        self.forget_space_memory(profile);
        self.project_profiles();
    }

    fn rostered(&self, profile: ProfileId) -> bool {
        self.profiles
            .get(profile)
            .is_some_and(|candidate| candidate.kind != ProfileKind::Incognito)
            && !self.profile_deletion_quarantines(profile)
    }

    pub(super) fn profiles_view(&self) -> Option<ProfilesView> {
        if !self.bootstrapped {
            return None;
        }
        let private = self.private_shown();
        let current = self.windows.focused().map(|window| window.profile);
        let current = if private {
            self.private
                .as_ref()
                .map(|session| session.regular_profile())
        } else {
            current
        };
        Some(ProfilesView {
            current: current.map(|profile| profile.to_string()),
            private,
            private_tabs: self.private_tab_count(),
            profiles: self
                .profiles
                .iter()
                .filter(|profile| self.rostered(profile.id))
                .map(|profile| ProfileEntryView {
                    id: profile.id.to_string(),
                    name: profile.name.clone(),
                    kind: match profile.kind {
                        ProfileKind::Default => zephium_ipc::ProfileKindView::Default,
                        ProfileKind::Named => zephium_ipc::ProfileKindView::Named,
                        ProfileKind::Incognito => zephium_ipc::ProfileKindView::Incognito,
                    },
                    avatar: avatar_view(self.profile_avatar(profile.id)),
                })
                .collect(),
        })
    }

    pub(super) fn project_profiles(&self) {
        if let Some(view) = self.profiles_view() {
            (self.emit)(Projection::Profiles(view));
        }
    }

    fn admitted_avatar(
        avatar: ProfileAvatar,
        photo: Option<&Arc<Vec<u8>>>,
    ) -> Option<ProfileAvatar> {
        let photo_style = avatar.style == ProfileAvatarStyle::Photo;
        (avatar.valid()
            && photo_style == photo.is_some()
            && photo.is_none_or(|bytes| {
                !bytes.is_empty()
                    && bytes.len() <= zephium_core::profiles::MAX_PROFILE_AVATAR_PHOTO_BYTES
            }))
        .then_some(avatar)
    }

    pub(super) fn save_avatar(
        &mut self,
        profile: ProfileId,
        avatar: ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
    ) {
        let revision = self.profile_avatar(profile).revision.saturating_add(1);
        let avatar = ProfileAvatar { revision, ..avatar };
        self.roster.avatars.insert(profile, avatar);
        self.store.save_profile_avatar(
            profile,
            avatar,
            photo,
            Box::new(move |saved| {
                if !saved {
                    crate::diagnostic!("profiles: avatar was not saved");
                }
            }),
        );
    }

    pub(super) fn operation_create_profile(
        &mut self,
        name: &str,
        avatar: ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
        bookmark_folder: Option<i64>,
        switch: bool,
    ) -> OperationDisposition {
        self.begin_profile_creation(None, name, avatar, photo, bookmark_folder, switch)
            .unwrap_or_else(|| {
                operation_result(
                    OperationOutcome::Deferred,
                    OperationReason::StoreWorkPending,
                )
            })
    }

    pub(super) fn begin_profile_creation(
        &mut self,
        operation_id: Option<String>,
        name: &str,
        avatar: ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
        bookmark_folder: Option<i64>,
        switch: bool,
    ) -> Option<OperationDisposition> {
        if !self.bootstrapped {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::NoFocusedWindow,
            ));
        }
        let (Some(name), Some(avatar)) = (
            profile_name(name),
            Self::admitted_avatar(avatar, photo.as_ref()),
        ) else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidInput,
            ));
        };
        if self
            .profiles
            .iter()
            .count()
            .saturating_add(self.pending_profile_creations.len())
            >= zephium_core::session::MAX_SESSION_PROFILES
        {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            ));
        }
        if let Some(folder) = bookmark_folder {
            let Some(source) = self
                .profiles_view()
                .and_then(|view| view.current)
                .and_then(|profile| ProfileId::parse(&profile))
            else {
                return Some(operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::NoFocusedWindow,
                ));
            };
            if !self.rostered(source) {
                return Some(operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::InvalidScope,
                ));
            }
            let Some(callback) = self.self_queue.as_ref().map(|queue| CallbackHandle {
                queue: std::sync::Arc::downgrade(&queue.inner),
            }) else {
                return Some(operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::StoreAdmissionRejected,
                ));
            };
            self.next_profile_creation_generation =
                self.next_profile_creation_generation.wrapping_add(1);
            let generation = self.next_profile_creation_generation;
            self.pending_profile_creations
                .insert(generation, operation_id.clone());
            let name = name.to_owned();
            self.store.bookmark_folders_async(
                source,
                Box::new(move |folders| {
                    let exists = folders
                        .map(|folders| folders.iter().any(|candidate| candidate.id == folder));
                    let rejected = Command::ProfileBookmarkFolderValidated {
                        generation,
                        operation_id: operation_id.clone(),
                        source,
                        name: name.clone(),
                        avatar,
                        photo: photo.clone(),
                        folder,
                        switch,
                        exists: None,
                    };
                    if !callback.dispatch(Command::ProfileBookmarkFolderValidated {
                        generation,
                        operation_id,
                        source,
                        name,
                        avatar,
                        photo,
                        folder,
                        switch,
                        exists,
                    }) {
                        callback.dispatch(rejected);
                    }
                }),
            );
            return None;
        }
        self.next_profile_creation_generation =
            self.next_profile_creation_generation.wrapping_add(1);
        let generation = self.next_profile_creation_generation;
        self.pending_profile_creations
            .insert(generation, operation_id.clone());
        self.create_profile_validated(name, avatar, photo, bookmark_folder, switch, generation)
    }

    pub(super) fn finish_profile_creation(
        &mut self,
        validation: ProfileCreationValidation,
    ) -> OperationDisposition {
        if validation.exists != Some(true) {
            return operation_result(
                OperationOutcome::Rejected,
                if validation.exists == Some(false) {
                    OperationReason::InvalidInput
                } else {
                    OperationReason::StoreAdmissionRejected
                },
            );
        }
        let current = self
            .profiles_view()
            .and_then(|view| view.current)
            .and_then(|profile| ProfileId::parse(&profile));
        if !self.bootstrapped
            || current != Some(validation.source)
            || !self.rostered(validation.source)
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        self.create_profile_validated(
            validation.name,
            validation.avatar,
            validation.photo,
            Some(validation.folder),
            validation.switch,
            validation.generation,
        )
        .unwrap_or_else(|| {
            operation_result(
                OperationOutcome::Deferred,
                OperationReason::StoreWorkPending,
            )
        })
    }

    fn create_profile_validated(
        &mut self,
        name: String,
        avatar: ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
        bookmark_folder: Option<i64>,
        switch: bool,
        generation: u64,
    ) -> Option<OperationDisposition> {
        let Some(profile) = (0..8).map(|_| ProfileId::generate()).find(|id| {
            !self.profile_deletion.states.contains_key(id)
                && self.profiles.insert(Profile {
                    id: *id,
                    name: name.clone(),
                    kind: ProfileKind::Named,
                })
        }) else {
            self.pending_profile_creations.remove(&generation);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            ));
        };
        if !self.profiles.set_bookmark_folder(profile, bookmark_folder) {
            self.profiles.remove(profile);
            self.pending_profile_creations.remove(&generation);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidInput,
            ));
        }
        if !self.initialize_new_blocker_profile(profile) {
            self.profiles.remove(profile);
            self.pending_profile_creations.remove(&generation);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        }
        let space = (0..8).map(|_| SpaceId::generate()).find(|id| {
            self.spaces.insert(Space {
                id: *id,
                profile,
                name: "Space".into(),
                look: Default::default(),
            })
        });
        let Some(space) = space else {
            self.blocker.discard_uncommitted_profile(profile);
            self.finish_terminalized_blocker_native_operations();
            self.profiles.remove(profile);
            self.pending_profile_creations.remove(&generation);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            ));
        };
        if self.self_queue.is_none() {
            if !self.publish_current_session_checked() {
                self.blocker.discard_uncommitted_profile(profile);
                self.finish_terminalized_blocker_native_operations();
                self.spaces.remove(space);
                self.profiles.remove(profile);
                self.pending_profile_creations.remove(&generation);
                return Some(operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::StoreAdmissionRejected,
                ));
            }
            self.pending_profile_creations.remove(&generation);
            self.save_avatar(profile, avatar, photo);
            self.project_profiles();
            return Some(if switch {
                self.operation_switch_profile(profile)
            } else {
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            });
        }
        let Some(session) = self.current_session_snapshot() else {
            self.blocker.discard_uncommitted_profile(profile);
            self.finish_terminalized_blocker_native_operations();
            self.spaces.remove(space);
            self.profiles.remove(profile);
            self.pending_profile_creations.remove(&generation);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            ));
        };
        let Some(callback) = self.self_queue.as_ref().map(|queue| CallbackHandle {
            queue: std::sync::Arc::downgrade(&queue.inner),
        }) else {
            self.rollback_profile_creation(profile, space, generation);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            ));
        };
        let store = self.store.clone();
        let publication_state = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
        self.pending_profile_rollbacks
            .insert(generation, (profile, space, publication_state.clone()));
        std::thread::spawn(move || {
            let durable = if publication_state
                .compare_exchange(
                    0,
                    1,
                    std::sync::atomic::Ordering::AcqRel,
                    std::sync::atomic::Ordering::Acquire,
                )
                .is_ok()
            {
                store.save_session_checked(session)
            } else {
                false
            };
            let mut command = Some(Command::ProfileCreationPublished {
                generation,
                profile,
                space,
                avatar,
                photo,
                switch,
                durable,
            });
            while let Some(next) = command.take() {
                if callback.dispatch(next.clone()) {
                    break;
                }
                if !callback.is_accepting_commands() {
                    break;
                }
                command = Some(next);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });
        None
    }

    pub(super) fn rollback_profile_creation(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        generation: u64,
    ) {
        self.pending_profile_rollbacks.remove(&generation);
        self.blocker.discard_uncommitted_profile(profile);
        self.finish_terminalized_blocker_native_operations();
        self.spaces.remove(space);
        self.profiles.remove(profile);
        self.pending_profile_creations.remove(&generation);
        self.project_profiles();
    }

    pub(super) fn operation_rename_profile(
        &mut self,
        profile: ProfileId,
        name: &str,
    ) -> OperationDisposition {
        if !self.rostered(profile) || !self.profiles.rename(profile, name) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        self.project_items();
        self.project_profiles();
        self.schedule_persist();
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    pub(super) fn operation_set_profile_avatar(
        &mut self,
        profile: ProfileId,
        avatar: ProfileAvatar,
        photo: Option<Arc<Vec<u8>>>,
    ) -> OperationDisposition {
        if !self.rostered(profile) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(avatar) = Self::admitted_avatar(avatar, photo.as_ref()) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        let current = self.profile_avatar(profile);
        let unchanged = ProfileAvatar {
            revision: current.revision,
            ..avatar
        } == current;
        if photo.is_none() && current.revision > 0 && unchanged {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        self.save_avatar(profile, avatar, photo);
        self.project_profiles();
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    /// Switches the focused window to the neighbouring rostered profile,
    /// wrapping around in roster order.
    pub(super) fn operation_step_profile(&mut self, forward: bool) -> OperationDisposition {
        let current = match self.browser_after_return.as_deref() {
            Some(Command::SwitchProfile(queued)) => Some(*queued),
            _ if self.private_shown() => self
                .private
                .as_ref()
                .map(|session| session.regular_profile()),
            _ => self.windows.focused().map(|window| window.profile),
        };
        let Some(current) = current else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let rostered: Vec<ProfileId> = self
            .profiles
            .iter()
            .map(|profile| profile.id)
            .filter(|profile| self.rostered(*profile))
            .collect();
        let Some(index) = rostered.iter().position(|profile| *profile == current) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        if rostered.len() < 2 {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let next = if forward {
            (index + 1) % rostered.len()
        } else {
            (index + rostered.len() - 1) % rostered.len()
        };
        self.operation_switch_profile(rostered[next])
    }

    pub(super) fn operation_switch_profile(&mut self, profile: ProfileId) -> OperationDisposition {
        if !self.rostered(profile) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if self.active_browser_page().is_some() && !self.browser_return_ready {
            self.browser_after_return = Some(Box::new(Command::SwitchProfile(profile)));
            return self.operation_show_browser_page(None);
        }
        let mut effects = Vec::new();
        if self.private_shown() {
            effects.extend(self.swap_private_scope());
        }
        let Some((leaving, active)) = self
            .windows
            .focused()
            .map(|window| (window.profile, window.active))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if leaving == profile {
            return if effects.is_empty() {
                operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged)
            } else {
                mutation_result(self.commit(effects))
            };
        }
        let Some(space) = self.remembered_space(profile) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        if self.work_pane.is_some() {
            let _ = self.operation_work_pane_hide();
        }
        if let Some(active) = active {
            self.roster.left_at.insert(leaving, active);
        }
        self.remember_space_left();
        self.engine.cancel_automatic_pip();
        for id in self.items.view_ids() {
            if self.profile_of_item(id) == Some(leaving) {
                effects.extend(self.items.hibernate(id));
            }
        }
        if let Some(window) = self.windows.focused_mut() {
            window.profile = profile;
            window.space = space;
            window.active = None;
            window.splits = None;
        }
        if !self.start_blocker_profile(profile) {
            crate::diagnostic!("profiles: content policy compilation was not admitted");
        }
        let returning = self
            .roster
            .left_at
            .get(&profile)
            .copied()
            .filter(|id| self.item_in_scope(*id, profile, space))
            .or_else(|| self.today_tabs(space).first().copied());
        match returning {
            Some(id) => effects.extend(self.focus_tab(id)),
            None => effects.extend(self.open_tab()),
        }
        self.hydrate_favicon_cache(profile, space);
        let native = self.commit(effects);
        self.project_profiles();
        mutation_result(native)
    }

    pub(super) fn profile_avatar_photo(
        &self,
        profile: ProfileId,
        reply: std::sync::mpsc::SyncSender<Option<Vec<u8>>>,
    ) {
        if !self.rostered(profile)
            || self.profile_avatar(profile).style != ProfileAvatarStyle::Photo
        {
            let _ = reply.try_send(None);
            return;
        }
        self.store.profile_avatar_photo(
            profile,
            Box::new(move |photo| {
                let _ = reply.try_send(photo);
            }),
        );
    }
}
