//! Main-frame navigation identity and commit attribution.
//!
//! Native engines may overlap navigations and preserve one native identity
//! across any number of HTTP redirects. URLs are policy inputs and observed
//! state, never navigation identities. This state machine binds a Wry-native
//! identity to one non-wrapping Zephium epoch and makes `Committed` the only
//! transition that can authorize rendered content/chrome attribution.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use wry::{NavigationEvent, NavigationEventPhase, NavigationId};

use zephium_core::navigation;
use zephium_core::ports::engine::{NavigationPresentationId, NavigationRequestId};

#[derive(Clone)]
pub(crate) struct NavigationEpochTracker {
    state: Arc<Mutex<NavigationEpochState>>,
}

struct NavigationEpochState {
    current: Option<CurrentNavigation>,
    // Monotonic within this physical tracker and deliberately not restored
    // when a provisional navigation fails. Document-scoped capabilities can
    // therefore prove that no navigation attempt occurred since issuance,
    // even if the same committed epoch becomes visible again.
    activity: Option<NavigationActivity>,
    revoked: bool,
    // Provider redirects may advance epochs. Explicit browser actions and
    // native main-frame link/history/reload navigation relinquish cleanup.
    auth_cleanup_owned: bool,
    auth_cleanup_fence: Option<zephium_core::extensions::AuthTabCleanupPermit>,
}

// Presentation identities cross the shell timer boundary, where an old wake
// may already have escaped cancellation while the same logical ItemId is
// destroyed and recreated. Per-view counters would therefore alias the
// replacement's first navigation. A process-global non-wrapping sequence
// keeps every native generation distinct without retaining tombstones.
static NEXT_NAVIGATION_EPOCH: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
struct CurrentNavigation {
    epoch: NavigationEpoch,
    target: String,
    phase: TrackedNavigationPhase,
    native_id: Option<NavigationId>,
    request: Option<NavigationRequestId>,
    previous_committed: Option<CommittedNavigation>,
}

