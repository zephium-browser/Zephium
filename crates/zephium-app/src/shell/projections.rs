//! Revisioned privileged-chrome projections.

use std::collections::HashSet;

use super::*;

#[derive(Default)]
struct SidebarProjection {
    nodes: Vec<SidebarNodeView>,
    tabs: Vec<TabView>,
    visited_ids: HashSet<ItemId>,
    tab_ids: HashSet<ItemId>,
}

impl Shell {
    pub(super) fn project_page_permission_prompt(&self) {
        let prompt =
            self.page_permissions
                .visible()
                .and_then(|(profile, item, request, processing)| {
                    let kinds = match request.kind {
                    zephium_core::permissions::PagePermissionRequestKind::Single(kind) => {
                        page_permission_kind_view(kind).into_iter().collect()
                    }
                    zephium_core::permissions::PagePermissionRequestKind::CameraAndMicrophone => {
                        vec![
                            PagePermissionKindView::Camera,
                            PagePermissionKindView::Microphone,
                        ]
                    }
                };
                    if kinds.is_empty() {
                        return None;
                    }
                    Some(PagePermissionPromptEntryView {
                        profile_id: profile.to_string(),
                        item_id: item.to_string(),
                        request_id: format!("{:016x}", request.id.get()),
                        origin: request.origin.to_string(),
                        kinds,
                        rememberable: self.page_permissions.visible_rememberable(),
                        processing,
                    })
                });
        (self.emit)(Projection::PagePermissionPrompt(PagePermissionPromptView {
            projection_revision: format!("{:032x}", self.next_projection_revision()),
            prompt,
        }));
    }

    /// Projects only the focused profile and exact active tab. A missing or
    /// stale native snapshot is represented by an empty replacement cohort;
    /// old buttons can never survive a focus/surface transition by inference.
    pub(super) fn project_extension_actions(&self, profile: ProfileId) {
        let Some(window) = self
            .windows
            .focused()
            .filter(|window| window.profile == profile)
        else {
            return;
        };
        let surface = self
            .extension_browser_surfaces
            .published_surface(profile)
            .filter(|surface| {
                surface
                    .windows()
                    .first()
                    .and_then(zephium_core::extensions::ExtensionBrowserWindow::active)
                    == window.active
            });
        let tab = surface.and_then(|surface| {
            surface
                .windows()
                .first()
                .and_then(zephium_core::extensions::ExtensionBrowserWindow::active)
                .map(|tab| (tab, surface.generation()))
        });
        let actions = tab.map_or_else(Vec::new, |(tab, generation)| {
            self.extension_actions
                .projected_actions(profile, tab, generation)
        });
        (self.emit)(Projection::ExtensionActions(ExtensionActionsView {
            projection_revision: format!("{:032x}", self.next_projection_revision()),
            profile_id: profile.to_string(),
            tab_id: tab.map(|(tab, _)| tab.to_string()),
            actions,
        }));
    }

    pub(super) fn project_extension_action_failure(
        &self,
        profile: ProfileId,
        expected_tab: Option<ItemId>,
        reason: zephium_core::extensions::ExtensionActionRejection,
    ) {
        let Some(tab) = self
            .windows
            .focused()
            .filter(|window| window.profile == profile)
            .and_then(|window| window.active)
            .filter(|tab| expected_tab.is_none_or(|expected| expected == *tab))
        else {
            return;
        };
        (self.emit)(Projection::ExtensionActionFailed(
            ExtensionActionFailedView {
                projection_revision: format!("{:032x}", self.next_projection_revision()),
                profile_id: profile.to_string(),
                tab_id: tab.to_string(),
                reason: extension_action_failure_view(reason),
            },
        ));
    }

    pub(super) fn project_extension_action_shortcut(
        &self,
        runtime: zephium_core::extensions::ExtensionRuntimeInstance,
        tab: ItemId,
        action_revision: zephium_core::extensions::ExtensionActionRevision,
    ) {
        let current = self
            .windows
            .focused()
            .filter(|window| window.profile == runtime.profile() && window.active == Some(tab))
            .is_some();
        if !current {
            return;
        }
        (self.emit)(Projection::ExtensionActionShortcut(
            ExtensionActionShortcutView {
                projection_revision: format!("{:032x}", self.next_projection_revision()),
                profile_id: runtime.profile().to_string(),
                tab_id: tab.to_string(),
                runtime: ExtensionActionRuntimeView {
                    install_id: runtime.install_id().to_string(),
                    generation: format!("{:016x}", runtime.generation().get()),
                },
                action_revision: format!("{:016x}", action_revision.get()),
            },
        ));
    }

