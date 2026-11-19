//! Folders in a space's open tabs: making, naming, colouring, folding,
//! moving, ungrouping and deleting them. A folder keeps its tabs: closing one
//! ends its page and leaves it in place (see `tabs::kept_slot`). A deleted
//! folder comes back whole from Reopen Closed Tab while the app runs.

use super::*;
use crate::FolderCommand;
use std::collections::HashMap;
use zephium_core::ids::ClosedSessionId;
use zephium_core::item::{FolderColor, ItemKind};
use zephium_core::items::MAX_FOLDER_DEPTH;

/// Deleted folders Reopen Closed Tab can still bring back.
const MAX_DELETED_FOLDERS: usize = 8;

pub(super) struct DeletedFolder {
    profile: ProfileId,
    parent: Option<ItemId>,
    before: Option<ItemId>,
    node: DeletedNode,
    /// The Recently Closed records its tabs left.
    closed: Vec<ClosedSessionId>,
    /// Every record Recently Closed held once it was deleted; any other is
    /// newer and comes back first.
    known: Vec<ClosedSessionId>,
}

enum DeletedNode {
    Folder {
        name: String,
        color: Option<FolderColor>,
        collapsed: bool,
        children: Vec<DeletedNode>,
    },
    Tab {
        url: url::Url,
        title: String,
        source: ItemId,
        /// Its Recently Closed record; it comes back only while that is there.
        record: Option<ClosedSessionId>,
    },
}

impl DeletedNode {
    fn link(&mut self, records: &HashMap<ItemId, ClosedSessionId>) {
        match self {
            Self::Folder { children, .. } => {
                children.iter_mut().for_each(|child| child.link(records));
                // A tab Recently Closed does not hold can never come back.
                children.retain(|child| !matches!(child, Self::Tab { record: None, .. }));
            }
            Self::Tab { source, record, .. } => *record = records.get(source).copied(),
        }
    }

    #[cfg(test)]
    fn tabs(&self) -> usize {
        match self {
            Self::Folder { children, .. } => children.iter().map(Self::tabs).sum(),
            Self::Tab { .. } => 1,
        }
    }

    /// Folder levels it holds, itself counted.
    fn height(&self) -> usize {
        match self {
            Self::Folder { children, .. } => {
                1 + children.iter().map(Self::height).max().unwrap_or(0)
            }
            Self::Tab { .. } => 0,
        }
    }

    /// Items a restore would make: every folder, and each tab still recorded.
    fn size(&self, live: &[ClosedSessionId]) -> usize {
        match self {
            Self::Folder { children, .. } => {
                1 + children.iter().map(|c| c.size(live)).sum::<usize>()
            }
            Self::Tab { record, .. } => usize::from(record.is_some_and(|r| live.contains(&r))),
        }
    }
}

fn rejected(reason: OperationReason) -> OperationDisposition {
    operation_result(OperationOutcome::Rejected, reason)
}

fn unchanged() -> OperationDisposition {
    operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged)
}

fn today(space: SpaceId) -> Placement {
    Placement::Space {
        space,
        section: SpaceSection::Today,
    }
}

impl Shell {
    pub(super) fn operation_folder(&mut self, command: FolderCommand) -> OperationDisposition {
        if !self.bootstrapped {
            return rejected(OperationReason::NoFocusedWindow);
        }
        // Private browsing keeps nothing, so it has nothing to file.
        let Some((profile, space)) = self.space_scope() else {
            return rejected(OperationReason::InvalidScope);
        };
        match command {
            FolderCommand::Create {
                name,
                color,
                tabs,
                parent,
                before,
            } => self.create_folder(space, &name, color, &tabs, parent, before),
            FolderCommand::Rename { id, name } => {
                self.change_folder(space, id, |items| items.rename_folder(id, &name))
            }
            FolderCommand::SetColor { id, color } => {
                self.change_folder(space, id, |items| items.set_folder_color(id, color))
            }
            FolderCommand::SetCollapsed { id, collapsed } => {
                self.change_folder(space, id, |items| items.set_folder_collapsed(id, collapsed))
            }
            FolderCommand::Move { id, parent, before } => self.move_into(space, id, parent, before),
            FolderCommand::RemoveFromFolder(id) => self.remove_from_folder(space, id),
            FolderCommand::Ungroup(id) => {
                if !self.folder_in_space(id, space) || !self.items.ungroup(id) {
                    return rejected(OperationReason::InvalidScope);
                }
                mutation_result(self.commit(Vec::new()))
            }
            FolderCommand::CloseTabs(id) => self.close_folder_tabs(space, id),
            FolderCommand::Delete(id) => self.delete_folder(profile, space, id),
            FolderCommand::MoveToSpace { id, space: target } => {
                self.move_folder_to_space(profile, space, id, target)
            }
        }
    }

