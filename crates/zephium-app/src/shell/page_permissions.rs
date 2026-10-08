//! On-demand, browser-owned page-permission consent coordination.
//!
//! Native WebKit completions remain in the engine. This actor retains only
//! their opaque identity, a bounded authority tuple, and at most two durable
//! catalog rows. The catalog is never loaded at startup. A remembered Allow
//! reaches native code only after Store has reported and this coordinator has
//! re-observed the exact durable decision.

use zephium_core::ids::PagePermissionGrantId;
use zephium_core::permissions::{
    PagePermissionCatalog, PagePermissionCatalogRevision, PagePermissionChange,
    PagePermissionGrant, PagePermissionKind, PagePermissionPatch, PagePermissionRequest,
    PagePermissionRequestId, PagePermissionRequestKind, PagePermissionRequestSettlement,
    RememberedPagePermission,
};
use zephium_core::ports::store::{
    PagePermissionCatalogLoadOutcome, PagePermissionCatalogMutationOutcome,
};

use super::*;

const PAGE_PERMISSION_SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(25);
const PAGE_PERMISSION_PROMPTS_ENABLED: bool = cfg!(any(
    test,
    all(
        target_os = "macos",
        feature = "macos-page-permission-prompts"
    )
));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReconciliationSource {
    Applied,
    Conflict,
    OutcomeUnknown,
}

#[derive(Clone, Debug)]
struct RetainedAuthority {
    kind: PagePermissionKind,
    grant: Option<PagePermissionGrant>,
}

#[derive(Clone, Debug)]
enum PagePermissionPhase {
    Loading,
    Prompting {
        expected: Option<PagePermissionCatalogRevision>,
        authorities: Vec<RetainedAuthority>,
    },
    Mutating {
        operation_id: String,
        desired: RememberedPagePermission,
    },
    Reconciling {
        operation_id: String,
        desired: RememberedPagePermission,
        source: ReconciliationSource,
    },
}

#[derive(Clone, Debug)]
struct PendingPagePermission {
    profile: ProfileId,
    item: ItemId,
    request: PagePermissionRequest,
    rememberable: bool,
    deadline: std::time::Instant,
    phase: PagePermissionPhase,
}

struct PagePermissionOperationCompletion {
    operation_id: String,
    outcome: OperationOutcome,
    reason: OperationReason,
}

#[derive(Default)]
pub(super) struct PagePermissionPromptState {
    pending: Option<PendingPagePermission>,
    failed_until_restart: bool,
    /// Remembered grants stay off until the browser can list and revoke
    /// them; until then every request is a one-time decision.
    pub(super) remember_enabled: bool,
}

impl PagePermissionPromptState {
    pub(super) fn visible(&self) -> Option<(ProfileId, ItemId, &PagePermissionRequest, bool)> {
        let pending = self.pending.as_ref()?;
        match pending.phase {
            PagePermissionPhase::Loading => None,
            PagePermissionPhase::Prompting { .. } => {
                Some((pending.profile, pending.item, &pending.request, false))
            }
            PagePermissionPhase::Mutating { .. } | PagePermissionPhase::Reconciling { .. } => {
                Some((pending.profile, pending.item, &pending.request, true))
            }
        }
    }

    pub(super) fn visible_rememberable(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.rememberable && self.is_visible())
    }

    pub(super) fn is_visible(&self) -> bool {
        self.visible().is_some()
    }

    #[cfg(test)]
    pub(super) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    #[cfg(test)]
    pub(super) fn failed_until_restart(&self) -> bool {
        self.failed_until_restart
    }

    fn exact(&self, profile: ProfileId, item: ItemId, request: PagePermissionRequestId) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.profile == profile && pending.item == item && pending.request.id == request
        })
    }

    fn take_exact(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
    ) -> Option<PendingPagePermission> {
        if !self.exact(profile, item, request) {
            return None;
        }
        self.pending.take()
    }
}

fn request_kinds(kind: PagePermissionRequestKind) -> &'static [PagePermissionKind] {
    const CAMERA: &[PagePermissionKind] = &[PagePermissionKind::Camera];
    const MICROPHONE: &[PagePermissionKind] = &[PagePermissionKind::Microphone];
    const MEDIA: &[PagePermissionKind] =
        &[PagePermissionKind::Camera, PagePermissionKind::Microphone];
    match kind {
        PagePermissionRequestKind::Single(PagePermissionKind::Camera) => CAMERA,
        PagePermissionRequestKind::Single(PagePermissionKind::Microphone) => MICROPHONE,
        PagePermissionRequestKind::CameraAndMicrophone => MEDIA,
        PagePermissionRequestKind::Single(_) => &[],
    }
}

