  .regular_tabs(space)
            .into_iter()
            .find(|tab| *tab != id)
            .filter(|tab| self.is_untouched_new_tab(*tab))
            .filter(|_| self.regular_tabs(space).len() == 2);
        let mut left_behind = None;
        let effects = if space == current {
            self.focus_tab(id)
        } else {
            let (effects, blank) = self.show_space_at(space, id);
            left_behind = blank;
            effects
        };
        let native = self.commit(effects);
        for stale in left_behind.into_iter().chain(stand_in) {
            self.close_in_any_space(stale);
        }
        Some((id, native))
    }

    /// Removes a failed native leaf from the retained split immediately. A
    /// create failure may arrive after `operation_split`/`apply_drop` has
    /// committed its optimistic topology; retaining that leaf would let a
    /// later single-tab retry silently resurrect the old group.
    pub(super) fn collapse_failed_split_leaf(&mut self, id: ItemId) -> bool {
        let Some((tree, failed_was_active)) = self.windows.focused().and_then(|window| {
            window
                .splits
                .as_ref()
                .filter(|tree| tree.contains(id))
                .cloned()
                .map(|tree| (tree, window.active == Some(id)))
        }) else {
            return false;
        };
        self.drop_divider();
        let remaining = tree.remove(id);
        let replacement = failed_was_active
            .then(|| {
                remaining.as_ref().and_then(|tree| {
                    tree.tabs().into_iter().find(|candidate| {
                        self.items.tab(*candidate).is_some_and(TabState::has_view)
                    })
                })
            })
            .flatten();
        if let Some(window) = self.windows.focused_mut() {
            window.splits = remaining;
            if replacement.is_some() {
                // `focus_tab` owns lifecycle/recent bookkeeping below.
                window.active = None;
            }
        }
        if let Some(replacement) = replacement {
            // The replacement was selected from live split leaves, so this
            // normally emits no construction effect. Keep the invariant even
            // if future lifecycle states add another recoverable resident.
            let effects = self.focus_tab(replacement);
            let _ = self.apply(effects);
        }
        true
    }

    pub(super) fn apply_drop(
        &mut self,
        target: ItemId,
        dropped: ItemId,
        edge: Edge,
    ) -> OperationDisposition {
        let Some((profile, space)) = self.windows.focused().map(|win| (win.profile, win.space))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if !self.item_in_scope(target, profile, space)
            || !self.item_in_scope(dropped, profile, space)
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if [target, dropped].into_iter().any(|id| {
            self.items
                .tab(id)
                .is_some_and(|tab| tab.content != zephium_core::item::TabContent::Web)
        }) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::LayoutUnavailable,
            );
        }
        if target == dropped {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let Some(mut tree) = self.pane_tree() else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::LayoutUnavailable,
            );
        };
        if !self.pane_in_scope(&tree, profile, space) || tree.tabs().len() >= MAX_VISIBLE_PANES {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::LayoutUnavailable,
            );
        }
        if tree.contains(dropped) {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let effects = self.items.ensure_view(dropped);
        let mut native = self.apply(effects);
        if !self.items.tab(dropped).is_some_and(TabState::has_view) {
            // Match `operation_split`: synchronous native refusal must leave
            // the previously rendered topology authoritative.
            return mutation_result(native);
        }
        self.touch(dropped);
        if !tree.split(target, dropped, edge.axis(), edge.before()) {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        if let Some(win) = self.windows.focused_mut() {
            win.splits = Some(tree);
        }
        native.merge(self.commit(Vec::new()));
        mutation_result(native)
    }

    pub(super) fn adopt_linked_native_tab(
        &mut self,
        source: ItemId,
        child: ItemId,
        foreground: bool,
        adoption: zephium_core::ports::engine::NativeTabAdoption,
    ) {
        if self.mini_window_of(source).is_some() {
            self.adopt_linked_mini(source, child, foreground, adoption);
            return;
        }
        let Some((profile, space)) = self.windows.focused().map(|win| (win.profile, win.space))
        else {
            return;
        };
        if !self.item_in_scope(source, profile, space)
            || self.items.view_ids().len() >= LIVE_VIEW_ABSOLUTE_LIMIT
            || self.items.get(child).is_some()
        {
            return;
        }
        if !self.items.insert_tab(
            child,
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ) {
            return;
        }
        if !self.items.adopt_native_view(child) {
            let _ = self.items.remove(child);
            return;
        }
        self.record_view_creation(child);
        self.items.clear_blocked_popup(source);
        self.project_tab(source);
        self.native_openers.insert(
            child,
            NativeOpener {
                source,
                activate_when_presentable: foreground,
            },
        );
        let placement = Placement::Space {
            space,
            section: SpaceSection::Today,
        };
        let siblings = self.items.roots(placement);
        let next = siblings
            .iter()
            .position(|id| *id == source)
            .and_then(|index| siblings.get(index + 1))
            .copied();
        self.items.move_tab_to_root(child, placement, next);
        // Keep the source selected while the native response is unresolved.
        // A download never presents a document and therefore never selects
        // this transient tab or flashes the privileged New Tab surface.
        self.commit(Vec::new());
        adoption.finish(true);
    }

    pub(super) fn activate_presented_native_tab(&mut self, child: ItemId) {
        let source = self.native_openers.get_mut(&child).and_then(|opener| {
            std::mem::take(&mut opener.activate_when_presentable).then_some(opener.source)
        });
        if source.is_some_and(|source| {
            self.windows
                .focused()
                .is_some_and(|window| window.active == Some(source))
        }) {
            let effects = self.focus_tab(child);
            self.commit(effects);
        }
    }

    /// Internal native ownership settlement, including after a space switch.
    /// User-facing Close retains its focused-scope authorization above.
    pub(super) fn close_owned_native_tab(&mut self, child: ItemId) {
        let Some(opener) = self.native_openers.remove(&child) else {
            #[cfg(target_os = "windows")]
            if self.extension_document_may_close(child) {
                self.close_in_any_space(child);
            }
            return;
        };
        if self.item_in_focused_scope(child) {
            if self
                .windows
                .focused()
                .is_some_and(|window| window.active == Some(child))
            {
                let effects = self.focus_tab(opener.source);
                self.commit(effects);
            }
            self.close(child);
            return;
        }
        self.cancel_page_permission_for_item(child);
        self.forget_page_permission_documents_of_item(child);
        self.cancel_pending_presentation(child);
        self.cancel_favicon_attempt(child);
        self.cancel_discard_probe(child);
        let windows: Vec<_> = self.windows.iter().map(|window| window.id).collect();
        for id in windows {
            if let Some(window) = self.windows.get_mut(id) {
                if window.active == Some(child) {
                    window.active = None;
                }
                if let Some(tree) = window.splits.take() {
                    window.splits = tree.remove(child);
                }
            }
        }
        let effects = self.items.remove(child);
        self.commit(effects);
    }

    /// `x`/`y` are window coords (the desktop layer normalizes per platform).
    /// window.open / target=_blank lands as a new Today tab next to its
    /// source, routed through the same navigation policy. The split group
    /// survives: the new tab shows alone, the group stays a tab away.
    pub(super) fn open_linked_tab(&mut self, source: ItemId, url: &str) {
        if self.mini_window_of(source).is_some() {
            if !self.open_linked_mini(source, url) {
                self.open_mini_link_as_tab(source, url);
            }
            return;
        }
        let Some((profile, space)) = self.windows.focused().map(|win| (win.profile, win.space))
        else {
            return;
        };
        if !self.item_in_scope(source, profile, space) {
            return;
        }
        let id = ItemId::generate();
        if !self.items.insert_tab(
            id,
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
        ) {
            return;
        }
        let mut fx = self.focus_tab(id);
        fx.extend(self.items.navigate(id, url));
        self.commit(fx);
    }

    /// A new Today tab of `space` for an ordinary web page, right after
    /// `after` when that tab is a root of the same Today, never focused:
    /// the active tab, window focus and split stay as they are.
    #[cfg(test)]
    pub(super) fn open_background_tab(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        after: Option<ItemId>,
        url: &str,
    ) -> Option<ItemId> {
        self.open_page_tab(profile, space, after, url, false)
    }

    pub(super) fn open_page_tab(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        after: Option<ItemId>,
        url: &str,
        focus: bool,
    ) -> Option<ItemId> {
        let url = url::Url::parse(url.trim()).ok().filter(|url| {
            matches!(url.scheme(), "http" | "https") && navigation::is_allowed(url)
        })?;
        if self.incognito_profile(profile)
            || self.profile_deletion_quarantines(profile)
            || self
                .windows
                .focused()
                .is_none_or(|win| win.profile != profile)
            || self
                .spaces
                .get(space)
                .is_none_or(|owner| owner.profile != profile)
        {
            return None;
        }
        let today = Placement::Space {
            space,
            section: SpaceSection::Today,
        };
        let id = ItemId::generate();
        if !self.items.insert_tab(id, today) {
            return None;
        }
        let next = after.and_then(|after| {
            let roots = self.items.roots(today);
            let at = roots.iter().position(|root| *root == after)?;
            roots.get(at + 1).copied().filter(|next| *next != id)
        });
        if let Some(next) = next {
            self.items.move_tab_to_root(id, today, Some(next));
        }
        let mut effects = if focus {
            self.focus_tab(id)
        } else {
            Default::default()
        };
        effects.extend(self.items.navigate(id, url.as_str()));
        let native = self.commit(effects);
        // A view the engine refused and no capacity wait explains is a dead
        // tab; nothing was opened.
        if native.rejected
            && self.items.tab(id).is_some_and(|tab| !tab.has_view())
            && self.capacity_presentation(id).is_none()
        {
            self.close_in_any_space(id);
            return None;
        }
        Some(id)
    }
}

