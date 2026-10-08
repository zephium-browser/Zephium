//! What a page asks for that only the person can let through: opening a link
//! in another application, or a new tab the browser refused. Each waits on
//! its tab, in chrome the page cannot draw over, until answered or until the
//! tab commits another document.

use super::*;
use crate::PageRequestDecision;
use zephium_core::item::PageRequest;

impl Shell {
    pub(super) fn on_external_app_requested(
        &mut self,
        id: ItemId,
        url: String,
        app: Option<String>,
    ) {
        let Some(url) = navigation::external_app_link(&url) else {
            return;
        };
        if !self.item_in_focused_scope(id) {
            return;
        }
        if self
            .external_app_grant(id, &url)
            .is_some_and(|grant| self.external_apps_allowed.contains(&grant))
        {
            let _ = self.engine.open_external_app(url.as_str());
            return;
        }
        if self
            .items
            .set_page_request(id, PageRequest::ExternalApp { url, app })
        {
            self.project_tab(id);
        }
    }

    pub(super) fn operation_answer_page_request(
        &mut self,
        id: ItemId,
        decision: PageRequestDecision,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        let Some(request) = self.items.take_page_request(id) else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        self.project_tab(id);
        match (request, decision) {
            (_, PageRequestDecision::Dismiss) | (PageRequest::Popup { url: None }, _) => {
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            (PageRequest::ExternalApp { url, .. }, decision) => {
                if decision == PageRequestDecision::AlwaysAllow {
                    if let Some(grant) = self.external_app_grant(id, &url) {
                        self.external_apps_allowed.insert(grant);
                    }
                }
                match self.engine.open_external_app(url.as_str()) {
                    NativeDispatch::Rejected => operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::NativeDispatchRejected,
                    ),
                    _ => operation_result(
                        OperationOutcome::Applied,
                        OperationReason::MutationApplied,
                    ),
                }
            }
            (PageRequest::Popup { url: Some(url) }, _) => {
                self.operation_open_url(url.to_string(), true)
            }
        }
    }

    /// What an "always" answer is remembered by: the profile, the site the
    /// tab shows, and the kind of link.
    fn external_app_grant(
        &self,
        id: ItemId,
        url: &url::Url,
    ) -> Option<(ProfileId, String, String)> {
        let host = self.items.tab(id)?.url.as_ref()?.host_str()?.to_owned();
        Some((self.profile_of_item(id)?, host, url.scheme().to_owned()))
    }
}
