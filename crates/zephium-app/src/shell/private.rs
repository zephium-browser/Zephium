//! Private browsing: an incognito profile the main window switches into.
//! Its pages run in an ephemeral partition (a non-persistent WebKit store on
//! macOS, an InPrivate WebView2 profile in a private runtime folder on
//! Windows), every store write already refuses incognito profiles, and the
//! session snapshot leaves them out. Closing the last private tab, or the
//! private window, erases the partition.

use super::*;

/// One scope the main window can show.
#[derive(Clone, Debug)]
struct Scope {
    profile: ProfileId,
    space: SpaceId,
    active: Option<ItemId>,
    splits: Option<Pane>,
}

#[derive(Debug)]
pub(super) struct PrivateSession {
    profile: ProfileId,
    space: SpaceId,
    /// The scope the window is not showing: the regular one while private
    /// tabs are in front, the private one after "back to personal".
    other: Scope,
    shown: bool,
}

impl Shell {
    pub(super) fn private_shown(&self) -> bool {
        self.private.as_ref().is_some_and(|session| session.shown)
    }

    /// The regular scope the durable session should name, whichever scope
    /// the window shows: a private one is never restored.
    pub(super) fn persisted_scope(&self) -> (Option<SpaceId>, Option<ItemId>, Option<&Pane>) {
        match &self.private {
            Some(session) if session.shown => (
                Some(session.other.space),
                session.other.active,
                session.other.splits.as_ref(),
            ),
            _ => {
                let win = self.windows.focused();
                (
                    win.map(|w| w.space),
                    win.and_then(|w| w.active),
                    win.and_then(|w| w.splits.as_ref()),
                )
            }
        }
    }

    pub(super) fn operation_enter_private(&mut self) -> OperationDisposition {
        if self.active_browser_page().is_some() && !self.browser_return_ready {
            self.browser_after_return = Some(Box::new(Command::Run("window.newPrivate".into())));
            return self.operation_show_browser_page(None);
        }
        if self.windows.focused().is_none() {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        }
        match self.private.as_ref().map(|session| session.shown) {
            // Already private: the shortcut opens another private tab.
            Some(true) => self.operation_open(),
            Some(false) => {
                let effects = self.swap_private_scope();
                mutation_result(self.commit(effects))
            }
            None => self.begin_private_session(),
        }
    }

    pub(super) fn operation_leave_private(&mut self) -> OperationDisposition {
        if !self.private_shown() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let effects = self.swap_private_scope();
        mutation_result(self.commit(effects))
    }

    pub(super) fn operation_close_private(&mut self) -> OperationDisposition {
        if self.private.is_none() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        mutation_result(self.end_private_session())
    }

    fn begin_private_session(&mut self) -> OperationDisposition {
        let Some(regular) = self.windows.focused().map(|win| Scope {
            profile: win.profile,
            space: win.space,
            active: win.active,
            splits: win.splits.clone(),
        }) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::NoFocusedWindow);
        };
        // A fresh identity every time: engine erasure tombstones and storage
        // bindings are per profile id, so one is never reused.
        let Some(profile) = (0..8).map(|_| ProfileId::generate()).find(|id| {
            self.profiles.insert(Profile {
                id: *id,
                name: "Private".into(),
                kind: ProfileKind::Incognito,
            })
        }) else {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        if !self.initialize_new_blocker_profile(profile) {
            self.profiles.remove(profile);
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(space) = (0..8).map(|_| SpaceId::generate()).find(|id| {
            self.spaces.insert(Space {
                id: *id,
                profile,
                name: "Private".into(),
            })
        }) else {
            self.blocker.retire_profile(profile);
            self.profiles.remove(profile);
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        if !self.start_blocker_profile(profile) {
            crate::diagnostic!("private: content policy compilation was not admitted");
        }
        if let Some(active) = regular.active {
            self.items.set_lifecycle(active, Lifecycle::Inactive);
        }
        if let Some(win) = self.windows.focused_mut() {
            win.profile = profile;
            win.space = space;
            win.active = None;
            win.splits = None;
        }
        self.private = Some(PrivateSession {
            profile,
            space,
            other: regular,
            shown: true,
        });
        let Some((_id, effects)) = self.open_tab_with_id() else {
            let _ = self.end_private_session();
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::ItemLimitReached,
            );
        };
        mutation_result(self.commit(effects))
    }

    /// Shows whichever scope the window is not showing, keeping the other's
    /// tabs, focus and splits as they were.
    fn swap_private_scope(&mut self) -> Vec<Effect> {
        let Some(session) = self.private.as_mut() else {
            return Vec::new();
        };
        let Some(win) = self.windows.focused_mut() else {
            return Vec::new();
        };
        let leaving = Scope {
            profile: win.profile,
            space: win.space,
            active: win.active,
            splits: win.splits.take(),
        };
        let entering = std::mem::replace(&mut session.other, leaving.clone());
        session.shown = !session.shown;
        win.profile = entering.profile;
        win.space = entering.space;
        win.active = None;
        win.splits = entering.splits;
        if let Some(active) = leaving.active {
            self.items.set_lifecycle(active, Lifecycle::Inactive);
        }
        match entering.active {
            Some(active) => self.focus_tab(active),
            None => Vec::new(),
        }
    }

    /// The last private tab closed: end the session once its space is empty.
    pub(super) fn end_private_session_if_empty(&mut self) -> Option<NativeWork> {
        let session = self.private.as_ref()?;
        if !session.shown
            || !self
                .keyboard_tabs(session.profile, session.space)
                .is_empty()
        {
            return None;
        }
        Some(self.end_private_session())
    }

    /// Returns to the regular scope and erases the private partition. The
    /// engine's erasure closes the private views; nothing of them was ever
    /// written to the session, history, favicons or time.
    pub(super) fn end_private_session(&mut self) -> NativeWork {
        let Some(session) = self.private.take() else {
            return NativeWork::default();
        };
        let mut effects = Vec::new();
        if session.shown {
            if let Some(win) = self.windows.focused_mut() {
                win.profile = session.other.profile;
                win.space = session.other.space;
                win.active = None;
                win.splits = session.other.splits.clone();
            }
            if let Some(active) = session.other.active {
                effects.extend(self.focus_tab(active));
            }
        }
        self.apply_profile_tombstone(session.profile);
        self.engine.erase_profile_data(
            session.profile,
            Box::new(|outcome| {
                if outcome != zephium_core::ports::engine::ProfileDataErasureOutcome::Verified {
                    crate::diagnostic!("private: partition erasure did not verify: {outcome:?}");
                }
            }),
        );
        self.commit(effects)
    }
}
