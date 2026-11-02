use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use zephium_core::ids::ItemId;
use zephium_core::navigation;
use zephium_core::ports::engine::EngineEvent;

use crate::navigation_epoch::{NavigationEpoch, NavigationEpochTracker};

use super::dispatch::{with_navigation_commit, with_navigation_settlement};

#[derive(Clone)]
pub(super) struct Sink(crate::EngineEventIngressSink);

impl Sink {
    pub(super) fn new(inner: crate::EngineEventIngressSink) -> Self {
        Self(inner)
    }

    pub(super) fn emit(&self, ev: EngineEvent) {
        (self.0)(crate::EngineEventIngress::global(ev));
    }

    pub(super) fn emit_for(&self, token: Arc<AtomicBool>, ev: EngineEvent) {
        (self.0)(crate::EngineEventIngress::for_item(ev, token));
    }
}

/// Every native WebView owns one permit identity. A normal view is bound at
/// construction; a warm spare may be bound exactly once when it is adopted.
/// Revocation is terminal, so a callback retained by a dropped same-id view
/// can never acquire the token of a replacement view.
#[derive(Clone)]
pub(super) struct EventPermit {
    state: Arc<Mutex<EventPermitState>>,
    #[cfg(target_os = "windows")]
    extensions: Option<ExtensionNavigationGrants>,
}

#[cfg(target_os = "windows")]
pub(super) type ExtensionNavigationGrants = Arc<Mutex<std::collections::HashSet<String>>>;

enum EventPermitState {
    #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
    Inactive,
    Bound(Weak<AtomicBool>),
    #[cfg(any(target_os = "windows", test))]
    DownloadOnly,
    Revoked,
}

impl EventPermit {
    #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
    pub(super) fn inactive() -> Self {
        Self {
            state: Arc::new(Mutex::new(EventPermitState::Inactive)),
            #[cfg(target_os = "windows")]
            extensions: None,
        }
    }

    pub(super) fn bound(token: &Arc<AtomicBool>) -> Self {
        Self {
            state: Arc::new(Mutex::new(EventPermitState::Bound(Arc::downgrade(token)))),
            #[cfg(target_os = "windows")]
            extensions: None,
        }
    }

    #[cfg(target_os = "windows")]
    pub(super) fn with_extensions(mut self, grants: ExtensionNavigationGrants) -> Self {
        self.extensions = Some(grants);
        self
    }

    #[cfg(target_os = "windows")]
    pub(super) fn allows_extension_document_close(&self, target: &str) -> bool {
        self.active_token().is_some()
            && url::Url::parse(target).ok().is_some_and(|url| {
                zephium_core::navigation::extension_document_id(&url).is_some()
                    && self.allows_target(target)
            })
    }

    pub(super) fn allows_target(&self, target: &str) -> bool {
        if navigation::is_allowed_str(target) {
            return true;
        }
        #[cfg(target_os = "windows")]
        if let (Some(grants), Ok(url)) = (&self.extensions, url::Url::parse(target)) {
            return navigation::extension_document_id(&url)
                .is_some_and(|id| grants.lock().is_ok_and(|grants| grants.contains(id)));
        }
        false
    }

