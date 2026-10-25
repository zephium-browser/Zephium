#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! Bounded production ingress for the native agent-browser context adapter.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use zephium_agentic::{
    AgentBrowserPort, ContextCancellationSettlement, ContextConstructionSettlement,
    ContextCookieTransferFailure, ContextCookieTransferOutcome, ContextCookieTransferRequest,
    ContextCookieTransferSettlement, ContextDispatch, ContextNativeEvent, ContextNativeRequest,
    ContextNativeResourceSnapshot, ContextPortFailure, ContextResourceAuditId,
    ContextResourceAuditSettlement, ContextShutdownAuditSettlement, ContextShutdownDispatch,
    SemanticActionNativeCompletion, SemanticActionNativeRequest, SemanticRuntimeCorrelation,
    SemanticRuntimeInvocation, SemanticRuntimePortFailure, SemanticRuntimeSettlement,
    SemanticScreenshotNativeCompletion, SemanticScreenshotNativeRequest,
    MAX_PENDING_NATIVE_CONTEXT_TASKS, MAX_PENDING_SEMANTIC_SCREENSHOTS,
};
use zephium_agentic::{ContextJoin, ContextOperationKind};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use zephium_agentic::{ContextNavigationReplacement, ContextNavigationTarget, ContextRendererLoss};
#[cfg(target_os = "macos")]
use zephium_agentic::{SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure};

#[cfg(target_os = "macos")]
use zephium_agentic::{
    SemanticActionExecutionInstant, SemanticActionNativeFailure, SemanticActionNativeSettlement,
};

use crate::MainThreadDispatch;

#[cfg(all(target_os = "macos", feature = "native-agentic-foreground-probe"))]
#[path = "agent_foreground_probe_port.rs"]
mod foreground_probe;
#[cfg(all(target_os = "macos", feature = "native-agentic-foreground-probe"))]
pub(crate) use foreground_probe::AgentForegroundProbeTask;
#[cfg(all(target_os = "macos", feature = "native-agentic-work-resource-probe"))]
#[path = "agent_work_resource_probe_port.rs"]
pub(crate) mod resource_witness;
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[path = "work_resource_port.rs"]
mod work_resource;
#[cfg(all(
    any(target_os = "macos", target_os = "windows"),
    feature = "native-agentic-work-lifetime-diagnostic"
))]
#[path = "work_resource_failure_diagnostic.rs"]
pub(crate) mod work_resource_failure_diagnostic;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use work_resource::{work_browser_monotonic_deadline, work_browser_monotonic_now};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn work_browser_monotonic_now() -> Option<zephium_agentic::AgentPolicyInstant> {
    static WORK_CLOCK_ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    u64::try_from(
        WORK_CLOCK_ORIGIN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_millis(),
    )
    .ok()
    .map(zephium_agentic::AgentPolicyInstant::from_millis)
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn work_browser_monotonic_deadline(
    deadline: std::time::Instant,
) -> Option<zephium_agentic::AgentPolicyInstant> {
    static WORK_CLOCK_ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let origin = *WORK_CLOCK_ORIGIN.get_or_init(std::time::Instant::now);
    let nanos = deadline.checked_duration_since(origin)?.as_nanos();
    u64::try_from(nanos.div_ceil(1_000_000))
        .ok()
        .map(zephium_agentic::AgentPolicyInstant::from_millis)
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) use work_resource::{
    WorkActionTask, WorkHistoryBackTask, WorkLifecycleTask, WorkNavigationTask,
    WorkNotificationPermit, WorkObservationTask, WorkResourceGuard,
};

/// Where the last action ran out of time: a closed stage, never page content,
/// so a failed page task can name what it was waiting for.
static ACTION_TIMEOUT_STAGE: std::sync::Mutex<&'static str> = std::sync::Mutex::new("");

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) fn note_action_timeout_stage(stage: &'static str) {
    *ACTION_TIMEOUT_STAGE
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = stage;
}

/// The stage the most recent action deadline expired in.
pub fn last_action_timeout_stage() -> &'static str {
    *ACTION_TIMEOUT_STAGE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub(crate) type AgentContextEventSink = Arc<dyn Fn(ContextNativeEvent) + Send + Sync>;

/// Cloneable fail-stop authority retained by bounded native callbacks.
///
/// A callback rejected during ordinary operation means an accepted native
/// owner can no longer rejoin the shell. Shutdown sealing is different: the
/// host teardown path still owns and terminally drops every retained task.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone)]
pub(crate) struct AgentContextCallbackGuard {
    admission: Arc<AgentPortAdmission>,
    sink: AgentContextEventSink,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl AgentContextCallbackGuard {
    pub(crate) fn callback_dispatch_rejected(&self) {
        let sealed = match self.admission.state.lock() {
            Ok(state) => state.sealed,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.admission.report_fatal_once();
                return;
            }
        };
        if !sealed {
            self.admission.fail_invariant();
        }
    }

    pub(crate) fn emit_renderer_lost(&self, prior: ContextJoin) {
        if self.admission.pending().is_none() {
            return;
        }
        emit_event(
            &self.sink,
            &self.admission,
            ContextNativeEvent::RendererLost(ContextRendererLoss::new(prior)),
        );
    }

    pub(crate) fn emit_navigation_replaced(
        &self,
        prior: ContextJoin,
        target: ContextNavigationTarget,
    ) {
        if self.admission.pending().is_none() {
            return;
        }
        emit_event(
            &self.sink,
            &self.admission,
            ContextNativeEvent::NavigationReplaced(ContextNavigationReplacement::new(
                prior, target,
            )),
        );
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub(crate) struct AgentScreenshotCallbackGuard {
    admission: Arc<AgentPortAdmission>,
}

#[cfg(target_os = "macos")]
impl AgentScreenshotCallbackGuard {
    pub(crate) fn callback_dispatch_rejected(&self) {
        let sealed = match self.admission.state.lock() {
            Ok(state) => state.sealed,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.admission.report_fatal_once();
                return;
            }
        };
        if !sealed {
            self.admission.fail_invariant();
        }
    }
}

#[derive(Default)]
struct AgentPortAdmissionState {
    pending: usize,
    physical_screenshots: usize,
    sealed: bool,
    invariant_failed: bool,
    native_shutdown_verified: bool,
    retired: bool,
}

struct AgentPortAdmission {
    group: Option<std::sync::Weak<AgentPortGroup>>,
    state: Mutex<AgentPortAdmissionState>,
    fatal: Arc<dyn Fn(&'static str) + Send + Sync>,
    fatal_reported: AtomicBool,
    lineage_failed: Option<Arc<AtomicBool>>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    work: Mutex<work_resource::WorkIngress>,
}

impl AgentPortAdmission {
    fn new(fatal: Arc<dyn Fn(&'static str) + Send + Sync>) -> Self {
        Self {
            group: None,
            state: Mutex::new(AgentPortAdmissionState::default()),
            fatal,
            fatal_reported: AtomicBool::new(false),
            lineage_failed: None,
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            work: Mutex::new(work_resource::WorkIngress::default()),
        }
    }

    fn lineage_failed(&self) -> bool {
        self.lineage_failed
            .as_ref()
            .is_some_and(|failed| failed.load(Ordering::Acquire))
    }

    fn reserve(self: &Arc<Self>) -> Result<AgentTaskPermit, ContextPortFailure> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                return Err(ContextPortFailure::Shutdown);
            }
        };
        if state.sealed
            || state.invariant_failed
            || self.lineage_failed()
            || self.group.as_ref().is_some_and(|group| {
                group
                    .upgrade()
                    .is_none_or(|group| group.sealed.load(Ordering::Acquire))
            })
        {
            return Err(ContextPortFailure::Shutdown);
        }
        if state.pending >= MAX_PENDING_NATIVE_CONTEXT_TASKS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        state.pending += 1;
        Ok(AgentTaskPermit {
            admission: Arc::clone(self),
            released: false,
        })
    }

    /// Reserves a read-only audit without reopening mutation admission.
    fn reserve_audit(self: &Arc<Self>) -> Result<AgentTaskPermit, ContextPortFailure> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                return Err(ContextPortFailure::Shutdown);
            }
        };
        if state.invariant_failed || state.retired || self.lineage_failed() {
            return Err(ContextPortFailure::Shutdown);
        }
        if state.pending >= MAX_PENDING_NATIVE_CONTEXT_TASKS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        state.pending += 1;
        Ok(AgentTaskPermit {
            admission: Arc::clone(self),
            released: false,
        })
    }

    /// Linearizes the permanent mutation seal with one shutdown-audit slot.
    fn reserve_shutdown_audit(self: &Arc<Self>) -> Result<AgentTaskPermit, ContextPortFailure> {
        if let Some(group) = self.group.as_ref().and_then(std::sync::Weak::upgrade) {
            group.sealed.store(true, Ordering::Release);
        }
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                return Err(ContextPortFailure::Shutdown);
            }
        };
        if state.sealed || state.invariant_failed || self.lineage_failed() {
            return Err(ContextPortFailure::Shutdown);
        }
        // Seal before checking capacity: even a full queue must not leave a
        // post-barrier mutation race. A later bounded ordinary audit may
        // observe drain once one retained task releases its permit.
        state.sealed = true;
        if state.pending >= MAX_PENDING_NATIVE_CONTEXT_TASKS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        state.pending += 1;
        Ok(AgentTaskPermit {
            admission: Arc::clone(self),
            released: false,
        })
    }

    #[cfg(any(target_os = "macos", test))]
    fn reserve_screenshot(
        self: &Arc<Self>,
    ) -> Result<(AgentTaskPermit, AgentScreenshotPhysicalPermit), ContextPortFailure> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                return Err(ContextPortFailure::Shutdown);
            }
        };
        if state.sealed
            || state.invariant_failed
            || self.lineage_failed()
            || self.group.as_ref().is_some_and(|group| {
                group
                    .upgrade()
                    .is_none_or(|group| group.sealed.load(Ordering::Acquire))
            })
        {
            return Err(ContextPortFailure::Shutdown);
        }
        if state.pending >= MAX_PENDING_NATIVE_CONTEXT_TASKS
            || state.physical_screenshots >= MAX_PENDING_SEMANTIC_SCREENSHOTS
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        state.pending += 1;
        state.physical_screenshots += 1;
        Ok((
            AgentTaskPermit {
                admission: Arc::clone(self),
                released: false,
            },
            AgentScreenshotPhysicalPermit {
                admission: Arc::clone(self),
                released: false,
            },
        ))
    }

    fn release(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                return;
            }
        };
        let Some(pending) = state.pending.checked_sub(1) else {
            state.invariant_failed = true;
            state.sealed = true;
            drop(state);
            self.fail_invariant();
            return;
        };
        state.pending = pending;
    }

    #[cfg(any(target_os = "macos", test))]
    fn release_screenshot(&self) {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                return;
            }
        };
        let Some(physical_screenshots) = state.physical_screenshots.checked_sub(1) else {
            state.invariant_failed = true;
            state.sealed = true;
            drop(state);
            self.fail_invariant();
            return;
        };
        state.physical_screenshots = physical_screenshots;
    }

    fn counts(&self) -> Option<(usize, usize)> {
        match self.state.lock() {
            Ok(state) => (!state.invariant_failed
                && !state.retired
                && !self.lineage_failed()
                && state.pending <= MAX_PENDING_NATIVE_CONTEXT_TASKS
                && state.physical_screenshots <= MAX_PENDING_SEMANTIC_SCREENSHOTS)
                .then_some((state.pending, state.physical_screenshots)),
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
                None
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", test))]
    fn pending(&self) -> Option<usize> {
        self.counts().map(|(pending, _)| pending)
    }

    fn seal(&self) {
        match self.state.lock() {
            Ok(mut state) => state.sealed = true,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.invariant_failed = true;
                state.sealed = true;
                drop(state);
                self.report_fatal_once();
            }
        }
    }

    // Only the exact native shutdown task may attest its own sealed cohort.
    // Its permit is still held; every other request/capture must be absent.
    fn verify_native_shutdown(&self, snapshot: ContextNativeResourceSnapshot) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if !self.work_is_absent() {
            return;
        }
        let Ok(mut state) = self.state.lock() else {
            self.report_fatal_once();
            return;
        };
        if state.sealed
            && !state.invariant_failed
            && !state.retired
            && !self.lineage_failed()
            && state.pending == 1
            && state.physical_screenshots == 0
            && Self::native_resources_are_empty(snapshot)
        {
            state.native_shutdown_verified = true;
        }
    }

    fn native_resources_are_empty(snapshot: ContextNativeResourceSnapshot) -> bool {
        matches!(
            snapshot.counts(),
            zephium_agentic::ContextNativeResourceCounts {
                known_bindings: 0,
                resident_views: 0,
                owned_reservations: 0,
                borrowed_leases: 0,
                visible_surfaces: 0,
                suspended_views: 0,
                pending_operations: 0,
                pending_captures: 0,
                queued_tasks: 0,
            }
        )
    }

    // A later failed/lost/nonempty read-only audit cannot certify retirement
    // using an earlier empty receipt. Invalidate before releasing its permit;
    // ordinary audit retries may reconcile debt but never mint a new receipt.
    fn invalidate_retirement_audit(&self) {
        match self.state.lock() {
            Ok(mut state) => state.native_shutdown_verified = false,
            Err(poisoned) => {
                drop(poisoned.into_inner());
                self.report_fatal_once();
            }
        }
    }

    // Linearizes retirement with old read-only audit admission. An old port
    // never unseals, changes its sink, or audits a successor's native cohort.
    fn retire_for_successor(&self) -> Result<(), ContextPortFailure> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if !self.work_is_absent() {
            return Err(ContextPortFailure::ProfileBusy);
        }
        let mut state = self.state.lock().map_err(|_| {
            if let Some(failed) = &self.lineage_failed {
                failed.store(true, Ordering::Release);
            }
            ContextPortFailure::Shutdown
        })?;
        if state.invariant_failed || self.lineage_failed() || state.retired {
            return Err(ContextPortFailure::Shutdown);
        }
        if !state.sealed
            || !state.native_shutdown_verified
            || state.pending != 0
            || state.physical_screenshots != 0
        {
            return Err(ContextPortFailure::ProfileBusy);
        }
        state.retired = true;
        Ok(())
    }

    fn fail_invariant(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.invariant_failed = true;
            state.sealed = true;
        }
        self.report_fatal_once();
    }

    fn report_fatal_once(&self) {
        if let Some(failed) = &self.lineage_failed {
            failed.store(true, Ordering::Release);
        }
        if !self.fatal_reported.swap(true, Ordering::AcqRel) {
            // This callback can be reached from native navigation delegates,
            // timeout handlers, and `AgentContextTask::drop`. A consumer panic
            // must not cross an Objective-C/libdispatch boundary or trigger a
            // second panic during unwinding; the admission seal above remains
            // the authoritative fail-stop state even if reporting misbehaves.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.fatal)("agent-context port admission invariant failed");
            }));
        }
    }
}

