//! The single native engine-event fold into authoritative shell state.

use super::*;

fn valid_native_split_update(current: &Pane, candidate: &Pane) -> bool {
    match (current, candidate) {
        (Pane::Leaf(expected), Pane::Leaf(actual)) => expected == actual,
        (
            Pane::Branch {
                axis: expected_axis,
                a: expected_a,
                b: expected_b,
                ..
            },
            Pane::Branch { axis, ratio, a, b },
        ) => {
            axis == expected_axis
                && ratio.is_finite()
                && (0.05..=0.95).contains(ratio)
                && valid_native_split_update(expected_a, a)
                && valid_native_split_update(expected_b, b)
        }
        _ => false,
    }
}

impl Shell {
    pub(super) fn on_engine_event(&mut self, event: EngineEvent) {
        // Except for the process-global runtime signal, every engine event is
        // evidence about native state that bootstrap owns. Reject the whole
        // cohort until bootstrap has completed: loading completion can warm a
        // renderer, while crash/process-exit recovery can create replacement
        // views. Neither is safe before the session has been restored.
        if !self.bootstrapped {
            match &event {
                EngineEvent::RuntimeRestartRequired => {}
                EngineEvent::ExtensionBrowserRequested { request } => {
                    self.on_extension_browser_request(request.clone());
                    return;
                }
                EngineEvent::ExtensionPageClosed { profile, id } => {
                    self.close_extension_owned_marker(*profile, *id);
                    return;
                }
                EngineEvent::PermissionRequested {
                    id,
                    profile,
                    request,
                } => {
                    self.on_page_permission_request(*profile, *id, request.clone());
                    return;
                }
                _ => {
                    crate::diagnostic!("engine: ignored native event before bootstrap");
                    return;
                }
            }
        }
        // Retirement may already have installed a permanent extension-worker
        // fence even when its caller observed only a retryable result. From
        // deletion admission onward, stale native facts for that profile may
        // neither mutate actor state nor recreate a renderer. Process-global
        // runtime status remains independent of any profile quarantine.
        if self.engine_event_targets_quarantined_profile(&event) {
            if let EngineEvent::PermissionRequested {
                id,
                profile,
                request,
            } = &event
            {
                let _ = self.engine.settle_page_permission_request(
                    *profile,
                    *id,
                    request.id,
                    zephium_core::permissions::PagePermissionRequestSettlement::Deny,
                );
            }
            crate::diagnostic!("engine: ignored native event for quarantined profile");
            return;
        }
        // Native navigation/teardown independently revokes its retained
        // completion. Close the browser-owned surface in the same actor turn
        // so stale consent never follows a document, renderer, or profile.
        match &event {
            EngineEvent::UrlChanged { id, .. }
            | EngineEvent::PresentationPending { id, .. }
            | EngineEvent::PresentationReady { id, .. }
            | EngineEvent::NavigationFailed { id, .. }
            | EngineEvent::NavigationFailureReported { id, .. }
            | EngineEvent::ViewCreationFailed { id }
            | EngineEvent::Crashed { id }
            | EngineEvent::ViewDiscarded { id, .. } => {
                self.cancel_page_permission_for_item(*id);
                let capture_invalidated = matches!(
                    &event,
                    EngineEvent::ViewCreationFailed { .. }
                        | EngineEvent::Crashed { .. }
                        | EngineEvent::ViewDiscarded { .. }
                );
                if capture_invalidated {
                    self.items.set_media_capture(*id, None);
                    // A replacement view starts out of fullscreen.
                    self.forget_content_fullscreen(*id);
                }
            }
            EngineEvent::ProfileProcessExited { profile, .. } => {
                self.cancel_page_permission_for_profile(*profile);
            }
            _ => {}
        }
        match event {
            EngineEvent::RuntimeRestartRequired => {
                if !self.runtime_restart_required {
                    self.runtime_restart_required = true;
                    self.project_runtime_status();
                }
            }
            EngineEvent::ContentRulesSettled {
                profile,
                requested,
                settlement,
            } => self.on_content_rules_settled(profile, requested, settlement),
            EngineEvent::UserContentSettled {
                scope,
                requested,
                settlement,
            } => {
                if matches!(scope, ContentScope::Profile(profile) if self.profiles.get(profile).is_none())
                {
                    crate::diagnostic!(
                        "engine: ignored user-content settlement for unknown profile"
                    );
                    return;
                }
                let observation = self
                    .user_content_status
                    .observe(scope, requested, &settlement);
                if !matches!(
                    observation,
                    user_content_status::UserContentObservation::Unchanged
                        | user_content_status::UserContentObservation::Stale
                ) {
                    self.project_runtime_status();
                }
                if !matches!(
                    settlement,
                    zephium_core::ports::engine::UserContentSettlement::Applied { generation }
                        if generation == requested
                ) {
                    crate::diagnostic!(
                        "engine: user-content generation {} for {scope:?} was not applied: {settlement:?}",
                        requested.get()
                    );
                }
                if matches!(
                    observation,
                    user_content_status::UserContentObservation::Contradictory
                        | user_content_status::UserContentObservation::CapacityExceeded
                ) {
                    crate::diagnostic!(
                        "engine: contradictory or over-capacity user-content settlement"
                    );
                }
            }
            EngineEvent::ExtensionBrowserRequested { request } => {
                self.on_extension_browser_request(request)
            }
            EngineEvent::ExtensionCreatedTabReplied {
                profile,
                request: _,
                tab,
                url,
                intent,
            } => self.on_extension_created_tab_replied(profile, tab, url, intent),
            EngineEvent::ExtensionPageClosed { profile, id } => {
                self.close_extension_owned_marker(profile, id)
            }
            EngineEvent::ExtensionPageChanged {
                profile,
                id,
                title,
                loading,
                can_go_back,
                can_go_forward,
            } => {
                if self.profile_of_item(id) == Some(profile)
                    && self.items.tab(id).is_some_and(|tab| {
                        tab.content == zephium_core::item::TabContent::ExtensionOwned
                    })
                {
                    self.items.set_title(id, title);
                    self.items.set_loading(id, loading);
                    self.items.set_nav_flags(id, can_go_back, can_go_forward);
                    self.project_tab(id);
                    self.sync_extension_browser_surface_metadata(id);
                }
            }
            EngineEvent::ExtensionActionsSnapshotSettled {
                profile,
                tab,
                surface_generation,
                settlement,
            } => {
                let observation = self.extension_actions.observe(
                    self.extension_browser_surfaces.published_surface(profile),
                    profile,
                    tab,
                    surface_generation,
                    settlement,
                );
                if matches!(
                    observation,
                    extension_actions::ExtensionActionObservation::Applied
                ) {
                    self.project_extension_actions(profile);
                }
                if matches!(
                    observation,
                    extension_actions::ExtensionActionObservation::Retained
                ) {
                    crate::diagnostic!(
                        "extensions: native toolbar action refresh failed; retaining prior state"
                    );
                }
                if matches!(
                    observation,
                    extension_actions::ExtensionActionObservation::Contradictory
                        | extension_actions::ExtensionActionObservation::CapacityExceeded
                ) {
                    crate::diagnostic!(
                        "extensions: contradictory or over-capacity toolbar action settlement"
                    );
                }
            }
            EngineEvent::ExtensionActionSettled {
                profile,
                request,
                settlement,
            } => {
                let observation = self
                    .extension_actions
                    .settle_invocation(profile, request, settlement);
                if let extension_actions::ExtensionActionInvocationObservation::Rejected {
                    tab,
                    reason,
                } = observation
                {
                    crate::diagnostic!(
                        "extensions: toolbar action rejected with typed reason {reason:?}"
                    );
                    self.project_extension_action_failure(profile, Some(tab), reason);
                    if matches!(
                        reason,
                        zephium_core::extensions::ExtensionActionRejection::RuntimeSuperseded
                            | zephium_core::extensions::ExtensionActionRejection::RuntimeUnavailable
                            | zephium_core::extensions::ExtensionActionRejection::ActionUnavailable
                            | zephium_core::extensions::ExtensionActionRejection::ActionDisabled
                    ) {
                        let _ = self.refresh_extension_actions(profile);
                    }
                }
                if matches!(
                    observation,
                    extension_actions::ExtensionActionInvocationObservation::Contradictory
                ) {
                    crate::diagnostic!(
                        "extensions: toolbar action settlement crossed profile authority"
                    );
                }
            }
            EngineEvent::WebExtensionSettled {
                profile,
                install,
                result,
            } => self.on_web_extension_settled(profile, install, result),
            EngineEvent::WebExtensionAccessRequested(request) => {
                let request = *request;
                (self.emit)(Projection::WebExtensionAccessRequest(
                    zephium_ipc::WebExtensionAccessRequestView {
                        profile_id: request.profile.to_string(),
                        request: request.request.to_string(),
                        extension_id: request.extension_id,
                        warnings: request.warnings,
                        permissions: request.permissions,
                        patterns: request.patterns,
                    },
                ));
            }
            EngineEvent::ExtensionActionsInvalidated { profile } => {
                let refresh = self.refresh_extension_actions(profile);
                if refresh.rejected {
                    crate::diagnostic!(
                        "extensions: native action invalidation awaits maintenance retry"
                    );
                }
            }
            EngineEvent::ExtensionActionShortcutRequested {
                runtime,
                tab,
                surface_generation,
            } => match self.extension_actions.shortcut_action_revision(
                self.extension_browser_surfaces
                    .published_surface(runtime.profile()),
                runtime,
                tab,
                surface_generation,
            ) {
                Ok(revision) => {
                    self.project_extension_action_shortcut(runtime, tab, revision);
                }
                Err(reason) => {
                    crate::diagnostic!(
                        "extensions: action shortcut rejected with typed reason {reason:?}"
                    );
                    self.project_extension_action_failure(runtime.profile(), Some(tab), reason);
                    if matches!(
                        reason,
                        zephium_core::extensions::ExtensionActionRejection::RuntimeSuperseded
                            | zephium_core::extensions::ExtensionActionRejection::RuntimeUnavailable
                            | zephium_core::extensions::ExtensionActionRejection::ActionUnavailable
                            | zephium_core::extensions::ExtensionActionRejection::ActionDisabled
                    ) {
                        let _ = self.refresh_extension_actions(runtime.profile());
                    }
                }
            },
            EngineEvent::SplitChanged { window, tree } => {
                // Native divider drags may update ratios only. Never let a
                // stale or malformed callback mutate topology, swap tabs, or
                // inject a non-finite layout value or cross-window item into
                // Rust-owned state.
                let valid = self
                    .windows
                    .get(window)
                    .and_then(|win| {
                        win.splits.as_ref().map(|current| {
                            valid_native_split_update(current, &tree)
                                && self.pane_in_scope(current, win.profile, win.space)
                                && self.pane_in_scope(&tree, win.profile, win.space)
                        })
                    })
                    .unwrap_or(false);
                if valid {
                    if let Some(win) = self.windows.get_mut(window) {
                        win.splits = Some(tree);
                    }
                    self.schedule_persist();
                    let _ = self.relayout();
                } else {
                    crate::diagnostic!("engine: rejected invalid native split tree");
                    let _ = self.relayout();
                }
            }
            EngineEvent::NavState {
                id,
                can_go_back,
                can_go_forward,
            } => {
                self.items.set_nav_flags(id, can_go_back, can_go_forward);
                self.project_tab(id);
            }
            EngineEvent::NativeTabOpened {
                id,
                child,
                foreground,
                adoption,
            } => self.adopt_linked_native_tab(id, child, foreground, adoption),
            EngineEvent::NativeTabCloseRequested { id } => self.close_owned_native_tab(id),
            EngineEvent::PageOpenBlocked { id, url } => {
                let url = url
                    .and_then(|url| url::Url::parse(&url).ok())
                    .filter(|url| url.as_str() != "about:blank" && navigation::is_allowed(url));
                if self
                    .items
                    .set_page_request(id, zephium_core::item::PageRequest::Popup { url })
                {
                    self.project_tab(id);
                }
            }
            EngineEvent::ExternalAppRequested { id, url, app } => {
                self.on_external_app_requested(id, url, app)
            }
            EngineEvent::FocusBlocked { id, url } => self.on_focus_blocked(id, url),
            EngineEvent::LinkedDownloadStarted { id } => {
                if self.items.tab(id).is_some_and(|tab| {
                    tab.url
                        .as_ref()
                        .is_none_or(|url| url.as_str() == "about:blank")
                }) {
                    self.close_owned_native_tab(id);
                }
            }
            EngineEvent::NewWindowRequested { id, url } => self.open_linked_tab(id, &url),
            EngineEvent::WorkPageFavicon {
                profile,
                page_url,
                rgba,
            } => self.work_page_favicon(profile, &page_url, rgba),
            EngineEvent::FaviconPixels { id, page_url, rgba } => {
                self.favicon_pixels(id, &page_url, rgba);
            }
            EngineEvent::DiscardSafety {
                id,
                probe,
                can_discard,
            } => self.on_discard_safety(id, probe, can_discard),
            EngineEvent::ViewDiscarded { id, profile, probe } => {
                self.on_view_discarded(id, profile, probe)
            }
            EngineEvent::ViewDiscardRefused { id, profile, probe } => {
                self.on_view_discard_refused(id, profile, probe)
            }
            EngineEvent::PageMemory { id, profile, bytes } => {
                self.on_page_memory(id, profile, bytes)
            }
            EngineEvent::MediaCaptureChanged {
                id,
                navigation,
                state,
            } => {
                let current_document = self
                    .presentation
                    .pending_presentations
                    .get(&id)
                    .map(|pending| pending.navigation)
                    .or_else(|| {
                        self.presentation
                            .presented_navigations
                            .get(&id)
                            .map(|(current, _)| *current)
                    });
                if current_document == Some(navigation)
                    && self
                        .items
                        .set_media_capture(id, state.is_capturing().then_some((navigation, state)))
                {
                    self.project_tab(id);
                }
            }
            EngineEvent::FullscreenChanged { id, active } => self.on_fullscreen_changed(id, active),
            EngineEvent::PermissionRequested {
                id,
                profile,
                request,
            } => {
                self.on_page_permission_request(profile, id, request);
            }
            EngineEvent::DownloadRequested { .. } => {}
            EngineEvent::PresentationPending {
                id,
                navigation,
                url,
            }
            | EngineEvent::PresentationReady {
                id,
                navigation,
                url,
            } => self.on_presentation_fact(id, navigation, url),
            EngineEvent::NavigationFailed { id, request } => {
                // Only the matching latest intent is affected. The displayed
                // URL was never changed optimistically, so a stale native
                // rejection cannot roll chrome or persistence forward or back.
                if self.items.navigation_failed(id, request) {
                    self.project_tab(id);
                }
            }
            EngineEvent::NavigationFailureReported { id, reason } => {
                if self.items.record_navigation_failure(id, reason) {
                    self.project_tab(id);
                }
            }
            EngineEvent::ZoomSettled {
                id,
                request,
                applied_scale,
                succeeded,
            } => self.on_zoom_settled(id, request, applied_scale, succeeded),
            EngineEvent::NativeActionFailed { id, action } => {
                if self.items.tab(id).is_some_and(TabState::has_view) {
                    let action = match action {
                        NativeAction::Reload => "reload",
                        NativeAction::GoBack => "back",
                        NativeAction::GoForward => "forward",
                    };
                    // The engine separately re-observes authoritative
                    // source/history. Keep this diagnostic bounded and never
                    // include a page-derived URL or native error string.
                    crate::diagnostic!("engine: native {action} action failed");
                }
            }
            EngineEvent::ViewCreationFailed { id } => {
                self.on_view_creation_failed(id);
            }
            EngineEvent::ProfileProcessExited { profile, ids } => {
                self.on_profile_process_exit(profile, ids)
            }
            EngineEvent::Crashed { id } => self.on_crashed(id),
            EngineEvent::Captured { .. } => {}
            EngineEvent::HtmlExtracted { .. } => {}
            EngineEvent::FindResult {
                id,
                query,
                matches,
                active,
            } => {
                // Only the page being searched may answer; a result from a
                // page left behind would describe matches no one can see.
                if self.find_target == Some(id) {
                    (self.emit)(Projection::FindResult(zephium_ipc::FindResultView {
                        query,
                        matches,
                        active,
                    }));
                }
            }
            EngineEvent::ShortcutPressed { .. } => {}
            EngineEvent::TitleChanged { id, title } => {
                self.crash.presentations.remove(&id);
                self.items.set_title(id, title.clone());
                self.amend_recorded_visit_title(id, &title);
                self.project_tab(id);
                self.sync_extension_browser_surface_metadata(id);
            }
            EngineEvent::LoadingChanged { id, loading } => {
                // Loading callbacks belong to an exact live native view. A
                // queued callback for a closed/discarded or unknown item must
                // not mutate shell state, start favicon work, or warm a
                // renderer using a fallback partition.
                if !self.items.tab(id).is_some_and(TabState::has_view) {
                    crate::diagnostic!("engine: ignored loading event for unknown native view");
                    return;
                }
                if loading {
                    self.cancel_discard_probe(id);
                    self.crash.presentations.remove(&id);
                }
                self.items.set_loading(id, loading);
                self.project_tab(id);
                if !loading {
                    // The first URL observation may precede the renderer's
                    // asynchronous image decode. Poll immediately at load
                    // completion as well as through the bounded timer.
                    self.favicon_load_completed(id);
                    self.engine.warm_spare(self.partition_of(id));
                }
                self.sync_extension_browser_surface_metadata(id);
            }
            EngineEvent::UrlChanged { id, url } => {
                self.cancel_discard_probe(id);
                let Ok(committed_url) = url::Url::parse(&url) else {
                    crate::diagnostic!("engine: rejected invalid or unknown committed URL event");
                    return;
                };
                if !navigation::is_browser_target(&committed_url) {
                    crate::diagnostic!("engine: rejected invalid or unknown committed URL event");
                    return;
                }
                let first_committed_url = self.items.tab(id).is_some_and(|tab| tab.url.is_none());
                let replace_stale_title = self
                    .items
                    .tab(id)
                    .and_then(|tab| tab.url.as_ref())
                    .is_none_or(|previous| !same_browser_origin(previous, &committed_url));
                if !self.items.set_committed_url(id, committed_url.clone()) {
                    crate::diagnostic!("engine: rejected invalid or unknown committed URL event");
                    return;
                }
                if replace_stale_title {
                    // Browser chrome may be projected before the new document
                    // publishes a title. Never carry a prior origin's trusted
                    // label across the exact URL acknowledgement that unlocks
                    // presentation; use a neutral URL-derived label meanwhile.
                    self.items
                        .set_title(id, neutral_title_for_url(&committed_url));
                }
                self.maybe_discover_favicon(id);
                // History is attributed to the profile that owns the item,
                // not the focused window; incognito profiles never record.
                let recording = self.profile_of_item(id).filter(|p| {
                    self.profiles
                        .get(*p)
                        .is_some_and(|x| x.kind != ProfileKind::Incognito)
                });
                if let Some(profile) = recording.filter(|_| self.should_record_visit(id, &url)) {
                    let title = self
                        .items
                        .tab(id)
                        .map(|t| t.title.clone())
                        .unwrap_or_default();
                    self.store.record_visit(profile, url, title);
                }
                self.schedule_url_checkpoint(id);
                if first_committed_url {
                    // Keep the real privileged New Tab projection and native
                    // frame until the exact presentation eval replaces it.
                    // All generic projections remain URL-free for this item,
                    // so they cannot create an empty gap ahead of that eval.
                    self.presentation.deferred_first_content_layout.insert(id);
                } else {
                    self.project_tab(id);
                }
                self.sync_extension_browser_surface_metadata(id);
            }
        }
    }

