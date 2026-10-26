//! Tabs in the order they were last activated. Each live tab appears at most
//! once and a closed tab never remains: `Shell::note_activation` is the only
//! push, and `Shell::remove_item` / `Shell::remove_profile_items` prune after
//! every removal.

use zephium_core::ids::ItemId;
use zephium_core::items::Items;

#[derive(Clone, Default)]
pub(super) struct RecentTabs {
    // Index 0 is the least recently pushed; the last entry is the top.
    stack: Vec<ItemId>,
}

impl RecentTabs {
    /// Puts `id` on top, moving it there if it is already present.
    pub(super) fn push(&mut self, id: ItemId) {
        self.stack.retain(|entry| *entry != id);
        self.stack.push(id);
    }

    /// Drops entries whose tab no longer exists in `items`. Removal is
    /// recursive and profile removal takes many tabs at once, so pruning
    /// compares against what is left rather than a single id.
    pub(super) fn retain_live(&mut self, items: &Items) {
        self.stack.retain(|id| items.tab(*id).is_some());
    }

    pub(super) fn most_recent_first(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.stack.iter().rev().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::ids::SpaceId;
    use zephium_core::item::{Item, ItemKind, Placement, SpaceSection};

    fn today() -> Placement {
        Placement::Space {
            space: SpaceId::from(1),
            section: SpaceSection::Today,
        }
    }

    fn order(recent: &RecentTabs) -> Vec<ItemId> {
        recent.most_recent_first().collect()
    }

    #[test]
    fn push_moves_existing_entry_to_top() {
        let [a, b, c] = [1, 2, 3].map(ItemId::from);
        let mut recent = RecentTabs::default();
        for id in [a, b, c, b] {
            recent.push(id);
        }
        assert_eq!(order(&recent), vec![b, c, a]);
    }

    #[test]
    fn push_never_duplicates() {
        let a = ItemId::from(1);
        let mut recent = RecentTabs::default();
        recent.push(a);
        recent.push(a);
        assert_eq!(order(&recent), vec![a]);
    }

    #[test]
    fn retain_live_drops_removed_tabs_and_their_descendants() {
        let [folder, child, other] = [10, 11, 12].map(ItemId::from);
        let mut items = Items::default();
        assert!(items.insert_tab(child, today()));
        let tab = items.tab(child).cloned().unwrap();
        let _ = items.remove(child);
        assert!(items.insert(Item {
            id: folder,
            parent: None,
            placement: today(),
            kind: ItemKind::Folder { name: "F".into() },
        }));
        assert!(items.insert(Item {
            id: child,
            parent: Some(folder),
            placement: today(),
            kind: ItemKind::Tab(tab),
        }));
        assert!(items.insert_tab(other, today()));
        let mut recent = RecentTabs::default();
        recent.push(child);
        recent.push(other);

        let _ = items.remove(folder);
        recent.retain_live(&items);

        assert_eq!(order(&recent), vec![other]);
    }

    #[test]
    fn most_recent_first_is_top_to_bottom() {
        let [a, b, c] = [1, 2, 3].map(ItemId::from);
        let mut recent = RecentTabs::default();
        for id in [a, b, c] {
            recent.push(id);
        }
        assert_eq!(order(&recent), vec![c, b, a]);
    }
}
