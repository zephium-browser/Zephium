//! Shell-owned settlement of native WebExtension browser mutations.

use super::*;

impl Shell {
    pub(super) fn on_extension_browser_request(&mut self, request: ExtensionBrowserRequest) {
        let profile = request.profile();
        let id = request.id();
        // Only macOS can revoke a created tab whose first navigation was lost.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut settlement = if !self.bootstrapped
            || !self.extension_browser_surfaces.is_active(profile)
            || self.profile_deletion_quarantines(profile)
        {
            ExtensionBrowserRequestSettlement::Rejected(
                ExtensionBrowserRequestRejection::InvalidContext,
            )
        } else {
            if self.active_browser_page().is_some()
                && !self.browser_return_ready
                && matches!(
                    request.action(),
                    ExtensionBrowserRequestAction::CreateTab { .. }
                )
            {
                if self.browser_after_return.is_some() {
                    let _ = self.engine.settle_extension_browser_request(
                        profile,
                        id,
                        ExtensionBrowserRequestSettlement::Rejected(
                            ExtensionBrowserRequestRejection::CapacityExceeded,
                        ),
                        None,
                    );
                    return;
                }
                self.browser_after_return = Some(Box::new(Command::Engine(
                    EngineEvent::ExtensionBrowserRequested { request },
                )));
                let _ = self.operation_show_browser_page(None);
                return;
            }
            match request.action() {
                ExtensionBrowserRequestAction::OpenExtensionPage => {
                    self.extension_open_page(profile)
                }
                ExtensionBrowserRequestAction::CreateTab {
                    window,
                    url,
                    active,
                } => self.extension_create_tab(profile, *window, url.as_deref(), *active),
                ExtensionBrowserRequestAction::ActivateTab { tab } => {
                    self.extension_activate_tab(profile, *tab)
                }
                ExtensionBrowserRequestAction::CloseTab { tab } => {
                    self.extension_close_tab(profile, *tab)
                }
                ExtensionBrowserRequestAction::CloseTabIfUnchanged {
                    tab,
                    navigation,
                    url,
                    cleanup,
                } => {
                    if !cleanup.is_owned()
                        || self.items.pending_navigation_request(*tab).is_some()
                        || self.presentation.pending_presentations.contains_key(tab)
                        || !self
                            .presentation
                            .presented_navigations
                            .get(tab)
                            .is_some_and(|(current, _)| current == navigation)
                        || !self.items.tab(*tab).is_some_and(|state| {
                            state.has_view()
                                && state
                                    .url
                                    .as_ref()
                                    .is_some_and(|current| current.as_str() == url.as_ref())
                        })
                    {
                        rejected(ExtensionBrowserRequestRejection::InvalidScope)
                    } else {
                        self.extension_close_tab(profile, *tab)
                    }
                }
                ExtensionBrowserRequestAction::CloseTabIfPristine { tab } => {
                    if self.items.pending_navigation_request(*tab).is_none()
                        && !self.presentation.pending_presentations.contains_key(tab)
                        && self
                            .items
                            .tab(*tab)
                            .is_some_and(|state| !state.has_view() && state.url.is_none())
                    {
                        self.extension_close_tab(profile, *tab)
                    } else {
                        rejected(ExtensionBrowserRequestRejection::InvalidScope)
                    }
                }
                ExtensionBrowserRequestAction::LoadTabUrl { tab, url } => {
                    self.extension_load_tab_url(profile, *tab, url)
                }
                ExtensionBrowserRequestAction::ReloadTab { tab } => {
                    self.extension_reload_tab(profile, *tab)
                }
                ExtensionBrowserRequestAction::GoBack { tab } => {
                    self.extension_traverse_history(profile, *tab, false)
                }
                ExtensionBrowserRequestAction::GoForward { tab } => {
                    self.extension_traverse_history(profile, *tab, true)
                }
                ExtensionBrowserRequestAction::DuplicateTab { tab } => {
                    self.extension_duplicate_tab(profile, *tab)
                }
                ExtensionBrowserRequestAction::SetTabMuted { tab, muted } => {
                    self.extension_set_tab_muted(profile, *tab, *muted)
                }
                ExtensionBrowserRequestAction::SetTabZoom { tab, percent } => {
                    self.extension_set_tab_zoom(profile, *tab, *percent)
                }
                ExtensionBrowserRequestAction::MoveTabs { tabs, before } => {
                    self.extension_move_tabs(profile, tabs, *before)
                }
            }
        };

        #[cfg(target_os = "macos")]
        let first_url_after_reply = match (request.action(), settlement) {
            (
                ExtensionBrowserRequestAction::CreateTab { url: Some(url), .. },
                ExtensionBrowserRequestSettlement::Applied(
                    ExtensionBrowserRequestResult::CreatedTab(tab),
                ),
            ) => {
                match self.items.reserve_deferred_navigation(tab) {
                    Some(intent) => Some((url.clone(), intent)),
                    None => {
                        crate::diagnostic!("extensions: deferred first tab navigation lost its exact logical marker");
                        let _ = self.close(tab);
                        settlement = ExtensionBrowserRequestSettlement::Rejected(
                            ExtensionBrowserRequestRejection::NativeAdmissionFailed,
                        );
                        None
                    }
                }
            }
            _ => None,
        };
        #[cfg(not(target_os = "macos"))]
        let first_url_after_reply = None;
        let dispatch = self.engine.settle_extension_browser_request(
            profile,
            id,
            settlement,
            first_url_after_reply,
        );
        if let ExtensionBrowserRequestSettlement::Applied(
            ExtensionBrowserRequestResult::ExtensionPageAuthorized { tab, .. },
        ) = settlement
        {
            if dispatch == NativeDispatch::Scheduled {
                self.items.adopt_extension_view(tab);
                let _ = self.operation_activate(tab);
            } else {
                self.close_extension_owned_marker(profile, tab);
            }
        }
        if dispatch != NativeDispatch::Scheduled {
            // The native broker owns an independent exact-once timeout, so a
            // saturated response queue cannot leave WebKit waiting forever.
            crate::diagnostic!("extensions: native browser request settlement was not admitted");
        }
    }