#[derive(Clone, Debug)]
struct CommittedNavigation {
    epoch: NavigationEpoch,
    target: String,
    native_id: Option<NavigationId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NavigationEpoch(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NavigationActivity(u64);

impl NavigationEpoch {
    pub(crate) const fn presentation_id(self) -> NavigationPresentationId {
        NavigationPresentationId::from_raw(self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrackedNavigationPhase {
    AwaitingStart,
    Started,
    Committed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigationTransition {
    Started(NavigationEpoch),
    Redirected(NavigationEpoch),
    Committed(NavigationEpoch),
    Finished(NavigationEpoch),
    Failed {
        failed: NavigationEpoch,
        restored: Option<NavigationEpoch>,
        request: Option<NavigationRequestId>,
    },
}

impl NavigationEpochTracker {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(NavigationEpochState {
                current: None,
                activity: None,
                revoked: false,
                auth_cleanup_owned: true,
                auth_cleanup_fence: None,
            })),
        }
    }

    pub(crate) fn begin(&self, target: &str) -> Option<NavigationEpoch> {
        self.begin_with_request(target, None)
    }

    pub(crate) fn begin_request(
        &self,
        target: &str,
        request: NavigationRequestId,
    ) -> Option<NavigationEpoch> {
        self.begin_with_request(target, Some(request))
    }

    fn begin_with_request(
        &self,
        target: &str,
        request: Option<NavigationRequestId>,
    ) -> Option<NavigationEpoch> {
        let target = canonical_navigation_target(target)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::begin_locked(&mut state, &target, request)
    }

    fn begin_locked(
        state: &mut NavigationEpochState,
        target: &str,
        request: Option<NavigationRequestId>,
    ) -> Option<NavigationEpoch> {
        if state.revoked {
            return None;
        }
        let previous_committed = state.current.as_ref().and_then(|current| {
            if current.phase == TrackedNavigationPhase::Committed {
                Some(CommittedNavigation {
                    epoch: current.epoch,
                    target: current.target.clone(),
                    native_id: current.native_id,
                })
            } else {
                current.previous_committed.clone()
            }
        });
        let Ok(next) =
            NEXT_NAVIGATION_EPOCH.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
        else {
            // A wrapping epoch could make a callback from the first
            // navigation indistinguishable from the newest one. Permanently
            // retire this tracker instead of panicking in a native callback.
            state.revoked = true;
            if let Some(fence) = &state.auth_cleanup_fence {
                fence.revoke();
            }
            state.current = None;
            return None;
        };
        let epoch = NavigationEpoch(next);
        if request.is_some() {
            state.auth_cleanup_owned = false;
            if let Some(fence) = &state.auth_cleanup_fence {
                fence.revoke();
            }
        }
        state.activity = Some(NavigationActivity(next));
        state.current = Some(CurrentNavigation {
            epoch,
            target: target.to_owned(),
            phase: TrackedNavigationPhase::AwaitingStart,
            native_id: None,
            request,
            previous_committed,
        });
        Some(epoch)
    }

    /// A synchronous native load refusal must not strand the still-rendered
    /// previous document behind an uncommitted epoch. This is generation- and
    /// epoch-exact, so a re-entrant replacement navigation is never rolled
    /// back. Once native content committed, rollback is no longer truthful.
    pub(crate) fn fail_synchronous(&self, epoch: NavigationEpoch) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return false;
        }
        let Some(current) = state.current.as_ref() else {
            return false;
        };
        if current.epoch != epoch || current.phase == TrackedNavigationPhase::Committed {
            return false;
        }
        Self::restore_previous_locked(&mut state);
        true
    }

    /// Apply URL policy without deriving identity from a policy callback.
    /// Native engines can invoke policy hooks for redirects and frames; the
    /// identity-bearing main-frame event sequence is authoritative.
    pub(crate) fn admits_target(&self, target: &str) -> bool {
        if canonical_navigation_target(target).is_none() {
            return false;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.revoked
    }

    pub(crate) fn observe_navigation(
        &self,
        event: &NavigationEvent,
    ) -> Option<NavigationTransition> {
        let target = match event.phase {
            NavigationEventPhase::Started
            | NavigationEventPhase::Redirected
            | NavigationEventPhase::Committed => canonical_navigation_target(&event.url),
            // Terminal phases are correlated by native identity. Their URL
            // can legitimately describe the prior document or native error
            // surface, and is never committed into browser chrome here.
            NavigationEventPhase::Finished
            | NavigationEventPhase::Failed
            | NavigationEventPhase::Cancelled => None,
        };
        if matches!(
            event.phase,
            NavigationEventPhase::Started
                | NavigationEventPhase::Redirected
                | NavigationEventPhase::Committed
        ) && target.is_none()
        {
            return None;
        }

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return None;
        }

        match event.phase {
            NavigationEventPhase::Started => {
                let target = target?;
                let current_phase = state.current.as_ref().map(|current| current.phase);
                match current_phase {
                    None => {
                        let epoch = Self::begin_locked(&mut state, &target, None)?;
                        let current = state.current.as_mut()?;
                        current.native_id = Some(event.id);
                        current.phase = TrackedNavigationPhase::Started;
                        Some(NavigationTransition::Started(epoch))
                    }
                    Some(TrackedNavigationPhase::AwaitingStart) => {
                        let current = state.current.as_mut()?;
                        if current.target != target {
                            return None;
                        }
                        current.native_id = Some(event.id);
                        current.phase = TrackedNavigationPhase::Started;
                        Some(NavigationTransition::Started(current.epoch))
                    }
                    Some(TrackedNavigationPhase::Started) => {
                        let current = state.current.as_ref()?;
                        if current.native_id == Some(event.id) {
                            return (current.target == target)
                                .then_some(NavigationTransition::Started(current.epoch));
                        }
                        let epoch = Self::begin_locked(&mut state, &target, None)?;
                        let current = state.current.as_mut()?;
                        current.native_id = Some(event.id);
                        current.phase = TrackedNavigationPhase::Started;
                        Some(NavigationTransition::Started(epoch))
                    }
                    Some(TrackedNavigationPhase::Committed) => {
                        let epoch = Self::begin_locked(&mut state, &target, None)?;
                        let current = state.current.as_mut()?;
                        current.native_id = Some(event.id);
                        current.phase = TrackedNavigationPhase::Started;
                        Some(NavigationTransition::Started(epoch))
                    }
                }
            }
            NavigationEventPhase::Redirected => {
                let target = target?;
                let current = state.current.as_mut()?;
                if current.phase != TrackedNavigationPhase::Started
                    || current.native_id != Some(event.id)
                {
                    return None;
                }
                current.target = target;
                Some(NavigationTransition::Redirected(current.epoch))
            }
            NavigationEventPhase::Committed => {
                let target = target?;
                let current = state.current.as_mut()?;
                if current.native_id != Some(event.id)
                    || !matches!(
                        current.phase,
                        TrackedNavigationPhase::Started | TrackedNavigationPhase::Committed
                    )
                {
                    return None;
                }
                current.target = target;
                current.phase = TrackedNavigationPhase::Committed;
                current.request = None;
                let epoch = current.epoch;
                Some(NavigationTransition::Committed(epoch))
            }
            NavigationEventPhase::Finished => {
                let current = state.current.as_mut()?;
                if current.native_id != Some(event.id) {
                    return None;
                }
                if current.phase != TrackedNavigationPhase::Committed {
                    let failed = current.epoch;
                    let request = current.request;
                    let restored = Self::restore_previous_locked(&mut state);
                    return Some(NavigationTransition::Failed {
                        failed,
                        restored,
                        request,
                    });
                }
                current.previous_committed = None;
                Some(NavigationTransition::Finished(current.epoch))
            }
            NavigationEventPhase::Failed | NavigationEventPhase::Cancelled => {
                let current = state.current.as_mut()?;
                if current.native_id != Some(event.id) {
                    return None;
                }
                let failed = current.epoch;
                let request = current.request;
                if current.phase == TrackedNavigationPhase::Committed {
                    current.previous_committed = None;
                    return Some(NavigationTransition::Failed {
                        failed,
                        restored: Some(failed),
                        request: None,
                    });
                }
                let restored = Self::restore_previous_locked(&mut state);
                Some(NavigationTransition::Failed {
                    failed,
                    restored,
                    request,
                })
            }
        }
    }

    fn restore_previous_locked(state: &mut NavigationEpochState) -> Option<NavigationEpoch> {
        let previous = state
            .current
            .take()
            .and_then(|current| current.previous_committed);
        let restored = previous.as_ref().map(|previous| previous.epoch);
        state.current = previous.map(|previous| CurrentNavigation {
            epoch: previous.epoch,
            target: previous.target,
            phase: TrackedNavigationPhase::Committed,
            native_id: previous.native_id,
            request: None,
            previous_committed: None,
        });
        restored
    }

    pub(crate) fn current(&self) -> Option<NavigationEpoch> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (!state.revoked)
            .then(|| state.current.as_ref().map(|current| current.epoch))
            .flatten()
    }

