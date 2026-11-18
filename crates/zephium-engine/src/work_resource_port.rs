//! Bounded Work ingress. Stable resource callbacks and temporary lease tasks
//! have separate owners; failure is scoped unless the shared executor fails.

use super::*;
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Instant;
use zephium_agentic::{
    AgentPolicyInstant, ContextId, ContextNavigationTarget, ContextProfileStorageClass,
    WorkBrowserExecutionLease, WorkBrowserLeaseDeliveryCompletion,
    WorkBrowserObservationCompletionCallback, WorkBrowserObservationDispatch,
    WorkBrowserObservationRequest, WorkBrowserResourceCompletionCallback,
    WorkBrowserResourceDispatch, WorkBrowserResourceHealthReporter, WorkBrowserResourceJoin,
    WorkBrowserResourceNativeOutcome as Outcome, WorkBrowserResourceOperation as Operation,
    WorkBrowserResourceRequest, MAX_LIVE_CONTEXTS,
};

/// Process-monotonic Work clock, initialized only at the explicit Work edge.
/// Original core deadlines must use this domain; no lease rebases its origin.
pub fn work_browser_monotonic_now() -> Option<AgentPolicyInstant> {
    u64::try_from(
        WORK_CLOCK_ORIGIN
            .get_or_init(Instant::now)
            .elapsed()
            .as_millis(),
    )
    .ok()
    .map(AgentPolicyInstant::from_millis)
}

static WORK_CLOCK_ORIGIN: OnceLock<Instant> = OnceLock::new();

/// Projects an existing absolute deadline into the same epoch as native Work
/// admission. Fractional milliseconds round up because policy time is discrete;
/// the original `Instant` remains the independently enforced execution deadline.
/// This never starts a new timeout or adds an arbitrary grace period.
pub fn work_browser_monotonic_deadline(deadline: Instant) -> Option<AgentPolicyInstant> {
    project_work_deadline(*WORK_CLOCK_ORIGIN.get_or_init(Instant::now), deadline)
}

fn project_work_deadline(origin: Instant, deadline: Instant) -> Option<AgentPolicyInstant> {
    let nanos = deadline.checked_duration_since(origin)?.as_nanos();
    u64::try_from(nanos.div_ceil(1_000_000))
        .ok()
        .map(AgentPolicyInstant::from_millis)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Constructing,
    Retained,
    ReadingMedia,
    Acquiring,
    Leased,
    Revoking,
    PresentingHuman,
    PresentedHuman,
    ContinuingAfterHuman,
    Destroying,
    Destroyed,
    Quarantined,
}
struct State {
    phase: Phase,
    construction_pending: bool,
    lease: Option<WorkBrowserExecutionLease>,
    // The lease terminal can transfer before its physical callback returns.
    // Keep that exact delivery reservation separate from the active lease.
    retirement_delivery: Option<WorkBrowserExecutionLease>,
    human_delivery: Option<Operation>,
    media_delivery: bool,
    human_deadline: Option<AgentPolicyInstant>,
    reads: usize,
    navigation: Option<zephium_agentic::ContextOperationJoin>,
    action: Option<zephium_agentic::SemanticActionAttemptId>,
    // Explicit lease retirement closes evidence waiting too, even when sticky
    // quarantine prevents admission of the retirement lifecycle itself.
    action_drain_closed: bool,
    observed: Option<zephium_agentic::ContextJoin>,
    document_epoch: u64,
    callbacks: usize,
    uncertain: bool,
    notification_pending: bool,
}

