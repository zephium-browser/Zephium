//! Profile, space, and item authorization scope helpers.

use super::*;

impl Shell {
    /// Extensions may address Browse tabs throughout their profile, but never
    /// the tab currently borrowed or owned by the Work pane. Agent contexts
    /// have separate identities and are never members of `items`.
    pub(super) fn extension_tab_in_scope(&self, id: ItemId, profile: ProfileId) -> bool {
        !self.profile_deletion_quarantines(profile)
            && self.profile_of_item(id) == Some(profile)
            && self.items.tab(id).is_some()
            && self.work_pane.as_ref().is_none_or(|pane| pane.tab != id)
    }

    pub(super) fn profile_of_item(&self, id: ItemId) -> Option<ProfileId> {
        let (Placement::Favorites { space } | Placement::Space { space, .. }) =
            self.items.get(id)?.placement;
        self.spaces.get(space).map(|s| s.profile)
    }

    /// A space is an authorization boundary for UI-directed item IDs: its
    /// Essentials and its tabs both require an exact match.
    pub(super) fn item_in_scope(&self, id: ItemId, profile: ProfileId, space: SpaceId) -> bool {
        if self.profile_deletion_quarantines(profile) {
            return false;
        }
        if self
            .spaces
            .get(space)
            .is_none_or(|candidate| candidate.profile != profile)
        {
            return false;
        }
        let Some(item) = self.items.get(id).filter(|item| item.tab().is_some()) else {
            return false;
        };
        let (Placement::Favorites { space: item_space }
        | Placement::Space {
            space: item_space, ..
        }) = item.placement;
        item_space == space
    }

    pub(super) fn item_in_focused_scope(&self, id: ItemId) -> bool {
        self.windows
            .focused()
            .is_some_and(|win| self.item_in_scope(id, win.profile, win.space))
    }

    pub(super) fn pane_in_scope(&self, tree: &Pane, profile: ProfileId, space: SpaceId) -> bool {
        let tabs = tree.tabs();
        tabs.len() <= MAX_VISIBLE_PANES
            && tabs.into_iter().all(|id| {
                self.item_in_scope(id, profile, space)
                    && self
                        .items
                        .tab(id)
                        .is_some_and(|tab| tab.content == zephium_core::item::TabContent::Web)
            })
    }

    pub(super) fn partition_of(&self, id: ItemId) -> Partition {
        let profile = self
            .profile_of_item(id)
            .or_else(|| self.windows.focused().map(|w| w.profile))
            .unwrap_or_else(|| {
                self.profiles
                    .default_profile()
                    .unwrap_or(ProfileId::from(0))
            });
        match self.profiles.get(profile).map(|p| p.kind) {
            Some(ProfileKind::Named) => Partition::Persistent(profile),
            Some(ProfileKind::Incognito) => Partition::Ephemeral(profile),
            Some(ProfileKind::Default) | None => Partition::Default(profile),
        }
    }

    pub(super) fn tab_metadata(
        &self,
        profile: ProfileId,
        ids: &[ItemId],
    ) -> Vec<crate::TabMetadata> {
        ids.iter()
            .filter(|id| self.profile_of_item(**id) == Some(profile))
            .filter_map(|id| {
                let tab = self.items.tab(*id)?;
                Some(crate::TabMetadata {
                    id: *id,
                    title: tab.title.clone(),
                    url: tab.url.as_ref().map(|url| url.to_string()),
                })
            })
            .collect()
    }

    /// The open tabs of the focused window, when it shows `profile`.
    pub(super) fn window_tabs(&self, profile: ProfileId) -> Vec<crate::TabMetadata> {
        let Some(window) = self
            .windows
            .focused()
            .filter(|window| window.profile == profile)
        else {
            return Vec::new();
        };
        let ids = self.today_tabs(window.space);
        self.tab_metadata(profile, &ids)
    }

    /// Tabs in the order the sidebar reads top to bottom: Essentials, pinned
    /// (folders opened in place), then today. Keyboard selection and cycling
    /// walk this one list so a pinned tab is never a dead end.
    pub(super) fn keyboard_tabs(&self, space: SpaceId) -> Vec<ItemId> {
        let mut ordered = self.placement_tabs(Placement::Favorites { space });
        ordered.extend(self.placement_tabs(Placement::Space {
            space,
            section: SpaceSection::Pinned,
        }));
        ordered.extend(self.regular_tabs(space));
        ordered
    }

    /// The tabs of a space's Today section in sidebar order, folders opened
    /// in place: the ones a closing tab may hand the window to.
    pub(super) fn regular_tabs(&self, space: SpaceId) -> Vec<ItemId> {
        self.placement_tabs(Placement::Space {
            space,
            section: SpaceSection::Today,
        })
    }

    fn placement_tabs(&self, placement: Placement) -> Vec<ItemId> {
        let mut ordered = Vec::new();
        let mut pending: Vec<ItemId> = self.items.roots(placement).iter().rev().copied().collect();
        while let Some(id) = pending.pop() {
            if self.items.tab(id).is_some() {
                ordered.push(id);
            } else {
                pending.extend(self.items.children(id).iter().rev().copied());
            }
        }
        ordered
    }

    pub(super) fn today_tabs(&self, space: SpaceId) -> Vec<ItemId> {
        self.items
            .roots(Placement::Space {
                space,
                section: SpaceSection::Today,
            })
            .iter()
            .copied()
            .filter(|id| self.items.tab(*id).is_some())
            .collect()
    }
}
