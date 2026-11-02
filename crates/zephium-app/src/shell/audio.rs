//! Sound and mute per tab. The engine reports when a page makes sound; the
//! person mutes. Mute belongs to the tab, so it is sent again to every native
//! view the tab gets, and it ends with the process.

use super::*;

impl Shell {
    pub(super) fn on_audio_changed(&mut self, id: ItemId, audible: bool) {
        let now = std::time::Instant::now();
        if self.items.observe_audio(id, audible, now) {
            if audible {
                self.watch_tab_media(id);
            }
            self.project_tab(id);
            self.sync_extension_browser_surface_metadata(id);
        }
        if let Some(deadline) = self.items.tab(id).and_then(|tab| tab.audio.deadline()) {
            self.audio_quiet.insert(id);
            if let Some(queue) = &self.self_queue {
                queue.schedule_audio(deadline);
            }
        }
    }

    pub(super) fn audio_wake(&mut self) {
        self.settle_audio(std::time::Instant::now());
    }

    /// Hides the speaker of every tab quiet past its hold and waits for the
    /// next one due.
    pub(super) fn settle_audio(&mut self, now: std::time::Instant) {
        let mut next: Option<std::time::Instant> = None;
        for id in std::mem::take(&mut self.audio_quiet) {
            if self.items.settle_audio(id, now) {
                self.project_tab(id);
                self.sync_extension_browser_surface_metadata(id);
            }
            if let Some(deadline) = self.items.tab(id).and_then(|tab| tab.audio.deadline()) {
                self.audio_quiet.insert(id);
                next = Some(next.map_or(deadline, |next| next.min(deadline)));
            }
        }
        if let (Some(deadline), Some(queue)) = (next, &self.self_queue) {
            queue.schedule_audio(deadline);
        }
    }

    /// A tab's view or its process went away, taking any sound with it.
    pub(super) fn silence_tab_audio(&mut self, id: ItemId) {
        self.audio_quiet.remove(&id);
        if self.items.silence_audio(id) {
            self.project_tab(id);
            self.sync_extension_browser_surface_metadata(id);
        }
        self.forget_tab_media(id);
    }

    pub(super) fn operation_set_tab_muted(
        &mut self,
        id: ItemId,
        muted: bool,
    ) -> OperationDisposition {
        // The player may show a tab of another space.
        if !self.item_in_focused_scope(id) && self.media_player_tab() != Some(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if !self.engine.can_mute() {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::UnsupportedCommand,
            );
        }
        let Some(current) = self
            .items
            .tab(id)
            .filter(|tab| tab.content == zephium_core::item::TabContent::Web)
            .map(|tab| tab.audio.muted)
        else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        if current == muted {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        self.items.set_muted(id, muted);
        // A tab without a view takes its mute when one is made for it.
        if self.items.tab(id).is_some_and(TabState::has_view) {
            let _ = self.engine.set_muted(id, muted);
        }
        self.project_tab(id);
        self.sync_extension_browser_surface_metadata(id);
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }

    /// A new native view starts unmuted.
    pub(super) fn restore_tab_mute(&self, id: ItemId) {
        if self.items.tab(id).is_some_and(|tab| tab.audio.muted) {
            let _ = self.engine.set_muted(id, true);
        }
    }
}
