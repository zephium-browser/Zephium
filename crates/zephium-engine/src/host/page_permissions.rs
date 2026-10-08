//! macOS page-permission completion ownership.
//!
//! WebKit owns the capability decision; Zephium owns only a bounded,
//! origin-labelled prompt transaction. The retained native block never leaves
//! Wry's main-thread delegate. This module binds its opaque identity to the
//! exact profile, logical item, physical-view permit, and committed navigation
//! epoch, then supplies independent timeout/navigation/Shell terminal paths.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use wry::{
    PermissionRequestDisposition as WryDisposition, PermissionRequestKind as WryRequestKind,
    PermissionResponse as WryResponse, WebViewExtMacOS,
};

use zephium_core::ids::{ItemId, ProfileId};
use zephium_core::permissions::{
    PageOrigin, PagePermissionKind, PagePermissionRequest, PagePermissionRequestId,
    PagePermissionRequestKind, PagePermissionRequestSettlement,
};
use zephium_core::ports::engine::EngineEvent;

use crate::navigation_epoch::{NavigationEpoch, NavigationEpochTracker};

use super::dispatch::{try_with, with_page_permission_terminal};
use super::permits::EventPermit;
use super::EngineHost;

pub(super) use zephium_core::permissions::MAX_PENDING_PAGE_PERMISSION_REQUESTS;
const PAGE_PERMISSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

struct PendingPagePermissionRequest {
    profile: ProfileId,
    item: ItemId,
    request: PagePermissionRequestId,
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    epoch: NavigationEpoch,
    _watchdog: crate::platform::imp::ContentPolicyTimeout,
}

pub(super) struct PagePermissionBroker {
    pending: HashMap<PagePermissionRequestId, PendingPagePermissionRequest>,
    // One process-wide hint avoids a heap allocation in every ObservedView.
    // False makes ordinary navigation allocation- and dispatch-free; a rare
    // prompt may cause unrelated navigation to enqueue an inert exact check.
    presence: Arc<AtomicBool>,
}

impl Default for PagePermissionBroker {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            presence: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl PagePermissionBroker {
    pub(super) fn has_pending_for(&self, item: ItemId) -> bool {
        self.pending.values().any(|pending| pending.item == item)
    }
    pub(super) fn pending_presence(&self) -> Arc<AtomicBool> {
        self.presence.clone()
    }

    fn can_insert(&self, request: PagePermissionRequestId, item: ItemId) -> bool {
        self.pending.len() < MAX_PENDING_PAGE_PERMISSION_REQUESTS
            && !self.pending.contains_key(&request)
            && !self.pending.values().any(|pending| pending.item == item)
    }

    fn insert(&mut self, pending: PendingPagePermissionRequest) -> bool {
        if !self.can_insert(pending.request, pending.item) {
            return false;
        }
        self.pending.insert(pending.request, pending).is_none()
    }

    fn take(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
    ) -> Option<PendingPagePermissionRequest> {
        let pending = self.pending.get(&request)?;
        if pending.profile != profile || pending.item != item {
            return None;
        }
        self.pending.remove(&request)
    }

    fn take_request(
        &mut self,
        request: PagePermissionRequestId,
    ) -> Option<PendingPagePermissionRequest> {
        self.pending.remove(&request)
    }

    fn take_generation(
        &mut self,
        item: ItemId,
        permit: &EventPermit,
    ) -> Vec<PendingPagePermissionRequest> {
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(request, pending)| {
                (pending.item == item && pending.permit.same_generation(permit)).then_some(*request)
            })
            .collect();
        ids.into_iter()
            .filter_map(|request| self.pending.remove(&request))
            .collect()
    }
}

fn core_request(request: &wry::PermissionRequest) -> Option<PagePermissionRequest> {
    let origin = request.origin();
    core_request_from_components(
        request.id().get(),
        origin.scheme(),
        origin.host(),
        origin.port(),
        request.kind(),
    )
}

fn core_request_from_components(
    id: u64,
    scheme: &str,
    host: &str,
    port: Option<u16>,
    kind: WryRequestKind,
) -> Option<PagePermissionRequest> {
    let id = PagePermissionRequestId::new(id)?;
    let origin = PageOrigin::from_native_components(scheme, host, port).ok()?;
    let kind = match kind {
        WryRequestKind::Single(wry::PermissionKind::Camera) => {
            PagePermissionRequestKind::Single(PagePermissionKind::Camera)
        }
        WryRequestKind::Single(wry::PermissionKind::Microphone) => {
            PagePermissionRequestKind::Single(PagePermissionKind::Microphone)
        }
        WryRequestKind::CameraAndMicrophone => PagePermissionRequestKind::CameraAndMicrophone,
        _ => return None,
    };
    Some(PagePermissionRequest { id, origin, kind })
}

