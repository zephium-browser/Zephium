//! Mini windows: a small window of its own for one page, opened by a link
//! another application hands over, by the launcher, or by a link a mini
//! page opens. Its page lives outside every space until the person moves it
//! into the main window, where it becomes an ordinary tab without reloading.
//! A mini window is never saved; closing it ends its page.

use super::*;

/// Mini windows open at once; a page beyond this opens as a tab instead.
pub const MAX_MINI_WINDOWS: usize = 8;
/// The toolbar the mini window's chrome draws above its page.
pub const MINI_TOOLBAR_HEIGHT: f64 = 40.0;
/// The size of the first mini window; later ones open at the size the
/// person last gave one, for as long as the browser runs.
pub const MINI_WINDOW_SIZE: Size = Size {
    width: 760.0,
    height: 640.0,
};
/// How far the page stays inside the window's left, right and bottom edges.
/// A Windows mini window draws no frame, and the ring it resizes by lies
/// along those edges inside the window; a page there would cover it.
pub const MINI_PAGE_INSET: f64 = if cfg!(target_os = "windows") {
    4.0
} else {
    0.0
};

/// Where a mini window's page sits in a window of `size`: under the toolbar,
/// clear of the edges it is resized by.
pub(crate) fn mini_page_region(size: Size) -> Rect {
    Rect::new(
        MINI_PAGE_INSET,
        MINI_TOOLBAR_HEIGHT,
        (size.width - 2.0 * MINI_PAGE_INSET).max(0.0),
        (size.height - MINI_TOOLBAR_HEIGHT - MINI_PAGE_INSET).max(0.0),
    )
}

/// The smallest content a mini window keeps; the native window enforces it.
pub const MINI_WINDOW_MIN_SIZE: Size = Size {
    width: 360.0,
    height: 240.0,
};
/// A reported size beyond this is not a window anyone made.
const MINI_WINDOW_MAX_EDGE: f64 = 16_384.0;

#[derive(Debug)]
pub(super) struct MiniWindow {
    /// The space the page joins when it opens in the main window, if the
    /// main window no longer shows one of its profile's spaces by then.
    space: SpaceId,
    pub(super) page: ItemId,
    /// The native window exists and its stage can take the page.
    pub(super) attached: bool,
    /// Kept above other applications' windows, as the person asked.
    pinned: bool,
    /// The page takes the keyboard once its view is laid out, so a window
    /// that opens can be scrolled and typed into at once.
    focus_pending: std::cell::Cell<bool>,
}

#[derive(Debug, Default)]
pub(super) struct MiniState {
    pub(super) windows: std::collections::BTreeMap<WindowId, MiniWindow>,
    /// The size the person last gave a mini window.
    size: Option<Size>,
}

/// How a new mini window's page gets its document.
enum MiniSeed {
    Navigate(String),
    /// The engine already built the view for a page another page opened.
    Adopt {
        child: ItemId,
        adoption: zephium_core::ports::engine::NativeTabAdoption,
    },
}

/// A size a native window reported, if it is one a window can have.
pub fn admitted_mini_size(size: Size) -> Option<Size> {
    let edge = |value: f64| value.is_finite() && (1.0..=MINI_WINDOW_MAX_EDGE).contains(&value);
    (edge(size.width) && edge(size.height)).then_some(size)
}

impl Shell {
    /// The mini window showing `id`, if one does.
    pub(super) fn mini_window_of(&self, id: ItemId) -> Option<WindowId> {
        self.mini
            .windows
            .iter()
            .find(|(_, mini)| mini.page == id)
            .map(|(window, _)| *window)
    }

    pub(super) fn mini_pages(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.mini.windows.values().map(|mini| mini.page)
    }

    /// Whether a link from another application should open in a mini window.
    pub(super) fn external_links_open_mini(&self) -> bool {
        self.tab_preferences.external_mini && self.engine.hosts_mini_windows()
    }

    /// The regular profile and space a new mini window belongs to: a
    /// private window in front is not where another application's link goes.
    fn mini_scope(&self) -> Option<(ProfileId, SpaceId)> {
        if let Some(scope) = self
            .private
            .as_ref()
            .and_then(|session| session.regular_scope())
        {
            return Some(scope);
        }
        self.windows
            .focused()
            .map(|window| (window.profile, window.space))
    }