pub(crate) struct WorkResourceGuard {
    #[cfg(all(
        target_os = "windows",
        feature = "native-agentic-work-lifetime-diagnostic"
    ))]
    pub(super) frame_capture: Mutex<Option<std::sync::Weak<AtomicBool>>>,
    #[cfg(all(
        target_os = "windows",
        feature = "native-agentic-work-lifetime-diagnostic"
    ))]
    pub(super) frame_capture_dispatched: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(super) failure_cause:
        Mutex<Option<super::work_resource_failure_diagnostic::WorkResourceFailureCause>>,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    pub(super) construction_evidence_claimed: AtomicBool,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    pub(super) construction_evidence: Mutex<Option<super::resource_witness::ConstructionEvidence>>,
    admission: std::sync::Weak<AgentPortAdmission>,
    #[cfg(target_os = "windows")]
    notification_native_dispatch: Option<MainThreadDispatch>,
    resource: WorkBrowserResourceJoin,
    storage: ContextProfileStorageClass,
    isolated_public: bool,
    anonymous_session: Option<zephium_agentic::WorkBrowserSession>,
    work_website_data: bool,
    document: Option<ContextNavigationTarget>,
    document_policy: zephium_agentic::WorkBrowserDocumentPolicy,
    construction_window: Option<(zephium_agentic::WorkBrowserConstructionAttempt, Instant)>,
    redirect: Option<zephium_agentic::WorkBrowserRedirect>,
    unreachable: Option<zephium_agentic::WorkBrowserUnreachable>,
    state: Mutex<State>,
    image_receipt: Mutex<image_receipt::NativeImageSlot>,
    fresh_native_store: Mutex<Option<crate::host::FreshNativeStore>>,
    // Drop order is deliberate: the original native reporting owner retires
    // before its counted delivery lane. No audit may overlook a live reporter.
    health: Option<WorkBrowserResourceHealthReporter>,
    health_permit: Option<AgentTaskPermit>,
    #[cfg(test)]
    notification_dispatch: Mutex<Option<MainThreadDispatch>>,
}
pub(crate) struct WorkNotificationPermit {
    _permit: AgentTaskPermit,
}
impl WorkResourceGuard {
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(crate) fn record_failure_cause(
        &self,
        failure: super::work_resource_failure_diagnostic::WorkResourceFailureCause,
    ) {
        // Diagnostics neither fail nor wake the guard. Preserve the first
        // content-free cause across construction, retention, execution and
        // cleanup; poisoned diagnostic storage stays observationally absent.
        if let Ok(mut first) = self.failure_cause.lock() {
            first.get_or_insert(failure);
        }
    }
    fn new(request: &WorkBrowserResourceRequest, admission: &Arc<AgentPortAdmission>) -> Self {
        Self {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            failure_cause: Mutex::new(None),
            #[cfg(all(
                target_os = "windows",
                feature = "native-agentic-work-lifetime-diagnostic"
            ))]
            frame_capture: Mutex::new(None),
            #[cfg(all(
                target_os = "windows",
                feature = "native-agentic-work-lifetime-diagnostic"
            ))]
            frame_capture_dispatched: Mutex::new(None),
            #[cfg(feature = "native-agentic-work-resource-probe")]
            construction_evidence_claimed: AtomicBool::new(false),
            #[cfg(feature = "native-agentic-work-resource-probe")]
            construction_evidence: Mutex::new(None),
            admission: Arc::downgrade(admission),
            #[cfg(target_os = "windows")]
            notification_native_dispatch: None,
            resource: request.resource().clone(),
            storage: request.storage(),
            isolated_public: request.isolated_public(),
            anonymous_session: request.anonymous_session().cloned(),
            work_website_data: request.work_website_data(),
            document: request.document().cloned(),
            document_policy: request.document_policy(),
            construction_window: request.construction_window(),
            redirect: request.redirect().cloned(),
            unreachable: request.unreachable().cloned(),
            health: None,
            health_permit: None,
            #[cfg(test)]
            notification_dispatch: Mutex::new(None),
            image_receipt: Mutex::new(image_receipt::NativeImageSlot::default()),
            fresh_native_store: Mutex::new(None),
            state: Mutex::new(State {
                phase: Phase::Constructing,
                construction_pending: true,
                lease: None,
                retirement_delivery: None,
                human_delivery: None,
                media_delivery: false,
                human_deadline: None,
                reads: 0,
                navigation: None,
                action: None,
                action_drain_closed: false,
                observed: None,
                document_epoch: 1,
                callbacks: 0,
                uncertain: false,
                notification_pending: false,
            }),
        }
    }
    pub(crate) fn resource(&self) -> &WorkBrowserResourceJoin {
        &self.resource
    }
    /// Hands the owner of a public construction the destination its first
    /// load was redirected to; the document still fails.
    pub(crate) fn record_redirect(&self, destination: ContextNavigationTarget) {
        if let Some(redirect) = &self.redirect {
            redirect.record(destination);
        }
    }
    /// Tells the construction's owner that its main-frame load could not
    /// reach the site; the document still fails. The platform calls it only
    /// for the construction load's own failure, never for a later load.
    pub(crate) fn record_unreachable(&self) {
        if let Some(unreachable) = &self.unreachable {
            unreachable.record();
        }
    }
    pub(crate) fn notify_frame_projection(&self) {
        if let Some(health) = &self.health {
            health.notify_projection();
        }
    }
    #[cfg(all(
        target_os = "windows",
        feature = "native-agentic-work-lifetime-diagnostic"
    ))]
    pub(crate) fn bind_frame_capture_diagnostic(&self, pending: &Arc<AtomicBool>) {
        if let Ok(mut slot) = self.frame_capture.lock() {
            *slot = Some(Arc::downgrade(pending));
        }
    }
    #[cfg(all(
        target_os = "windows",
        feature = "native-agentic-work-lifetime-diagnostic"
    ))]
    pub(crate) fn notify_frame_capture_dispatched_diagnostic(&self) {
        let pending = self
            .frame_capture
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(std::sync::Weak::upgrade));
        if !pending.is_some_and(|pending| pending.load(Ordering::Acquire)) {
            return;
        }
        // A qualifier may close its own original session at this exact native
        // dispatch edge. Never hold diagnostic or authority locks across it.
        let callback = self
            .frame_capture_dispatched
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(callback) = callback {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
        }
    }
    #[cfg(target_os = "windows")]
    pub(crate) fn frame_projection_current(&self) -> bool {
        self.session_current()
            && self.port_open()
            && self.is_healthy()
            && self.state.lock().is_ok_and(|state| {
                !matches!(
                    state.phase,
                    Phase::Destroying | Phase::Destroyed | Phase::Quarantined
                )
            })
    }
    fn health_current(&self) -> bool {
        self.health
            .as_ref()
            .is_none_or(|health| health.is_current(&self.resource))
    }
    fn install_health(&self) {
        if self
            .health
            .as_ref()
            .is_some_and(|health| !health.install(&self.resource))
        {
            self.fail();
        }
    }
    #[cfg_attr(feature = "native-agentic-work-lifetime-diagnostic", track_caller)]
    fn report_uncertainty(&self) {
        let uncertain = self.state.lock().map_or(true, |state| state.uncertain);
        if uncertain {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            self.record_unclassified_failure();
            if let Some(health) = &self.health {
                health.invalidate();
            }
        }
    }
    /// Session retirement must not keep the guard (and its session) alive.
    #[cfg(target_os = "windows")]
    pub(crate) fn retirement_notification(
        self: &Arc<Self>,
        task: impl FnOnce() + Send + 'static,
    ) -> Option<Box<dyn FnOnce() + Send>> {
        let dispatch = self.notification_native_dispatch.clone()?;
        let guard = Arc::downgrade(self);
        Some(Box::new(move || {
            if !dispatch(Box::new(task)) {
                if let Some(guard) = guard.upgrade() {
                    guard.fail();
                }
            }
        }))
    }

    pub(crate) fn dispatch_notification(&self, task: impl FnOnce() + Send + 'static) {
        #[cfg(test)]
        if let Some(dispatch) = self.notification_dispatch.lock().unwrap().clone() {
            if !dispatch(Box::new(task)) {
                self.fail();
            }
            return;
        }
        #[cfg(target_os = "macos")]
        dispatch2::DispatchQueue::main().exec_async(task);
        #[cfg(target_os = "windows")]
        {
            let Some(dispatch) = &self.notification_native_dispatch else {
                self.fail();
                return;
            };
            if !dispatch(Box::new(task)) {
                self.fail();
            }
        }
    }
    pub(crate) fn port_open(&self) -> bool {
        self.admission.upgrade().is_some_and(|admission| {
            admission.state.lock().is_ok_and(|state| {
                !state.sealed
                    && !state.invariant_failed
                    && !state.retired
                    && !admission.lineage_failed()
            })
        })
    }
    pub(crate) fn anonymous_session(&self) -> Option<&zephium_agentic::WorkBrowserSession> {
        self.anonymous_session.as_ref()
    }
    pub(crate) fn isolated_public(&self) -> bool {
        self.isolated_public
    }
    /// Work's own persistent store for the profile, never the profile's own.
    pub(crate) fn work_website_data(&self) -> bool {
        self.isolated_public && self.work_website_data
    }
    pub(crate) fn storage(&self) -> ContextProfileStorageClass {
        self.storage
    }
    fn session_current(&self) -> bool {
        self.anonymous_session
            .as_ref()
            .is_none_or(|session| session.is_current())
    }
    pub(crate) fn construction_current(&self) -> bool {
        self.session_current()
            && self.port_open()
            && self.health_current()
            && self.state.lock().is_ok_and(|state| {
                state.phase == Phase::Constructing && state.construction_pending && !state.uncertain
            })
    }
    fn destruction_started(&self) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.phase == Phase::Destroying)
    }
    fn construction_returned(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.construction_pending {
            state.uncertain = true;
            return false;
        }
        state.construction_pending = false;
        state.phase == Phase::Constructing
    }
    pub(crate) fn document(&self) -> Option<&ContextNavigationTarget> {
        self.document.as_ref()
    }
    pub(crate) fn construction_deadline(&self, now: Instant) -> Option<Instant> {
        let (attempt, original) = self.construction_window.unwrap_or((
            zephium_agentic::WorkBrowserConstructionAttempt::Initial,
            now.checked_add(zephium_agentic::WorkBrowserConstructionAttempt::Initial.budget())?,
        ));
        now.checked_add(attempt.budget())
            .map(|deadline| deadline.min(original))
    }
    pub(crate) fn construction_timed_out(&self) {
        if self.construction_current() && self.is_healthy() {
            if let Some(health) = &self.health {
                health.construction_timed_out();
            }
        }
    }
    pub(crate) fn document_policy(&self) -> zephium_agentic::WorkBrowserDocumentPolicy {
        self.document_policy
    }
    #[cfg_attr(feature = "native-agentic-work-lifetime-diagnostic", track_caller)]
    pub(crate) fn fail(&self) {
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        self.record_unclassified_failure();
        // Invalidate the stable application observation before waking it. No
        // native ownership lock is held while invoking its coalesced wake.
        if let Some(health) = &self.health {
            health.invalidate();
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.uncertain = true;
        if !matches!(state.phase, Phase::Destroying | Phase::Destroyed) {
            state.phase = Phase::Quarantined;
        }
        drop(state);
        self.invalidate_image_sources();
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    #[track_caller]
    fn record_unclassified_failure(&self) {
        use super::work_resource_failure_diagnostic::{
            WorkNativeGuardFailureSource, WorkResourceFailureCause,
        };
        let caller = std::panic::Location::caller();
        self.record_failure_cause(WorkResourceFailureCause::NativeGuardFailure {
            source: WorkNativeGuardFailureSource::of(caller.file()),
            line: caller.line(),
        });
    }
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn bind_fresh_native_store(
        &self,
        proof: crate::host::FreshNativeStore,
    ) -> Result<(), ContextPortFailure> {
        if !self.construction_current()
            || self.work_website_data
            || !self.isolated_public
            || self.anonymous_session.is_none()
        {
            return Err(ContextPortFailure::Stale);
        }
        let mut slot = self
            .fresh_native_store
            .lock()
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        if slot.is_some() {
            return Err(ContextPortFailure::Stale);
        }
        *slot = Some(proof);
        Ok(())
    }
    pub(crate) fn revoke_public_visual_lineage(&self) {
        let proof = self
            .fresh_native_store
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(proof) = proof {
            proof.revoke();
        }
    }
    fn admit_lifecycle(
        &self,
        request: &WorkBrowserResourceRequest,
        now: AgentPolicyInstant,
    ) -> Result<(), ContextPortFailure> {
        if request.resource() != &self.resource
            || request.storage() != self.storage
            || request.isolated_public() != self.isolated_public
            || request.document() != self.document.as_ref()
            || request.document_policy() != self.document_policy
        {
            return Err(ContextPortFailure::Stale);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        if request.operation() == Operation::Revoke
            && request.lease().is_some()
            && state.lease.as_ref() == request.lease()
        {
            state.action_drain_closed = true;
        }
        match request.operation() {
            Operation::ReadMedia
                if self.session_current()
                    && self.health_current()
                    && state.phase == Phase::Retained
                    && !state.uncertain
                    && state.lease.is_none()
                    && state.retirement_delivery.is_none()
                    && state.human_delivery.is_none()
                    && !state.media_delivery
                    && state.reads == 0
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && state.callbacks == 0 =>
            {
                if request
                    .media_plans()
                    .is_none_or(|plans| plans.is_empty() || plans.len() > 12)
                    || request.media_deadline().is_none_or(|deadline| {
                        deadline <= now || deadline.millis() - now.millis() > 8000
                    })
                {
                    return Err(ContextPortFailure::Stale);
                }
                state.phase = Phase::ReadingMedia;
                state.media_delivery = true;
            }
            Operation::Acquire
                if self.session_current()
                    && self.health_current()
                    && state.phase == Phase::Retained
                    && !state.uncertain
                    && state.lease.is_none()
                    && state.retirement_delivery.is_none()
                    && state.human_delivery.is_none()
                    && !state.media_delivery
                    && state.reads == 0
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && state.callbacks == 0 =>
            {
                let lease = request
                    .lease()
                    .filter(|lease| now < lease.deadline())
                    .ok_or(ContextPortFailure::TimedOut)?;
                state.lease = Some(lease.clone());
                state.phase = Phase::Acquiring;
            }
            Operation::PresentHuman
                if self.session_current()
                    && self.health_current()
                    && state.phase == Phase::Retained
                    && !state.uncertain
                    && state.lease.is_none()
                    && state.retirement_delivery.is_none()
                    && state.human_delivery.is_none()
                    && !state.media_delivery
                    && state.reads == 0
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && state.callbacks == 0 =>
            {
                let deadline = request
                    .human_deadline()
                    .filter(|deadline| {
                        *deadline > now
                            && deadline.millis() - now.millis()
                                <= zephium_agentic::MAX_WORK_HUMAN_WAIT_MILLIS
                    })
                    .ok_or(ContextPortFailure::TimedOut)?;
                if request.human_source().is_none() {
                    return Err(ContextPortFailure::Stale);
                }
                state.human_deadline = Some(deadline);
                state.human_delivery = Some(Operation::PresentHuman);
                state.observed = None;
                state.phase = Phase::PresentingHuman;
            }
            Operation::ContinueAfterHuman
                if self.session_current()
                    && self.health_current()
                    && state.phase == Phase::PresentedHuman
                    && !state.uncertain
                    && state.human_delivery.is_none()
                    && !state.media_delivery
                    && state.human_deadline == request.human_deadline()
                    && state.human_deadline.is_some_and(|deadline| now < deadline) =>
            {
                state.human_delivery = Some(Operation::ContinueAfterHuman);
                state.phase = Phase::ContinuingAfterHuman;
            }
            Operation::Revoke if state.phase == Phase::Leased || state.phase == Phase::Revoking => {
                if state.lease.as_ref() != request.lease() || state.retirement_delivery.is_some() {
                    return Err(ContextPortFailure::Stale);
                }
                state.phase = Phase::Revoking;
                state.retirement_delivery = request.lease().cloned();
            }
            Operation::Destroy
                if state.phase != Phase::Destroyed && state.phase != Phase::Destroying =>
            {
                state.phase = Phase::Destroying;
            }
            _ => return Err(ContextPortFailure::Stale),
        }
        drop(state);
        self.invalidate_image_sources();
        if request.operation() == Operation::PresentHuman {
            self.revoke_public_visual_lineage();
        }
        Ok(())
    }
    pub(crate) fn admits(
        &self,
        lease: &WorkBrowserExecutionLease,
        now: AgentPolicyInstant,
    ) -> bool {
        self.session_current()
            && self.port_open()
            && self.health_current()
            && self.state.lock().is_ok_and(|state| {
                !state.uncertain
                    && state.phase == Phase::Leased
                    && state.retirement_delivery.is_none()
                    && state.callbacks == 0
                    && state.lease.as_ref() == Some(lease)
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && now < lease.deadline()
            })
    }
    pub(crate) fn acquisition_current(
        &self,
        lease: &WorkBrowserExecutionLease,
        now: AgentPolicyInstant,
    ) -> bool {
        self.session_current()
            && self.port_open()
            && self.health_current()
            && self.state.lock().is_ok_and(|state| {
                !state.uncertain
                    && state.phase == Phase::Acquiring
                    && state.retirement_delivery.is_none()
                    && state.callbacks == 0
                    && state.lease.as_ref() == Some(lease)
                    && now < lease.deadline()
            })
    }
    pub(crate) fn execution_reserved(&self) -> bool {
        self.state.lock().map_or(true, |state| {
            state.lease.is_some() || state.retirement_delivery.is_some()
        })
    }
    pub(crate) fn human_current(&self, operation: Operation, now: AgentPolicyInstant) -> bool {
        self.session_current()
            && self.port_open()
            && self.health_current()
            && self.state.lock().is_ok_and(|state| {
                !state.uncertain
                    && state.lease.is_none()
                    && state.retirement_delivery.is_none()
                    && state.reads == 0
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && state.callbacks == 0
                    && state.human_deadline.is_some_and(|deadline| now < deadline)
                    && matches!(
                        (operation, state.phase),
                        (
                            Operation::PresentHuman,
                            Phase::PresentingHuman | Phase::PresentedHuman
                        ) | (Operation::ContinueAfterHuman, Phase::ContinuingAfterHuman)
                    )
            })
    }
    #[cfg(all(
        target_os = "windows",
        feature = "native-agentic-work-lifetime-diagnostic"
    ))]
    pub(crate) fn revocation_drain_facts(
        &self,
        lease: &WorkBrowserExecutionLease,
    ) -> Option<[bool; 8]> {
        let state = self.state.lock().ok()?;
        Some([
            self.health_current(),
            !state.uncertain && state.phase == Phase::Revoking,
            state.lease.as_ref() == Some(lease),
            state.retirement_delivery.as_ref() == Some(lease),
            state.reads == 0,
            state.navigation.is_none(),
            state.action.is_none(),
            state.callbacks == 0,
        ])
    }
    pub(crate) fn lease_drained(&self, lease: &WorkBrowserExecutionLease) -> bool {
        self.health_current()
            && self.state.lock().is_ok_and(|state| {
                !state.uncertain
                    && state.phase == Phase::Revoking
                    && state.lease.as_ref() == Some(lease)
                    && state.retirement_delivery.as_ref() == Some(lease)
                    && state.reads == 0
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && state.callbacks == 0
            })
    }
    pub(crate) fn callbacks_drained(&self) -> bool {
        self.state.lock().is_ok_and(|state| {
            !state.construction_pending
                && state.retirement_delivery.is_none()
                && state.human_delivery.is_none()
                && !state.media_delivery
                && state.reads == 0
                && state.navigation.is_none()
                && state.action.is_none()
                && state.callbacks == 0
                && !state.notification_pending
        })
    }
    pub(crate) fn begin_notification(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.notification_pending || state.phase == Phase::Destroyed {
            return false;
        }
        state.notification_pending = true;
        true
    }
    pub(crate) fn notification_permit(&self) -> Option<WorkNotificationPermit> {
        match self
            .admission
            .upgrade()
            .and_then(|admission| admission.reserve_audit().ok())
        {
            Some(permit) => Some(WorkNotificationPermit { _permit: permit }),
            None => {
                self.consume_notification();
                self.fail();
                None
            }
        }
    }
    pub(crate) fn consume_notification(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.notification_pending {
            state.uncertain = true;
        }
        state.notification_pending = false;
        drop(state);
        self.report_uncertainty();
    }
    pub(crate) fn is_healthy(&self) -> bool {
        self.health_current() && self.state.lock().is_ok_and(|state| !state.uncertain)
    }
    fn admit_read(
        &self,
        request: &WorkBrowserObservationRequest,
        now: AgentPolicyInstant,
    ) -> Result<(), ContextPortFailure> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        if !self.session_current()
            || state.uncertain
            || !self.health_current()
            || state.phase != Phase::Leased
            || state.lease.as_ref() != Some(request.lease())
        {
            return Err(ContextPortFailure::Stale);
        }
        if now >= request.lease().deadline() {
            return Err(ContextPortFailure::TimedOut);
        }
        if state.reads != 0
            || state.navigation.is_some()
            || state.action.is_some()
            || state.callbacks != 0
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let context = request.invocation().frame().context();
        if context.navigation_epoch().get() != state.document_epoch
            || context.frame_generation().get() != state.document_epoch
        {
            return Err(ContextPortFailure::Stale);
        }
        state.reads = 1;
        state.observed = Some(context);
        Ok(())
    }
    fn read_terminal_begin(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.reads != 1 || state.callbacks != 0 {
            state.uncertain = true;
        }
        state.callbacks = 1;
        drop(state);
        self.report_uncertainty();
    }
    fn read_terminal_end(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.reads != 1 || state.callbacks != 1 {
            state.uncertain = true;
        }
        state.reads = 0;
        state.callbacks = 0;
        drop(state);
        self.report_uncertainty();
    }
    fn outcome(&self, request: &WorkBrowserResourceRequest, outcome: Outcome) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if request.operation() == Operation::Construct {
            if !state.construction_pending {
                state.uncertain = true;
            }
            state.construction_pending = false;
        }
        match (request.operation(), outcome) {
            (Operation::ReadMedia, Outcome::MediaRead)
                if state.media_delivery && !state.uncertain =>
            {
                if state.phase == Phase::ReadingMedia {
                    state.phase = Phase::Retained;
                }
            }
            (Operation::Construct, Outcome::Constructed)
                if state.phase == Phase::Constructing && !state.uncertain =>
            {
                state.phase = Phase::Retained
            }
            (Operation::Acquire, Outcome::Acquired)
                if state.phase == Phase::Acquiring && !state.uncertain =>
            {
                state.phase = if work_browser_monotonic_now()
                    .is_some_and(|now| request.lease().is_some_and(|lease| now < lease.deadline()))
                {
                    Phase::Leased
                } else {
                    Phase::Revoking
                };
            }
            (
                Operation::Revoke,
                Outcome::Revoked {
                    debt,
                    resource_retained,
                },
            ) if state.phase == Phase::Revoking
                && !state.uncertain
                && debt == zephium_agentic::WorkBrowserLeaseNativeDebt::default()
                && resource_retained
                && state.reads == 0
                && state.navigation.is_none()
                && state.action.is_none()
                && state.callbacks == 0 =>
            {
                state.phase = Phase::Retained;
                state.lease = None;
                // Native Acquire remains closed until the exact callback and
                // its task permit have finished, even for legacy consumers.
            }
            (Operation::Destroy, Outcome::Destroyed)
                if !state.construction_pending
                    && state.retirement_delivery.is_none()
                    && state.human_delivery.is_none()
                    && !state.media_delivery
                    && state.reads == 0
                    && state.navigation.is_none()
                    && state.action.is_none()
                    && state.callbacks == 0 =>
            {
                state.phase = Phase::Destroyed;
                state.lease = None;
            }
            (Operation::PresentHuman, Outcome::HumanPresented)
                if state.phase == Phase::PresentingHuman
                    && !state.uncertain
                    && state.human_delivery == Some(Operation::PresentHuman) =>
            {
                state.phase = Phase::PresentedHuman;
            }
            (Operation::ContinueAfterHuman, Outcome::HumanContinued)
                if state.phase == Phase::ContinuingAfterHuman
                    && !state.uncertain
                    && state.human_delivery == Some(Operation::ContinueAfterHuman) =>
            {
                if let Some(epoch) = state.document_epoch.checked_add(1) {
                    state.document_epoch = epoch;
                    state.phase = Phase::Retained;
                    state.human_deadline = None;
                } else {
                    state.uncertain = true;
                    state.phase = Phase::Quarantined;
                }
            }
            _ => {
                state.uncertain = true;
                if !matches!(state.phase, Phase::Destroying | Phase::Destroyed) {
                    state.phase = Phase::Quarantined;
                }
            }
        }
        drop(state);
        self.report_uncertainty();
    }
    fn finish_revocation_delivery(
        &self,
        lease: &WorkBrowserExecutionLease,
        callback_returned: bool,
        permit_released: bool,
        mut delivery: Option<WorkBrowserLeaseDeliveryCompletion>,
    ) {
        let notification = delivery
            .as_mut()
            .and_then(|delivery| delivery.take_notification());
        let port_open = self.port_open();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let exact = state.retirement_delivery.as_ref() == Some(lease);
        let retained = exact
            && callback_returned
            && permit_released
            && port_open
            && self.health_current()
            && !state.uncertain
            && state.phase == Phase::Retained
            && state.lease.is_none()
            && state.reads == 0
            && state.navigation.is_none()
            && state.action.is_none()
            && state.callbacks == 0;
        // Publication joins the fixed slot with its short registration lock.
        // Registration never invokes code, so it cannot call back into this
        // guard. Acquire cannot observe an open gate before publication.
        let published = if retained {
            delivery.is_none_or(|owner| owner.publish_returned())
        } else {
            drop(delivery);
            false
        };
        if !published {
            state.uncertain = true;
            if !matches!(state.phase, Phase::Destroying | Phase::Destroyed) {
                state.phase = Phase::Quarantined;
            }
        }
        // The physical receipt is published, but this exact retirement owner
        // still seals Acquire/destruction drain across notification reentrancy.
        // Never invoke a listener while holding the native guard mutex.
        drop(state);
        let notified = notification.is_none_or(|notification| notification.notify());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !notified {
            state.uncertain = true;
            if !matches!(state.phase, Phase::Destroying | Phase::Destroyed) {
                state.phase = Phase::Quarantined;
            }
        }
        if exact && permit_released && state.retirement_delivery.as_ref() == Some(lease) {
            // Failure still remains quarantined; physical return may release
            // this debt so original resource destruction can subsequently drain.
            state.retirement_delivery = None;
        }
        drop(state);
        self.report_uncertainty();
    }
    fn not_admitted(&self, request: &WorkBrowserResourceRequest) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if request.operation() == Operation::Acquire && state.lease.as_ref() == request.lease() {
            // Returned original request proves that this binding never
            // entered native execution. Do not retain a fictitious lease.
            state.lease = None;
            if state.phase == Phase::Acquiring && !state.uncertain {
                state.phase = Phase::Retained;
            }
        } else {
            if request.operation() == Operation::ReadMedia && state.media_delivery {
                state.media_delivery = false;
                if state.phase == Phase::ReadingMedia {
                    state.phase = Phase::Retained;
                }
            }
            if state.human_delivery == Some(request.operation()) {
                state.human_delivery = None;
            }
            if request.operation() == Operation::Revoke
                && state.retirement_delivery.as_ref() == request.lease()
            {
                state.retirement_delivery = None;
            }
            state.uncertain = true;
            if state.phase != Phase::Destroyed {
                state.phase = Phase::Quarantined;
            }
        }
        drop(state);
        self.report_uncertainty();
    }
    pub(crate) fn media_current(&self) -> bool {
        self.port_open()
            && self.session_current()
            && self.health_current()
            && self.state.lock().is_ok_and(|state| {
                !state.uncertain && state.phase == Phase::ReadingMedia && state.media_delivery
            })
    }
    fn finish_media_delivery(&self, returned: bool, released: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.media_delivery || !returned || !released {
            state.uncertain = true;
        }
        if released {
            state.media_delivery = false;
        }
        drop(state);
        self.report_uncertainty();
    }
    fn finish_human_delivery(&self, operation: Operation, returned: bool, released: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let exact = state.human_delivery == Some(operation);
        if !exact || !returned || !released {
            state.uncertain = true;
            if !matches!(state.phase, Phase::Destroying | Phase::Destroyed) {
                state.phase = Phase::Quarantined;
            }
        }
        if exact && released {
            state.human_delivery = None;
        }
        drop(state);
        self.report_uncertainty();
    }
}

