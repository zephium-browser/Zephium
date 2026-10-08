//! The sidebar item tree: one aggregate for favorites, pinned tabs, folders
//! and ephemeral (Today) tabs. Mutations return `Effect`s for the engine.

use std::collections::{HashMap, HashSet};

use url::Url;

use crate::ids::ItemId;
use crate::item::{
    sanitize_page_title, BrowserOwnedTab, Item, ItemKind, Lifecycle, Placement, TabContent,
    TabState,
};
use crate::navigation;
use crate::ports::engine::NavigationRequestId;
use crate::spaces::RemovedProfileSpaces;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    CreateView {
        id: ItemId,
        url: String,
    },
    Navigate {
        id: ItemId,
        url: String,
        request: NavigationRequestId,
    },
    Close {
        id: ItemId,
    },
}

#[derive(Default)]
pub struct Items {
    items: HashMap<ItemId, Item>,
    roots: HashMap<Placement, Vec<ItemId>>,
    children: HashMap<ItemId, Vec<ItemId>>,
    // Runtime-only navigation intents. Session state and chrome projections
    // use TabState::url, which changes only after an authoritative native
    // UrlChanged observation.
    pending_navigations: HashMap<ItemId, PendingNavigation>,
    next_navigation_request: u64,
}

#[derive(Clone, Debug)]
struct PendingNavigation {
    request: NavigationRequestId,
    /// The address asked for, when known, so a failure can name it.
    url: Option<Url>,
}

impl Items {
    pub fn get(&self, id: ItemId) -> Option<&Item> {
        self.items.get(&id)
    }

    pub fn tab(&self, id: ItemId) -> Option<&TabState> {
        self.items.get(&id).and_then(Item::tab)
    }

