//! Home: the space's untouched new tab while Home is on, the searches asked
//! from its field and the tabs opened from them. A search that reads as a map
//! stays on Home and its host reads the engine's results through a background
//! tab here; any other search loads its results page as before.

use super::*;

use crate::home::{
    HomeCommand, HomeMaps, HomePage, HomeRecords, MapKind, MapRefusal, MapStart, ResultsDocument,
    ResultsRefusal, ResultsReply,
};
use zephium_core::home::{
    clipped, HomeMode, MapId, Origin, Page, PageRow, Search, SearchId, SearchKind,
    MAX_PAGE_HOST_BYTES, MAX_PAGE_TITLE_BYTES, MAX_SEARCH_TEXT_BYTES,
};
use zephium_core::ports::engine::{DocumentReadId, PageHtml};

use super::tabs::unix_millis;

/// Results reads waiting at once; a further one is refused.
pub(super) const MAX_PENDING_READS: usize = 8;
/// Longest row id of a map page kept with the tab opened from it.
const MAX_ENTRY_BYTES: usize = 64;

#[derive(Default)]
pub(super) struct HomeState {
    maps: Option<Arc<dyn HomeMaps>>,
    pub(super) searches: zephium_core::home::Searches,
    origins: std::collections::HashMap<ItemId, Origin>,
    pub(super) reads: std::collections::HashMap<MapId, PendingRead>,
    /// Spaces left while their Home was in front.
    front: std::collections::HashSet<SpaceId>,
    next: u64,
}

pub(super) struct PendingRead {
    tab: ItemId,
    /// Set once the document is asked for; until then the page loads.
    read: Option<DocumentReadId>,
    done: ResultsReply,
}

impl HomeState {
    fn next(&mut self) -> u64 {
        self.next = self.next.wrapping_add(1).max(1);
        self.next
    }
}

/// What a run from Home's field asked for.
pub(super) enum HomeAsk<'a> {
    Search(&'a str),
    Address,
}

impl Shell {
    pub(super) fn home_on(&self) -> bool {
        self.home.maps.is_some() && self.tab_preferences.home_mode == HomeMode::Home
    }

    /// The space's Home: an untouched new tab of its open tabs while Home is
    /// on and a new tab shows the built-in page.
    pub(super) fn is_home_tab(&self, id: ItemId) -> bool {
        self.home_on()
            && self.is_untouched_new_tab(id)
            && self.items.get(id).is_some_and(|item| {
                matches!(
                    item.placement,
                    Placement::Space {
                        section: SpaceSection::Today,
                        ..
                    }
                ) && self
                    .profile_of_item(id)
                    .is_some_and(|profile| self.new_tab_is_home(profile))
            })
    }

    pub(super) fn new_tab_is_home(&self, profile: ProfileId) -> bool {
        self.incognito_profile(profile) || self.tab_preferences.new_page.is_none()
    }

    /// The Home already open in the focused space, to show instead of a
    /// second one.
    pub(super) fn open_home_tab(&self) -> Option<ItemId> {
        let window = self.windows.focused()?;
        if !self.home_on() || !self.new_tab_is_home(window.profile) {
            return None;
        }
        self.regular_tabs(window.space)
            .into_iter()
            .find(|id| self.is_home_tab(*id))
    }

    /// Closing Home does nothing while the space has another tab to show.
    pub(super) fn home_stays(&self, id: ItemId) -> bool {
        self.windows.focused().is_some_and(|window| {
            self.is_home_tab(id)
                && self
                    .successor_pool(window.profile, window.space)
                    .iter()
                    .any(|other| *other != id)
        })
    }

    pub(super) fn home_tab_search(&self, id: ItemId) -> Option<zephium_ipc::TabSearchView> {
        let origin = self.home.origins.get(&id)?;
        let search = self.home.searches.get(origin.search)?;
        Some(zephium_ipc::TabSearchView {
            id: search.id.to_string(),
            text: search.text.clone(),
            results: search.results == Some(id),
        })
    }

