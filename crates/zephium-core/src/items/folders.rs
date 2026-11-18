//! Folders in the item tree. Every caller gets the same invariants: a folder
//! never lands in Essentials or inside itself, nesting stays bounded, and a
//! subtree shares one placement.

use super::*;
use crate::item::FolderColor;

/// How many folders may stand inside one another. A restored tree deeper
/// than this keeps its shape; only new nesting is refused.
pub const MAX_FOLDER_DEPTH: usize = 3;

/// A person's folder name, bounded like other session names and trimmed.
pub(crate) fn folder_name(name: &str) -> String {
    crate::session::bounded_name(name.trim(), "Folder")
        .trim()
        .to_owned()
}

impl Items {
    pub fn is_folder(&self, id: ItemId) -> bool {
        self.items
            .get(&id)
            .is_some_and(|item| matches!(item.kind, ItemKind::Folder { .. }))
    }

    /// Folders from the section root down to `id`, `id` included when it is
    /// one.
    pub fn folder_depth(&self, id: ItemId) -> usize {
        let mut depth = 0;
        let mut at = Some(id);
        for _ in 0..=crate::session::MAX_ITEM_TREE_DEPTH {
            let Some(item) = at.and_then(|id| self.items.get(&id)) else {
                break;
            };
            if matches!(item.kind, ItemKind::Folder { .. }) {
                depth += 1;
            }
            at = item.parent;
        }
        depth
    }

    /// Folder levels in `id`'s subtree, `id` included.
    fn folder_height(&self, id: ItemId) -> usize {
        if !self.is_folder(id) {
            return 0;
        }
        1 + self
            .children(id)
            .iter()
            .map(|child| self.folder_height(*child))
            .max()
            .unwrap_or(0)
    }

    /// Whether `id` is `ancestor` or stands somewhere inside it.
    pub fn is_within(&self, id: ItemId, ancestor: ItemId) -> bool {
        let mut at = Some(id);
        for _ in 0..=crate::session::MAX_ITEM_TREE_DEPTH {
            let Some(current) = at else {
                return false;
            };
            if current == ancestor {
                return true;
            }
            at = self.items.get(&current).and_then(|item| item.parent);
        }
        false
    }

    /// The tabs in `id`'s subtree in sidebar order, `id` itself when a tab.
    pub fn subtree_tabs(&self, id: ItemId) -> Vec<ItemId> {
        let mut ordered = Vec::new();
        let mut pending = vec![id];
        while let Some(next) = pending.pop() {
            if self.tab(next).is_some() {
                ordered.push(next);
            } else {
                pending.extend(self.children(next).iter().rev().copied());
            }
        }
        ordered
    }

    /// Whether `before` may anchor an insertion into this container.
    fn anchors(
        &self,
        before: Option<ItemId>,
        parent: Option<ItemId>,
        placement: Placement,
    ) -> bool {
        before.is_none_or(|before| {
            self.items
                .get(&before)
                .is_some_and(|item| item.parent == parent && item.placement == placement)
        })
    }

    fn container_mut(&mut self, parent: Option<ItemId>, placement: Placement) -> &mut Vec<ItemId> {
        match parent {
            Some(parent) => self.children.entry(parent).or_default(),
            None => self.roots.entry(placement).or_default(),
        }
    }

    fn detach(&mut self, id: ItemId, parent: Option<ItemId>, placement: Placement) {
        let siblings = match parent {
            Some(parent) => self.children.get_mut(&parent),
            None => self.roots.get_mut(&placement),
        };
        if let Some(siblings) = siblings {
            siblings.retain(|item| *item != id);
        }
    }

    fn attach(
        &mut self,
        id: ItemId,
        parent: Option<ItemId>,
        placement: Placement,
        before: Option<ItemId>,
    ) {
        let target = self.container_mut(parent, placement);
        let index = before
            .and_then(|before| target.iter().position(|item| *item == before))
            .unwrap_or(target.len());
        target.insert(index, id);
    }

    /// Inserts an open folder before `before` in its container, or at the
    /// end. Never in Essentials, and never past [`MAX_FOLDER_DEPTH`].
    pub fn insert_folder(
        &mut self,
        id: ItemId,
        placement: Placement,
        parent: Option<ItemId>,
        before: Option<ItemId>,
        name: &str,
        color: Option<FolderColor>,
    ) -> bool {
        if matches!(placement, Placement::Favorites { .. })
            || parent.is_some_and(|parent| {
                !self.is_folder(parent) || self.folder_depth(parent) >= MAX_FOLDER_DEPTH
            })
            || !self.anchors(before, parent, placement)
        {
            return false;
        }
        let inserted = self.insert(Item {
            id,
            parent,
            placement,
            kind: ItemKind::Folder {
                name: folder_name(name),
                color,
                collapsed: false,
            },
        });
        if inserted && before.is_some() {
            self.detach(id, parent, placement);
            self.attach(id, parent, placement, before);
        }
        inserted
    }