    pub(crate) fn current_committed(&self) -> Option<NavigationEpoch> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (!state.revoked)
            .then(|| {
                state.current.as_ref().and_then(|current| {
                    (current.phase == TrackedNavigationPhase::Committed).then_some(current.epoch)
                })
            })
            .flatten()
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn auth_cleanup_is_owned(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.revoked && state.auth_cleanup_owned
    }

    pub(crate) fn release_auth_cleanup(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.auth_cleanup_owned = false;
        if let Some(fence) = &state.auth_cleanup_fence {
            fence.revoke();
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn attach_auth_cleanup_fence(
        &self,
        fence: zephium_core::extensions::AuthTabCleanupPermit,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked || !state.auth_cleanup_owned {
            fence.revoke();
        }
        if let Some(previous) = state.auth_cleanup_fence.replace(fence.clone()) {
            if previous != fence {
                previous.revoke();
            }
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn resident_document_snapshot(&self) -> Option<(NavigationEpoch, String)> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return None;
        }
        let current = state.current.as_ref()?;
        if current.phase == TrackedNavigationPhase::Committed {
            Some((current.epoch, current.target.clone()))
        } else {
            current
                .previous_committed
                .as_ref()
                .map(|previous| (previous.epoch, previous.target.clone()))
        }
    }

    /// Native media state still belongs to the previous resident document
    /// during a provisional load. This observation/stop identity grants no
    /// permission, presentation, scripting or navigation authority.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn resident_media_epoch(&self) -> Option<NavigationEpoch> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return None;
        }
        let current = state.current.as_ref()?;
        if current.phase == TrackedNavigationPhase::Committed {
            Some(current.epoch)
        } else {
            current
                .previous_committed
                .as_ref()
                .map(|previous| previous.epoch)
        }
    }

