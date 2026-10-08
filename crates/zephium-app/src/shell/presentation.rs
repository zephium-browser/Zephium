//! Exact navigation-presentation attribution and privileged-chrome acknowledgement.

use super::*;

// A committed document is acknowledged as soon as privileged chrome verifies
// its exact revision-bearing URL projection. This deadline bounds only
// callback/native-dispatch admission retries; it is never an intentional
// first-paint delay or authority for a timeout reveal.
const PRESENTATION_ADMISSION_HARD_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);
const PRESENTATION_ADMISSION_RETRY_DELAYS: [std::time::Duration; 7] = [
    std::time::Duration::from_millis(25),
    std::time::Duration::from_millis(50),
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(200),
    std::time::Duration::from_millis(400),
    std::time::Duration::from_millis(800),
    std::time::Duration::from_secs(1),
];
pub(super) const MAX_PRESENTATION_ADMISSION_REJECTIONS: u8 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingPresentation {
    pub(super) navigation: NavigationPresentationId,
    pub(super) url: String,
    pub(super) hard_deadline: std::time::Instant,
    pub(super) admission_rejections: u8,
    pub(super) chrome_applied: bool,
    pub(super) chrome_request_in_flight: bool,
}

#[derive(Default)]
pub(super) struct PresentationState {
    pub(super) pending_presentations: std::collections::HashMap<ItemId, PendingPresentation>,
    pub(super) presented_navigations:
        std::collections::HashMap<ItemId, (NavigationPresentationId, String)>,
    /// A fresh tab keeps its real privileged New Tab document until the first
    /// exact committed-URL presentation eval replaces and verifies it. Native
    /// content geometry is admitted only after that callback.
    pub(super) deferred_first_content_layout: std::collections::HashSet<ItemId>,
    /// Last revision actually offered to privileged chrome for each item.
    /// Exact eval callbacks must still match this value when the actor
    /// receives them; a newer masked or full projection invalidates an older
    /// success without unrelated-tab churn causing starvation.
    pub(super) last_tab_projection_revision:
        std::cell::RefCell<std::collections::HashMap<ItemId, String>>,
    /// The last ordinary view offered for each tab. A tab whose view is
    /// unchanged keeps its revision, so chrome keeps the same object and a
    /// tab switch does not re-render every row in the column.
    pub(super) last_tab_views: std::cell::RefCell<std::collections::HashMap<ItemId, TabView>>,
    pub(super) projection_sequence: std::cell::Cell<u128>,
}

