//! Which native view holds element fullscreen. The rules live apart from the
//! platforms so they can be exercised without a window: one owner per host,
//! a newcomer displaces it, and a view the layout no longer shows is asked to
//! leave rather than kept fullscreen behind the browser's back.

use std::collections::{HashMap, HashSet};

use zephium_core::ids::ItemId;

/// A view's native fullscreen state, read from the engine at the moment it is
/// needed rather than remembered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeFullscreen {
    Inactive,
    Entering,
    Active,
    Exiting,
}

impl NativeFullscreen {
    /// WebKit's `WKFullscreenState`. A value this build does not know is
    /// treated as fullscreen, so the view is left alone rather than moved.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn from_webkit(raw: isize) -> Self {
        match raw {
            0 => Self::Inactive,
            1 => Self::Entering,
            3 => Self::Exiting,
            _ => Self::Active,
        }
    }

    fn presented(self) -> bool {
        matches!(self, Self::Entering | Self::Active)
    }
}

/// A rectangle in virtual-screen pixels.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScreenRect {
    pub(crate) left: i32,
    pub(crate) top: i32,
    pub(crate) right: i32,
    pub(crate) bottom: i32,
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
impl ScreenRect {
    fn covers(self, other: Self) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

/// A window as the person sees it: the area the shell draws in is what a
/// fullscreen page has to fill. The window rectangle around it is not a
/// guide, since a maximized window keeps its frame off-screen and may keep
/// the monitor-sized rectangle fullscreen gave it after leaving.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowPlacement {
    pub(crate) window: ScreenRect,
    pub(crate) client: ScreenRect,
}

