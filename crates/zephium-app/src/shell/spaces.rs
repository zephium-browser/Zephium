//! The focused profile's spaces: making, editing, ordering, deleting and
//! switching them, and moving a tab from one to another. A space owns its
//! Essentials, pinned tabs and open tabs alike.

use super::*;
use crate::SpaceCommand;
use std::collections::HashMap;
use zephium_core::spaces::{SpaceIcon, SpaceLook, SpaceTheme};
use zephium_ipc::{SpaceIconView, SpaceThemeView};

/// How many tabs of a space the switcher stacks icons for.
pub(super) const SPACE_PREVIEWS: usize = 3;

#[derive(Default)]
pub(super) struct SpaceMemory {
    /// The tab each space had in front when the window last left it.
    left_at: HashMap<SpaceId, ItemId>,
    /// The space each profile last showed.
    last: HashMap<ProfileId, SpaceId>,
}

fn icon_view(icon: &SpaceIcon) -> SpaceIconView {
    match icon {
        SpaceIcon::Symbol(index) => SpaceIconView::Symbol { index: *index },
        SpaceIcon::Emoji(text) => SpaceIconView::Emoji { text: text.clone() },
    }
}

fn theme_view(theme: &SpaceTheme) -> SpaceThemeView {
    match theme {
        SpaceTheme::Preset(id) => SpaceThemeView::Preset { id: id.clone() },
        SpaceTheme::Custom(seeds) => SpaceThemeView::Custom {
            seeds: seeds.clone(),
        },
    }
}

fn rejected(reason: OperationReason) -> OperationDisposition {
    operation_result(OperationOutcome::Rejected, reason)
}

fn unchanged() -> OperationDisposition {
    operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged)
}

impl Shell {
    pub(super) fn space_view(&self, space: &Space) -> SpaceView {
        SpaceView {
            id: space.id.to_string(),
            name: space.name.clone(),
            icon: space.look.icon.as_ref().map(icon_view),
            color: space.look.color,
            theme: space.look.theme.as_ref().map(theme_view),
            tabs: self.space_tab_count(space.id),
            previews: self.space_previews(space),
        }
    }

    /// The first tabs of a space in sidebar order, pinned then open, with the
    /// icon cached for each.
    pub(super) fn space_previews(&self, space: &Space) -> Vec<Option<zephium_ipc::IconRef>> {
        self.space_first_tabs(space.id)
            .into_iter()
            .map(|id| {
                self.items.tab(id).and_then(|tab| {
                    self.icon_ref(zephium_ipc::IconSurface::Chrome, tab, Some(space.profile))
                })
            })
            .collect()
    }

    pub(super) fn space_first_tabs(&self, space: SpaceId) -> Vec<ItemId> {
        let mut roots = self.space_roots(space);
        roots.reverse();
        let mut found = Vec::new();
        while let Some(id) = roots.pop() {
            if self.items.tab(id).is_some() {
                found.push(id);
                if found.len() == SPACE_PREVIEWS {
                    break;
                }
            } else {
                roots.extend(self.items.children(id).iter().rev().copied());
            }
        }
        found
    }

    fn space_roots(&self, space: SpaceId) -> Vec<ItemId> {
        [SpaceSection::Pinned, SpaceSection::Today]
            .into_iter()
            .flat_map(|section| {
                self.items
                    .roots(Placement::Space { space, section })
                    .iter()
                    .copied()
            })
            .collect()
    }

    fn space_tab_count(&self, space: SpaceId) -> u32 {
        let mut pending = self.space_roots(space);
        let mut count = 0_u32;
        while let Some(id) = pending.pop() {
            if self.items.tab(id).is_some() {
                count += 1;
            } else {
                pending.extend_from_slice(self.items.children(id));
            }
        }
        count
    }