    /// Opens a mini window on `url` in the regular scope. `false` means none
    /// opened and the caller falls back.
    pub(super) fn open_mini(&mut self, url: String) -> bool {
        let Some((profile, space)) = self.mini_scope() else {
            return false;
        };
        self.open_mini_page(profile, space, MiniSeed::Navigate(url))
            .is_some()
    }

    /// A page in a mini window asked for a new window: the link opens in a
    /// mini window of its own, in the same profile, so the person's place in
    /// the main window stays as it was. `false` when none can open.
    pub(super) fn open_linked_mini(&mut self, source: ItemId, url: &str) -> bool {
        let Some((profile, space)) = self.mini_origin(source) else {
            return false;
        };
        self.open_mini_page(profile, space, MiniSeed::Navigate(url.to_owned()))
            .is_some()
    }

    /// A link a mini page opened when no further mini window can: it waits
    /// as a tab in the main window, which stays where it was.
    pub(super) fn open_mini_link_as_tab(&mut self, source: ItemId, url: &str) {
        if let Some((profile, space)) = self.mini_origin(source) {
            let _ = self.open_page_tab(profile, space, None, url, false);
        }
    }

    /// The same for a page the engine already built, such as a sign-in
    /// popup that keeps its opener.
    pub(super) fn adopt_linked_mini(
        &mut self,
        source: ItemId,
        child: ItemId,
        foreground: bool,
        adoption: zephium_core::ports::engine::NativeTabAdoption,
    ) {
        let Some((profile, space)) = self.mini_origin(source) else {
            adoption.finish(false);
            return;
        };
        if self.items.get(child).is_some()
            || self.items.view_ids().len() >= LIVE_VIEW_ABSOLUTE_LIMIT
        {
            adoption.finish(false);
            return;
        }
        // Past the cap the page is kept, as a background tab in the space
        // the mini window belongs to, rather than closed.
        if self.mini.windows.len() >= MAX_MINI_WINDOWS {
            self.adopt_mini_link_as_tab(source, child, space, adoption);
            return;
        }
        if self
            .open_mini_page(profile, space, MiniSeed::Adopt { child, adoption })
            .is_none()
        {
            return;
        }
        self.items.clear_blocked_popup(source);
        self.native_openers.insert(
            child,
            NativeOpener {
                source,
                activate_when_presentable: foreground,
            },
        );
    }

    fn adopt_mini_link_as_tab(
        &mut self,
        source: ItemId,
        child: ItemId,
        space: SpaceId,
        adoption: zephium_core::ports::engine::NativeTabAdoption,
    ) {
        let placement = Placement::Space {
            space,
            section: SpaceSection::Today,
        };
        if !self.items.insert_tab(child, placement) {
            adoption.finish(false);
            return;
        }
        if !self.items.adopt_native_view(child) {
            let _ = self.items.remove(child);
            adoption.finish(false);
            return;
        }
        self.record_view_creation(child);
        self.items.clear_blocked_popup(source);
        // A popup that closes itself, as a sign-in window does, still goes.
        self.native_openers.insert(
            child,
            NativeOpener {
                source,
                activate_when_presentable: false,
            },
        );
        self.commit(Vec::new());
        adoption.finish(true);
    }

    /// The profile and space of the mini window showing `source`.
    fn mini_origin(&self, source: ItemId) -> Option<(ProfileId, SpaceId)> {
        let window = self.mini_window_of(source)?;
        let mini = self.mini.windows.get(&window)?;
        let profile = self.windows.get(window)?.profile;
        Some((profile, mini.space))
    }

