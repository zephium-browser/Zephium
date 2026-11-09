//! Little windows: a small window of its own for a link another application
//! hands over, or for an address chosen in the launcher. Its one page lives outside every space
//! until the person opens it in the main window, where it becomes an ordinary
//! tab without reloading.

use super::*;

/// Little windows open at once; a link beyond this opens as a tab instead.
pub const MAX_LITTLE_WINDOWS: usize = 8;
/// The toolbar the Little window's chrome draws above its page.
pub const LITTLE_TOOLBAR_HEIGHT: f64 = 40.0;
pub const LITTLE_WINDOW_SIZE: Size = Size {
    width: 720.0,
    height: 640.0,
};

#[derive(Debug)]
struct LittleWindow {
    /// The space the page joins when it opens in the main window, if the
    /// window no longer shows one of its profile's spaces by then.
    space: SpaceId,
    page: ItemId,
    /// The native window exists and its stage can take the page.
    attached: bool,
}

#[derive(Debug, Default)]
pub(super) struct LittleState {
    windows: std::collections::BTreeMap<WindowId, LittleWindow>,
}

impl Shell {
    /// The Little window showing `id`, if one does.
    pub(super) fn little_window_of(&self, id: ItemId) -> Option<WindowId> {
        self.little
            .windows
            .iter()
            .find(|(_, little)| little.page == id)
            .map(|(window, _)| *window)
    }

    pub(super) fn little_pages(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.little.windows.values().map(|little| little.page)
    }

    /// Whether a link from another application should open in a Little window.
    pub(super) fn external_links_open_little(&self) -> bool {
        self.tab_preferences.external_little && self.engine.hosts_little_windows()
    }

    /// The regular profile and space a new Little window belongs to: a
    /// private window in front is not where another application's link goes.
    fn little_scope(&self) -> Option<(ProfileId, SpaceId)> {
        if let Some(scope) = self.private.as_ref().and_then(|session| {
            session
                .shown_private()
                .then(|| session.regular_scope())
                .flatten()
        }) {
            return Some(scope);
        }
        self.windows
            .focused()
            .map(|window| (window.profile, window.space))
    }

    /// Opens a Little window on `url`. `false` means none opened and the
    /// caller falls back.
    pub(super) fn open_little(&mut self, url: String) -> bool {
        if !self.bootstrapped
            || !self.engine.hosts_little_windows()
            || self.little.windows.len() >= MAX_LITTLE_WINDOWS
        {
            return false;
        }
        let Some((profile, space)) = self.little_scope() else {
            return false;
        };
        if self.profile_deletion_quarantines(profile) {
            return false;
        }
        let page = ItemId::generate();
        if !self.items.insert_tab(page, Placement::Little { profile }) {
            return false;
        }
        let window = self.windows.create_unfocused(
            zephium_core::windows::WindowKind::Little,
            profile,
            space,
            LITTLE_WINDOW_SIZE,
        );
        if let Some(win) = self.windows.get_mut(window) {
            win.active = Some(page);
        }
        self.little.windows.insert(
            window,
            LittleWindow {
                space,
                page,
                attached: false,
            },
        );
        (self.emit)(Projection::LittleOpened(zephium_ipc::LittleOpenedView {
            window,
        }));
        self.project_little(window);
        let effects = self.items.navigate(page, &url);
        self.commit(effects);
        true
    }

    /// The native window for `window` exists and the engine took its handle.
    pub(super) fn little_attached(&mut self, window: WindowId, size: Size) {
        let Some(little) = self.little.windows.get_mut(&window) else {
            // Closed before the desktop finished building it.
            let _ = self.engine.release_window(window);
            return;
        };
        little.attached = true;
        let page = little.page;
        if let Some(win) = self.windows.get_mut(window) {
            win.size = size;
        }
        self.project_little(window);
        let _ = self.relayout_little(window);
        // A first document can commit before the toolbar exists to verify
        // it; that toolbar is ready now.
        if let Some(pending) = self
            .presentation
            .pending_presentations
            .get(&page)
            .filter(|pending| !pending.chrome_applied && !pending.chrome_request_in_flight)
            .map(|pending| pending.navigation)
        {
            self.request_chrome_presentation(page, pending);
        }
    }

    pub(super) fn little_resized(&mut self, window: WindowId, size: Size) {
        if !self.little.windows.contains_key(&window) {
            return;
        }
        if let Some(win) = self.windows.get_mut(window) {
            win.size = size;
        }
        let _ = self.relayout_little(window);
    }

