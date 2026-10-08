//! Element fullscreen of the page on screen. The engine reports which page
//! holds it; the shell decides whether that page still may, and lays the
//! window out for it where the engine presents fullscreen inside the window.

use super::*;
use zephium_core::ports::engine::FullscreenPresentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ContentFullscreen {
    window: WindowId,
    item: ItemId,
}

impl Shell {
    pub(super) fn on_fullscreen_changed(&mut self, id: ItemId, active: bool) {
        if active {
            let window = self.windows.focused().map(|window| window.id);
            let Some(window) = window.filter(|_| self.content_fullscreen_admits(id)) else {
                // A page behind the browser's own surfaces, in another tab,
                // or one the shell already took fullscreen away from.
                let _ = self.engine.exit_fullscreen(id);
                return;
            };
            let previous = self
                .content_fullscreen
                .replace(Some(ContentFullscreen { window, item: id }));
            if let Some(previous) = previous.filter(|previous| previous.item != id) {
                let _ = self.engine.exit_fullscreen(previous.item);
            }
        } else if self
            .content_fullscreen
            .get()
            .is_some_and(|current| current.item == id)
        {
            self.content_fullscreen.set(None);
        } else {
            return;
        }
        let _ = self.relayout();
    }

    /// The page shown fullscreen, if it still may be. One that may not is
    /// asked to leave, and the browser takes its own layout back at once
    /// rather than waiting for the page to agree.
    pub(super) fn content_fullscreen(&self) -> Option<ItemId> {
        let current = self.content_fullscreen.get()?;
        if self
            .windows
            .focused()
            .is_some_and(|window| window.id == current.window)
            && self.content_fullscreen_admits(current.item)
        {
            return Some(current.item);
        }
        self.end_content_fullscreen();
        None
    }

    /// Takes fullscreen away from the page before the browser changes what
    /// is on screen.
    pub(super) fn end_content_fullscreen(&self) {
        if let Some(current) = self.content_fullscreen.take() {
            let _ = self.engine.exit_fullscreen(current.item);
        }
    }

    /// Moving to another tab is moving away from the fullscreen page.
    pub(super) fn end_content_fullscreen_unless(&self, id: ItemId) {
        if self
            .content_fullscreen
            .get()
            .is_some_and(|current| current.item != id)
        {
            self.end_content_fullscreen();
        }
    }

    /// The closing page's view goes with it; nothing is left to exit.
    pub(super) fn forget_content_fullscreen(&self, id: ItemId) {
        if self
            .content_fullscreen
            .get()
            .is_some_and(|current| current.item == id)
        {
            self.content_fullscreen.set(None);
        }
    }

    /// The page that fills the whole window, where the engine presents
    /// fullscreen inside it. The window follows, through the desktop.
    pub(super) fn window_filling_fullscreen(&self) -> Option<ItemId> {
        let fill = self
            .content_fullscreen()
            .filter(|_| self.fills_host_window());
        if self.host_fullscreen.replace(fill.is_some()) != fill.is_some() {
            (self.emit)(Projection::HostFullscreen(fill.is_some()));
        }
        fill
    }

    fn fills_host_window(&self) -> bool {
        self.engine.fullscreen_presentation() == FullscreenPresentation::FillHostWindow
    }

    fn content_fullscreen_admits(&self, id: ItemId) -> bool {
        let Some(window) = self.windows.focused() else {
            return false;
        };
        // A browser page or modal prompt covers content with chrome, and a
        // window-filling page would cover that chrome in turn. Work keeps
        // its pane's page beside the canvas, so only an engine with its own
        // fullscreen window may take it from there.
        self.window_visible
            && !self.page_permissions.is_visible()
            && !self.focus_covers()
            && self.blocker.native_policy_available(window.profile)
            && (self.active_browser_page().is_none() || !self.fills_host_window())
            && self.items.tab(id).is_some_and(|tab| {
                tab.has_view() && tab.content == zephium_core::item::TabContent::Web
            })
            && self.visible_tree().is_some_and(|tree| tree.contains(id))
    }
}