/// Whether the host window stands where an element fullscreen transition is
/// headed. Entering, its drawing area has to fill the monitor. Leaving, it
/// has to have stopped doing so, or be back at the drawing area it had
/// before: a window that was monitor-sized to begin with returns to exactly
/// the fullscreen rectangle, and one that was snapped or maximized does not
/// return to the rectangle it left.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn window_settled(
    now: WindowPlacement,
    monitor: ScreenRect,
    entering: bool,
    before: Option<ScreenRect>,
) -> bool {
    if entering {
        return now.client.covers(monitor);
    }
    now.client != monitor || before == Some(now.client)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenEffect {
    /// Tell the shell whether `id` is the fullscreen page.
    Report { id: ItemId, active: bool },
    /// Ask `id` to leave fullscreen.
    Exit(ItemId),
}

#[derive(Debug, Default)]
pub(crate) struct FullscreenLedger {
    owner: Option<ItemId>,
    reported: Option<ItemId>,
    native: HashMap<ItemId, NativeFullscreen>,
    exit_requested: HashSet<ItemId>,
}

impl FullscreenLedger {
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(crate) fn owner(&self) -> Option<ItemId> {
        self.owner
    }

    /// A native fullscreen transition of `id`, as the engine now reports it.
    pub(crate) fn observe(&mut self, id: ItemId, state: NativeFullscreen) -> Vec<FullscreenEffect> {
        let previous = self
            .native
            .get(&id)
            .copied()
            .unwrap_or(NativeFullscreen::Inactive);
        if state == NativeFullscreen::Inactive {
            self.native.remove(&id);
        } else {
            self.native.insert(id, state);
        }
        let mut effects = Vec::new();
        if state.presented() {
            if previous == NativeFullscreen::Inactive {
                // A fresh request from the page; an earlier exit request
                // belonged to the session that already ended.
                self.exit_requested.remove(&id);
            } else if state == NativeFullscreen::Active && self.exit_requested.contains(&id) {
                // An exit asked for while the page was still entering can be
                // dropped by the engine; ask again once it has arrived.
                effects.push(FullscreenEffect::Exit(id));
            }
            if let Some(previous_owner) = self.owner.filter(|owner| *owner != id) {
                self.request_exit(previous_owner, &mut effects);
                if self.reported == Some(previous_owner) {
                    self.reported = None;
                    effects.push(FullscreenEffect::Report {
                        id: previous_owner,
                        active: false,
                    });
                }
            }
            self.owner = Some(id);
            if self.reported != Some(id) {
                self.reported = Some(id);
                effects.push(FullscreenEffect::Report { id, active: true });
            }
        } else {
            if state == NativeFullscreen::Inactive {
                self.exit_requested.remove(&id);
            }
            if self.owner == Some(id) {
                self.owner = None;
            }
            if self.reported == Some(id) {
                self.reported = None;
                effects.push(FullscreenEffect::Report { id, active: false });
            }
        }
        effects
    }

    /// A layout was applied to a window. The owner leaves fullscreen when
    /// that window hosts it but no longer shows it.
    pub(crate) fn layout(
        &mut self,
        hosts: impl Fn(ItemId) -> bool,
        shown: &[ItemId],
    ) -> Vec<FullscreenEffect> {
        let mut effects = Vec::new();
        if let Some(owner) = self.owner {
            if hosts(owner) && !shown.contains(&owner) {
                self.request_exit(owner, &mut effects);
            }
        }
        effects
    }

    /// The view committed a new main-frame document; whatever was fullscreen
    /// belonged to the one it replaced.
    pub(crate) fn committed(&mut self, id: ItemId) -> Vec<FullscreenEffect> {
        let mut effects = Vec::new();
        if self.native.get(&id).is_some_and(|state| state.presented()) {
            self.request_exit(id, &mut effects);
        }
        effects
    }

    /// The view closed or its renderer died. Nothing is reported: the item's
    /// events are already retired with it.
    pub(crate) fn forget(&mut self, id: ItemId) {
        self.native.remove(&id);
        self.exit_requested.remove(&id);
        if self.owner == Some(id) {
            self.owner = None;
        }
        if self.reported == Some(id) {
            self.reported = None;
        }
    }

    /// An explicit request from the shell, deduplicated like the others.
    pub(crate) fn exit(&mut self, id: ItemId) -> Vec<FullscreenEffect> {
        let mut effects = Vec::new();
        if self.native.contains_key(&id) {
            self.request_exit(id, &mut effects);
        }
        effects
    }

    fn request_exit(&mut self, id: ItemId, effects: &mut Vec<FullscreenEffect>) {
        if self.exit_requested.insert(id) {
            effects.push(FullscreenEffect::Exit(id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FullscreenEffect::{Exit, Report};
    use super::NativeFullscreen::{Active, Entering, Exiting, Inactive};
    use super::*;

    fn id() -> ItemId {
        ItemId::generate()
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn placed(window: ScreenRect, client: ScreenRect) -> WindowPlacement {
        WindowPlacement { window, client }
    }

    const MONITOR: ScreenRect = ScreenRect {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    #[test]
    fn a_normal_window_settles_when_it_fills_the_monitor_and_when_it_is_back() {
        let before = rect(200, 100, 1400, 900);
        let normal = placed(before, before);
        let full = placed(MONITOR, MONITOR);
        assert!(!window_settled(normal, MONITOR, true, Some(before)));
        assert!(window_settled(full, MONITOR, true, Some(before)));
        assert!(!window_settled(full, MONITOR, false, Some(before)));
        assert!(window_settled(normal, MONITOR, false, Some(before)));
    }

    #[test]
    fn a_maximized_window_settles_on_its_work_area_not_on_the_rectangle_it_left() {
        // Taskbar at the bottom: the frame hangs 8 px off-screen on three
        // sides, and the drawing area is clamped to the work area.
        let work = rect(0, 0, 1920, 1032);
        let maximized = placed(rect(-8, -8, 1928, 1040), work);
        assert!(!window_settled(maximized, MONITOR, true, Some(work)));
        assert!(window_settled(
            placed(MONITOR, MONITOR),
            MONITOR,
            true,
            Some(work)
        ));
        // Leaving, Windows may keep the monitor-sized rectangle fullscreen
        // gave the window and only clamp its drawing area again.
        assert!(!window_settled(
            placed(MONITOR, MONITOR),
            MONITOR,
            false,
            Some(work)
        ));
        assert!(window_settled(
            placed(MONITOR, work),
            MONITOR,
            false,
            Some(work)
        ));
        assert!(window_settled(maximized, MONITOR, false, Some(work)));
    }

    #[test]
    fn a_maximized_window_settles_beside_a_side_or_hidden_taskbar() {
        for work in [
            rect(96, 0, 1920, 1080),
            rect(0, 0, 1824, 1080),
            rect(0, 48, 1920, 1080),
            rect(0, 0, 1920, 1079),
            rect(1, 0, 1920, 1080),
        ] {
            assert!(!window_settled(
                placed(MONITOR, work),
                MONITOR,
                true,
                Some(work)
            ));
            assert!(window_settled(
                placed(MONITOR, MONITOR),
                MONITOR,
                true,
                Some(work)
            ));
            assert!(!window_settled(
                placed(MONITOR, MONITOR),
                MONITOR,
                false,
                Some(work)
            ));
            assert!(window_settled(
                placed(MONITOR, work),
                MONITOR,
                false,
                Some(work)
            ));
        }
    }

    #[test]
    fn a_window_that_returns_elsewhere_still_settles() {
        // A snapped window comes back to the rectangle it had before it was
        // snapped.
        let snapped = rect(960, 0, 1920, 1032);
        let unsnapped = rect(300, 200, 1500, 900);
        assert!(window_settled(
            placed(unsnapped, unsnapped),
            MONITOR,
            false,
            Some(snapped)
        ));
        assert!(!window_settled(
            placed(MONITOR, MONITOR),
            MONITOR,
            false,
            Some(snapped)
        ));
    }

    #[test]
    fn a_window_that_was_already_monitor_sized_settles_where_it_began() {
        // Fullscreen before the page took the screen, or sized to the monitor
        // by hand: leaving puts it back in the same place.
        let full = placed(MONITOR, MONITOR);
        assert!(window_settled(full, MONITOR, true, Some(MONITOR)));
        assert!(window_settled(full, MONITOR, false, Some(MONITOR)));
        assert!(!window_settled(full, MONITOR, false, None));
        let work = rect(0, 0, 1920, 1032);
        assert!(window_settled(placed(MONITOR, work), MONITOR, false, None));
    }

    #[test]
    fn a_page_is_reported_once_on_entry_and_once_on_exit() {
        let mut ledger = FullscreenLedger::default();
        let page = id();
        assert_eq!(
            ledger.observe(page, Entering),
            [Report {
                id: page,
                active: true
            }]
        );
        assert!(ledger.observe(page, Active).is_empty());
        assert_eq!(ledger.owner(), Some(page));
        assert_eq!(
            ledger.observe(page, Exiting),
            [Report {
                id: page,
                active: false
            }]
        );
        assert_eq!(ledger.owner(), None);
        assert!(ledger.observe(page, Inactive).is_empty());
    }

    #[test]
    fn a_second_page_displaces_the_first() {
        let mut ledger = FullscreenLedger::default();
        let (first, second) = (id(), id());
        ledger.observe(first, Active);
        assert_eq!(
            ledger.observe(second, Entering),
            [
                Exit(first),
                Report {
                    id: first,
                    active: false
                },
                Report {
                    id: second,
                    active: true
                }
            ]
        );
        assert_eq!(ledger.owner(), Some(second));
        // The displaced page's own exit is no news to the shell.
        assert!(ledger.observe(first, Exiting).is_empty());
        assert!(ledger.observe(first, Inactive).is_empty());
        assert_eq!(ledger.owner(), Some(second));
    }

    #[test]
    fn a_hidden_owner_is_asked_to_leave_once() {
        let mut ledger = FullscreenLedger::default();
        let (page, other) = (id(), id());
        ledger.observe(page, Active);
        assert!(ledger.layout(|_| true, &[page]).is_empty());
        assert_eq!(ledger.layout(|_| true, &[other]), [Exit(page)]);
        assert!(ledger.layout(|_| true, &[]).is_empty());
        // Another window's layout says nothing about this page.
        let mut elsewhere = FullscreenLedger::default();
        elsewhere.observe(page, Active);
        assert!(elsewhere.layout(|_| false, &[]).is_empty());
    }

    #[test]
    fn an_exit_dropped_while_entering_is_asked_again_on_arrival() {
        let mut ledger = FullscreenLedger::default();
        let page = id();
        ledger.observe(page, Entering);
        assert_eq!(ledger.layout(|_| true, &[]), [Exit(page)]);
        assert_eq!(ledger.observe(page, Active), [Exit(page)]);
    }

    #[test]
    fn leaving_clears_the_request_so_a_new_session_can_be_asked_again() {
        let mut ledger = FullscreenLedger::default();
        let page = id();
        ledger.observe(page, Active);
        assert_eq!(ledger.exit(page), [Exit(page)]);
        assert!(ledger.exit(page).is_empty());
        ledger.observe(page, Inactive);
        assert!(ledger.exit(page).is_empty());
        ledger.observe(page, Active);
        assert_eq!(ledger.exit(page), [Exit(page)]);
    }

    #[test]
    fn a_new_document_ends_the_old_one_fullscreen() {
        let mut ledger = FullscreenLedger::default();
        let page = id();
        assert!(ledger.committed(page).is_empty());
        ledger.observe(page, Active);
        assert_eq!(ledger.committed(page), [Exit(page)]);
        ledger.observe(page, Exiting);
        assert!(ledger.committed(page).is_empty());
    }

    #[test]
    fn a_closed_or_crashed_owner_is_forgotten_without_a_report() {
        let mut ledger = FullscreenLedger::default();
        let page = id();
        ledger.observe(page, Active);
        ledger.forget(page);
        assert_eq!(ledger.owner(), None);
        assert!(ledger.exit(page).is_empty());
        // A late native fact for the retired view starts over cleanly.
        assert_eq!(
            ledger.observe(page, Active),
            [Report {
                id: page,
                active: true
            }]
        );
    }

    #[test]
    fn unknown_webkit_states_keep_the_view_with_webkit() {
        assert_eq!(NativeFullscreen::from_webkit(0), Inactive);
        assert_eq!(NativeFullscreen::from_webkit(1), Entering);
        assert_eq!(NativeFullscreen::from_webkit(2), Active);
        assert_eq!(NativeFullscreen::from_webkit(3), Exiting);
        assert_eq!(NativeFullscreen::from_webkit(7), Active);
        assert_eq!(NativeFullscreen::from_webkit(-1), Active);
    }
}