    /// Legacy QA Settings tabs are decoded so durable snapshots remain
    /// canonical, then retired by the Shell before its first projection.
    pub fn retired_settings_tab_ids(&self) -> Vec<ItemId> {
        let mut ids = self
            .items
            .iter()
            .filter_map(|(id, item)| {
                item.tab()
                    .is_some_and(|tab| {
                        tab.content == TabContent::BrowserOwned(BrowserOwnedTab::Settings)
                    })
                    .then_some(*id)
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    fn tab_mut(&mut self, id: ItemId) -> Option<&mut TabState> {
        self.items.get_mut(&id).and_then(Item::tab_mut)
    }

    pub fn set_media_capture(
        &mut self,
        id: ItemId,
        capture: Option<(
            crate::ports::engine::NavigationPresentationId,
            crate::ports::engine::MediaCaptureState,
        )>,
    ) -> bool {
        let Some(tab) = self.tab_mut(id) else {
            return false;
        };
        if tab.capture == capture {
            return false;
        }
        tab.capture = capture;
        true
    }

    pub fn roots(&self, placement: Placement) -> &[ItemId] {
        self.roots.get(&placement).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn children(&self, id: ItemId) -> &[ItemId] {
        self.children.get(&id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Inserts at the end of its container. Parent (when set) must be an
    /// existing folder with the same placement; otherwise the insert is refused.
    pub fn insert(&mut self, item: Item) -> bool {
        if self.items.len() >= crate::session::MAX_SESSION_ITEMS
            || self.items.contains_key(&item.id)
        {
            return false;
        }
        match item.parent {
            Some(parent) => {
                let ok = self.items.get(&parent).is_some_and(|p| {
                    matches!(p.kind, ItemKind::Folder { .. }) && p.placement == item.placement
                });
                if !ok {
                    return false;
                }
                // Item snapshots are flat, so JSON's recursion limit cannot
                // protect the later recursive projection/persistence walks.
                // Keep the aggregate's invariant bounded at insertion time,
                // including future non-storage callers.
                let mut ancestor = Some(parent);
                let mut depth = 1_usize;
                while let Some(id) = ancestor {
                    if depth > crate::session::MAX_ITEM_TREE_DEPTH {
                        return false;
                    }
                    ancestor = self.items.get(&id).and_then(|item| item.parent);
                    depth += 1;
                }
                self.children.entry(parent).or_default().push(item.id);
            }
            None => self.roots.entry(item.placement).or_default().push(item.id),
        }
        self.items.insert(item.id, item);
        true
    }

    pub fn insert_tab(&mut self, id: ItemId, placement: Placement) -> bool {
        self.insert(Item {
            id,
            parent: None,
            placement,
            kind: ItemKind::Tab(TabState::new()),
        })
    }

    /// A tab that knows its page but has not loaded it, exactly as a restored
    /// one: it creates no view, so nothing reaches the network until it is
    /// opened.
    pub fn insert_unloaded_tab(
        &mut self,
        id: ItemId,
        placement: Placement,
        url: Url,
        title: &str,
    ) -> bool {
        if !navigation::is_allowed(&url) {
            return false;
        }
        let mut tab = TabState::new();
        tab.url = Some(url);
        tab.title = sanitize_page_title(title);
        self.insert(Item {
            id,
            parent: None,
            placement,
            kind: ItemKind::Tab(tab),
        })
    }

    /// Inserts a browser-owned page as a real, URL-less tab item. It never
    /// acquires an ordinary native content view.
    pub fn insert_browser_tab(
        &mut self,
        id: ItemId,
        placement: Placement,
        page: BrowserOwnedTab,
    ) -> bool {
        // Settings is a window-scoped browser page. Keep the enum variant only
        // to decode and discard snapshots produced by an earlier QA build.
        if page == BrowserOwnedTab::Settings {
            return false;
        }
        self.insert(Item {
            id,
            parent: None,
            placement,
            kind: ItemKind::Tab(TabState::browser_owned(page)),
        })
    }

    /// Inserts a transient extension-owned tab marker. The extension broker
    /// must bind a native guest separately; this does not create a webview.
    pub fn insert_extension_tab(&mut self, id: ItemId, placement: Placement) -> bool {
        self.insert(Item {
            id,
            parent: None,
            placement,
            kind: ItemKind::Tab(TabState::extension_owned()),
        })
    }

    /// Removes an item and its whole subtree. Returns `Close` effects for
    /// every removed tab that had a live view.
    /// Reparents one existing tab without recreating its native identity or
    /// cancelling navigation. The caller authorizes the profile/space scope.
    pub fn move_tab_to_root(
        &mut self,
        id: ItemId,
        placement: Placement,
        before: Option<ItemId>,
    ) -> bool {
        let Some(item) = self.items.get(&id).filter(|item| item.tab().is_some()) else {
            return false;
        };
        match item.tab().expect("tab filtered above").content {
            TabContent::Web => {}
            TabContent::BrowserOwned(_) => {
                if !matches!(
                    (item.placement, placement),
                    (
                        Placement::Space { space: current, .. },
                        Placement::Space { space: destination, .. }
                    ) if current == destination
                ) {
                    return false;
                }
            }
            TabContent::ExtensionOwned if item.placement != placement => return false,
            TabContent::ExtensionOwned => {}
        }
        if before == Some(id) {
            return item.placement == placement && item.parent.is_none();
        }
        if before.is_some_and(|target| {
            self.items
                .get(&target)
                .is_none_or(|item| item.parent.is_some() || item.placement != placement)
        }) {
            return false;
        }
        let parent = item.parent;
        let old_placement = item.placement;
        let siblings = match parent {
            Some(parent) => self.children.get_mut(&parent),
            None => self.roots.get_mut(&old_placement),
        };
        if let Some(siblings) = siblings {
            siblings.retain(|item| *item != id);
        }
        let target = self.roots.entry(placement).or_default();
        let index = before
            .and_then(|before| target.iter().position(|item| *item == before))
            .unwrap_or(target.len());
        target.insert(index, id);
        if let Some(item) = self.items.get_mut(&id) {
            item.parent = None;
            item.placement = placement;
        }
        true
    }

    pub fn remove(&mut self, id: ItemId) -> Vec<Effect> {
        let Some(item) = self.items.get(&id) else {
            return Vec::new();
        };
        match item.parent {
            Some(parent) => {
                if let Some(list) = self.children.get_mut(&parent) {
                    list.retain(|x| *x != id);
                }
            }
            None => {
                if let Some(list) = self.roots.get_mut(&item.placement) {
                    list.retain(|x| *x != id);
                }
            }
        }
        let mut effects = Vec::new();
        let mut stack = vec![id];
        while let Some(next) = stack.pop() {
            stack.extend(self.children.remove(&next).unwrap_or_default());
            self.pending_navigations.remove(&next);
            if let Some(removed) = self.items.remove(&next) {
                if removed.tab().is_some_and(TabState::has_view) {
                    effects.push(Effect::Close { id: next });
                }
            }
        }
        effects
    }

    /// Removes items whose ownership is directly proven by a profile's
    /// favorites placement or by the exact `Spaces` aggregate removal proof.
    /// A space absent from that proof is never inferred to belong to the
    /// profile. Returns at most `MAX_SESSION_ITEMS` native close effects.
    pub fn remove_for_profile(&mut self, spaces: &RemovedProfileSpaces) -> Vec<Effect> {
        let removed_spaces: HashSet<_> = spaces.ids().iter().copied().collect();
        let owned: HashSet<ItemId> = self
            .items
            .iter()
            .filter_map(|(id, item)| {
                let owned = match item.placement {
                    Placement::Favorites { profile } => profile == spaces.profile(),
                    Placement::Space { space, .. } => removed_spaces.contains(&space),
                };
                owned.then_some(*id)
            })
            .collect();
        if owned.is_empty() {
            return Vec::new();
        }

        let mut ordered: Vec<ItemId> = owned.iter().copied().collect();
        ordered.sort_unstable();
        let effects = ordered
            .iter()
            .filter(|id| {
                self.items
                    .get(id)
                    .and_then(Item::tab)
                    .is_some_and(TabState::has_view)
            })
            .map(|id| Effect::Close { id: *id })
            .collect();

        // Preserve any structurally unexpected child that does not itself
        // carry direct ownership proof. Re-rooting is safer than recursively
        // deleting across an invalid ownership boundary.
        let mut reroot = Vec::new();
        for parent in &ordered {
            if let Some(children) = self.children.remove(parent) {
                for child in children {
                    if owned.contains(&child) {
                        continue;
                    }
                    let Some(item) = self.items.get_mut(&child) else {
                        continue;
                    };
                    if item.parent == Some(*parent) {
                        item.parent = None;
                        reroot.push((item.placement, child));
                    }
                }
            }
        }

        for roots in self.roots.values_mut() {
            roots.retain(|id| !owned.contains(id));
        }
        for children in self.children.values_mut() {
            children.retain(|id| !owned.contains(id));
        }
        for (placement, id) in reroot {
            let roots = self.roots.entry(placement).or_default();
            if !roots.contains(&id) {
                roots.push(id);
            }
        }
        for id in ordered {
            self.pending_navigations.remove(&id);
            self.items.remove(&id);
        }
        self.roots.retain(|_, roots| !roots.is_empty());
        self.children.retain(|_, children| !children.is_empty());
        effects
    }

    pub fn navigate(&mut self, id: ItemId, input: &str) -> Vec<Effect> {
        let Some(url) = navigation::classify(input) else {
            return Vec::new();
        };
        if !self
            .tab(id)
            .is_some_and(|tab| tab.content == TabContent::Web)
        {
            return Vec::new();
        }
        let request = self.mint_navigation_request();
        self.pending_navigations.insert(
            id,
            PendingNavigation {
                request,
                url: Some(url.clone()),
            },
        );
        let Some(tab) = self.tab_mut(id) else {
            // Keep this path non-panicking even if a future mutation is added
            // between the existence check and this borrow.  A failed admission
            // must not leave a request that can later be attributed to another
            // native view generation.
            self.pending_navigations.remove(&id);
            return Vec::new();
        };
        tab.failure = None;
        let url = url.to_string();
        if tab.view {
            vec![Effect::Navigate { id, url, request }]
        } else {
            tab.view = true;
            vec![Effect::CreateView { id, url }]
        }
    }

    /// Reserves one exact first-navigation intent while a native tabs.create
    /// reply is pending. A later admitted navigation supersedes this marker
    /// even if that later native load fails and removes its own request.
    pub fn reserve_deferred_navigation(&mut self, id: ItemId) -> Option<NavigationRequestId> {
        if !self
            .tab(id)
            .is_some_and(|tab| tab.content == TabContent::Web && !tab.view && tab.url.is_none())
        {
            return None;
        }
        let request = self.mint_navigation_request();
        self.pending_navigations
            .insert(id, PendingNavigation { request, url: None });
        Some(request)
    }

    pub fn pending_navigation_request(&self, id: ItemId) -> Option<NavigationRequestId> {
        self.pending_navigations
            .get(&id)
            .map(|pending| pending.request)
    }

    fn mint_navigation_request(&mut self) -> NavigationRequestId {
        self.next_navigation_request = self.next_navigation_request.wrapping_add(1);
        if self.next_navigation_request == 0 {
            self.next_navigation_request = 1;
        }
        NavigationRequestId(self.next_navigation_request)
    }

    /// Attach a preconfigured native popup without issuing Create/Navigate or
    /// claiming a URL commit. Only later native observations attribute content.
    pub fn adopt_native_view(&mut self, id: ItemId) -> bool {
        let Some(tab) = self.tab_mut(id) else {
            return false;
        };
        if tab.content != TabContent::Web || tab.view || tab.url.is_some() {
            return false;
        }
        tab.view = true;
        tab.loading = true;
        true
    }

    /// Marks an already broker-authorized native extension guest ready for
    /// presentation. This does not create an ordinary page WebView or URL.
    pub fn adopt_extension_view(&mut self, id: ItemId) -> bool {
        let Some(tab) = self.tab_mut(id) else {
            return false;
        };
        if tab.content != TabContent::ExtensionOwned || tab.view || tab.url.is_some() {
            return false;
        }
        tab.view = true;
        tab.loading = false;
        true
    }

    pub fn ensure_view(&mut self, id: ItemId) -> Vec<Effect> {
        if let Some(tab) = self.tab_mut(id) {
            if tab.content == TabContent::Web && !tab.view {
                if let Some(url) = tab.url.clone() {
                    tab.view = true;
                    return vec![Effect::CreateView {
                        id,
                        url: url.to_string(),
                    }];
                }
            }
        }
        Vec::new()
    }

    pub fn view_ids(&self) -> Vec<ItemId> {
        self.items
            .iter()
            .filter(|(_, item)| {
                item.tab()
                    .is_some_and(|tab| tab.content == TabContent::Web && tab.has_view())
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Drops the tab's webview but keeps the item; activation recreates it.
    pub fn hibernate(&mut self, id: ItemId) -> Vec<Effect> {
        if !self
            .tab(id)
            .is_some_and(|tab| tab.content == TabContent::Web)
        {
            return Vec::new();
        }
        if self.mark_view_discarded(id) {
            vec![Effect::Close { id }]
        } else {
            Vec::new()
        }
    }

    /// Records an engine-acknowledged native discard without emitting a
    /// second close. Used only after the engine has physically destroyed the
    /// exact view generation and released its same-id reuse gate.
    pub fn mark_view_discarded(&mut self, id: ItemId) -> bool {
        self.pending_navigations.remove(&id);
        let Some(tab) = self.tab_mut(id) else {
            return false;
        };
        if !tab.view {
            return false;
        }
        tab.view = false;
        tab.capture = None;
        tab.loading = false;
        tab.lifecycle = Lifecycle::Hibernated;
        true
    }

    pub fn set_lifecycle(&mut self, id: ItemId, lifecycle: Lifecycle) {
        if let Some(tab) = self.tab_mut(id) {
            tab.lifecycle = lifecycle;
        }
    }

    /// Roll back the optimistic `view` bit when the native engine cannot
    /// create a controller. The next explicit navigation/activation can then
    /// retry instead of leaving an unrecoverable blank tab.
    pub fn view_creation_failed(&mut self, id: ItemId) {
        self.pending_navigations.remove(&id);
        if let Some(tab) = self.tab_mut(id) {
            tab.view = false;
            tab.capture = None;
            tab.loading = false;
            tab.lifecycle = Lifecycle::Hibernated;
            tab.title = "Page failed to open".into();
        }
    }

    pub fn set_title(&mut self, id: ItemId, title: String) {
        if let Some(tab) = self.tab_mut(id) {
            tab.title = sanitize_page_title(&title);
        }
    }

    pub fn set_loading(&mut self, id: ItemId, loading: bool) {
        if let Some(tab) = self.tab_mut(id) {
            tab.loading = loading;
        }
    }

    pub fn set_page_request(&mut self, id: ItemId, request: crate::item::PageRequest) -> bool {
        match self.tab_mut(id) {
            Some(tab) if tab.content == TabContent::Web => {
                tab.page_request = Some(Box::new(request));
                true
            }
            _ => false,
        }
    }

    pub fn take_page_request(&mut self, id: ItemId) -> Option<crate::item::PageRequest> {
        self.tab_mut(id)?
            .page_request
            .take()
            .map(|request| *request)
    }

    /// A new tab from this page did open, so an earlier refusal is moot.
    pub fn clear_blocked_popup(&mut self, id: ItemId) -> bool {
        match self.tab_mut(id) {
            Some(tab)
                if matches!(
                    tab.page_request.as_deref(),
                    Some(crate::item::PageRequest::Popup { .. })
                ) =>
            {
                tab.page_request = None;
                true
            }
            _ => false,
        }
    }
    pub fn set_committed_url(&mut self, id: ItemId, url: Url) -> bool {
        if let Some(tab) = self.tab_mut(id) {
            if tab.content != TabContent::Web {
                return false;
            }
            tab.url = Some(url);
            tab.page_request = None;
            tab.failure = None;
            self.pending_navigations.remove(&id);
            true
        } else {
            false
        }
    }

    /// Clears only the still-current intent. A delayed failure from an older
    /// native request cannot cancel a newer navigation.
    pub fn navigation_failed(&mut self, id: ItemId, request: NavigationRequestId) -> bool {
        let current = self
            .pending_navigations
            .get(&id)
            .is_some_and(|pending| pending.request == request);
        if current {
            self.pending_navigations.remove(&id);
        }
        current
    }

    /// Remembers why the address the person asked for did not load, so chrome
    /// can say so and offer it again. A page's own navigation has no pending
    /// address and is left alone.
    pub fn record_navigation_failure(
        &mut self,
        id: ItemId,
        reason: crate::ports::engine::NavigationFailureReason,
    ) -> bool {
        let Some(url) = self
            .pending_navigations
            .get(&id)
            .and_then(|pending| pending.url.clone())
        else {
            return false;
        };
        let Some(tab) = self.tab_mut(id) else {
            return false;
        };
        tab.failure = Some(Box::new(crate::item::NavigationFailure { url, reason }));
        true
    }

    #[cfg(test)]
    fn pending_navigation(&self, id: ItemId) -> Option<NavigationRequestId> {
        self.pending_navigation_request(id)
    }

    pub fn set_committed_url_str(&mut self, id: ItemId, url: &str) -> bool {
        if let Ok(parsed) = Url::parse(url) {
            if !navigation::is_browser_target(&parsed) {
                return false;
            }
            return self.set_committed_url(id, parsed);
        }
        false
    }

    pub fn set_zoom(&mut self, id: ItemId, zoom: f64) {
        if let Some(tab) = self.tab_mut(id) {
            tab.zoom = zoom;
        }
    }

    pub fn set_nav_flags(&mut self, id: ItemId, can_go_back: bool, can_go_forward: bool) {
        if let Some(tab) = self.tab_mut(id) {
            tab.can_go_back = can_go_back;
            tab.can_go_forward = can_go_forward;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ProfileId, SpaceId};
    use crate::item::SpaceSection;
    use crate::spaces::{Space, Spaces};
    use proptest::prelude::*;

    fn id(n: u128) -> ItemId {
        ItemId::from(n)
    }

    fn today() -> Placement {
        Placement::Space {
            space: SpaceId::from(1),
            section: SpaceSection::Today,
        }
    }

    #[test]
    fn extension_guest_requires_explicit_adoption_and_closes_without_lru_admission() {
        let mut items = Items::default();
        let id = ItemId::from(90);
        assert!(items.insert_extension_tab(id, today()));
        assert!(items.ensure_view(id).is_empty());
        assert!(items.navigate(id, "https://example.test/").is_empty());
        assert!(!items.adopt_native_view(id));
        assert!(items.adopt_extension_view(id));
        assert!(items.tab(id).unwrap().has_view());
        assert!(items.view_ids().is_empty());
        assert!(items.hibernate(id).is_empty());
        assert_eq!(items.remove(id), vec![Effect::Close { id }]);
    }

    fn folder(items: &mut Items, fid: u128) -> ItemId {
        let fid = id(fid);
        assert!(items.insert(Item {
            id: fid,
            parent: None,
            placement: today(),
            kind: ItemKind::Folder { name: "F".into() },
        }));
        fid
    }

    #[test]
    fn profile_removal_requires_direct_ownership_and_preserves_unknown_spaces() {
        let target = ProfileId::from(1);
        let other = ProfileId::from(2);
        let target_space = SpaceId::from(10);
        let other_space = SpaceId::from(11);
        let unknown_space = SpaceId::from(99);
        let mut spaces = Spaces::default();
        for (id, profile) in [(target_space, target), (other_space, other)] {
            assert!(spaces.insert(Space {
                id,
                profile,
                name: "Space".into(),
            }));
        }

        let target_favorite = id(1);
        let target_tab = id(2);
        let other_tab = id(3);
        let unknown_tab = id(4);
        let target_folder = id(5);
        let structurally_foreign_child = id(6);
        let mut items = Items::default();
        assert!(items.insert_tab(target_favorite, Placement::Favorites { profile: target },));
        assert!(items.insert_tab(
            target_tab,
            Placement::Space {
                space: target_space,
                section: SpaceSection::Today,
            },
        ));
        assert!(items.insert_tab(
            other_tab,
            Placement::Space {
                space: other_space,
                section: SpaceSection::Today,
            },
        ));
        assert!(items.insert_tab(
            unknown_tab,
            Placement::Space {
                space: unknown_space,
                section: SpaceSection::Today,
            },
        ));
        assert!(items.insert(Item {
            id: target_folder,
            parent: None,
            placement: Placement::Favorites { profile: target },
            kind: ItemKind::Folder { name: "F".into() },
        }));
        assert!(items.insert_tab(
            structurally_foreign_child,
            Placement::Favorites { profile: other },
        ));

        for tab in [target_favorite, target_tab, other_tab, unknown_tab] {
            assert!(!items.navigate(tab, "https://example.com").is_empty());
        }

        // Model a damaged internal parent edge. Direct placement ownership,
        // not ancestry, must decide whether the child is deleted.
        items
            .roots
            .get_mut(&Placement::Favorites { profile: other })
            .unwrap()
            .retain(|id| *id != structurally_foreign_child);
        items
            .items
            .get_mut(&structurally_foreign_child)
            .unwrap()
            .parent = Some(target_folder);
        items
            .children
            .entry(target_folder)
            .or_default()
            .push(structurally_foreign_child);

        let proof = spaces.remove_for_profile(target);
        let effects = items.remove_for_profile(&proof);
        assert_eq!(
            effects,
            vec![
                Effect::Close {
                    id: target_favorite,
                },
                Effect::Close { id: target_tab },
            ]
        );
        for removed in [target_favorite, target_tab, target_folder] {
            assert!(items.get(removed).is_none());
        }
        for preserved in [other_tab, unknown_tab, structurally_foreign_child] {
            assert!(items.get(preserved).is_some());
        }
        assert_eq!(items.get(structurally_foreign_child).unwrap().parent, None);
        assert_eq!(
            items.roots(Placement::Favorites { profile: other }),
            &[structurally_foreign_child]
        );
    }

    #[test]
    fn item_tree_depth_is_bounded_before_recursive_consumers_see_it() {
        let mut items = Items::default();
        let mut parent = folder(&mut items, 1);
        for depth in 1..=crate::session::MAX_ITEM_TREE_DEPTH {
            let child = id(depth as u128 + 1);
            assert!(items.insert(Item {
                id: child,
                parent: Some(parent),
                placement: today(),
                kind: ItemKind::Folder { name: "F".into() },
            }));
            parent = child;
        }

        assert!(!items.insert(Item {
            id: id(10_000),
            parent: Some(parent),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
        assert_eq!(items.items.len(), crate::session::MAX_ITEM_TREE_DEPTH + 1);
    }

    #[test]
    fn navigate_creates_view_then_navigates() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(!items.tab(id(1)).unwrap().has_view());

        let fx = items.navigate(id(1), "example.com");
        assert_eq!(
            fx,
            vec![Effect::CreateView {
                id: id(1),
                url: "https://example.com/".into()
            }]
        );
        assert!(items.tab(id(1)).unwrap().has_view());
        assert!(items.tab(id(1)).unwrap().url.is_none());
        assert!(!items.tab(id(1)).unwrap().loading);
        assert_eq!(
            items.pending_navigation(id(1)),
            Some(NavigationRequestId(1))
        );

        items.set_committed_url_str(id(1), "https://example.com/");
        assert_eq!(
            items.tab(id(1)).unwrap().url.as_ref().map(Url::as_str),
            Some("https://example.com/")
        );
        assert_eq!(items.pending_navigation(id(1)), None);

        let fx = items.navigate(id(1), "github.com");
        assert_eq!(
            fx,
            vec![Effect::Navigate {
                id: id(1),
                url: "https://github.com/".into(),
                request: NavigationRequestId(2),
            }]
        );
        // An intent is never exposed as the page currently displayed.
        assert_eq!(
            items.tab(id(1)).unwrap().url.as_ref().map(Url::as_str),
            Some("https://example.com/")
        );
        assert_eq!(
            items.pending_navigation(id(1)),
            Some(NavigationRequestId(2))
        );
    }

    #[test]
    fn an_unloaded_tab_knows_its_page_without_a_view() {
        let mut items = Items::default();
        let kept = Placement::Favorites {
            profile: ProfileId::from(1),
        };
        let url = Url::parse("https://app.slack.com/client").unwrap();
        assert!(items.insert_unloaded_tab(id(1), kept, url, "Slack\u{202e}"));
        let tab = items.tab(id(1)).unwrap();
        assert!(!tab.has_view());
        assert_eq!(tab.title, "Slack");
        assert_eq!(
            tab.url.as_ref().map(Url::as_str),
            Some("https://app.slack.com/client")
        );
        assert_eq!(items.roots(kept), &[id(1)]);
        assert!(matches!(
            items.ensure_view(id(1)).as_slice(),
            [Effect::CreateView { url, .. }] if url == "https://app.slack.com/client"
        ));

        let script = Url::parse("javascript:alert(1)").unwrap();
        assert!(!items.insert_unloaded_tab(id(2), kept, script, "x"));
        assert!(items.tab(id(2)).is_none());
    }

    #[test]
    fn acknowledged_native_discard_preserves_metadata_without_a_second_close() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(matches!(
            items.navigate(id(1), "https://example.com/path").as_slice(),
            [Effect::CreateView { .. }]
        ));
        assert!(items.set_committed_url_str(id(1), "https://example.com/path"));
        items.set_title(id(1), "Kept title".into());

        assert!(items.mark_view_discarded(id(1)));
        let tab = items.tab(id(1)).unwrap();
        assert!(!tab.has_view());
        assert_eq!(tab.lifecycle, Lifecycle::Hibernated);
        assert_eq!(tab.title, "Kept title");
        assert_eq!(
            tab.url.as_ref().map(Url::as_str),
            Some("https://example.com/path")
        );
        assert!(!items.mark_view_discarded(id(1)));
        assert!(matches!(
            items.ensure_view(id(1)).as_slice(),
            [Effect::CreateView { url, .. }] if url == "https://example.com/path"
        ));
    }

    #[test]
    fn only_current_navigation_failure_clears_pending_intent() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        items.navigate(id(1), "first.example");
        let fx = items.navigate(id(1), "second.example");
        let Effect::Navigate { request, .. } = fx[0] else {
            panic!("existing view navigation expected");
        };

        assert!(!items.navigation_failed(id(1), NavigationRequestId(1)));
        assert_eq!(items.pending_navigation(id(1)), Some(request));
        assert!(items.navigation_failed(id(1), request));
        assert_eq!(items.pending_navigation(id(1)), None);
        assert!(items.tab(id(1)).unwrap().url.is_none());
    }

    #[test]
    fn insert_refuses_duplicate_and_bad_parent() {
        let mut items = Items::default();
        assert!(items.insert_tab(id(1), today()));
        assert!(!items.insert_tab(id(1), today()));

        // parent must exist and be a folder
        assert!(!items.insert(Item {
            id: id(2),
            parent: Some(id(1)),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
        assert!(!items.insert(Item {
            id: id(3),
            parent: Some(id(99)),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
    }

    #[test]
    fn remove_folder_removes_subtree_and_closes_views() {
        let mut items = Items::default();
        let f = folder(&mut items, 10);
        assert!(items.insert(Item {
            id: id(11),
            parent: Some(f),
            placement: today(),
            kind: ItemKind::Tab(TabState::new()),
        }));
        items.navigate(id(11), "example.com");
        assert!(items.insert_tab(id(12), today()));

        let fx = items.remove(f);
        assert_eq!(fx, vec![Effect::Close { id: id(11) }]);
        assert!(items.get(f).is_none());
        assert!(items.get(id(11)).is_none());
        assert_eq!(items.roots(today()), &[id(12)]);
    }

    fn check_invariants(items: &Items) {
        let mut seen = std::collections::HashSet::new();
        for (placement, list) in &items.roots {
            for rid in list {
                assert!(seen.insert(*rid), "id listed twice");
                let item = items.get(*rid).expect("root exists");
                assert_eq!(item.parent, None);
                assert_eq!(item.placement, *placement);
            }
        }
        for (parent, list) in &items.children {
            let p = items.get(*parent).expect("parent exists");
            assert!(matches!(p.kind, ItemKind::Folder { .. }));
            for cid in list {
                assert!(seen.insert(*cid), "id listed twice");
                let c = items.get(*cid).expect("child exists");
                assert_eq!(c.parent, Some(*parent));
                assert_eq!(c.placement, p.placement);
            }
        }
        assert_eq!(
            seen.len(),
            items.items.len(),
            "every item is listed exactly once"
        );
    }

    #[derive(Clone, Debug)]
    enum Op {
        InsertTab(u128),
        InsertFolder(u128),
        InsertChild(u128, usize),
        Remove(usize),
        Navigate(usize, String),
    }

    fn ops() -> impl Strategy<Value = Op> {
        prop_oneof![
            (1u128..200).prop_map(Op::InsertTab),
            (1u128..200).prop_map(Op::InsertFolder),
            ((1u128..200), any::<usize>()).prop_map(|(n, i)| Op::InsertChild(n, i)),
            any::<usize>().prop_map(Op::Remove),
            (any::<usize>(), "\\PC*").prop_map(|(i, s)| Op::Navigate(i, s)),
        ]
    }

    proptest! {
        #[test]
        fn invariants_hold_under_random_ops(seq in proptest::collection::vec(ops(), 0..60)) {
            let mut items = Items::default();
            let mut ids: Vec<ItemId> = Vec::new();
            let pick = |ids: &Vec<ItemId>, i: usize| ids.get(i % ids.len().max(1)).copied();
            for op in seq {
                match op {
                    Op::InsertTab(n) => {
                        if items.insert_tab(id(n), today()) {
                            ids.push(id(n));
                        }
                    }
                    Op::InsertFolder(n) => {
                        let item = Item {
                            id: id(n),
                            parent: None,
                            placement: today(),
                            kind: ItemKind::Folder { name: "F".into() },
                        };
                        if items.insert(item) {
                            ids.push(id(n));
                        }
                    }
                    Op::InsertChild(n, i) => {
                        if let Some(parent) = pick(&ids, i) {
                            let item = Item {
                                id: id(n),
                                parent: Some(parent),
                                placement: today(),
                                kind: ItemKind::Tab(TabState::new()),
                            };
                            if items.insert(item) {
                                ids.push(id(n));
                            }
                        }
                    }
                    Op::Remove(i) => {
                        if let Some(target) = pick(&ids, i) {
                            items.remove(target);
                            ids.retain(|x| items.get(*x).is_some());
                        }
                    }
                    Op::Navigate(i, s) => {
                        if let Some(target) = pick(&ids, i) {
                            items.navigate(target, &s);
                        }
                    }
                }
                check_invariants(&items);
            }
        }
    }
}
