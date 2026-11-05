//! What a page asks for that only the person can let through: opening a link
//! in another application, a new tab the browser refused, saving a file, or
//! signing in to the site; and a page that stopped responding. Each waits on its tab, in
//! chrome the page cannot draw over, until answered or until the tab commits
//! another document.

use super::*;
use crate::PageRequestDecision;
use zephium_core::http_auth::{HttpAuthPrompt, HttpCredentials};
use zephium_core::item::PageRequest;
use zephium_core::permissions::{DownloadPermissionPrompt, PageOrigin};

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
        initiator: navigation::AppLinkSource,
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
        // Only a site the engine can name may open by a kept answer; one it
        // cannot name is asked about without naming any site.
        let site = match initiator {
            navigation::AppLinkSource::Page => self.tab_origin(id),
            navigation::AppLinkSource::Frame(origin) => Some(origin),
            navigation::AppLinkSource::Unknown => None,
        };
        if site
            .as_ref()
            .is_some_and(|site| self.app_link_allowed(id, site, url.scheme()))
        {
            let _ = self.engine.open_external_app(url.as_str());
            return;
        }
        if self
            .items
            .set_page_request(id, PageRequest::ExternalApp { url, app, site })
        {
            self.project_tab(id);
        }
    }

    pub(super) fn on_http_auth_requested(&mut self, id: ItemId, prompt: HttpAuthPrompt) {
        let request = prompt.request;
        self.withdraw_download_prompt(id);
        if self
            .items
            .set_page_request(id, PageRequest::HttpAuth(prompt))
        {
            self.project_tab(id);
        } else {
            let _ = self.engine.answer_http_auth(id, request, None);
        }
    }

    /// A download waits on this tab for the person to answer.
    pub(super) fn on_download_permission_requested(
        &mut self,
        id: ItemId,
        prompt: DownloadPermissionPrompt,
    ) {
        let request = prompt.request;
        let shown = matches!(
            self.items
                .tab(id)
                .and_then(|tab| tab.page_request.as_deref()),
            None | Some(PageRequest::Popup { .. } | PageRequest::ExternalApp { .. })
        ) || self.download_prompt(id).is_some();
        if shown {
            self.withdraw_download_prompt(id);
        }
        if shown
            && self
                .items
                .set_page_request(id, PageRequest::Download(prompt))
        {
            self.project_tab(id);
        } else {
            let _ = self.engine.answer_download_permission(id, request, false);
        }
    }

    pub(super) fn on_download_permission_ended(&mut self, id: ItemId, request: u32) {
        if self.download_prompt(id) == Some(request) {
            let _ = self.items.take_page_request(id);
            self.project_tab(id);
        }
    }

    fn download_prompt(&self, id: ItemId) -> Option<u32> {
        match self.items.tab(id)?.page_request.as_deref()? {
            PageRequest::Download(prompt) => Some(prompt.request),
            _ => None,
        }
    }

    /// Something else takes the tab's card: the download it showed is
    /// refused rather than left waiting unseen.
    fn withdraw_download_prompt(&mut self, id: ItemId) {
        if let Some(request) = self.download_prompt(id) {
            let _ = self.items.take_page_request(id);
            let _ = self.engine.answer_download_permission(id, request, false);
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
        let Some(request) = self
            .items
            .tab(id)
            .and_then(|tab| tab.page_request.as_deref())
            .cloned()
        else {
            return operation_result(OperationOutcome::NoOp, OperationReason::StateUnchanged);
        };
        // A lasting answer is kept before anything opens; one that cannot be
        // kept leaves the card up to say so.
        if decision == PageRequestDecision::AlwaysAllow
            && !self.keep_page_request_answer(id, &request)
        {
            return operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreAdmissionRejected,
            );
        }
        let _ = self.items.take_page_request(id);
        self.project_tab(id);
        match (request, decision) {
            // Signing in has its own answer; anything else here cancels.
            (PageRequest::HttpAuth(prompt), _) => {
                let _ = self.engine.answer_http_auth(id, prompt.request, None);
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            }
            (PageRequest::Download(prompt), decision) => {
                self.answer_download_prompt(id, prompt, decision)
            }
            (PageRequest::Popup { url }, PageRequestDecision::AlwaysAllow) => match url {
                Some(url) => self.operation_open_url(url.to_string(), true),
                None => {
                    operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
                }
            },
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
            (PageRequest::ExternalApp { url, .. }, _) => {
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

    /// Keeps an Always allow for the site that asked. A download keeps its
    /// own answer, and a card that names no site keeps nothing.
    fn keep_page_request_answer(&mut self, id: ItemId, request: &PageRequest) -> bool {
        match request {
            PageRequest::ExternalApp {
                url,
                site: Some(site),
                ..
            } => self.profile_of_item(id).is_some_and(|profile| {
                self.set_app_link_site(profile, site.clone(), url.scheme().to_owned(), true)
            }),
            PageRequest::Popup { .. } => self.allow_popups_for_tab(id),
            _ => true,
        }
    }

    /// Allow keeps the site allowed, except for a download nobody clicked,
    /// which is let through once; anything else refuses this download only.
    fn answer_download_prompt(
        &mut self,
        id: ItemId,
        prompt: DownloadPermissionPrompt,
        decision: PageRequestDecision,
    ) -> OperationDisposition {
        let mut allow = matches!(
            decision,
            PageRequestDecision::Allow | PageRequestDecision::AlwaysAllow
        );
        let profile = self.profile_of_item(id);
        let remembered = profile.and_then(|profile| {
            self.ensure_download_sites(profile);
            self.download_sites
                .get(&profile)?
                .get(&prompt.site)
                .copied()
        });
        // Blocked from Site settings while the card waited: the block stands.
        let refused =
            allow && !zephium_core::permissions::download_answer_allows(allow, remembered, false);
        if refused {
            allow = false;
        }
        if allow && !prompt.allowed {
            if let Some(profile) = profile {
                if !self.set_download_site(profile, prompt.site.clone(), Some(true)) {
                    crate::diagnostic!("downloads: the site's answer could not be kept");
                }
            }
        }
        match self
            .engine
            .answer_download_permission(id, prompt.request, allow)
        {
            NativeDispatch::Rejected | NativeDispatch::Unsupported => operation_result(
                OperationOutcome::Rejected,
                OperationReason::NativeDispatchRejected,
            ),
            _ if refused => {
                operation_result(OperationOutcome::Rejected, OperationReason::StoreConflict)
            }
            _ => operation_result(OperationOutcome::Applied, OperationReason::MutationApplied),
        }
    }

    /// The site a tab shows, as an origin.
    fn tab_origin(&self, id: ItemId) -> Option<PageOrigin> {
        PageOrigin::from_url(self.items.tab(id)?.url.as_ref()?).ok()
    }

    fn app_link_allowed(&mut self, id: ItemId, site: &PageOrigin, scheme: &str) -> bool {
        let Some(profile) = self.profile_of_item(id) else {
            return false;
        };
        self.ensure_app_link_sites(profile);
        self.app_link_sites
            .get(&profile)
            .is_some_and(|sites| sites.contains(&(site.clone(), scheme.to_owned())))
    }
}