struct AgentTaskPermit {
    admission: Arc<AgentPortAdmission>,
    released: bool,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) struct AgentScreenshotPhysicalPermit {
    admission: Arc<AgentPortAdmission>,
    released: bool,
}

#[cfg(any(target_os = "macos", test))]
impl Drop for AgentScreenshotPhysicalPermit {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            self.admission.release_screenshot();
        }
    }
}

impl AgentTaskPermit {
    fn counts(&self) -> Option<(usize, usize)> {
        self.admission.counts()
    }

    fn release(&mut self) {
        if self.released {
            self.admission.fail_invariant();
            return;
        }
        self.released = true;
        self.admission.release();
    }
}

impl Drop for AgentTaskPermit {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            self.admission.release();
        }
    }
}

enum AgentPendingRequest {
    Native(ContextNativeRequest),
    Cookie(ContextCookieTransferRequest),
    Audit(ContextResourceAuditId),
    ShutdownAudit(ContextResourceAuditId),
    Semantic(AgentPendingSemantic),
}

struct AgentPendingSemantic {
    correlation: SemanticRuntimeCorrelation,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    invocation: Option<SemanticRuntimeInvocation>,
}

pub(crate) struct AgentContextTask {
    request: Option<AgentPendingRequest>,
    admitted_at: std::time::Instant,
    permit: AgentTaskPermit,
    sink: AgentContextEventSink,
}

/// Move-only native capture task sharing the context port's global admission.
#[cfg(target_os = "macos")]
pub(crate) struct AgentScreenshotTask {
    request: Option<SemanticScreenshotNativeRequest>,
    completion: Option<SemanticScreenshotNativeCompletion>,
    physical: Option<AgentScreenshotPhysicalPermit>,
    admitted_at: std::time::Instant,
    permit: AgentTaskPermit,
}

/// Move-only semantic action task sharing the context port's global admission.
#[cfg(target_os = "macos")]
pub(crate) struct AgentActionTask {
    request: Option<SemanticActionNativeRequest>,
    completion: Option<SemanticActionNativeCompletion>,
    admitted_at: std::time::Instant,
    permit: AgentTaskPermit,
}

#[cfg(target_os = "macos")]
impl AgentActionTask {
    fn new(
        request: SemanticActionNativeRequest,
        completion: SemanticActionNativeCompletion,
        permit: AgentTaskPermit,
    ) -> Self {
        Self {
            request: Some(request),
            completion: Some(completion),
            admitted_at: std::time::Instant::now(),
            permit,
        }
    }

    pub(crate) fn request(&self) -> Option<&SemanticActionNativeRequest> {
        self.request.as_ref()
    }

    pub(crate) fn take_request(&mut self) -> Option<SemanticActionNativeRequest> {
        self.request.take()
    }

