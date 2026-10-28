//! Bounded launcher search and asynchronous history reconciliation.

use super::*;
use crate::store_reads::{BookmarkHit, Sources};
use zephium_core::search::{
    capacity_group, interleave_bookmarks, kind_capacity, match_quality, result_section,
    scoped_query, SearchScope, MAX_RESULTS,
};

const MAX_ASYNC_SEARCH_QUERY_BYTES: usize = 4 * 1024;

pub(super) struct PendingSearch {
    pub(super) generation: u64,
    pub(super) profile: ProfileId,
    pub(super) lookup_query: String,
    pub(super) display_query: String,
    pub(super) open_urls: std::collections::HashSet<String>,
    pub(super) base_results: Vec<SearchResult>,
}

#[derive(Default)]
pub(super) struct SearchState {
    pub(super) supplementary_pending: bool,
    pub(super) query: String,
    pub(super) custom_url: String,
    pub(super) engine: zephium_core::search::SearchEngine,
    /// Pages visited before are offered as places to go. An explicit
    /// history search still reads them.
    pub(super) include_history: bool,
    pub(super) context: Option<zephium_ipc::SearchContext>,
    pub(super) results: Vec<SearchResult>,
    pub(super) pending: Option<PendingSearch>,
    pub(super) generation: u64,
}

impl Shell {
    fn publish_search(&mut self, mut results: SearchResults) {
        let mut identities = std::collections::HashSet::new();
        results.results.retain(|result| {
            identities.insert(match &result.action {
                SearchAction::OpenNote { id } => format!("note:{id}"),
                SearchAction::ActivateTab { id } => format!("tab:{id}"),
                SearchAction::OpenUrl { url } => format!("url:{url}"),
                SearchAction::RunCommand { id } => format!("command:{id}"),
            })
        });
        // Sections carry a fixed presentation order and each result is already
        // pushed in its section's own preference order, so a stable sort by
        // section alone settles the list. A late note or suggestion therefore
        // extends its own section instead of re-ranking rows the user is
        // already reading. Within Search the typed action is pushed first and
        // stays first; that is the action Enter runs.
        results
            .results
            .sort_by_key(|result| result_section(&result.kind));
        let mut taken: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        results.results.retain(|result| {
            let group = capacity_group(&result.kind);
            let count = taken.entry(group.to_owned()).or_default();
            *count += 1;
            *count <= kind_capacity(group)
        });
        results.results.truncate(MAX_RESULTS);
        results.completion = self.search_completion(&results.query, &results.results);
        results.pending = self.search.pending.is_some() || self.search.supplementary_pending;
        self.search.results = results.results.clone();
        self.publish_icons();
        (self.emit)(Projection::Search(results));
    }

    /// Offered only from destinations the user has actually reached: an open
    /// tab or a visited address. Nothing a remote provider supplied can steer
    /// what the field types on the user's behalf.
    fn search_completion(&self, query: &str, results: &[SearchResult]) -> Option<String> {
        let (scope, typed) = scoped_query(query);
        if scope != SearchScope::All {
            return None;
        }
        results
            .iter()
            .filter(|result| matches!(result.kind.as_str(), "tab" | "history"))
            .find_map(|result| match &result.action {
                SearchAction::ActivateTab { .. } => zephium_core::search::host_completion(
                    typed,
                    &format!("https://{}", result.detail),
                ),
                SearchAction::OpenUrl { url } => zephium_core::search::host_completion(typed, url),
                _ => None,
            })
    }

    pub(super) fn search_supplementary_finished(
        &mut self,
        context: zephium_ipc::SearchContext,
        query: String,
    ) {
        if self.search.context.as_ref() != Some(&context)
            || self.search.query != query
            || !self.search_context_current(&context)
        {
            return;
        }
        self.search.supplementary_pending = false;
        self.publish_search(SearchResults {
            pending: false,
            completion: None,
            context: Some(context),
            query,
            results: self.search.results.clone(),
        });
    }