    pub(crate) fn committed_snapshot(&self) -> Option<(NavigationEpoch, String)> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return None;
        }
        let current = state.current.as_ref()?;
        (current.phase == TrackedNavigationPhase::Committed)
            .then(|| (current.epoch, current.target.clone()))
    }

    pub(crate) fn matches_committed_snapshot(&self, epoch: NavigationEpoch, target: &str) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.revoked
            && state.current.as_ref().is_some_and(|current| {
                current.phase == TrackedNavigationPhase::Committed
                    && current.epoch == epoch
                    && current.target == target
            })
    }

    /// Captures the newest attempted main-frame navigation in this physical
    /// view generation. Unlike `current`, this never rolls back to a restored
    /// document after provisional failure.
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn activity_snapshot(&self) -> Option<NavigationActivity> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (!state.revoked).then_some(state.activity).flatten()
    }

    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    pub(crate) fn matches_activity(&self, activity: NavigationActivity) -> bool {
        self.activity_snapshot() == Some(activity)
    }

    pub(crate) fn is_current(&self, epoch: NavigationEpoch) -> bool {
        self.current() == Some(epoch)
    }

    pub(crate) fn committed_epoch_for_presentation(
        &self,
        presentation: NavigationPresentationId,
    ) -> Option<NavigationEpoch> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return None;
        }
        state.current.as_ref().and_then(|current| {
            (current.phase == TrackedNavigationPhase::Committed
                && current.epoch.presentation_id() == presentation)
                .then_some(current.epoch)
        })
    }

    pub(crate) fn same_generation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    /// Validate a source queried from the native view for a queued source or
    /// history callback. Only an identity-matched native `Committed` event can
    /// authorize a new document; subsequent same-document History API URLs
    /// remain in that epoch.
    pub(crate) fn observe_source(&self, epoch: NavigationEpoch, source: &str) -> bool {
        let Some(source) = canonical_navigation_target(source) else {
            return false;
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.revoked {
            return false;
        }
        let Some(current) = state.current.as_mut() else {
            return false;
        };
        if current.epoch != epoch || current.phase != TrackedNavigationPhase::Committed {
            return false;
        }
        // This observer carries no native navigation identity. It is
        // authority only for same-document History API/hash changes, which
        // cannot change origin. Cross-origin documents must arrive through
        // an identity-bearing Committed event; otherwise KVO/notify ordering
        // could rebind the still-visible committed epoch.
        if !same_document_origin(&current.target, &source) {
            return false;
        }
        current.target = source;
        true
    }

    pub(crate) fn revoke(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.revoked = true;
        if let Some(fence) = &state.auth_cleanup_fence {
            fence.revoke();
        }
        state.current = None;
    }
}

fn canonical_navigation_target(target: &str) -> Option<String> {
    if !navigation::is_browser_target_str(target) {
        return None;
    }
    url::Url::parse(target).ok().map(|url| url.to_string())
}

