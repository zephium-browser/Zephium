//! Private retained adapter; original Work ownership never enters the worker.

use super::*;
use std::task::Waker;
use zephium_agent_controller::{AgentWorkFailure, AgentWorkRetainedBrowser};

#[cfg(all(test, feature = "work-execution-probe"))]
#[path = "work_resources_controller_tests.rs"]
mod tests;

/// One immutable lease listener. It carries no terminal, registry or port.
pub(super) struct LeaseSignal {
    lease: WorkBrowserExecutionLease,
    retired: Arc<LeaseRetirement>,
    waker: Waker,
    pending: AtomicBool,
    failed: AtomicBool,
}
impl LeaseSignal {
    fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.retired.fail();
    }
    fn notify(&self) -> bool {
        if self.retired.is_retired() {
            return true;
        }
        if self.failed.load(Ordering::Acquire) {
            return false;
        }
        // Scalar publications coalesce before arbitrary wake code. A worker may
        // rearm while another publisher's wake is returning; Waker is Send+Sync,
        // so that legitimate overlap is not evidence of authority failure.
        if !self.pending.swap(true, Ordering::AcqRel)
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.waker.wake_by_ref()))
                .is_err()
        {
            self.fail();
        }
        !self.failed.load(Ordering::Acquire)
    }
    fn rearm(&self) -> Result<(), Refusal> {
        self.pending.swap(false, Ordering::AcqRel);
        if self.failed.load(Ordering::Acquire) {
            Err(Refusal::Uncertain)
        } else {
            Ok(())
        }
    }
}
impl Wake for LeaseSignal {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        // Native delivery owns this invocation and contains the panic. A failed
        // listener must quarantine, never be mistaken for returned notification.
        assert!(self.notify(), "retained lease notification failed");
    }
}

impl Notifications {
    pub(super) fn publish_actor_wakes(&self) {
        let mut listeners: [Option<Arc<LeaseSignal>>; MAX_LIVE_CONTEXTS] =
            std::array::from_fn(|_| None);
        match self.actors.lock() {
            Ok(actors) => {
                for (slot, actor) in listeners.iter_mut().zip(actors.iter()) {
                    *slot = actor.upgrade();
                }
            }
            Err(_) => {
                self.failed.store(true, Ordering::Release);
                return;
            }
        }
        // Neither invocation nor last-Waker drop occurs under the lane mutex.
        for listener in listeners.into_iter().flatten() {
            listener.notify();
        }
    }
}