#[derive(Default)]
pub(super) struct WorkIngress {
    rows: BTreeMap<ContextId, Arc<WorkResourceGuard>>,
}
impl AgentPortAdmission {
    #[cfg(all(test, feature = "native-agentic-work-lifetime-diagnostic"))]
    pub(super) fn resource_failure_cause(
        &self,
        resource: &WorkBrowserResourceJoin,
    ) -> Option<super::work_resource_failure_diagnostic::WorkResourceFailureCause> {
        let guard = self.resource_diagnostic_guard(resource)?;
        let result = *guard.failure_cause.lock().ok()?;
        result
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    pub(super) fn resource_diagnostic_guard(
        &self,
        resource: &WorkBrowserResourceJoin,
    ) -> Option<Arc<WorkResourceGuard>> {
        self.work
            .lock()
            .ok()?
            .rows
            .get(&resource.identity().context())
            .filter(|guard| guard.resource() == resource)
            .cloned()
    }
    #[cfg(feature = "native-agentic-work-resource-probe")]
    pub(super) fn witness_resource(
        &self,
        resource: &WorkBrowserResourceJoin,
    ) -> Option<Arc<WorkResourceGuard>> {
        self.work
            .lock()
            .ok()?
            .rows
            .get(&resource.identity().context())
            .filter(|guard| guard.resource() == resource)
            .cloned()
    }
    pub(super) fn work_is_absent(&self) -> bool {
        self.work
            .lock()
            .is_ok_and(|ingress| ingress.rows.is_empty())
    }
    fn work_construction_returned(&self, guard: &Arc<WorkResourceGuard>) {
        let wake = {
            let mut ingress = self
                .work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Same ingress lock as Destroy admission: a returned Construct
            // cannot erase the cleanup owner admitted during dispatch setup.
            let remove = guard.construction_returned();
            let id = guard.resource.identity().context();
            if remove
                && ingress
                    .rows
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, guard))
            {
                ingress.rows.remove(&id);
                false
            } else {
                true
            }
        };
        guard.report_uncertainty();
        if wake {
            crate::host::notify_work_resource(guard.clone());
        }
    }
}