    pub(super) fn project_runtime_status(&self) {
        (self.emit)(Projection::RuntimeStatus(RuntimeStatus {
            restart_required: self.runtime_restart_required,
            session_set_aside: self.session_set_aside,
            user_content_degraded_scope_count: self.user_content_status.degraded_scope_count(),
            security_advisories: self
                .engine
                .runtime_security_advisories()
                .iter()
                .map(runtime_security_advisory_view)
                .collect(),
        }));
    }

    pub(super) fn reconcile_runtime_restart_requirement(&mut self) -> bool {
        if self.runtime_restart_required || !self.engine.runtime_restart_required() {
            return false;
        }
        self.runtime_restart_required = true;
        self.project_runtime_status();
        true
    }

    pub(super) fn project_items(&self) {
        if let Some(window) = self.windows.focused() {
            if let Some(profile) = self.profiles.get(window.profile) {
                (self.emit)(Projection::PanelOwner(zephium_ipc::PanelOwner {
                    private: profile.kind == zephium_core::profiles::ProfileKind::Incognito,
                    window_id: window.id.to_string(),
                    profile_id: profile.id.to_string(),
                    profile_name: profile.name.clone(),
                    space_id: window.space.to_string(),
                }));
            }
        }
        self.project_blocker_status();
        if let Some(snapshot) = self.items_snapshot() {
            self.publish_icons();
            (self.emit)(Projection::Items(snapshot));
        }
    }

    pub(super) fn items_snapshot(&self) -> Option<ItemsState> {
        let win = self.windows.focused()?;
        self.items_snapshot_from(
            &self.profiles,
            &self.spaces,
            &self.items,
            win.profile,
            win.space,
            win.active,
            win.splits.as_ref(),
        )
    }

    #[allow(clippy::too_many_arguments)] // One projection of a single session scope.
    fn items_snapshot_from(
        &self,
        profiles: &Profiles,
        spaces: &Spaces,
        items: &Items,
        profile_id: ProfileId,
        space_id: SpaceId,
        active: Option<ItemId>,
        splits: Option<&Pane>,
    ) -> Option<ItemsState> {
        let profile = profiles.get(profile_id)?;
        let active_space = spaces
            .get(space_id)
            .filter(|space| space.profile == profile.id)?;

        let profile_view = ProfileView {
            id: profile.id.to_string(),
            name: profile.name.clone(),
            kind: match profile.kind {
                ProfileKind::Default => ProfileKindView::Default,
                ProfileKind::Named => ProfileKindView::Named,
                ProfileKind::Incognito => ProfileKindView::Incognito,
            },
        };
        let spaces = spaces
            .iter()
            .filter(|space| space.profile == profile.id)
            .map(|space| SpaceView {
                id: space.id.to_string(),
                name: space.name.clone(),
            })
            .collect();

        let mut sidebar = SidebarProjection::default();
        for (placement, section) in [
            (
                Placement::Favorites {
                    profile: profile.id,
                },
                SidebarSectionView::Favorites,
            ),
            (
                Placement::Space {
                    space: active_space.id,
                    section: SpaceSection::Pinned,
                },
                SidebarSectionView::Pinned,
            ),
            (
                Placement::Space {
                    space: active_space.id,
                    section: SpaceSection::Today,
                },
                SidebarSectionView::Today,
            ),
        ] {
            for id in items.roots(placement) {
                self.project_sidebar_node(
                    items,
                    *id,
                    None,
                    placement,
                    section,
                    profile.id,
                    &mut sidebar,
                );
            }
        }

        let split_group = splits.and_then(|tree| {
            let members = tree.tabs();
            let mut unique = HashSet::with_capacity(members.len());
            let valid = (2..=MAX_VISIBLE_PANES).contains(&members.len())
                && members
                    .iter()
                    .all(|id| unique.insert(*id) && sidebar.tab_ids.contains(id));
            valid.then(|| SplitGroupView {
                members: members.into_iter().map(|id| id.to_string()).collect(),
            })
        });
        self.record_tab_projection_revisions(&sidebar.tabs);
        if let Ok(mut views) = self.presentation.last_tab_views.try_borrow_mut() {
            views.retain(|id, _| self.items.tab(*id).is_some());
        }
        Some(ItemsState {
            projection_revision: format!("{:032x}", self.next_projection_revision()),
            profile: Some(profile_view),
            spaces,
            active_space_id: Some(active_space.id.to_string()),
            nodes: sidebar.nodes,
            tabs: sidebar.tabs,
            active: active
                .filter(|id| sidebar.tab_ids.contains(id))
                .map(|id| id.to_string()),
            split_group,
        })
    }

