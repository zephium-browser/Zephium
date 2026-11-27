//! The entries a tab's Back and Forward buttons can reach, read from the
//! engine on request for the native menu those buttons open.

use super::*;

use crate::api::HistoryListCompletion;
use zephium_core::ports::engine::{HistoryEntries, HistoryRequestId};

/// Menus waiting on the engine; an older one is answered empty to make room.
const MAX_PENDING_LISTS: usize = 4;

#[derive(Default)]
pub(super) struct HistoryListState {
    pending: std::collections::VecDeque<(HistoryRequestId, ItemId, HistoryListCompletion)>,
    next_request: u64,
}

impl Shell {
    pub(super) fn history_list(&mut self, id: ItemId, done: HistoryListCompletion) {
        let listable = self.item_in_focused_scope(id)
            && self.items.tab(id).is_some_and(|tab| {
                tab.content == zephium_core::item::TabContent::Web
                    && tab.has_view()
                    && (tab.can_go_back || tab.can_go_forward)
            });
        if !listable {
            done.finish(HistoryEntries::default());
            return;
        }
        if self.history_lists.pending.len() >= MAX_PENDING_LISTS {
            if let Some((_, _, oldest)) = self.history_lists.pending.pop_front() {
                oldest.finish(HistoryEntries::default());
            }
        }
        self.history_lists.next_request = self.history_lists.next_request.wrapping_add(1);
        let request = HistoryRequestId(self.history_lists.next_request);
        if self.engine.history_entries(id, request) == NativeDispatch::Scheduled {
            self.history_lists.pending.push_back((request, id, done));
        } else {
            done.finish(HistoryEntries::default());
        }
    }

    pub(super) fn on_history_entries(
        &mut self,
        id: ItemId,
        request: HistoryRequestId,
        entries: HistoryEntries,
    ) {
        let at = self
            .history_lists
            .pending
            .iter()
            .position(|(asked, tab, _)| *asked == request && *tab == id);
        if let Some((_, _, done)) = at.and_then(|at| self.history_lists.pending.remove(at)) {
            done.finish(entries);
        }
    }

    /// Goes to one entry of a list the person was shown. The engine checks
    /// the entry is still at `url` before moving.
    pub(super) fn operation_go_to_history_entry(
        &mut self,
        id: ItemId,
        offset: i32,
        url: String,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let reachable = self.items.tab(id).is_some_and(|tab| {
            tab.has_view()
                && if offset < 0 {
                    tab.can_go_back
                } else {
                    tab.can_go_forward
                }
        });
        let bounded = offset != 0
            && offset.unsigned_abs() as usize <= zephium_core::ports::engine::MAX_HISTORY_SIDE;
        if !reachable || !bounded || !navigation::is_allowed_str(&url) {
            return operation_result(OperationOutcome::NoOp, OperationReason::HistoryUnavailable);
        }
        if self.recreate_after_inflight_discard(id) {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::HistoryUnavailable,
            );
        }
        self.cancel_discard_probe(id);
        self.cancel_page_permission_for_item(id);
        let mut native = NativeWork::default();
        native.record(self.engine.go_to_history_entry(id, offset, &url));
        mutation_result(native)
    }

    /// Every menu is answered, including when the shell is shutting down.
    pub(super) fn fail_pending_history_lists(&mut self) {
        for (_, _, done) in std::mem::take(&mut self.history_lists.pending) {
            done.finish(HistoryEntries::default());
        }
    }
}