    fn folder_in_space(&self, id: ItemId, space: SpaceId) -> bool {
        self.items.get(id).is_some_and(|item| {
            matches!(item.kind, ItemKind::Folder { .. })
                && matches!(item.placement, Placement::Space { space: at, .. } if at == space)
        })
    }

    /// A tab or folder of the focused space, by the same rule as every
    /// other item the chrome names.
    fn node_in_space(&self, id: ItemId, space: SpaceId) -> bool {
        self.folder_in_space(id, space) || self.item_in_focused_scope(id)
    }

    fn change_folder(
        &mut self,
        space: SpaceId,
        id: ItemId,
        change: impl FnOnce(&mut Items) -> Option<bool>,
    ) -> OperationDisposition {
        if !self.folder_in_space(id, space) {
            return rejected(OperationReason::InvalidScope);
        }
        match change(&mut self.items) {
            None => rejected(OperationReason::InvalidInput),
            Some(false) => unchanged(),
            Some(true) => mutation_result(self.commit(Vec::new())),
        }
    }

    fn create_folder(
        &mut self,
        space: SpaceId,
        name: &str,
        color: Option<FolderColor>,
        tabs: &[ItemId],
        parent: Option<ItemId>,
        before: Option<ItemId>,
    ) -> OperationDisposition {
        if tabs.len() > zephium_core::session::MAX_SESSION_ITEMS {
            return rejected(OperationReason::InvalidInput);
        }
        if tabs.iter().any(|tab| {
            !self.item_in_focused_scope(*tab)
                || self
                    .items
                    .tab(*tab)
                    .is_none_or(|tab| tab.content != zephium_core::item::TabContent::Web)
        }) {
            return rejected(OperationReason::InvalidScope);
        }
        // The tabs go in as the sidebar lists them, whatever order they came.
        let order = self.keyboard_tabs(space);
        let mut tabs = tabs.to_vec();
        tabs.sort_by_cached_key(|tab| order.iter().position(|id| id == tab));
        tabs.dedup();
        let placement = today(space);
        let (mut parent, mut before) = match tabs.iter().find(|tab| {
            self.items
                .get(**tab)
                .is_some_and(|item| item.placement == placement)
        }) {
            // A folder made of tabs stands where the first of them stood.
            Some(first) => (
                self.items.get(*first).and_then(|item| item.parent),
                Some(*first),
            ),
            None if !tabs.is_empty() => (None, None),
            None => {
                if parent.is_some_and(|parent| {
                    !self.folder_in_space(parent, space)
                        || self.items.get(parent).map(|item| item.placement) != Some(placement)
                }) || before.is_some_and(|before| !self.node_in_space(before, space))
                {
                    return rejected(OperationReason::InvalidScope);
                }
                (parent, before)
            }
        };
        // Too deep to nest further: it stands beside the deepest folder.
        while let Some(folder) =
            parent.filter(|folder| self.items.folder_depth(*folder) >= MAX_FOLDER_DEPTH)
        {
            before = Some(folder);
            parent = self.items.get(folder).and_then(|item| item.parent);
        }
        let Some(id) = (0..8).map(|_| ItemId::generate()).find(|id| {
            self.items
                .insert_folder(*id, placement, parent, before, name, color)
        }) else {
            return rejected(OperationReason::InvalidInput);
        };
        for tab in tabs {
            let _ = self.items.move_item(tab, placement, Some(id), None);
        }
        mutation_result(self.commit(Vec::new()))
    }