impl Shell {
    fn track_pending_presentation(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
        url: String,
        now: std::time::Instant,
    ) -> PendingPresentation {
        let candidate_hard = now
            .checked_add(PRESENTATION_ADMISSION_HARD_LIMIT)
            .unwrap_or(now);
        let pending = match self.presentation.pending_presentations.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                // A hidden view may commit another navigation before the
                // first one presents. Advance the exact identity but preserve
                // the original absolute cap so navigation churn cannot keep
                // trusted backing over executable content indefinitely.
                let hard_deadline = entry.get().hard_deadline.min(candidate_hard);
                let admission_rejections = entry.get().admission_rejections;
                let same_fact = entry.get().navigation == navigation && entry.get().url == url;
                let pending = PendingPresentation {
                    navigation,
                    url,
                    hard_deadline,
                    admission_rejections,
                    chrome_applied: same_fact && entry.get().chrome_applied,
                    chrome_request_in_flight: same_fact && entry.get().chrome_request_in_flight,
                };
                entry.insert(pending.clone());
                pending
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let pending = PendingPresentation {
                    navigation,
                    url,
                    hard_deadline: candidate_hard,
                    admission_rejections: 0,
                    chrome_applied: false,
                    chrome_request_in_flight: false,
                };
                entry.insert(pending.clone());
                pending
            }
        };
        pending
    }

    pub(super) fn cancel_pending_presentation(&mut self, id: ItemId) {
        self.presentation.pending_presentations.remove(&id);
        self.presentation.presented_navigations.remove(&id);
        self.presentation.deferred_first_content_layout.remove(&id);
        if let Ok(mut revisions) = self
            .presentation
            .last_tab_projection_revision
            .try_borrow_mut()
        {
            revisions.remove(&id);
        }
        if let Some(queue) = &self.self_queue {
            queue.cancel_presentation(id);
        }
    }

    fn cancel_exact_pending_presentation(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
    ) -> bool {
        if !self
            .presentation
            .pending_presentations
            .get(&id)
            .is_some_and(|pending| pending.navigation == navigation)
        {
            return false;
        }
        self.presentation.pending_presentations.remove(&id);
        if let Some(queue) = &self.self_queue {
            queue.cancel_presentation(id);
        }
        true
    }

    fn presentation_matches_current_url(&self, id: ItemId, pending: &PendingPresentation) -> bool {
        self.items.tab(id).is_some_and(|tab| {
            tab.has_view()
                && tab
                    .url
                    .as_ref()
                    .is_some_and(|url| url.as_str() == pending.url)
        })
    }

    fn schedule_exact_presentation_retry(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
        reason: &'static str,
    ) {
        let now = std::time::Instant::now();
        let Some(current) = self
            .presentation
            .pending_presentations
            .get_mut(&id)
            .filter(|current| current.navigation == navigation)
        else {
            return;
        };
        current.chrome_request_in_flight = false;
        current.admission_rejections = current.admission_rejections.saturating_add(1);
        if now >= current.hard_deadline
            || current.admission_rejections >= MAX_PRESENTATION_ADMISSION_REJECTIONS
        {
            self.fail_exact_pending_presentation(id, navigation, reason);
            return;
        }
        let retry_index = usize::from(current.admission_rejections.saturating_sub(1))
            .min(PRESENTATION_ADMISSION_RETRY_DELAYS.len() - 1);
        let wake = now
            .checked_add(PRESENTATION_ADMISSION_RETRY_DELAYS[retry_index])
            .unwrap_or(now)
            .min(current.hard_deadline);
        let hard_deadline = current.hard_deadline;
        if let Some(queue) = &self.self_queue {
            queue.retry_presentation(id, navigation, wake, hard_deadline);
        } else {
            self.fail_exact_pending_presentation(
                id,
                navigation,
                "privileged presentation retry queue is unavailable",
            );
        }
    }

    /// Applies the exact tab projection inside privileged chrome and waits for
    /// its eval callback before allowing the raw child to reveal. This method
    /// never blocks the shell actor or a native UI thread.
    fn request_chrome_presentation(&mut self, id: ItemId, navigation: NavigationPresentationId) {
        let Some(pending) = self
            .presentation
            .pending_presentations
            .get(&id)
            .cloned()
            .filter(|pending| pending.navigation == navigation)
        else {
            return;
        };
        if !self.presentation_matches_current_url(id, &pending) {
            self.cancel_exact_pending_presentation(id, navigation);
            return;
        }
        if pending.chrome_applied {
            let _ = self.admit_pending_presentation(id, navigation);
            return;
        }
        if pending.chrome_request_in_flight {
            return;
        }
        if std::time::Instant::now() >= pending.hard_deadline {
            self.fail_exact_pending_presentation(
                id,
                navigation,
                "privileged chrome did not verify the committed URL before its deadline",
            );
            return;
        }

        let Some(tab) = self.items.tab(id) else {
            self.cancel_exact_pending_presentation(id, navigation);
            return;
        };
        let projection = self.presentation_tab_view(
            id,
            tab,
            self.icon_ref(
                zephium_ipc::IconSurface::Chrome,
                tab,
                self.profile_of_item(id),
            ),
        );
        let projection_revision = projection.projection_revision.clone();
        self.record_tab_projection_revision(id, &projection_revision);
        self.remember_tab_view(id, &projection);
        self.publish_icons();
        let active = self.windows.focused().and_then(|window| window.active);
        let Some(queue) = self.self_queue.clone() else {
            let synchronous_projection = projection.clone();
            let dispatch = self.chrome.apply_tab_for_presentation(
                ChromePresentation {
                    settings_visible: self.active_browser_page()
                        == Some(crate::BrowserPage::Settings),
                    id,
                    navigation,
                    url: pending.url.clone(),
                    tab: projection,
                    active,
                },
                Box::new(|_| {}),
            );
            if dispatch == ChromePresentationDispatch::Applied {
                // A synchronous adapter already applied this exact revision;
                // mirror it to non-chrome projection observers afterward.
                // Privileged chrome ignores the equal-revision duplicate.
                (self.emit)(Projection::Tab(synchronous_projection));
                self.on_chrome_presentation_applied(
                    id,
                    navigation,
                    pending.url,
                    active,
                    projection_revision,
                    true,
                );
            } else {
                self.fail_exact_pending_presentation(
                    id,
                    navigation,
                    "asynchronous privileged presentation has no callback ingress",
                );
            }
            return;
        };
        let callback = CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        };
        let callback_url = pending.url.clone();
        let callback_active = active;
        let callback_projection_revision = projection_revision.clone();
        if let Some(current) = self
            .presentation
            .pending_presentations
            .get_mut(&id)
            .filter(|current| current.navigation == navigation)
        {
            current.chrome_request_in_flight = true;
        }
        let synchronous_projection = projection.clone();
        let dispatch = self.chrome.apply_tab_for_presentation(
            ChromePresentation {
                settings_visible: self.active_browser_page() == Some(crate::BrowserPage::Settings),
                id,
                navigation,
                url: pending.url.clone(),
                tab: projection,
                active,
            },
            Box::new(move |applied| {
                let _ = callback.dispatch(Command::ChromePresentationApplied {
                    id,
                    navigation,
                    url: callback_url,
                    active: callback_active,
                    projection_revision: callback_projection_revision,
                    applied,
                });
            }),
        );
        match dispatch {
            ChromePresentationDispatch::Applied => {
                (self.emit)(Projection::Tab(synchronous_projection));
                self.on_chrome_presentation_applied(
                    id,
                    navigation,
                    pending.url,
                    active,
                    projection_revision,
                    true,
                );
            }
            ChromePresentationDispatch::Scheduled => {
                // Callback loss and a full callback queue are both covered by
                // this exact timer. A retry can duplicate an in-flight eval,
                // but stale results cannot pass actor revalidation.
                let retry_index = usize::from(pending.admission_rejections)
                    .min(PRESENTATION_ADMISSION_RETRY_DELAYS.len() - 1);
                let now = std::time::Instant::now();
                let wake = now
                    .checked_add(PRESENTATION_ADMISSION_RETRY_DELAYS[retry_index])
                    .unwrap_or(now)
                    .min(pending.hard_deadline);
                queue.retry_presentation(id, navigation, wake, pending.hard_deadline);
            }
            ChromePresentationDispatch::Rejected => self.schedule_exact_presentation_retry(
                id,
                navigation,
                "privileged chrome presentation admission remained unavailable",
            ),
        }
    }

    pub(super) fn on_chrome_presentation_applied(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
        url: String,
        active: Option<ItemId>,
        projection_revision: String,
        applied: bool,
    ) {
        let pending_exact = self
            .presentation
            .pending_presentations
            .get(&id)
            .is_some_and(|pending| {
                pending.navigation == navigation
                    && pending.url == url
                    && self.presentation_matches_current_url(id, pending)
            });
        if !pending_exact {
            return;
        }
        let revision_exact = self
            .presentation
            .last_tab_projection_revision
            .try_borrow()
            .is_ok_and(|revisions| revisions.get(&id) == Some(&projection_revision));
        let active_exact = self.windows.focused().and_then(|window| window.active) == active;
        if !revision_exact || !active_exact {
            // The original exact timer remains armed. It will clear the
            // in-flight bit and issue a newer projection; this stale callback
            // cannot reveal content under a later masked/full tab state.
            return;
        }
        if !applied {
            self.schedule_exact_presentation_retry(
                id,
                navigation,
                "privileged chrome rejected the exact committed URL projection",
            );
            return;
        }
        if let Some(pending) = self.presentation.pending_presentations.get_mut(&id) {
            pending.chrome_applied = true;
            pending.chrome_request_in_flight = false;
        }
        if let Some(queue) = &self.self_queue {
            queue.cancel_presentation(id);
        }
        let _ = self.admit_pending_presentation(id, navigation);
    }

    /// Retains the exact hidden-document obligation until the native reveal
    /// task has actually entered its owning UI queue. Public dispatcher
    /// overload is recoverable, so a rejection gets one bounded per-tab timer
    /// with capped backoff instead of becoming a permanently hidden page.
    fn admit_pending_presentation(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
    ) -> NativeDispatch {
        let Some(pending) = self
            .presentation
            .pending_presentations
            .get(&id)
            .cloned()
            .filter(|pending| pending.navigation == navigation)
        else {
            return NativeDispatch::Rejected;
        };

        // Actor ordering is not enough: the privileged renderer must have
        // completed and verified the exact revision-bearing tab projection.
        if !pending.chrome_applied || !self.presentation_matches_current_url(id, &pending) {
            self.cancel_exact_pending_presentation(id, navigation);
            return NativeDispatch::Rejected;
        }

        // On a fresh tab the exact eval is also the first projection allowed
        // to remove the real New Tab surface. Queue privileged frame + raw
        // content geometry only afterward, on the same ordered native
        // dispatcher used by presentation. Refusal retains the hidden exact
        // obligation and follows the ordinary bounded retry path.
        if self
            .presentation
            .deferred_first_content_layout
            .contains(&id)
        {
            let layout = self.relayout();
            if layout != NativeDispatch::Scheduled {
                self.schedule_exact_presentation_retry(
                    id,
                    navigation,
                    "first content layout was not admitted after privileged chrome verification",
                );
                return layout;
            }
        }

        let admission = self.engine.present_navigation(id, navigation);
        match admission {
            NativeDispatch::Scheduled => {
                // The native task is generation/epoch checked again when it
                // executes. Clear only the same obligation: a re-entrant newer
                // commit must retain its own hidden-document gate and timer.
                self.presentation
                    .presented_navigations
                    .insert(id, (navigation, pending.url.clone()));
                self.presentation.deferred_first_content_layout.remove(&id);
                self.cancel_exact_pending_presentation(id, navigation);
                self.activate_presented_native_tab(id);
            }
            NativeDispatch::Rejected => {
                self.schedule_exact_presentation_retry(
                    id,
                    navigation,
                    "native presentation dispatcher remained unavailable",
                );
            }
            NativeDispatch::Unsupported => {
                // Emitting Pending/Ready proves this exact native surface is
                // gated. Claiming the matching acknowledgement is unsupported
                // is therefore a lifecycle invariant failure, not permission
                // to forget a potentially permanently hidden document.
                self.fail_exact_pending_presentation(
                    id,
                    navigation,
                    "gated native presentation acknowledgement is unsupported",
                );
            }
        }
        admission
    }

    fn fail_exact_pending_presentation(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
        reason: &'static str,
    ) {
        if !self
            .presentation
            .pending_presentations
            .get(&id)
            .is_some_and(|pending| pending.navigation == navigation)
        {
            return;
        }
        crate::diagnostic!("engine: {reason}; retiring exact hidden view");
        // `close` revokes the engine's item token synchronously before its
        // native cleanup is dispatched. If that dispatch is itself rejected,
        // the production engine seals native authority and invokes its fatal
        // lifecycle callback; either way this id cannot silently keep using
        // the hidden generation after the shell marks it failed.
        let _ = self.engine.close(id);
        self.on_view_creation_failed(id);
    }

    pub(super) fn on_presentation_fallback(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
        hard_deadline: std::time::Instant,
    ) {
        let Some(pending) = self.presentation.pending_presentations.get(&id).cloned() else {
            return;
        };
        if pending.navigation != navigation || pending.hard_deadline != hard_deadline {
            // The timer wake escaped before a newer navigation/ready/close
            // replaced this exact obligation.
            return;
        }
        let Some(_tab) = self.items.tab(id).filter(|tab| tab.has_view()) else {
            self.cancel_pending_presentation(id);
            return;
        };
        let now = std::time::Instant::now();
        if now >= hard_deadline {
            self.fail_exact_pending_presentation(
                id,
                navigation,
                "exact presentation barrier exceeded its hard deadline",
            );
            return;
        }
        if pending.chrome_request_in_flight {
            if let Some(current) = self.presentation.pending_presentations.get_mut(&id) {
                current.chrome_request_in_flight = false;
                current.admission_rejections = current.admission_rejections.saturating_add(1);
                if current.admission_rejections >= MAX_PRESENTATION_ADMISSION_REJECTIONS {
                    self.fail_exact_pending_presentation(
                        id,
                        navigation,
                        "privileged chrome presentation callback remained unavailable",
                    );
                    return;
                }
            }
        }
        // Page loading state is irrelevant. Re-drive the missing chrome or
        // native admission, but never turn the deadline into a timeout reveal.
        self.request_chrome_presentation(id, navigation);
    }

    pub(super) fn on_presentation_fact(
        &mut self,
        id: ItemId,
        navigation: NavigationPresentationId,
        url: String,
    ) {
        let Ok(url) = url::Url::parse(&url) else {
            crate::diagnostic!("engine: rejected malformed native presentation URL");
            return;
        };
        if !navigation::is_browser_target(&url)
            || !self.items.tab(id).is_some_and(|tab| {
                tab.has_view() && tab.url.as_ref().is_some_and(|current| current == &url)
            })
        {
            // A newer URL may already have replaced this coalesced native
            // fact. Do not cancel its obligation and never acknowledge the
            // stale token against whichever URL happens to be current.
            return;
        }
        if !self.presentation.pending_presentations.contains_key(&id) {
            if let Some((presented, presented_url)) =
                self.presentation.presented_navigations.get_mut(&id)
            {
                if *presented == navigation {
                    // A native same-document URL observation retains this exact
                    // already-presented epoch. Update the store-request binding
                    // without hiding/revealing the view or trusting page DOM.
                    *presented_url = url.as_str().to_owned();
                    return;
                }
            }
        }
        if self
            .presentation
            .pending_presentations
            .get(&id)
            .is_some_and(|pending| pending.navigation != navigation)
        {
            // Replace the retained shell obligation and its one timer as one
            // logical transition. An already-escaped old wake remains safe:
            // `on_presentation_fallback` checks both token and hard deadline.
            if let Some(queue) = &self.self_queue {
                queue.cancel_presentation(id);
            }
        }
        let pending = self.track_pending_presentation(
            id,
            navigation,
            url.as_str().to_owned(),
            std::time::Instant::now(),
        );
        self.request_chrome_presentation(id, pending.navigation);
    }
}