    fn sync_extension_browser_surface_metadata(&mut self, id: ItemId) {
        let Some(profile) = self.profile_of_item(id) else {
            return;
        };
        let settlement = self.sync_extension_browser_surface(profile);
        if settlement.failed(profile) {
            crate::diagnostic!(
                "extensions: native browser metadata projection was not admitted; retaining the prior generation"
            );
        }
    }

    fn engine_event_targets_quarantined_profile(&self, event: &EngineEvent) -> bool {
        let profile = match event {
            EngineEvent::RuntimeRestartRequired => None,
            EngineEvent::ContentRulesSettled { profile, .. }
            | EngineEvent::ViewDiscarded { profile, .. }
            | EngineEvent::ViewDiscardRefused { profile, .. }
            | EngineEvent::PageMemory { profile, .. }
            | EngineEvent::ProfileProcessExited { profile, .. } => Some(*profile),
            EngineEvent::UserContentSettled {
                scope: ContentScope::Profile(profile),
                ..
            } => Some(*profile),
            EngineEvent::UserContentSettled {
                scope: ContentScope::Global,
                ..
            } => None,
            // Requests must reach their handler even during retirement so the
            // retained native completion receives an explicit rejection.
            EngineEvent::ExtensionBrowserRequested { .. } => None,
            EngineEvent::ExtensionCreatedTabReplied { profile, .. } => Some(*profile),
            // Teardown must still remove a typed marker during retirement.
            EngineEvent::ExtensionPageClosed { .. } => None,
            EngineEvent::ExtensionPageChanged { profile, .. } => Some(*profile),
            EngineEvent::ExtensionActionsSnapshotSettled { profile, .. } => Some(*profile),
            EngineEvent::ExtensionActionSettled { profile, .. } => Some(*profile),
            EngineEvent::ExtensionActionsInvalidated { profile } => Some(*profile),
            EngineEvent::WebExtensionSettled { profile, .. } => Some(*profile),
            EngineEvent::WebExtensionAccessRequested(request) => Some(request.profile),
            EngineEvent::ExtensionActionShortcutRequested { runtime, .. } => {
                Some(runtime.profile())
            }
            EngineEvent::SplitChanged { window, .. } => {
                self.windows.get(*window).map(|window| window.profile)
            }
            EngineEvent::TitleChanged { id, .. }
            | EngineEvent::UrlChanged { id, .. }
            | EngineEvent::PresentationPending { id, .. }
            | EngineEvent::PresentationReady { id, .. }
            | EngineEvent::NavigationFailed { id, .. }
            | EngineEvent::NavigationFailureReported { id, .. }
            | EngineEvent::ZoomSettled { id, .. }
            | EngineEvent::NativeActionFailed { id, .. }
            | EngineEvent::LoadingChanged { id, .. }
            | EngineEvent::FaviconPixels { id, .. }
            | EngineEvent::DiscardSafety { id, .. }
            | EngineEvent::NavState { id, .. }
            | EngineEvent::NativeTabCloseRequested { id }
            | EngineEvent::PageOpenBlocked { id, .. }
            | EngineEvent::ExternalAppRequested { id, .. }
            | EngineEvent::FocusBlocked { id, .. }
            | EngineEvent::NativeTabOpened { id, .. }
            | EngineEvent::LinkedDownloadStarted { id }
            | EngineEvent::NewWindowRequested { id, .. }
            | EngineEvent::DownloadRequested { id, .. }
            | EngineEvent::ViewCreationFailed { id }
            | EngineEvent::Crashed { id }
            | EngineEvent::Captured { id, .. }
            | EngineEvent::HtmlExtracted { id, .. }
            | EngineEvent::FindResult { id, .. } => self.profile_of_item(*id),
            EngineEvent::MediaCaptureChanged { id, .. }
            | EngineEvent::FullscreenChanged { id, .. } => self.profile_of_item(*id),
            EngineEvent::PermissionRequested { profile, .. }
            | EngineEvent::WorkPageFavicon { profile, .. } => Some(*profile),
            EngineEvent::ShortcutPressed { item, .. } => self.profile_of_item(*item),
        };
        profile.is_some_and(|profile| self.profile_deletion_quarantines(profile))
    }