    pub(super) fn home_reading_tabs(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.home.reads.values().map(|read| read.tab)
    }

    pub(super) fn home_command(&mut self, command: HomeCommand) {
        match command {
            HomeCommand::Attach(attachment) => {
                self.home.maps = Some(attachment.0);
                self.home_keep_one_per_space();
                self.project_items();
            }
            HomeCommand::Records { done } => done.finish(self.home_records()),
            HomeCommand::ReadResults { map, url, done } => self.home_read_results(map, url, done),
            HomeCommand::ReadAgain { map, done } => self.home_read_again(map, done),
            HomeCommand::ReadAbandoned { map } => {
                self.home.reads.remove(&map);
            }
            intent => {
                let _ = self.home_intent(intent);
            }
        }
    }

    pub(super) fn home_intent(&mut self, command: HomeCommand) -> OperationDisposition {
        if !self.home_on() {
            return operation_result(OperationOutcome::Rejected, OperationReason::HomeUnavailable);
        }
        let search = match &command {
            HomeCommand::OpenPage { search, .. }
            | HomeCommand::OpenResults { search }
            | HomeCommand::FocusSearch { search }
            | HomeCommand::ForgetSearch { search } => *search,
            _ => {
                return operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::UnsupportedCommand,
                )
            }
        };
        if !self.home_search_in_focus(search) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::HomeSearchUnknown,
            );
        }
        match command {
            HomeCommand::OpenPage {
                search,
                slot,
                entry,
                url,
            } => self.home_open_page(search, slot, entry, url),
            HomeCommand::OpenResults { search } => self.home_open_results(search),
            HomeCommand::FocusSearch { search } => self.home_focus_search(search),
            HomeCommand::ForgetSearch { search } => {
                self.home_forget(search);
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            _ => operation_result(
                OperationOutcome::Rejected,
                OperationReason::UnsupportedCommand,
            ),
        }
    }

    fn home_search_in_focus(&self, id: SearchId) -> bool {
        self.windows.focused().is_some_and(|window| {
            self.home.searches.get(id).is_some_and(|search| {
                search.profile == window.profile && search.space == window.space
            })
        })
    }

    fn home_changed(&self, profile: ProfileId, space: SpaceId) {
        (self.emit)(Projection::HomeChanged {
            profile: profile.to_string(),
            space: space.to_string(),
        });
    }

    fn home_show(&self, search: SearchId, row: Option<PageRow>) {
        let (slot, entry) = row.map_or((None, None), |row| (Some(row.slot), Some(row.entry)));
        (self.emit)(Projection::HomeShow {
            search: search.to_string(),
            slot,
            entry,
        });
    }

    pub(super) fn home_records(&self) -> Option<HomeRecords> {
        let window = self.windows.focused()?;
        if !self.home_on() {
            return None;
        }
        let searches: Vec<Search> = self
            .home
            .searches
            .in_scope(window.profile, window.space)
            .rev()
            .cloned()
            .collect();
        let pages = searches
            .iter()
            .map(|search| {
                search
                    .pages
                    .iter()
                    .map(|page| self.home_page(page))
                    .collect()
            })
            .collect();
        Some(HomeRecords {
            profile: window.profile,
            space: window.space,
            searches,
            pages,
        })
    }

    fn home_page(&self, page: &Page) -> HomePage {
        let tab = self.items.tab(page.tab).filter(|_| page.open);
        let (title, host) = tab.map_or_else(
            || (page.title.clone(), page.host.clone()),
            |tab| {
                (
                    clipped(&tab.title, MAX_PAGE_TITLE_BYTES),
                    tab.url.as_ref().and_then(|url| url.host_str()).map_or_else(
                        || page.host.clone(),
                        |host| clipped(host, MAX_PAGE_HOST_BYTES),
                    ),
                )
            },
        );
        HomePage {
            tab: tab.is_some().then_some(page.tab),
            row: page.row.clone(),
            open: tab.is_some(),
            title,
            host,
        }
    }

    /// A run from Home's field that may stay on Home as a map. `None` lets
    /// the run load its page as before; `home_ran` then records it.
    pub(super) fn home_try_map(
        &mut self,
        context: &zephium_ipc::SearchContext,
        query: &str,
        url: &str,
    ) -> Option<OperationDisposition> {
        let maps = self.home.maps.clone().filter(|_| self.home_on())?;
        if context.plain() {
            return None;
        }
        let window = self.windows.focused()?;
        let (window_id, profile, space) = (window.id, window.profile, window.space);
        let engine = self.search.engine;
        let query = query.trim();
        if query.is_empty()
            || query.len() > MAX_SEARCH_TEXT_BYTES
            || engine
                .search(query)
                .is_none_or(|plain| plain.as_str() != url)
        {
            return None;
        }
        if let Some(same) = self.home.searches.same(profile, space, query).filter(|id| {
            self.home
                .searches
                .get(*id)
                .and_then(|search| search.map)
                .is_some_and(|map| maps.holds(map))
        }) {
            self.home.searches.touch(same, unix_millis().unwrap_or(0));
            self.home_changed(profile, space);
            self.home_show(same, None);
            return Some(operation_result(
                OperationOutcome::Applied,
                OperationReason::HomeMapStarted,
            ));
        }
        let search = self.home_record(profile, space, query, SearchKind::Other);
        let map = MapId(self.home.next());
        let started = maps.start(MapStart {
            map,
            search,
            window: window_id,
            profile,
            private: self.incognito_profile(profile),
            engine,
            query: query.to_owned(),
        });
        let kind = match started {
            Ok(MapKind::Place) => SearchKind::Place,
            Ok(MapKind::Compare) => SearchKind::Compare,
            Err(MapRefusal::NotAMap | MapRefusal::Busy | MapRefusal::Unavailable) => return None,
        };
        let gone = self.home.searches.get_mut(search).and_then(|record| {
            record.kind = kind;
            record.map.replace(map)
        });
        if let Some(old) = gone {
            if let Some(read) = self.home.reads.remove(&old) {
                read.done.finish(Err(ResultsRefusal::Unknown));
            }
            maps.forget(old);
        }
        self.home_changed(profile, space);
        self.home_show(search, None);
        Some(operation_result(
            OperationOutcome::Applied,
            OperationReason::HomeMapStarted,
        ))
    }

    /// Records what a run from Home's field opened in `tab`, the Home it
    /// was run from.
    pub(super) fn home_ran(&mut self, tab: ItemId, ask: HomeAsk<'_>, url: &str) {
        if !self.home_on() {
            return;
        }
        let Some((profile, space)) = self.windows.focused().map(|w| (w.profile, w.space)) else {
            return;
        };
        let Ok(parsed) = url::Url::parse(url) else {
            return;
        };
        let search = match ask {
            HomeAsk::Search(query) => {
                let query = query.trim();
                if query.is_empty() {
                    return;
                }
                let id = self.home_record(profile, space, query, SearchKind::Other);
                let seen = self.home_in_front(tab);
                if let Some(record) = self.home.searches.get_mut(id) {
                    record.results = Some(tab);
                    record.results_url = Some(parsed.to_string());
                    record.results_seen |= seen;
                }
                id
            }
            HomeAsk::Address => {
                let Some(host) = parsed
                    .host_str()
                    .map(|host| clipped(host, MAX_PAGE_HOST_BYTES))
                else {
                    return;
                };
                let id = self.home_record(profile, space, &host, SearchKind::Address);
                if let Some(record) = self.home.searches.get_mut(id) {
                    record.keep_page(Page {
                        tab,
                        row: None,
                        open: true,
                        title: String::new(),
                        host,
                    });
                }
                id
            }
        };
        self.home.origins.insert(
            tab,
            Origin {
                search,
                opener: None,
            },
        );
        self.project_tab(tab);
        self.home_changed(profile, space);
    }

    /// The search asked with `text` in this space: the one already there,
    /// made the newest, or a new one.
    fn home_record(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        text: &str,
        kind: SearchKind,
    ) -> SearchId {
        let at_ms = unix_millis().unwrap_or(0);
        if let Some(same) = self.home.searches.same(profile, space, text) {
            self.home.searches.touch(same, at_ms);
            return same;
        }
        let idle: Vec<SearchId> = self
            .home
            .searches
            .in_scope(profile, space)
            .filter(|search| search.pages.is_empty() && !self.home_results_seen(search))
            .map(|search| search.id)
            .collect();
        for gone in idle {
            if let Some(search) = self.home.searches.forget(gone) {
                self.home_let_go(search, true);
            }
        }
        let id = SearchId(self.home.next());
        let evicted = self.home.searches.record(Search {
            id,
            text: clipped(text, MAX_SEARCH_TEXT_BYTES),
            at_ms,
            kind,
            engine: self.search.engine,
            profile,
            space,
            map: None,
            results: None,
            results_url: None,
            results_seen: false,
            pages: Vec::new(),
        });
        for gone in evicted {
            self.home_let_go(gone, true);
        }
        id
    }

    fn home_forget(&mut self, id: SearchId) {
        if let Some(search) = self.home.searches.forget(id) {
            let (profile, space) = (search.profile, search.space);
            self.home_let_go(search, true);
            self.home_changed(profile, space);
        }
    }

    /// The tab is shown in a window: in front, in a split or in the pane.
    fn home_in_front(&self, tab: ItemId) -> bool {
        self.windows.iter().any(|window| {
            window.active == Some(tab) || window.splits.as_ref().is_some_and(|t| t.contains(tab))
        }) || self.work_pane.as_ref().is_some_and(|pane| pane.tab == tab)
    }

    fn home_results_seen(&self, search: &Search) -> bool {
        search.results_seen || search.results.is_some_and(|tab| self.home_in_front(tab))
    }

    /// A tab came to the front: a search's results tab has now been seen.
    pub(super) fn home_fronted(&mut self, tab: ItemId) {
        for search in self.home.searches.iter_mut() {
            if search.results == Some(tab) {
                search.results_seen = true;
            }
        }
    }

    /// Closes a tab of Home's own without choosing a successor or leaving it
    /// in Recently Closed.
    fn home_close_quietly(&mut self, tab: ItemId) {
        self.forget_content_fullscreen(tab);
        self.forget_tab_media(tab);
        if self.work_pane.as_ref().is_some_and(|pane| pane.tab == tab) {
            self.clear_work_pane();
        }
        self.remove_tab_quietly(tab);
    }

    /// Lets go of a search no longer kept: its map ends and the tabs opened
    /// from it stay open, no longer naming it. With `close_unseen`, a results
    /// tab that was never in front closes with it.
    fn home_let_go(&mut self, search: Search, close_unseen: bool) {
        if let Some(tab) = search.results.filter(|tab| {
            close_unseen && self.items.tab(*tab).is_some() && !self.home_results_seen(&search)
        }) {
            self.home_close_quietly(tab);
        }
        if let Some(map) = search.map {
            if let Some(read) = self.home.reads.remove(&map) {
                read.done.finish(Err(ResultsRefusal::Unknown));
            }
            if let Some(maps) = &self.home.maps {
                maps.forget(map);
            }
        }
        let tabs: Vec<ItemId> = self
            .home
            .origins
            .iter()
            .filter(|(_, origin)| origin.search == search.id)
            .map(|(tab, _)| *tab)
            .collect();
        for tab in tabs {
            self.home.origins.remove(&tab);
            self.project_tab(tab);
        }
    }

    pub(super) fn home_forget_profile(&mut self, profile: ProfileId) {
        for search in self
            .home
            .searches
            .forget_where(|search| search.profile == profile)
        {
            self.home_let_go(search, false);
        }
        if let Some(maps) = &self.home.maps {
            maps.forget_profile(profile);
        }
    }

    /// A tab went away: a read waiting on it ends, and the searches keep
    /// what it last showed.
    pub(super) fn home_tab_removed(&mut self, id: ItemId) {
        let reading: Vec<MapId> = self
            .home
            .reads
            .iter()
            .filter(|(_, read)| read.tab == id)
            .map(|(map, _)| *map)
            .collect();
        for map in reading {
            if let Some(read) = self.home.reads.remove(&map) {
                read.done.finish(Err(ResultsRefusal::Unavailable));
            }
        }
        self.home.origins.remove(&id);
        self.home.origins.retain(|_, origin| {
            if origin.opener == Some(id) {
                origin.opener = None;
            }
            true
        });
        let (title, host) = self.items.tab(id).map_or_else(Default::default, |tab| {
            (
                clipped(&tab.title, MAX_PAGE_TITLE_BYTES),
                tab.url
                    .as_ref()
                    .and_then(|url| url.host_str())
                    .map(|host| clipped(host, MAX_PAGE_HOST_BYTES))
                    .unwrap_or_default(),
            )
        });
        let mut changed = None;
        for search in self.home.searches.iter_mut() {
            if search.results == Some(id) {
                search.results = None;
                changed = Some((search.profile, search.space));
            }
            if let Some(page) = search.page_mut(id) {
                page.open = false;
                if !title.is_empty() {
                    page.title.clone_from(&title);
                }
                if !host.is_empty() {
                    page.host.clone_from(&host);
                }
                changed = Some((search.profile, search.space));
            }
        }
        if let Some((profile, space)) = changed {
            self.home_changed(profile, space);
        }
    }

    /// Who takes over from a closed tab opened from Home: its opener while
    /// open, else Home, shown at its search.
    pub(super) fn home_take_over(
        &mut self,
        tab: ItemId,
        closed: Option<Origin>,
        profile: ProfileId,
        space: SpaceId,
    ) -> Option<Vec<Effect>> {
        let origin = closed.filter(|_| self.home_on())?;
        if let Some(opener) = origin
            .opener
            .filter(|opener| self.item_in_scope(*opener, profile, space))
            .filter(|opener| !self.is_home_tab(*opener))
        {
            return Some(self.focus_tab(opener));
        }
        let effects = self.show_home()?;
        if let Some(search) = self.home.searches.get(origin.search) {
            let row = search.page(tab).and_then(|page| page.row.clone());
            self.home_show(origin.search, row);
        }
        Some(effects)
    }

    pub(super) fn home_origin(&self, id: ItemId) -> Option<Origin> {
        self.home.origins.get(&id).copied()
    }

    fn show_home(&mut self) -> Option<Vec<Effect>> {
        match self.open_home_tab() {
            Some(home) => Some(self.focus_tab(home)),
            None => self.open_new_tab_with_id().map(|(_, effects)| effects),
        }
    }

    /// Each space keeps one untouched tab while Home is on: the one in front
    /// if it is among them, else the first.
    pub(super) fn home_keep_one_per_space(&mut self) {
        if !self.home_on() {
            return;
        }
        let spaces: Vec<SpaceId> = self.spaces.iter().map(|space| space.id).collect();
        for space in spaces {
            let homes: Vec<ItemId> = self
                .regular_tabs(space)
                .into_iter()
                .filter(|id| self.is_home_tab(*id))
                .collect();
            if homes.len() < 2 {
                continue;
            }
            let shown = self
                .windows
                .iter()
                .find(|window| window.space == space)
                .and_then(|window| window.active);
            let keep = shown.filter(|id| homes.contains(id)).unwrap_or(homes[0]);
            for id in homes.into_iter().filter(|id| *id != keep) {
                self.home_close_quietly(id);
            }
        }
    }

    pub(super) fn home_left_space(&mut self, space: SpaceId, home_in_front: bool) {
        if home_in_front && self.home_on() {
            self.home.front.insert(space);
        } else {
            self.home.front.remove(&space);
        }
    }

    pub(super) fn home_returns_to(&mut self, profile: ProfileId, space: SpaceId) -> bool {
        self.home.front.remove(&space) && self.home_on() && self.new_tab_is_home(profile)
    }

    /// A page opened from a tab opened from Home came from the same search.
    pub(super) fn home_link_opened(&mut self, source: ItemId, tab: ItemId) {
        let Some(origin) = self.home.origins.get(&source).copied() else {
            return;
        };
        self.home.origins.insert(
            tab,
            Origin {
                search: origin.search,
                opener: Some(source),
            },
        );
        self.home_place_after_run(tab, origin.search);
        self.project_tab(tab);
    }

    /// The tab rows opened from one search stay together: one dragged away
    /// from them no longer names it; one dropped among them moves past them.
    pub(super) fn home_tab_moved(&mut self, id: ItemId) {
        let Some(item) = self.items.get(id) else {
            return;
        };
        let placement = item.placement;
        let Placement::Space {
            section: SpaceSection::Today,
            ..
        } = placement
        else {
            if self.home.origins.remove(&id).is_some() {
                self.project_tab(id);
            }
            return;
        };
        let roots = self.items.roots(placement);
        let Some(at) = roots.iter().position(|root| *root == id) else {
            return;
        };
        let search_of = |tab: Option<&ItemId>| {
            tab.and_then(|tab| self.home.origins.get(tab))
                .map(|origin| origin.search)
        };
        let before = search_of(at.checked_sub(1).and_then(|at| roots.get(at)));
        let after = search_of(roots.get(at + 1));
        match self.home.origins.get(&id).map(|origin| origin.search) {
            Some(own) => {
                if before != Some(own) && after != Some(own) && self.home_run_len(own) > 1 {
                    self.home.origins.remove(&id);
                    self.project_tab(id);
                }
            }
            None => {
                if let Some(run) = before.filter(|before| Some(*before) == after) {
                    self.home_place_after_run(id, run);
                }
            }
        }
    }

    fn home_run_len(&self, search: SearchId) -> usize {
        self.home
            .origins
            .values()
            .filter(|origin| origin.search == search)
            .count()
    }

    /// Puts `tab` right after the last Today row opened from `search`.
    fn home_place_after_run(&mut self, tab: ItemId, search: SearchId) {
        let Some(placement) = self.items.get(tab).map(|item| item.placement) else {
            return;
        };
        let roots = self.items.roots(placement);
        let Some(last) = roots.iter().rposition(|root| {
            *root != tab
                && self
                    .home
                    .origins
                    .get(root)
                    .is_some_and(|origin| origin.search == search)
        }) else {
            return;
        };
        let before = roots[last + 1..].iter().copied().find(|next| *next != tab);
        self.items.move_tab_to_root(tab, placement, before);
    }

    /// A tab of the search's space for a page of the search; private
    /// sessions too, as Home is theirs as well.
    fn home_open_tab(&mut self, search: SearchId, url: &url::Url, focus: bool) -> Option<ItemId> {
        let (profile, space) = self
            .home
            .searches
            .get(search)
            .map(|search| (search.profile, search.space))?;
        if url.scheme() != "https"
            || !navigation::is_allowed(url)
            || self.profile_deletion_quarantines(profile)
            || self
                .spaces
                .get(space)
                .is_none_or(|owner| owner.profile != profile)
        {
            return None;
        }
        let opener = self
            .windows
            .focused()
            .filter(|window| window.profile == profile && window.space == space)
            .and_then(|window| window.active);
        let today = Placement::Space {
            space,
            section: SpaceSection::Today,
        };
        let id = ItemId::generate();
        if !self.items.insert_tab(id, today) {
            return None;
        }
        self.home.origins.insert(id, Origin { search, opener });
        self.home_place_after_run(id, search);
        let mut effects = if focus {
            self.focus_tab(id)
        } else {
            Vec::new()
        };
        effects.extend(self.items.navigate(id, url.as_str()));
        let native = self.commit(effects);
        if native.rejected
            && self.items.tab(id).is_some_and(|tab| !tab.has_view())
            && self.capacity_presentation(id).is_none()
        {
            self.close_in_any_space(id);
            return None;
        }
        Some(id)
    }

    fn home_open_page(
        &mut self,
        search: SearchId,
        slot: u8,
        entry: String,
        url: Option<url::Url>,
    ) -> OperationDisposition {
        if entry.is_empty() || entry.len() > MAX_ENTRY_BYTES {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        let Some((url, tab)) = url.and_then(|url| {
            let tab = self.home_open_tab(search, &url, true)?;
            Some((url, tab))
        }) else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::HomeTargetUnavailable,
            );
        };
        let host = url
            .host_str()
            .map(|host| clipped(host, MAX_PAGE_HOST_BYTES))
            .unwrap_or_default();
        if let Some(record) = self.home.searches.get_mut(search) {
            record.keep_page(Page {
                tab,
                row: Some(PageRow { slot, entry }),
                open: true,
                title: String::new(),
                host,
            });
            let (profile, space) = (record.profile, record.space);
            self.home_changed(profile, space);
        }
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    fn home_open_results(&mut self, search: SearchId) -> OperationDisposition {
        let Some(record) = self.home.searches.get(search) else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::HomeSearchUnknown,
            );
        };
        if let Some(tab) = record
            .results
            .filter(|tab| self.item_in_focused_scope(*tab))
        {
            return self.operation_activate(tab);
        }
        let url = record
            .results_url
            .as_deref()
            .and_then(|url| url::Url::parse(url).ok())
            .filter(|url| record.engine.shows_results_for(url, &record.text))
            .or_else(|| record.engine.search(&record.text));
        let Some(url) = url else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::HomeTargetUnavailable,
            );
        };
        let Some(tab) = self.home_open_tab(search, &url, true) else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        if let Some(record) = self.home.searches.get_mut(search) {
            record.results = Some(tab);
            record.results_url = Some(url.to_string());
            record.results_seen = true;
            let (profile, space) = (record.profile, record.space);
            self.home_changed(profile, space);
        }
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    fn home_focus_search(&mut self, search: SearchId) -> OperationDisposition {
        let Some(effects) = self.show_home() else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        let native = self.commit(effects);
        if let Some((profile, space)) = self.windows.focused().map(|w| (w.profile, w.space)) {
            self.home_changed(profile, space);
        }
        self.home_show(search, None);
        mutation_result(native)
    }

    /// The page `tab` shows is the results page of the search `map` belongs
    /// to: its engine's results address, asking that search's words.
    fn home_shows_results(&self, map: MapId, url: &url::Url) -> bool {
        self.home
            .searches
            .by_map(map)
            .is_some_and(|search| search.engine.shows_results_for(url, &search.text))
    }

    fn home_read_results(&mut self, map: MapId, url: url::Url, done: ResultsReply) {
        let Some(search) = self.home.searches.by_map(map) else {
            return done.finish(Err(ResultsRefusal::Unknown));
        };
        if search
            .engine
            .search(&search.text)
            .is_none_or(|expected| expected != url)
        {
            return done.finish(Err(ResultsRefusal::Unknown));
        }
        let (id, results) = (search.id, search.results);
        if let Some(old) = self.home.reads.remove(&map) {
            old.done.finish(Err(ResultsRefusal::Unavailable));
        }
        if self.home.reads.len() >= MAX_PENDING_READS {
            return done.finish(Err(ResultsRefusal::Unavailable));
        }
        let open = results.filter(|tab| self.items.tab(*tab).is_some());
        let tab = match open {
            Some(tab) => {
                let showing = self.items.tab(tab).is_some_and(|state| {
                    state.has_view()
                        && state
                            .url
                            .as_ref()
                            .is_some_and(|shown| self.home_shows_results(map, shown))
                });
                if !showing {
                    let effects = self.items.navigate(tab, url.as_str());
                    if self.commit(effects).rejected {
                        return done.finish(Err(ResultsRefusal::Unavailable));
                    }
                }
                tab
            }
            None => {
                let Some(tab) = self.home_open_tab(id, &url, false) else {
                    return done.finish(Err(ResultsRefusal::Unavailable));
                };
                if let Some(record) = self.home.searches.get_mut(id) {
                    record.results = Some(tab);
                    record.results_url = Some(url.to_string());
                }
                tab
            }
        };
        if self.capacity_presentation(tab).is_some() {
            return done.finish(Err(ResultsRefusal::Unavailable));
        }
        self.home.reads.insert(
            map,
            PendingRead {
                tab,
                read: None,
                done,
            },
        );
        let ready = self.items.tab(tab).is_some_and(|state| {
            !state.loading && self.items.pending_navigation_request(tab).is_none()
        });
        if ready {
            self.home_read_document(map);
        }
    }

    fn home_read_again(&mut self, map: MapId, done: ResultsReply) {
        let Some(tab) = self
            .home
            .searches
            .by_map(map)
            .and_then(|search| search.results)
            .filter(|tab| {
                self.items
                    .tab(*tab)
                    .is_some_and(|tab| tab.has_view() && !tab.loading)
            })
        else {
            return done.finish(Err(ResultsRefusal::Unavailable));
        };
        if let Some(old) = self.home.reads.remove(&map) {
            old.done.finish(Err(ResultsRefusal::Unavailable));
        }
        self.home.reads.insert(
            map,
            PendingRead {
                tab,
                read: None,
                done,
            },
        );
        self.home_read_document(map);
    }

    /// Asks the engine for the document only while the tab shows this
    /// search's results; a consent, sign-in or other page is never read.
    fn home_read_document(&mut self, map: MapId) {
        let Some(tab) = self.home.reads.get(&map).map(|pending| pending.tab) else {
            return;
        };
        let shown = self.items.tab(tab).and_then(|state| state.url.clone());
        if !shown.is_some_and(|shown| self.home_shows_results(map, &shown)) {
            if let Some(pending) = self.home.reads.remove(&map) {
                pending.done.finish(Err(ResultsRefusal::Redirected));
            }
            return;
        }
        let read = DocumentReadId(self.home.next());
        if let Some(pending) = self.home.reads.get_mut(&map) {
            pending.read = Some(read);
        }
        if self.engine.read_document(tab, read) != NativeDispatch::Scheduled {
            if let Some(pending) = self.home.reads.remove(&map) {
                pending.done.finish(Err(ResultsRefusal::Unavailable));
            }
        }
    }

    pub(super) fn home_results_loaded(&mut self, id: ItemId) {
        let waiting: Vec<MapId> = self
            .home
            .reads
            .iter()
            .filter(|(_, read)| read.tab == id && read.read.is_none())
            .map(|(map, _)| *map)
            .collect();
        if waiting.is_empty() || self.items.tab(id).is_none_or(|tab| tab.url.is_none()) {
            return;
        }
        for map in waiting {
            self.home_read_document(map);
        }
    }

    pub(super) fn home_results_failed(&mut self, id: ItemId) {
        let failed: Vec<MapId> = self
            .home
            .reads
            .iter()
            .filter(|(_, read)| read.tab == id)
            .map(|(map, _)| *map)
            .collect();
        for map in failed {
            if let Some(read) = self.home.reads.remove(&map) {
                read.done.finish(Err(ResultsRefusal::Unavailable));
            }
        }
    }

    pub(super) fn on_document_read(
        &mut self,
        id: ItemId,
        read: DocumentReadId,
        url: String,
        html: PageHtml,
        truncated: bool,
    ) {
        let Some(map) = self
            .home
            .reads
            .iter()
            .find(|(_, pending)| pending.tab == id && pending.read == Some(read))
            .map(|(map, _)| *map)
        else {
            return;
        };
        let Some(pending) = self.home.reads.remove(&map) else {
            return;
        };
        if self.home.searches.by_map(map).is_none() {
            return pending.done.finish(Err(ResultsRefusal::Unknown));
        }
        let answer = match url::Url::parse(&url) {
            Ok(url) if self.home_shows_results(map, &url) => Ok(ResultsDocument {
                url,
                html,
                truncated,
            }),
            _ => Err(ResultsRefusal::Redirected),
        };
        pending.done.finish(answer);
    }
}