    #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
    pub(super) fn bind_once(&self, token: &Arc<AtomicBool>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
            EventPermitState::Inactive => {
                *state = EventPermitState::Bound(Arc::downgrade(token));
                true
            }
            #[cfg(any(target_os = "windows", test))]
            EventPermitState::DownloadOnly => false,
            EventPermitState::Bound(_) | EventPermitState::Revoked => false,
        }
    }

    /// Terminal retirement: only a blank parking navigation remains possible;
    /// no shell events, new downloads or rebinding can regain page authority.
    #[cfg(any(target_os = "windows", test))]
    pub(super) fn retire_for_download(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !matches!(*state, EventPermitState::Bound(_)) {
            return false;
        }
        *state = EventPermitState::DownloadOnly;
        true
    }

    pub(super) fn revoke(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = EventPermitState::Revoked;
    }

    pub(super) fn active_token(&self) -> Option<Arc<AtomicBool>> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*state {
            EventPermitState::Bound(token) => token
                .upgrade()
                .filter(|token| token.load(Ordering::Acquire)),
            #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
            EventPermitState::Inactive => None,
            #[cfg(any(target_os = "windows", test))]
            EventPermitState::DownloadOnly => None,
            EventPermitState::Revoked => None,
        }
    }

    pub(super) fn same_generation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub(super) fn matches_token(&self, token: &Arc<AtomicBool>) -> bool {
        self.active_token()
            .is_some_and(|bound| Arc::ptr_eq(&bound, token))
    }

    pub(super) fn allows_navigation(&self, target: &str) -> bool {
        if !self.allows_target(target) {
            return false;
        }
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match &*state {
            #[cfg(any(not(all(unix, not(target_os = "macos"))), test))]
            EventPermitState::Inactive => target == "about:blank",
            EventPermitState::Bound(token) => token
                .upgrade()
                .is_some_and(|token| token.load(Ordering::Acquire)),
            #[cfg(any(target_os = "windows", test))]
            EventPermitState::DownloadOnly => target == "about:blank",
            EventPermitState::Revoked => false,
        }
    }

    pub(super) fn emit(&self, sink: &Sink, event: EngineEvent) {
        if let Some(token) = self.active_token() {
            sink.emit_for(token, event);
        }
    }
}

pub(super) fn navigation_callback_matches(
    view_permit: &EventPermit,
    view_navigation: &NavigationEpochTracker,
    source_permit: &EventPermit,
    source_navigation: &NavigationEpochTracker,
    epoch: NavigationEpoch,
) -> bool {
    view_permit.same_generation(source_permit)
        && view_navigation.same_generation(source_navigation)
        && view_navigation.is_current(epoch)
}

pub(super) fn queue_navigation_commit(
    id: ItemId,
    permit: &EventPermit,
    navigation: &NavigationEpochTracker,
    epoch: NavigationEpoch,
) {
    if permit.active_token().is_none() {
        return;
    }
    let queued_permit = permit.clone();
    let queued_navigation = navigation.clone();
    let admitted = with_navigation_commit(id, move |host| {
        if host.rearm_navigation_presentation(id, &queued_permit, &queued_navigation, epoch) {
            host.emit_navigation_observation(id, &queued_permit, &queued_navigation, epoch);
            host.apply_popup_policy(id);
        }
    });
    if !admitted {
        // Wry has already hidden the committed native surface before invoking
        // this callback. Reserved globally-coalesced capacity proves ordinary
        // overload cannot reach this branch; shutdown/sealed-host rejection
        // terminally revokes the orphaned callback generation.
        permit.revoke();
        navigation.revoke();
        eprintln!("security: committed-document presentation gate was not admitted");
    }
}

pub(super) fn queue_navigation_completion(
    id: ItemId,
    permit: &EventPermit,
    navigation: &NavigationEpochTracker,
    epoch: NavigationEpoch,
) {
    if permit.active_token().is_none() {
        return;
    }
    let queued_permit = permit.clone();
    let queued_navigation = navigation.clone();
    with_navigation_settlement(id, move |host| {
        if host.rearm_navigation_presentation(id, &queued_permit, &queued_navigation, epoch)
            && host.emit_navigation_observation(id, &queued_permit, &queued_navigation, epoch)
        {
            host.complete_title_attribution(id, &queued_permit, &queued_navigation, epoch);
            host.emit_navigation_ready(id, &queued_permit, &queued_navigation, epoch);
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            if let Some((committed, url)) = queued_navigation.committed_snapshot() {
                if committed == epoch {
                    host.count_site_load(id, &url);
                }
            }
        }
    });
}

pub(super) fn queue_navigation_failure(
    id: ItemId,
    permit: &EventPermit,
    navigation: &NavigationEpochTracker,
    failed: NavigationEpoch,
    restored: Option<NavigationEpoch>,
    ending: super::navigation::FailedNavigationEnd,
) {
    if permit.active_token().is_none() {
        return;
    }
    let queued_permit = permit.clone();
    let queued_navigation = navigation.clone();
    with_navigation_settlement(id, move |host| {
        host.settle_navigation_failure(
            id,
            &queued_permit,
            &queued_navigation,
            failed,
            restored,
            ending,
        );
    });
}

#[cfg(test)]
mod tests;
