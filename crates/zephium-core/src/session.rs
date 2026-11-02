//! Persistable snapshot of profiles, spaces, the item tree, focus and splits.
//! Restore validates every reference and drops anything dangling: store data
//! is semi-trusted input.

use std::collections::HashSet;

use url::Url;

use crate::ids::{ClosedSessionId, ItemId, ProfileId, SpaceId};
use crate::item::{
    sanitize_page_title, BrowserOwnedTab, Item, ItemKind, Placement, TabContent, TabState,
};
use crate::items::Items;
use crate::navigation;
use crate::profiles::{Profile, ProfileKind, Profiles};
use crate::spaces::{Space, SpaceLook, Spaces};
use crate::split::Pane;

pub const MAX_SESSION_PROFILES: usize = 64;
pub const MAX_SESSION_SPACES: usize = 512;
pub const MAX_SESSION_ITEMS: usize = 1024;
pub const MAX_RECENTLY_CLOSED_TABS: usize = 32;
pub const MAX_SESSION_NAME_CHARS: usize = 256;
pub const MAX_ITEM_TREE_DEPTH: usize = 64;
pub const MAX_SPLIT_DEPTH: usize = 64;
/// Persisted wall-clock times stay within JavaScript's exact integer range.
pub const MAX_EXACT_MS: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionState {
    pub profiles: Vec<PersistedProfile>,
    pub spaces: Vec<PersistedSpace>,
    /// DFS order: parents precede children.
    pub items: Vec<PersistedItem>,
    pub active_space: Option<SpaceId>,
    pub active_item: Option<ItemId>,
    pub splits: Option<Pane>,
    #[serde(default)]
    pub recently_closed: Vec<PersistedClosedTab>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedProfile {
    pub id: ProfileId,
    pub name: String,
    pub kind: ProfileKind,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedSpace {
    pub id: SpaceId,
    pub profile: ProfileId,
    pub name: String,
    /// Absent on snapshots from before spaces had looks.
    #[serde(default, skip_serializing_if = "SpaceLook::is_default")]
    pub look: SpaceLook,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedItem {
    pub id: ItemId,
    pub parent: Option<ItemId>,
    pub placement: Placement,
    pub kind: PersistedKind,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PersistedKind {
    Folder {
        name: String,
    },
    Tab {
        url: String,
        title: String,
        zoom: f64,
        /// Absent on legacy snapshots and for tabs never in front.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_active_ms: Option<u64>,
    },
    /// Browser-owned pages have no navigable URL or native content view.
    BrowserTab {
        page: BrowserOwnedTab,
    },
}

/// Bounded browser-owned state for restoring a recently closed regular tab.
/// A fresh [`ItemId`] is allocated on restore, so stale native/item authority
/// can never be resurrected with the presentation record.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PersistedClosedTab {
    pub profile: ProfileId,
    pub space: SpaceId,
    pub url: String,
    pub title: String,
    pub zoom: f64,
    /// Absent on legacy snapshots. Never manufacture historical metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<ClosedSessionId>,
    /// Real Unix close time, bounded to JavaScript's exact integer range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at_ms: Option<u64>,
    /// Where the tab stood among its space's open tabs, for putting it back.
    /// Kept for this run only: a snapshot that names it could not be read
    /// back by the release before, so a restart restores to the end.
    #[serde(skip)]
    pub position: Option<u32>,
}

pub fn snapshot(
    profiles: &Profiles,
    spaces: &Spaces,
    items: &Items,
    active_space: Option<SpaceId>,
    active_item: Option<ItemId>,
    splits: Option<&Pane>,
) -> SessionState {
    snapshot_with_recently_closed(
        profiles,
        spaces,
        items,
        active_space,
        active_item,
        splits,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
pub fn snapshot_with_recently_closed(
    profiles: &Profiles,
    spaces: &Spaces,
    items: &Items,
    active_space: Option<SpaceId>,
    active_item: Option<ItemId>,
    splits: Option<&Pane>,
    recently_closed: &[PersistedClosedTab],
) -> SessionState {
    let mut state = SessionState::default();

    for profile in profiles.iter().filter(|p| p.kind != ProfileKind::Incognito) {
        state.profiles.push(PersistedProfile {
            id: profile.id,
            name: profile.name.clone(),
            kind: profile.kind,
        });
    }
    for space in spaces.iter() {
        if !state.profiles.iter().any(|p| p.id == space.profile) {
            continue;
        }
        state.spaces.push(PersistedSpace {
            id: space.id,
            profile: space.profile,
            name: space.name.clone(),
            look: space.look.clone(),
        });
        for placement in space_placements(space.id) {
            collect(items, placement, &mut state.items);
        }
    }

    let active_space = active_space.filter(|s| state.spaces.iter().any(|x| x.id == *s));
    let persisted = |id: ItemId| state.items.iter().any(|i| i.id == id);
    let in_active_scope = |id: ItemId| {
        active_space.is_some_and(|space| item_in_space_scope(items, spaces, id, space))
    };
    state.active_space = active_space;
    state.active_item = active_item.filter(|id| persisted(*id) && in_active_scope(*id));
    state.splits = splits
        .filter(|tree| {
            active_space.is_some_and(|space| {
                valid_split_tree(tree, items, spaces, space)
                    && tree.tabs().iter().all(|id| persisted(*id))
            })
        })
        .cloned();
    state.recently_closed =
        canonical_recently_closed(recently_closed, |entry| {
            state.profiles.iter().any(|profile| {
                profile.id == entry.profile && profile.kind != ProfileKind::Incognito
            }) && state
                .spaces
                .iter()
                .any(|space| space.id == entry.space && space.profile == entry.profile)
        });
    state
}

/// A space's containers in the order a snapshot lists them.
pub fn space_placements(space: SpaceId) -> [Placement; 3] {
    [
        Placement::Favorites { space },
        Placement::Space {
            space,
            section: crate::item::SpaceSection::Pinned,
        },
        Placement::Space {
            space,
            section: crate::item::SpaceSection::Today,
        },
    ]
}

fn collect(items: &Items, placement: Placement, out: &mut Vec<PersistedItem>) {
    fn walk(items: &Items, id: ItemId, out: &mut Vec<PersistedItem>) {
        let Some(item) = items.get(id) else {
            return;
        };
        let kind = match &item.kind {
            ItemKind::Folder { name } => PersistedKind::Folder { name: name.clone() },
            ItemKind::Tab(tab) => match tab.content {
                // The document a source tab shows lives in memory only.
                TabContent::BrowserOwned(BrowserOwnedTab::Source) => return,
                TabContent::BrowserOwned(page) => PersistedKind::BrowserTab { page },
                TabContent::ExtensionOwned => return,
                TabContent::Web => match &tab.url {
                    // A tab without a url (empty New Tab) is not worth persisting.
                    None => return,
                    Some(url) => PersistedKind::Tab {
                        url: url.to_string(),
                        title: tab.title.clone(),
                        zoom: tab.zoom,
                        last_active_ms: tab.last_active_ms,
                    },
                },
            },
        };
        out.push(PersistedItem {
            id: item.id,
            parent: item.parent,
            placement: item.placement,
            kind,
        });
        for child in items.children(id) {
            walk(items, *child, out);
        }
    }
    for id in items.roots(placement) {
        walk(items, *id, out);
    }
}

pub struct Restored {
    pub profiles: Profiles,
    pub spaces: Spaces,
    pub items: Items,
    pub active_space: Option<SpaceId>,
    pub active_item: Option<ItemId>,
    pub splits: Option<Pane>,
    pub recently_closed: Vec<PersistedClosedTab>,
}

/// Rebuilds a persistence snapshot through the same validation path used at
/// startup. Store adapters use this before writing so dangling/private rows,
/// dangerous URLs, and oversized legacy display data never land on disk.
pub fn canonicalize(state: SessionState) -> SessionState {
    let restored = restore(state);
    snapshot_with_recently_closed(
        &restored.profiles,
        &restored.spaces,
        &restored.items,
        restored.active_space,
        restored.active_item,
        restored.splits.as_ref(),
        &restored.recently_closed,
    )
}

pub fn restore(state: SessionState) -> Restored {
    let mut profiles = Profiles::default();
    for p in state.profiles.into_iter().take(MAX_SESSION_PROFILES) {
        if p.kind == ProfileKind::Incognito {
            continue;
        }
        profiles.insert(Profile {
            id: p.id,
            name: bounded_name(&p.name, "Profile"),
            kind: p.kind,
        });
    }

    let mut spaces = Spaces::default();
    for s in state.spaces.into_iter().take(MAX_SESSION_SPACES) {
        if profiles.get(s.profile).is_some() {
            spaces.insert(Space {
                id: s.id,
                profile: s.profile,
                name: bounded_name(&s.name, "Space"),
                look: s.look.sanitized(),
            });
        }
    }

    let mut items = Items::default();
    let mut browser_tabs = HashSet::new();
    for i in state.items.into_iter().take(MAX_SESSION_ITEMS) {
        let (Placement::Favorites { space } | Placement::Space { space, .. }) = i.placement;
        if spaces.get(space).is_none() {
            continue;
        }
        let kind = match i.kind {
            PersistedKind::Folder { name } => ItemKind::Folder {
                name: bounded_name(&name, "Folder"),
            },
            PersistedKind::Tab {
                url,
                title,
                zoom,
                last_active_ms,
            } => {
                let Ok(url) = Url::parse(&url) else {
                    continue;
                };
                if !navigation::is_browser_target(&url) {
                    continue;
                }
                let mut tab = TabState::new();
                tab.url = Some(url);
                tab.title = sanitize_page_title(&title);
                tab.zoom = if zoom.is_finite() {
                    zoom.clamp(0.3, 3.0)
                } else {
                    1.0
                };
                tab.last_active_ms =
                    last_active_ms.filter(|ms| (1_000..=MAX_EXACT_MS).contains(ms));
                ItemKind::Tab(tab)
            }
            PersistedKind::BrowserTab { page } => {
                let Placement::Space { space, .. } = i.placement else {
                    continue;
                };
                if page == BrowserOwnedTab::Source
                    || i.parent.is_some()
                    || !browser_tabs.insert((space, page))
                {
                    continue;
                }
                ItemKind::Tab(TabState::browser_owned(page))
            }
        };
        // insert() enforces parent existence and placement consistency; DFS
        // order in the snapshot guarantees parents come first.
        items.insert(Item {
            id: i.id,
            parent: i.parent,
            placement: i.placement,
            kind,
        });
    }

    let active_space = state.active_space.filter(|id| spaces.get(*id).is_some());
    let active_item = state.active_item.filter(|id| {
        active_space.is_some_and(|space| item_in_space_scope(&items, &spaces, *id, space))
    });
    let splits = state.splits.filter(|tree| {
        active_space.is_some_and(|space| valid_split_tree(tree, &items, &spaces, space))
    });
    let recently_closed = canonical_recently_closed(&state.recently_closed, |entry| {
        profiles
            .get(entry.profile)
            .is_some_and(|profile| profile.kind != ProfileKind::Incognito)
            && spaces
                .get(entry.space)
                .is_some_and(|space| space.profile == entry.profile)
    });

    Restored {
        profiles,
        spaces,
        items,
        active_space,
        active_item,
        splits,
        recently_closed,
    }
}

fn canonical_recently_closed(
    input: &[PersistedClosedTab],
    owns_entry: impl Fn(&PersistedClosedTab) -> bool,
) -> Vec<PersistedClosedTab> {
    let mut seen = HashSet::new();
    let mut output = input
        .iter()
        .rev()
        .filter_map(|entry| {
            if !owns_entry(entry) {
                return None;
            }
            if entry.session_id.is_some() != entry.closed_at_ms.is_some()
                || entry
                    .closed_at_ms
                    .is_some_and(|ms| !(1_000..=9_007_199_254_740_991).contains(&ms))
            {
                return None;
            }
            let url = Url::parse(&entry.url)
                .ok()
                .filter(navigation::is_browser_target)?;
            if entry.session_id.is_some_and(|id| !seen.insert(id)) {
                return None;
            }
            Some(PersistedClosedTab {
                profile: entry.profile,
                space: entry.space,
                url: url.to_string(),
                title: sanitize_page_title(&entry.title),
                zoom: if entry.zoom.is_finite() {
                    entry.zoom.clamp(0.3, 3.0)
                } else {
                    1.0
                },
                session_id: entry.session_id,
                closed_at_ms: entry.closed_at_ms,
                position: entry
                    .position
                    .filter(|position| (*position as usize) < MAX_SESSION_ITEMS),
            })
        })
        .take(MAX_RECENTLY_CLOSED_TABS)
        .collect::<Vec<_>>();
    output.reverse();
    output
}

fn item_in_space_scope(items: &Items, spaces: &Spaces, id: ItemId, space: SpaceId) -> bool {
    if spaces.get(space).is_none() {
        return false;
    }
    let Some(item) = items.get(id).filter(|item| item.tab().is_some()) else {
        return false;
    };
    let (Placement::Favorites { space: item_space }
    | Placement::Space {
        space: item_space, ..
    }) = item.placement;
    item_space == space
}

fn valid_split_tree(tree: &Pane, items: &Items, spaces: &Spaces, space: SpaceId) -> bool {
    fn walk(
        tree: &Pane,
        items: &Items,
        spaces: &Spaces,
        space: SpaceId,
        depth: usize,
        unique: &mut HashSet<ItemId>,
    ) -> bool {
        if depth > MAX_SPLIT_DEPTH || unique.len() >= MAX_SESSION_ITEMS {
            return false;
        }
        match tree {
            Pane::Leaf(id) => {
                item_in_space_scope(items, spaces, *id, space)
                    && items
                        .tab(*id)
                        .is_some_and(|tab| tab.content == TabContent::Web)
                    && unique.insert(*id)
            }
            Pane::Branch { ratio, a, b, .. } => {
                ratio.is_finite()
                    && (0.05..=0.95).contains(ratio)
                    && walk(a, items, spaces, space, depth + 1, unique)
                    && walk(b, items, spaces, space, depth + 1, unique)
            }
        }
    }

    walk(tree, items, spaces, space, 0, &mut HashSet::new())
}

pub(crate) fn bounded_name(value: &str, fallback: &str) -> String {
    let value: String = value
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    *c,
                    '\u{061c}'
                        | '\u{200e}'
                        | '\u{200f}'
                        | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
        .take(MAX_SESSION_NAME_CHARS)
        .collect();
    if value.is_empty() {
        fallback.into()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::SpaceSection;

    fn seed() -> (Profiles, Spaces, Items, ProfileId, SpaceId) {
        let profile = ProfileId::from(1);
        let space = SpaceId::from(2);
        let mut profiles = Profiles::default();
        profiles.insert(Profile {
            id: profile,
            name: "Personal".into(),
            kind: ProfileKind::Default,
        });
        let mut spaces = Spaces::default();
        spaces.insert(Space {
            id: space,
            profile,
            name: "Space".into(),
            look: Default::default(),
        });
        (profiles, spaces, Items::default(), profile, space)
    }

    fn today(space: SpaceId) -> Placement {
        Placement::Space {
            space,
            section: SpaceSection::Today,
        }
    }

    fn navigate_and_commit(items: &mut Items, id: ItemId, input: &str) {
        assert!(!items.navigate(id, input).is_empty());
        let url = navigation::classify(input).expect("test URL must be valid");
        items.set_committed_url(id, url);
    }

    #[test]
    fn snapshot_restore_roundtrips_ids_order_and_splits() {
        let (profiles, spaces, mut items, _profile, space) = seed();
        let (a, b) = (ItemId::from(10), ItemId::from(11));
        items.insert_tab(a, today(space));
        items.insert_tab(b, today(space));
        navigate_and_commit(&mut items, a, "example.com");
        navigate_and_commit(&mut items, b, "github.com");
        let mut tree = Pane::leaf(a);
        tree.split(a, b, crate::split::Axis::Row, false);

        let state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(space),
            Some(b),
            Some(&tree),
        );
        let restored = restore(state);

        assert_eq!(restored.items.roots(today(space)), &[a, b]);
        assert_eq!(restored.active_item, Some(b));
        assert_eq!(restored.active_space, Some(space));
        assert_eq!(restored.splits, Some(tree));
        assert!(!restored.items.tab(a).unwrap().has_view());
    }

    #[test]
    fn each_space_snapshots_and_restores_its_own_essentials_before_its_tabs() {
        let (profiles, mut spaces, mut items, profile, first) = seed();
        let second = SpaceId::from(3);
        spaces.insert(Space {
            id: second,
            profile,
            name: "Second".into(),
            look: Default::default(),
        });
        let (kept_first, open_first, kept_second) =
            (ItemId::from(10), ItemId::from(11), ItemId::from(12));
        for (id, placement, url) in [
            (open_first, today(first), "open.example"),
            (
                kept_second,
                Placement::Favorites { space: second },
                "board.example",
            ),
            (
                kept_first,
                Placement::Favorites { space: first },
                "mail.example",
            ),
        ] {
            assert!(items.insert_tab(id, placement));
            navigate_and_commit(&mut items, id, url);
        }

        let state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(first),
            Some(kept_first),
            None,
        );
        assert_eq!(
            state.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            [kept_first, open_first, kept_second],
            "a space lists its Essentials before its tabs"
        );
        assert_eq!(state.active_item, Some(kept_first));

        let restored = restore(state.clone());
        assert_eq!(
            restored.items.roots(Placement::Favorites { space: first }),
            &[kept_first]
        );
        assert_eq!(
            restored.items.roots(Placement::Favorites { space: second }),
            &[kept_second]
        );
        assert_eq!(restored.active_item, Some(kept_first));
        assert_eq!(canonicalize(state.clone()), state);
    }

    #[test]
    fn essentials_of_a_space_that_is_gone_are_dropped_on_restore() {
        let (profiles, spaces, items, _profile, space) = seed();
        let mut state = snapshot(&profiles, &spaces, &items, Some(space), None, None);
        state.items.push(PersistedItem {
            id: ItemId::from(40),
            parent: None,
            placement: Placement::Favorites {
                space: SpaceId::from(99),
            },
            kind: PersistedKind::Tab {
                url: "https://mail.example/".into(),
                title: "Mail".into(),
                zoom: 1.0,
                last_active_ms: None,
            },
        });

        assert!(restore(state).items.get(ItemId::from(40)).is_none());
    }

    #[test]
    fn recently_closed_tabs_are_bounded_sanitized_and_profile_scoped() {
        let (profiles, spaces, items, profile, space) = seed();
        let mut recent = (0..(MAX_RECENTLY_CLOSED_TABS + 3))
            .map(|index| PersistedClosedTab {
                profile,
                space,
                url: format!("https://example.com/{index}"),
                title: format!("Title {index}\u{202e}"),
                zoom: 2.0,
                session_id: None,
                closed_at_ms: None,
                position: None,
            })
            .collect::<Vec<_>>();
        recent.push(PersistedClosedTab {
            profile: ProfileId::from(999),
            space,
            url: "https://foreign.example/".into(),
            title: "Foreign".into(),
            zoom: 1.0,
            session_id: None,
            closed_at_ms: None,
            position: None,
        });
        recent.push(PersistedClosedTab {
            profile,
            space,
            url: "file:///private.txt".into(),
            title: "Local".into(),
            zoom: 1.0,
            session_id: None,
            closed_at_ms: None,
            position: None,
        });

        let state = snapshot_with_recently_closed(
            &profiles,
            &spaces,
            &items,
            Some(space),
            None,
            None,
            &recent,
        );
        assert_eq!(state.recently_closed.len(), MAX_RECENTLY_CLOSED_TABS);
        assert_eq!(state.recently_closed[0].url, "https://example.com/3");
        assert!(state
            .recently_closed
            .iter()
            .all(|entry| !entry.title.contains('\u{202e}')));
        assert_eq!(
            restore(state.clone()).recently_closed,
            state.recently_closed
        );
        assert_eq!(canonicalize(state.clone()), state);
    }

    #[test]
    fn a_closed_tabs_place_lasts_the_run_and_never_reaches_the_snapshot() {
        let (profiles, spaces, items, profile, space) = seed();
        let record = |position| PersistedClosedTab {
            profile,
            space,
            url: "https://example.test/place".into(),
            title: "Place".into(),
            zoom: 1.0,
            session_id: Some(ClosedSessionId::from(11)),
            closed_at_ms: Some(1_700_000_000_000),
            position,
        };
        let state = snapshot_with_recently_closed(
            &profiles,
            &spaces,
            &items,
            Some(space),
            None,
            None,
            &[record(Some(3))],
        );
        assert_eq!(state.recently_closed[0].position, Some(3));
        let written = serde_json::to_string(&state).unwrap();
        assert!(!written.contains("position"));
        let read: SessionState = serde_json::from_str(&written).unwrap();
        assert_eq!(read.recently_closed, vec![record(None)]);

        let wild = snapshot_with_recently_closed(
            &profiles,
            &spaces,
            &items,
            Some(space),
            None,
            None,
            &[record(Some(u32::MAX))],
        );
        assert_eq!(wild.recently_closed[0].position, None);
    }

    #[test]
    fn closed_session_metadata_is_paired_unique_and_legacy_records_remain_reopenable() {
        let (profiles, spaces, items, profile, space) = seed();
        let record = |suffix: &str, session_id, closed_at_ms| PersistedClosedTab {
            profile,
            space,
            url: format!("https://example.test/{suffix}"),
            title: suffix.into(),
            zoom: 1.0,
            session_id,
            closed_at_ms,
            position: None,
        };
        let same_id = ClosedSessionId::from(7);
        let state = snapshot_with_recently_closed(
            &profiles,
            &spaces,
            &items,
            Some(space),
            None,
            None,
            &[
                record("legacy", None, None),
                record("older", Some(same_id), Some(1_700_000_000_000)),
                record("newer", Some(same_id), Some(1_700_000_001_000)),
                record("partial", Some(ClosedSessionId::from(8)), None),
            ],
        );
        assert_eq!(state.recently_closed.len(), 2);
        assert_eq!(state.recently_closed[0].title, "legacy");
        assert_eq!(state.recently_closed[0].session_id, None);
        assert_eq!(state.recently_closed[1].title, "newer");
        assert_eq!(state.recently_closed[1].session_id, Some(same_id));
    }

    #[test]
    fn snapshot_skips_incognito_and_urlless_tabs() {
        let (mut profiles, mut spaces, mut items, _profile, space) = seed();
        let incognito = ProfileId::from(9);
        let incognito_space = SpaceId::from(19);
        profiles.insert(Profile {
            id: incognito,
            name: "Incognito".into(),
            kind: ProfileKind::Incognito,
        });
        spaces.insert(Space {
            id: incognito_space,
            profile: incognito,
            name: "Private".into(),
            look: Default::default(),
        });
        let fav = ItemId::from(20);
        items.insert_tab(
            fav,
            Placement::Favorites {
                space: incognito_space,
            },
        );
        navigate_and_commit(&mut items, fav, "example.com");
        let empty = ItemId::from(21);
        items.insert_tab(empty, today(space));

        let state = snapshot(&profiles, &spaces, &items, None, Some(empty), None);
        assert!(state.items.is_empty());
        assert!(state
            .profiles
            .iter()
            .all(|p| p.kind != ProfileKind::Incognito));
        assert_eq!(state.active_item, None);
    }

    #[test]
    fn browser_owned_tabs_restore_without_urls_and_extension_tabs_do_not_persist() {
        let (profiles, spaces, mut items, _profile, space) = seed();
        let extensions_tab = ItemId::from(31);
        let extension = ItemId::from(32);
        assert!(items.insert_browser_tab(
            extensions_tab,
            today(space),
            BrowserOwnedTab::Extensions,
        ));
        assert!(!items.insert_browser_tab(
            ItemId::from(34),
            today(space),
            BrowserOwnedTab::Settings
        ));
        assert!(items.insert_extension_tab(extension, today(space)));
        let mut state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(space),
            Some(extensions_tab),
            None,
        );
        assert!(state.items.iter().any(|item| {
            item.id == extensions_tab
                && matches!(
                    item.kind,
                    PersistedKind::BrowserTab {
                        page: BrowserOwnedTab::Extensions
                    }
                )
        }));
        assert!(!state.items.iter().any(|item| item.id == extension));
        let duplicate = ItemId::from(33);
        state.items.push(PersistedItem {
            id: duplicate,
            parent: None,
            placement: today(space),
            kind: PersistedKind::BrowserTab {
                page: BrowserOwnedTab::Extensions,
            },
        });
        let mut restored = restore(state);
        assert_eq!(restored.active_item, Some(extensions_tab));
        assert_eq!(
            restored.items.tab(extensions_tab).map(|tab| tab.content),
            Some(TabContent::BrowserOwned(BrowserOwnedTab::Extensions)),
        );
        assert!(restored.items.tab(duplicate).is_none());
        assert!(restored.items.ensure_view(extensions_tab).is_empty());
        assert!(restored
            .items
            .navigate(extensions_tab, "https://example.test/")
            .is_empty());
    }

    #[test]
    fn a_source_tab_is_never_saved_and_a_saved_one_is_not_restored() {
        let (profiles, spaces, mut items, _profile, space) = seed();
        let source = ItemId::from(41);
        assert!(items.insert_source_tab(source, today(space), "view-source:example.com"));
        assert!(!items.insert_browser_tab(ItemId::from(42), today(space), BrowserOwnedTab::Source));
        assert_eq!(
            items.tab(source).map(|tab| tab.title.as_str()),
            Some("view-source:example.com")
        );

        let mut state = snapshot(&profiles, &spaces, &items, Some(space), Some(source), None);
        assert!(state.items.iter().all(|item| item.id != source));
        state.items.push(PersistedItem {
            id: source,
            parent: None,
            placement: today(space),
            kind: PersistedKind::BrowserTab {
                page: BrowserOwnedTab::Source,
            },
        });
        assert!(restore(state).items.tab(source).is_none());
    }

    #[test]
    fn qa_settings_tab_remains_canonical_until_shell_migrates_it() {
        let (profiles, spaces, items, _profile, space) = seed();
        let mut state = snapshot(&profiles, &spaces, &items, Some(space), None, None);
        let qa_settings = ItemId::from(35);
        state.items.push(PersistedItem {
            id: qa_settings,
            parent: None,
            placement: today(space),
            kind: PersistedKind::BrowserTab {
                page: BrowserOwnedTab::Settings,
            },
        });
        state.active_item = Some(qa_settings);
        let restored = restore(state.clone());
        assert_eq!(restored.active_item, Some(qa_settings));
        assert_eq!(
            restored.items.tab(qa_settings).map(|tab| tab.content),
            Some(TabContent::BrowserOwned(BrowserOwnedTab::Settings)),
        );
        assert_eq!(canonicalize(state.clone()), state);
    }

    #[test]
    fn a_tabs_last_active_time_survives_restore_and_legacy_rows_have_none() {
        let profile = ProfileId::from(1);
        let space = SpaceId::from(2);
        let tab = |id: u128, last_active_ms: Option<u64>| PersistedItem {
            id: ItemId::from(id),
            parent: None,
            placement: today(space),
            kind: PersistedKind::Tab {
                url: format!("https://example.com/{id}"),
                title: "E".into(),
                zoom: 1.0,
                last_active_ms,
            },
        };
        let state = SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "P".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: space,
                profile,
                name: "S".into(),
                look: Default::default(),
            }],
            items: vec![tab(10, Some(1_700_000_000_000)), tab(11, None)],
            ..SessionState::default()
        };
        let restored = restore(state.clone());
        assert_eq!(
            restored.items.tab(ItemId::from(10)).unwrap().last_active_ms,
            Some(1_700_000_000_000)
        );
        assert_eq!(canonicalize(state.clone()), state);
        let json = serde_json::to_string(&state.items[1]).unwrap();
        assert!(!json.contains("last_active_ms"));
        let legacy: PersistedItem = serde_json::from_str(&json).unwrap();
        assert_eq!(legacy, state.items[1]);

        let mut hostile = state;
        hostile.items = vec![tab(12, Some(MAX_EXACT_MS + 1)), tab(13, Some(0))];
        let restored = restore(hostile);
        for id in [12, 13] {
            assert_eq!(
                restored.items.tab(ItemId::from(id)).unwrap().last_active_ms,
                None
            );
        }
    }

    #[test]
    fn restore_drops_dangling_references() {
        let state = SessionState {
            profiles: vec![],
            spaces: vec![PersistedSpace {
                id: SpaceId::from(2),
                profile: ProfileId::from(1),
                name: "Orphan".into(),
                look: Default::default(),
            }],
            items: vec![PersistedItem {
                id: ItemId::from(10),
                parent: None,
                placement: today(SpaceId::from(2)),
                kind: PersistedKind::Tab {
                    url: "https://example.com/".into(),
                    title: "E".into(),
                    zoom: 1.0,
                    last_active_ms: None,
                },
            }],
            active_space: Some(SpaceId::from(2)),
            active_item: Some(ItemId::from(10)),
            splits: Some(Pane::leaf(ItemId::from(10))),
            recently_closed: Vec::new(),
        };
        let restored = restore(state);
        assert!(restored.spaces.is_empty());
        assert!(restored.items.get(ItemId::from(10)).is_none());
        assert_eq!(restored.active_item, None);
        assert_eq!(restored.splits, None);
    }

    #[test]
    fn canonicalize_filters_private_dangerous_and_hostile_display_data() {
        let profile = ProfileId::from(1);
        let private = ProfileId::from(2);
        let space = SpaceId::from(3);
        let private_space = SpaceId::from(4);
        let kept = ItemId::from(10);
        let dangerous = ItemId::from(11);
        let private_tab = ItemId::from(12);
        let state = SessionState {
            profiles: vec![
                PersistedProfile {
                    id: profile,
                    name: format!("\u{202e}Personal\0{}", "x".repeat(300)),
                    kind: ProfileKind::Default,
                },
                PersistedProfile {
                    id: private,
                    name: "Private".into(),
                    kind: ProfileKind::Incognito,
                },
            ],
            spaces: vec![
                PersistedSpace {
                    id: space,
                    profile,
                    name: "\u{2066}Work\n".into(),
                    look: Default::default(),
                },
                PersistedSpace {
                    id: private_space,
                    profile: private,
                    name: "Secret".into(),
                    look: Default::default(),
                },
            ],
            items: vec![
                PersistedItem {
                    id: kept,
                    parent: None,
                    placement: today(space),
                    kind: PersistedKind::Tab {
                        url: "https://example.com/".into(),
                        title: format!("\u{202e}Example\0{}", "y".repeat(600)),
                        zoom: f64::INFINITY,
                        last_active_ms: None,
                    },
                },
                PersistedItem {
                    id: dangerous,
                    parent: None,
                    placement: today(space),
                    kind: PersistedKind::Tab {
                        url: "file:///etc/passwd".into(),
                        title: "Local file".into(),
                        zoom: 1.0,
                        last_active_ms: None,
                    },
                },
                PersistedItem {
                    id: private_tab,
                    parent: None,
                    placement: today(private_space),
                    kind: PersistedKind::Tab {
                        url: "https://private.example/".into(),
                        title: "Private".into(),
                        zoom: 1.0,
                        last_active_ms: None,
                    },
                },
            ],
            active_space: Some(private_space),
            active_item: Some(dangerous),
            splits: Some(Pane::Branch {
                axis: crate::split::Axis::Row,
                ratio: 0.5,
                a: Box::new(Pane::Leaf(kept)),
                b: Box::new(Pane::Leaf(kept)),
            }),
            recently_closed: Vec::new(),
        };

        let clean = canonicalize(state);
        assert_eq!(clean.profiles.len(), 1);
        assert_eq!(clean.spaces.len(), 1);
        assert_eq!(clean.items.len(), 1);
        assert_eq!(
            clean.profiles[0].name.chars().count(),
            MAX_SESSION_NAME_CHARS
        );
        assert!(clean.profiles[0]
            .name
            .chars()
            .all(|c| !c.is_control() && c != '\u{202e}'));
        assert_eq!(clean.spaces[0].name, "Work");
        let PersistedKind::Tab { title, zoom, .. } = &clean.items[0].kind else {
            panic!("kept item changed kind")
        };
        assert_eq!(title.chars().count(), crate::item::MAX_PAGE_TITLE_CHARS);
        assert!(title.chars().all(|c| !c.is_control() && c != '\u{202e}'));
        assert_eq!(*zoom, 1.0);
        assert_eq!(clean.active_space, None);
        assert_eq!(clean.active_item, None);
        assert_eq!(
            clean.splits, None,
            "duplicate split leaves must be rejected"
        );
    }

    #[test]
    fn focus_and_splits_cannot_cross_the_active_space_or_profile() {
        let (mut profiles, mut spaces, mut items, profile, active_space) = seed();
        let sibling_space = SpaceId::from(3);
        spaces.insert(Space {
            id: sibling_space,
            profile,
            name: "Sibling".into(),
            look: Default::default(),
        });
        let foreign_profile = ProfileId::from(4);
        let foreign_space = SpaceId::from(5);
        profiles.insert(Profile {
            id: foreign_profile,
            name: "Foreign".into(),
            kind: ProfileKind::Named,
        });
        spaces.insert(Space {
            id: foreign_space,
            profile: foreign_profile,
            name: "Foreign".into(),
            look: Default::default(),
        });

        let local = ItemId::from(10);
        let sibling = ItemId::from(11);
        let foreign = ItemId::from(12);
        let favorite = ItemId::from(13);
        let foreign_favorite = ItemId::from(14);
        let sibling_favorite = ItemId::from(15);
        for (id, placement, url) in [
            (local, today(active_space), "local.example"),
            (sibling, today(sibling_space), "sibling.example"),
            (foreign, today(foreign_space), "foreign.example"),
            (
                favorite,
                Placement::Favorites {
                    space: active_space,
                },
                "favorite.example",
            ),
            (
                sibling_favorite,
                Placement::Favorites {
                    space: sibling_space,
                },
                "sibling-favorite.example",
            ),
            (
                foreign_favorite,
                Placement::Favorites {
                    space: foreign_space,
                },
                "foreign-favorite.example",
            ),
        ] {
            assert!(items.insert_tab(id, placement));
            navigate_and_commit(&mut items, id, url);
        }

        // A space's own Essentials are in its scope, a sibling space's are not.
        let favorite_state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(active_space),
            Some(favorite),
            Some(&Pane::Leaf(favorite)),
        );
        assert_eq!(favorite_state.active_item, Some(favorite));
        assert_eq!(favorite_state.splits, Some(Pane::Leaf(favorite)));

        for attacker in [sibling, foreign, sibling_favorite, foreign_favorite] {
            let state = snapshot(
                &profiles,
                &spaces,
                &items,
                Some(active_space),
                Some(attacker),
                Some(&Pane::Branch {
                    axis: crate::split::Axis::Row,
                    ratio: 0.5,
                    a: Box::new(Pane::Leaf(local)),
                    b: Box::new(Pane::Leaf(attacker)),
                }),
            );
            assert_eq!(state.active_item, None);
            assert_eq!(state.splits, None);

            // Restore must independently reject equivalent forged store
            // references; it cannot rely on snapshots being the producer.
            let mut forged = snapshot(
                &profiles,
                &spaces,
                &items,
                Some(active_space),
                Some(local),
                None,
            );
            forged.active_item = Some(attacker);
            forged.splits = Some(Pane::Branch {
                axis: crate::split::Axis::Row,
                ratio: 0.5,
                a: Box::new(Pane::Leaf(local)),
                b: Box::new(Pane::Leaf(attacker)),
            });
            let restored = restore(forged);
            assert_eq!(restored.active_space, Some(active_space));
            assert_eq!(restored.active_item, None);
            assert_eq!(restored.splits, None);
        }
    }

    #[test]
    fn restore_rejects_out_of_range_split_geometry() {
        let (profiles, spaces, mut items, _profile, space) = seed();
        let (a, b) = (ItemId::from(10), ItemId::from(11));
        items.insert_tab(a, today(space));
        items.insert_tab(b, today(space));
        navigate_and_commit(&mut items, a, "example.com");
        navigate_and_commit(&mut items, b, "example.org");
        let mut state = snapshot(&profiles, &spaces, &items, Some(space), Some(a), None);
        state.splits = Some(Pane::Branch {
            axis: crate::split::Axis::Col,
            ratio: -10.0,
            a: Box::new(Pane::Leaf(a)),
            b: Box::new(Pane::Leaf(b)),
        });

        assert_eq!(restore(state).splits, None);
    }

    #[test]
    fn spaces_keep_order_and_look_and_a_damaged_look_is_sanitized() {
        use crate::spaces::{SpaceIcon, SpaceTheme};
        let (profiles, mut spaces, items, profile, space) = seed();
        let look = SpaceLook {
            icon: Some(SpaceIcon::Symbol(4)),
            color: 6,
            theme: Some(SpaceTheme::Preset("dracula".into())),
        };
        assert!(spaces.create(SpaceId::from(3), profile, "Night", look.clone()));
        assert!(spaces.reorder(SpaceId::from(3), Some(space)));
        let mut state = snapshot(
            &profiles,
            &spaces,
            &items,
            Some(SpaceId::from(3)),
            None,
            None,
        );
        assert_eq!(canonicalize(state.clone()), state);
        let restored = restore(state.clone());
        let order: Vec<_> = restored
            .spaces
            .iter()
            .map(|s| (s.id, s.look.clone()))
            .collect();
        assert_eq!(
            order,
            vec![(SpaceId::from(3), look), (space, SpaceLook::default())]
        );
        assert_eq!(restored.active_space, Some(SpaceId::from(3)));

        state.spaces[0].look.color = 200;
        state.spaces[0].look.theme = Some(SpaceTheme::Custom("{}".into()));
        let restored = restore(state);
        let look = &restored.spaces.get(SpaceId::from(3)).unwrap().look;
        assert_eq!((look.color, &look.theme), (0, &None));
        assert_eq!(look.icon, Some(SpaceIcon::Symbol(4)));
    }
}