    fn move_into(
        &mut self,
        space: SpaceId,
        id: ItemId,
        parent: Option<ItemId>,
        before: Option<ItemId>,
    ) -> OperationDisposition {
        if !self.node_in_space(id, space)
            || parent.is_some_and(|parent| !self.folder_in_space(parent, space))
            || before.is_some_and(|before| !self.node_in_space(before, space))
        {
            return rejected(OperationReason::InvalidScope);
        }
        // Into the section its new neighbours are in: the open tabs, or the
        // pinned section an imported folder may still stand in.
        let placement = parent
            .or(before)
            .and_then(|anchor| self.items.get(anchor))
            .map(|item| item.placement)
            .filter(|placement| matches!(placement, Placement::Space { .. }))
            .unwrap_or_else(|| today(space));
        let Some(item) = self.items.get(id) else {
            return rejected(OperationReason::InvalidScope);
        };
        if item.parent == parent && item.placement == placement {
            let siblings = match parent {
                Some(parent) => self.items.children(parent),
                None => self.items.roots(placement),
            };
            let at = siblings.iter().position(|sibling| *sibling == id);
            let next = at.and_then(|at| siblings.get(at + 1).copied());
            if before == Some(id) || (at.is_some() && next == before) {
                return unchanged();
            }
        }
        if !self.items.move_item(id, placement, parent, before) {
            return rejected(OperationReason::InvalidInput);
        }
        mutation_result(self.commit(Vec::new()))
    }

    fn remove_from_folder(&mut self, space: SpaceId, id: ItemId) -> OperationDisposition {
        let Some(folder) = self
            .items
            .get(id)
            .and_then(|item| item.parent)
            .filter(|folder| self.folder_in_space(*folder, space))
        else {
            return rejected(OperationReason::InvalidScope);
        };
        let Some(holder) = self.items.get(folder) else {
            return rejected(OperationReason::InvalidScope);
        };
        let (parent, placement) = (holder.parent, holder.placement);
        let siblings = match parent {
            Some(parent) => self.items.children(parent),
            None => self.items.roots(placement),
        };
        let before = siblings
            .iter()
            .position(|sibling| *sibling == folder)
            .and_then(|at| siblings.get(at + 1).copied());
        if !self.item_in_focused_scope(id) || !self.items.move_item(id, placement, parent, before) {
            return rejected(OperationReason::InvalidInput);
        }
        mutation_result(self.commit(Vec::new()))
    }

    fn close_folder_tabs(&mut self, space: SpaceId, id: ItemId) -> OperationDisposition {
        if !self.folder_in_space(id, space) {
            return rejected(OperationReason::InvalidScope);
        }
        let awake: Vec<ItemId> = self
            .items
            .subtree_tabs(id)
            .into_iter()
            .filter(|tab| self.items.tab(*tab).is_some_and(TabState::has_view))
            .collect();
        if awake.is_empty() {
            return unchanged();
        }
        // The window leaves the folder first, so ending the one in front
        // never wakes the rest in turn.
        let mut native = NativeWork::default();
        if let Some(active) = self.windows.focused().and_then(|window| window.active) {
            if awake.contains(&active) {
                native.merge(self.close(active));
            }
        }
        for tab in awake {
            if self.items.tab(tab).is_some_and(TabState::has_view) {
                native.merge(self.close(tab));
            }
        }
        mutation_result(native)
    }

    fn capture(&self, id: ItemId) -> Option<DeletedNode> {
        let item = self.items.get(id)?;
        match &item.kind {
            ItemKind::Folder {
                name,
                color,
                collapsed,
            } => Some(DeletedNode::Folder {
                name: name.clone(),
                color: *color,
                collapsed: *collapsed,
                children: self
                    .items
                    .children(id)
                    .iter()
                    .filter_map(|child| self.capture(*child))
                    .collect(),
            }),
            ItemKind::Tab(tab) => (tab.content == zephium_core::item::TabContent::Web)
                .then(|| tab.url.clone())
                .flatten()
                .map(|url| DeletedNode::Tab {
                    url,
                    title: tab.title.clone(),
                    source: id,
                    record: None,
                }),
        }
    }