    #[allow(clippy::too_many_arguments)] // Bounded recursive projection retains its exact scope.
    fn project_sidebar_node(
        &self,
        items: &Items,
        id: ItemId,
        parent: Option<ItemId>,
        placement: Placement,
        section: SidebarSectionView,
        profile: ProfileId,
        projection: &mut SidebarProjection,
    ) {
        let Some(item) = items
            .get(id)
            .filter(|item| item.parent == parent && item.placement == placement)
        else {
            return;
        };
        if !projection.visited_ids.insert(id) {
            return;
        }

        let id_string = id.to_string();
        let kind = match &item.kind {
            ItemKind::Folder { name } => SidebarNodeKindView::Folder { name: name.clone() },
            ItemKind::Tab(tab) => {
                projection.tab_ids.insert(id);
                projection
                    .tabs
                    .push(self.generic_tab_view(id, tab, Some(profile)));
                SidebarNodeKindView::Tab {
                    tab_id: id_string.clone(),
                }
            }
        };
        projection.nodes.push(SidebarNodeView {
            id: id_string,
            parent_id: parent.map(|id| id.to_string()),
            section,
            kind,
        });

        if matches!(item.kind, ItemKind::Folder { .. }) {
            for child in items.children(id) {
                self.project_sidebar_node(
                    items,
                    *child,
                    Some(id),
                    placement,
                    section,
                    profile,
                    projection,
                );
            }
        }
    }

    pub(super) fn project_tab(&self, id: ItemId) {
        if self
            .windows
            .focused()
            .is_some_and(|window| window.active == Some(id))
        {
            self.project_blocker_status();
        }
        let profile = self.profile_of_item(id);
        if let Some(tab) = self.items.tab(id) {
            let projection = self.generic_tab_view(id, tab, profile);
            self.record_tab_projection_revision(id, &projection.projection_revision);
            self.publish_icons();
            (self.emit)(Projection::Tab(projection));
        }
    }

    fn record_tab_projection_revisions(&self, tabs: &[TabView]) {
        let Ok(mut revisions) = self
            .presentation
            .last_tab_projection_revision
            .try_borrow_mut()
        else {
            // The shell actor is single-threaded and these borrows never span
            // callbacks. Retaining the older value fails closed by making an
            // otherwise valid presentation callback stale.
            return;
        };
        for tab in tabs {
            if let Some(id) = ItemId::parse(&tab.id) {
                revisions.insert(id, tab.projection_revision.clone());
            }
        }
    }

    pub(super) fn record_tab_projection_revision(&self, id: ItemId, revision: &str) {
        if let Ok(mut revisions) = self
            .presentation
            .last_tab_projection_revision
            .try_borrow_mut()
        {
            revisions.insert(id, revision.to_owned());
        }
    }

    fn generic_tab_view(&self, id: ItemId, tab: &TabState, profile: Option<ProfileId>) -> TabView {
        let mut view = self.presentation_tab_view(
            id,
            tab,
            self.icon_ref(zephium_ipc::IconSurface::Chrome, tab, profile),
        );
        if self
            .presentation
            .deferred_first_content_layout
            .contains(&id)
        {
            // A full Items snapshot may still be necessary for focus or tab
            // topology. Preserve that delivery while ensuring the exact
            // presentation eval remains the first URL-bearing projection.
            view.url = None;
            view.title = "New Tab".into();
            view.loading = false;
            view.can_go_back = false;
            view.can_go_forward = false;
            view.icon = None;
            view.availability = None;
        }
        self.stable_revision(id, view)
    }

