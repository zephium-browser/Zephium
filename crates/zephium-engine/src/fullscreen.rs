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
