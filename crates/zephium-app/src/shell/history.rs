//! Profile-scoped history reads and deletions for the history surfaces.

use super::*;

use crate::api::HistoryCompletion;
use zephium_ipc::{
    ClosedTabView, HistoryCall, HistoryError, HistoryResponse, HistoryVisitView, IconSurface,
};

/// Completions outlive one actor turn, so the map is bounded by the same
/// number of requests the read queue will accept.
const MAX_PENDING_HISTORY_CALLS: usize = 8;

/// How many closed tabs the menus list.
const MENU_CLOSED_TABS: usize = 10;

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
        match &call {
            HistoryCall::ClosedTabs => {
                done.finish(self.closed_tabs_response(expected_profile));
                return;
            }
            HistoryCall::ReopenClosed { id } => {
                let reopened = zephium_core::ids::ClosedSessionId::parse(id).is_some_and(|id| {
                    matches!(
                        self.operation_reopen_closed_tab(Some(id)).outcome,
                        OperationOutcome::Applied | OperationOutcome::Deferred
                    )
                });
                done.finish(if reopened {
                    HistoryResponse::Reopened
                } else {
                    HistoryResponse::Error {
                        error: HistoryError::Invalid,
                    }
                });
                return;
            }
            _ => {}
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
        let clearing = match &call {
            HistoryCall::Clear { range } => Some(
                range
                    .window_seconds()
                    .map(|window| chrono::Utc::now().timestamp() - window),
            ),
            HistoryCall::ClearSince { since } => Some(Some(*since)),
            _ => None,
        };
        if let Some(since) = clearing {
            if !self.clear_time(expected_profile, since) {
                done.finish(HistoryResponse::Error {
                    error: HistoryError::Unavailable,
                });
                return;
            }
            self.reset_blocker_statistics(expected_profile);
            self.clear_zoom_levels(expected_profile, since);
            self.forget_recently_closed(expected_profile, since);
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
            HistoryCall::Forget { .. } | HistoryCall::Clear { .. } | HistoryCall::ClearSince { .. }
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

    /// Closed tabs are history too: a clear takes the profile's closed after
    /// `since` (unix seconds), or all of them, out of Recently Closed. A tab
    /// closed by an older release carries no time and goes only with all.
    fn forget_recently_closed(&mut self, profile: ProfileId, since: Option<i64>) {
        let floor = since.map(|since| u64::try_from(since).unwrap_or(0).saturating_mul(1000));
        let before = self.recently_closed.len();
        self.recently_closed.retain(|entry| {
            entry.profile != profile
                || floor.is_some_and(|floor| entry.closed_at_ms.is_some_and(|at| at < floor))
        });
        if self.recently_closed.len() != before {
            self.project_recently_closed_if_changed();
            self.schedule_persist();
            self.extension_sessions_changed(profile);
        }
    }

    /// What a profile can reopen by name, newest first. Recently Closed
    /// lives in the shell, not the store.
    fn reopenable(
        &self,
        profile: ProfileId,
    ) -> impl Iterator<
        Item = (
            zephium_core::ids::ClosedSessionId,
            &zephium_core::session::PersistedClosedTab,
        ),
    > {
        self.recently_closed
            .iter()
            .rev()
            .filter(move |entry| entry.profile == profile)
            .filter_map(|entry| Some((entry.session_id?, entry)))
    }

    /// Tells the menus when the focused profile's newest closed tabs change.
    pub(super) fn project_recently_closed_if_changed(&mut self) {
        let profile = self
            .windows
            .focused()
            .map(|window| window.profile)
            .filter(|profile| !self.incognito_profile(*profile));
        let now: Vec<_> = profile
            .into_iter()
            .flat_map(|profile| {
                self.reopenable(profile)
                    .take(MENU_CLOSED_TABS)
                    .map(move |(id, _)| (profile, id))
            })
            .collect();
        if now == self.closed_projected {
            return;
        }
        let tabs = profile
            .into_iter()
            .flat_map(|profile| {
                self.reopenable(profile)
                    .take(MENU_CLOSED_TABS)
                    .map(|(id, entry)| ClosedTabView {
                        id: id.to_string(),
                        url: entry.url.clone(),
                        title: entry.title.clone(),
                        icon: None,
                    })
            })
            .collect();
        self.closed_projected = now;
        (self.emit)(Projection::RecentlyClosed(tabs));
    }

    fn closed_tabs_response(&mut self, profile: ProfileId) -> HistoryResponse {
        let tabs = self
            .reopenable(profile)
            .map(|(id, entry)| ClosedTabView {
                id: id.to_string(),
                icon: self.icon_ref_for_url(IconSurface::Chrome, profile, &entry.url),
                url: entry.url.clone(),
                title: entry.title.clone(),
            })
            .collect::<Vec<_>>();
        self.want_icons(
            IconSurface::Chrome,
            profile,
            tabs.iter()
                .filter(|tab| tab.icon.is_none())
                .map(|tab| tab.url.as_str()),
        );
        self.publish_icons();
        HistoryResponse::ClosedTabs { tabs }
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
