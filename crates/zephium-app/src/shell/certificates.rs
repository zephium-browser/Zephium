//! The way past a refused certificate, offered only for a host on the
//! person's own network, and the warning a page loaded that way keeps.

use super::*;
use zephium_core::certificates::may_continue;
use zephium_core::http_auth::AuthOrigin;
use zephium_core::ports::engine::NavigationFailureReason;

impl Shell {
    pub(super) fn operation_continue_past_certificate(
        &mut self,
        id: ItemId,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(url) = self
            .items
            .tab(id)
            .and_then(|tab| tab.failure.as_deref())
            .filter(|failure| matches!(failure.reason, NavigationFailureReason::Certificate(_)))
            .map(|failure| failure.url.clone())
        else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        let (true, Some(profile), Some(origin)) = (
            may_continue(&url),
            self.profile_of_item(id),
            AuthOrigin::of_url(&url),
        ) else {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidInput);
        };
        // The engine accepts only the leaf it refused at this origin.
        if self.engine.allow_certificate_exception(id, url.to_string()) == NativeDispatch::Rejected
        {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::NativeDispatchRejected,
            );
        }
        self.continued_certificates.insert((profile, origin));
        self.operation_navigate(id, url.to_string())
    }

    /// The tab shows a page from an origin whose certificate the person
    /// continued past.
    pub(super) fn under_certificate_exception(&self, id: ItemId, tab: &TabState) -> bool {
        let Some(origin) = tab.url.as_ref().and_then(AuthOrigin::of_url) else {
            return false;
        };
        self.profile_of_item(id)
            .is_some_and(|profile| self.continued_certificates.contains(&(profile, origin)))
    }
}