    /// The space a profile returns to: the one it last showed, else its first.
    pub(super) fn remembered_space(&self, profile: ProfileId) -> Option<SpaceId> {
        self.space_memory
            .last
            .get(&profile)
            .copied()
            .filter(|space| {
                self.spaces
                    .get(*space)
                    .is_some_and(|candidate| candidate.profile == profile)
            })
            .or_else(|| self.spaces.first_for(profile))
    }

    /// Records where the focused window stands before it shows another scope.
    pub(super) fn remember_space_left(&mut self) {
        let Some(window) = self.windows.focused() else {
            return;
        };
        let (profile, space, active) = (window.profile, window.space, window.active);
        self.space_memory.last.insert(profile, space);
        match active {
            Some(id) => self.space_memory.left_at.insert(space, id),
            None => self.space_memory.left_at.remove(&space),
        };
    }

    pub(super) fn forget_space_memory(&mut self, profile: ProfileId) {
        self.space_memory.last.remove(&profile);
        let spaces = &self.spaces;
        self.space_memory
            .left_at
            .retain(|space, _| spaces.get(*space).is_some());
    }

    /// The focused window's persistent profile and space. Private browsing
    /// keeps a single space, and a profile being deleted has none to change.
    fn space_scope(&self) -> Option<(ProfileId, SpaceId)> {
        let window = self.windows.focused()?;
        let persistent = self
            .profiles
            .get(window.profile)
            .is_some_and(|profile| profile.kind != ProfileKind::Incognito);
        (persistent && !self.profile_deletion_quarantines(window.profile))
            .then_some((window.profile, window.space))
    }

    fn owned_space(&self, profile: ProfileId, space: SpaceId) -> bool {
        self.spaces
            .get(space)
            .is_some_and(|candidate| candidate.profile == profile)
    }

    pub(super) fn operation_space(&mut self, command: SpaceCommand) -> OperationDisposition {
        if !self.bootstrapped {
            return rejected(OperationReason::NoFocusedWindow);
        }
        let Some((profile, current)) = self.space_scope() else {
            return rejected(OperationReason::InvalidScope);
        };
        match command {
            SpaceCommand::Create { name, look } => self.create_space(profile, &name, look),
            SpaceCommand::Update { id, name, look } => {
                if !self.owned_space(profile, id) {
                    return rejected(OperationReason::InvalidScope);
                }
                if !look.valid() || zephium_core::spaces::space_name(&name).is_none() {
                    return rejected(OperationReason::InvalidInput);
                }
                if !self.spaces.update(id, &name, look) {
                    return unchanged();
                }
                self.spaces_changed()
            }
            SpaceCommand::Reorder { id, before } => {
                if !self.owned_space(profile, id) {
                    return rejected(OperationReason::InvalidScope);
                }
                if !self.spaces.reorder(id, before) {
                    return unchanged();
                }
                self.spaces_changed()
            }
            SpaceCommand::Delete(id) => self.delete_space(profile, current, id),
            SpaceCommand::Switch(id) => {
                if !self.owned_space(profile, id) {
                    return rejected(OperationReason::InvalidScope);
                }
                self.switch_space(id)
            }
            SpaceCommand::Step(step) => {
                let order = self.spaces.of(profile);
                let at = order.iter().position(|space| *space == current);
                let next = at
                    .and_then(|at| at.checked_add_signed(isize::from(step.signum())))
                    .and_then(|at| order.get(at).copied());
                match next {
                    Some(next) if step != 0 => self.switch_space(next),
                    _ => unchanged(),
                }
            }
            SpaceCommand::MoveTab { tab, space } => self.move_tab_to_space(profile, tab, space),
            SpaceCommand::MoveActive(space) => {
                match self.windows.focused().and_then(|window| window.active) {
                    Some(tab) => self.move_tab_to_space(profile, tab, space),
                    None => unchanged(),
                }
            }
        }
    }