    /// A visit is recorded when its URL commits, which is before the document
    /// publishes a title, so the row holds a URL-derived placeholder until the
    /// real one arrives. Replace it while that visit is still the newest.
    fn amend_recorded_visit_title(&mut self, id: ItemId, title: &str) {
        let Some((recorded_url, _)) = self.last_visits.get(&id) else {
            return;
        };
        let recorded_url = recorded_url.clone();
        if self
            .items
            .tab(id)
            .and_then(|tab| tab.url.as_ref())
            .is_none_or(|url| url.as_str() != recorded_url)
        {
            return;
        }
        let Some(profile) = self.profile_of_item(id).filter(|profile| {
            self.profiles
                .get(*profile)
                .is_some_and(|profile| profile.kind != ProfileKind::Incognito)
        }) else {
            return;
        };
        self.store
            .amend_visit_title(profile, recorded_url, title.to_owned());
    }

    fn should_record_visit(&mut self, id: ItemId, url: &str) -> bool {
        const REPEATED_URL_MIN: std::time::Duration = std::time::Duration::from_secs(30);
        const NAVIGATION_MIN: std::time::Duration = std::time::Duration::from_secs(1);
        let now = std::time::Instant::now();
        if let Some((previous, recorded)) = self.last_visits.get(&id) {
            let minimum = if previous == url {
                REPEATED_URL_MIN
            } else {
                NAVIGATION_MIN
            };
            if now.duration_since(*recorded) < minimum {
                return false;
            }
        }
        self.last_visits.insert(id, (url.to_owned(), now));
        true
    }
}

fn same_browser_origin(left: &url::Url, right: &url::Url) -> bool {
    match (origin_of(left), origin_of(right)) {
        (Some(left), Some(right)) => left == right,
        // `about:blank` is the only admitted opaque browser target. Treat it
        // as same-document only when its canonical URL is exactly unchanged.
        (None, None) => left.as_str() == right.as_str(),
        _ => false,
    }
}

fn neutral_title_for_url(url: &url::Url) -> String {
    let Some(host) = url.host_str() else {
        return url.as_str().to_owned();
    };
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}