fn same_document_origin(current: &str, observed: &str) -> bool {
    let (Ok(current), Ok(observed)) = (url::Url::parse(current), url::Url::parse(observed)) else {
        return false;
    };
    if current.as_str() == "about:blank" || observed.as_str() == "about:blank" {
        return current.as_str() == observed.as_str();
    }
    current.scheme() == observed.scheme()
        && current.host() == observed.host()
        && current.port_or_known_default() == observed.port_or_known_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_provider_redirects_keep_auth_cleanup_but_user_navigation_releases_it() {
        let tracker = NavigationEpochTracker::new();
        commit(
            &tracker,
            91,
            "https://provider.example/login",
            "https://provider.example/login",
        );
        assert!(tracker.auth_cleanup_is_owned());
        commit(
            &tracker,
            92,
            "https://sso.example/challenge",
            "https://sso.example/challenge",
        );
        assert!(tracker.auth_cleanup_is_owned());
        let user = tracker
            .begin_request("https://provider.example/article", NavigationRequestId(71))
            .unwrap();
        assert!(!tracker.auth_cleanup_is_owned());
        tracker.fail_synchronous(user);
        assert!(!tracker.auth_cleanup_is_owned());
        commit(
            &tracker,
            93,
            "https://provider.example/login",
            "https://provider.example/login",
        );
        assert!(!tracker.auth_cleanup_is_owned());
    }

    #[test]
    fn history_or_reload_intent_releases_auth_cleanup_even_before_native_outcome() {
        let tracker = NavigationEpochTracker::new();
        commit(
            &tracker,
            94,
            "https://provider.example/login",
            "https://provider.example/login",
        );
        tracker.release_auth_cleanup();
        assert!(!tracker.auth_cleanup_is_owned());
    }

    #[test]
    fn media_observation_keeps_the_resident_document_during_failed_provisional_load() {
        let tracker = NavigationEpochTracker::new();
        let resident = commit(
            &tracker,
            81,
            "https://call.example/",
            "https://call.example/",
        );
        tracker.begin("https://next.example/").unwrap();
        tracker.observe_navigation(&event(
            82,
            NavigationEventPhase::Started,
            "https://next.example/",
        ));
        assert!(tracker.current_committed().is_none());
        assert_eq!(tracker.resident_media_epoch(), Some(resident));
        tracker.observe_navigation(&event(
            82,
            NavigationEventPhase::Failed,
            "https://next.example/",
        ));
        assert_eq!(tracker.resident_media_epoch(), Some(resident));
        let replacement = commit(
            &tracker,
            83,
            "https://next.example/",
            "https://next.example/",
        );
        assert_eq!(tracker.resident_media_epoch(), Some(replacement));
        assert_ne!(replacement, resident);
        tracker.revoke();
        assert!(tracker.resident_media_epoch().is_none());
    }

    #[test]
    fn policy_cancellation_restores_previous_document_without_committing_download_url() {
        let tracker = NavigationEpochTracker::new();
        let visible = commit(
            &tracker,
            1,
            "https://fixture.test/",
            "https://fixture.test/",
        );
        let pending = tracker.begin("https://fixture.test/download").unwrap();
        tracker.observe_navigation(&event(
            2,
            NavigationEventPhase::Started,
            "https://fixture.test/download",
        ));
        assert_eq!(
            tracker.observe_navigation(&event(
                2,
                NavigationEventPhase::Cancelled,
                "https://fixture.test/download"
            )),
            Some(NavigationTransition::Failed {
                failed: pending,
                restored: Some(visible),
                request: None
            })
        );
        assert_eq!(
            tracker.committed_snapshot(),
            Some((visible, "https://fixture.test/".into()))
        );
    }

    fn event(id: u64, phase: NavigationEventPhase, url: &str) -> NavigationEvent {
        NavigationEvent {
            id: NavigationId::from_raw(id),
            phase,
            url: url.to_owned(),
        }
    }

    fn commit(
        tracker: &NavigationEpochTracker,
        id: u64,
        requested: &str,
        committed: &str,
    ) -> NavigationEpoch {
        let epoch = tracker.begin(requested).unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(id, NavigationEventPhase::Started, requested)),
            Some(NavigationTransition::Started(epoch))
        );
        assert_eq!(
            tracker.observe_navigation(&event(id, NavigationEventPhase::Committed, committed)),
            Some(NavigationTransition::Committed(epoch))
        );
        epoch
    }

    #[test]
    fn presentation_identity_never_aliases_across_recreated_view_trackers() {
        let old_generation = NavigationEpochTracker::new()
            .begin("https://old.test/")
            .unwrap()
            .presentation_id();
        let replacement = NavigationEpochTracker::new()
            .begin("https://replacement.test/")
            .unwrap()
            .presentation_id();

        assert_ne!(old_generation, replacement);
    }

    #[test]
    fn same_origin_redirect_commits_final_url_under_one_native_identity() {
        let tracker = NavigationEpochTracker::new();
        let epoch = tracker.begin("https://example.test/start").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                7,
                NavigationEventPhase::Started,
                "https://example.test/start"
            )),
            Some(NavigationTransition::Started(epoch))
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                7,
                NavigationEventPhase::Redirected,
                "https://example.test/final"
            )),
            Some(NavigationTransition::Redirected(epoch))
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                7,
                NavigationEventPhase::Committed,
                "https://example.test/final"
            )),
            Some(NavigationTransition::Committed(epoch))
        );
        assert_eq!(
            tracker.committed_snapshot(),
            Some((epoch, "https://example.test/final".into()))
        );
    }

    #[test]
    fn cross_origin_multihop_redirect_commits_only_the_final_allowed_target() {
        let tracker = NavigationEpochTracker::new();
        let epoch = tracker.begin("https://one.test/").unwrap();
        assert!(matches!(
            tracker.observe_navigation(&event(
                11,
                NavigationEventPhase::Started,
                "https://one.test/"
            )),
            Some(NavigationTransition::Started(observed)) if observed == epoch
        ));
        for target in ["https://two.test/hop", "https://three.test/final"] {
            assert_eq!(
                tracker.observe_navigation(&event(11, NavigationEventPhase::Redirected, target)),
                Some(NavigationTransition::Redirected(epoch))
            );
        }
        assert_eq!(
            tracker.observe_navigation(&event(
                11,
                NavigationEventPhase::Committed,
                "https://three.test/final"
            )),
            Some(NavigationTransition::Committed(epoch))
        );
        assert_eq!(
            tracker.committed_snapshot(),
            Some((epoch, "https://three.test/final".into()))
        );
    }

    #[test]
    fn provisional_failure_restores_the_still_visible_committed_document() {
        let tracker = NavigationEpochTracker::new();
        let visible = commit(
            &tracker,
            1,
            "https://visible.test/",
            "https://visible.test/",
        );
        let failing = tracker.begin("https://failing.test/").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                2,
                NavigationEventPhase::Started,
                "https://failing.test/"
            )),
            Some(NavigationTransition::Started(failing))
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                2,
                NavigationEventPhase::Failed,
                "https://failing.test/"
            )),
            Some(NavigationTransition::Failed {
                failed: failing,
                restored: Some(visible),
                request: None,
            })
        );
        assert_eq!(
            tracker.committed_snapshot(),
            Some((visible, "https://visible.test/".into()))
        );
    }

    #[test]
    fn overlapping_navigation_rejects_late_commit_and_completion() {
        let tracker = NavigationEpochTracker::new();
        let first = tracker.begin("https://first.test/").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                41,
                NavigationEventPhase::Started,
                "https://first.test/"
            )),
            Some(NavigationTransition::Started(first))
        );
        let second = tracker.begin("https://second.test/").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                42,
                NavigationEventPhase::Started,
                "https://second.test/"
            )),
            Some(NavigationTransition::Started(second))
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                41,
                NavigationEventPhase::Committed,
                "https://first.test/"
            )),
            None
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                41,
                NavigationEventPhase::Finished,
                "https://first.test/"
            )),
            None
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                42,
                NavigationEventPhase::Committed,
                "https://second.test/"
            )),
            Some(NavigationTransition::Committed(second))
        );
    }

    #[test]
    fn adopted_warm_spare_rejects_late_bootstrap_identity() {
        let tracker = NavigationEpochTracker::new();
        let bootstrap = tracker.begin("about:blank").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(91, NavigationEventPhase::Started, "about:blank")),
            Some(NavigationTransition::Started(bootstrap))
        );
        let adopted = tracker.begin("https://adopted.test/").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(91, NavigationEventPhase::Committed, "about:blank")),
            None
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                92,
                NavigationEventPhase::Started,
                "https://adopted.test/"
            )),
            Some(NavigationTransition::Started(adopted))
        );
    }

    #[test]
    fn synchronous_load_failure_rolls_back_only_the_exact_uncommitted_epoch() {
        let tracker = NavigationEpochTracker::new();
        let visible = commit(
            &tracker,
            1,
            "https://visible.test/",
            "https://visible.test/",
        );
        let failing = tracker.begin("https://failing.test/").unwrap();
        assert!(tracker.fail_synchronous(failing));
        assert_eq!(tracker.current_committed(), Some(visible));
        assert!(!tracker.fail_synchronous(failing));

        let replacement = tracker.begin("https://replacement.test/").unwrap();
        assert!(!tracker.fail_synchronous(failing));
        assert!(tracker.is_current(replacement));
    }

    #[test]
    fn navigation_activity_never_rolls_back_with_a_restored_document() {
        let tracker = NavigationEpochTracker::new();
        let visible = commit(
            &tracker,
            91,
            "https://example.test/visible",
            "https://example.test/visible",
        );
        let visible_activity = tracker.activity_snapshot().unwrap();

        let provisional = tracker.begin("https://example.test/provisional").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                92,
                NavigationEventPhase::Started,
                "https://example.test/provisional",
            )),
            Some(NavigationTransition::Started(provisional))
        );
        let provisional_activity = tracker.activity_snapshot().unwrap();
        assert_ne!(visible_activity, provisional_activity);
        assert_eq!(
            tracker.observe_navigation(&event(
                92,
                NavigationEventPhase::Failed,
                "https://example.test/provisional",
            )),
            Some(NavigationTransition::Failed {
                failed: provisional,
                restored: Some(visible),
                request: None,
            })
        );

        assert_eq!(tracker.current_committed(), Some(visible));
        assert!(!tracker.matches_activity(visible_activity));
        assert!(tracker.matches_activity(provisional_activity));
    }

    #[test]
    fn asynchronous_failure_returns_only_its_explicit_request_identity() {
        let tracker = NavigationEpochTracker::new();
        let request = NavigationRequestId(77);
        let epoch = tracker
            .begin_request("https://failing.test/", request)
            .unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                9,
                NavigationEventPhase::Started,
                "https://failing.test/"
            )),
            Some(NavigationTransition::Started(epoch))
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                9,
                NavigationEventPhase::Failed,
                "https://failing.test/"
            )),
            Some(NavigationTransition::Failed {
                failed: epoch,
                restored: None,
                request: Some(request),
            })
        );
    }

    #[test]
    fn disallowed_redirect_never_changes_the_tracked_target() {
        let tracker = NavigationEpochTracker::new();
        let epoch = tracker.begin("https://safe.test/").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                5,
                NavigationEventPhase::Started,
                "https://safe.test/"
            )),
            Some(NavigationTransition::Started(epoch))
        );
        assert_eq!(
            tracker.observe_navigation(&event(
                5,
                NavigationEventPhase::Redirected,
                "file:///etc/passwd"
            )),
            None
        );
        assert_eq!(tracker.committed_snapshot(), None);
    }

    #[test]
    fn source_observation_cannot_manufacture_a_commit() {
        let tracker = NavigationEpochTracker::new();
        let epoch = tracker.begin("https://example.test/").unwrap();
        assert_eq!(
            tracker.observe_navigation(&event(
                8,
                NavigationEventPhase::Started,
                "https://example.test/"
            )),
            Some(NavigationTransition::Started(epoch))
        );
        assert!(!tracker.observe_source(epoch, "https://example.test/"));
        assert_eq!(tracker.current_committed(), None);
    }

    #[test]
    fn identity_free_source_observation_accepts_only_same_document_origin() {
        let tracker = NavigationEpochTracker::new();
        let epoch = commit(
            &tracker,
            12,
            "https://example.test/start",
            "https://example.test/start",
        );

        assert!(tracker.observe_source(epoch, "https://example.test/path?q=1#fragment"));
        assert_eq!(
            tracker.committed_snapshot(),
            Some((epoch, "https://example.test/path?q=1#fragment".into()))
        );

        for forged in [
            "https://other.test/",
            "http://example.test/",
            "https://example.test:444/",
            "about:blank",
        ] {
            assert!(!tracker.observe_source(epoch, forged), "accepted {forged}");
            assert_eq!(
                tracker.committed_snapshot(),
                Some((epoch, "https://example.test/path?q=1#fragment".into()))
            );
        }
    }

    #[test]
    fn about_blank_source_observation_is_exact() {
        let tracker = NavigationEpochTracker::new();
        let epoch = commit(&tracker, 13, "about:blank", "about:blank");
        assert!(tracker.observe_source(epoch, "about:blank"));
        assert!(!tracker.observe_source(epoch, "https://example.test/"));
    }
}