    /// Lays out every Little window's page. Main-window layouts call this, so
    /// a page that gains or loses its view, or a policy that settles, reaches
    /// them too.
    pub(super) fn relayout_little_windows(&self) {
        for window in self.little.windows.keys() {
            let _ = self.relayout_little(*window);
        }
    }

    fn relayout_little(&self, window: WindowId) -> NativeDispatch {
        let (Some(little), Some(win)) =
            (self.little.windows.get(&window), self.windows.get(window))
        else {
            return NativeDispatch::Rejected;
        };
        if !little.attached {
            return NativeDispatch::Rejected;
        }
        let page = Some(little.page).filter(|id| {
            self.items.tab(*id).is_some_and(TabState::has_view)
                && self.blocker.native_policy_available(win.profile)
        });
        let region = Rect::new(
            0.0,
            LITTLE_TOOLBAR_HEIGHT,
            win.size.width.max(0.0),
            (win.size.height - LITTLE_TOOLBAR_HEIGHT).max(0.0),
        );
        match page {
            Some(id) => self
                .engine
                .set_content(window, Some(Pane::Leaf(id)), Some(region)),
            None => self.engine.set_content(window, None, None),
        }
    }

    /// Sends the Little window its page as chrome shows it.
    pub(super) fn project_little(&self, window: WindowId) {
        let Some(little) = self.little.windows.get(&window) else {
            return;
        };
        let tab = Some(little.page).and_then(|id| {
            let profile = self.profile_of_item(id);
            let view = self.generic_tab_view(id, self.items.tab(id)?, profile);
            self.record_tab_projection_revision(id, &view.projection_revision);
            Some(view)
        });
        (self.emit)(Projection::Little(self.little_view(window, tab)));
    }