/// Wall-clock now in Unix milliseconds, within the range persisted times use.
pub(super) fn unix_millis() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .filter(|ms| (1_000..=zephium_core::session::MAX_EXACT_MS).contains(ms))
}

#[cfg(test)]
mod tests {
    use super::restored_tab_is_new_tab_destination;
    use url::Url;

    #[test]
    fn restored_destination_matches_www_redirects() {
        for (destination, restored) in [
            ("https://google.com/", "https://www.google.com/"),
            ("https://www.google.com/", "https://google.com/"),
            ("http://google.com/", "https://www.google.com/"),
        ] {
            assert!(
                restored_tab_is_new_tab_destination(
                    &Url::parse(destination).unwrap(),
                    &Url::parse(restored).unwrap(),
                ),
                "{destination} and {restored}",
            );
        }
    }

    #[test]
    fn restored_destination_rejects_other_url_changes() {
        for (destination, restored) in [
            ("https://google.com/", "https://www.google.com/other"),
            ("https://google.com/", "https://www.google.com/?q=1"),
            ("https://google.com/", "https://www.google.com/#top"),
            ("https://google.com/", "https://www.google.com:8443/"),
            ("https://google.com/", "http://www.google.com/"),
            ("https://google.com/", "https://www.www.google.com/"),
            ("https://www.com/", "https://com/"),
            ("https://www.com./", "https://com./"),
            ("http://127.0.0.1/", "https://127.0.0.1/"),
            ("https://google.com/", "https://user@www.google.com/"),
            (
                "https://user:one@google.com/",
                "https://user:two@www.google.com/",
            ),
        ] {
            assert!(
                !restored_tab_is_new_tab_destination(
                    &Url::parse(destination).unwrap(),
                    &Url::parse(restored).unwrap(),
                ),
                "{destination} and {restored}",
            );
        }
    }
}
