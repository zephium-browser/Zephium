//! Essentials used to be one set per profile and now belong to a space. A
//! store that still has the old shape puts each profile's set in one of its
//! spaces (the home), and `spread` gives every other space of the profile a
//! copy, so nothing a person kept disappears in the change.

use std::collections::HashMap;

use zephium_core::ids::{ItemId, ProfileId, SpaceId};
use zephium_core::item::Placement;
use zephium_core::session::{space_placements, PersistedItem, PersistedSpace, MAX_SESSION_ITEMS};

/// The space of `profile` that keeps the original items: the one the person
/// was in, else the profile's first. The original ids stay with it, so a kept
/// site that was in front stays in front.
pub(crate) fn home_space(
    spaces: &[PersistedSpace],
    profile: ProfileId,
    active: Option<SpaceId>,
) -> Option<SpaceId> {
    active
        .filter(|active| {
            spaces
                .iter()
                .any(|space| space.id == *active && space.profile == profile)
        })
        .or_else(|| {
            spaces
                .iter()
                .find(|space| space.profile == profile)
                .map(|space| space.id)
        })
}

/// Copies each profile's Essentials from its home space into the profile's
/// other spaces, under fresh ids and in the same order. The result lists
/// every space's containers in snapshot order.
///
/// A profile whose copies would push the session past its item limit gets
/// none: the limit would otherwise drop the tabs that come last, and a
/// person's open tabs matter more than a second copy of a kept site.
pub(crate) fn spread(
    items: Vec<PersistedItem>,
    spaces: &[PersistedSpace],
    homes: &HashMap<ProfileId, SpaceId>,
) -> Vec<PersistedItem> {
    let mut room = MAX_SESSION_ITEMS.saturating_sub(items.len());
    let mut by_placement: HashMap<Placement, Vec<PersistedItem>> = HashMap::new();
    let mut unplaced = Vec::new();
    let known: std::collections::HashSet<SpaceId> = spaces.iter().map(|space| space.id).collect();
    for item in items {
        let (Placement::Favorites { space } | Placement::Space { space, .. }) = item.placement;
        if known.contains(&space) {
            by_placement.entry(item.placement).or_default().push(item);
        } else {
            unplaced.push(item);
        }
    }

    let mut homes: Vec<(&ProfileId, &SpaceId)> = homes.iter().collect();
    homes.sort_by_key(|(profile, _)| **profile);
    for (profile, home) in homes {
        let Some(original) = by_placement
            .get(&Placement::Favorites { space: *home })
            .filter(|original| !original.is_empty())
            .cloned()
        else {
            continue;
        };
        let siblings: Vec<SpaceId> = spaces
            .iter()
            .filter(|space| space.profile == *profile && space.id != *home)
            .map(|space| space.id)
            .collect();
        if original.len().saturating_mul(siblings.len()) > room {
            continue;
        }
        room -= original.len() * siblings.len();
        for sibling in siblings {
            let mut renamed: HashMap<ItemId, ItemId> = HashMap::new();
            let copies = original
                .iter()
                .map(|item| {
                    let id = ItemId::generate();
                    renamed.insert(item.id, id);
                    PersistedItem {
                        id,
                        // A parent that is not in the set stays as it was and
                        // so stays dangling, for restore to drop.
                        parent: item
                            .parent
                            .map(|parent| renamed.get(&parent).copied().unwrap_or(parent)),
                        placement: Placement::Favorites { space: sibling },
                        kind: item.kind.clone(),
                    }
                })
                .collect();
            by_placement.insert(Placement::Favorites { space: sibling }, copies);
        }
    }

    let mut out = Vec::new();
    for space in spaces {
        for placement in space_placements(space.id) {
            out.extend(by_placement.remove(&placement).unwrap_or_default());
        }
    }
    out.extend(unplaced);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::item::SpaceSection;
    use zephium_core::session::PersistedKind;

    fn space(id: u128, profile: u128) -> PersistedSpace {
        PersistedSpace {
            id: SpaceId::from(id),
            profile: ProfileId::from(profile),
            name: "Space".into(),
            look: Default::default(),
        }
    }

    fn tab(id: u128, parent: Option<u128>, placement: Placement, url: &str) -> PersistedItem {
        PersistedItem {
            id: ItemId::from(id),
            parent: parent.map(ItemId::from),
            placement,
            kind: PersistedKind::Tab {
                url: url.into(),
                title: url.into(),
                zoom: 1.0,
                last_active_ms: None,
            },
        }
    }

    fn essentials(space: u128) -> Placement {
        Placement::Favorites {
            space: SpaceId::from(space),
        }
    }

    fn today(space: u128) -> Placement {
        Placement::Space {
            space: SpaceId::from(space),
            section: SpaceSection::Today,
        }
    }

    fn urls(items: &[PersistedItem], placement: Placement) -> Vec<String> {
        items
            .iter()
            .filter(|item| item.placement == placement)
            .filter_map(|item| match &item.kind {
                PersistedKind::Tab { url, .. } => Some(url.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_home_is_the_space_the_person_was_in_else_the_profiles_first() {
        let spaces = [space(1, 10), space(2, 10), space(3, 11)];
        let profile = ProfileId::from(10);
        assert_eq!(
            home_space(&spaces, profile, Some(SpaceId::from(2))),
            Some(SpaceId::from(2))
        );
        assert_eq!(
            home_space(&spaces, profile, Some(SpaceId::from(3))),
            Some(SpaceId::from(1)),
            "another profile's space is not a home"
        );
        assert_eq!(home_space(&spaces, profile, None), Some(SpaceId::from(1)));
        assert_eq!(home_space(&spaces, ProfileId::from(12), None), None);
    }

    #[test]
    fn every_sibling_space_gets_a_copy_in_order_and_the_originals_keep_their_ids() {
        let spaces = [space(1, 10), space(2, 10), space(3, 11)];
        let items = vec![
            tab(21, None, essentials(1), "https://one.example/"),
            tab(22, None, essentials(1), "https://two.example/"),
            tab(23, None, today(1), "https://today.example/"),
            tab(24, None, essentials(3), "https://other-profile.example/"),
        ];
        let homes = HashMap::from([(ProfileId::from(10), SpaceId::from(1))]);

        let spread = spread(items.clone(), &spaces, &homes);

        assert_eq!(
            urls(&spread, essentials(2)),
            ["https://one.example/", "https://two.example/"]
        );
        assert_eq!(
            urls(&spread, essentials(1)),
            ["https://one.example/", "https://two.example/"]
        );
        assert_eq!(
            urls(&spread, essentials(3)),
            ["https://other-profile.example/"],
            "a profile that was not spread is left alone"
        );
        let originals: Vec<ItemId> = spread
            .iter()
            .filter(|item| item.placement == essentials(1))
            .map(|item| item.id)
            .collect();
        assert_eq!(originals, [ItemId::from(21), ItemId::from(22)]);
        let mut ids: Vec<ItemId> = spread.iter().map(|item| item.id).collect();
        let count = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), count, "no id is owned twice");
        assert_eq!(count, items.len() + 2);
    }

    #[test]
    fn items_come_out_in_snapshot_order_whatever_order_they_went_in() {
        let spaces = [space(1, 10), space(2, 10)];
        let items = vec![
            tab(31, None, today(2), "https://b-today.example/"),
            tab(32, None, today(1), "https://a-today.example/"),
            tab(33, None, essentials(1), "https://kept.example/"),
        ];
        let homes = HashMap::from([(ProfileId::from(10), SpaceId::from(1))]);

        let placements: Vec<Placement> = spread(items, &spaces, &homes)
            .into_iter()
            .map(|item| item.placement)
            .collect();

        assert_eq!(
            placements,
            [essentials(1), today(1), essentials(2), today(2),]
        );
    }

    #[test]
    fn copied_folders_keep_their_children() {
        let spaces = [space(1, 10), space(2, 10)];
        let folder = PersistedItem {
            id: ItemId::from(41),
            parent: None,
            placement: essentials(1),
            kind: PersistedKind::Folder { name: "F".into() },
        };
        let items = vec![
            folder,
            tab(42, Some(41), essentials(1), "https://child.example/"),
        ];
        let homes = HashMap::from([(ProfileId::from(10), SpaceId::from(1))]);

        let spread = spread(items, &spaces, &homes);

        let copies: Vec<&PersistedItem> = spread
            .iter()
            .filter(|item| item.placement == essentials(2))
            .collect();
        assert_eq!(copies.len(), 2);
        assert_eq!(copies[1].parent, Some(copies[0].id));
        assert_ne!(copies[0].id, ItemId::from(41));
    }

    #[test]
    fn copies_never_cost_a_person_their_tabs() {
        let spaces = [space(1, 10), space(2, 10)];
        let mut items = vec![tab(1, None, essentials(1), "https://kept.example/")];
        items.extend((0..MAX_SESSION_ITEMS - 1).map(|index| {
            tab(
                100 + index as u128,
                None,
                today(2),
                &format!("https://open.example/{index}"),
            )
        }));
        assert_eq!(items.len(), MAX_SESSION_ITEMS);
        let homes = HashMap::from([(ProfileId::from(10), SpaceId::from(1))]);

        let spread = spread(items, &spaces, &homes);

        assert_eq!(spread.len(), MAX_SESSION_ITEMS);
        assert!(urls(&spread, essentials(2)).is_empty());
    }
}
