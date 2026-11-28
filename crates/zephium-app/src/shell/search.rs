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
        let (scope, scoped) = scoped_query(query);
        let parsed = self.search.shortcuts.parse(query);
        let shortcut = if scope == SearchScope::All {
            parsed
                .shortcut
                .and_then(|id| self.search.shortcuts.get(id))
                .cloned()
        } else {
            None
        };
        let q = if shortcut.is_some() {
            parsed.query.as_ref()
        } else {
            scoped
        };
        let needle = q.to_lowercase();
        let mut stack = Vec::new();
        for placement in [
            Placement::Favorites { space },
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
        // A blank tab is not a site, and it must not use up a row.
        if self.search.mini {
            tabs.retain(|id| {
                self.items
                    .tab(*id)
                    .and_then(|tab| tab.url.as_ref())
                    .is_some_and(|url| navigation::is_allowed_str(url.as_str()))
            });
        }
        tabs.sort_by_key(|id| std::cmp::Reverse(self.residency.last_focus.get(id).copied()));
        let mut results = Vec::new();
        self.search.generation = self.search.generation.wrapping_add(1);
        if self.search.generation == 0 {
            self.search.generation = 1;
        }
        let generation = self.search.generation;
        self.search.pending = None;

        if q.is_empty() && shortcut.is_none() {
            results.extend(
                tabs.iter()
                    .filter_map(|id| {
                        self.items.tab(*id).map(|t| {
                            tab_result(
                                *id,
                                t,
                                self.icon_ref(self.search_surface(), t, Some(profile)),
                                self.search.mini,
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
                    self.search.mini,
                )
            }));

            if let Some(url) = (scope == SearchScope::All)
                .then(|| {
                    self.search.shortcuts.resolve(
                        query,
                        self.search.engine,
                        &self.search.custom_url,
                        &self.search.kagi_token,
                    )
                })
                .flatten()
            {
                if shortcut.is_some() || navigation::is_query(q) {
                    results.push(SearchResult {
                        kind: "search".into(),
                        title: if q.is_empty() {
                            shortcut.as_ref().map_or("", |item| item.name.as_str())
                        } else {
                            q
                        }
                        .into(),
                        detail: shortcut
                            .as_ref()
                            .map_or(self.search.engine.name(), |item| item.name.as_str())
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
                shortcut: None,
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
            shortcut: None,
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
            shortcut: None,
            pending: false,
            completion: None,
            context: self.search.context.clone(),
            query: pending.display_query,
            results,
        });
    }
}

fn tab_result(
    id: ItemId,
    tab: &TabState,
    icon: Option<zephium_ipc::IconRef>,
    mini: bool,
) -> SearchResult {
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
        action: match tab.url.as_ref().filter(|_| mini) {
            // The tab stays where it is; its site opens in the mini window.
            Some(url) => SearchAction::OpenUrl {
                url: url.to_string(),
            },
            None => SearchAction::ActivateTab { id: id.to_string() },
        },
    }
}