    fn delete_folder(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        id: ItemId,
    ) -> OperationDisposition {
        if !self.folder_in_space(id, space) {
            return rejected(OperationReason::InvalidScope);
        }
        let Some(item) = self.items.get(id) else {
            return rejected(OperationReason::InvalidScope);
        };
        let (parent, placement) = (item.parent, item.placement);
        let siblings = match parent {
            Some(parent) => self.items.children(parent),
            None => self.items.roots(placement),
        };
        let before = siblings
            .iter()
            .position(|sibling| *sibling == id)
            .and_then(|at| siblings.get(at + 1).copied());
        let node = self.capture(id);
        let mut records = HashMap::new();
        let mut native = NativeWork::default();
        for tab in self.items.subtree_tabs(id) {
            let newest = |shell: &Self| shell.recently_closed.last().and_then(|e| e.session_id);
            let previous = newest(self);
            native.merge(self.close_tab(tab, false));
            if self.items.get(tab).is_some() {
                // The lone New Tab of a space stays open, where the folder stood.
                let _ = self.items.move_item(tab, placement, parent, Some(id));
                continue;
            }
            if let Some(record) = newest(self).filter(|record| Some(*record) != previous) {
                records.insert(tab, record);
            }
        }
        let effects = self.items.remove(id);
        native.merge(self.commit(effects));
        if let Some(mut node) = node {
            // A large folder can push its own first records out again.
            records.retain(|_, record| {
                self.recently_closed
                    .iter()
                    .any(|entry| entry.session_id == Some(*record))
            });
            node.link(&records);
            if self.deleted_folders.len() == MAX_DELETED_FOLDERS {
                self.deleted_folders.remove(0);
            }
            self.deleted_folders.push(DeletedFolder {
                profile,
                parent,
                before,
                node,
                closed: records.into_values().collect(),
                known: self
                    .recently_closed
                    .iter()
                    .filter_map(|entry| entry.session_id)
                    .collect(),
            });
        }
        mutation_result(native)
    }

    /// The addresses every deleted folder still holds.
    #[cfg(test)]
    pub(super) fn held_deleted_tabs(&self) -> usize {
        self.deleted_folders.iter().map(|f| f.node.tabs()).sum()
    }

    /// Brings back the newest deleted folder of `profile` when nothing closed
    /// after it is still in Recently Closed. Its tabs return asleep, except
    /// those already reopened on their own; a session without room for all
    /// of it keeps the deletion for later.
    pub(super) fn reopen_deleted_folder(&mut self, profile: ProfileId) -> Option<NativeWork> {
        let (current, persistent) = self.windows.focused().map(|window| {
            (
                window.space,
                window.profile == profile && !self.incognito_profile(profile),
            )
        })?;
        if !persistent {
            return None;
        }
        let at = self
            .deleted_folders
            .iter()
            .rposition(|folder| folder.profile == profile)?;
        let folder = &self.deleted_folders[at];
        if let Some(entry) = self
            .recently_closed
            .iter()
            .rev()
            .find(|entry| entry.profile == profile)
        {
            // Closed after it: the tab comes first. A record with no id was
            // written by an older build or without a clock, before any
            // deletion this run could know of, so the folder goes before it.
            if entry
                .session_id
                .is_some_and(|record| !folder.known.contains(&record))
            {
                return None;
            }
        }
        let live: Vec<ClosedSessionId> = self
            .recently_closed
            .iter()
            .filter_map(|entry| entry.session_id)
            .filter(|record| folder.closed.contains(record))
            .collect();
        if folder.node.size(&live) > self.items.room() {
            return None;
        }
        let deleted = self.deleted_folders.remove(at);
        // Where it was, when that is still the space in front; else here.
        let space = current;
        let placement = today(space);
        // The nearest folder it came from that still has room for all of it.
        let height = deleted.node.height();
        let mut parent = deleted
            .parent
            .filter(|parent| self.folder_in_space(*parent, space));
        while let Some(at) =
            parent.filter(|at| self.items.folder_depth(*at) + height > MAX_FOLDER_DEPTH)
        {
            parent = self.items.get(at).and_then(|item| item.parent);
        }
        let before = deleted.before.filter(|before| {
            self.items
                .get(*before)
                .is_some_and(|item| item.parent == parent && item.placement == placement)
        });
        let mut restored = Vec::new();
        if !self.restore_node(
            &deleted.node,
            &live,
            &mut restored,
            placement,
            parent,
            before,
        ) {
            return None;
        }
        // A tab that could not come back keeps its record for a plain reopen.
        self.recently_closed
            .retain(|entry| entry.session_id.is_none_or(|id| !restored.contains(&id)));
        self.extension_sessions_changed(profile);
        Some(self.commit(Vec::new()))
    }