    /// Moves a tab or a folder, with everything in it, into `parent` (a
    /// folder, or the section root when `None`) before `before`, or at the
    /// end. Native identities are untouched; the caller authorizes scope.
    pub fn move_item(
        &mut self,
        id: ItemId,
        placement: Placement,
        parent: Option<ItemId>,
        before: Option<ItemId>,
    ) -> bool {
        let Some(item) = self.items.get(&id) else {
            return false;
        };
        if let Some(parent) = parent {
            let Some(folder) = self.items.get(&parent) else {
                return false;
            };
            if !matches!(folder.kind, ItemKind::Folder { .. })
                || folder.placement != placement
                || self.is_within(parent, id)
            {
                return false;
            }
        }
        match &item.kind {
            ItemKind::Folder { .. } => {
                if matches!(placement, Placement::Favorites { .. }) {
                    return false;
                }
                let base = parent.map_or(0, |parent| self.folder_depth(parent));
                if parent != item.parent && base + self.folder_height(id) > MAX_FOLDER_DEPTH {
                    return false;
                }
            }
            ItemKind::Tab(tab) => match tab.content {
                TabContent::Web => {}
                // Pages the browser owns stay loose in their own space: a
                // snapshot never restores one inside a folder.
                TabContent::BrowserOwned(_) => {
                    if parent.is_some()
                        || !matches!(
                            (item.placement, placement),
                            (
                                Placement::Space { space: current, .. },
                                Placement::Space { space: destination, .. }
                            ) if current == destination
                        )
                    {
                        return false;
                    }
                }
                TabContent::ExtensionOwned => {
                    if parent.is_some() || item.placement != placement {
                        return false;
                    }
                }
            },
        }
        if before == Some(id) {
            return item.parent == parent && item.placement == placement;
        }
        if !self.anchors(before, parent, placement) {
            return false;
        }
        let (old_parent, old_placement) = (item.parent, item.placement);
        self.detach(id, old_parent, old_placement);
        self.attach(id, parent, placement, before);
        if let Some(item) = self.items.get_mut(&id) {
            item.parent = parent;
            item.placement = placement;
        }
        if old_placement != placement {
            let mut pending = self.children(id).to_vec();
            while let Some(next) = pending.pop() {
                pending.extend(self.children(next).iter().copied());
                if let Some(item) = self.items.get_mut(&next) {
                    item.placement = placement;
                }
            }
        }
        true
    }

    fn folder_mut(
        &mut self,
        id: ItemId,
    ) -> Option<(&mut String, &mut Option<FolderColor>, &mut bool)> {
        match &mut self.items.get_mut(&id)?.kind {
            ItemKind::Folder {
                name,
                color,
                collapsed,
            } => Some((name, color, collapsed)),
            ItemKind::Tab(_) => None,
        }
    }

    /// `None` when `id` is not a folder; otherwise whether the name changed.
    pub fn rename_folder(&mut self, id: ItemId, name: &str) -> Option<bool> {
        let name = folder_name(name);
        let (current, _, _) = self.folder_mut(id)?;
        let changed = *current != name;
        *current = name;
        Some(changed)
    }

    pub fn set_folder_color(&mut self, id: ItemId, color: Option<FolderColor>) -> Option<bool> {
        let (_, current, _) = self.folder_mut(id)?;
        let changed = *current != color;
        *current = color;
        Some(changed)
    }

    pub fn set_folder_collapsed(&mut self, id: ItemId, collapsed: bool) -> Option<bool> {
        let (_, _, current) = self.folder_mut(id)?;
        let changed = *current != collapsed;
        *current = collapsed;
        Some(changed)
    }

    /// Takes a folder away and leaves what it held in its place, in order.
    pub fn ungroup(&mut self, id: ItemId) -> bool {
        let Some(item) = self
            .items
            .get(&id)
            .filter(|item| matches!(item.kind, ItemKind::Folder { .. }))
        else {
            return false;
        };
        let (parent, placement) = (item.parent, item.placement);
        let held = self.children.remove(&id).unwrap_or_default();
        for child in &held {
            if let Some(child) = self.items.get_mut(child) {
                child.parent = parent;
            }
        }
        let container = self.container_mut(parent, placement);
        let at = container
            .iter()
            .position(|item| *item == id)
            .unwrap_or(container.len());
        container.splice(at..(at + 1).min(container.len()), held);
        self.items.remove(&id);
        true
    }