    /// The ⌃1…⌃9 keys: by position among the profile's spaces.
    pub(super) fn operation_select_space(&mut self, position: usize) -> OperationDisposition {
        let Some((profile, _)) = self.space_scope() else {
            return unchanged();
        };
        match position
            .checked_sub(1)
            .and_then(|index| self.spaces.of(profile).get(index).copied())
        {
            Some(space) => self.operation_space(SpaceCommand::Switch(space)),
            None => unchanged(),
        }
    }

    fn spaces_changed(&mut self) -> OperationDisposition {
        self.schedule_persist();
        self.project_items();
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    fn create_space(
        &mut self,
        profile: ProfileId,
        name: &str,
        look: SpaceLook,
    ) -> OperationDisposition {
        if !look.valid() {
            return rejected(OperationReason::InvalidInput);
        }
        let count = self.spaces.of(profile).len();
        if count >= zephium_core::spaces::MAX_PROFILE_SPACES {
            return rejected(OperationReason::ItemLimitReached);
        }
        let name = zephium_core::spaces::space_name(name)
            .unwrap_or_else(|| format!("Space {}", count + 1));
        let Some(space) = (0..8)
            .map(|_| SpaceId::generate())
            .find(|id| self.spaces.create(*id, profile, &name, look.clone()))
        else {
            return rejected(OperationReason::ItemLimitReached);
        };
        self.switch_space(space)
    }

    /// Shows `space` in the focused window. Work stays on its canvas, which
    /// follows the space, and any other browser page steps back to browsing first.
    fn switch_space(&mut self, space: SpaceId) -> OperationDisposition {
        match self.active_browser_page() {
            Some(crate::BrowserPage::Work) => {}
            Some(_) if !self.browser_return_ready => {
                self.browser_after_return = Some(Box::new(Command::Space(Box::new(
                    SpaceCommand::Switch(space),
                ))));
                return self.operation_show_browser_page(None);
            }
            _ => {}
        }
        if self.windows.focused().is_some_and(|w| w.space == space) {
            self.schedule_persist();
            self.project_items();
            return unchanged();
        }
        let (effects, blank) = self.show_space(space);
        let native = self.commit(effects);
        if let Some(blank) = blank {
            self.close_in_any_space(blank);
        }
        mutation_result(native)
    }

    /// `show_space` arriving on `tab`, not on what the space last had in front.
    pub(super) fn show_space_at(
        &mut self,
        space: SpaceId,
        tab: ItemId,
    ) -> (Vec<Effect>, Option<ItemId>) {
        self.space_memory.left_at.insert(space, tab);
        self.show_space(space)
    }

    /// Also returns the untouched new tab left behind, for the caller to
    /// close once the window shows the new space.
    fn show_space(&mut self, space: SpaceId) -> (Vec<Effect>, Option<ItemId>) {
        let Some((profile, leaving, active)) = self
            .windows
            .focused()
            .map(|window| (window.profile, window.space, window.active))
        else {
            return (Vec::new(), None);
        };
        if self.work_pane.is_some() {
            let _ = self.operation_work_pane_hide();
        }
        self.remember_space_left();
        // A new tab left untouched is not worth keeping in the space left.
        let blank = active.filter(|id| {
            self.items.get(*id).is_some_and(
                |item| matches!(item.placement, Placement::Space { space, .. } if space == leaving),
            ) && self.is_untouched_new_tab(*id)
        });
        if let Some(window) = self.windows.focused_mut() {
            window.space = space;
            window.splits = None;
        }
        self.space_memory.last.insert(profile, space);
        let returning = self
            .space_memory
            .left_at
            .get(&space)
            .copied()
            .filter(|id| Some(*id) != blank && self.item_in_scope(*id, profile, space))
            .or_else(|| {
                self.space_roots(space)
                    .into_iter()
                    .flat_map(|root| {
                        let mut pending = vec![root];
                        let mut found = None;
                        while let Some(id) = pending.pop() {
                            if self.items.tab(id).is_some() {
                                found = Some(id);
                                break;
                            }
                            pending.extend(self.items.children(id).iter().rev().copied());
                        }
                        found
                    })
                    .next()
            });
        let effects = match returning {
            Some(id) => self.focus_tab(id),
            None => self.open_tab(),
        };
        let shown = returning.is_some()
            || self.windows.focused().and_then(|window| window.active) != active;
        if !shown {
            if let Some(window) = self.windows.focused_mut() {
                window.active = None;
            }
            if let Some(id) = active {
                self.items.set_lifecycle(id, Lifecycle::Inactive);
            }
        }
        if blank.is_some() {
            self.space_memory.left_at.remove(&leaving);
        }
        self.hydrate_favicon_cache(profile, space);
        (effects, blank)
    }

    fn delete_space(
        &mut self,
        profile: ProfileId,
        current: SpaceId,
        id: SpaceId,
    ) -> OperationDisposition {
        if !self.owned_space(profile, id) {
            return rejected(OperationReason::InvalidScope);
        }
        let order = self.spaces.of(profile);
        if order.len() < 2 {
            return rejected(OperationReason::InvalidInput);
        }
        let mut native = NativeWork::default();
        if id == current {
            if matches!(self.active_browser_page(), Some(crate::BrowserPage::Work)) {
                return rejected(OperationReason::InvalidScope);
            }
            let at = order.iter().position(|space| *space == id).unwrap_or(0);
            let neighbour = order
                .get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|before| order.get(before)))
                .copied()
                .expect("a second space exists");
            let (effects, blank) = self.show_space(neighbour);
            native.merge(self.commit(effects));
            if let Some(blank) = blank {
                self.close_in_any_space(blank);
            }
        }
        let essentials = self
            .items
            .roots(Placement::Favorites { space: id })
            .to_vec();
        for root in self.space_roots(id).into_iter().chain(essentials) {
            self.close_in_any_space(root);
        }
        self.recently_closed.retain(|entry| entry.space != id);
        self.project_recently_closed_if_changed();
        self.spaces.remove(id);
        self.space_memory.left_at.remove(&id);
        self.space_memory.last.retain(|_, space| *space != id);
        self.schedule_persist();
        self.project_items();
        mutation_result(native)
    }

    fn move_tab_to_space(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        target: SpaceId,
    ) -> OperationDisposition {
        if !self.owned_space(profile, target) || !self.item_in_focused_scope(tab) {
            return rejected(OperationReason::InvalidScope);
        }
        let Some(placement) = self.items.get(tab).map(|i| i.placement) else {
            return rejected(OperationReason::InvalidScope);
        };
        // An Essential stays an Essential, now of the other space.
        let (space, destination) = match placement {
            Placement::Space { space, section } => (
                space,
                Placement::Space {
                    space: target,
                    section,
                },
            ),
            Placement::Favorites { space } => (space, Placement::Favorites { space: target }),
        };
        if space == target {
            return unchanged();
        }
        let at = self
            .successor_pool(profile, space)
            .iter()
            .position(|id| *id == tab);
        if !self.items.move_tab_to_root(tab, destination, None) {
            return rejected(OperationReason::InvalidInput);
        }
        let was_active = self.windows.focused().and_then(|w| w.active) == Some(tab);
        if let Some(window) = self.windows.focused_mut() {
            if let Some(tree) = window.splits.take() {
                window.splits = tree.remove(tab);
            }
            if was_active {
                window.active = None;
            }
        }
        if self.work_pane.as_ref().is_some_and(|pane| pane.tab == tab) {
            self.clear_work_pane();
        }
        let mut effects = Vec::new();
        if was_active {
            self.items.set_lifecycle(tab, Lifecycle::Inactive);
            self.space_memory.left_at.insert(target, tab);
            effects.extend(self.take_over(profile, space, at));
        }
        mutation_result(self.commit(effects))
    }
}
