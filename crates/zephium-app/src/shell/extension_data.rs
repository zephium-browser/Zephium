//! Extensions' history, bookmarks, top sites and closed tabs. Store work
//! runs on the store-read worker; changes reach the engine only for what
//! some extension in the profile listens to.

use super::*;

use crate::store_reads::{ExtensionDataWork, MAX_PENDING_EXTENSION_CALLS};
use zephium_core::extension_data::{
    ClosedTab, ExtensionDataCall, ExtensionDataChange, ExtensionDataInterest, ExtensionDataReply,
    HistoryReply, HistoryRequest,
};
use zephium_core::ports::engine::WebExtensionDataRequest;

#[derive(Default)]
pub(super) struct ExtensionDataState {
    /// Store reads by token, with the engine request each answers.
    pending: std::collections::HashMap<u64, (ProfileId, u64)>,
    next_token: u64,
    interest: std::collections::HashMap<ProfileId, ExtensionDataInterest>,
}

/// Lookups for events carry no request to answer.
const EVENT_TOKEN: u64 = 0;

impl Shell {
    fn extension_data_interest(&self, profile: ProfileId) -> ExtensionDataInterest {
        self.extension_data
            .interest
            .get(&profile)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn set_extension_data_interest(
        &mut self,
        profile: ProfileId,
        interest: ExtensionDataInterest,
    ) {
        if interest.any() {
            self.extension_data.interest.insert(profile, interest);
        } else {
            self.extension_data.interest.remove(&profile);
        }
        if let Some(reads) = &self.store_reads {
            reads.set_interest(profile, interest);
        }
    }

    fn answer_extension_data(&self, profile: ProfileId, request: u64, reply: ExtensionDataReply) {
        if self
            .engine
            .answer_web_extension_data(profile, request, reply)
            != NativeDispatch::Scheduled
        {
            crate::diagnostic!("extensions: data answer was not admitted");
        }
    }

    pub(super) fn on_extension_data_request(&mut self, request: WebExtensionDataRequest) {
        let WebExtensionDataRequest {
            profile,
            request,
            call,
        } = request;
        let keeps_data = self.bootstrapped
            && self.profiles.get(profile).is_some()
            && !self.incognito_profile(profile)
            && !self.degraded_storage_profiles.contains(&profile)
            && !self.profile_deletion_quarantines(profile);
        if !keeps_data || !call.validate() {
            return self.answer_extension_data(profile, request, ExtensionDataReply::Unavailable);
        }
        let work = match call {
            ExtensionDataCall::RecentlyClosed { limit } => {
                let closed = self.closed_tabs(profile).take(limit as usize).collect();
                return self.answer_extension_data(
                    profile,
                    request,
                    ExtensionDataReply::Closed(closed),
                );
            }
            ExtensionDataCall::Restore { session } => {
                let reply = self.extension_restore(profile, session.as_deref());
                return self.answer_extension_data(profile, request, reply);
            }
            ExtensionDataCall::History(HistoryRequest::Add { url }) => {
                self.store.record_visit(profile, url.clone(), String::new());
                self.extension_visit_recorded(profile, &url);
                return self.answer_extension_data(
                    profile,
                    request,
                    ExtensionDataReply::History(HistoryReply::Entries(Vec::new())),
                );
            }
            ExtensionDataCall::History(request) => ExtensionDataWork::History(request),
            ExtensionDataCall::Bookmarks(request) => ExtensionDataWork::Bookmarks(request),
        };
        let removes_history = matches!(
            work,
            ExtensionDataWork::History(
                HistoryRequest::RemoveUrl { .. }
                    | HistoryRequest::RemoveRange { .. }
                    | HistoryRequest::RemoveAll
            )
        );
        let token = self.extension_data.next_token.wrapping_add(1).max(1);
        self.extension_data.next_token = token;
        let admitted = self.extension_data.pending.len() < MAX_PENDING_EXTENSION_CALLS
            && self
                .store_reads
                .as_ref()
                .is_some_and(|reads| reads.request_extension_data(token, profile, work));
        if !admitted {
            return self.answer_extension_data(profile, request, ExtensionDataReply::Busy);
        }
        self.extension_data
            .pending
            .insert(token, (profile, request));
        // Opaque back/forward state must not bring removed history back.
        if removes_history {
            self.engine.forget_discarded_state(profile, None);
        }
    }

    pub(super) fn on_extension_data_read(&mut self, token: u64, reply: ExtensionDataReply) {
        if let Some((profile, request)) = self.extension_data.pending.remove(&token) {
            self.answer_extension_data(profile, request, reply);
        }
    }

    pub(super) fn on_extension_data_changed(
        &mut self,
        profile: ProfileId,
        change: ExtensionDataChange,
    ) {
        let interest = self.extension_data_interest(profile);
        let heard = match change {
            ExtensionDataChange::Visited(_) | ExtensionDataChange::VisitsRemoved { .. } => {
                interest.history
            }
            ExtensionDataChange::Bookmark(_) => interest.bookmarks,
            ExtensionDataChange::Sessions => interest.sessions,
        };
        if heard
            && self.engine.web_extension_data_changed(profile, change) != NativeDispatch::Scheduled
        {
            crate::diagnostic!("extensions: data change was not admitted");
        }
    }

    /// A visit was recorded; listeners hear it once the store has it.
    pub(super) fn extension_visit_recorded(&mut self, profile: ProfileId, url: &str) {
        if !self.extension_data_interest(profile).history {
            return;
        }
        if let Some(reads) = &self.store_reads {
            let _ = reads.request_extension_data(
                EVENT_TOKEN,
                profile,
                ExtensionDataWork::Visited(url.to_owned()),
            );
        }
    }

    pub(super) fn extension_sessions_changed(&mut self, profile: ProfileId) {
        self.on_extension_data_changed(profile, ExtensionDataChange::Sessions);
    }

    fn closed_tabs(&self, profile: ProfileId) -> impl Iterator<Item = ClosedTab> + '_ {
        self.recently_closed
            .iter()
            .rev()
            .filter(move |entry| entry.profile == profile)
            .filter_map(|entry| {
                Some(ClosedTab {
                    session: entry.session_id?.to_string(),
                    url: entry.url.clone(),
                    title: entry.title.clone(),
                    closed_at_ms: entry.closed_at_ms,
                })
            })
    }

    /// Reopens as the Reopen Closed Tab command does, in the focused window
    /// of the extension's profile only.
    fn extension_restore(
        &mut self,
        profile: ProfileId,
        session: Option<&str>,
    ) -> ExtensionDataReply {
        let focused = self
            .windows
            .focused()
            .is_some_and(|window| window.profile == profile);
        let named = match session {
            Some(id) => match zephium_core::ids::ClosedSessionId::parse(id) {
                Some(id) => Some(id),
                None => return ExtensionDataReply::Unavailable,
            },
            None => None,
        };
        let Some(closed) = self
            .closed_tabs(profile)
            .find(|tab| session.is_none_or(|id| tab.session == id))
        else {
            return ExtensionDataReply::Unavailable;
        };
        if !focused {
            return ExtensionDataReply::Unavailable;
        }
        match self.operation_reopen_closed_tab(named).outcome {
            OperationOutcome::Applied | OperationOutcome::Deferred => {
                ExtensionDataReply::Restored(closed)
            }
            _ => ExtensionDataReply::Unavailable,
        }
    }

    pub(super) fn fail_pending_extension_data(&mut self) {
        for (_, (profile, request)) in std::mem::take(&mut self.extension_data.pending) {
            self.answer_extension_data(profile, request, ExtensionDataReply::Unavailable);
        }
    }
}