    fn restore_node(
        &mut self,
        node: &DeletedNode,
        live: &[ClosedSessionId],
        restored: &mut Vec<ClosedSessionId>,
        placement: Placement,
        parent: Option<ItemId>,
        before: Option<ItemId>,
    ) -> bool {
        match node {
            DeletedNode::Tab { record, .. } if !record.is_some_and(|r| live.contains(&r)) => true,
            DeletedNode::Tab {
                url, title, record, ..
            } => {
                let back = (0..8).any(|_| {
                    self.items.insert_unloaded_tab_at(
                        ItemId::generate(),
                        placement,
                        parent,
                        before,
                        url.clone(),
                        title,
                    )
                });
                if back {
                    restored.extend(*record);
                }
                back
            }
            DeletedNode::Folder {
                name,
                color,
                collapsed,
                children,
            } => {
                let Some(id) = (0..8).map(|_| ItemId::generate()).find(|id| {
                    self.items
                        .insert_folder(*id, placement, parent, before, name, *color)
                }) else {
                    return false;
                };
                let _ = self.items.set_folder_collapsed(id, *collapsed);
                for child in children {
                    // A nested folder past the depth limit comes back loose.
                    if !self.restore_node(child, live, restored, placement, Some(id), None) {
                        let _ = self.restore_node(child, live, restored, placement, parent, before);
                    }
                }
                true
            }
        }
    }

    fn move_folder_to_space(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        id: ItemId,
        target: SpaceId,
    ) -> OperationDisposition {
        if !self.folder_in_space(id, space) || !self.owned_space(profile, target) {
            return rejected(OperationReason::InvalidScope);
        }
        if space == target {
            return unchanged();
        }
        let tabs = self.items.subtree_tabs(id);
        let active = self.windows.focused().and_then(|window| window.active);
        // Where the front tab stood among the tabs that stay behind.
        let at = active.and_then(|active| {
            let pool = self.successor_pool(profile, space);
            let position = pool.iter().position(|tab| *tab == active)?;
            Some(
                pool[..position]
                    .iter()
                    .filter(|tab| !tabs.contains(tab))
                    .count(),
            )
        });
        if !self.items.move_item(id, today(target), None, None) {
            return rejected(OperationReason::InvalidInput);
        }
        let was_active = active.filter(|active| tabs.contains(active));
        if let Some(window) = self.windows.focused_mut() {
            if let Some(tree) = window.splits.take() {
                window.splits = tabs.iter().try_fold(tree, |tree, tab| tree.remove(*tab));
            }
            if was_active.is_some() {
                window.active = None;
            }
        }
        if self
            .work_pane
            .as_ref()
            .is_some_and(|pane| tabs.contains(&pane.tab))
        {
            self.clear_work_pane();
        }
        let mut effects = Vec::new();
        if let Some(tab) = was_active {
            self.items.set_lifecycle(tab, Lifecycle::Inactive);
            self.remember_left_tab(target, tab);
            effects.extend(self.take_over(profile, space, at));
        }
        mutation_result(self.commit(effects))
    }
}
