//! Accepted user-operation interpretation and completion disposition.

use super::*;

impl Shell {
    pub(super) fn handle_operation(&mut self, command: Command) -> OperationDisposition {
        match command {
            Command::StopMediaCapture { item, navigation } => {
                self.operation_stop_media_capture(item, navigation)
            }
            Command::ShowBrowserPage(page) => self.operation_show_browser_page(page),
            Command::WorkPaneShow { target, rect } => self.operation_work_pane_show(target, rect),
            Command::WorkPaneHide => self.operation_work_pane_hide(),
            Command::Open => self.operation_open(),
            Command::Activate(id) => self.operation_activate(id),
            Command::Close(id) => self.operation_close(id),
            Command::SetTabEssential {
                id,
                essential,
                before,
            } => self.operation_set_tab_essential(id, essential, before),
            Command::KeepSite(id) => self.operation_keep_site(&id),
            Command::RenameFocusedProfile(name) => self.operation_rename_focused_profile(&name),
            Command::Navigate { id, input } => self.operation_navigate(id, input),
            Command::Reload(id) => self.operation_reload(id),
            Command::AnswerPageRequest { id, decision } => {
                self.operation_answer_page_request(id, decision)
            }
            Command::GoBack(id) => self.operation_history(id, false),
            Command::GoForward(id) => self.operation_history(id, true),
            Command::SplitWith { other, axis } => self.operation_split(other, axis),
            Command::Unsplit => self.operation_unsplit(),
            Command::LeaveSplit(id) => self.operation_leave_split(id),
            Command::TabAction { id, action } => self.operation_tab_action(id, action),
            Command::DropTab { id, x, y } => self.operation_drop_tab(id, x, y),
            Command::DividerRelease { x, y } => self.operation_divider_release(x.zip(y)),
            Command::Run(id) => self.operation_run_command(&id),
            Command::RunSearchAction {
                context,
                action,
                background,
            } => self.operation_run_search_action(*context, action, background),
            Command::InvokeExtensionAction {
                runtime,
                revision,
                anchor,
            } => self.operation_invoke_extension_action(runtime, revision, anchor),
            Command::OpenUrl { input, new_tab } => self.operation_open_url(input, new_tab),
            Command::SetAppSetting { key, value } => self.operation_set_app_setting(key, value),
            Command::Focus(control) => self.operation_focus(control),
            Command::RetryContentPolicy {
                profile,
                failed_generation,
            } => self.operation_retry_content_policy(profile, failed_generation),
            Command::RetryFocusedContentPolicy { failed_generation } => {
                let Some(profile) = self.windows.focused().map(|window| window.profile) else {
                    return operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::NoFocusedWindow,
                    );
                };
                self.operation_retry_content_policy(profile, failed_generation)
            }
            _ => operation_result(
                OperationOutcome::Rejected,
                OperationReason::UnsupportedCommand,
            ),
        }
    }

    pub(super) fn operation_stop_media_capture(
        &mut self,
        item: ItemId,
        navigation: NavigationPresentationId,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(item)
            || !self.items.tab(item).is_some_and(|tab| {
                tab.capture
                    .is_some_and(|(current, state)| current == navigation && state.is_capturing())
            })
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if self.engine.stop_media_capture(item, navigation) == NativeDispatch::Scheduled {
            operation_result(
                OperationOutcome::Deferred,
                OperationReason::NativeWorkPending,
            )
        } else {
            operation_result(
                OperationOutcome::NativeAdmissionFailed,
                OperationReason::NativeDispatchRejected,
            )
        }
    }

    pub(super) fn operation_set_tab_essential(
        &mut self,
        id: ItemId,
        essential: bool,
        before: Option<ItemId>,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(window) = self.windows.focused() else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let placement = if essential {
            Placement::Favorites {
                profile: window.profile,
            }
        } else {
            Placement::Space {
                space: window.space,
                section: SpaceSection::Today,
            }
        };
        if before.is_some_and(|before| !self.item_in_focused_scope(before)) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if !self.items.move_tab_to_root(id, placement, before) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        mutation_result(self.commit(Vec::new()))
    }

    pub(super) fn operation_tab_action(
        &mut self,
        id: ItemId,
        action: TabAction,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        match action {
            TabAction::Duplicate => self.operation_duplicate(id),
            TabAction::Bookmark => self.operation_bookmark_page(Some(id)),
            TabAction::CloseOthers => self.operation_close_around(id, false),
            TabAction::CloseBelow => self.operation_close_around(id, true),
        }
    }

    fn operation_duplicate(&mut self, id: ItemId) -> OperationDisposition {
        let Some(url) = self
            .items
            .tab(id)
            .filter(|tab| tab.content == zephium_core::item::TabContent::Web)
            .and_then(|tab| tab.url.as_ref())
            .map(ToString::to_string)
        else {
            return operation_result(OperationOutcome::NoOp, OperationReason::InvalidInput);
        };
        let Some(space) = self.windows.focused().map(|window| window.space) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Some((copy, mut effects)) = self.open_tab_with_id() else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        let today = Placement::Space {
            space,
            section: SpaceSection::Today,
        };
        let roots = self.items.roots(today);
        if let Some(at) = roots.iter().position(|root| *root == id) {
            let before = roots[at + 1..].iter().copied().find(|root| *root != copy);
            self.items.move_tab_to_root(copy, today, before);
        }
        effects.extend(self.items.navigate(copy, &url));
        mutation_result(self.commit(effects))
    }

    /// Closes the space's open tabs other than `id`, or only those after it.
    /// Kept and pinned tabs stay, as they do when a day's tabs are cleared.
    fn operation_close_around(&mut self, id: ItemId, below: bool) -> OperationDisposition {
        let Some((space, active)) = self
            .windows
            .focused()
            .map(|window| (window.space, window.active))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let today = self.today_tabs(space);
        let closing: Vec<ItemId> = if below {
            let Some(at) = today.iter().position(|tab| *tab == id) else {
                return operation_result(OperationOutcome::NoOp, OperationReason::InvalidScope);
            };
            today[at + 1..].to_vec()
        } else {
            today.into_iter().filter(|tab| *tab != id).collect()
        };
        if closing.is_empty() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let mut native = NativeWork::default();
        // Hand the window to the tab that stays first, so closing the one in
        // front never wakes each of the others in turn on its way out.
        if active.is_some_and(|active| closing.contains(&active)) {
            let effects = self.focus_tab(id);
            native.merge(self.commit(effects));
        }
        for tab in closing {
            native.merge(self.close(tab));
        }
        mutation_result(native)
    }

    /// A tab dragged out of a split stands alone; a split left with one tab
    /// is no split at all.
    pub(super) fn operation_leave_split(&mut self, id: ItemId) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(win) = self.windows.focused_mut() else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Some(tree) = win.splits.take_if(|tree| tree.contains(id)) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        win.splits = tree.remove(id).filter(|rest| rest.tabs().len() > 1);
        self.drop_divider();
        mutation_result(self.commit(Vec::new()))
    }

    fn operation_invoke_extension_action(
        &mut self,
        runtime: zephium_core::extensions::ExtensionRuntimeInstance,
        revision: zephium_core::extensions::ExtensionActionRevision,
        anchor: zephium_core::extensions::ExtensionPopupAnchor,
    ) -> OperationDisposition {
        match self.invoke_extension_action(runtime, revision, anchor) {
            Ok(_) => operation_result(
                OperationOutcome::Deferred,
                OperationReason::NativeWorkPending,
            ),
            Err(
                reason @ zephium_core::extensions::ExtensionActionRejection::NativeAdmissionFailed,
            ) => {
                crate::diagnostic!(
                    "extensions: toolbar action synchronously rejected with typed reason {reason:?}"
                );
                self.project_extension_action_failure(runtime.profile(), None, reason);
                operation_result(
                    OperationOutcome::NativeAdmissionFailed,
                    OperationReason::NativeDispatchRejected,
                )
            }
            Err(
                reason @ (zephium_core::extensions::ExtensionActionRejection::UnsupportedPlatform
                | zephium_core::extensions::ExtensionActionRejection::PopupUnavailable),
            ) => {
                crate::diagnostic!(
                    "extensions: toolbar action synchronously rejected with typed reason {reason:?}"
                );
                self.project_extension_action_failure(runtime.profile(), None, reason);
                operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::UnsupportedCommand,
                )
            }
            Err(reason) => {
                crate::diagnostic!(
                    "extensions: toolbar action synchronously rejected with typed reason {reason:?}"
                );
                self.project_extension_action_failure(runtime.profile(), None, reason);
                operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope)
            }
        }
    }

    pub(super) fn operation_open(&mut self) -> OperationDisposition {
        if self.active_browser_page().is_some() && !self.browser_return_ready {
            self.browser_after_return = Some(Box::new(Command::Open));
            return self.operation_show_browser_page(None);
        }

        if self.windows.focused().is_none() {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        }
        let Some((_id, effects)) = self.open_tab_with_id() else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        mutation_result(self.commit(effects))
    }

    /// Opens an address in the focused window without naming a tab, which the
    /// launcher panel and the history surfaces cannot do.
    pub(super) fn operation_open_url(
        &mut self,
        input: String,
        new_tab: bool,
    ) -> OperationDisposition {
        let Some(input) = self
            .search
            .engine
            .configured_classify(&input, &self.search.custom_url)
            .map(|url| url.to_string())
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        // Work hands an address back to Browse rather than opening it behind itself.
        if (self.active_browser_page() == Some(crate::BrowserPage::Work)
            || (new_tab && self.active_browser_page().is_some()))
            && !self.browser_return_ready
        {
            self.browser_after_return = Some(Box::new(Command::OpenUrl { input, new_tab }));
            return self.operation_show_browser_page(None);
        }
        let Some(active) = self.windows.focused().map(|window| window.active) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if new_tab && self.tab_preferences.switch_to_open {
            if let Some(open) = self.open_tab_for(&input) {
                return self.operation_activate(open);
            }
        }
        if let Some(id) = active.filter(|_| !new_tab) {
            // Navigating in place also returns from a browser page, which is
            // what opening a row from the history library should do.
            return self.operation_navigate(id, input);
        }
        let Some((id, mut effects)) = self.open_tab_with_id() else {
            // Reaching the bounded item limit must not repurpose and navigate
            // the caller's existing active tab.
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        effects.extend(self.items.navigate(id, &input));
        mutation_result(self.commit(effects))
    }

    /// Opens an address in a new tab of the focused window without selecting
    /// it, as a modified click on a link does.
    /// Finds in the page in front. Moving to another page ends the search in
    /// the one before, so its highlights never linger out of sight.
    pub(super) fn find_in_page(
        &mut self,
        request: Option<zephium_core::ports::engine::FindRequest>,
    ) {
        let page = if self.active_browser_page().is_some() {
            self.work_pane_tab()
        } else {
            self.windows.focused().and_then(|window| window.active)
        };
        if let Some(previous) = self
            .find_target
            .filter(|previous| Some(*previous) != page || request.is_none())
        {
            let _ = self.engine.find(previous, None);
            self.find_target = None;
        }
        let (Some(page), Some(request)) = (page, request) else {
            return;
        };
        if self.engine.find(page, Some(request))
            != zephium_core::ports::engine::NativeDispatch::Rejected
        {
            self.find_target = Some(page);
        }
    }

    /// Opens what another application handed over, each in its own tab and
    /// the last one in front, the way a clicked link opens elsewhere.
    pub(super) fn open_external(&mut self, urls: Vec<String>) {
        let limit = zephium_core::navigation::MAX_EXTERNAL_TARGETS;
        if !self.bootstrapped {
            let room = limit.saturating_sub(self.pending_external.len());
            self.pending_external.extend(urls.into_iter().take(room));
            return;
        }
        // Another application's link is not a private page: it opens with
        // the regular tabs, the way a private window leaves it to another.
        if self.private_shown() {
            let _ = self.operation_leave_private();
        }
        for url in urls.into_iter().take(limit) {
            if zephium_core::navigation::external_target(&url).is_some() {
                let _ = self.operation_open_url(url, true);
            }
        }
    }

    pub(super) fn operation_open_url_background(&mut self, input: String) -> OperationDisposition {
        let Some(input) = self
            .search
            .engine
            .configured_classify(&input, &self.search.custom_url)
            .map(|url| url.to_string())
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        let Some((profile, space)) = self
            .windows
            .focused()
            .map(|window| (window.profile, window.space))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let id = ItemId::generate();
        if self.profile_deletion_quarantines(profile)
            || !self.items.insert_tab(
                id,
                Placement::Space {
                    space,
                    section: SpaceSection::Today,
                },
            )
        {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        }
        let effects = self.items.navigate(id, &input);
        mutation_result(self.commit(effects))
    }

    pub(super) fn operation_activate(&mut self, id: ItemId) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let active = self.windows.focused().and_then(|window| window.active);
        if active == Some(id)
            && self
                .items
                .tab(id)
                .is_some_and(|tab| tab.content != zephium_core::item::TabContent::Web)
        {
            self.touch(id);
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        if self.active_browser_page().is_some()
            && !self.browser_return_ready
            && self.items.tab(id).is_some_and(|tab| {
                !matches!(tab.content, zephium_core::item::TabContent::BrowserOwned(_))
            })
        {
            self.browser_after_return = Some(Box::new(Command::Activate(id)));
            return self.operation_show_browser_page(None);
        }
        if self.items.tab(id).is_some_and(|tab| {
            matches!(tab.content, zephium_core::item::TabContent::BrowserOwned(_))
        }) {
            self.browser_page = None;
        }
        let has_view = self.items.tab(id).is_some_and(TabState::has_view);
        let discard_closing = matches!(
            self.residency.discard_probes.get(&id),
            Some(PendingDiscardProbe::Closing { .. })
        );
        if active == Some(id) && has_view && !discard_closing {
            self.touch(id);
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let effects = self.focus_tab(id);
        let native = self.commit(effects);
        if discard_closing {
            operation_result(
                OperationOutcome::Deferred,
                OperationReason::DiscardCompletionPending,
            )
        } else {
            mutation_result(native)
        }
    }

    pub(super) fn operation_close(&mut self, id: ItemId) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if !self.browser_return_ready
            && self
                .windows
                .focused()
                .is_some_and(|window| window.active == Some(id))
            && self
                .items
                .tab(id)
                .is_some_and(|tab| tab.content != zephium_core::item::TabContent::Web)
        {
            self.browser_after_return = Some(Box::new(Command::Close(id)));
            return self.request_browser_return();
        }
        let discard_closing = matches!(
            self.residency.discard_probes.get(&id),
            Some(PendingDiscardProbe::Closing { .. })
        );
        let native = self.close(id);
        if discard_closing && !native.rejected {
            operation_result(
                OperationOutcome::Deferred,
                OperationReason::DiscardCompletionPending,
            )
        } else {
            mutation_result(native)
        }
    }

    pub(super) fn operation_navigate(&mut self, id: ItemId, input: String) -> OperationDisposition {
        if self.active_browser_page().is_some()
            && !self.browser_return_ready
            && !self.work_pane_shows(id)
        {
            self.browser_after_return = Some(Box::new(
                if self.items.tab(id).is_some_and(|tab| {
                    matches!(tab.content, zephium_core::item::TabContent::BrowserOwned(_))
                }) {
                    Command::OpenUrl {
                        input,
                        new_tab: true,
                    }
                } else {
                    Command::Navigate { id, input }
                },
            ));
            return self.operation_show_browser_page(None);
        }

        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if self
            .items
            .tab(id)
            .is_some_and(|tab| tab.content != zephium_core::item::TabContent::Web)
        {
            // The address field never turns a browser-owned or extension-owned
            // principal into an ordinary site. Navigate in a fresh web tab.
            return self.operation_open_url(input, true);
        }
        let Some(input) = self
            .search
            .engine
            .configured_classify(&input, &self.search.custom_url)
            .map(|url| url.to_string())
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        if let Some(context) = self
            .search
            .context
            .as_ref()
            .filter(|context| context.session_id.starts_with(&format!("newtab:{id}:")))
        {
            let session = context.session_id.clone();
            self.cancel_scoped_search(&session);
        }
        if let Some(PendingDiscardProbe::Closing {
            probe,
            recreate,
            deferred_navigation,
            reload_on_refusal,
            ..
        }) = self.residency.discard_probes.get_mut(&id)
        {
            *recreate = true;
            *deferred_navigation = Some(input);
            *reload_on_refusal = false;
            self.engine.cancel_discard(id, *probe);
            return operation_result(
                OperationOutcome::Deferred,
                OperationReason::DiscardCompletionPending,
            );
        }
        self.cancel_discard_probe(id);
        let effects = self.items.navigate(id, &input);
        if effects.is_empty() {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        self.cancel_page_permission_for_item(id);
        mutation_result(self.commit(effects))
    }

    pub(super) fn operation_reload(&mut self, id: ItemId) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if self.recreate_after_inflight_discard(id) {
            if let Some(PendingDiscardProbe::Closing {
                reload_on_refusal,
                deferred_navigation,
                ..
            }) = self.residency.discard_probes.get_mut(&id)
            {
                *reload_on_refusal = true;
                *deferred_navigation = None;
            }
            return operation_result(
                OperationOutcome::Deferred,
                OperationReason::DiscardCompletionPending,
            );
        }
        self.cancel_discard_probe(id);
        self.cancel_page_permission_for_item(id);
        if !self.items.tab(id).is_some_and(TabState::has_view) {
            let effects = self.items.ensure_view(id);
            if effects.is_empty() {
                return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
            }
            return mutation_result(self.commit(effects));
        }
        let mut native = NativeWork::default();
        native.record(self.engine.reload(id));
        mutation_result(native)
    }

    pub(super) fn operation_history(&mut self, id: ItemId, forward: bool) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(tab) = self.items.tab(id) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        let available = if forward {
            tab.can_go_forward
        } else {
            tab.can_go_back
        };
        if !available || !tab.has_view() {
            return operation_result(OperationOutcome::NoOp, OperationReason::HistoryUnavailable);
        }
        if self.recreate_after_inflight_discard(id) {
            // Native back/forward history belongs to the controller that is
            // already closing and cannot be reconstructed by URL alone.
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::HistoryUnavailable,
            );
        }
        self.cancel_discard_probe(id);
        self.cancel_page_permission_for_item(id);
        let admission = if forward {
            self.engine.go_forward(id)
        } else {
            self.engine.go_back(id)
        };
        let mut native = NativeWork::default();
        native.record(admission);
        mutation_result(native)
    }

    pub(super) fn operation_split(&mut self, other: ItemId, axis: Axis) -> OperationDisposition {
        let Some((active, profile, space)) = self
            .windows
            .focused()
            .and_then(|win| win.active.map(|active| (active, win.profile, win.space)))
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if !self.item_in_scope(active, profile, space) || !self.item_in_scope(other, profile, space)
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if [active, other].into_iter().any(|id| {
            self.items.tab(id).is_some_and(|tab| {
                matches!(tab.content, zephium_core::item::TabContent::BrowserOwned(_))
            })
        }) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::LayoutUnavailable,
            );
        }
        if active == other {
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
        if tree.contains(other) {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let closing = [active, other].into_iter().any(|id| {
            matches!(
                self.residency.discard_probes.get(&id),
                Some(PendingDiscardProbe::Closing { .. })
            )
        });
        let effects = self.items.ensure_view(other);
        let mut native = self.apply(effects);
        if !self.items.tab(other).is_some_and(TabState::has_view) {
            // A synchronous create-dispatch refusal already rolled back the
            // optimistic view bit. Do not install a split whose new leaf can
            // never be represented by the native layout admitted below.
            return mutation_result(native);
        }
        self.touch(other);
        if !tree.split(active, other, axis, false) {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        if let Some(win) = self.windows.focused_mut() {
            win.splits = Some(tree);
        }
        native.merge(self.commit(Vec::new()));
        if closing && !native.rejected {
            operation_result(
                OperationOutcome::Deferred,
                OperationReason::DiscardCompletionPending,
            )
        } else {
            mutation_result(native)
        }
    }

    pub(super) fn operation_unsplit(&mut self) -> OperationDisposition {
        let Some(win) = self.windows.focused_mut() else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        if win.splits.take().is_none() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        mutation_result(self.commit(Vec::new()))
    }

    pub(super) fn operation_drop_tab(
        &mut self,
        id: ItemId,
        x: f64,
        y: f64,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(window) = self.windows.focused().map(|window| window.id) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        let Some(drop) = self.resolve_drop(x, y) else {
            let _ = self.engine.set_drop_indicator(window, None);
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        let result = self.apply_drop(drop.tab, id, drop.edge);
        let _ = self.engine.set_drop_indicator(window, None);
        result
    }

    pub(super) fn operation_divider_release(
        &mut self,
        final_pointer: Option<(f64, f64)>,
    ) -> OperationDisposition {
        if self.divider.is_none() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        if let Some((x, y)) = final_pointer {
            self.divider_drag(x, y);
        }
        if !self.commit_divider() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        self.schedule_persist();
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    pub(super) fn operation_set_app_setting(
        &mut self,
        key: String,
        value: String,
    ) -> OperationDisposition {
        if !zephium_core::preferences::value_allowed(&key, &value) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        }
        if !self.store.set_app_setting(key.clone(), value.clone()) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        }
        if key == "search.custom-url" {
            self.search.custom_url = value.clone();
        }
        if key == "search.engine" {
            self.search.engine =
                zephium_core::search::SearchEngine::from_id(&value).unwrap_or_default();
        }
        if key == "search.history" {
            self.search.include_history = value == "true";
        }
        self.tab_preferences.apply(&key, &value);
        self.apply_time_setting(&key, &value);
        self.apply_performance_setting(&key, &value);
        // This projection is downstream of truthful store-queue admission.
        // The desktop composition root applies native theme state from this
        // signal, never optimistically from the IPC request itself.
        let command = if key == "appearance" {
            format!("theme.{value}")
        } else {
            format!("preference.{key}={value}")
        };
        (self.emit)(Projection::UiCommand(command));
        operation_result(
            OperationOutcome::Deferred,
            OperationReason::StoreWorkPending,
        )
    }

    pub(super) fn operation_run_command(&mut self, id: &str) -> OperationDisposition {
        // Inside Work the pane's tab is the only page a shortcut can mean.
        let in_work = self.active_browser_page().is_some();
        let active = if in_work {
            self.work_pane_tab()
        } else {
            self.windows.focused().and_then(|window| window.active)
        };
        match id {
            "tab.new" => self.operation_open(),
            "window.newPrivate" => self.operation_enter_private(),
            "window.closePrivate" => self.operation_close_private(),
            "tab.close" if in_work => self.operation_work_pane_hide(),
            "tab.close" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_close(id),
            ),
            "work.pane.close" => self.operation_work_pane_hide(),
            "work.pane.openInBrowse" => active.filter(|_| in_work).map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged),
                |id| self.operation_activate(id),
            ),
            "tab.reopen" => self.operation_reopen_closed_tab(),
            "tab.next" => self.operation_cycle_tab(1),
            "tab.previous" => self.operation_cycle_tab(-1),
            "nav.back" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_history(id, false),
            ),
            "nav.forward" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_history(id, true),
            ),
            "nav.reload" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| self.operation_reload(id),
            ),
            "nav.stop" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| {
                    let mut native = NativeWork::default();
                    native.record(self.engine.stop(id));
                    mutation_result(native)
                },
            ),
            "tab.select.last" => self.operation_select_tab(None),
            id if id.starts_with("tab.select.") => id["tab.select.".len()..]
                .parse::<usize>()
                .ok()
                .filter(|position| (1..=8).contains(position))
                .map_or_else(
                    || {
                        operation_result(
                            OperationOutcome::Rejected,
                            OperationReason::UnsupportedCommand,
                        )
                    },
                    |position| self.operation_select_tab(Some(position)),
                ),
            "bookmark.add" => self.operation_bookmark_page(None),
            "page.print" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| {
                    let mut native = NativeWork::default();
                    native.record(self.engine.print(id));
                    mutation_result(native)
                },
            ),
            "page.devtools" => active.map_or_else(
                || operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow),
                |id| {
                    let mut native = NativeWork::default();
                    native.record(self.engine.open_devtools(id));
                    mutation_result(native)
                },
            ),
            "zoom.in" => self.operation_adjust_zoom(Some(0.1)),
            "zoom.out" => self.operation_adjust_zoom(Some(-0.1)),
            "zoom.reset" => self.operation_adjust_zoom(None),
            "url.focus" => {
                (self.emit)(Projection::UiCommand("url.focus".into()));
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            _ => operation_result(
                OperationOutcome::Rejected,
                OperationReason::UnsupportedCommand,
            ),
        }
    }

    /// Restores the newest tab closed in the focused window's space. Nothing
    /// to restore is a no-op, not a failure.
    fn operation_reopen_closed_tab(&mut self) -> OperationDisposition {
        if self.active_browser_page().is_some() {
            self.browser_after_return = Some(Box::new(Command::Run("tab.reopen".into())));
            return self.operation_show_browser_page(None);
        }
        let Some(profile) = self.windows.focused().map(|window| window.profile) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        match self.restore_recently_closed_tab(profile) {
            Some((_, native)) => mutation_result(native),
            None => operation_result(OperationOutcome::NoOp, OperationReason::MutationApplied),
        }
    }

    fn operation_cycle_tab(&mut self, step: isize) -> OperationDisposition {
        let Some(win) = self.windows.focused() else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let Some(active) = win.active else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let tabs = self.keyboard_tabs(win.profile, win.space);
        let Some(position) = tabs.iter().position(|id| *id == active) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        if tabs.len() < 2 {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let next = (position as isize + step).rem_euclid(tabs.len() as isize) as usize;
        self.operation_activate(tabs[next])
    }

    /// Selects by sidebar position, counting from one. `None` is the last tab,
    /// however many there are.
    fn operation_select_tab(&mut self, position: Option<usize>) -> OperationDisposition {
        let Some(win) = self.windows.focused() else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let tabs = self.keyboard_tabs(win.profile, win.space);
        let target = match position {
            Some(position) => position.checked_sub(1).and_then(|index| tabs.get(index)),
            None => tabs.last(),
        };
        match target {
            Some(id) if win.active != Some(*id) => self.operation_activate(*id),
            _ => operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged),
        }
    }

    fn operation_adjust_zoom(&mut self, delta: Option<f64>) -> OperationDisposition {
        let Some(active) = self.windows.focused().and_then(|window| window.active) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::NoFocusedWindow);
        };
        let Some(settled) = self.items.tab(active).map(|tab| tab.zoom) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        let current = self
            .zoom
            .pending
            .get(&active)
            .map_or(settled, |pending| pending.desired_scale);
        let zoom = match delta {
            Some(delta) => (current + delta).clamp(0.3, 3.0),
            None => 1.0,
        };
        if (zoom - current).abs() < f64::EPSILON {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        // `Items.zoom` is durable authoritative state. Keep the responsive
        // desired value in a separate bounded map until the exact native view
        // generation reports what it actually applied; otherwise any
        // unrelated session save could persist an optimistic lie.
        let admission = self.request_zoom(active, zoom);
        let mut native = NativeWork::default();
        native.record(admission);
        mutation_result(native)
    }
}