pub(super) struct RetainedBrowser {
    browser: LeaseBrowser,
    binding: WorkBrowserReadBinding,
    listener: Option<Arc<LeaseSignal>>,
    read: Option<(PendingRead, SemanticObservationRequest)>,
    navigation: Option<PendingNavigation>,
    action: Option<PendingAction>,
    screenshot: bool,
    revoke: Option<PendingLifecycle>,
    delivered: bool,
}
impl WorkResourceOwner {
    pub(super) fn retained_browser(
        &self,
        lease: WorkBrowserExecutionLease,
        now: AgentPolicyInstant,
    ) -> Result<RetainedBrowser, Refusal> {
        let binding = self.shared.lock_rows()?.read_binding(&lease, now)?;
        let browser = self.browser(lease, now)?;
        Ok(RetainedBrowser {
            browser,
            binding,
            listener: None,
            read: None,
            navigation: None,
            action: None,
            screenshot: false,
            revoke: None,
            delivered: false,
        })
    }
}
impl RetainedBrowser {
    fn listener(&self) -> Result<&Arc<LeaseSignal>, Refusal> {
        self.listener
            .as_ref()
            .filter(|listener| listener.lease == self.browser.lease)
            .ok_or_else(|| self.browser.refusal())
    }
    fn rearm(&self) -> Result<(), Refusal> {
        self.listener()?.rearm()?;
        // The stable resource observer remains app-owned, but the active worker
        // also rearms it so an idle health failure cannot be coalesced forever.
        let health = self
            .browser
            .resource
            .lock_local(&self.browser.resource.health)?
            .poll();
        if health != WorkBrowserResourceHealthState::Current {
            return Err(self.browser.resource.refusal());
        }
        Ok(())
    }
    fn error(error: Refusal) -> AgentWorkFailure {
        match error {
            Refusal::NativeAdmission(failure) => AgentWorkFailure::Native(failure),
            _ => AgentWorkFailure::ContextLost,
        }
    }
}
// The constructor is private to the original retained adapter. Callers get
// only this narrow broker, never the underlying native port or registry.
struct RetainedImageSource {
    port: Arc<dyn AgentBrowserPort>,
    lease: WorkBrowserExecutionLease,
}
impl zephium_agentic::SemanticScreenshotSourcePort for RetainedImageSource {
    fn lease(&self) -> &WorkBrowserExecutionLease {
        &self.lease
    }
    fn public_candidate(&self, context: zephium_agentic::ContextJoin) -> bool {
        self.port
            .work_screenshot_public_candidate(&self.lease, context)
    }
    fn claim(
        &self,
        image: &zephium_agentic::SemanticScreenshot,
    ) -> Option<Arc<dyn zephium_agentic::SemanticScreenshotSource>> {
        self.port.claim_work_screenshot_source(&self.lease, image)
    }
}
impl AgentWorkRetainedBrowser for RetainedBrowser {
    fn screenshot_source_port(
        &self,
    ) -> Option<Arc<dyn zephium_agentic::SemanticScreenshotSourcePort>> {
        if self.revoke.is_some() || self.delivered || self.listener().is_err() {
            return None;
        }
        Some(Arc::new(RetainedImageSource {
            port: self.browser.shared.port.clone(),
            lease: self.browser.lease.clone(),
        }))
    }
    fn supports_screenshots(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
    }
    fn dispatch_screenshot(
        &mut self,
        request: SemanticScreenshotNativeRequest,
        completion: SemanticScreenshotNativeCompletion,
        now: AgentPolicyInstant,
    ) -> ContextDispatch {
        if !self.supports_screenshots()
            || self.listener().is_err()
            || self.check_health(now).is_err()
            || self.read.is_some()
            || self.navigation.is_some()
            || self.action.is_some()
            || self.screenshot
            || self.revoke.is_some()
            || self.delivered
        {
            return ContextDispatch::Rejected(ContextPortFailure::Stale);
        }
        let dispatch = self
            .browser
            .shared
            .port
            .capture_semantic_screenshot(request, completion);
        self.screenshot = matches!(dispatch, ContextDispatch::Scheduled);
        dispatch
    }
    fn account_screenshot_terminal(
        &mut self,
        _now: AgentPolicyInstant,
    ) -> Result<(), AgentWorkFailure> {
        if !self.screenshot {
            return Err(crate::contract_failure!());
        }
        self.screenshot = false;
        // This is original callback accounting, not new browser authority.
        // It must remain possible after cancellation revoked the lease.
        Ok(())
    }
    fn binding(&self) -> &WorkBrowserReadBinding {
        &self.binding
    }
    fn register_listener(&mut self, waker: Waker) -> Result<(), AgentWorkFailure> {
        if self.listener.is_some()
            || self.read.is_some()
            || self.navigation.is_some()
            || self.action.is_some()
            || self.screenshot
            || self.revoke.is_some()
        {
            return Err(Self::error(self.browser.refusal()));
        }
        self.browser.retired.check_active().map_err(Self::error)?;
        let listener = Arc::new(LeaseSignal {
            lease: self.browser.lease.clone(),
            retired: self.browser.retired.clone(),
            waker,
            pending: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        {
            let mut actors = self
                .browser
                .shared
                .notifications
                .actors
                .lock()
                .map_err(|_| Self::error(self.browser.shared.refusal()))?;
            actors.retain(|actor| actor.strong_count() != 0);
            if actors.len() >= MAX_LIVE_CONTEXTS {
                return Err(Self::error(self.browser.refusal()));
            }
            actors.push(Arc::downgrade(&listener));
        }
        self.listener = Some(listener);
        self.rearm().map_err(Self::error)?;
        self.browser
            .shared
            .current(&self.browser.resource)
            .map_err(Self::error)
    }
    fn check_health(&self, now: AgentPolicyInstant) -> Result<(), AgentWorkFailure> {
        // Construction may inspect before worker registration; every dispatch
        // below independently requires that immutable listener to exist.
        if self.listener.is_some() {
            self.rearm().map_err(Self::error)?;
        }
        self.browser.health(now).map_err(Self::error)
    }
    fn supports_navigation(&self) -> bool {
        true
    }
    fn supports_history_back(&self) -> bool {
        self.browser
            .shared
            .port
            .supports_work_resource_history_back()
    }
    fn supports_actions(&self) -> bool {
        true
    }
    fn dispatch_action(
        &mut self,
        request: SemanticActionNativeRequest,
        now: AgentPolicyInstant,
    ) -> ContextDispatch {
        if self.listener().is_err()
            || self.check_health(now).is_err()
            || self.read.is_some()
            || self.navigation.is_some()
            || self.action.is_some()
            || self.screenshot
            || self.revoke.is_some()
            || self.delivered
        {
            return ContextDispatch::Rejected(ContextPortFailure::Stale);
        }
        match PendingAction::dispatch(&self.browser, request, now) {
            Ok((pending, dispatch)) => {
                if !pending.finished() {
                    self.action = Some(pending);
                }
                dispatch
            }
            Err(Refusal::NativeAdmission(failure)) => ContextDispatch::Rejected(failure),
            Err(_) => ContextDispatch::Rejected(ContextPortFailure::NativeRefused),
        }
    }
    fn poll_action(
        &mut self,
        now: AgentPolicyInstant,
    ) -> Result<Option<SemanticActionNativeSettlement>, AgentWorkFailure> {
        self.listener()
            .map_err(Self::error)?
            .rearm()
            .map_err(Self::error)?;
        let pending = self
            .action
            .as_mut()
            .ok_or_else(|| crate::contract_failure!())?;
        let Some(event) = pending.poll(now).map_err(Self::error)? else {
            return Ok(None);
        };
        self.action.take();
        Ok(Some(event.into_terminal()))
    }
    fn automation_state(
        &self,
        now: AgentPolicyInstant,
    ) -> Result<ContextAutomationState, AgentWorkFailure> {
        self.check_health(now)?;
        self.browser
            .shared
            .lock_rows()
            .map_err(Self::error)?
            .automation_state(&self.browser.lease, now)
            .map_err(Refusal::from)
            .map_err(Self::error)
    }
    fn prepare_navigation(
        &mut self,
        source: ContextJoin,
        now: AgentPolicyInstant,
    ) -> Result<ContextOperationJoin, AgentWorkFailure> {
        self.listener().map_err(Self::error)?;
        self.check_health(now)?;
        if self.read.is_some()
            || self.navigation.is_some()
            || self.action.is_some()
            || self.screenshot
            || self.revoke.is_some()
            || self.delivered
        {
            return Err(crate::contract_failure!());
        }
        let (pending, operation) =
            PendingNavigation::prepare(&self.browser, source, now).map_err(Self::error)?;
        self.navigation = Some(pending);
        Ok(operation)
    }
    fn cancel_navigation_preparation(&mut self) -> Result<(), AgentWorkFailure> {
        let pending = self
            .navigation
            .as_mut()
            .ok_or_else(|| crate::contract_failure!())?;
        pending.cancel_preparation().map_err(Self::error)?;
        self.navigation.take();
        Ok(())
    }
    fn dispatch_navigation(&mut self, active: &AgentActiveNavigation) -> ContextDispatch {
        let Some(pending) = self.navigation.as_mut() else {
            self.browser.refusal();
            return ContextDispatch::Rejected(ContextPortFailure::NativeRefused);
        };
        let result = pending.dispatch(active);
        if pending.finished() {
            self.navigation.take();
        }
        result
    }
    fn dispatch_history_back(&mut self, active: &AgentActiveNavigation) -> ContextDispatch {
        let Some(pending) = self.navigation.as_mut() else {
            self.browser.refusal();
            return ContextDispatch::Rejected(ContextPortFailure::NativeRefused);
        };
        let result = pending.dispatch_history_back(active);
        if pending.finished() {
            self.navigation.take();
        }
        result
    }
    fn poll_navigation(
        &mut self,
        now: AgentPolicyInstant,
    ) -> Result<Option<ContextNavigationSettlement>, AgentWorkFailure> {
        // Health may already be revoked. Reconcile the exact original receipt,
        // never issue new work or require live execution authority for cleanup.
        self.listener()
            .map_err(Self::error)?
            .rearm()
            .map_err(Self::error)?;
        let pending = self
            .navigation
            .as_mut()
            .ok_or_else(|| crate::contract_failure!())?;
        let Some(event) = pending.poll(now).map_err(Self::error)? else {
            return Ok(None);
        };
        self.navigation.take();
        if event.is_current() {
            match self.browser.shared.lock_rows().and_then(|mut rows| {
                rows.read_binding(&self.browser.lease, now)
                    .map_err(Into::into)
            }) {
                Ok(binding) => self.binding = binding,
                Err(_) => {
                    self.browser.refusal();
                }
            }
        }
        Ok(Some(event.into_terminal()))
    }
    fn begin_observation(&mut self, now: AgentPolicyInstant) -> Result<(), AgentWorkFailure> {
        self.begin_observation_with_capability(WorkBrowserObservationCapability::RESTRICTED, now)
    }
    fn begin_observation_with_capability(
        &mut self,
        capability: WorkBrowserObservationCapability,
        now: AgentPolicyInstant,
    ) -> Result<(), AgentWorkFailure> {
        self.listener().map_err(Self::error)?;
        self.check_health(now)?;
        if self.read.is_some()
            || self.navigation.is_some()
            || self.action.is_some()
            || self.revoke.is_some()
            || self.delivered
        {
            return Err(crate::contract_failure!());
        }
        let request = self
            .browser
            .shared
            .lock_rows()
            .map_err(Self::error)?
            .observe_initial_with_capability(&self.browser.lease, capability, now)
            .map_err(Refusal::from)
            .map_err(Self::error)?;
        let observation = request.observation().clone();
        let pending = PendingRead::dispatch(
            self.browser.shared.clone(),
            self.browser.resource.clone(),
            request,
        )
        .map_err(Self::error)?;
        self.read = Some((pending, observation));
        Ok(())
    }
    fn supports_expansion(&self) -> bool {
        true
    }
    fn begin_expansion(
        &mut self,
        previous: &SemanticObservation,
        acknowledgement: &SemanticObservationAcknowledgement,
        target: SemanticReferenceId,
        kind: SemanticExpansionKind,
        now: AgentPolicyInstant,
    ) -> Result<(), AgentWorkFailure> {
        self.begin_expansion_with_capability(
            previous,
            acknowledgement,
            target,
            kind,
            WorkBrowserObservationCapability::RESTRICTED,
            now,
        )
    }
    fn begin_expansion_with_capability(
        &mut self,
        previous: &SemanticObservation,
        acknowledgement: &SemanticObservationAcknowledgement,
        target: SemanticReferenceId,
        kind: SemanticExpansionKind,
        capability: WorkBrowserObservationCapability,
        now: AgentPolicyInstant,
    ) -> Result<(), AgentWorkFailure> {
        self.listener().map_err(Self::error)?;
        self.check_health(now)?;
        if self.read.is_some()
            || self.navigation.is_some()
            || self.action.is_some()
            || self.revoke.is_some()
            || self.delivered
        {
            return Err(crate::contract_failure!());
        }
        let request = self
            .browser
            .shared
            .lock_rows()
            .map_err(Self::error)?
            .observe_expansion_with_capability(
                &self.browser.lease,
                previous,
                acknowledgement,
                target,
                kind,
                capability,
                now,
            )
            .map_err(Refusal::from)
            .map_err(Self::error)?;
        let observation = request.observation().clone();
        let pending = PendingRead::dispatch(
            self.browser.shared.clone(),
            self.browser.resource.clone(),
            request,
        )
        .map_err(Self::error)?;
        self.read = Some((pending, observation));
        Ok(())
    }
    fn poll_observation(
        &mut self,
        now: AgentPolicyInstant,
    ) -> Result<Option<SemanticObservation>, AgentWorkFailure> {
        self.rearm().map_err(Self::error)?;
        let (pending, _) = self
            .read
            .as_mut()
            .ok_or_else(|| crate::contract_failure!())?;
        let Some(event) = pending.poll(now).map_err(Self::error)? else {
            return Ok(None);
        };
        let (_, request) = self.read.take().ok_or_else(|| crate::contract_failure!())?;
        let snapshot = match event {
            WorkBrowserObservationEvent::Snapshot(snapshot) => *snapshot,
            WorkBrowserObservationEvent::Refused(failure) => {
                if request.scope().anchor().is_some()
                    && failure
                        == SemanticRuntimePortFailure::Result(SemanticRuntimeResultError::Runtime(
                            SemanticRuntimeFault::AnchorMissing,
                        ))
                {
                    // The exact callback has already been drained. Recheck the
                    // original resource binding before classifying anchor churn;
                    // navigation, account takeover and lease loss stay terminal.
                    self.check_health(now)?;
                    let current = self
                        .browser
                        .shared
                        .lock_rows()
                        .map_err(Self::error)?
                        .read_binding(&self.browser.lease, now)
                        .map_err(Refusal::from)
                        .map_err(Self::error)?;
                    if current.lease() != self.binding.lease()
                        || current.frame() != self.binding.frame()
                        || current.document() != self.binding.document()
                        || current.current_requested_document()
                            != self.binding.current_requested_document()
                    {
                        return Err(AgentWorkFailure::ContextLost);
                    }
                    return Err(AgentWorkFailure::InspectionAnchorLost);
                }
                return Err(AgentWorkFailure::Observation(failure));
            }
            WorkBrowserObservationEvent::DebtSettled => return Err(AgentWorkFailure::ContextLost),
        };
        if snapshot.frame() != self.binding.frame() {
            return Err(AgentWorkFailure::ContextLost);
        }
        let boundaries = snapshot
            .nodes()
            .iter()
            .filter(|node| node.role() == SemanticRole::FrameBoundary)
            .map(SemanticNode::reference)
            .collect::<Vec<_>>();
        let mut assembler = SemanticObservationAssembler::new(request, snapshot)
            .map_err(|_| AgentWorkFailure::Context)?;
        for boundary in boundaries {
            assembler
                .mark_frame_unsupported(
                    FrameId::MAIN,
                    boundary,
                    SemanticFrameUnsupported::PolicyBlocked,
                )
                .map_err(|_| AgentWorkFailure::Context)?;
        }
        let observation = assembler.finish().map_err(|_| AgentWorkFailure::Context)?;
        self.check_health(now)?;
        Ok(Some(observation))
    }
    fn begin_revocation(&mut self) -> Result<(), AgentWorkFailure> {
        if self.revoke.is_some() || self.delivered {
            return Ok(());
        }
        let listener = self.listener().map_err(Self::error)?.clone();
        let (request, mut ticket) = self
            .browser
            .shared
            .lock_rows()
            .map_err(Self::error)?
            .revoke_with_delivery(&self.browser.lease)
            .map_err(Refusal::from)
            .map_err(Self::error)?;
        // Original ticket is registered BEFORE dispatch. Registration never
        // runs the wake and does not manufacture a physical delivery receipt.
        ticket
            .register_waker(listener.into())
            .map_err(|_| Self::error(self.browser.refusal()))?;
        self.revoke = Some(
            PendingLifecycle::dispatch(
                self.browser.shared.clone(),
                self.browser.resource.clone(),
                request,
                Some(ticket),
                Some(self.browser.retired.clone()),
            )
            .map_err(Self::error)?,
        );
        Ok(())
    }
    fn poll_revocation(
        &mut self,
        now: AgentPolicyInstant,
    ) -> Result<Option<WorkBrowserLeaseDeliveryProof>, AgentWorkFailure> {
        self.listener()
            .map_err(Self::error)?
            .rearm()
            .map_err(Self::error)?;
        // The scoped controller must first consume the exact navigation event
        // and reconcile its independent policy/audit receipt. Never silently
        // drain navigation here just to make the resource lease look closed.
        if self.navigation.is_some() || self.action.is_some() {
            return Ok(None);
        }
        if self.screenshot {
            return Ok(None);
        }
        if let Some((pending, _)) = self.read.as_mut() {
            match pending.poll(now) {
                Ok(None) => return Ok(None),
                Ok(Some(_)) | Err(_) => {
                    self.read.take();
                }
            }
        }
        // Physical receipt may be consumed during notifier Running. Scoped
        // actor drain is not notifier completion or successor admission: the
        // original native reservation and application health still gate those.
        let pending = self
            .revoke
            .as_mut()
            .ok_or_else(|| crate::contract_failure!())?;
        match pending.poll(now).map_err(Self::error)? {
            None => Ok(None),
            Some(LifecycleResult::Delivered(proof)) => {
                self.delivered = true;
                self.revoke.take();
                Ok(Some(proof))
            }
            Some(LifecycleResult::Event(_)) => Err(AgentWorkFailure::ContextLost),
        }
    }
}