impl AgentContextTask {
    pub(crate) fn work_ingress_matches(&self, guards: Vec<Arc<WorkResourceGuard>>) -> bool {
        if guards
            .iter()
            .map(|guard| guard.resource.identity().context())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != guards.len()
        {
            return false;
        }
        if let Some(group) = &self.permit.admission.group {
            let Some(group) = group.upgrade() else {
                return false;
            };
            let Ok(members) = group.members.lock() else {
                return false;
            };
            let Ok(ingress) = members
                .iter()
                .map(|member| member.work.lock())
                .collect::<Result<Vec<_>, _>>()
            else {
                return false;
            };
            return ingress.iter().map(|rows| rows.rows.len()).sum::<usize>() == guards.len()
                && guards.iter().all(|guard| {
                    ingress.iter().any(|rows| {
                        rows.rows
                            .get(&guard.resource.identity().context())
                            .is_some_and(|current| Arc::ptr_eq(current, guard))
                    })
                });
        }
        self.permit.admission.work.lock().is_ok_and(|ingress| {
            ingress.rows.len() == guards.len()
                && guards.iter().all(|guard| {
                    ingress
                        .rows
                        .get(&guard.resource.identity().context())
                        .is_some_and(|current| Arc::ptr_eq(current, guard))
                })
        })
    }
}