    pub(super) fn search_additional(
        &mut self,
        context: zephium_ipc::SearchContext,
        query: String,
        results: Vec<SearchResult>,
    ) {
        if self.search.context.as_ref() != Some(&context)
            || self.search.query != query
            || !self.search_context_current(&context)
        {
            return;
        }
        let results: Vec<_> = results
            .into_iter()
            .take(6)
            .filter(|result| {
                result.title.len() <= 1024
                    && result.detail.len() <= 1024
                    && result.icon.is_none()
                    && match &result.action {
                        SearchAction::OpenNote { id } => {
                            result.kind == "note" && zephium_core::resources::valid_id(id)
                        }
                        SearchAction::OpenUrl { url } => {
                            result.kind == "suggestion" && navigation::is_allowed_str(url)
                        }
                        _ => false,
                    }
            })
            .collect();
        if let Some(pending) = &mut self.search.pending {
            pending.base_results.extend(results.clone());
        }
        let mut combined = self.search.results.clone();
        combined.extend(results);
        self.publish_search(SearchResults {
            pending: false,
            completion: None,
            context: Some(context),
            query,
            results: combined,
        });
    }

    pub(super) fn search_scoped(&mut self, query: &str, context: zephium_ipc::SearchContext) {
        if !self.search_context_current(&context) {
            return;
        }
        // A launcher panel is created fresh each time it opens, so a new
        // session id means its raster cache is empty again.
        let reopened = !context.session_id.starts_with("newtab:")
            && self
                .search
                .context
                .as_ref()
                .is_none_or(|current| current.session_id != context.session_id);
        if reopened {
            self.forget_delivered_icons(zephium_ipc::IconSurface::Panel);
        }
        self.search.supplementary_pending = !query.trim().is_empty();
        self.search.query = query.to_owned();
        self.search.context = Some(context);
        self.search(query);
    }

    /// Search results reach New Tab through chrome and the launcher through
    /// the panel, so their icons must be delivered to the same webview.
    fn search_surface(&self) -> zephium_ipc::IconSurface {
        if self.search_is_newtab() {
            zephium_ipc::IconSurface::Chrome
        } else {
            zephium_ipc::IconSurface::Panel
        }
    }

    /// The bound surface is New Tab rather than the floating launcher.
    fn search_is_newtab(&self) -> bool {
        self.search
            .context
            .as_ref()
            .is_some_and(|context| context.session_id.starts_with("newtab:"))
    }

    fn search_context_current(&self, context: &zephium_ipc::SearchContext) -> bool {
        self.windows.focused().is_some_and(|window| {
            if let Some(tab) = context.session_id.strip_prefix("newtab:") {
                let Some(id) = tab.split_once(':').and_then(|(id, _)| ItemId::parse(id)) else {
                    return false;
                };
                if window.active != Some(id)
                    || self.items.tab(id).is_none_or(|tab| tab.url.is_some())
                    || self.active_browser_page().is_some()
                {
                    return false;
                }
            }
            window.id.to_string() == context.window_id
                && window.profile.to_string() == context.profile_id
                && window.space.to_string() == context.space_id
        })
    }

    pub(super) fn cancel_scoped_search(&mut self, session_id: &str) {
        if self
            .search
            .context
            .as_ref()
            .is_some_and(|context| context.session_id == session_id)
        {
            self.search.pending = None;
            self.search.supplementary_pending = false;
            self.search.context = None;
            self.search.results.clear();
        }
    }

