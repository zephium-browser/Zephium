//! A page the filter lists refused. Chrome covers the tab and says why; the
//! person leaves it, or opens it anyway and its host stays allowed until the
//! app quits.

use super::*;
use zephium_core::ports::engine::{NavigationBlockReason, NavigationFailureReason};

impl Shell {
    pub(super) fn on_navigation_blocked(
        &mut self,
        id: ItemId,
        url: String,
        reason: NavigationBlockReason,
    ) {
        let Some(url) = url::Url::parse(&url)
            .ok()
            .filter(navigation::is_browser_target)
        else {
            return;
        };
        if self.items.record_navigation_failure(
            id,
            NavigationFailureReason::Blocked(reason),
            Some(url),
            true,
        ) {
            // The refused page never started; nothing is loading behind it.
            self.items.set_loading(id, false);
            let _ = self.relayout();
            self.project_tab(id);
        }
    }

    fn blocked_url(&self, id: ItemId) -> Option<url::Url> {
        self.items
            .tab(id)
            .and_then(|tab| tab.failure.as_deref())
            .filter(|failure| matches!(failure.reason, NavigationFailureReason::Blocked(_)))
            .map(|failure| failure.url.clone())
    }

    pub(super) fn operation_continue_past_block(&mut self, id: ItemId) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(url) = self.blocked_url(id) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        if self.engine.allow_blocked_navigation(id, url.to_string()) == NativeDispatch::Rejected {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::NativeDispatchRejected,
            );
        }
        self.operation_navigate(id, url.to_string())
    }

    /// Back to the page that was showing, or back in history when the
    /// refused page was the tab's first.
    pub(super) fn operation_leave_blocked_page(&mut self, id: ItemId) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if self.blocked_url(id).is_none() {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let Some((shown, can_go_back)) = self
            .items
            .tab(id)
            .map(|tab| (tab.url.is_some(), tab.can_go_back))
        else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        self.items.clear_navigation_failure(id);
        let _ = self.relayout();
        self.project_tab(id);
        if !shown && can_go_back {
            return self.operation_history(id, false);
        }
        operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
    }
}