    pub(crate) const fn admitted_at(&self) -> std::time::Instant {
        self.admitted_at
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn callback_guard(&self) -> AgentScreenshotCallbackGuard {
        AgentScreenshotCallbackGuard {
            admission: self.permit.admission.clone(),
        }
    }

    pub(crate) fn complete(mut self, settlement: SemanticActionNativeSettlement) {
        self.request = None;
        self.permit.release();
        self.invoke_completion(settlement);
    }

    pub(crate) fn refuse(self, failure: SemanticActionNativeFailure) {
        let completed_at = self.failure_instant(failure);
        let mut task = self;
        let Some(request) = task.request.take() else {
            task.completion = None;
            task.permit.release();
            task.permit.admission.fail_invariant();
            return;
        };
        let settlement = request.fail(failure, completed_at);
        task.permit.release();
        task.invoke_completion(settlement);
    }

    fn failure_instant(
        &self,
        failure: SemanticActionNativeFailure,
    ) -> SemanticActionExecutionInstant {
        let Some(request) = self.request.as_ref() else {
            return SemanticActionExecutionInstant::from_millis(0);
        };
        if failure == SemanticActionNativeFailure::TimedOut {
            return request.deadline();
        }
        let elapsed = u64::try_from(self.admitted_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        SemanticActionExecutionInstant::from_millis(
            request.requested_at().millis().saturating_add(elapsed),
        )
    }

    fn cancel_without_completion(mut self) {
        self.request = None;
        self.completion = None;
        self.permit.release();
    }

    fn invoke_completion(&mut self, settlement: SemanticActionNativeSettlement) {
        let Some(completion) = self.completion.take() else {
            self.permit.admission.fail_invariant();
            return;
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| completion(settlement)))
            .is_err()
        {
            self.permit.admission.fail_invariant();
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for AgentActionTask {
    fn drop(&mut self) {
        let Some(request) = self.request.take() else {
            if self.completion.take().is_some() {
                self.permit.release();
                self.permit.admission.fail_invariant();
            }
            return;
        };
        let elapsed = u64::try_from(self.admitted_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let completed_at = SemanticActionExecutionInstant::from_millis(
            request.requested_at().millis().saturating_add(elapsed),
        );
        let settlement = request.fail(SemanticActionNativeFailure::Transport, completed_at);
        self.permit.release();
        self.invoke_completion(settlement);
    }
}

#[cfg(target_os = "macos")]
impl AgentScreenshotTask {
    fn new(
        request: SemanticScreenshotNativeRequest,
        completion: SemanticScreenshotNativeCompletion,
        physical: AgentScreenshotPhysicalPermit,
        permit: AgentTaskPermit,
    ) -> Self {
        Self {
            request: Some(request),
            completion: Some(completion),
            physical: Some(physical),
            admitted_at: std::time::Instant::now(),
            permit,
        }
    }

    pub(crate) fn request(&self) -> Option<&SemanticScreenshotNativeRequest> {
        self.request.as_ref()
    }

    pub(crate) fn take_request(&mut self) -> Option<SemanticScreenshotNativeRequest> {
        self.request.take()
    }

    pub(crate) fn take_physical(&mut self) -> Option<AgentScreenshotPhysicalPermit> {
        self.physical.take()
    }

    pub(crate) const fn admitted_at(&self) -> std::time::Instant {
        self.admitted_at
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn callback_guard(&self) -> AgentScreenshotCallbackGuard {
        AgentScreenshotCallbackGuard {
            admission: self.permit.admission.clone(),
        }
    }

    pub(crate) fn complete(
        mut self,
        outcome: Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>,
    ) {
        self.request = None;
        self.physical = None;
        self.permit.release();
        self.invoke_completion(outcome);
    }

    pub(crate) fn refuse(self, failure: SemanticScreenshotNativeFailure) {
        self.complete(Err(failure));
    }

    fn cancel_without_completion(mut self) {
        self.request = None;
        self.completion = None;
        self.physical = None;
        self.permit.release();
    }

    fn invoke_completion(
        &mut self,
        outcome: Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>,
    ) {
        let Some(completion) = self.completion.take() else {
            self.permit.admission.fail_invariant();
            return;
        };
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| completion(outcome))).is_err() {
            self.permit.admission.fail_invariant();
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for AgentScreenshotTask {
    fn drop(&mut self) {
        if self.request.take().is_none() && self.completion.is_none() {
            return;
        }
        self.permit.release();
        self.invoke_completion(Err(SemanticScreenshotNativeFailure::Transport));
    }
}

impl AgentContextTask {
    fn new(
        request: AgentPendingRequest,
        permit: AgentTaskPermit,
        sink: AgentContextEventSink,
    ) -> Self {
        Self {
            request: Some(request),
            admitted_at: std::time::Instant::now(),
            permit,
            sink,
        }
    }

    pub(crate) fn request(&self) -> Option<&ContextNativeRequest> {
        match self.request.as_ref() {
            Some(AgentPendingRequest::Native(request)) => Some(request),
            _ => None,
        }
    }

    pub(crate) fn audit(&self) -> Option<ContextResourceAuditId> {
        match self.request.as_ref() {
            Some(AgentPendingRequest::Audit(audit) | AgentPendingRequest::ShutdownAudit(audit)) => {
                Some(*audit)
            }
            _ => None,
        }
    }

    /// Exact cookie request and native-clock anchor captured after admission.
    pub(crate) fn cookie(&self) -> Option<(&ContextCookieTransferRequest, std::time::Instant)> {
        match self.request.as_ref() {
            Some(AgentPendingRequest::Cookie(request)) => Some((request, self.admitted_at)),
            _ => None,
        }
    }

    pub(crate) fn complete_audit(
        self,
        outcome: Result<ContextNativeResourceSnapshot, ContextPortFailure>,
    ) {
        let event = match self.request.as_ref() {
            Some(AgentPendingRequest::Audit(audit)) => {
                Some(ContextNativeEvent::ResourceAuditSettled(
                    ContextResourceAuditSettlement::new(*audit, outcome),
                ))
            }
            Some(AgentPendingRequest::ShutdownAudit(audit)) => {
                Some(ContextNativeEvent::ShutdownAuditSettled(
                    ContextShutdownAuditSettlement::new(*audit, outcome),
                ))
            }
            _ => None,
        };
        match event {
            Some(event) => self.complete(event),
            None => self.refuse(ContextPortFailure::NativeRefused),
        }
    }

    pub(crate) fn is_semantic(&self) -> bool {
        matches!(self.request, Some(AgentPendingRequest::Semantic(_)))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn take_semantic_invocation(&mut self) -> Option<SemanticRuntimeInvocation> {
        match self.request.as_mut() {
            Some(AgentPendingRequest::Semantic(request)) => request.invocation.take(),
            _ => None,
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn semantic_correlation(&self) -> Option<&SemanticRuntimeCorrelation> {
        match self.request.as_ref() {
            Some(AgentPendingRequest::Semantic(request)) => Some(&request.correlation),
            _ => None,
        }
    }

    pub(crate) fn admission_counts(&self) -> Option<(usize, usize)> {
        match self.permit.admission.group.as_ref() {
            Some(group) => group.upgrade()?.members.lock().ok()?.iter().try_fold(
                (0usize, 0usize),
                |(pending, captures), member| {
                    let counts = member.counts()?;
                    Some((
                        pending.checked_add(counts.0)?,
                        captures.checked_add(counts.1)?,
                    ))
                },
            ),
            None => self.permit.counts(),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    pub(crate) fn callback_guard(&self) -> AgentContextCallbackGuard {
        AgentContextCallbackGuard {
            admission: self.permit.admission.clone(),
            sink: self.sink.clone(),
        }
    }

    pub(crate) fn complete(mut self, event: ContextNativeEvent) {
        if !self.matches(&event) {
            self.permit.admission.fail_invariant();
            self.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        if let ContextNativeEvent::ShutdownAuditSettled(settlement) = &event {
            if let Ok(snapshot) = settlement.outcome() {
                self.permit.admission.verify_native_shutdown(snapshot);
            }
        }
        if let ContextNativeEvent::ResourceAuditSettled(settlement) = &event {
            if !settlement
                .outcome()
                .is_ok_and(AgentPortAdmission::native_resources_are_empty)
            {
                self.permit.admission.invalidate_retirement_audit();
            }
        }
        self.request = None;
        self.permit.release();
        emit_event(&self.sink, &self.permit.admission, event);
    }

    pub(crate) fn refuse(mut self, failure: ContextPortFailure) {
        let Some(request) = self.request.take() else {
            self.permit.admission.fail_invariant();
            return;
        };
        if matches!(&request, AgentPendingRequest::Audit(_)) {
            self.permit.admission.invalidate_retirement_audit();
        }
        let event = refusal_event(request, failure);
        self.permit.release();
        match event {
            Some(event) => emit_event(&self.sink, &self.permit.admission, event),
            None => self.permit.admission.fail_invariant(),
        }
    }

    fn cancel_without_event(mut self) {
        if matches!(self.request, Some(AgentPendingRequest::Audit(_))) {
            self.permit.admission.invalidate_retirement_audit();
        }
        self.request = None;
        self.permit.release();
    }

    fn matches(&self, event: &ContextNativeEvent) -> bool {
        match (self.request.as_ref(), event) {
            (
                Some(AgentPendingRequest::Native(ContextNativeRequest::Construct(request))),
                ContextNativeEvent::ConstructionSettled(settlement),
            ) => request.operation() == settlement.operation(),
            (
                Some(AgentPendingRequest::Native(ContextNativeRequest::Navigate(request))),
                ContextNativeEvent::NavigationSettled(settlement),
            ) => request.operation() == settlement.operation(),
            (
                Some(AgentPendingRequest::Native(ContextNativeRequest::Transition(request))),
                ContextNativeEvent::TransitionSettled(settlement),
            ) => request.operation() == settlement.operation(),
            (
                Some(AgentPendingRequest::Native(ContextNativeRequest::Cancel(request))),
                ContextNativeEvent::CancellationSettled(settlement),
            ) => request.current() == settlement.current(),
            (
                Some(AgentPendingRequest::Cookie(request)),
                ContextNativeEvent::CookieTransferSettled(settlement),
            ) => request == settlement.request(),
            (
                Some(AgentPendingRequest::Audit(audit)),
                ContextNativeEvent::ResourceAuditSettled(settlement),
            ) => *audit == settlement.audit(),
            (
                Some(AgentPendingRequest::ShutdownAudit(audit)),
                ContextNativeEvent::ShutdownAuditSettled(settlement),
            ) => *audit == settlement.audit(),
            (
                Some(AgentPendingRequest::Semantic(request)),
                ContextNativeEvent::SemanticRuntimeSettled(settlement),
            ) => &request.correlation == settlement.correlation(),
            _ => false,
        }
    }
}

impl Drop for AgentContextTask {
    fn drop(&mut self) {
        let Some(request) = self.request.take() else {
            return;
        };
        if matches!(&request, AgentPendingRequest::Audit(_)) {
            self.permit.admission.invalidate_retirement_audit();
        }
        let event = refusal_event(request, ContextPortFailure::NativeRefused);
        self.permit.release();
        match event {
            Some(event) => emit_event(&self.sink, &self.permit.admission, event),
            None => self.permit.admission.fail_invariant(),
        }
    }
}

fn emit_event(
    sink: &AgentContextEventSink,
    admission: &Arc<AgentPortAdmission>,
    event: ContextNativeEvent,
) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (sink)(event))).is_err() {
        admission.fail_invariant();
    }
}

/// Contains an external dispatcher/host panic in unwind-capable builds and
/// makes the port fail-stop.
///
/// This helper is used on both sides of `MainThreadDispatch`: once while the
/// composition root accepts the closure, and again when the event loop later
/// executes it. `None` never means success; admission is already sealed. The
/// optimized desktop retains the workspace's process-terminal `panic=abort`
/// policy and therefore cannot unwind into or out of this boundary.
fn contain_agent_port_panic<T>(
    admission: &AgentPortAdmission,
    operation: impl FnOnce() -> T,
) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(value) => Some(value),
        Err(_) => {
            admission.fail_invariant();
            None
        }
    }
}

fn refusal_event(
    request: AgentPendingRequest,
    failure: ContextPortFailure,
) -> Option<ContextNativeEvent> {
    match request {
        AgentPendingRequest::Native(ContextNativeRequest::Construct(request)) => {
            ContextConstructionSettlement::try_new(request.operation(), Err(failure))
                .ok()
                .map(ContextNativeEvent::ConstructionSettled)
        }
        AgentPendingRequest::Native(ContextNativeRequest::Navigate(request)) => {
            zephium_agentic::ContextNavigationSettlement::try_new(request.operation(), Err(failure))
                .ok()
                .map(ContextNativeEvent::NavigationSettled)
        }
        AgentPendingRequest::Native(ContextNativeRequest::Transition(request)) => {
            zephium_agentic::ContextTransitionSettlement::try_new(request.operation(), Err(failure))
                .ok()
                .map(ContextNativeEvent::TransitionSettled)
        }
        AgentPendingRequest::Native(ContextNativeRequest::Cancel(request)) => {
            Some(ContextNativeEvent::CancellationSettled(
                ContextCancellationSettlement::new(request.current(), Err(failure)),
            ))
        }
        AgentPendingRequest::Cookie(request) => {
            let cookie_failure = match failure {
                ContextPortFailure::Cancelled => ContextCookieTransferFailure::Cancelled,
                ContextPortFailure::TimedOut => ContextCookieTransferFailure::TimedOut,
                ContextPortFailure::Shutdown => ContextCookieTransferFailure::Shutdown,
                ContextPortFailure::Unsupported
                | ContextPortFailure::ResourceExhausted
                | ContextPortFailure::ProfileUnavailable
                | ContextPortFailure::ProfileBusy
                | ContextPortFailure::ExtensionIsolationUnproven
                | ContextPortFailure::CookieTransferFailed
                | ContextPortFailure::NativeRefused
                | ContextPortFailure::Stale => ContextCookieTransferFailure::DestinationUnavailable,
            };
            ContextCookieTransferSettlement::try_new(
                request,
                ContextCookieTransferOutcome::Refused(cookie_failure),
            )
            .ok()
            .map(Box::new)
            .map(ContextNativeEvent::CookieTransferSettled)
        }
        AgentPendingRequest::Audit(audit) => Some(ContextNativeEvent::ResourceAuditSettled(
            ContextResourceAuditSettlement::new(audit, Err(failure)),
        )),
        AgentPendingRequest::ShutdownAudit(audit) => {
            Some(ContextNativeEvent::ShutdownAuditSettled(
                ContextShutdownAuditSettlement::new(audit, Err(failure)),
            ))
        }
        AgentPendingRequest::Semantic(request) => SemanticRuntimeSettlement::try_new(
            request.correlation,
            Err(map_context_failure_to_semantic(failure)),
        )
        .ok()
        .map(Box::new)
        .map(ContextNativeEvent::SemanticRuntimeSettled),
    }
}

const fn map_context_failure_to_semantic(
    failure: ContextPortFailure,
) -> SemanticRuntimePortFailure {
    match failure {
        ContextPortFailure::Unsupported => SemanticRuntimePortFailure::Unsupported,
        ContextPortFailure::ResourceExhausted => SemanticRuntimePortFailure::ResourceExhausted,
        ContextPortFailure::Cancelled => SemanticRuntimePortFailure::Cancelled,
        ContextPortFailure::TimedOut => SemanticRuntimePortFailure::TimedOut,
        ContextPortFailure::Stale => SemanticRuntimePortFailure::Stale,
        ContextPortFailure::Shutdown => SemanticRuntimePortFailure::Shutdown,
        ContextPortFailure::ProfileUnavailable
        | ContextPortFailure::ProfileBusy
        | ContextPortFailure::ExtensionIsolationUnproven
        | ContextPortFailure::CookieTransferFailed
        | ContextPortFailure::NativeRefused => SemanticRuntimePortFailure::Transport,
    }
}

pub(crate) struct AgentContextPortSlot {
    dispatch: MainThreadDispatch,
    fatal: Arc<dyn Fn(&'static str) + Send + Sync>,
    fatal_reported: AtomicBool,
    state: Mutex<AgentContextPortSlotState>,
}

#[derive(Default)]
struct AgentContextPortSlotState {
    taken: bool,
    sealed: bool,
    admission: Option<Arc<AgentPortAdmission>>,
    factory: Option<Arc<AgentLifetimeFactoryInner>>,
}

/// Hard bound on sequential native lifetimes; no history or callback queue is
/// retained by the factory. Application persistence has its own matching cap.
pub const MAX_AGENT_BROWSER_LIFETIMES: u16 = 1024;

/// Process-unique native lifetime authority, mutually exclusive with the legacy
/// one-shot port. Each issued port is distinct and permanently seals itself.
/// This factory is not task/policy permission: the application must also join
/// the prior runtime, audit and durable terminal before a new run is admitted.
pub struct AgentBrowserLifetimeFactory {
    inner: Arc<AgentLifetimeFactoryInner>,
}

struct AgentLifetimeFactoryInner {
    dispatch: MainThreadDispatch,
    fatal: Arc<dyn Fn(&'static str) + Send + Sync>,
    failed: Arc<AtomicBool>,
    state: Mutex<AgentLifetimeFactoryState>,
}

#[derive(Default)]
struct AgentLifetimeFactoryState {
    sealed: bool,
    issued: u16,
    active: Option<Arc<AgentPortAdmission>>,
    group: Option<Arc<AgentPortGroup>>,
}

struct AgentPortGroup {
    work: zephium_agentic::WorkId,
    capacity: usize,
    sealed: AtomicBool,
    members: Mutex<Vec<Arc<AgentPortAdmission>>>,
}
impl AgentPortGroup {
    fn retire(&self) -> Result<(), ContextPortFailure> {
        let members = self
            .members
            .lock()
            .map_err(|_| ContextPortFailure::Shutdown)?;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if members.iter().any(|member| !member.work_is_absent()) {
            return Err(ContextPortFailure::ProfileBusy);
        }
        let mut states = members
            .iter()
            .map(|member| {
                member
                    .state
                    .lock()
                    .map_err(|_| ContextPortFailure::Shutdown)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if states
            .iter()
            .any(|state| state.invariant_failed || state.retired)
            || members.iter().any(|member| member.lineage_failed())
        {
            return Err(ContextPortFailure::Shutdown);
        }
        if states.iter().any(|state| {
            !state.sealed
                || !state.native_shutdown_verified
                || state.pending != 0
                || state.physical_screenshots != 0
        }) {
            return Err(ContextPortFailure::ProfileBusy);
        }
        for state in &mut states {
            state.retired = true;
        }
        Ok(())
    }
    fn seal(&self) {
        self.sealed.store(true, Ordering::Release);
        if let Ok(members) = self.members.lock() {
            for member in members.iter() {
                member.seal();
            }
        }
    }
}

impl AgentBrowserLifetimeFactory {
    /// Starts one fresh lifetime only after the prior exact native shutdown
    /// task proved zero resources and every native permit has drained. Failure
    /// neither replaces the active owner nor retries any native operation.
    pub fn begin(
        &mut self,
        sink: impl Fn(ContextNativeEvent) + Send + Sync + 'static,
    ) -> Result<Arc<dyn AgentBrowserPort>, ContextPortFailure> {
        let mut state = self.inner.state.lock().map_err(|_| {
            self.inner.failed.store(true, Ordering::Release);
            ContextPortFailure::Shutdown
        })?;
        if state.sealed || self.inner.failed.load(Ordering::Acquire) {
            return Err(ContextPortFailure::Shutdown);
        }
        if state.issued >= MAX_AGENT_BROWSER_LIFETIMES {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if let Some(group) = &state.group {
            group.retire()?;
        }
        state.group = None;
        if let Some(active) = state.active.clone() {
            if let Err(failure) = active.retire_for_successor() {
                drop(state);
                if failure == ContextPortFailure::Shutdown {
                    active.report_fatal_once();
                }
                return Err(failure);
            }
        }
        let mut admission = AgentPortAdmission::new(self.inner.fatal.clone());
        admission.lineage_failed = Some(self.inner.failed.clone());
        let admission = Arc::new(admission);
        state.issued += 1;
        state.active = Some(admission.clone());
        Ok(Arc::new(EngineAgentBrowserPort {
            dispatch: self.inner.dispatch.clone(),
            admission,
            sink: Arc::new(sink),
        }))
    }

    /// Starts one isolated page in a bounded Work group. Global zero-resource
    /// auditing still covers every member before another lifetime may begin.
    pub fn begin_work_page(
        &mut self,
        work: zephium_agentic::WorkId,
        capacity: u8,
        sink: impl Fn(ContextNativeEvent) + Send + Sync + 'static,
    ) -> Result<Arc<dyn AgentBrowserPort>, ContextPortFailure> {
        if !(1..=3).contains(&capacity) {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| ContextPortFailure::Shutdown)?;
        if state.sealed || self.inner.failed.load(Ordering::Acquire) {
            return Err(ContextPortFailure::Shutdown);
        }
        if state.issued >= MAX_AGENT_BROWSER_LIFETIMES {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if let Some(active) = &state.active {
            active.retire_for_successor()?;
        }
        state.active = None;
        if let Some(group) = &state.group {
            if group.work != work
                || group.capacity != usize::from(capacity)
                || group.sealed.load(Ordering::Acquire)
            {
                group.retire()?;
                state.group = None;
            }
        }
        let group = state
            .group
            .get_or_insert_with(|| {
                Arc::new(AgentPortGroup {
                    work,
                    capacity: usize::from(capacity),
                    sealed: AtomicBool::new(false),
                    members: Mutex::new(Vec::new()),
                })
            })
            .clone();
        let mut members = group
            .members
            .lock()
            .map_err(|_| ContextPortFailure::Shutdown)?;
        if members.len() >= group.capacity {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let mut admission = AgentPortAdmission::new(self.inner.fatal.clone());
        admission.lineage_failed = Some(self.inner.failed.clone());
        admission.group = Some(Arc::downgrade(&group));
        let admission = Arc::new(admission);
        members.push(admission.clone());
        state.issued += 1;
        Ok(Arc::new(EngineAgentBrowserPort {
            dispatch: self.inner.dispatch.clone(),
            admission,
            sink: Arc::new(sink),
        }))
    }
}

impl AgentLifetimeFactoryInner {
    fn seal(&self) {
        match self.state.lock() {
            Ok(mut state) => {
                state.sealed = true;
                if let Some(active) = &state.active {
                    active.seal();
                }
                if let Some(group) = &state.group {
                    group.seal();
                }
            }
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                self.failed.store(true, Ordering::Release);
                state.sealed = true;
                if let Some(active) = &state.active {
                    active.seal();
                }
                if let Some(group) = &state.group {
                    group.seal();
                }
            }
        }
    }
}

impl AgentContextPortSlot {
    pub(crate) fn new(
        dispatch: MainThreadDispatch,
        fatal: Arc<dyn Fn(&'static str) + Send + Sync>,
    ) -> Self {
        Self {
            dispatch,
            fatal,
            fatal_reported: AtomicBool::new(false),
            state: Mutex::new(AgentContextPortSlotState::default()),
        }
    }

    pub(crate) fn take(&self, sink: AgentContextEventSink) -> Option<Arc<dyn AgentBrowserPort>> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.sealed = true;
                if let Some(admission) = &state.admission {
                    admission.seal();
                }
                if let Some(factory) = &state.factory {
                    factory.seal();
                }
                drop(state);
                self.report_fatal_once();
                return None;
            }
        };
        if state.taken || state.sealed {
            return None;
        }
        let admission = Arc::new(AgentPortAdmission::new(self.fatal.clone()));
        state.taken = true;
        state.admission = Some(admission.clone());
        Some(Arc::new(EngineAgentBrowserPort {
            dispatch: self.dispatch.clone(),
            admission,
            sink,
        }))
    }

    pub(crate) fn take_factory(&self) -> Option<AgentBrowserLifetimeFactory> {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.sealed = true;
                if let Some(admission) = &state.admission {
                    admission.seal();
                }
                if let Some(factory) = &state.factory {
                    factory.seal();
                }
                drop(state);
                self.report_fatal_once();
                return None;
            }
        };
        if state.taken || state.sealed {
            return None;
        }
        let inner = Arc::new(AgentLifetimeFactoryInner {
            dispatch: self.dispatch.clone(),
            fatal: self.fatal.clone(),
            failed: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(AgentLifetimeFactoryState::default()),
        });
        state.taken = true;
        state.factory = Some(inner.clone());
        Some(AgentBrowserLifetimeFactory { inner })
    }

    pub(crate) fn seal(&self) {
        match self.state.lock() {
            Ok(mut state) => {
                state.sealed = true;
                if let Some(admission) = &state.admission {
                    admission.seal();
                }
                if let Some(factory) = &state.factory {
                    factory.seal();
                }
            }
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.sealed = true;
                if let Some(admission) = &state.admission {
                    admission.seal();
                }
                if let Some(factory) = &state.factory {
                    factory.seal();
                }
                drop(state);
                self.report_fatal_once();
            }
        }
    }

    fn report_fatal_once(&self) {
        if !self.fatal_reported.swap(true, Ordering::AcqRel) {
            // Slot poisoning can be observed from process teardown in an
            // unwind-capable build. Contain a consumer panic there; optimized
            // desktop builds retain the workspace's process-terminal abort
            // policy and cannot unwind across this callback.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.fatal)("agent-context port slot invariant failed");
            }));
        }
    }

    #[cfg(test)]
    pub(crate) fn disabled_for_test() -> Self {
        let slot = Self::new(Arc::new(|_| false), Arc::new(|_| {}));
        slot.seal();
        slot
    }
}

struct EngineAgentBrowserPort {
    dispatch: MainThreadDispatch,
    admission: Arc<AgentPortAdmission>,
    sink: AgentContextEventSink,
}

impl EngineAgentBrowserPort {
    fn schedule(&self, request: AgentPendingRequest) -> ContextDispatch {
        let permit = match self.admission.reserve() {
            Ok(permit) => permit,
            Err(failure) => return ContextDispatch::Rejected(failure),
        };
        self.schedule_reserved(request, permit)
    }

    fn schedule_audit(&self, request: AgentPendingRequest) -> ContextDispatch {
        let permit = match self.admission.reserve_audit() {
            Ok(permit) => permit,
            Err(failure) => return ContextDispatch::Rejected(failure),
        };
        self.schedule_reserved(request, permit)
    }

    fn schedule_reserved(
        &self,
        request: AgentPendingRequest,
        permit: AgentTaskPermit,
    ) -> ContextDispatch {
        let task = AgentContextTask::new(request, permit, self.sink.clone());
        let slot = Arc::new(Mutex::new(Some(task)));
        let for_dispatch = slot.clone();
        let executed = Arc::new(AtomicBool::new(false));
        let executed_in_dispatch = executed.clone();
        let callback_admission = self.admission.clone();
        let dispatch = self.dispatch.clone();
        let accepted = contain_agent_port_panic(&self.admission, || {
            dispatch(Box::new(move || {
                executed_in_dispatch.store(true, Ordering::Release);
                let _ = contain_agent_port_panic(&callback_admission, || {
                    let task = for_dispatch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some(task) = task {
                        dispatch_to_host(task);
                    }
                });
            }))
        })
        .unwrap_or(false);
        if accepted || executed.load(Ordering::Acquire) {
            ContextDispatch::Scheduled
        } else {
            if let Some(task) = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                task.cancel_without_event();
            }
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        }
    }

    #[cfg(target_os = "macos")]
    fn schedule_screenshot(
        &self,
        request: SemanticScreenshotNativeRequest,
        completion: SemanticScreenshotNativeCompletion,
    ) -> ContextDispatch {
        let (permit, physical) = match self.admission.reserve_screenshot() {
            Ok(permits) => permits,
            Err(failure) => return ContextDispatch::Rejected(failure),
        };
        let task = AgentScreenshotTask::new(request, completion, physical, permit);
        let slot = Arc::new(Mutex::new(Some(task)));
        let for_dispatch = slot.clone();
        let executed = Arc::new(AtomicBool::new(false));
        let executed_in_dispatch = executed.clone();
        let callback_admission = self.admission.clone();
        let dispatch = self.dispatch.clone();
        let accepted = contain_agent_port_panic(&self.admission, || {
            dispatch(Box::new(move || {
                executed_in_dispatch.store(true, Ordering::Release);
                let _ = contain_agent_port_panic(&callback_admission, || {
                    let task = for_dispatch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some(task) = task {
                        dispatch_screenshot_to_host(task);
                    }
                });
            }))
        })
        .unwrap_or(false);
        if accepted || executed.load(Ordering::Acquire) {
            ContextDispatch::Scheduled
        } else {
            if let Some(task) = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                task.cancel_without_completion();
            }
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        }
    }

    #[cfg(target_os = "macos")]
    fn schedule_action(
        &self,
        request: SemanticActionNativeRequest,
        completion: SemanticActionNativeCompletion,
    ) -> ContextDispatch {
        let permit = match self.admission.reserve() {
            Ok(permit) => permit,
            Err(failure) => return ContextDispatch::Rejected(failure),
        };
        let task = AgentActionTask::new(request, completion, permit);
        let slot = Arc::new(Mutex::new(Some(task)));
        let for_dispatch = slot.clone();
        let executed = Arc::new(AtomicBool::new(false));
        let executed_in_dispatch = executed.clone();
        let callback_admission = self.admission.clone();
        let dispatch = self.dispatch.clone();
        let accepted = contain_agent_port_panic(&self.admission, || {
            dispatch(Box::new(move || {
                executed_in_dispatch.store(true, Ordering::Release);
                let _ = contain_agent_port_panic(&callback_admission, || {
                    let task = for_dispatch
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    if let Some(task) = task {
                        dispatch_action_to_host(task);
                    }
                });
            }))
        })
        .unwrap_or(false);
        if accepted || executed.load(Ordering::Acquire) {
            ContextDispatch::Scheduled
        } else {
            if let Some(task) = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            {
                task.cancel_without_completion();
            }
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        }
    }
}

impl AgentBrowserPort for EngineAgentBrowserPort {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn work_screenshot_public_candidate(
        &self,
        lease: &zephium_agentic::WorkBrowserExecutionLease,
        context: zephium_agentic::ContextJoin,
    ) -> bool {
        self.native_image_public_candidate(lease, context)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn work_screenshot_public_candidate(
        &self,
        _lease: &zephium_agentic::WorkBrowserExecutionLease,
        _context: zephium_agentic::ContextJoin,
    ) -> bool {
        false
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn claim_work_screenshot_source(
        &self,
        lease: &zephium_agentic::WorkBrowserExecutionLease,
        image: &zephium_agentic::SemanticScreenshot,
    ) -> Option<Arc<dyn zephium_agentic::SemanticScreenshotSource>> {
        self.claim_native_image_source(lease, image)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn claim_work_screenshot_source(
        &self,
        _lease: &zephium_agentic::WorkBrowserExecutionLease,
        _image: &zephium_agentic::SemanticScreenshot,
    ) -> Option<Arc<dyn zephium_agentic::SemanticScreenshotSource>> {
        None
    }

    #[cfg(all(
        feature = "agentic-browser",
        any(target_os = "macos", target_os = "windows")
    ))]
    fn latest_work_frame(
        &self,
        resource: &zephium_agentic::WorkBrowserResourceJoin,
    ) -> Option<Arc<zephium_agentic::WorkBrowserFrame>> {
        crate::host::work_frames::latest(resource.identity().context())
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn supports_work_resource_history_back(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            let version = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
            version.majorVersion >= 26
        }
        #[cfg(target_os = "windows")]
        {
            true
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn work_resource_act(
        &self,
        request: zephium_agentic::WorkBrowserActionRequest,
        completion: zephium_agentic::WorkBrowserActionCompletionCallback,
    ) -> zephium_agentic::WorkBrowserActionDispatch {
        self.schedule_work_action(request, completion)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn work_resource_navigate(
        &self,
        request: zephium_agentic::WorkBrowserNavigationRequest,
        completion: zephium_agentic::WorkBrowserNavigationCompletionCallback,
    ) -> zephium_agentic::WorkBrowserNavigationDispatch {
        self.schedule_work_navigation(request, completion)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn work_resource_back(
        &self,
        request: zephium_agentic::WorkBrowserHistoryBackRequest,
        completion: zephium_agentic::WorkBrowserHistoryBackCompletionCallback,
    ) -> zephium_agentic::WorkBrowserHistoryBackDispatch {
        self.schedule_work_history_back(request, completion)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn work_resource_lifecycle(
        &self,
        request: zephium_agentic::WorkBrowserResourceRequest,
        completion: zephium_agentic::WorkBrowserResourceCompletionCallback,
    ) -> zephium_agentic::WorkBrowserResourceDispatch {
        self.schedule_work_lifecycle(request, completion)
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn work_resource_observe(
        &self,
        request: zephium_agentic::WorkBrowserObservationRequest,
        completion: zephium_agentic::WorkBrowserObservationCompletionCallback,
    ) -> zephium_agentic::WorkBrowserObservationDispatch {
        self.schedule_work_observation(request, completion)
    }
    #[cfg(all(target_os = "macos", feature = "native-agentic-foreground-probe"))]
    fn probe_foreground_rendering(
        &self,
        request: zephium_agentic::ForegroundRenderingProbeRequest,
        completion: zephium_agentic::ForegroundRenderingProbeCompletion,
    ) -> ContextDispatch {
        self.schedule_foreground_probe(request, completion)
    }

    fn dispatch(&self, request: ContextNativeRequest) -> ContextDispatch {
        if self.admission.group.is_some() {
            return ContextDispatch::Rejected(ContextPortFailure::NativeRefused);
        }
        if !supports_native_request(&request) {
            return ContextDispatch::Unsupported;
        }
        self.schedule(AgentPendingRequest::Native(request))
    }

    fn transfer_cookies(&self, request: ContextCookieTransferRequest) -> ContextDispatch {
        if !supports_cookie_transfer() {
            return ContextDispatch::Unsupported;
        }
        self.schedule(AgentPendingRequest::Cookie(request))
    }

    fn audit_resources(&self, audit: ContextResourceAuditId) -> ContextDispatch {
        self.schedule_audit(AgentPendingRequest::Audit(audit))
    }

    fn seal_for_shutdown(&self, audit: ContextResourceAuditId) -> ContextShutdownDispatch {
        let permit = match self.admission.reserve_shutdown_audit() {
            Ok(permit) => permit,
            Err(failure) => return ContextShutdownDispatch::SealedWithoutAudit(failure),
        };
        match self.schedule_reserved(AgentPendingRequest::ShutdownAudit(audit), permit) {
            ContextDispatch::Scheduled => ContextShutdownDispatch::AuditScheduled,
            ContextDispatch::Rejected(failure) => {
                ContextShutdownDispatch::SealedWithoutAudit(failure)
            }
            ContextDispatch::Unsupported => {
                self.admission.fail_invariant();
                ContextShutdownDispatch::SealedWithoutAudit(ContextPortFailure::Shutdown)
            }
        }
    }

    fn invoke_semantic(&self, invocation: SemanticRuntimeInvocation) -> ContextDispatch {
        if !supports_semantic_invocation(&invocation) {
            return ContextDispatch::Unsupported;
        }
        let correlation = invocation.correlation();
        self.schedule(AgentPendingRequest::Semantic(AgentPendingSemantic {
            correlation,
            invocation: Some(invocation),
        }))
    }

    fn execute_semantic_action(
        &self,
        request: SemanticActionNativeRequest,
        completion: SemanticActionNativeCompletion,
    ) -> ContextDispatch {
        #[cfg(target_os = "macos")]
        {
            if !matches!(
                request.kind(),
                zephium_agentic::SemanticActionKind::Click
                    | zephium_agentic::SemanticActionKind::Fill
                    | zephium_agentic::SemanticActionKind::Select
            ) || request.frame().frame() != zephium_agentic::FrameId::MAIN
                || request.frame().trust() != zephium_agentic::SemanticFrameTrust::SameOrigin
                || request.frame().context().identity().kind()
                    != zephium_agentic::ContextKind::Owned
            {
                return ContextDispatch::Unsupported;
            }
            self.schedule_action(request, completion)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (request, completion);
            ContextDispatch::Unsupported
        }
    }

    fn capture_semantic_screenshot(
        &self,
        request: SemanticScreenshotNativeRequest,
        completion: SemanticScreenshotNativeCompletion,
    ) -> ContextDispatch {
        #[cfg(target_os = "macos")]
        {
            if !supports_semantic_screenshot(&request) {
                return ContextDispatch::Unsupported;
            }
            self.schedule_screenshot(request, completion)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (request, completion);
            ContextDispatch::Unsupported
        }
    }
}

fn dispatch_to_host(task: AgentContextTask) {
    let slot = Arc::new(Mutex::new(Some(task)));
    let for_host = slot.clone();
    if !crate::host::try_with_agent_context(move |host| {
        let task = for_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            host.handle_agent_context_task(task);
        }
    }) {
        if let Some(task) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.refuse(ContextPortFailure::Shutdown);
        }
    }
}

#[cfg(target_os = "macos")]
fn dispatch_screenshot_to_host(task: AgentScreenshotTask) {
    let slot = Arc::new(Mutex::new(Some(task)));
    let for_host = slot.clone();
    if !crate::host::try_with_agent_context(move |host| {
        let task = for_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            host.handle_agent_screenshot_task(task);
        }
    }) {
        if let Some(task) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.refuse(SemanticScreenshotNativeFailure::Shutdown);
        }
    }
}

#[cfg(target_os = "macos")]
fn dispatch_action_to_host(task: AgentActionTask) {
    let slot = Arc::new(Mutex::new(Some(task)));
    let for_host = slot.clone();
    if !crate::host::try_with_agent_context(move |host| {
        let task = for_host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            host.handle_agent_action_task(task);
        }
    }) {
        if let Some(task) = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            task.refuse(SemanticActionNativeFailure::Shutdown);
        }
    }
}

fn supports_native_request(request: &ContextNativeRequest) -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        match request {
            ContextNativeRequest::Construct(request) => matches!(
                request.source(),
                zephium_agentic::ContextConstructionSource::Owned
            ),
            ContextNativeRequest::Transition(request) => {
                supports_owned_transition(request.operation().kind())
                    && request.operation().context().identity().kind()
                        == zephium_agentic::ContextKind::Owned
            }
            ContextNativeRequest::Cancel(request) => {
                request.current().identity().kind() == zephium_agentic::ContextKind::Owned
            }
            ContextNativeRequest::Navigate(request) => {
                request.operation().context().identity().kind()
                    == zephium_agentic::ContextKind::Owned
            }
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = request;
        false
    }
}

const fn supports_owned_transition(kind: ContextOperationKind) -> bool {
    #[cfg(target_os = "windows")]
    {
        matches!(
            kind,
            ContextOperationKind::Suspend
                | ContextOperationKind::Resume
                | ContextOperationKind::Recover
                | ContextOperationKind::Close
        )
    }
    #[cfg(target_os = "macos")]
    {
        matches!(
            kind,
            ContextOperationKind::Recover | ContextOperationKind::Close
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = kind;
        false
    }
}

const fn supports_cookie_transfer() -> bool {
    #[cfg(target_os = "windows")]
    {
        true
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

fn supports_semantic_invocation(invocation: &SemanticRuntimeInvocation) -> bool {
    #[cfg(target_os = "macos")]
    {
        invocation.frame().context().identity().kind() == zephium_agentic::ContextKind::Owned
            && invocation.frame().frame() == zephium_agentic::FrameId::MAIN
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = invocation;
        false
    }
}

#[cfg(target_os = "macos")]
fn supports_semantic_screenshot(request: &SemanticScreenshotNativeRequest) -> bool {
    request.context().identity().kind() == zephium_agentic::ContextKind::Owned
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use zephium_agentic::{
        decode_semantic_snapshot, encode_semantic_observation, encode_semantic_runtime_invocation,
        prepare_semantic_screenshot, ContextCapabilities, ContextCapability,
        ContextConstructionProof, ContextConstructionRequest, ContextConstructionSource,
        ContextCookieOrigin, ContextCookieScope, ContextCookieTransferId,
        ContextCookieTransferInstant, ContextCookieTransferWindow, ContextId, ContextIdentity,
        ContextKind, ContextOperationId, ContextProfileLeaseId, ContextProfileLeasePurpose,
        ContextProfileLeaseRegistry, ContextProfileStorageClass, ContextRegistry, ContextRunId,
        ContextSettlement, FrameId, SemanticCaptureInstant, SemanticDecodeContext,
        SemanticFrameJoin, SemanticFrameTrust, SemanticInvocationId,
        SemanticModelDeliverySettlement, SemanticModelEncodingBudget, SemanticObservationAssembler,
        SemanticObservationBudget, SemanticObservationId, SemanticObservationRequest,
        SemanticOrigin, SemanticRuntimeBudget, SemanticScreenshotBudget,
        SemanticScreenshotCoordinator, SemanticScreenshotRequestId, SemanticSnapshotGeneration,
        SemanticTokenCountQuality, SemanticTokenCountRequirement, SemanticTokenCounter,
        SemanticTokenCounterError, SemanticTokenMeasurement, SemanticTokenizerRevision,
        SEMANTIC_WIRE_VERSION,
    };
    use zephium_core::ids::ProfileId;

    fn construction_request() -> ContextNativeRequest {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::generate(),
            zephium_agentic::ContextKind::Owned,
        );
        let capabilities = ContextCapabilities::try_new(
            zephium_agentic::ContextKind::Owned,
            &[ContextCapability::Navigate],
        )
        .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let operation = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("begin");
        let lease = ContextProfileLeaseRegistry::new()
            .acquire(
                ContextProfileLeaseId::new(1).expect("lease id"),
                identity,
                ContextProfileStorageClass::Durable,
                ContextProfileLeasePurpose::Owned,
            )
            .expect("lease");
        ContextNativeRequest::Construct(
            ContextConstructionRequest::try_new(
                operation,
                capabilities,
                lease,
                ContextConstructionSource::Owned,
            )
            .expect("request"),
        )
    }

    fn cookie_request() -> ContextCookieTransferRequest {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::generate(),
            ContextKind::Owned,
        );
        let capabilities =
            ContextCapabilities::try_new(ContextKind::Owned, &[ContextCapability::ImportCookies])
                .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("construction");
        registry
            .settle_construction(identity.id(), construction, ContextSettlement::Applied)
            .expect("settlement");
        let destination = registry.join(identity.id()).expect("join");
        let lease = ContextProfileLeaseRegistry::new()
            .acquire(
                ContextProfileLeaseId::new(1).expect("lease"),
                identity,
                ContextProfileStorageClass::Durable,
                ContextProfileLeasePurpose::Owned,
            )
            .expect("profile lease");
        ContextCookieTransferRequest::selected_profile_to_owned(
            ContextCookieTransferId::new(1).expect("transfer"),
            ContextCookieTransferWindow::try_new(
                ContextCookieTransferInstant::from_millis(1_000),
                ContextCookieTransferInstant::from_millis(31_000),
            )
            .expect("window"),
            destination,
            capabilities,
            lease,
            ContextConstructionProof::WindowsOwnedAutomationSubprofileEmptyInventory,
            ContextCookieScope::try_new(vec![ContextCookieOrigin::parse(
                "https://port.example.test/private",
            )
            .expect("origin")])
            .expect("scope"),
        )
        .expect("cookie request")
    }

    fn semantic_invocation(frame_id: FrameId) -> SemanticRuntimeInvocation {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::generate(),
            zephium_agentic::ContextKind::Owned,
        );
        let capabilities = ContextCapabilities::try_new(
            zephium_agentic::ContextKind::Owned,
            &[ContextCapability::Observe],
        )
        .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("construction");
        registry
            .settle_construction(identity.id(), construction, ContextSettlement::Applied)
            .expect("settlement");
        let context = registry.join(identity.id()).expect("join");
        let request = SemanticObservationRequest::initial(
            SemanticObservationId::new(1).expect("observation"),
            context,
            SemanticObservationBudget::INITIAL_FILTERED,
        );
        let frame = SemanticFrameJoin::try_new(
            context,
            frame_id,
            context.frame_generation(),
            SemanticOrigin::parse("https://port.example.test/path").expect("origin"),
            SemanticFrameTrust::SameOrigin,
        )
        .expect("frame");
        encode_semantic_runtime_invocation(
            &request,
            frame,
            SemanticInvocationId::new(1).expect("invocation"),
            SemanticSnapshotGeneration::INITIAL,
            SemanticRuntimeBudget::INITIAL_FILTERED,
        )
        .expect("encode")
    }

    struct ScreenshotTokenCounter {
        revision: SemanticTokenizerRevision,
    }

    impl SemanticTokenCounter for ScreenshotTokenCounter {
        fn count_tokens(
            &self,
            input: &str,
        ) -> Result<SemanticTokenMeasurement, SemanticTokenCounterError> {
            if input.is_empty() {
                return Err(SemanticTokenCounterError::InvalidResult);
            }
            SemanticTokenMeasurement::try_new(
                self.revision.clone(),
                32,
                SemanticTokenCountQuality::ExactLocal,
            )
            .map_err(|_| SemanticTokenCounterError::InvalidResult)
        }
    }

    fn screenshot_native_request(seed: u64) -> SemanticScreenshotNativeRequest {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::from(u128::from(seed + 2)),
            ContextKind::Owned,
        );
        let capabilities =
            ContextCapabilities::try_new(ContextKind::Owned, &[ContextCapability::Observe])
                .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("construction");
        registry
            .settle_construction(identity.id(), construction, ContextSettlement::Applied)
            .expect("settlement");
        let context = registry.join(identity.id()).expect("context");
        let frame = SemanticFrameJoin::try_new(
            context,
            FrameId::MAIN,
            context.frame_generation(),
            SemanticOrigin::parse("https://screenshot-port.example.test/private").expect("origin"),
            SemanticFrameTrust::SameOrigin,
        )
        .expect("frame");
        let bytes = serde_json::to_vec(&serde_json::json!({
            "v": SEMANTIC_WIRE_VERSION,
            "i": seed + 10,
            "g": 1,
            "c": "complete",
            "n": [
                {"k": 1, "r": "document", "o": 16},
                {"k": 2, "p": 0, "r": "paragraph", "t": "sensitive", "q": "sensitive"}
            ]
        }))
        .expect("wire");
        let snapshot = decode_semantic_snapshot(
            SemanticDecodeContext::new(
                SemanticInvocationId::new(seed + 10).expect("invocation"),
                frame,
                SemanticSnapshotGeneration::INITIAL,
            ),
            &bytes,
        )
        .expect("snapshot");
        let observation_request = SemanticObservationRequest::initial(
            SemanticObservationId::new(seed + 20).expect("observation"),
            context,
            SemanticObservationBudget::INITIAL_FILTERED,
        );
        let observation = SemanticObservationAssembler::new(observation_request, snapshot)
            .expect("assembler")
            .finish()
            .expect("observation");
        let revision = SemanticTokenizerRevision::try_new("engine-screenshot-port-v1".to_owned())
            .expect("revision");
        let acknowledgement = encode_semantic_observation(
            &observation,
            SemanticModelEncodingBudget::try_new(
                32 * 1024,
                1_000,
                SemanticTokenCountRequirement::Exact,
            )
            .expect("budget"),
        )
        .expect("encode")
        .admit(
            &ScreenshotTokenCounter {
                revision: revision.clone(),
            },
            &revision,
        )
        .expect("admit")
        .settle_delivery(SemanticModelDeliverySettlement::Committed)
        .expect("delivery");
        let request = prepare_semantic_screenshot(
            SemanticScreenshotRequestId::new(seed + 30).expect("screenshot"),
            &observation,
            &acknowledgement,
            SemanticCaptureInstant::from_millis(1_000),
            SemanticCaptureInstant::from_millis(2_000),
            SemanticScreenshotBudget::STANDARD,
        )
        .expect("screenshot request");
        SemanticScreenshotCoordinator::new()
            .begin(request)
            .expect("begin screenshot")
            .1
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn recovery_request() -> ContextNativeRequest {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            ProfileId::generate(),
            zephium_agentic::ContextKind::Owned,
        );
        let capabilities = ContextCapabilities::try_new(
            zephium_agentic::ContextKind::Owned,
            &[ContextCapability::Recover],
        )
        .expect("capabilities");
        let mut registry = ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("operation"),
            )
            .expect("construction");
        registry
            .settle_construction(
                identity.id(),
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction settlement");
        let prior = registry.join(identity.id()).expect("join");
        registry
            .renderer_lost(identity.id(), prior)
            .expect("renderer loss");
        let recovery = registry
            .begin_recovery(
                identity.id(),
                ContextOperationId::new(2).expect("operation"),
            )
            .expect("recovery");
        ContextNativeRequest::Transition(
            zephium_agentic::ContextTransitionRequest::try_new(recovery)
                .expect("transition request"),
        )
    }

    #[test]
    fn slot_is_exact_once_and_shutdown_seals_untaken_port() {
        let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        let first = slot.take(Arc::new(|_| {}));
        assert!(first.is_some());
        assert!(slot.take(Arc::new(|_| {})).is_none());

        let sealed = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        sealed.seal();
        assert!(sealed.take(Arc::new(|_| {})).is_none());
    }

    fn zero_native_snapshot() -> ContextNativeResourceSnapshot {
        ContextNativeResourceSnapshot::try_new(zephium_agentic::ContextNativeResourceCounts {
            known_bindings: 0,
            resident_views: 0,
            owned_reservations: 0,
            borrowed_leases: 0,
            visible_surfaces: 0,
            suspended_views: 0,
            pending_operations: 0,
            pending_captures: 0,
            queued_tasks: 0,
        })
        .unwrap()
    }

    fn factory_admission(factory: &AgentBrowserLifetimeFactory) -> Arc<AgentPortAdmission> {
        factory.inner.state.lock().unwrap().active.clone().unwrap()
    }

    fn settle_factory_native_shutdown(factory: &AgentBrowserLifetimeFactory) {
        let admission = factory_admission(factory);
        let permit = admission.reserve_shutdown_audit().unwrap();
        AgentContextTask::new(
            AgentPendingRequest::ShutdownAudit(ContextResourceAuditId::new(1).unwrap()),
            permit,
            Arc::new(|_| {}),
        )
        .complete_audit(Ok(zero_native_snapshot()));
    }

    #[test]
    fn work_page_factory_caps_members_and_requires_every_original_shutdown_receipt() {
        let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        let mut factory = slot.take_factory().unwrap();
        let work = zephium_agentic::WorkId::generate();
        let ports: Vec<_> = (0..3)
            .map(|_| factory.begin_work_page(work, 3, |_| {}).unwrap())
            .collect();
        assert!(matches!(
            factory.begin_work_page(work, 3, |_| {}),
            Err(ContextPortFailure::ResourceExhausted)
        ));
        assert!(matches!(
            factory.begin_work_page(zephium_agentic::WorkId::generate(), 3, |_| {}),
            Err(ContextPortFailure::ProfileBusy)
        ));
        assert!(matches!(
            factory.begin(|_| {}),
            Err(ContextPortFailure::ProfileBusy)
        ));
        let group = factory.inner.state.lock().unwrap().group.clone().unwrap();
        let members = group.members.lock().unwrap().clone();
        let (capture_task, capture) = members[1].reserve_screenshot().unwrap();
        let pending = members[2].reserve().unwrap();
        let audit = AgentContextTask::new(
            AgentPendingRequest::Audit(ContextResourceAuditId::new(1).unwrap()),
            members[0].reserve_audit().unwrap(),
            Arc::new(|_| {}),
        );
        assert_eq!(audit.admission_counts(), Some((3, 1)));
        drop(capture_task);
        drop(pending);
        assert_eq!(audit.admission_counts(), Some((1, 1)));
        drop(capture);
        assert_eq!(audit.admission_counts(), Some((1, 0)));
        audit.complete_audit(Ok(zero_native_snapshot()));
        for (index, admission) in members.iter().enumerate() {
            let permit = admission.reserve_shutdown_audit().unwrap();
            assert!(members
                .iter()
                .all(|member| matches!(member.reserve(), Err(ContextPortFailure::Shutdown))));
            let task = AgentContextTask::new(
                AgentPendingRequest::ShutdownAudit(ContextResourceAuditId::new(1).unwrap()),
                permit,
                Arc::new(|_| {}),
            );
            assert_eq!(task.admission_counts(), Some((1, 0)));
            task.complete_audit(Ok(zero_native_snapshot()));
            if index < 2 {
                assert!(matches!(
                    factory.begin(|_| {}),
                    Err(ContextPortFailure::ProfileBusy)
                ));
            }
        }
        let next = factory.begin_work_page(work, 3, |_| {}).unwrap();
        assert!(!Arc::ptr_eq(&ports[0], &next));
        assert!(ports.iter().all(|port| matches!(
            port.audit_resources(ContextResourceAuditId::new(2).unwrap()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        )));
        slot.seal();
        assert!(matches!(
            factory.begin_work_page(work, 3, |_| {}),
            Err(ContextPortFailure::Shutdown)
        ));
    }

    #[test]
    fn lifetime_factory_is_exclusive_bounded_and_never_reopens_an_old_port() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::AcqRel);
                false
            }),
            Arc::new(|_| {}),
        );
        let mut factory = slot.take_factory().unwrap();
        assert!(slot.take_factory().is_none());
        assert!(slot.take(Arc::new(|_| {})).is_none());
        let first = factory.begin(|_| {}).unwrap();
        assert!(matches!(
            factory.begin(|_| {}),
            Err(ContextPortFailure::ProfileBusy)
        ));
        settle_factory_native_shutdown(&factory);
        let second = factory.begin(|_| {}).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(
            first.dispatch(construction_request()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(
            first.audit_resources(ContextResourceAuditId::new(2).unwrap()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(calls.load(Ordering::Acquire), 0);
        factory.inner.state.lock().unwrap().issued = MAX_AGENT_BROWSER_LIFETIMES;
        settle_factory_native_shutdown(&factory);
        assert!(matches!(
            factory.begin(|_| {}),
            Err(ContextPortFailure::ResourceExhausted)
        ));
        slot.seal();
        assert!(matches!(
            factory.begin(|_| {}),
            Err(ContextPortFailure::Shutdown)
        ));

        let legacy = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        assert!(legacy.take(Arc::new(|_| {})).is_some());
        assert!(legacy.take_factory().is_none());
    }

    #[test]
    fn lifetime_factory_requires_exact_shutdown_not_an_ordinary_zero_or_missing_receipt() {
        for fault in 0..5 {
            let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
            let mut factory = slot.take_factory().unwrap();
            let _port = factory.begin(|_| {}).unwrap();
            let admission = factory_admission(&factory);
            let audit = ContextResourceAuditId::new(1).unwrap();
            let ordinary = fault == 0;
            let permit = if ordinary {
                admission.seal();
                admission.reserve_audit().unwrap()
            } else {
                admission.reserve_shutdown_audit().unwrap()
            };
            let task = AgentContextTask::new(
                if ordinary {
                    AgentPendingRequest::Audit(audit)
                } else {
                    AgentPendingRequest::ShutdownAudit(audit)
                },
                permit,
                Arc::new(|_| {}),
            );
            match fault {
                0 => task.complete_audit(Ok(zero_native_snapshot())),
                1 => task.complete_audit(Err(ContextPortFailure::NativeRefused)),
                2 => drop(task),
                3 => {
                    let mut counts = zero_native_snapshot().counts();
                    counts.queued_tasks = 1;
                    task.complete_audit(
                        Ok(ContextNativeResourceSnapshot::try_new(counts).unwrap()),
                    );
                }
                4 => task.complete(ContextNativeEvent::ShutdownAuditSettled(
                    ContextShutdownAuditSettlement::new(
                        ContextResourceAuditId::new(2).unwrap(),
                        Ok(zero_native_snapshot()),
                    ),
                )),
                _ => unreachable!(),
            }
            assert!(factory.begin(|_| {}).is_err(), "fault {fault}");
            assert!(Arc::ptr_eq(&factory_admission(&factory), &admission));
            assert!(!admission.state.lock().unwrap().retired);
        }
    }

    #[test]
    fn lifetime_factory_retains_pending_native_and_physical_owners() {
        for physical in [false, true] {
            let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
            let mut factory = slot.take_factory().unwrap();
            let _port = factory.begin(|_| {}).unwrap();
            let admission = factory_admission(&factory);
            let (mut permit, capture) = if physical {
                let (permit, capture) = admission.reserve_screenshot().unwrap();
                (permit, Some(capture))
            } else {
                (admission.reserve().unwrap(), None)
            };
            if physical {
                permit.release();
            }
            settle_factory_native_shutdown(&factory);
            assert!(matches!(
                factory.begin(|_| {}),
                Err(ContextPortFailure::ProfileBusy)
            ));
            drop(permit);
            drop(capture);
            // A later empty count is not the missing exact shutdown receipt.
            assert!(matches!(
                factory.begin(|_| {}),
                Err(ContextPortFailure::ProfileBusy)
            ));
        }
    }

    #[test]
    fn later_audit_uncertainty_revokes_retirement_before_its_permit_releases() {
        for fault in 0..7 {
            let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
            let mut factory = slot.take_factory().unwrap();
            let _port = factory.begin(|_| {}).unwrap();
            settle_factory_native_shutdown(&factory);
            let old = factory_admission(&factory);
            let task = AgentContextTask::new(
                AgentPendingRequest::Audit(ContextResourceAuditId::new(2).unwrap()),
                old.reserve_audit().unwrap(),
                Arc::new(|_| {}),
            );
            assert!(factory.begin(|_| {}).is_err());
            match fault {
                0 => task.complete_audit(Ok(zero_native_snapshot())),
                1 => task.complete_audit(Err(ContextPortFailure::NativeRefused)),
                2 => task.refuse(ContextPortFailure::NativeRefused),
                3 => drop(task),
                4 => task.cancel_without_event(),
                5 => {
                    let mut counts = zero_native_snapshot().counts();
                    counts.queued_tasks = 1;
                    task.complete_audit(
                        Ok(ContextNativeResourceSnapshot::try_new(counts).unwrap()),
                    );
                }
                6 => task.complete(ContextNativeEvent::ResourceAuditSettled(
                    ContextResourceAuditSettlement::new(
                        ContextResourceAuditId::new(3).unwrap(),
                        Ok(zero_native_snapshot()),
                    ),
                )),
                _ => unreachable!(),
            }
            assert_eq!(factory.begin(|_| {}).is_ok(), fault == 0, "fault {fault}");
            if (1..6).contains(&fault) {
                // An ordinary retry cannot manufacture the missing shutdown receipt.
                AgentContextTask::new(
                    AgentPendingRequest::Audit(ContextResourceAuditId::new(4).unwrap()),
                    old.reserve_audit().unwrap(),
                    Arc::new(|_| {}),
                )
                .complete_audit(Ok(zero_native_snapshot()));
                assert!(factory.begin(|_| {}).is_err());
            }
        }
    }

    #[test]
    fn retired_lifetime_integrity_failure_blocks_its_successor_lineage() {
        let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        let mut factory = slot.take_factory().unwrap();
        let _old_port = factory.begin(|_| {}).unwrap();
        let old = factory_admission(&factory);
        settle_factory_native_shutdown(&factory);
        let current = factory.begin(|_| {}).unwrap();
        old.fail_invariant();
        assert_eq!(
            current.dispatch(construction_request()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert!(matches!(
            factory.begin(|_| {}),
            Err(ContextPortFailure::Shutdown)
        ));
    }

    #[test]
    fn successor_retirement_linearizes_with_old_read_only_audit_and_global_shutdown() {
        for _ in 0..32 {
            let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
            let mut factory = slot.take_factory().unwrap();
            let _port = factory.begin(|_| {}).unwrap();
            let old = factory_admission(&factory);
            settle_factory_native_shutdown(&factory);
            let racing = old.clone();
            let audit = std::thread::spawn(move || racing.reserve_audit());
            let next = factory.begin(|_| {});
            let permit = audit.join().unwrap();
            assert!(!(next.is_ok() && permit.is_ok()));
            drop(permit);
            if next.is_err() {
                assert!(factory.begin(|_| {}).is_ok());
            }
            assert!(old.state.lock().unwrap().retired);
            settle_factory_native_shutdown(&factory);
            let thread = std::thread::spawn(move || (factory.begin(|_| {}), factory));
            slot.seal();
            let (port, mut factory) = thread.join().unwrap();
            if let Ok(port) = port {
                assert_eq!(
                    port.dispatch(construction_request()),
                    ContextDispatch::Rejected(ContextPortFailure::Shutdown)
                );
            }
            assert!(matches!(
                factory.begin(|_| {}),
                Err(ContextPortFailure::Shutdown)
            ));
        }
    }

    #[test]
    fn panicking_slot_fatal_reporter_is_contained_and_runs_once() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|_| false),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                panic!("external slot fatal reporter panicked");
            }),
        );

        slot.report_fatal_once();
        slot.report_fatal_once();
        assert_eq!(reports.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn poisoned_slot_shutdown_still_seals_the_taken_port() {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = reports.clone();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let counted_dispatches = dispatches.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |_| {
                counted_dispatches.fetch_add(1, Ordering::Relaxed);
                false
            }),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = slot.state.lock().expect("slot state");
            panic!("poison slot state");
        }));

        slot.seal();
        assert_eq!(reports.load(Ordering::Relaxed), 1);
        let state = match slot.state.lock() {
            Ok(_) => panic!("slot must remain poison-marked"),
            Err(poisoned) => poisoned.into_inner(),
        };
        let admission = state.admission.as_ref().expect("taken admission");
        assert!(matches!(
            admission.reserve(),
            Err(ContextPortFailure::Shutdown)
        ));
        drop(state);
        drop(port);
        assert_eq!(dispatches.load(Ordering::Relaxed), 0);
        assert!(slot.take(Arc::new(|_| {})).is_none());
        assert_eq!(reports.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn cookie_task_retains_exact_request_and_post_admission_native_anchor() {
        let request = cookie_request();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let admission = Arc::new(AgentPortAdmission::new(Arc::new(|_| {})));
        let permit = admission.reserve().expect("permit");
        let before = std::time::Instant::now();
        let task = AgentContextTask::new(
            AgentPendingRequest::Cookie(request.clone()),
            permit,
            Arc::new(move |event| captured.lock().expect("events").push(event)),
        );
        let after = std::time::Instant::now();
        let (retained, admitted_at) = task.cookie().expect("cookie task");
        assert_eq!(retained, &request);
        assert!(admitted_at >= before && admitted_at <= after);
        task.refuse(ContextPortFailure::Cancelled);
        assert_eq!(admission.pending(), Some(0));
        let events = events.lock().expect("events");
        assert!(matches!(
            events.as_slice(),
            [ContextNativeEvent::CookieTransferSettled(settlement)]
                if settlement.request() == &request
                    && settlement.outcome()
                        == ContextCookieTransferOutcome::Refused(
                            ContextCookieTransferFailure::Cancelled,
                        )
        ));
    }

    #[test]
    fn rejected_outer_dispatch_releases_capacity_without_terminal_event() {
        let events = Arc::new(AtomicUsize::new(0));
        let counted = events.clone();
        let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        let port = slot
            .take(Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }))
            .expect("port");
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(events.load(Ordering::Relaxed), 0);
        let state = slot.state.lock().expect("state");
        assert_eq!(
            state
                .admission
                .as_ref()
                .and_then(|admission| admission.pending()),
            Some(0)
        );
    }

    #[test]
    fn panicking_outer_dispatch_is_rejected_and_fail_stopped_without_unwinding() {
        let events = Arc::new(AtomicUsize::new(0));
        let counted_events = events.clone();
        let fatals = Arc::new(AtomicUsize::new(0));
        let counted_fatals = fatals.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|_| panic!("external main-thread dispatcher panicked")),
            Arc::new(move |_| {
                counted_fatals.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot
            .take(Arc::new(move |_| {
                counted_events.fetch_add(1, Ordering::Relaxed);
            }))
            .expect("port");

        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(events.load(Ordering::Relaxed), 0);
        assert_eq!(fatals.load(Ordering::Relaxed), 1);
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(2).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        let state = slot.state.lock().expect("state");
        let admission = state.admission.as_ref().expect("admission");
        let admission_state = admission.state.lock().expect("admission state");
        assert_eq!(admission_state.pending, 0);
        assert!(admission_state.sealed);
        assert!(admission_state.invariant_failed);
    }

    #[test]
    fn post_execution_dispatch_panic_preserves_scheduled_terminal_and_seals() {
        crate::host::make_unavailable_for_test();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let fatals = Arc::new(AtomicUsize::new(0));
        let counted = fatals.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                panic!("external dispatcher panicked after synchronous execution");
            }),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot
            .take(Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }))
            .expect("port");

        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Scheduled
        );
        assert_eq!(fatals.load(Ordering::Relaxed), 1);
        let events = events.lock().expect("events");
        assert!(matches!(
            events.as_slice(),
            [ContextNativeEvent::ResourceAuditSettled(settlement)]
                if settlement.outcome() == Err(ContextPortFailure::Shutdown)
        ));
        drop(events);
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(2).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
    }

    #[test]
    fn accepted_dispatch_has_one_terminal_even_when_host_is_absent() {
        crate::host::make_unavailable_for_test();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot
            .take(Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }))
            .expect("port");
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Scheduled
        );
        let events = events.lock().expect("events");
        assert!(matches!(
            events.as_slice(),
            [ContextNativeEvent::ResourceAuditSettled(settlement)]
                if settlement.outcome() == Err(ContextPortFailure::Shutdown)
        ));
    }

    #[test]
    fn shutdown_barrier_is_distinct_exact_once_and_keeps_only_audits_open() {
        crate::host::make_unavailable_for_test();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot
            .take(Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }))
            .expect("port");

        assert_eq!(
            port.seal_for_shutdown(ContextResourceAuditId::new(41).expect("shutdown audit")),
            ContextShutdownDispatch::AuditScheduled
        );
        assert_eq!(
            port.seal_for_shutdown(ContextResourceAuditId::new(42).expect("repeat audit")),
            ContextShutdownDispatch::SealedWithoutAudit(ContextPortFailure::Shutdown)
        );
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(
            port.dispatch(construction_request()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(43).expect("drain audit")),
            ContextDispatch::Scheduled
        );

        let events = events.lock().expect("events");
        assert!(matches!(
            events.as_slice(),
            [
                ContextNativeEvent::ShutdownAuditSettled(shutdown),
                ContextNativeEvent::ResourceAuditSettled(drain),
            ] if shutdown.audit().get() == 41
                && shutdown.outcome() == Err(ContextPortFailure::Shutdown)
                && drain.audit().get() == 43
                && drain.outcome() == Err(ContextPortFailure::Shutdown)
        ));
    }

    #[test]
    fn queue_full_shutdown_still_seals_and_post_seal_audit_can_observe_drain() {
        let pending = Arc::new(Mutex::new(Vec::new()));
        let retained = pending.clone();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let counted = dispatches.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |task| {
                counted.fetch_add(1, Ordering::Relaxed);
                retained.lock().expect("pending tasks").push(task);
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        for raw in 1..=MAX_PENDING_NATIVE_CONTEXT_TASKS {
            assert_eq!(
                port.audit_resources(ContextResourceAuditId::new(raw as u64).expect("audit")),
                ContextDispatch::Scheduled
            );
        }

        assert_eq!(
            port.seal_for_shutdown(ContextResourceAuditId::new(100).expect("shutdown audit")),
            ContextShutdownDispatch::SealedWithoutAudit(ContextPortFailure::ResourceExhausted)
        );
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(
            port.dispatch(construction_request()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(
            dispatches.load(Ordering::Relaxed),
            MAX_PENDING_NATIVE_CONTEXT_TASKS
        );

        pending.lock().expect("pending tasks").pop();
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(101).expect("drain audit")),
            ContextDispatch::Scheduled
        );
        assert_eq!(
            dispatches.load(Ordering::Relaxed),
            MAX_PENDING_NATIVE_CONTEXT_TASKS + 1
        );
        assert_eq!(
            port.seal_for_shutdown(ContextResourceAuditId::new(102).expect("repeat audit")),
            ContextShutdownDispatch::SealedWithoutAudit(ContextPortFailure::Shutdown)
        );
    }

    #[test]
    fn rejected_shutdown_audit_dispatch_retains_the_permanent_seal() {
        let dispatches = Arc::new(AtomicUsize::new(0));
        let counted = dispatches.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                false
            }),
            Arc::new(|_| {}),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        assert_eq!(
            port.seal_for_shutdown(ContextResourceAuditId::new(1).expect("shutdown audit")),
            ContextShutdownDispatch::SealedWithoutAudit(ContextPortFailure::Shutdown)
        );
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(
            port.dispatch(construction_request()),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(2).expect("drain audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(dispatches.load(Ordering::Relaxed), 2);
        let state = slot.state.lock().expect("state");
        let admission = state.admission.as_ref().expect("admission");
        assert!(admission.state.lock().expect("admission state").sealed);
        assert_eq!(admission.pending(), Some(0));
    }

    #[test]
    fn shutdown_seal_linearizes_against_racing_mutation_admission() {
        for _ in 0..64 {
            let admission = Arc::new(AgentPortAdmission::new(Arc::new(|_| {})));
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let racing_admission = admission.clone();
            let racing_barrier = barrier.clone();
            let mutation = std::thread::spawn(move || {
                racing_barrier.wait();
                racing_admission.reserve()
            });

            barrier.wait();
            let shutdown = admission
                .reserve_shutdown_audit()
                .expect("shutdown audit reservation");
            let mutation = mutation.join().expect("mutation race");
            match mutation {
                Ok(permit) => drop(permit),
                Err(ContextPortFailure::Shutdown) => {}
                Err(failure) => panic!("unexpected mutation refusal: {failure:?}"),
            }
            assert!(matches!(
                admission.reserve(),
                Err(ContextPortFailure::Shutdown)
            ));
            drop(shutdown);
            let audit = admission.reserve_audit().expect("post-seal audit");
            drop(audit);
            assert_eq!(admission.pending(), Some(0));
        }
    }

    #[test]
    fn screenshot_dispatch_has_exactly_one_move_only_terminal_when_host_is_absent() {
        crate::host::make_unavailable_for_test();
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let captured = outcomes.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        let dispatch = port.capture_semantic_screenshot(
            screenshot_native_request(100),
            Box::new(move |outcome| {
                captured.lock().expect("outcomes").push(outcome.err());
            }),
        );
        #[cfg(target_os = "macos")]
        {
            assert_eq!(dispatch, ContextDispatch::Scheduled);
            assert_eq!(
                outcomes.lock().expect("outcomes").as_slice(),
                [Some(SemanticScreenshotNativeFailure::Shutdown)]
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(dispatch, ContextDispatch::Unsupported);
            assert!(outcomes.lock().expect("outcomes").is_empty());
        }
        let state = slot.state.lock().expect("state");
        let admission = state.admission.as_ref().expect("admission");
        assert_eq!(admission.counts(), Some((0, 0)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn rejected_screenshot_dispatch_releases_both_slots_without_callback() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let counted = callbacks.clone();
        let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        let port = slot.take(Arc::new(|_| {})).expect("port");
        assert_eq!(
            port.capture_semantic_screenshot(
                screenshot_native_request(200),
                Box::new(move |_| {
                    counted.fetch_add(1, Ordering::Relaxed);
                }),
            ),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(callbacks.load(Ordering::Relaxed), 0);
        let state = slot.state.lock().expect("state");
        assert_eq!(
            state
                .admission
                .as_ref()
                .and_then(|admission| admission.counts()),
            Some((0, 0))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn panicking_screenshot_dispatch_releases_both_slots_without_callback() {
        let callbacks = Arc::new(AtomicUsize::new(0));
        let counted_callbacks = callbacks.clone();
        let fatals = Arc::new(AtomicUsize::new(0));
        let counted_fatals = fatals.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|_| panic!("external screenshot dispatcher panicked")),
            Arc::new(move |_| {
                counted_fatals.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");

        assert_eq!(
            port.capture_semantic_screenshot(
                screenshot_native_request(250),
                Box::new(move |_| {
                    counted_callbacks.fetch_add(1, Ordering::Relaxed);
                }),
            ),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        assert_eq!(callbacks.load(Ordering::Relaxed), 0);
        assert_eq!(fatals.load(Ordering::Relaxed), 1);
        let state = slot.state.lock().expect("state");
        let admission = state.admission.as_ref().expect("admission");
        let admission_state = admission.state.lock().expect("admission state");
        assert_eq!(admission_state.pending, 0);
        assert_eq!(admission_state.physical_screenshots, 0);
        assert!(admission_state.sealed);
        assert!(admission_state.invariant_failed);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn panicking_screenshot_completion_is_fail_stopped_without_unwinding() {
        crate::host::make_unavailable_for_test();
        let fatals = Arc::new(AtomicUsize::new(0));
        let counted = fatals.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        assert_eq!(
            port.capture_semantic_screenshot(
                screenshot_native_request(300),
                Box::new(|_| panic!("external screenshot completion panicked")),
            ),
            ContextDispatch::Scheduled
        );
        assert_eq!(fatals.load(Ordering::Relaxed), 1);
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
    }

    #[test]
    fn semantic_invocation_uses_the_same_bounded_queue_and_exact_terminal_identity() {
        crate::host::make_unavailable_for_test();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot
            .take(Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }))
            .expect("port");
        let invocation = semantic_invocation(FrameId::MAIN);
        #[cfg(target_os = "macos")]
        let correlation = invocation.correlation();
        let dispatch = port.invoke_semantic(invocation);
        #[cfg(target_os = "macos")]
        {
            assert_eq!(dispatch, ContextDispatch::Scheduled);
            let events = events.lock().expect("events");
            assert!(matches!(
                events.as_slice(),
                [ContextNativeEvent::SemanticRuntimeSettled(settlement)]
                    if settlement.correlation() == &correlation
                        && settlement.outcome()
                            == &Err(SemanticRuntimePortFailure::Shutdown)
            ));
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(dispatch, ContextDispatch::Unsupported);
            assert!(events.lock().expect("events").is_empty());
        }
    }

    #[test]
    fn semantic_port_refuses_non_main_frames_before_native_admission() {
        let dispatches = Arc::new(AtomicUsize::new(0));
        let counted = dispatches.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        let child = FrameId::new(2).expect("child frame");
        assert_eq!(
            port.invoke_semantic(semantic_invocation(child)),
            ContextDispatch::Unsupported
        );
        assert_eq!(dispatches.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn native_support_is_closed_before_admission() {
        let dispatches = Arc::new(AtomicUsize::new(0));
        let counted = dispatches.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
                false
            }),
            Arc::new(|_| {}),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(supports_native_request(&recovery_request()));
        assert_eq!(
            supports_owned_transition(ContextOperationKind::Suspend),
            cfg!(target_os = "windows")
        );
        assert_eq!(
            supports_owned_transition(ContextOperationKind::Resume),
            cfg!(target_os = "windows")
        );
        let outcome = port.dispatch(construction_request());
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(
            outcome,
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(outcome, ContextDispatch::Unsupported);
        assert_eq!(
            dispatches.load(Ordering::Relaxed),
            usize::from(cfg!(any(target_os = "macos", target_os = "windows")))
        );
    }

    #[test]
    fn pending_admission_is_exactly_bounded_and_reopens_after_task_drop() {
        let pending = Arc::new(Mutex::new(Vec::new()));
        let retained = pending.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(move |task| {
                retained.lock().expect("pending tasks").push(task);
                true
            }),
            Arc::new(|_| {}),
        );
        let port = slot
            .take(Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }))
            .expect("port");

        for raw in 1..=MAX_PENDING_NATIVE_CONTEXT_TASKS {
            assert_eq!(
                port.audit_resources(ContextResourceAuditId::new(raw as u64).expect("audit")),
                ContextDispatch::Scheduled
            );
        }
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(17).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::ResourceExhausted)
        );
        assert_eq!(pending.lock().expect("pending tasks").len(), 16);

        pending.lock().expect("pending tasks").clear();
        assert_eq!(events.lock().expect("events").len(), 16);
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(18).expect("audit")),
            ContextDispatch::Scheduled
        );
        pending.lock().expect("pending tasks").clear();
        let events = events.lock().expect("events");
        assert_eq!(events.len(), 17);
        assert!(events.iter().all(|event| matches!(
            event,
            ContextNativeEvent::ResourceAuditSettled(settlement)
                if settlement.outcome() == Err(ContextPortFailure::NativeRefused)
        )));
    }

    #[test]
    fn physical_screenshot_debt_remains_bounded_after_logical_settlement() {
        let admission = Arc::new(AgentPortAdmission::new(Arc::new(|_| {})));
        let (logical_one, physical_one) = admission.reserve_screenshot().expect("first capture");
        let (logical_two, physical_two) = admission.reserve_screenshot().expect("second capture");
        drop(logical_one);
        drop(logical_two);

        assert!(matches!(
            admission.reserve_screenshot(),
            Err(ContextPortFailure::ResourceExhausted)
        ));
        {
            let state = admission.state.lock().expect("admission state");
            assert_eq!(state.pending, 0);
            assert_eq!(state.physical_screenshots, MAX_PENDING_SEMANTIC_SCREENSHOTS);
        }

        drop(physical_one);
        let (logical_three, physical_three) = admission
            .reserve_screenshot()
            .expect("released physical slot");
        drop(logical_three);
        drop(physical_two);
        drop(physical_three);
        let state = admission.state.lock().expect("admission state");
        assert_eq!(state.pending, 0);
        assert_eq!(state.physical_screenshots, 0);
    }

    #[test]
    fn poisoned_admission_fails_terminally_without_deadlocking_or_repeating_fatal() {
        let fatal = Arc::new(AtomicUsize::new(0));
        let counted = fatal.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|_| false),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        let admission = slot
            .state
            .lock()
            .expect("slot state")
            .admission
            .clone()
            .expect("admission");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = admission.state.lock().expect("admission state");
            panic!("poison admission");
        }));

        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
        slot.seal();
        assert_eq!(fatal.load(Ordering::Relaxed), 1);
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn rejected_native_callback_seals_admission_and_reports_fatal_once() {
        let fatal = Arc::new(AtomicUsize::new(0));
        let counted = fatal.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|_| false),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot.take(Arc::new(|_| {})).expect("port");
        let admission = slot
            .state
            .lock()
            .expect("slot state")
            .admission
            .clone()
            .expect("admission");
        let guard = AgentContextCallbackGuard {
            admission,
            sink: Arc::new(|_| {}),
        };
        guard.callback_dispatch_rejected();
        guard.callback_dispatch_rejected();
        assert_eq!(fatal.load(Ordering::Relaxed), 1);
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn retained_callback_guard_emits_only_closed_unsolicited_events() {
        let prior = construction_request().context();
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let sink: AgentContextEventSink = Arc::new(move |event| {
            captured.lock().expect("events").push(event);
        });
        let slot = AgentContextPortSlot::new(Arc::new(|_| false), Arc::new(|_| {}));
        let _port = slot.take(sink.clone()).expect("port");
        let admission = slot
            .state
            .lock()
            .expect("slot state")
            .admission
            .clone()
            .expect("admission");
        let guard = AgentContextCallbackGuard { admission, sink };
        guard.emit_renderer_lost(prior);
        let target =
            zephium_agentic::ContextNavigationTarget::parse("https://example.test/same-document")
                .expect("target");
        guard.emit_navigation_replaced(prior, target.clone());

        let events = events.lock().expect("events");
        assert!(
            matches!(events.first(), Some(ContextNativeEvent::RendererLost(loss)) if loss.prior() == prior)
        );
        assert!(matches!(
            events.get(1),
            Some(ContextNativeEvent::NavigationReplaced(replacement))
                if replacement.prior() == prior && replacement.target() == &target
        ));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn panicking_event_sink_cannot_unwind_a_native_callback() {
        crate::host::make_unavailable_for_test();
        let fatal = Arc::new(AtomicUsize::new(0));
        let counted = fatal.clone();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(move |_| {
                counted.fetch_add(1, Ordering::Relaxed);
            }),
        );
        let port = slot
            .take(Arc::new(|_| panic!("external event sink panicked")))
            .expect("port");
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Scheduled
        );
        assert_eq!(fatal.load(Ordering::Relaxed), 1);
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(2).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
    }

    #[test]
    fn panicking_fatal_reporter_cannot_escape_a_panicking_sink() {
        crate::host::make_unavailable_for_test();
        let slot = AgentContextPortSlot::new(
            Arc::new(|task| {
                task();
                true
            }),
            Arc::new(|_| panic!("external fatal reporter panicked")),
        );
        let port = slot
            .take(Arc::new(|_| panic!("external event sink panicked")))
            .expect("port");

        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Scheduled
        );
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(2).expect("audit")),
            ContextDispatch::Rejected(ContextPortFailure::Shutdown)
        );
    }
}