fn retain_authorities(
    catalog: &PagePermissionCatalog,
    request: &PagePermissionRequest,
) -> Vec<RetainedAuthority> {
    request_kinds(request.kind)
        .iter()
        .map(|kind| RetainedAuthority {
            kind: *kind,
            grant: catalog
                .grants()
                .iter()
                .find(|grant| grant.origin == request.origin && grant.kind == *kind)
                .cloned(),
        })
        .collect()
}

fn empty_authorities(request: &PagePermissionRequest) -> Vec<RetainedAuthority> {
    request_kinds(request.kind)
        .iter()
        .map(|kind| RetainedAuthority {
            kind: *kind,
            grant: None,
        })
        .collect()
}

fn remembered_decision(authorities: &[RetainedAuthority]) -> Option<RememberedPagePermission> {
    // A remembered block dominates an atomic combined request even if the
    // other capability has never been decided. Prompting in that state would
    // let an Allow Once gesture bypass durable denial.
    if authorities.iter().any(|entry| {
        entry
            .grant
            .as_ref()
            .is_some_and(|grant| grant.decision == RememberedPagePermission::Deny)
    }) {
        return Some(RememberedPagePermission::Deny);
    }
    if authorities.is_empty() || authorities.iter().any(|entry| entry.grant.is_none()) {
        return None;
    }
    authorities
        .iter()
        .all(|entry| {
            entry
                .grant
                .as_ref()
                .is_some_and(|grant| grant.decision == RememberedPagePermission::Allow)
        })
        .then_some(RememberedPagePermission::Allow)
}

fn durable_decision_matches(
    authorities: &[RetainedAuthority],
    desired: RememberedPagePermission,
) -> bool {
    !authorities.is_empty()
        && authorities.iter().all(|entry| {
            entry
                .grant
                .as_ref()
                .is_some_and(|grant| grant.decision == desired)
        })
}

fn patch_for_decision(
    request: &PagePermissionRequest,
    authorities: &[RetainedAuthority],
    desired: RememberedPagePermission,
) -> Option<PagePermissionPatch> {
    let changes: Vec<_> = authorities
        .iter()
        .filter_map(|entry| match &entry.grant {
            Some(grant) if grant.decision == desired => None,
            Some(grant) => Some(PagePermissionChange::Update {
                id: grant.id,
                expected: grant.revision,
                decision: desired,
            }),
            None => Some(PagePermissionChange::Create {
                id: PagePermissionGrantId::generate(),
                origin: request.origin.clone(),
                kind: entry.kind,
                decision: desired,
            }),
        })
        .collect();
    PagePermissionPatch::new(changes).ok()
}

impl Shell {
    fn page_permission_request_is_foreground(&self, profile: ProfileId, item: ItemId) -> bool {
        self.window_visible
            && self.window_focused
            && self.windows.focused().is_some_and(|window| {
                window.profile == profile
                    && (window.active == Some(item) || self.work_pane_shows(item))
            })
            && self.items.tab(item).is_some_and(TabState::has_view)
    }

    pub(super) fn on_page_permission_request(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequest,
    ) {
        let profile_kind = self.profiles.get(profile).map(|profile| profile.kind);
        let rememberable = self.page_permissions.remember_enabled
            && matches!(
                profile_kind,
                Some(ProfileKind::Default | ProfileKind::Named)
            );
        if !self.bootstrapped
            || !PAGE_PERMISSION_PROMPTS_ENABLED
            || self.page_permissions.failed_until_restart
            || self.page_permissions.pending.is_some()
            || profile_kind.is_none()
            || (rememberable && self.degraded_storage_profiles.contains(&profile))
            || request_kinds(request.kind).is_empty()
            || !self.page_permission_request_is_foreground(profile, item)
        {
            let _ = self.settle_native_page_permission(
                profile,
                item,
                request.id,
                PagePermissionRequestSettlement::Deny,
            );
            return;
        }

        let now = std::time::Instant::now();
        let deadline = now
            .checked_add(PAGE_PERMISSION_SHELL_TIMEOUT)
            .unwrap_or(now);
        let request_id = request.id;
        let phase = if rememberable {
            PagePermissionPhase::Loading
        } else {
            PagePermissionPhase::Prompting {
                expected: None,
                authorities: empty_authorities(&request),
            }
        };
        self.page_permissions.pending = Some(PendingPagePermission {
            profile,
            item,
            request,
            rememberable,
            deadline,
            phase,
        });
        if let Some(queue) = &self.self_queue {
            queue.schedule_page_permission(profile, item, request_id, deadline);
        }
        if !rememberable {
            if self.relayout() != NativeDispatch::Scheduled {
                self.fail_page_permission_request(
                    profile,
                    item,
                    request_id,
                    None,
                    OperationReason::NativeDispatchRejected,
                    false,
                );
                return;
            }
            self.project_page_permission_prompt();
            return;
        }
        if !self.load_page_permission_catalog(profile, item) {
            self.fail_page_permission_request(
                profile,
                item,
                request_id,
                None,
                OperationReason::StoreAdmissionRejected,
                false,
            );
        }
    }