    /// Makes the window and its page. An adopted view's page takes the
    /// adoption's own id. `None` means nothing opened; an adoption was
    /// answered.
    fn open_mini_page(
        &mut self,
        profile: ProfileId,
        space: SpaceId,
        seed: MiniSeed,
    ) -> Option<WindowId> {
        let refused = !self.bootstrapped
            || !self.engine.hosts_mini_windows()
            || self.mini.windows.len() >= MAX_MINI_WINDOWS
            // A private page never leaves the private window.
            || self.incognito_profile(profile)
            || self.profile_deletion_quarantines(profile)
            || self
                .spaces
                .get(space)
                .is_none_or(|known| known.profile != profile);
        let page = match &seed {
            MiniSeed::Adopt { child, .. } => *child,
            MiniSeed::Navigate(_) => ItemId::generate(),
        };
        if refused || !self.items.insert_tab(page, Placement::Mini { profile }) {
            if let MiniSeed::Adopt { adoption, .. } = seed {
                adoption.finish(false);
            }
            return None;
        }
        let size = self.mini.size.unwrap_or(MINI_WINDOW_SIZE);
        let window = self.windows.create_unfocused(
            zephium_core::windows::WindowKind::Mini,
            profile,
            space,
            size,
        );
        if let Some(win) = self.windows.get_mut(window) {
            win.active = Some(page);
        }
        self.mini.windows.insert(
            window,
            MiniWindow {
                space,
                page,
                attached: false,
                pinned: false,
                focus_pending: std::cell::Cell::new(true),
            },
        );
        (self.emit)(Projection::MiniOpened(zephium_ipc::MiniOpenedView {
            window,
            width: size.width,
            height: size.height,
        }));
        match seed {
            MiniSeed::Navigate(url) => {
                self.project_mini(window);
                let effects = self.items.navigate(page, &url);
                self.commit(effects);
            }
            MiniSeed::Adopt { adoption, .. } => {
                if !self.items.adopt_native_view(page) {
                    adoption.finish(false);
                    let _ = self.close_mini(window);
                    return None;
                }
                self.record_view_creation(page);
                self.project_mini(window);
                self.commit(Vec::new());
                adoption.finish(true);
            }
        }
        Some(window)
    }