    pub(super) fn little_view(
        &self,
        window: WindowId,
        tab: Option<zephium_ipc::TabView>,
    ) -> zephium_ipc::LittleView {
        let profile = self.windows.get(window).map(|win| win.profile);
        let spaces = profile
            .map(|profile| {
                self.spaces
                    .of(profile)
                    .into_iter()
                    .filter_map(|id| self.spaces.get(id))
                    .map(|space| zephium_ipc::LittleSpaceView {
                        id: space.id.to_string(),
                        name: space.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        zephium_ipc::LittleView {
            window,
            tab,
            spaces,
            space: self.little_target(window).map(|space| space.to_string()),
        }
    }

    /// The space "Open in Zephium" uses by default: the one the main window
    /// shows when it is the page's profile, else the one the window opened
    /// from.
    fn little_target(&self, window: WindowId) -> Option<SpaceId> {
        let profile = self.windows.get(window)?.profile;
        let origin = self.little.windows.get(&window)?.space;
        let shown = self
            .private
            .as_ref()
            .filter(|session| session.shown_private())
            .and_then(|session| session.regular_scope())
            .or_else(|| {
                self.windows
                    .focused()
                    .map(|main| (main.profile, main.space))
            });
        shown
            .filter(|(main, _)| *main == profile)
            .map(|(_, space)| space)
            .or(Some(origin))
            .filter(|space| {
                self.spaces
                    .get(*space)
                    .is_some_and(|known| known.profile == profile)
            })
    }

    /// Loads an address the launcher chose in a Little window's page.
    pub(super) fn navigate_little(&mut self, window: WindowId, url: &str) -> bool {
        let Some(page) = self.little.windows.get(&window).map(|little| little.page) else {
            return false;
        };
        self.cancel_discard_probe(page);
        self.cancel_page_permission_for_item(page);
        let effects = self.items.navigate(page, url);
        self.commit(effects);
        self.project_little(window);
        true
    }

    pub(super) fn little_action(&mut self, window: WindowId, action: crate::LittleAction) {
        let Some(page) = self.little.windows.get(&window).map(|little| little.page) else {
            return;
        };
        match action {
            crate::LittleAction::Close => self.close_little(window),
            crate::LittleAction::OpenInMain => self.open_little_in_main(window, None),
            crate::LittleAction::OpenInSpace(space) => {
                self.open_little_in_main(window, Some(space));
            }
            crate::LittleAction::Back | crate::LittleAction::Forward => {
                let id = page;
                let forward = action == crate::LittleAction::Forward;
                let available = self.items.tab(id).is_some_and(|tab| {
                    tab.has_view()
                        && if forward {
                            tab.can_go_forward
                        } else {
                            tab.can_go_back
                        }
                });
                if !available {
                    return;
                }
                self.cancel_discard_probe(id);
                self.cancel_page_permission_for_item(id);
                let _ = if forward {
                    self.engine.go_forward(id)
                } else {
                    self.engine.go_back(id)
                };
            }
            crate::LittleAction::Reload => {
                let id = page;
                self.cancel_discard_probe(id);
                self.cancel_page_permission_for_item(id);
                if self.items.tab(id).is_some_and(TabState::has_view) {
                    let _ = self.engine.reload(id);
                } else {
                    let effects = self.items.ensure_view(id);
                    self.commit(effects);
                }
            }
        }
    }

    /// Ends a Little window and the page in it.
    pub(super) fn close_little(&mut self, window: WindowId) {
        let Some(little) = self.little.windows.remove(&window) else {
            return;
        };
        self.windows.remove(window);
        let _ = self.engine.release_window(window);
        // A page that went away underneath the window has nothing left to end.
        if self.items.tab(little.page).is_some() {
            self.forget_little_page(little.page);
            let effects = self.items.remove(little.page);
            self.commit(effects);
        }
        (self.emit)(Projection::LittleClosed(zephium_ipc::LittleClosedView {
            window,
            reveal_main: false,
        }));
    }

    fn forget_little_page(&mut self, id: ItemId) {
        self.native_openers.remove(&id);
        self.cancel_page_permission_for_item(id);
        self.cancel_pending_presentation(id);
        self.cancel_favicon_attempt(id);
        if matches!(
            self.residency.discard_probes.get(&id),
            Some(PendingDiscardProbe::Closing { .. })
        ) {
            self.cancel_discard_probe(id);
            self.residency.discard_probes.remove(&id);
            if let Some(queue) = &self.self_queue {
                queue.cancel_discard_probe(id);
            }
        } else {
            self.cancel_discard_probe(id);
        }
        if let Some(profile) = self.profile_of_item(id) {
            self.engine.forget_discarded_state(profile, Some(id));
        }
    }

    /// Moves the page into the main window as a tab in front of `target`, or
    /// of the default space, keeping the live page, and closes the Little
    /// window. A blank one only closes. A space of another profile is
    /// ignored.
    pub(super) fn open_little_in_main(&mut self, window: WindowId, target: Option<SpaceId>) {
        let Some(little) = self.little.windows.get(&window) else {
            return;
        };
        let id = little.page;
        if self.items.tab(id).is_none() {
            self.close_little(window);
            return;
        }
        let origin = little.space;
        let Some(profile) = self.profile_of_item(id) else {
            self.close_little(window);
            return;
        };
        let target = target.filter(|space| {
            self.spaces
                .get(*space)
                .is_some_and(|known| known.profile == profile)
        });
        if self.private_shown() {
            let _ = self.operation_leave_private();
        }
        if self.active_browser_page().is_some() {
            let _ = self.operation_show_browser_page(None);
        }
        if self
            .windows
            .focused()
            .is_some_and(|main| main.profile != profile)
        {
            let _ = self.operation_switch_profile(profile);
        }
        let Some(space) = target
            .or_else(|| {
                self.windows
                    .focused()
                    .filter(|main| main.profile == profile)
                    .map(|main| main.space)
            })
            .or_else(|| {
                self.spaces
                    .get(origin)
                    .filter(|space| space.profile == profile)
                    .map(|_| origin)
            })
        else {
            return;
        };
        if self
            .windows
            .focused()
            .is_some_and(|main| main.space != space)
        {
            let _ = self.operation_space(crate::SpaceCommand::Switch(space));
        }
        let before = self.today_tabs(space).into_iter().next();
        if !self.items.move_tab_to_root(
            id,
            Placement::Space {
                space,
                section: SpaceSection::Today,
            },
            before,
        ) {
            return;
        }
        self.little.windows.remove(&window);
        self.windows.remove(window);
        // The page leaves the Little window's stage before the main window's
        // layout takes it, both on the native thread in this order.
        let _ = self.engine.release_window(window);
        let effects = self.focus_tab(id);
        self.commit(effects);
        (self.emit)(Projection::LittleClosed(zephium_ipc::LittleClosedView {
            window,
            reveal_main: true,
        }));
    }

    /// Closes Little windows whose page went away underneath them, as a
    /// deleted profile's pages do.
    pub(super) fn prune_little_windows(&mut self) {
        let orphaned: Vec<WindowId> = self
            .little
            .windows
            .iter()
            .filter(|(_, little)| self.items.tab(little.page).is_none())
            .map(|(window, _)| *window)
            .collect();
        for window in orphaned {
            self.close_little(window);
        }
    }
}
