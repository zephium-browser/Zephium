//! Profile-scoped history reads and deletions for the history surfaces.

use super::*;

use crate::api::HistoryCompletion;
use zephium_ipc::{HistoryCall, HistoryError, HistoryResponse, HistoryVisitView, IconSurface};

/// Completions outlive one actor turn, so the map is bounded by the same
/// number of requests the read queue will accept.
const MAX_PENDING_HISTORY_CALLS: usize = 8;

#[derive(Default)]
pub(super) struct HistoryState {
    pending: std::collections::HashMap<u64, HistoryCompletion>,
    next_token: u64,
}

impl Shell {
    pub(super) fn history_call(
        &mut self,
        expected_profile: ProfileId,
        call: HistoryCall,
        done: HistoryCompletion,
    ) {
        let authorized = self
            .windows
            .focused()
            .map(|window| window.profile)
            .is_some_and(|profile| profile == expected_profile);
        if !authorized || !call.validate() {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Invalid,
            });
            return;
        }
        // A degraded profile answers every read with nothing and drops every
        // write. Say so: an empty list here reads as "you have no history".
        if self.degraded_storage_profiles.contains(&expected_profile) {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Unavailable,
            });
            return;
        }
        if self.history.pending.len() >= MAX_PENDING_HISTORY_CALLS {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Capacity,
            });
            return;
        }
        if let HistoryCall::Clear { range } = &call {
            let since = range
                .window_seconds()
                .map(|window| chrono::Utc::now().timestamp() - window);
            if !self.clear_time(expected_profile, since) {
                done.finish(HistoryResponse::Error {
                    error: HistoryError::Unavailable,
                });
                return;
            }
            self.reset_blocker_statistics(expected_profile);
        }
        let Some(reads) = &self.store_reads else {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Unavailable,
            });
            return;
        };
        self.history.next_token = self.history.next_token.wrapping_add(1);
        let token = self.history.next_token;
        let forget_native_state = matches!(
            &call,
            HistoryCall::Forget { .. } | HistoryCall::Clear { .. }
        );
        if !reads.request_history_call(token, expected_profile, call) {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Unavailable,
            });
            return;
        }
        self.history.pending.insert(token, done);
        // Opaque native back/forward state must not resurrect deleted history.
        // Clear it only after this validated deletion entered the store queue.
        // A failed queue admission retains both history and restoration state.
        // The native hook also invalidates an in-flight state capture.
        if forget_native_state {
            self.engine.forget_discarded_state(expected_profile, None);
        }
    }

    pub(super) fn on_history_surface_read(
        &mut self,
        token: u64,
        profile: ProfileId,
        visits: Vec<zephium_core::ports::store::HistoryVisit>,
        next: Option<i64>,
        removed: Option<u32>,
    ) {
        let Some(done) = self.history.pending.remove(&token) else {
            return;
        };
        if let Some(count) = removed {
            done.finish(HistoryResponse::Removed { count });
            return;
        }
        let visits: Vec<HistoryVisitView> = visits
            .into_iter()
            .map(|visit| HistoryVisitView {
                id: visit.id.to_string(),
                icon: self.icon_ref_for_url(IconSurface::Chrome, profile, &visit.url),
                url: visit.url,
                title: visit.title,
                visited_at: visit.visited_at.to_string(),
            })
            .collect();
        self.want_icons(
            IconSurface::Chrome,
            profile,
            visits
                .iter()
                .filter(|visit| visit.icon.is_none())
                .map(|visit| visit.url.as_str()),
        );
        self.publish_icons();
        done.finish(HistoryResponse::Page {
            visits,
            next: next.map(|id| id.to_string()),
        });
    }

    /// Every completion is answered, including when the shell is shutting down
    /// and the read queue will never deliver.
    pub(super) fn fail_pending_history_calls(&mut self) {
        for (_, done) in std::mem::take(&mut self.history.pending) {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Unavailable,
            });
        }
    }

    pub(super) fn on_history_surface_failed(&mut self, token: u64, _profile: ProfileId) {
        if let Some(done) = self.history.pending.remove(&token) {
            done.finish(HistoryResponse::Error {
                error: HistoryError::Unavailable,
            });
        }
    }
}