    /// The native window for `window` exists and the engine took its handle.
    pub(super) fn mini_attached(&mut self, window: WindowId, size: Size) {
        let Some(mini) = self.mini.windows.get_mut(&window) else {
            // Closed before the desktop finished building it.
            let _ = self.engine.release_window(window);
            return;
        };
        // A window no screen could hold gets no page laid out in it; its
        // next real size attaches it.
        let Some(size) = admitted_mini_size(size) else {
            return;
        };
        mini.attached = true;
        let page = mini.page;
        if let Some(win) = self.windows.get_mut(window) {
            win.size = size;
        }
        self.project_mini(window);
        let _ = self.relayout_mini(window);
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

    pub(super) fn mini_resized(&mut self, window: WindowId, size: Size) {
        let Some(size) = admitted_mini_size(size) else {
            return;
        };
        let Some(mini) = self.mini.windows.get(&window) else {
            return;
        };
        if mini.attached {
            self.mini.size = Some(size);
        }
        if let Some(win) = self.windows.get_mut(window) {
            win.size = size;
        }
        let _ = self.relayout_mini(window);
    }

    /// Lays out every mini window's page. Main-window layouts call this, so
    /// a page that gains or loses its view, or a policy that settles, reaches
    /// them too.
    pub(super) fn relayout_mini_windows(&self) {
        for window in self.mini.windows.keys() {
            let _ = self.relayout_mini(*window);
        }
    }

    fn relayout_mini(&self, window: WindowId) -> NativeDispatch {
        let (Some(mini), Some(win)) = (self.mini.windows.get(&window), self.windows.get(window))
        else {
            return NativeDispatch::Rejected;
        };
        if !mini.attached {
            return NativeDispatch::Rejected;
        }
        let page = Some(mini.page).filter(|id| {
            self.items.tab(*id).is_some_and(TabState::has_view)
                && self.blocker.native_policy_available(win.profile)
        });
        let region = mini_page_region(win.size);
        let dispatch = match page {
            Some(id) => self
                .engine
                .set_content(window, Some(Pane::Leaf(id)), Some(region)),
            None => self.engine.set_content(window, None, None),
        };
        if let Some(id) = page.filter(|_| mini.focus_pending.get()) {
            if self.engine.focus_page(id) == NativeDispatch::Scheduled {
                mini.focus_pending.set(false);
            }
        }
        dispatch
    }

    /// Sends the mini window its page as chrome shows it.
    pub(super) fn project_mini(&self, window: WindowId) {
        let Some(mini) = self.mini.windows.get(&window) else {
            return;
        };
        let tab = Some(mini.page).and_then(|id| {
            let profile = self.profile_of_item(id);
            let view = self.generic_tab_view(id, self.items.tab(id)?, profile);
            self.record_tab_projection_revision(id, &view.projection_revision);
            Some(view)
        });
        (self.emit)(Projection::Mini(self.mini_view(window, tab)));
    }

    pub(super) fn mini_view(
        &self,
        window: WindowId,
        tab: Option<zephium_ipc::TabView>,
    ) -> zephium_ipc::MiniView {
        let profile = self.windows.get(window).map(|win| win.profile);
        let spaces = profile
            .map(|profile| {
                self.spaces
                    .of(profile)
                    .into_iter()
                    .filter_map(|id| self.spaces.get(id))
                    .map(|space| zephium_ipc::MiniSpaceView {
                        id: space.id.to_string(),
                        name: space.name.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        zephium_ipc::MiniView {
            window,
            tab,
            spaces,
            space: self.mini_target(window).map(|space| space.to_string()),
            pinned: self
                .mini
                .windows
                .get(&window)
                .is_some_and(|mini| mini.pinned),
        }
    }

    /// The space "Open in Zephium" uses by default: the one the main window
    /// shows when it is the page's profile, else the one the window opened
    /// from.
    fn mini_target(&self, window: WindowId) -> Option<SpaceId> {
        let profile = self.windows.get(window)?.profile;
        let origin = self.mini.windows.get(&window)?.space;
        self.mini_scope()
            .filter(|(main, _)| *main == profile)
            .map(|(_, space)| space)
            .or(Some(origin))
            .filter(|space| {
                self.spaces
                    .get(*space)
                    .is_some_and(|known| known.profile == profile)
            })
    }

    /// Loads an address the launcher chose in a mini window's page.
    pub(super) fn navigate_mini(&mut self, window: WindowId, url: &str) -> bool {
        let Some(page) = self.mini.windows.get(&window).map(|mini| mini.page) else {
            return false;
        };
        self.cancel_discard_probe(page);
        self.cancel_page_permission_for_item(page);
        let effects = self.items.navigate(page, url);
        self.commit(effects);
        self.project_mini(window);
        true
    }

    pub(super) fn mini_action(
        &mut self,
        window: WindowId,
        action: crate::MiniAction,
    ) -> OperationDisposition {
        let Some(page) = self.mini.windows.get(&window).map(|mini| mini.page) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        match action {
            crate::MiniAction::Close => self.close_mini(window),
            crate::MiniAction::OpenInMain => self.open_mini_in_main(window, None),
            crate::MiniAction::OpenInSpace(space) => self.open_mini_in_main(window, Some(space)),
            // The desktop could not build the window: the page still opens,
            // as a tab, rather than being lost.
            crate::MiniAction::Unbuilt => {
                if self
                    .mini
                    .windows
                    .get(&window)
                    .is_some_and(|mini| mini.attached)
                {
                    return operation_result(
                        OperationOutcome::NoOp,
                        OperationReason::StateUnchanged,
                    );
                }
                self.open_mini_in_main(window, None)
            }
            crate::MiniAction::KeepOnTop(pinned) => {
                let Some(mini) = self.mini.windows.get_mut(&window) else {
                    return operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::InvalidScope,
                    );
                };
                let pinned = pinned.unwrap_or(!mini.pinned);
                if mini.pinned == pinned {
                    return operation_result(
                        OperationOutcome::NoOp,
                        OperationReason::StateUnchanged,
                    );
                }
                mini.pinned = pinned;
                self.project_mini(window);
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            crate::MiniAction::Back | crate::MiniAction::Forward => {
                let forward = action == crate::MiniAction::Forward;
                let available = self.items.tab(page).is_some_and(|tab| {
                    tab.has_view()
                        && if forward {
                            tab.can_go_forward
                        } else {
                            tab.can_go_back
                        }
                });
                if !available {
                    return operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::HistoryUnavailable,
                    );
                }
                self.cancel_discard_probe(page);
                self.cancel_page_permission_for_item(page);
                native_result(if forward {
                    self.engine.go_forward(page)
                } else {
                    self.engine.go_back(page)
                })
            }
            crate::MiniAction::Reload => {
                self.cancel_discard_probe(page);
                self.cancel_page_permission_for_item(page);
                if self.items.tab(page).is_some_and(TabState::has_view) {
                    native_result(self.engine.reload(page))
                } else {
                    let effects = self.items.ensure_view(page);
                    mutation_result(self.commit(effects))
                }
            }
        }
    }

    /// Ends a mini window and the page in it.
    pub(super) fn close_mini(&mut self, window: WindowId) -> OperationDisposition {
        let Some(mini) = self.mini.windows.remove(&window) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        self.windows.remove(window);
        // The page is ended before the stage that holds it is released, so
        // its view goes back to the main window's keeping, never with the
        // native window.
        let mut native = NativeWork::default();
        if self.items.tab(mini.page).is_some() {
            self.forget_mini_page(mini.page);
            let effects = self.items.remove(mini.page);
            native = self.commit(effects);
        }
        let _ = self.engine.release_window(window);
        (self.emit)(Projection::MiniClosed(zephium_ipc::MiniClosedView {
            window,
            reveal_main: false,
        }));
        mutation_result(native)
    }

    fn forget_mini_page(&mut self, id: ItemId) {
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
    /// of the default space, keeping the live page, and closes the mini
    /// window. A space of another profile is ignored.
    pub(super) fn open_mini_in_main(
        &mut self,
        window: WindowId,
        target: Option<SpaceId>,
    ) -> OperationDisposition {
        let Some(mini) = self.mini.windows.get(&window) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        let id = mini.page;
        let origin = mini.space;
        let Some(profile) = self.items.tab(id).and_then(|_| self.profile_of_item(id)) else {
            return self.close_mini(window);
        };
        let target = target.filter(|space| {
            self.spaces
                .get(*space)
                .is_some_and(|known| known.profile == profile)
        });
        // A browser page in front returns first; the whole handoff runs
        // again once it has, and the mini window stays until then.
        if self.active_browser_page().is_some() && !self.browser_return_ready {
            let action = target.map_or(
                crate::MiniAction::OpenInMain,
                crate::MiniAction::OpenInSpace,
            );
            self.browser_after_return = Some(Box::new(Command::Mini { window, action }));
            return self.operation_show_browser_page(None);
        }
        if self.private_shown() {
            let _ = self.operation_leave_private();
        }
        if self
            .windows
            .focused()
            .is_some_and(|main| main.profile != profile)
        {
            let _ = self.operation_switch_profile(profile);
        }
        // A switch that did not land leaves the page where it is.
        if self
            .windows
            .focused()
            .is_none_or(|main| main.profile != profile)
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
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
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        };
        if self
            .windows
            .focused()
            .is_some_and(|main| main.space != space)
        {
            let _ = self.operation_space(crate::SpaceCommand::Switch(space));
        }
        if self
            .windows
            .focused()
            .is_none_or(|main| main.space != space)
        {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
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
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        self.mini.windows.remove(&window);
        self.windows.remove(window);
        // The page leaves the mini window's stage before the main window's
        // layout takes it, both on the native thread in this order.
        let _ = self.engine.release_window(window);
        let effects = self.focus_tab(id);
        let native = self.commit(effects);
        (self.emit)(Projection::MiniClosed(zephium_ipc::MiniClosedView {
            window,
            reveal_main: true,
        }));
        mutation_result(native)
    }

    /// Closes mini windows whose page went away underneath them, as a
    /// deleted profile's pages do.
    pub(super) fn prune_mini_windows(&mut self) {
        let orphaned: Vec<WindowId> = self
            .mini
            .windows
            .iter()
            .filter(|(_, mini)| self.items.tab(mini.page).is_none())
            .map(|(window, _)| *window)
            .collect();
        for window in orphaned {
            let _ = self.close_mini(window);
        }
    }
}

fn native_result(admission: NativeDispatch) -> OperationDisposition {
    let mut native = NativeWork::default();
    native.record(admission);
    mutation_result(native)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_sizes_a_window_can_have_are_admitted() {
        assert_eq!(
            admitted_mini_size(Size::new(800.0, 600.0)),
            Some(Size::new(800.0, 600.0))
        );
        for size in [
            Size::new(0.0, 600.0),
            Size::new(800.0, -1.0),
            Size::new(f64::NAN, 600.0),
            Size::new(800.0, f64::INFINITY),
            Size::new(MINI_WINDOW_MAX_EDGE + 1.0, 600.0),
        ] {
            assert_eq!(admitted_mini_size(size), None, "{size:?}");
        }
    }

    #[test]
    fn the_page_sits_under_the_toolbar_clear_of_the_resize_edges() {
        let region = mini_page_region(Size::new(760.0, 640.0));
        assert_eq!(region.x, MINI_PAGE_INSET);
        assert_eq!(region.y, MINI_TOOLBAR_HEIGHT);
        assert_eq!(region.width, 760.0 - 2.0 * MINI_PAGE_INSET);
        assert_eq!(region.height, 640.0 - MINI_TOOLBAR_HEIGHT - MINI_PAGE_INSET);
        let tiny = mini_page_region(Size::new(0.0, 10.0));
        assert_eq!((tiny.width, tiny.height), (0.0, 0.0));
    }
}