    /// Keeps the revision of a tab whose view did not change since it was
    /// last offered; a changed view keeps its new revision and is remembered.
    fn stable_revision(&self, id: ItemId, view: TabView) -> TabView {
        if let Ok(views) = self.presentation.last_tab_views.try_borrow() {
            if let Some(previous) = views.get(&id) {
                let unchanged = TabView {
                    projection_revision: previous.projection_revision.clone(),
                    ..view.clone()
                };
                if unchanged == *previous {
                    return unchanged;
                }
            }
        }
        self.remember_tab_view(id, &view);
        view
    }

    pub(super) fn remember_tab_view(&self, id: ItemId, view: &TabView) {
        if let Ok(mut views) = self.presentation.last_tab_views.try_borrow_mut() {
            views.insert(id, view.clone());
        }
    }

    pub(super) fn presentation_tab_view(
        &self,
        id: ItemId,
        tab: &TabState,
        icon: Option<zephium_ipc::IconRef>,
    ) -> TabView {
        let mut view = tab_view(id, tab, icon, self.next_projection_revision());
        if self.crash.presentations.contains(&id) {
            view.title = "Page crashed".into();
        }
        view.availability = self.capacity_presentation(id).or_else(|| {
            (!tab.has_view()
                && tab.url.is_some()
                && tab.content == zephium_core::item::TabContent::Web)
                .then_some(zephium_ipc::TabAvailability::Sleeping)
        });
        view
    }

    pub(super) fn next_projection_revision(&self) -> u128 {
        // Saturation is fail-closed: subsequent equal revisions are ignored
        // by privileged chrome, so no older projection can become current.
        let next = self
            .presentation
            .projection_sequence
            .get()
            .saturating_add(1);
        self.presentation.projection_sequence.set(next);
        next
    }

    pub(super) fn icon_ref(
        &self,
        surface: zephium_ipc::IconSurface,
        tab: &TabState,
        profile: Option<ProfileId>,
    ) -> Option<zephium_ipc::IconRef> {
        let origin = tab.url.as_ref().and_then(origin_of)?;
        self.icon_ref_for(surface, profile?, &origin)
    }

    pub(super) fn icon_ref_for_url(
        &self,
        surface: zephium_ipc::IconSurface,
        profile: ProfileId,
        url: &str,
    ) -> Option<zephium_ipc::IconRef> {
        let parsed = url::Url::parse(url).ok()?;
        self.icon_ref_for(surface, profile, &origin_of(&parsed)?)
    }
}

fn page_permission_kind_view(
    kind: zephium_core::permissions::PagePermissionKind,
) -> Option<PagePermissionKindView> {
    match kind {
        zephium_core::permissions::PagePermissionKind::Camera => {
            Some(PagePermissionKindView::Camera)
        }
        zephium_core::permissions::PagePermissionKind::Microphone => {
            Some(PagePermissionKindView::Microphone)
        }
        // The current native broker never admits these capabilities. Keep the
        // projection total while preserving a closed UI vocabulary.
        zephium_core::permissions::PagePermissionKind::Geolocation
        | zephium_core::permissions::PagePermissionKind::Notifications
        | zephium_core::permissions::PagePermissionKind::ClipboardRead => None,
    }
}

fn extension_action_failure_view(
    reason: zephium_core::extensions::ExtensionActionRejection,
) -> ExtensionActionFailure {
    use zephium_core::extensions::ExtensionActionRejection as Core;
    match reason {
        Core::InvalidRequest => ExtensionActionFailure::InvalidRequest,
        Core::RuntimeUnavailable => ExtensionActionFailure::RuntimeUnavailable,
        Core::RuntimeSuperseded => ExtensionActionFailure::RuntimeSuperseded,
        Core::TabUnavailable => ExtensionActionFailure::TabUnavailable,
        Core::TabDiscarded => ExtensionActionFailure::TabDiscarded,
        Core::ActionUnavailable => ExtensionActionFailure::ActionUnavailable,
        Core::ActionDisabled => ExtensionActionFailure::ActionDisabled,
        Core::CapacityExceeded => ExtensionActionFailure::CapacityExceeded,
        Core::PopupUnavailable => ExtensionActionFailure::PopupUnavailable,
        Core::PopupCapacityExceeded => ExtensionActionFailure::PopupCapacityExceeded,
        Core::NativeAdmissionFailed => ExtensionActionFailure::NativeAdmissionFailed,
        Core::ShuttingDown => ExtensionActionFailure::ShuttingDown,
        Core::UnsupportedPlatform => ExtensionActionFailure::UnsupportedPlatform,
    }
}