pub(crate) struct WorkLifecycleTask {
    request: Option<WorkBrowserResourceRequest>,
    completion: Option<WorkBrowserResourceCompletionCallback>,
    guard: Arc<WorkResourceGuard>,
    permit: AgentTaskPermit,
}
impl WorkLifecycleTask {
    pub(crate) fn request(&self) -> Option<&WorkBrowserResourceRequest> {
        self.request.as_ref()
    }
    pub(crate) fn guard(&self) -> Arc<WorkResourceGuard> {
        self.guard.clone()
    }
    pub(crate) fn complete(mut self, outcome: Outcome) {
        self.deliver(outcome);
    }
    pub(crate) fn complete_document(mut self, effective: ContextNavigationTarget) {
        self.deliver_document(Outcome::Constructed, Some(effective));
    }
    pub(crate) fn complete_human_document(mut self, effective: ContextNavigationTarget) {
        self.deliver_document(Outcome::HumanContinued, Some(effective));
    }
    fn deliver(&mut self, outcome: Outcome) {
        self.deliver_document(outcome, None);
    }
    fn deliver_document(&mut self, outcome: Outcome, effective: Option<ContextNavigationTarget>) {
        let Some(mut request) = self.request.take() else {
            return;
        };
        let construction = request.operation() == Operation::Construct;
        let media = request.operation() == Operation::ReadMedia;
        let human = matches!(
            request.operation(),
            Operation::PresentHuman | Operation::ContinueAfterHuman
        )
        .then_some(request.operation());
        let revocation = (request.operation() == Operation::Revoke)
            .then(|| request.lease().cloned())
            .flatten();
        let delivery = request.take_lease_delivery_completion();
        self.guard.outcome(&request, outcome);
        // Moving this request into the FnOnce argument transfers its sole
        // lease-bearing terminal owner to the application at callback entry.
        // The physical delivery remains independently owned until return. Keep
        // its exact reservation and shared task permit, preventing early scoped
        // drain, native acquisition, destruction, or global-zero proof.
        let callback_returned = if let Some(completion) = self.completion.take() {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                completion(match effective {
                    Some(document) if outcome == Outcome::HumanContinued => {
                        request.complete_human_document(document)
                    }
                    Some(document) => request.complete_document(document),
                    None => request.complete(outcome),
                })
            }))
            .is_err()
            {
                self.guard.fail();
                false
            } else {
                true
            }
        } else {
            false
        };
        if outcome == Outcome::Destroyed {
            let mut ingress = self
                .permit
                .admission
                .work
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let matched = if ingress
                .rows
                .get(&self.guard.resource.identity().context())
                .is_some_and(|guard| Arc::ptr_eq(guard, &self.guard))
            {
                ingress
                    .rows
                    .remove(&self.guard.resource.identity().context());
                true
            } else {
                false
            };
            drop(ingress);
            if !matched {
                self.guard.fail();
            }
        }
        self.permit.release();
        if media {
            self.guard.finish_media_delivery(
                callback_returned,
                self.permit.released && self.permit.admission.counts().is_some(),
            );
        }
        if let Some(operation) = human {
            self.guard.finish_human_delivery(
                operation,
                callback_returned,
                self.permit.released && self.permit.admission.counts().is_some(),
            );
        }
        if let Some(lease) = &revocation {
            self.guard.finish_revocation_delivery(
                lease,
                callback_returned,
                self.permit.released && self.permit.admission.counts().is_some(),
                delivery,
            );
        }
        if (construction || revocation.is_some() || human.is_some() || media)
            && self.guard.destruction_started()
        {
            crate::host::notify_work_resource(self.guard.clone());
        }
    }
    fn rejected(mut self) -> Option<WorkBrowserResourceRequest> {
        self.completion = None;
        let request = self.request.take();
        if request
            .as_ref()
            .is_some_and(|request| request.operation() == Operation::Construct)
        {
            self.permit
                .admission
                .work_construction_returned(&self.guard);
        } else {
            if let Some(request) = &request {
                self.guard.not_admitted(request);
            }
        }
        self.permit.release();
        request
    }
}
impl Drop for WorkLifecycleTask {
    fn drop(&mut self) {
        if self.request.is_some() {
            self.guard.fail();
            self.deliver(Outcome::Refused);
        }
    }
}

