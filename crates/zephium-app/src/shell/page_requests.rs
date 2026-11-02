//! What a page asks for that only the person can let through: opening a link
//! in another application, a new tab the browser refused, or signing in to
//! the site; and a page that stopped responding. Each waits on its tab, in
//! chrome the page cannot draw over, until answered or until the tab commits
//! another document.

use super::*;
use crate::PageRequestDecision;
use zephium_core::http_auth::{HttpAuthPrompt, HttpCredentials};
use zephium_core::item::PageRequest;

/// After the person chooses to wait, a page still hung this long later is
/// asked about again.
pub(super) const UNRESPONSIVE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

impl Shell {
    /// Asks about a hung page only while it is the tab in front, and not again
    /// soon after the person chose to wait. A page that answers again takes
    /// the question away.
    pub(super) fn on_page_responsiveness(&mut self, id: ItemId, responding: bool) {
        let shown = self.unresponsive_shown(id);
        if responding {
            self.crash.unresponsive_waits.remove(&id);
            if shown {
                let _ = self.items.take_page_request(id);
                self.project_tab(id);
            }
            return;
        }
        let in_front = self.windows.focused().and_then(|window| window.active) == Some(id);
        let waiting = self
            .crash
            .unresponsive_waits
            .get(&id)
            .is_some_and(|since| since.elapsed() < UNRESPONSIVE_WAIT);
        if shown || waiting || !in_front || !self.items.tab(id).is_some_and(TabState::has_view) {
            return;
        }
        if self.items.set_page_request(id, PageRequest::Unresponsive) {
            self.project_tab(id);
        }
    }

    fn unresponsive_shown(&self, id: ItemId) -> bool {
        self.items
            .tab(id)
            .and_then(|tab| tab.page_request.as_deref())
            .is_some_and(|request| matches!(request, PageRequest::Unresponsive))
    }

    pub(super) fn on_external_app_requested(
        &mut self,
        id: ItemId,
        url: String,
        app: Option<String>,
        user_gesture: bool,
    ) {
        let Some(url) = navigation::external_app_link(&url) else {
            return;
        };
        if navigation::app_link_outcome(app.is_some(), user_gesture)
            == navigation::AppLinkOutcome::Silent
        {
            return;
        }
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

    pub(super) fn on_http_auth_requested(&mut self, id: ItemId, prompt: HttpAuthPrompt) {
        let request = prompt.request;
        if self
            .items
            .set_page_request(id, PageRequest::HttpAuth(prompt))
        {
            self.project_tab(id);
        } else {
            let _ = self.engine.answer_http_auth(id, request, None);
        }
    }

    pub(super) fn on_http_auth_ended(&mut self, id: ItemId, request: u32) {
        self.end_http_auth_prompt(id, Some(request));
    }

    /// Takes down the tab's sign-in prompt, or only `request` when given.
    pub(super) fn end_http_auth_prompt(&mut self, id: ItemId, request: Option<u32>) {
        if self
            .http_auth_prompt(id)
            .is_some_and(|shown| request.is_none_or(|r| r == shown))
        {
            let _ = self.items.take_page_request(id);
            self.project_tab(id);
        }
    }

    fn http_auth_prompt(&self, id: ItemId) -> Option<u32> {
        match self.items.tab(id)?.page_request.as_deref()? {
            PageRequest::HttpAuth(prompt) => Some(prompt.request),
            _ => None,
        }
    }

    pub(super) fn operation_answer_http_auth(
        &mut self,
        id: ItemId,
        request: u32,
        credentials: Option<HttpCredentials>,
    ) -> OperationDisposition {
        if !self.item_in_focused_scope(id) {
            return operation_result(OperationOutcome::Rejected, OperationReason::InvalidScope);
        }
        if self.http_auth_prompt(id) != Some(request) {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        }
        let _ = self.items.take_page_request(id);
        self.project_tab(id);
        match self.engine.answer_http_auth(id, request, credentials) {
            NativeDispatch::Rejected => operation_result(
                OperationOutcome::Rejected,
                OperationReason::NativeDispatchRejected,
            ),
            _ => operation_result(OperationOutcome::Applied, OperationReason::MutationApplied),
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
            // Signing in has its own answer; anything else here cancels.
            (PageRequest::HttpAuth(prompt), _) => {
                let _ = self.engine.answer_http_auth(id, prompt.request, None);
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            (PageRequest::Popup { url }, PageRequestDecision::AlwaysAllow) => {
                if !self.allow_popups_for_tab(id) {
                    return operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::StoreAdmissionRejected,
                    );
                }
                match url {
                    Some(url) => self.operation_open_url(url.to_string(), true),
                    None => operation_result(
                        OperationOutcome::Applied,
                        OperationReason::MutationApplied,
                    ),
                }
            }
            (PageRequest::Unresponsive, PageRequestDecision::Dismiss) => {
                self.crash
                    .unresponsive_waits
                    .insert(id, std::time::Instant::now());
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            // Reload: the hung view is closed and the crash path brings the
            // page back in a fresh one.
            (PageRequest::Unresponsive, _) => {
                self.crash.unresponsive_waits.remove(&id);
                match self.engine.end_unresponsive_page(id) {
                    NativeDispatch::Rejected | NativeDispatch::Unsupported => operation_result(
                        OperationOutcome::Rejected,
                        OperationReason::NativeDispatchRejected,
                    ),
                    _ => operation_result(
                        OperationOutcome::Applied,
                        OperationReason::MutationApplied,
                    ),
                }
            }
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