    fn extension_open_page(&mut self, profile: ProfileId) -> ExtensionBrowserRequestSettlement {
        let Some(window) = self
            .windows
            .focused()
            .filter(|window| window.profile == profile)
        else {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        };
        let window_id = window.id;
        let placement = Placement::Space {
            space: window.space,
            section: SpaceSection::Today,
        };
        let tab = ItemId::generate();
        if !self.items.insert_extension_tab(tab, placement) {
            return rejected(ExtensionBrowserRequestRejection::CapacityExceeded);
        }
        if self.commit(Vec::new()).rejected {
            self.close_extension_owned_marker(profile, tab);
            return rejected(ExtensionBrowserRequestRejection::NativeAdmissionFailed);
        }
        ExtensionBrowserRequestSettlement::Applied(
            ExtensionBrowserRequestResult::ExtensionPageAuthorized {
                tab,
                window: window_id,
            },
        )
    }

    fn extension_create_tab(
        &mut self,
        profile: ProfileId,
        requested_window: Option<WindowId>,
        url: Option<&str>,
        active: bool,
    ) -> ExtensionBrowserRequestSettlement {
        // The current product model owns one extension-visible window per
        // profile and exposes no background-window focus primitive. Refuse an
        // ambiguous or inactive creation rather than mutating whichever
        // window happens to be focused.
        let Some(window) = self.windows.focused() else {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        };
        if window.profile != profile
            || requested_window.is_some_and(|requested| requested != window.id)
        {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        if !active {
            return rejected(ExtensionBrowserRequestRejection::Unsupported);
        }
        #[allow(unused_mut)]
        let Some((tab, mut effects)) = self.open_tab_with_id() else {
            return rejected(ExtensionBrowserRequestRejection::CapacityExceeded);
        };
        #[cfg(not(target_os = "macos"))]
        if let Some(url) = url {
            effects.extend(self.items.navigate(tab, url));
        }
        #[cfg(target_os = "macos")]
        let _ = url;
        let native = self.commit(effects);
        if native.rejected {
            // The logical tab has been created, but its first URL is still
            // held behind the native tabs.create reply. Reconcile any failed
            // surface publication before the broker looks up its tab.
            let _ = self.sync_extension_browser_surfaces();
        }
        ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::CreatedTab(tab))
    }