    /// A tab that knows its page but has not loaded it, put in a folder or
    /// section before `before`: how a kept tab stays where it was closed.
    pub fn insert_unloaded_tab_at(
        &mut self,
        id: ItemId,
        placement: Placement,
        parent: Option<ItemId>,
        before: Option<ItemId>,
        url: Url,
        title: &str,
    ) -> bool {
        if !navigation::is_allowed(&url) || !self.anchors(before, parent, placement) {
            return false;
        }
        let mut tab = TabState::new();
        tab.url = Some(url);
        tab.title = sanitize_page_title(title);
        if !self.insert(Item {
            id,
            parent,
            placement,
            kind: ItemKind::Tab(tab),
        }) {
            return false;
        }
        if before.is_some() {
            self.detach(id, parent, placement);
            self.attach(id, parent, placement, before);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SpaceId;
    use crate::item::SpaceSection;

    fn id(n: u128) -> ItemId {
        ItemId::from(n)
    }

    fn today() -> Placement {
        Placement::Space {
            space: SpaceId::from(1),
            section: SpaceSection::Today,
        }
    }

    fn tab(items: &mut Items, n: u128) -> ItemId {
        assert!(items.insert_tab(id(n), today()));
        id(n)
    }

    fn folder(items: &mut Items, n: u128, parent: Option<ItemId>) -> ItemId {
        assert!(items.insert_folder(id(n), today(), parent, None, "F", None));
        id(n)
    }

    #[test]
    fn a_folder_is_placed_before_its_anchor_and_its_name_is_bounded() {
        let mut items = Items::default();
        let a = tab(&mut items, 1);
        let b = tab(&mut items, 2);
        assert!(items.insert_folder(id(3), today(), None, Some(b), "  Research  ", None));
        assert_eq!(items.roots(today()), [a, id(3), b]);
        let Some(ItemKind::Folder {
            name, collapsed, ..
        }) = items.get(id(3)).map(|i| &i.kind)
        else {
            panic!("folder");
        };
        assert_eq!(name, "Research");
        assert!(!collapsed);

        assert!(items.insert_folder(id(4), today(), None, None, "\u{202e}\u{0}", None));
        assert!(matches!(
            &items.get(id(4)).unwrap().kind,
            ItemKind::Folder { name, .. } if name == "Folder"
        ));
        let long = "x".repeat(10_000);
        assert!(items.insert_folder(id(5), today(), None, None, &long, None));
        assert!(matches!(
            &items.get(id(5)).unwrap().kind,
            ItemKind::Folder { name, .. }
                if name.chars().count() == crate::session::MAX_SESSION_NAME_CHARS
        ));
    }

    #[test]
    fn folders_refuse_essentials_foreign_anchors_tabs_as_parents_and_deep_nesting() {
        let mut items = Items::default();
        let space = SpaceId::from(1);
        let loose = tab(&mut items, 1);
        assert!(!items.insert_folder(id(2), Placement::Favorites { space }, None, None, "F", None));
        assert!(!items.insert_folder(id(2), today(), Some(loose), None, "F", None));
        let outer = folder(&mut items, 3, None);
        // An anchor must be a sibling in the target container.
        assert!(!items.insert_folder(id(2), today(), Some(outer), Some(loose), "F", None));
        let middle = folder(&mut items, 4, Some(outer));
        let inner = folder(&mut items, 5, Some(middle));
        assert_eq!(items.folder_depth(inner), MAX_FOLDER_DEPTH);
        assert!(!items.insert_folder(id(6), today(), Some(inner), None, "F", None));
        // Tabs still go anywhere inside.
        assert!(items.move_item(loose, today(), Some(inner), None));
        assert_eq!(items.children(inner), [loose]);
    }

    #[test]
    fn a_folder_never_moves_into_itself_or_past_the_depth_limit() {
        let mut items = Items::default();
        let outer = folder(&mut items, 1, None);
        let inner = folder(&mut items, 2, Some(outer));
        let other = folder(&mut items, 3, None);
        assert!(!items.move_item(outer, today(), Some(outer), None));
        assert!(!items.move_item(outer, today(), Some(inner), None));
        // outer holds two levels; inside a nested folder it would make four.
        let nested = folder(&mut items, 4, Some(other));
        assert!(!items.move_item(outer, today(), Some(nested), None));
        assert!(items.move_item(outer, today(), Some(other), None));
        assert_eq!(items.children(other), [nested, outer]);
        assert_eq!(items.folder_depth(inner), 3);
        let space = SpaceId::from(1);
        assert!(!items.move_item(other, Placement::Favorites { space }, None, None));
    }

    #[test]
    fn moving_reorders_reparents_and_carries_the_subtree_between_sections() {
        let mut items = Items::default();
        let a = tab(&mut items, 1);
        let f = folder(&mut items, 2, None);
        let b = tab(&mut items, 3);
        assert!(items.move_item(b, today(), Some(f), None));
        assert!(items.move_item(a, today(), Some(f), Some(b)));
        assert_eq!(items.children(f), [a, b]);
        assert_eq!(items.roots(today()), [f]);
        assert_eq!(items.get(a).unwrap().parent, Some(f));
        // Before itself where it already is: nothing to do, and true.
        assert!(items.move_item(a, today(), Some(f), Some(a)));
        assert!(!items.move_item(a, today(), None, Some(a)));
        // A stale anchor in another container is refused, not guessed.
        assert!(!items.move_item(a, today(), None, Some(b)));

        let pinned = Placement::Space {
            space: SpaceId::from(1),
            section: SpaceSection::Pinned,
        };
        assert!(items.move_item(f, pinned, None, None));
        assert!(items.roots(today()).is_empty());
        for child in [a, b] {
            assert_eq!(items.get(child).unwrap().placement, pinned);
        }
        assert_eq!(items.subtree_tabs(f), [a, b]);
    }

    #[test]
    fn only_web_pages_go_into_folders() {
        let mut items = Items::default();
        let f = folder(&mut items, 1, None);
        assert!(items.insert_browser_tab(id(2), today(), BrowserOwnedTab::Extensions));
        assert!(items.insert_extension_tab(id(3), today()));
        assert!(!items.move_item(id(2), today(), Some(f), None));
        assert!(!items.move_item(id(3), today(), Some(f), None));
        assert!(items.children(f).is_empty());
        // Browser pages still reorder among the open tabs.
        assert!(items.move_item(id(2), today(), None, Some(f)));
        assert_eq!(items.roots(today()), [id(2), f, id(3)]);
    }

    #[test]
    fn ungrouping_leaves_the_contents_in_the_folders_place() {
        let mut items = Items::default();
        let a = tab(&mut items, 1);
        let f = folder(&mut items, 2, None);
        let z = tab(&mut items, 3);
        let inner = folder(&mut items, 4, Some(f));
        let b = tab(&mut items, 5);
        let c = tab(&mut items, 6);
        assert!(items.move_item(b, today(), Some(f), Some(inner)));
        assert!(items.move_item(c, today(), Some(inner), None));
        assert!(items.ungroup(f));
        assert_eq!(items.roots(today()), [a, b, inner, z]);
        assert_eq!(items.get(inner).unwrap().parent, None);
        assert_eq!(items.children(inner), [c]);
        assert!(items.get(f).is_none());
        assert!(!items.ungroup(a));
        assert!(!items.ungroup(f));
    }

    #[test]
    fn folder_settings_report_change_and_refuse_tabs() {
        let mut items = Items::default();
        let f = folder(&mut items, 1, None);
        let t = tab(&mut items, 2);
        assert_eq!(items.rename_folder(f, "Trips"), Some(true));
        assert_eq!(items.rename_folder(f, " Trips "), Some(false));
        assert_eq!(
            items.set_folder_color(f, Some(FolderColor::Mint)),
            Some(true)
        );
        assert_eq!(
            items.set_folder_color(f, Some(FolderColor::Mint)),
            Some(false)
        );
        assert_eq!(items.set_folder_collapsed(f, true), Some(true));
        assert_eq!(items.rename_folder(t, "x"), None);
        assert_eq!(items.set_folder_collapsed(t, true), None);
        assert_eq!(items.set_folder_color(id(99), None), None);
    }

    #[test]
    fn a_kept_tab_returns_unloaded_to_its_slot() {
        let mut items = Items::default();
        let f = folder(&mut items, 1, None);
        let a = tab(&mut items, 2);
        let b = tab(&mut items, 3);
        assert!(items.move_item(a, today(), Some(f), None));
        assert!(items.move_item(b, today(), Some(f), None));
        let url = Url::parse("https://example.com/").unwrap();
        items.remove(a);
        assert!(items.insert_unloaded_tab_at(id(4), today(), Some(f), Some(b), url.clone(), "E"));
        assert_eq!(items.children(f), [id(4), b]);
        assert!(!items.tab(id(4)).unwrap().has_view());
        let unsafe_url = Url::parse("javascript:alert(1)").unwrap();
        assert!(!items.insert_unloaded_tab_at(id(5), today(), Some(f), None, unsafe_url, "x"));
        assert!(!items.insert_unloaded_tab_at(id(5), today(), None, Some(b), url, "x"));
    }
}