pub(super) fn admit_native_request(
    profile: ProfileId,
    item: Rc<std::cell::Cell<ItemId>>,
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    presence: Arc<AtomicBool>,
    request: wry::PermissionRequest,
) -> WryDisposition {
    let Some(request) = core_request(&request) else {
        return WryDisposition::Deny;
    };
    let Some(epoch) = navigation.current_committed() else {
        return WryDisposition::Deny;
    };
    if permit.active_token().is_none() || !navigation.is_current(epoch) {
        return WryDisposition::Deny;
    }
    let item = item.get();
    let had_pending_request = presence.swap(true, Ordering::AcqRel);
    let admitted = try_with(move |host| {
        host.admit_page_permission_request(profile, item, permit, navigation, epoch, request);
    });
    if admitted {
        WryDisposition::Defer
    } else {
        if !had_pending_request {
            presence.store(false, Ordering::Release);
        }
        WryDisposition::Deny
    }
}

pub(super) fn queue_navigation_revocation(
    item: ItemId,
    permit: &EventPermit,
    navigation: &NavigationEpochTracker,
    presence: &Arc<AtomicBool>,
) {
    if permit.active_token().is_none()
        || presence
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return;
    }
    let permit = permit.clone();
    let navigation = navigation.clone();
    let queued_permit = permit.clone();
    let queued_navigation = navigation.clone();
    let admitted = with_page_permission_terminal(move |host| {
        host.revoke_page_permission_requests_for_navigation(
            item,
            &queued_permit,
            &queued_navigation,
        );
    });
    if !admitted {
        // The dedicated terminal channel already seals native ingress and
        // reports a process-fatal invariant. Revoke this view generation too;
        // no future page callback may retain authority behind the fail-stop.
        permit.revoke();
        navigation.revoke();
    }
}

impl EngineHost {
    pub(crate) fn stop_media_capture(
        &mut self,
        item: ItemId,
        navigation: zephium_core::ports::engine::NavigationPresentationId,
    ) {
        let Some(view) = self.views.get(&item) else {
            return;
        };
        if view
            .navigation
            .resident_media_epoch()
            .is_none_or(|epoch| epoch.presentation_id() != navigation)
        {
            return;
        }
        crate::platform::macos::capture::stop(&view.view);
    }
    fn admit_page_permission_request(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        permit: EventPermit,
        navigation: NavigationEpochTracker,
        epoch: NavigationEpoch,
        request: PagePermissionRequest,
    ) {
        let live = self.views.get(&item).is_some_and(|view| {
            crate::platform::imp::permission_owner_is_focused(self.parent.0)
                && view.event_permit.same_generation(&permit)
                && view.navigation.same_generation(&navigation)
                && view.navigation.is_current(epoch)
        }) && self
            .partitions
            .get(&item)
            .is_some_and(|partition| partition.profile() == profile);
        if !live || !self.page_permissions.can_insert(request.id, item) {
            self.resolve_native_page_permission(item, &permit, request.id, WryResponse::Deny);
            self.refresh_page_permission_presence();
            return;
        }

        let request_id = request.id;
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            PAGE_PERMISSION_REQUEST_TIMEOUT,
            move || {
                let _ = with_page_permission_terminal(move |host| {
                    host.timeout_page_permission_request(request_id);
                });
            },
        ) else {
            self.resolve_native_page_permission(item, &permit, request.id, WryResponse::Deny);
            self.refresh_page_permission_presence();
            return;
        };