    pub(super) fn operation_run_search_action(
        &mut self,
        context: zephium_ipc::SearchContext,
        action: SearchAction,
        background: bool,
        replace: Option<String>,
    ) -> OperationDisposition {
        if !self.search_context_current(&context)
            || self.search.context.as_ref() != Some(&context)
            || !self
                .search
                .results
                .iter()
                .any(|result| result.action == action)
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if let SearchAction::OpenUrl { url } = &action {
            if let Some(result) = self
                .search
                .results
                .iter()
                .find(|result| result.action == action)
            {
                let query = if result.kind == "suggestion" {
                    Some(result.title.as_str())
                } else if result.kind == "search" {
                    Some(
                        zephium_core::search::engine_shortcut(&self.search.query)
                            .map_or(self.search.query.as_str(), |(_, query)| query),
                    )
                } else {
                    None
                };
                if let Some(query) = query {
                    if let Some(profile) = ProfileId::parse(&context.profile_id) {
                        if self.profiles.get(profile).is_some_and(|profile| {
                            profile.kind != zephium_core::profiles::ProfileKind::Incognito
                        }) {
                            self.store
                                .record_search(profile, query.to_owned(), url.clone());
                        }
                    }
                }
            }
        }
        let newtab = context.session_id.starts_with("newtab:");
        if background && !newtab {
            if let SearchAction::OpenUrl { url } = action {
                // The search stays alive: the launcher remains open on these
                // results, and the next row the user sends must still admit.
                return self.operation_open_url_background(url);
            }
        }
        self.cancel_scoped_search(&context.session_id);
        match action {
            SearchAction::OpenNote { id } => {
                (self.emit)(Projection::OpenNote {
                    profile: context.profile_id,
                    id,
                });
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            SearchAction::ActivateTab { id } => ItemId::parse(&id).map_or_else(
                || operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput),
                |id| self.operation_activate(id),
            ),
            SearchAction::OpenUrl { url } => {
                if let Some(id) = context
                    .session_id
                    .strip_prefix("newtab:")
                    .and_then(|value| value.split_once(':'))
                    .and_then(|(id, _)| ItemId::parse(id))
                {
                    self.operation_navigate(id, url)
                } else if let Some(id) = replace
                    .as_deref()
                    .and_then(ItemId::parse)
                    .filter(|id| self.windows.focused().and_then(|w| w.active) == Some(*id))
                {
                    // The launcher stood in for the page's own address field.
                    self.operation_navigate(id, url)
                } else {
                    self.operation_open_url(url, true)
                }
            }
            SearchAction::RunCommand { id } if id.starts_with("theme.") => {
                self.operation_set_app_setting("appearance".into(), id[6..].into())
            }
            SearchAction::RunCommand { id }
                if matches!(
                    id.as_str(),
                    "split.choose" | "sidebar.toggleCompact" | "sidebar.toggleHidden"
                ) =>
            {
                (self.emit)(Projection::UiCommand(id));
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            SearchAction::RunCommand { id } => self.operation_run_command(&id),
        }
    }

    pub(super) fn search(&mut self, query: &str) {
        let Some((profile, space)) = self
            .windows
            .focused()
            .map(|window| (window.profile, window.space))
        else {
            return;
        };
        let (scope, q) = scoped_query(query);
        let needle = q.to_lowercase();
        let mut stack = Vec::new();
        for placement in [
            Placement::Favorites { profile },
            Placement::Space {
                space,
                section: SpaceSection::Pinned,
            },
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ] {
            stack.extend(self.items.roots(placement).iter().rev().copied());
        }
        let mut tabs = Vec::new();
        while let Some(id) = stack.pop() {
            if matches!(scope, SearchScope::All | SearchScope::Tabs)
                && self.items.tab(id).is_some()
                && self.item_in_scope(id, profile, space)
            {
                tabs.push(id);
            }
            stack.extend(self.items.children(id).iter().rev().copied());
        }
        tabs.sort_by_key(|id| std::cmp::Reverse(self.residency.last_focus.get(id).copied()));
        let mut results = Vec::new();
        self.search.generation = self.search.generation.wrapping_add(1);
        if self.search.generation == 0 {
            self.search.generation = 1;
        }
        let generation = self.search.generation;
        self.search.pending = None;

        if q.is_empty() {
            results.extend(
                tabs.iter()
                    .filter_map(|id| {
                        self.items.tab(*id).map(|t| {
                            tab_result(
                                *id,
                                t,
                                self.icon_ref(self.search_surface(), t, Some(profile)),
                            )
                        })
                    })
                    .take(4),
            );
        } else {
            let matched: Vec<(ItemId, &TabState)> = tabs
                .iter()
                .filter_map(|id| self.items.tab(*id).map(|t| (*id, t)))
                .filter(|(_, t)| {
                    match_quality(q, &t.title, t.url.as_ref().map_or("", |url| url.as_str())) > 0
                })
                .take(4)
                .collect();
            let open_urls: std::collections::HashSet<String> = matched
                .iter()
                .filter_map(|(_, t)| t.url.as_ref().map(ToString::to_string))
                .collect();
            results.extend(matched.iter().map(|(id, t)| {
                tab_result(
                    *id,
                    t,
                    self.icon_ref(self.search_surface(), t, Some(profile)),
                )
            }));

            if let Some(url) = (scope == SearchScope::All)
                .then(|| {
                    self.search
                        .engine
                        .configured_classify(q, &self.search.custom_url)
                })
                .flatten()
            {
                if navigation::is_query(q) {
                    results.push(SearchResult {
                        kind: "search".into(),
                        title: zephium_core::search::engine_shortcut(q)
                            .map_or(q, |(_, query)| query)
                            .into(),
                        detail: zephium_core::search::engine_shortcut(q)
                            .map_or(self.search.engine, |(engine, _)| engine)
                            .name()
                            .into(),
                        icon: None,
                        action: SearchAction::OpenUrl {
                            url: url.to_string(),
                        },
                    });
                } else {
                    results.push(SearchResult {
                        kind: "url".into(),
                        title: format!("Open {url}"),
                        detail: "New Tab".into(),
                        icon: self.icon_ref_for_url(self.search_surface(), profile, url.as_str()),
                        action: SearchAction::OpenUrl {
                            url: url.to_string(),
                        },
                    });
                }
            }

            results.extend(
                commands::REGISTRY
                    .iter()
                    // New Tab is an address field, not a command palette. Browser
                    // commands stay in the launcher, which is the surface built
                    // for them; the field offers only places to go.
                    .filter(|_| !self.search_is_newtab())
                    .filter(|_| matches!(scope, SearchScope::All | SearchScope::Commands))
                    .filter(|c| {
                        c.id != "launcher.toggle"
                            && c.available()
                            && !matches!(c.group, commands::Group::Work | commands::Group::Keys)
                            && commands::feature_of(c.id).is_none_or(|feature| {
                                zephium_core::preferences::feature_enabled(
                                    |key| self.store.app_setting(key),
                                    feature,
                                )
                            })
                    })
                    .filter(|c| c.title.to_lowercase().contains(&needle))
                    .take(3)
                    .map(|c| SearchResult {
                        kind: "command".into(),
                        title: c.title.into(),
                        detail: c.default_accelerator().unwrap_or_default().into(),
                        icon: None,
                        action: SearchAction::RunCommand { id: c.id.into() },
                    }),
            );

            // Caps belong to `publish_search`, which alone sees the merged
            // set. Trimming here would discard a local match before a late
            // provider could be weighed against it.
            // Project local tab/URL/command matches immediately. History is
            // presentation-only and arrives asynchronously; the exact query
            // generation below prevents a slow old result replacing newer UI.
            self.publish_search(SearchResults {
                pending: false,
                completion: None,
                context: self.search.context.clone(),
                query: query.into(),
                results: results.clone(),
            });
            // Saved pages are a choice the user made, so they are offered
            // whether or not visited pages are; `@history` looks at visits
            // alone.
            let sources = Sources {
                history: scope == SearchScope::History || self.search.include_history,
                bookmarks: scope == SearchScope::All
                    && self.profiles.get(profile).is_some_and(|profile| {
                        profile.kind != zephium_core::profiles::ProfileKind::Incognito
                    }),
            };
            if q.len() > MAX_ASYNC_SEARCH_QUERY_BYTES
                || !matches!(scope, SearchScope::All | SearchScope::History)
                || !(sources.history || sources.bookmarks)
            {
                return;
            }
            self.search.pending = Some(PendingSearch {
                generation,
                profile,
                lookup_query: q.to_owned(),
                display_query: query.to_owned(),
                open_urls,
                base_results: results,
            });
            if let Some(reads) = &self.store_reads {
                if !reads.request_history(generation, profile, q.to_owned(), sources) {
                    self.search.pending = None;
                }
            } else {
                #[cfg(test)]
                {
                    let (hits, bookmarks) =
                        crate::store_reads::read_suggestions(&*self.store, profile, q, sources);
                    self.on_store_read(StoreReadResult::History {
                        generation,
                        profile,
                        query: q.to_owned(),
                        hits,
                        bookmarks,
                    });
                }
                #[cfg(not(test))]
                {
                    // Production construction always installs the reader.
                    // Keep this defensive branch nonblocking if a future
                    // internal constructor violates that invariant.
                    self.search.pending = None;
                }
            }
            return;
        }

        self.publish_search(SearchResults {
            pending: false,
            completion: None,
            context: self.search.context.clone(),
            query: query.into(),
            results,
        });
    }

    pub(super) fn on_history_read(
        &mut self,
        generation: u64,
        profile: ProfileId,
        query: String,
        hits: Vec<zephium_core::ports::store::HistoryHit>,
        bookmarks: Vec<BookmarkHit>,
    ) {
        if self
            .search
            .context
            .as_ref()
            .is_some_and(|context| !self.search_context_current(context))
        {
            return;
        }
        let exact = self.search.pending.as_ref().is_some_and(|pending| {
            pending.generation == generation
                && pending.profile == profile
                && pending.lookup_query == query
        });
        if !exact {
            return;
        }
        let Some(pending) = self.search.pending.take() else {
            return;
        };
        if self
            .windows
            .focused()
            .is_none_or(|window| window.profile != profile)
        {
            return;
        }

        // Treat even our own store adapter as a serialization boundary: cap
        // cardinality, revalidate URLs, sanitize titles, and deduplicate before
        // values reach privileged launcher markup.
        let mut results = pending.base_results;
        let mut seen = pending.open_urls;
        let rank = |title: &str, url: &str| match_quality(&pending.lookup_query, title, url);
        // A saved page is claimed before the visits, so the same address
        // shown as both appears once, as the bookmark.
        let mut saved = Vec::new();
        for hit in bookmarks.into_iter().take(10) {
            let quality = rank(&hit.title, &hit.url);
            if quality == 0
                || !navigation::is_allowed_str(&hit.url)
                || !seen.insert(hit.url.clone())
            {
                continue;
            }
            saved.push((
                quality,
                SearchResult {
                    kind: "bookmark".into(),
                    title: if hit.title.is_empty() {
                        hit.url.clone()
                    } else {
                        hit.title
                    },
                    detail: hit.url.clone(),
                    icon: None,
                    action: SearchAction::OpenUrl { url: hit.url },
                },
            ));
        }
        let mut visited = Vec::new();
        for hit in hits.into_iter().take(10) {
            if !navigation::is_allowed_str(&hit.url) || !seen.insert(hit.url.clone()) {
                continue;
            }
            let title = zephium_core::item::sanitize_page_title(&hit.title);
            visited.push((
                rank(&title, &hit.url),
                SearchResult {
                    kind: if zephium_core::search::is_search_url(&hit.url) {
                        "search_history"
                    } else {
                        "history"
                    }
                    .into(),
                    title: if title.is_empty() {
                        hit.url.clone()
                    } else {
                        title
                    },
                    detail: hit.url.clone(),
                    icon: self.icon_ref_for_url(self.search_surface(), profile, &hit.url),
                    action: SearchAction::OpenUrl { url: hit.url },
                },
            ));
        }
        results.extend(interleave_bookmarks(saved, visited));
        // Only stored hits are asked for icons: a typed address changes with
        // every keystroke and must never reach the network as a probe.
        self.want_icons(
            self.search_surface(),
            profile,
            results
                .iter()
                .filter(|result| result.icon.is_none() && matches!(result.kind.as_str(), "history"))
                .map(|result| result.detail.as_str()),
        );
        self.publish_search(SearchResults {
            pending: false,
            completion: None,
            context: self.search.context.clone(),
            query: pending.display_query,
            results,
        });
    }
}

fn tab_result(id: ItemId, tab: &TabState, icon: Option<zephium_ipc::IconRef>) -> SearchResult {
    let detail = tab
        .url
        .as_ref()
        .and_then(|u| u.host_str().map(ToString::to_string))
        .unwrap_or_default();
    SearchResult {
        kind: "tab".into(),
        title: tab.title.clone(),
        detail,
        icon,
        action: SearchAction::ActivateTab { id: id.to_string() },
    }
}
