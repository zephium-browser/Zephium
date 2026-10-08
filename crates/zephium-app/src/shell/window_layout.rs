//! Authoritative window geometry, split layout, and divider capture.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct GrabbedDivider {
    window: WindowId,
    topology: Pane,
    divider: split::Divider,
    /// Where the pointer took the gutter, from its leading edge, so the
    /// divider does not jump to the pointer on pickup.
    anchor: (f64, f64),
    /// The ratio the guide shows; applied to the panes once, on release.
    ratio: Option<f64>,
}

impl Shell {
    pub(super) fn resolve_drop(&self, x: f64, y: f64) -> Option<split::Drop> {
        if self.active_browser_page().is_some() {
            return None;
        }
        let win = self.windows.focused()?;
        let tree = self.pane_tree()?;
        let region =
            layout::compute(win.size, win.mode, win.metrics, self.present(&tree)).content?;
        let local = Rect::new(0.0, 0.0, region.width, region.height);
        split::drop_target(&tree, local, win.metrics.gap, x - region.x, y - region.y)
    }

    pub(super) fn relayout(&self) -> NativeDispatch {
        self.relayout_with(false)
    }

    /// A relayout that is one step of a deliberate change of shape, which
    /// the chrome and the content carry out as a journey.
    pub(super) fn relayout_with(&self, travel: bool) -> NativeDispatch {
        let filling = self.window_filling_fullscreen();
        let Some(win) = self.windows.focused() else {
            return NativeDispatch::Rejected;
        };
        let browser_page_active = self.active_browser_page().is_some();
        // A window-filling fullscreen page stands alone; the split it belongs
        // to is untouched and returns with the next layout after it.
        let tree = if let Some(id) = filling {
            Some(Pane::Leaf(id))
        } else if browser_page_active {
            self.work_pane_tree()
        } else {
            self.pane_tree()
        };
        let present = tree.as_ref().is_some_and(|t| self.present(t));
        let mut l = layout::compute(win.size, win.mode, win.metrics, present);
        // A browser-owned consent prompt is window-modal. Native page views
        // are sibling views above chrome on every platform, so CSS alone
        // cannot prevent a page from obscuring the prompt or receiving input
        // behind it. Remove content from the native stage and expand
        // privileged chrome for exactly the lifetime of the retained prompt.
        let privileged_overlay_active = self.page_permissions.is_visible()
            || self.active_browser_page().is_some()
            || self.focus_covers();
        // `Items` marks a prospective view resident before its CreateView
        // effect is dispatched. While the profile's first explicit native
        // policy is still compiling/installing, that effect is intentionally
        // held. Do not expose the logical leaf to the engine lifecycle gate
        // until policy settlement releases the create and reserves its native
        // item token. The settlement path applies held effects before calling
        // `relayout`, so the first visible layout remains correctly ordered
        // after native policy registration and view admission.
        let native_policy_available = self.blocker.native_policy_available(win.profile);
        if !self.window_visible || !native_policy_available || privileged_overlay_active {
            l.content = None;
        }
        // The Work pane is the one content rect that coexists with full-window
        // chrome: the native leaf floats above the canvas inside a hole the
        // chrome draws around the applied rect. Modal prompts still win.
        let pane_admitted = browser_page_active
            && !self.page_permissions.is_visible()
            && self.window_visible
            && native_policy_available;
        let work_pane = self.work_pane_layout(pane_admitted && present);
        if let Some(pane) = work_pane.as_ref().filter(|pane| pane.presented) {
            l.content = Some(Rect::new(pane.x, pane.y, pane.width, pane.height));
        }
        if filling.is_some() {
            l.content = Some(Rect::new(0.0, 0.0, win.size.width, win.size.height));
        }
        // Raw native children still receive their final geometry while a
        // first navigation is provisional, but macOS must not shrink the
        // privileged chrome away from a fresh New Tab surface until at least
        // one visible leaf completed its exact chrome-verification transition.
        // The content stage itself is transparent and presentation-gated, so
        // it can sit above this real UI without painting an artificial box.
        let chrome_present = !privileged_overlay_active
            && self.window_visible
            && native_policy_available
            && tree.as_ref().is_some_and(|tree| {
                tree.tabs().iter().any(|id| {
                    self.items.tab(*id).is_some_and(|tab| {
                        tab.has_view()
                            && (tab.url.is_some()
                                || tab.content == zephium_core::item::TabContent::ExtensionOwned)
                            && !self.presentation.deferred_first_content_layout.contains(id)
                    })
                })
            });
        // Work chrome is full-bleed: header on the window material, composer on the edge.
        let chrome_metrics = if self.active_browser_page() == Some(crate::BrowserPage::Work) {
            layout::Metrics {
                padding: 0.0,
                ..win.metrics
            }
        } else {
            win.metrics
        };
        let chrome_layout = layout::compute(win.size, win.mode, chrome_metrics, chrome_present);
        if !self.chrome.position(ChromeFrame {
            rect: chrome_layout.chrome,
            fill_width: chrome_layout.content.is_none(),
            travel,
        }) {
            return NativeDispatch::Rejected;
        }
        let dividers = match (&tree, l.content) {
            (Some(tree), Some(region)) if !browser_page_active => {
                let local = Rect::new(0.0, 0.0, region.width, region.height);
                split::dividers(tree, local, win.metrics.gap)
                    .into_iter()
                    .map(|d| DividerView {
                        x: region.x + d.strip.x,
                        y: region.y + d.strip.y,
                        width: d.strip.width,
                        height: d.strip.height,
                        vertical: d.axis == Axis::Row,
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        (self.emit)(Projection::Layout(LayoutState {
            dividers,
            work_pane,
        }));
        self.engine.set_content(win.id, tree, l.content)
    }

    pub(super) fn locate_divider(&self, x: f64, y: f64) -> Option<GrabbedDivider> {
        if self.active_browser_page().is_some() {
            return None;
        }
        let win = self.windows.focused()?;
        let tree = self.pane_tree()?;
        let region =
            layout::compute(win.size, win.mode, win.metrics, self.present(&tree)).content?;
        let local = Rect::new(0.0, 0.0, region.width, region.height);
        let divider = split::divider_at(&tree, local, win.metrics.gap, x - region.x, y - region.y)?;
        let anchor = (
            x - region.x - divider.strip.x,
            y - region.y - divider.strip.y,
        );
        Some(GrabbedDivider {
            window: win.id,
            topology: tree,
            divider,
            anchor,
            ratio: None,
        })
    }

    /// Ends a divider capture without applying it, and takes its guide away.
    pub(super) fn drop_divider(&mut self) {
        if let Some(grabbed) = self.divider.take() {
            if grabbed.ratio.is_some() {
                let _ = self.engine.set_resize_guide(grabbed.window, None);
            }
        }
    }

    /// Applies the guided ratio to the panes in one layout, then clears the
    /// guide. Returns false when nothing was captured.
    pub(super) fn commit_divider(&mut self) -> bool {
        let Some(grabbed) = self.divider.take() else {
            return false;
        };
        if let Some(ratio) = grabbed.ratio {
            let _ = self.engine.set_resize_guide(grabbed.window, None);
            if let Some(win) = self.windows.focused_mut() {
                if win.id == grabbed.window {
                    if let Some(tree) = win.splits.as_mut() {
                        tree.set_ratio(&grabbed.divider.path, ratio);
                    }
                }
            }
            let _ = self.relayout();
        }
        true
    }

    /// Moves the guide only: page views keep their size until release, as on
    /// macOS, so no pane relayouts or repaints on every pointer frame.
    pub(super) fn divider_drag(&mut self, x: f64, y: f64) {
        let Some(grabbed) = self.divider.as_ref() else {
            return;
        };
        let grabbed_window = grabbed.window;
        let grabbed_path = grabbed.divider.path.clone();
        let Some(win) = self.windows.focused() else {
            self.drop_divider();
            return;
        };
        let current_tree = self.pane_tree();
        let topology_is_current = win.id == grabbed_window
            && current_tree
                .as_ref()
                .is_some_and(|tree| grabbed.topology.same_topology(tree));
        if !topology_is_current {
            // Focus/topology changed while the pointer was captured. The same
            // binary path may now identify a different live branch.
            self.drop_divider();
            return;
        }
        let Some(region) = layout::compute(win.size, win.mode, win.metrics, true).content else {
            return;
        };
        let gap = win.metrics.gap;
        let Some(tree) = current_tree.as_ref() else {
            self.drop_divider();
            return;
        };
        let local = Rect::new(0.0, 0.0, region.width, region.height);
        let Some(current) = split::divider_at_path(tree, local, gap, &grabbed_path) else {
            // The split tree changed while the pointer was captured. Its old
            // path is no longer authority for any live branch.
            self.drop_divider();
            return;
        };
        let (anchor_x, anchor_y) = grabbed.anchor;
        let ratio = split::ratio_for(
            current.axis,
            current.rect,
            gap,
            x - region.x - anchor_x,
            y - region.y - anchor_y,
        );
        let mut preview = tree.clone();
        preview.set_ratio(&grabbed_path, ratio);
        let Some(target) = split::divider_at_path(&preview, local, gap, &grabbed_path) else {
            return;
        };
        let (r, strip) = (target.rect, target.strip);
        let guide = match target.axis {
            Axis::Row => Rect::new(
                region.x + strip.x + strip.width / 2.0 - 1.0,
                region.y + r.y,
                2.0,
                r.height,
            ),
            Axis::Col => Rect::new(
                region.x + r.x,
                region.y + strip.y + strip.height / 2.0 - 1.0,
                r.width,
                2.0,
            ),
        };
        let window = win.id;
        if let Some(grabbed) = self.divider.as_mut() {
            grabbed.ratio = Some(ratio);
        }
        let _ = self.engine.set_resize_guide(window, Some(guide));
    }

    fn present(&self, tree: &Pane) -> bool {
        tree.tabs()
            .iter()
            .any(|id| self.items.tab(*id).is_some_and(TabState::has_view))
    }

    // The split group persists across tab switches (Arc model): members show
    // the whole group, other tabs show alone, the group is a tab away.
    pub(super) fn pane_tree(&self) -> Option<Pane> {
        let win = self.windows.focused()?;
        let active = win.active?;
        if !self.item_in_scope(active, win.profile, win.space) {
            return None;
        }
        if let Some(tree) = win.splits.clone() {
            if tree.contains(active)
                && self.pane_in_scope(&tree, win.profile, win.space)
                && tree
                    .tabs()
                    .into_iter()
                    .all(|id| self.items.tab(id).is_some_and(TabState::has_view))
            {
                return Some(tree);
            }
        }
        self.items
            .tab(active)
            .is_some_and(TabState::has_view)
            .then_some(Pane::Leaf(active))
    }

    pub(super) fn content_region(&self) -> Rect {
        let Some(win) = self.windows.focused() else {
            return Rect::default();
        };
        layout::compute(win.size, win.mode, win.metrics, true)
            .content
            .unwrap_or_default()
    }
}