    fn load_page_permission_catalog(&self, profile: ProfileId, item: ItemId) -> bool {
        let Some(pending) = self.page_permissions.pending.as_ref() else {
            return false;
        };
        let request = pending.request.id;
        let Some(queue) = self.self_queue.as_ref() else {
            return false;
        };
        let callback = CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.load_page_permission_catalog(
                profile,
                Box::new(move |outcome| {
                    let _ = callback.dispatch(Command::PagePermissionCatalogLoaded {
                        profile,
                        item,
                        request,
                        outcome: Box::new(outcome),
                    });
                }),
            )
        }))
        .unwrap_or(false)
    }

    pub(super) fn settle_page_permission_catalog_load(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        outcome: PagePermissionCatalogLoadOutcome,
    ) {
        if !self.page_permissions.exact(profile, item, request) {
            return;
        }
        if self
            .page_permissions
            .pending
            .as_ref()
            .is_some_and(|pending| std::time::Instant::now() >= pending.deadline)
        {
            self.on_page_permission_timeout(profile, item, request);
            return;
        }
        if !self.page_permission_request_is_foreground(profile, item) {
            self.cancel_page_permission_request(profile, item, request);
            return;
        }
        let Some(pending) = self.page_permissions.pending.as_ref() else {
            return;
        };
        let phase = pending.phase.clone();
        let request_payload = pending.request.clone();
        let catalog = match outcome {
            PagePermissionCatalogLoadOutcome::Loaded(catalog) => catalog,
            PagePermissionCatalogLoadOutcome::NotRegistered
            | PagePermissionCatalogLoadOutcome::DegradedProfile => {
                let operation_id = match phase {
                    PagePermissionPhase::Mutating { operation_id, .. }
                    | PagePermissionPhase::Reconciling { operation_id, .. } => Some(operation_id),
                    PagePermissionPhase::Loading | PagePermissionPhase::Prompting { .. } => None,
                };
                self.fail_page_permission_request(
                    profile,
                    item,
                    request,
                    operation_id,
                    OperationReason::StoreReconciliationFailed,
                    true,
                );
                return;
            }
            PagePermissionCatalogLoadOutcome::Failed => {
                let operation_id = match phase {
                    PagePermissionPhase::Mutating { operation_id, .. }
                    | PagePermissionPhase::Reconciling { operation_id, .. } => Some(operation_id),
                    PagePermissionPhase::Loading | PagePermissionPhase::Prompting { .. } => None,
                };
                self.fail_page_permission_request(
                    profile,
                    item,
                    request,
                    operation_id,
                    OperationReason::StoreAdmissionRejected,
                    false,
                );
                return;
            }
        };
        let authorities = retain_authorities(&catalog, &request_payload);

        match phase {
            PagePermissionPhase::Loading => {
                if let Some(decision) = remembered_decision(&authorities) {
                    let _ = self
                        .complete_page_permission_request(profile, item, request, decision, None);
                    return;
                }
                if let Some(pending) = self.page_permissions.pending.as_mut() {
                    pending.phase = PagePermissionPhase::Prompting {
                        expected: Some(catalog.revision()),
                        authorities,
                    };
                }
                if self.relayout() != NativeDispatch::Scheduled {
                    self.fail_page_permission_request(
                        profile,
                        item,
                        request,
                        None,
                        OperationReason::NativeDispatchRejected,
                        false,
                    );
                    return;
                }
                self.project_page_permission_prompt();
            }
            PagePermissionPhase::Reconciling {
                operation_id,
                desired,
                source,
            } => {
                // Reconciliation proves the complete requested authority
                // tuple, not merely the effective native outcome. A single
                // durable Deny is enough to reject a combined request during
                // admission, but it is not truthful evidence that an
                // `Always deny` mutation durably covered every capability.
                if durable_decision_matches(&authorities, desired) {
                    let (outcome, reason) = match source {
                        ReconciliationSource::Conflict => {
                            (OperationOutcome::NoOp, OperationReason::StateUnchanged)
                        }
                        ReconciliationSource::Applied | ReconciliationSource::OutcomeUnknown => {
                            (OperationOutcome::Applied, OperationReason::MutationApplied)
                        }
                    };
                    let _ = self.complete_page_permission_request(
                        profile,
                        item,
                        request,
                        desired,
                        Some(PagePermissionOperationCompletion {
                            operation_id,
                            outcome,
                            reason,
                        }),
                    );
                    return;
                }
                let (reason, terminal) = match source {
                    ReconciliationSource::Conflict => (OperationReason::StoreConflict, false),
                    ReconciliationSource::OutcomeUnknown => {
                        (OperationReason::StoreOutcomeUnknown, false)
                    }
                    ReconciliationSource::Applied => {
                        (OperationReason::StoreReconciliationFailed, true)
                    }
                };
                self.fail_page_permission_request(
                    profile,
                    item,
                    request,
                    Some(operation_id),
                    reason,
                    terminal,
                );
            }
            PagePermissionPhase::Prompting { .. } | PagePermissionPhase::Mutating { .. } => {
                self.fail_page_permission_request(
                    profile,
                    item,
                    request,
                    None,
                    OperationReason::StoreReconciliationFailed,
                    true,
                );
            }
        }
    }

    pub(super) fn begin_page_permission_response(
        &mut self,
        operation_id: String,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        decision: PagePermissionPromptDecision,
    ) -> Option<OperationDisposition> {
        if !self.page_permissions.exact(profile, item, request) {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        }
        let expired = self
            .page_permissions
            .pending
            .as_ref()
            .is_some_and(|pending| std::time::Instant::now() >= pending.deadline);
        if expired {
            self.on_page_permission_timeout(profile, item, request);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        }
        if !self.page_permission_request_is_foreground(profile, item) {
            self.cancel_page_permission_request(profile, item, request);
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        }
        let (expected, authorities, request_payload) = match self.page_permissions.pending.as_ref()
        {
            Some(PendingPagePermission {
                request,
                phase:
                    PagePermissionPhase::Prompting {
                        expected,
                        authorities,
                    },
                ..
            }) => (*expected, authorities.clone(), request.clone()),
            _ => {
                return Some(operation_result(
                    OperationOutcome::Rejected,
                    OperationReason::InvalidScope,
                ));
            }
        };

        let (desired, remember) = match decision {
            PagePermissionPromptDecision::AllowOnce => (RememberedPagePermission::Allow, false),
            PagePermissionPromptDecision::AlwaysAllow => (RememberedPagePermission::Allow, true),
            PagePermissionPromptDecision::DenyOnce => (RememberedPagePermission::Deny, false),
            PagePermissionPromptDecision::AlwaysDeny => (RememberedPagePermission::Deny, true),
        };
        if !remember {
            let native =
                self.complete_page_permission_request(profile, item, request, desired, None);
            return Some(if native == NativeDispatch::Scheduled {
                operation_result(OperationOutcome::Applied, OperationReason::MutationApplied)
            } else {
                operation_result(
                    OperationOutcome::NativeAdmissionFailed,
                    OperationReason::NativeDispatchRejected,
                )
            });
        }

        let Some(expected) = expected else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::InvalidScope,
            ));
        };

        let Some(patch) = patch_for_decision(&request_payload, &authorities, desired) else {
            return Some(operation_result(
                OperationOutcome::Rejected,
                OperationReason::StoreReconciliationFailed,
            ));
        };
        if let Some(pending) = self.page_permissions.pending.as_mut() {
            pending.phase = PagePermissionPhase::Mutating {
                operation_id: operation_id.clone(),
                desired,
            };
        }
        self.project_page_permission_prompt();
        let Some(queue) = self.self_queue.as_ref() else {
            self.fail_page_permission_request(
                profile,
                item,
                request,
                Some(operation_id),
                OperationReason::StoreAdmissionRejected,
                false,
            );
            return None;
        };
        let callback = CallbackHandle {
            queue: Arc::downgrade(&queue.inner),
        };
        let admitted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.store.mutate_page_permission_catalog(
                profile,
                expected,
                patch,
                Box::new(move |outcome| {
                    let _ = callback.dispatch(Command::PagePermissionCatalogMutated {
                        profile,
                        item,
                        request,
                        outcome: Box::new(outcome),
                    });
                }),
            )
        }))
        .unwrap_or(false);
        if !admitted {
            self.fail_page_permission_request(
                profile,
                item,
                request,
                Some(operation_id),
                OperationReason::StoreAdmissionRejected,
                false,
            );
        }
        None
    }

    pub(super) fn settle_page_permission_catalog_mutation(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        outcome: PagePermissionCatalogMutationOutcome,
    ) {
        if !self.page_permissions.exact(profile, item, request) {
            return;
        }
        let Some(PendingPagePermission {
            phase:
                PagePermissionPhase::Mutating {
                    operation_id,
                    desired,
                },
            ..
        }) = self.page_permissions.pending.as_ref()
        else {
            return;
        };
        let operation_id = operation_id.clone();
        let desired = *desired;
        let source = match outcome {
            PagePermissionCatalogMutationOutcome::Applied(_) => ReconciliationSource::Applied,
            PagePermissionCatalogMutationOutcome::Conflict { .. } => ReconciliationSource::Conflict,
            PagePermissionCatalogMutationOutcome::OutcomeUnknown => {
                ReconciliationSource::OutcomeUnknown
            }
            PagePermissionCatalogMutationOutcome::Invalid
            | PagePermissionCatalogMutationOutcome::RevisionExhausted => {
                self.fail_page_permission_request(
                    profile,
                    item,
                    request,
                    Some(operation_id),
                    OperationReason::StoreReconciliationFailed,
                    true,
                );
                return;
            }
            PagePermissionCatalogMutationOutcome::NotRegistered
            | PagePermissionCatalogMutationOutcome::DegradedProfile
            | PagePermissionCatalogMutationOutcome::LimitReached
            | PagePermissionCatalogMutationOutcome::Failed => {
                self.fail_page_permission_request(
                    profile,
                    item,
                    request,
                    Some(operation_id),
                    OperationReason::StoreAdmissionRejected,
                    false,
                );
                return;
            }
        };
        if let Some(pending) = self.page_permissions.pending.as_mut() {
            pending.phase = PagePermissionPhase::Reconciling {
                operation_id,
                desired,
                source,
            };
        }
        if !self.load_page_permission_catalog(profile, item) {
            let operation_id = match self.page_permissions.pending.as_ref().map(|p| &p.phase) {
                Some(PagePermissionPhase::Reconciling { operation_id, .. }) => {
                    Some(operation_id.clone())
                }
                _ => None,
            };
            self.fail_page_permission_request(
                profile,
                item,
                request,
                operation_id,
                OperationReason::StoreAdmissionRejected,
                true,
            );
        }
    }

    pub(super) fn on_page_permission_timeout(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
    ) {
        if !self.page_permissions.exact(profile, item, request) {
            return;
        }
        let operation_id =
            self.page_permissions
                .pending
                .as_ref()
                .and_then(|pending| match &pending.phase {
                    PagePermissionPhase::Mutating { operation_id, .. }
                    | PagePermissionPhase::Reconciling { operation_id, .. } => {
                        Some(operation_id.clone())
                    }
                    _ => None,
                });
        self.fail_page_permission_request(
            profile,
            item,
            request,
            operation_id,
            OperationReason::StoreAdmissionRejected,
            false,
        );
    }

    pub(super) fn cancel_page_permission_if_not_foreground(&mut self) {
        let Some(pending) = self.page_permissions.pending.as_ref() else {
            return;
        };
        if self.page_permission_request_is_foreground(pending.profile, pending.item) {
            return;
        }
        self.cancel_page_permission_request(pending.profile, pending.item, pending.request.id);
    }

    pub(super) fn cancel_page_permission_for_item(&mut self, item: ItemId) {
        let Some(pending) = self
            .page_permissions
            .pending
            .as_ref()
            .filter(|pending| pending.item == item)
        else {
            return;
        };
        self.cancel_page_permission_request(pending.profile, pending.item, pending.request.id);
    }

    pub(super) fn cancel_page_permission_for_profile(&mut self, profile: ProfileId) {
        let Some(pending) = self
            .page_permissions
            .pending
            .as_ref()
            .filter(|pending| pending.profile == profile)
        else {
            return;
        };
        self.cancel_page_permission_request(pending.profile, pending.item, pending.request.id);
    }

    pub(super) fn cancel_pending_page_permission_for_shutdown(&mut self) {
        let Some(pending) = self.page_permissions.pending.as_ref() else {
            return;
        };
        self.cancel_page_permission_request(pending.profile, pending.item, pending.request.id);
    }

    fn cancel_page_permission_request(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
    ) {
        let operation_id =
            self.page_permissions
                .pending
                .as_ref()
                .and_then(|pending| match &pending.phase {
                    PagePermissionPhase::Mutating { operation_id, .. }
                    | PagePermissionPhase::Reconciling { operation_id, .. } => {
                        Some(operation_id.clone())
                    }
                    _ => None,
                });
        self.fail_page_permission_request(
            profile,
            item,
            request,
            operation_id,
            OperationReason::InvalidScope,
            false,
        );
    }

    fn complete_page_permission_request(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        decision: RememberedPagePermission,
        operation: Option<PagePermissionOperationCompletion>,
    ) -> NativeDispatch {
        let was_visible = self.page_permissions.is_visible();
        if self
            .page_permissions
            .take_exact(profile, item, request)
            .is_none()
        {
            return NativeDispatch::Rejected;
        }
        if let Some(queue) = &self.self_queue {
            queue.cancel_page_permission(profile, item, request);
        }
        let settlement = match decision {
            RememberedPagePermission::Allow => PagePermissionRequestSettlement::Allow,
            RememberedPagePermission::Deny => PagePermissionRequestSettlement::Deny,
        };
        let native = self.settle_native_page_permission(profile, item, request, settlement);
        if let Some(operation) = operation {
            let (outcome, reason) = if native == NativeDispatch::Scheduled {
                (operation.outcome, operation.reason)
            } else {
                (
                    OperationOutcome::NativeAdmissionFailed,
                    OperationReason::NativeDispatchRejected,
                )
            };
            (self.emit)(Projection::OperationProcessed(OperationDisposition {
                operation_id: operation.operation_id,
                outcome,
                reason,
            }));
        }
        if was_visible {
            self.project_page_permission_prompt();
            let _ = self.relayout();
        }
        native
    }

    fn fail_page_permission_request(
        &mut self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        operation_id: Option<String>,
        reason: OperationReason,
        terminal: bool,
    ) {
        if terminal {
            self.page_permissions.failed_until_restart = true;
        }
        let _ = self.complete_page_permission_request(
            profile,
            item,
            request,
            RememberedPagePermission::Deny,
            operation_id.map(|operation_id| PagePermissionOperationCompletion {
                operation_id,
                outcome: OperationOutcome::Rejected,
                reason,
            }),
        );
    }

    fn settle_native_page_permission(
        &self,
        profile: ProfileId,
        item: ItemId,
        request: PagePermissionRequestId,
        settlement: PagePermissionRequestSettlement,
    ) -> NativeDispatch {
        self.engine
            .settle_page_permission_request(profile, item, request, settlement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authority(
        seed: u128,
        kind: PagePermissionKind,
        decision: RememberedPagePermission,
    ) -> RetainedAuthority {
        RetainedAuthority {
            kind,
            grant: Some(PagePermissionGrant {
                id: PagePermissionGrantId::from(seed),
                revision: zephium_core::permissions::PagePermissionGrantRevision::INITIAL,
                origin: zephium_core::permissions::PageOrigin::parse_exact("https://media.example")
                    .expect("canonical test origin"),
                kind,
                decision,
            }),
        }
    }

    #[test]
    fn durable_reconciliation_requires_every_requested_capability() {
        let camera_denied = authority(
            1,
            PagePermissionKind::Camera,
            RememberedPagePermission::Deny,
        );
        assert!(!durable_decision_matches(
            std::slice::from_ref(&camera_denied),
            RememberedPagePermission::Allow,
        ));
        assert!(!durable_decision_matches(
            &[
                camera_denied.clone(),
                RetainedAuthority {
                    kind: PagePermissionKind::Microphone,
                    grant: None,
                },
            ],
            RememberedPagePermission::Deny,
        ));
        assert!(durable_decision_matches(
            &[
                camera_denied,
                authority(
                    2,
                    PagePermissionKind::Microphone,
                    RememberedPagePermission::Deny,
                ),
            ],
            RememberedPagePermission::Deny,
        ));
    }
}