        let inserted = self.page_permissions.insert(PendingPagePermissionRequest {
            profile,
            item,
            request: request.id,
            permit: permit.clone(),
            navigation,
            epoch,
            _watchdog: watchdog,
        });
        if !inserted {
            self.resolve_native_page_permission(item, &permit, request.id, WryResponse::Deny);
            self.refresh_page_permission_presence();
            return;
        }
        permit.emit(
            &self.sink,
            EngineEvent::PermissionRequested {
                id: item,
                profile,
                request,
            },
        );
    }

    pub(crate) fn settle_page_permission_request(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        settlement: PagePermissionRequestSettlement,
    ) -> bool {
        let Some(pending) = self.page_permissions.take(profile, item, request) else {
            return false;
        };
        let allow = settlement == PagePermissionRequestSettlement::Allow
            && self.views.get(&item).is_some_and(|view| {
                crate::platform::imp::permission_owner_is_focused(self.parent.0)
                    && view.event_permit.same_generation(&pending.permit)
                    && view.navigation.same_generation(&pending.navigation)
                    && view.navigation.is_current(pending.epoch)
            });
        let response = if allow {
            WryResponse::Allow
        } else {
            WryResponse::Deny
        };
        let resolved =
            self.resolve_native_page_permission(item, &pending.permit, request, response);
        self.refresh_page_permission_presence();
        resolved
    }

    fn timeout_page_permission_request(&mut self, request: PagePermissionRequestId) {
        let Some(pending) = self.page_permissions.take_request(request) else {
            return;
        };
        self.resolve_native_page_permission(
            pending.item,
            &pending.permit,
            pending.request,
            WryResponse::Deny,
        );
        self.refresh_page_permission_presence();
    }

    pub(super) fn revoke_page_permission_requests_for_navigation(
        &mut self,
        item: ItemId,
        permit: &EventPermit,
        navigation: &NavigationEpochTracker,
    ) {
        if !self.views.get(&item).is_some_and(|view| {
            view.event_permit.same_generation(permit) && view.navigation.same_generation(navigation)
        }) {
            self.refresh_page_permission_presence();
            return;
        }
        for pending in self.page_permissions.take_generation(item, permit) {
            self.resolve_native_page_permission(item, permit, pending.request, WryResponse::Deny);
            self.refresh_page_permission_presence();
        }
        self.refresh_page_permission_presence();
    }

    pub(super) fn revoke_page_permission_requests_for_close(&mut self, item: ItemId) {
        let Some(permit) = self.views.get(&item).map(|view| view.event_permit.clone()) else {
            return;
        };
        for pending in self.page_permissions.take_generation(item, &permit) {
            self.resolve_native_page_permission(item, &permit, pending.request, WryResponse::Deny);
            self.refresh_page_permission_presence();
        }
        self.refresh_page_permission_presence();
    }

    fn resolve_native_page_permission(
        &self,
        item: ItemId,
        permit: &EventPermit,
        request: PagePermissionRequestId,
        response: WryResponse,
    ) -> bool {
        let Some(request) = wry::PermissionRequestId::from_u64(request.get()) else {
            return false;
        };
        self.views.get(&item).is_some_and(|view| {
            view.event_permit.same_generation(permit)
                && view.view.resolve_permission_request(request, response)
        })
    }

    fn refresh_page_permission_presence(&self) {
        self.page_permissions
            .presence
            .store(!self.page_permissions.pending.is_empty(), Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_request_mapping_accepts_only_supported_structured_media() {
        let camera = core_request_from_components(
            7,
            "https",
            "EXAMPLE.com",
            Some(443),
            WryRequestKind::Single(wry::PermissionKind::Camera),
        )
        .unwrap();
        assert_eq!(camera.id.get(), 7);
        assert_eq!(camera.origin.as_str(), "https://example.com");
        assert_eq!(
            camera.kind,
            PagePermissionRequestKind::Single(PagePermissionKind::Camera)
        );
        assert_eq!(
            core_request_from_components(
                8,
                "http",
                "::1",
                Some(8080),
                WryRequestKind::CameraAndMicrophone,
            )
            .unwrap()
            .origin
            .as_str(),
            "http://[::1]:8080"
        );
        for request in [
            core_request_from_components(
                0,
                "https",
                "example.com",
                None,
                WryRequestKind::Single(wry::PermissionKind::Camera),
            ),
            core_request_from_components(
                9,
                "file",
                "example.com",
                None,
                WryRequestKind::Single(wry::PermissionKind::Camera),
            ),
            core_request_from_components(
                10,
                "https",
                "example.com",
                None,
                WryRequestKind::Single(wry::PermissionKind::Other),
            ),
        ] {
            assert!(request.is_none());
        }
        assert_eq!(MAX_PENDING_PAGE_PERMISSION_REQUESTS, 8);
        assert_eq!(PAGE_PERMISSION_REQUEST_TIMEOUT, Duration::from_secs(30));
    }
}