pub(crate) struct WorkObservationTask {
    request: Option<WorkBrowserObservationRequest>,
    terminal: Option<zephium_agentic::WorkBrowserObservationCompletion>,
    completion: Option<WorkBrowserObservationCompletionCallback>,
    guard: Arc<WorkResourceGuard>,
    permit: AgentTaskPermit,
}
impl WorkObservationTask {
    pub(crate) fn request(&self) -> Option<&WorkBrowserObservationRequest> {
        self.request.as_ref()
    }
    pub(crate) fn guard(&self) -> Arc<WorkResourceGuard> {
        self.guard.clone()
    }
    pub(crate) fn take_invocation(&mut self) -> Option<SemanticRuntimeInvocation> {
        let (invocation, terminal) = self.request.take()?.into_parts();
        self.terminal = Some(terminal);
        Some(invocation)
    }
    pub(crate) fn complete(
        mut self,
        outcome: Result<zephium_agentic::SemanticSnapshot, SemanticRuntimePortFailure>,
    ) {
        if let Some(owner) = self.terminal.take() {
            self.deliver(owner.settle(outcome));
        } else {
            self.guard.fail();
        }
    }
    pub(crate) fn refuse(mut self, failure: SemanticRuntimePortFailure) {
        if let Some(request) = self.request.take() {
            let (_, owner) = request.into_parts();
            self.deliver(owner.settle(Err(failure)));
        }
    }
    fn deliver(&mut self, owner: zephium_agentic::WorkBrowserObservationCompletion) {
        self.guard.read_terminal_begin();
        if let Some(completion) = self.completion.take() {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| completion(owner))).is_err()
            {
                self.guard.fail();
            }
        }
        self.guard.read_terminal_end();
        self.permit.release();
        let guard = self.guard.clone();
        let rejected = guard.clone();
        if !crate::host::try_with_agent_context_terminal(move |host| {
            host.progress_work_resource(&guard)
        }) {
            rejected.fail();
        }
    }
    fn rejected(mut self) -> Option<WorkBrowserObservationRequest> {
        self.completion = None;
        let request = self.request.take();
        self.guard.read_terminal_begin();
        self.guard.read_terminal_end();
        self.permit.release();
        request
    }
}
impl Drop for WorkObservationTask {
    fn drop(&mut self) {
        if self.completion.is_some() {
            self.guard.fail();
            if let Some(request) = self.request.take() {
                let (_, owner) = request.into_parts();
                self.deliver(owner);
            } else if let Some(owner) = self.terminal.take() {
                self.deliver(owner);
            }
        }
    }
}

