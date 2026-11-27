//! Profile-owned website-data containers and per-tab assignment.

use super::*;
use crate::ContainerCommand;
use zephium_core::containers::{container_name, Container, ContainerKind, SpaceAccess};

fn rejected(reason: OperationReason) -> OperationDisposition {
    operation_result(OperationOutcome::Rejected, reason)
}

impl Shell {
    fn schedule_container_recreate(
        &mut self,
        tab: zephium_core::ids::ItemId,
        container: Option<zephium_core::ids::ContainerId>,
    ) {
        let Some(url) = self.items.tab(tab).and_then(|state| state.url.clone()) else {
            return;
        };
        self.container_recreates.insert(tab, (container, url));
        if let Some(queue) = &self.self_queue {
            queue.schedule_view_capacity(
                tab,
                std::time::Instant::now() + std::time::Duration::from_millis(50),
            );
        }
    }

    pub(super) fn operation_container(
        &mut self,
        command: ContainerCommand,
    ) -> OperationDisposition {
        if !self.bootstrapped {
            return rejected(OperationReason::NoFocusedWindow);
        }
        let Some(window) = self.windows.focused() else {
            return rejected(OperationReason::NoFocusedWindow);
        };
        let profile = window.profile;
        if self.profile_deletion_quarantines(profile)
            || self
                .profiles
                .get(profile)
                .is_none_or(|p| p.kind == ProfileKind::Incognito)
        {
            return rejected(OperationReason::InvalidScope);
        }
        let space = window.space;
        match command {
            ContainerCommand::Create { name, look, spaces } => {
                if !look.valid() || !self.valid_container_spaces(profile, &spaces) {
                    return rejected(OperationReason::InvalidInput);
                }
                let count = self.containers.for_profile(profile).count();
                if count >= zephium_core::containers::MAX_PROFILE_CONTAINERS {
                    return rejected(OperationReason::ItemLimitReached);
                }
                let Some(name) = container_name(&name) else {
                    return rejected(OperationReason::InvalidInput);
                };
                let container = Container {
                    id: (0..8)
                        .map(|_| zephium_core::ids::ContainerId::generate())
                        .find(|id| !self.containers.contains(*id))
                        .unwrap(),
                    profile,
                    name,
                    kind: ContainerKind::Browsing,
                    look,
                    spaces: self.canonical_access(spaces),
                };
                let previous = self.containers.clone();
                if !self.containers.insert(container) {
                    return rejected(OperationReason::ItemLimitReached);
                }
                self.containers_changed(previous)
            }
            ContainerCommand::Update {
                id,
                name,
                look,
                spaces,
            } => {
                if !look.valid() || !self.valid_container_spaces(profile, &spaces) {
                    return rejected(OperationReason::InvalidInput);
                }
                let Some(existing) = self.containers.get(id) else {
                    return rejected(OperationReason::InvalidScope);
                };
                if existing.profile != profile {
                    return rejected(OperationReason::InvalidScope);
                }
                let Some(name) = container_name(&name) else {
                    return rejected(OperationReason::InvalidInput);
                };
                let previous = self.containers.clone();
                let mut updated = existing.clone();
                updated.name = name;
                updated.look = look;
                updated.spaces = self.canonical_access(spaces);
                if !self.containers.replace(updated) {
                    return rejected(OperationReason::InvalidInput);
                }
                self.containers_changed(previous)
            }
            ContainerCommand::RemoveFromSpace(id) => {
                let Some(existing) = self.containers.get(id) else {
                    return rejected(OperationReason::InvalidScope);
                };
                if existing.profile != profile || !existing.available_in(space) {
                    return rejected(OperationReason::InvalidScope);
                }
                let previous = self.containers.clone();
                let mut updated = existing.clone();
                let mut spaces = match updated.spaces {
                    SpaceAccess::All => self.spaces.of(profile),
                    SpaceAccess::Selected(spaces) => spaces,
                };
                spaces.retain(|candidate| *candidate != space);
                updated.spaces = SpaceAccess::Selected(spaces);
                self.containers.replace(updated);
                if !self.persist_containers() {
                    self.containers = previous;
                    return rejected(OperationReason::StoreAdmissionRejected);
                }
                let mut effects = Vec::new();
                for tab in self.items.tabs_using_container_in_space(id, space) {
                    let tab_effects = self.items.set_container(tab, None);
                    if !tab_effects.is_empty() {
                        self.schedule_container_recreate(tab, None);
                    }
                    effects.extend(tab_effects);
                }
                let native = self.commit(effects);
                self.project_items();
                mutation_result(native)
            }
            ContainerCommand::Delete(id) => {
                if self
                    .containers
                    .get(id)
                    .is_none_or(|entry| entry.profile != profile)
                {
                    return rejected(OperationReason::InvalidScope);
                }
                if self.items.container_is_in_use(id) {
                    return rejected(OperationReason::InvalidInput);
                }
                // Keep the record until the native data store can be erased
                // through the platform's durable-data deletion protocol.
                rejected(OperationReason::UnsupportedCommand)
            }
            ContainerCommand::AssignTab { tab, container } => {
                if !self.item_in_scope(tab, profile, space) {
                    return rejected(OperationReason::InvalidScope);
                }
                if let Some(id) = container {
                    if self
                        .containers
                        .get(id)
                        .is_none_or(|entry| entry.profile != profile || !entry.available_in(space))
                    {
                        return rejected(OperationReason::InvalidScope);
                    }
                }
                let effects = self.items.set_container(tab, container);
                if !effects.is_empty() || self.container_recreates.contains_key(&tab) {
                    self.schedule_container_recreate(tab, container);
                }
                if effects.is_empty()
                    && self
                        .items
                        .tab(tab)
                        .is_some_and(|state| state.container == container)
                {
                    self.schedule_persist();
                    self.project_items();
                    operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
                } else {
                    mutation_result(self.commit(effects))
                }
            }
        }
    }

    fn valid_container_spaces(&self, profile: ProfileId, access: &SpaceAccess) -> bool {
        match access {
            SpaceAccess::All => true,
            SpaceAccess::Selected(spaces) => {
                spaces.len() <= zephium_core::session::MAX_SESSION_SPACES
                    && spaces
                        .iter()
                        .all(|id| self.spaces.get(*id).is_some_and(|s| s.profile == profile))
            }
        }
    }

    fn canonical_access(&self, access: SpaceAccess) -> SpaceAccess {
        match access {
            SpaceAccess::All => SpaceAccess::All,
            SpaceAccess::Selected(mut spaces) => {
                spaces.sort_unstable();
                spaces.dedup();
                SpaceAccess::Selected(spaces)
            }
        }
    }

    fn persist_containers(&self) -> bool {
        self.containers.setting().is_some_and(|value| {
            self.store
                .set_app_setting(zephium_core::containers::SETTINGS_KEY.to_owned(), value)
        })
    }

    fn containers_changed(
        &mut self,
        previous: zephium_core::containers::Containers,
    ) -> OperationDisposition {
        if !self.persist_containers() {
            self.containers = previous;
            return rejected(OperationReason::StoreAdmissionRejected);
        }
        self.schedule_persist();
        self.project_items();
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }
}