fn runtime_security_advisory_view(
    advisory: zephium_core::runtime_security::RuntimeSecurityAdvisory,
) -> RuntimeSecurityAdvisory {
    RuntimeSecurityAdvisory {
        kind: match advisory.kind() {
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::ReviewOverdue => {
                RuntimeSecurityAdvisoryKind::ReviewOverdue
            }
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::UpdateRecommended => {
                RuntimeSecurityAdvisoryKind::UpdateRecommended
            }
            zephium_core::runtime_security::RuntimeSecurityAdvisoryKind::UnreviewedRuntime => {
                RuntimeSecurityAdvisoryKind::UnreviewedRuntime
            }
        },
        update_target: match advisory.update_target() {
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::Zephium => {
                RuntimeSecurityUpdateTarget::Zephium
            }
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::OperatingSystem => {
                RuntimeSecurityUpdateTarget::OperatingSystem
            }
            zephium_core::runtime_security::RuntimeSecurityUpdateTarget::BrowserRuntime => {
                RuntimeSecurityUpdateTarget::BrowserRuntime
            }
        },
    }
}

fn tab_view(
    id: ItemId,
    tab: &TabState,
    icon: Option<zephium_ipc::IconRef>,
    revision: u128,
) -> TabView {
    TabView {
        id: id.to_string(),
        projection_revision: format!("{revision:032x}"),
        title: tab.title.clone(),
        url: tab.url.as_ref().map(ToString::to_string),
        content: match tab.content {
            zephium_core::item::TabContent::Web => zephium_ipc::TabContentView::Web,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Settings,
            ) => zephium_ipc::TabContentView::Settings,
            zephium_core::item::TabContent::BrowserOwned(
                zephium_core::item::BrowserOwnedTab::Extensions,
            ) => zephium_ipc::TabContentView::Extensions,
            zephium_core::item::TabContent::ExtensionOwned => {
                zephium_ipc::TabContentView::ExtensionOwned
            }
        },
        loading: tab.loading,
        page_request: tab.page_request.as_deref().map(|request| match request {
            zephium_core::item::PageRequest::ExternalApp { url, app } => {
                zephium_ipc::PageRequestView::ExternalApp {
                    site: tab
                        .url
                        .as_ref()
                        .and_then(|url| url.host_str())
                        .map(str::to_owned),
                    scheme: url.scheme().to_owned(),
                    app: app.clone(),
                }
            }
            zephium_core::item::PageRequest::Popup { url } => zephium_ipc::PageRequestView::Popup {
                host: url
                    .as_ref()
                    .and_then(|url| url.host_str())
                    .map(str::to_owned),
            },
        }),
        availability: None,
        failure: tab.failure.as_ref().map(|failure| {
            use zephium_core::ports::engine::NavigationFailureReason as Reason;
            zephium_ipc::TabFailure {
                url: failure.url.to_string(),
                reason: match failure.reason {
                    Reason::Offline => zephium_ipc::TabFailureReason::Offline,
                    Reason::HostNotFound => zephium_ipc::TabFailureReason::HostNotFound,
                    Reason::Unreachable => zephium_ipc::TabFailureReason::Unreachable,
                    Reason::TimedOut => zephium_ipc::TabFailureReason::TimedOut,
                    Reason::Insecure => zephium_ipc::TabFailureReason::Insecure,
                    Reason::Other => zephium_ipc::TabFailureReason::Other,
                },
            }
        }),
        can_go_back: tab.can_go_back,
        can_go_forward: tab.can_go_forward,
        icon,
        capture: tab.capture.filter(|(_, state)| state.is_capturing()).map(
            |(navigation, state)| {
                let device = |value| match value {
                    zephium_core::ports::engine::CaptureDeviceState::None => {
                        zephium_ipc::CaptureDeviceStateView::None
                    }
                    zephium_core::ports::engine::CaptureDeviceState::Active => {
                        zephium_ipc::CaptureDeviceStateView::Active
                    }
                    zephium_core::ports::engine::CaptureDeviceState::Muted => {
                        zephium_ipc::CaptureDeviceStateView::Muted
                    }
                };
                zephium_ipc::MediaCaptureView {
                    navigation_id: format!("{:016x}", navigation.into_raw()),
                    camera: device(state.camera),
                    microphone: device(state.microphone),
                }
            },
        ),
    }
}