impl EngineAgentBrowserPort {
    pub(super) fn native_image_public_candidate(
        &self,
        lease: &WorkBrowserExecutionLease,
        context: zephium_agentic::ContextJoin,
    ) -> bool {
        let guard = self
            .admission
            .work
            .lock()
            .ok()
            .and_then(|ingress| ingress.rows.get(&context.identity().id()).cloned());
        guard.is_some_and(|guard| guard.public_image_candidate(lease, context))
    }

    pub(super) fn claim_native_image_source(
        &self,
        lease: &WorkBrowserExecutionLease,
        image: &zephium_agentic::SemanticScreenshot,
    ) -> Option<Arc<dyn zephium_agentic::SemanticScreenshotSource>> {
        let guard = self
            .admission
            .work
            .lock()
            .ok()?
            .rows
            .get(&image.context().identity().id())?
            .clone();
        if guard.resource() != lease.resource() {
            return None;
        }
        guard.claim_image_source(lease, image)
    }

    pub(super) fn schedule_work_lifecycle(
        &self,
        mut request: WorkBrowserResourceRequest,
        completion: WorkBrowserResourceCompletionCallback,
    ) -> WorkBrowserResourceDispatch {
        let reject = |request, failure| WorkBrowserResourceDispatch::Rejected {
            request: Box::new(request),
            failure,
        };
        let Some(now) = work_browser_monotonic_now() else {
            return reject(request, ContextPortFailure::NativeRefused);
        };
        // A group's pages share one run; each still opens in its own store
        // (the run's anonymous one, or the person's session for its site).
        if let Some(group) = &self.admission.group {
            if group
                .upgrade()
                .is_none_or(|group| group.work != request.resource().identity().work())
            {
                return reject(request, ContextPortFailure::NativeRefused);
            }
        }
        let health = request.take_resource_health_reporter();
        let health_permit = if health.is_some() {
            match self.admission.reserve() {
                Ok(permit) => Some(permit),
                Err(failure) => return reject(request, failure),
            }
        } else {
            None
        };
        let guard = {
            let Ok(mut ingress) = self.admission.work.lock() else {
                return reject(request, ContextPortFailure::NativeRefused);
            };
            let id = request.resource().identity().context();
            if request.operation() == Operation::Construct {
                if self.admission.group.is_some() && !ingress.rows.is_empty() {
                    return reject(request, ContextPortFailure::ResourceExhausted);
                }
                if ingress.rows.contains_key(&id) {
                    return reject(request, ContextPortFailure::Stale);
                }
                if ingress.rows.len() >= MAX_LIVE_CONTEXTS {
                    return reject(request, ContextPortFailure::ResourceExhausted);
                }
                let mut guard = WorkResourceGuard::new(&request, &self.admission);
                #[cfg(target_os = "windows")]
                {
                    guard.notification_native_dispatch = Some(self.dispatch.clone());
                }
                guard.health = health;
                guard.health_permit = health_permit;
                let guard = Arc::new(guard);
                ingress.rows.insert(id, guard.clone());
                guard
            } else {
                let Some(guard) = ingress.rows.get(&id).cloned() else {
                    return reject(request, ContextPortFailure::Stale);
                };
                if request.operation() == Operation::Acquire
                    && ingress
                        .rows
                        .values()
                        .filter(|guard| guard.execution_reserved())
                        .count()
                        >= zephium_agentic::MAX_EXECUTING_CONTEXTS
                {
                    return reject(request, ContextPortFailure::ResourceExhausted);
                }
                if let Err(failure) = guard.admit_lifecycle(&request, now) {
                    return reject(request, failure);
                }
                guard
            }
        };
        // The immutable resource observer is installed only after releasing
        // ingress. Its wake may reenter trusted application code, never while
        // holding the native admission map or a resource-state mutex.
        if request.operation() == Operation::Construct {
            guard.install_health();
        }
        let permit = match self.admission.reserve() {
            Ok(permit) => permit,
            Err(failure) => {
                if request.operation() == Operation::Construct {
                    self.admission.work_construction_returned(&guard);
                } else {
                    guard.not_admitted(&request);
                }
                return reject(request, failure);
            }
        };
        let slot = Arc::new(Mutex::new(Some(WorkLifecycleTask {
            request: Some(request),
            completion: Some(completion),
            guard,
            permit,
        })));
        let for_dispatch = slot.clone();
        let executed = Arc::new(AtomicBool::new(false));
        let in_dispatch = executed.clone();
        let accepted = contain_agent_port_panic(&self.admission, || {
            (self.dispatch)(Box::new(move || {
                in_dispatch.store(true, Ordering::Release);
                let Some(task) = for_dispatch
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                else {
                    return;
                };
                let slot = Arc::new(Mutex::new(Some(task)));
                let for_host = slot.clone();
                if !crate::host::try_with_agent_context(move |host| {
                    if let Some(task) = for_host
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        let admission = task.permit.admission.clone();
                        let _ = contain_agent_port_panic(&admission, || {
                            host.handle_work_lifecycle_task(task)
                        });
                    }
                }) {
                    if let Some(task) = slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                        {
                            task.guard.record_failure_cause(super::work_resource_failure_diagnostic::WorkResourceFailureCause::NativeAdmission(ContextPortFailure::NativeRefused));
                        }
                        task.complete(Outcome::Refused);
                    }
                }
            }))
        })
        .unwrap_or(false);
        if accepted || executed.load(Ordering::Acquire) {
            return WorkBrowserResourceDispatch::Scheduled;
        }
        let task = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match task.and_then(WorkLifecycleTask::rejected) {
            Some(request) => reject(request, ContextPortFailure::Shutdown),
            None => {
                self.admission.fail_invariant();
                WorkBrowserResourceDispatch::Scheduled
            }
        }
    }

    pub(super) fn schedule_work_observation(
        &self,
        request: WorkBrowserObservationRequest,
        completion: WorkBrowserObservationCompletionCallback,
    ) -> WorkBrowserObservationDispatch {
        let reject = |request, failure| WorkBrowserObservationDispatch::Rejected {
            request: Box::new(request),
            failure,
        };
        let Some(now) = work_browser_monotonic_now() else {
            return reject(request, ContextPortFailure::NativeRefused);
        };
        let guard = self.admission.work.lock().ok().and_then(|ingress| {
            ingress
                .rows
                .get(&request.lease().resource().identity().context())
                .cloned()
        });
        let Some(guard) = guard else {
            return reject(request, ContextPortFailure::Stale);
        };
        let permit = match self.admission.reserve() {
            Ok(permit) => permit,
            Err(failure) => return reject(request, failure),
        };
        if let Err(failure) = guard.admit_read(&request, now) {
            return reject(request, failure);
        }
        let slot = Arc::new(Mutex::new(Some(WorkObservationTask {
            request: Some(request),
            terminal: None,
            completion: Some(completion),
            guard,
            permit,
        })));
        let for_dispatch = slot.clone();
        let executed = Arc::new(AtomicBool::new(false));
        let in_dispatch = executed.clone();
        let accepted = contain_agent_port_panic(&self.admission, || {
            (self.dispatch)(Box::new(move || {
                in_dispatch.store(true, Ordering::Release);
                let Some(task) = for_dispatch
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                else {
                    return;
                };
                let slot = Arc::new(Mutex::new(Some(task)));
                let for_host = slot.clone();
                if !crate::host::try_with_agent_context(move |host| {
                    if let Some(task) = for_host
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        let admission = task.permit.admission.clone();
                        let _ = contain_agent_port_panic(&admission, || {
                            host.handle_work_observation_task(task)
                        });
                    }
                }) {
                    if let Some(task) = slot
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        task.refuse(SemanticRuntimePortFailure::Shutdown);
                    }
                }
            }))
        })
        .unwrap_or(false);
        if accepted || executed.load(Ordering::Acquire) {
            return WorkBrowserObservationDispatch::Scheduled;
        }
        let task = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match task.and_then(WorkObservationTask::rejected) {
            Some(request) => reject(request, ContextPortFailure::Shutdown),
            None => {
                self.admission.fail_invariant();
                WorkBrowserObservationDispatch::Scheduled
            }
        }
    }
}

#[cfg(test)]
#[path = "work_resource_port_tests.rs"]
mod tests;

#[path = "work_resource_navigation_port.rs"]
mod navigation;
pub(crate) use navigation::{WorkHistoryBackTask, WorkNavigationTask};

#[path = "work_resource_action_port.rs"]
mod action;
pub(crate) use action::WorkActionTask;

#[path = "work_resource_image_receipt.rs"]
mod image_receipt;