    pub(super) fn on_extension_created_tab_replied(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        url: Arc<str>,
        intent: zephium_core::ports::engine::NavigationRequestId,
    ) {
        if !self.bootstrapped
            || self.profile_deletion_quarantines(profile)
            || !self.extension_tab_in_scope(tab, profile)
            || self.items.pending_navigation_request(tab) != Some(intent)
            || !self.items.tab(tab).is_some_and(|state| {
                state.content == zephium_core::item::TabContent::Web
                    && !state.has_view()
                    && state.url.is_none()
            })
        {
            return;
        }
        let effects = self.items.navigate(tab, &url);
        if effects.len() == 1 {
            self.commit(effects);
        }
    }

    fn extension_activate_tab(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
    ) -> ExtensionBrowserRequestSettlement {
        if !self.extension_tab_in_scope(tab, profile) || !self.item_in_focused_scope(tab) {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        let _ = self.operation_activate(tab);
        applied()
    }

    fn extension_close_tab(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
    ) -> ExtensionBrowserRequestSettlement {
        if !self.extension_tab_in_scope(tab, profile) {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        self.close_in_any_space(tab);
        applied()
    }

    fn extension_load_tab_url(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        url: &str,
    ) -> ExtensionBrowserRequestSettlement {
        if !self.extension_tab_in_scope(tab, profile) {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        let effects = self.items.navigate(tab, url);
        if effects.is_empty() {
            return rejected(ExtensionBrowserRequestRejection::InvalidRequest);
        }
        let native = self.commit(effects);
        if native.rejected {
            rejected(ExtensionBrowserRequestRejection::NativeAdmissionFailed)
        } else {
            applied()
        }
    }

    fn extension_reload_tab(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
    ) -> ExtensionBrowserRequestSettlement {
        let Some(state) = self.extension_resident_tab(profile, tab) else {
            return self.extension_tab_mutation_refusal(profile, tab);
        };
        debug_assert!(state.has_view());
        settle_native_dispatch(self.engine.reload(tab))
    }

    fn extension_traverse_history(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        forward: bool,
    ) -> ExtensionBrowserRequestSettlement {
        let Some(state) = self.extension_resident_tab(profile, tab) else {
            return self.extension_tab_mutation_refusal(profile, tab);
        };
        let available = if forward {
            state.can_go_forward
        } else {
            state.can_go_back
        };
        if !available {
            return rejected(ExtensionBrowserRequestRejection::InvalidRequest);
        }
        settle_native_dispatch(if forward {
            self.engine.go_forward(tab)
        } else {
            self.engine.go_back(tab)
        })
    }

    fn extension_duplicate_tab(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
    ) -> ExtensionBrowserRequestSettlement {
        if !self.extension_tab_in_scope(tab, profile) || !self.item_in_focused_scope(tab) {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        match self.duplicate_tab(tab) {
            Ok((copy, native)) if !native.rejected => ExtensionBrowserRequestSettlement::Applied(
                ExtensionBrowserRequestResult::CreatedTab(copy),
            ),
            Ok(_) => rejected(ExtensionBrowserRequestRejection::NativeAdmissionFailed),
            Err(disposition) => settle_disposition(&disposition),
        }
    }

    fn extension_set_tab_muted(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        muted: bool,
    ) -> ExtensionBrowserRequestSettlement {
        if !self.extension_tab_in_scope(tab, profile) {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        settle_disposition(&self.operation_set_tab_muted(tab, muted))
    }

    /// Sets the level a person would by zooming the tab, so its site keeps it.
    fn extension_set_tab_zoom(
        &mut self,
        profile: ProfileId,
        tab: ItemId,
        percent: Option<u16>,
    ) -> ExtensionBrowserRequestSettlement {
        let percent = percent.unwrap_or(self.zoom.default_percent);
        if !zephium_core::zoom::valid_percent(percent) {
            return rejected(ExtensionBrowserRequestRejection::InvalidRequest);
        }
        let Some(state) = self.extension_resident_tab(profile, tab) else {
            return self.extension_tab_mutation_refusal(profile, tab);
        };
        if state.content != zephium_core::item::TabContent::Web {
            return rejected(ExtensionBrowserRequestRejection::InvalidRequest);
        }
        let current = self
            .zoom
            .pending
            .get(&tab)
            .map_or(state.zoom, |pending| pending.desired_scale);
        let target = zephium_core::zoom::scale(percent);
        if zephium_core::zoom::percent(current) == percent {
            return applied();
        }
        settle_native_dispatch(self.request_zoom(tab, target, super::zoom::ZoomOrigin::Person))
    }

    /// Reorders the day's tabs of the focused window; pinned tabs and tabs
    /// in folders keep their places.
    fn extension_move_tabs(
        &mut self,
        profile: ProfileId,
        tabs: &[ItemId],
        before: Option<ItemId>,
    ) -> ExtensionBrowserRequestSettlement {
        let Some(space) = self
            .windows
            .focused()
            .filter(|window| window.profile == profile)
            .map(|window| window.space)
        else {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        };
        if tabs
            .iter()
            .chain(&before)
            .any(|tab| !self.extension_tab_in_scope(*tab, profile))
        {
            return rejected(ExtensionBrowserRequestRejection::InvalidScope);
        }
        let today = Placement::Space {
            space,
            section: SpaceSection::Today,
        };
        let roots = self.items.roots(today);
        if tabs.iter().chain(&before).any(|tab| !roots.contains(tab))
            || before.is_some_and(|before| tabs.contains(&before))
        {
            return rejected(ExtensionBrowserRequestRejection::Unsupported);
        }
        for tab in tabs {
            if !self.items.move_tab_to_root(*tab, today, before) {
                return rejected(ExtensionBrowserRequestRejection::InvalidRequest);
            }
        }
        if self.commit(Vec::new()).rejected {
            rejected(ExtensionBrowserRequestRejection::NativeAdmissionFailed)
        } else {
            applied()
        }
    }

    fn extension_resident_tab(&self, profile: ProfileId, tab: ItemId) -> Option<&TabState> {
        if !self.extension_tab_in_scope(tab, profile) {
            return None;
        }
        let state = self.items.tab(tab)?;
        if !state.has_view()
            || matches!(
                self.residency.discard_probes.get(&tab),
                Some(PendingDiscardProbe::Closing { .. })
            )
        {
            return None;
        }
        Some(state)
    }

    fn extension_tab_mutation_refusal(
        &self,
        profile: ProfileId,
        tab: ItemId,
    ) -> ExtensionBrowserRequestSettlement {
        if self.extension_tab_in_scope(tab, profile) {
            rejected(ExtensionBrowserRequestRejection::TabDiscarded)
        } else {
            rejected(ExtensionBrowserRequestRejection::InvalidScope)
        }
    }
}

const fn settle_native_dispatch(dispatch: NativeDispatch) -> ExtensionBrowserRequestSettlement {
    match dispatch {
        NativeDispatch::Scheduled => applied(),
        NativeDispatch::Rejected => {
            rejected(ExtensionBrowserRequestRejection::NativeAdmissionFailed)
        }
        NativeDispatch::Unsupported => rejected(ExtensionBrowserRequestRejection::Unsupported),
    }
}

fn settle_disposition(disposition: &OperationDisposition) -> ExtensionBrowserRequestSettlement {
    match (disposition.outcome, disposition.reason) {
        (OperationOutcome::Applied | OperationOutcome::NoOp | OperationOutcome::Deferred, _)
            if disposition.reason != OperationReason::InvalidInput =>
        {
            applied()
        }
        (OperationOutcome::NativeAdmissionFailed, _) => {
            rejected(ExtensionBrowserRequestRejection::NativeAdmissionFailed)
        }
        (_, OperationReason::UnsupportedCommand) => {
            rejected(ExtensionBrowserRequestRejection::Unsupported)
        }
        (_, OperationReason::InvalidScope | OperationReason::NoFocusedWindow) => {
            rejected(ExtensionBrowserRequestRejection::InvalidScope)
        }
        (_, OperationReason::ItemLimitReached) => {
            rejected(ExtensionBrowserRequestRejection::CapacityExceeded)
        }
        _ => rejected(ExtensionBrowserRequestRejection::InvalidRequest),
    }
}

const fn applied() -> ExtensionBrowserRequestSettlement {
    ExtensionBrowserRequestSettlement::Applied(ExtensionBrowserRequestResult::Complete)
}

const fn rejected(reason: ExtensionBrowserRequestRejection) -> ExtensionBrowserRequestSettlement {
    ExtensionBrowserRequestSettlement::Rejected(reason)
}
