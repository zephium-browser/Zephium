//! Optional application-owned Work admission, lifecycle and durable recovery.

#![deny(unsafe_code)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]

use crate::{CallbackHandle, Command};

#[cfg(feature = "work-execution-probe")]
#[path = "work_probe.rs"]
mod probe;
#[cfg(test)]
#[path = "work_tests.rs"]
mod tests;
use std::{
    collections::VecDeque,
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Wake, Waker},
    time::{Duration, Instant},
};
use zephium_agent_controller::{
    AgentBrowserModelAccess, AgentWorkController, AgentWorkEvent, AgentWorkFailure,
    AgentWorkHandle, AgentWorkOutcome, AgentWorkRunInput, AgentWorkTask,
};
use zephium_agent_provider_transport::AgentProviderTransportConfig;
use zephium_agent_runtime::{
    AgentRuntimeCompletion, AgentRuntimeConfig, AgentRuntimeHandle, AgentRuntimeStopReason,
    NativeEventSink, PendingAgentRuntime,
};
use zephium_agentic::*;

/// Trusted composition-root factory for the original one-shot engine port.
/// Invoked only after durable admission. It must not reopen a sealed port,
/// substitute ordinary tabs, or retain the sink as independent authority.
pub type AgentWorkNativeFactory =
    Box<dyn FnOnce(NativeEventSink) -> Option<Arc<dyn AgentBrowserPort>> + Send>;

/// Trusted non-UI execution ports; native factory is move-only.
pub struct AgentWorkApplicationPorts {
    engine: crate::SharedEngine,
    audit: Arc<dyn AgentAuditPort>,
    native: AgentWorkNativeFactory,
}

impl AgentWorkApplicationPorts {
    /// Binds the existing durable audit adapter and actual engine-port factory.
    pub fn new(
        engine: crate::SharedEngine,
        audit: Arc<dyn AgentAuditPort>,
        native: AgentWorkNativeFactory,
    ) -> Self {
        Self {
            engine,
            audit,
            native,
        }
    }
}

/// Existing runtime and provider settings, with no parallel model semantics.
#[derive(Clone)]
pub struct AgentWorkApplicationConfig {
    runtime: AgentRuntimeConfig,
    provider: AgentProviderTransportConfig,
}

impl AgentWorkApplicationConfig {
    pub(crate) fn into_parts(self) -> (AgentRuntimeConfig, AgentProviderTransportConfig) {
        (self.runtime, self.provider)
    }
    /// Uses the same bounded runtime and transport configuration as the actor.
    pub fn new(runtime: AgentRuntimeConfig, provider: AgentProviderTransportConfig) -> Self {
        Self { runtime, provider }
    }
}

/// Dormant controller ownership. Preparation starts no runtime or browser.
#[must_use]
pub struct PreparedAgentWork {
    engine: crate::SharedEngine,
    audit: Arc<dyn AgentAuditPort>,
    controller: Box<AgentWorkController>,
    handle: AgentWorkHandle,
    runtime: AgentRuntimeConfig,
    native: AgentWorkNativeFactory,
    deadline: Instant,
    run: ContextRunId,
    result_profile: Option<zephium_core::ids::ProfileId>,
    browser_profile: Option<crate::AgentWorkProfileBinding>,
}

impl PreparedAgentWork {
    /// Pins an actor-selected session and verifies that trusted input did not
    /// substitute its profile or storage class. Shell rechecks admission later.
    pub fn with_browser_profile(
        mut self,
        binding: crate::AgentWorkProfileBinding,
    ) -> Result<Self, AgentWorkFailure> {
        if self.controller.profile_storage_binding()?
            != (binding.profile(), binding.storage_class())
        {
            return Err(crate::contract_failure!());
        }
        self.browser_profile = Some(binding);
        Ok(self)
    }
    /// Builds the shipping stateless actor from trusted product inputs.
    pub fn try_new(
        input: AgentWorkRunInput,
        config: AgentWorkApplicationConfig,
        access: impl Into<AgentBrowserModelAccess>,
        task: Box<dyn AgentWorkTask>,
        ports: AgentWorkApplicationPorts,
    ) -> Result<Self, AgentWorkFailure> {
        let (controller, handle) = AgentWorkController::try_new(
            input,
            config.provider,
            access,
            ports.audit.clone(),
            task,
        )?;
        Self::from_controller(controller, handle, config.runtime, ports)
    }

    fn from_controller(
        controller: AgentWorkController,
        handle: AgentWorkHandle,
        runtime: AgentRuntimeConfig,
        ports: AgentWorkApplicationPorts,
    ) -> Result<Self, AgentWorkFailure> {
        let deadline = controller.deadline()?;
        if deadline <= Instant::now() {
            return Err(AgentWorkFailure::Deadline);
        }
        let run = controller.run_identity()?;
        let result_profile = controller.durable_result_profile()?;
        Ok(Self {
            engine: ports.engine,
            audit: ports.audit,
            controller: Box::new(controller),
            handle,
            runtime,
            native: ports.native,
            deadline,
            run,
            result_profile,
            browser_profile: None,
        })
    }
}

impl fmt::Debug for PreparedAgentWork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedAgentWork([owned, redacted])")
    }
}

/// Typed application phase; success is published only after durable terminal CAS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkApplicationPhase {
    Loading,
    Ready,
    Admitting,
    Running,
    Closing,
    NeedsReview,
    /// Exact review/cancellation CAS acknowledged after original failed-run
    /// drain. Only a separately admitted task/context may execute next.
    Reviewed,
    Recovery,
    PersistenceUncertain,
    Succeeded,
    /// Task failed after its original execution/lifecycle owners proved drain.
    Failed,
    /// Task was cancelled with all original execution/lifecycle owners drained.
    Cancelled,
    /// The run closed cleanly after requesting a person. No old execution
    /// authority remains; continuation requires a fresh admission.
    WaitingForHuman,
}

/// Human review never resumes an old proposal or clears execution debt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentWorkReviewDecision {
    AcceptFreshAdmission,
    Reject,
}

/// Compact product projection with no objective, model/page content or native handles.
#[derive(Clone, Copy, Debug)]
pub struct AgentWorkApplicationSnapshot {
    pub phase: AgentWorkApplicationPhase,
    pub run: Option<ContextRunId>,
    pub failure: Option<AgentWorkFailure>,
    pub persistence_failure: Option<AgentWorkJournalError>,
    pub last_review: Option<Result<AgentWorkRecord, AgentWorkJournalError>>,
    pub recovery_audit: Option<Result<AgentAuditLedgerStatus, AgentWorkFailure>>,
    /// Exact durable result identity after atomic publication or explicit read.
    pub artifact: Option<AgentWorkArtifactDescriptor>,
    /// Closed result-retrieval state; bodies never enter this projection.
    pub artifact_read: Option<Result<bool, AgentWorkJournalError>>,
}

struct Projection {
    archived: Option<AgentWorkArchivedExtraction>,
    extraction: Option<SemanticOwnedExtractionResult>,
    snapshot: AgentWorkApplicationSnapshot,
    records: Vec<AgentWorkRecord>,
    events: VecDeque<AgentWorkEvent>,
}

/// Stable shell port for future Work state/UI adapters. No native authority leaks.
#[derive(Clone)]
pub struct AgentWorkApplicationHandle {
    callback: CallbackHandle,
    projection: Arc<Mutex<Projection>>,
    admission: Arc<AtomicBool>,
}

impl AgentWorkApplicationHandle {
    /// Explicit profile-scoped retrieval after completion/restart, never replay.
    pub fn read_artifact(
        &self,
        record: AgentWorkRecord,
        profile: zephium_core::ids::ProfileId,
    ) -> bool {
        self.send_control(WorkControl::ReadArtifact { record, profile })
    }
    /// Moves one read result. Archived data cannot restore execution authority.
    pub fn take_archived_extraction(&self) -> Option<AgentWorkArchivedExtraction> {
        lock(&self.projection).archived.take()
    }
    /// Moves one explicitly model-mapped result only after the original clean
    /// lifecycle and durable terminal ACK. This is user-result content, never
    /// diagnostic data or factual proof. A durable-result opt-in additionally
    /// requires the atomic artifact publication ACK before this handoff.
    pub fn take_extraction(&self) -> Option<SemanticOwnedExtractionResult> {
        let mut projection = lock(&self.projection);
        if projection.snapshot.phase != AgentWorkApplicationPhase::Succeeded {
            return None;
        }
        projection.extraction.take()
    }
    /// Latest bounded content-free state.
    pub fn snapshot(&self) -> AgentWorkApplicationSnapshot {
        lock(&self.projection).snapshot
    }
    /// Explicit bounded durable recovery inventory; decoded facts are not authority.
    pub fn records(&self) -> Vec<AgentWorkRecord> {
        lock(&self.projection).records.clone()
    }
    /// Removes one exact ordered content-free execution event.
    pub fn take_event(&self) -> Option<AgentWorkEvent> {
        let event = lock(&self.projection).events.pop_front();
        if event.is_some() {
            let _ = self.callback.dispatch(Command::WorkWake);
        }
        event
    }
    /// Transfers at most one run to this application/native-port lifetime.
    pub fn admit(&self, run: PreparedAgentWork) -> Result<(), Box<PreparedAgentWork>> {
        if self.admission.swap(true, Ordering::AcqRel) {
            return Err(Box::new(run));
        }
        let submission = WorkSubmission(Arc::new(Mutex::new(Some(run))), self.projection.clone());
        if self
            .callback
            .dispatch(Command::AdmitWork(submission.clone()))
        {
            Ok(())
        } else if let Some(run) = lock(&submission.0).take() {
            self.admission.store(false, Ordering::Release);
            Err(Box::new(run))
        } else {
            Ok(())
        }
    }
    /// Requests exact-run revocation; this is not native takeover acknowledgement.
    pub fn stop(&self, run: ContextRunId, reason: AgentRuntimeStopReason) -> bool {
        self.send_control(WorkControl::Stop { run, reason })
    }
    /// Reviews an exact durable revision. Accepting requires fresh admission later.
    pub fn review(&self, record: AgentWorkRecord, decision: AgentWorkReviewDecision) -> bool {
        self.send_control(WorkControl::Review { record, decision })
    }
    /// Explicitly reconciles only a retained durable operation, never an action.
    pub fn reconcile(&self) -> bool {
        self.send_control(WorkControl::Reconcile)
    }

    fn send_control(&self, control: WorkControl) -> bool {
        self.callback
            .dispatch(Command::WorkControl(Box::new(WorkCommand {
                projection: self.projection.clone(),
                control,
            })))
    }
}

#[derive(Clone)]
pub struct WorkSubmission(
    Arc<Mutex<Option<PreparedAgentWork>>>,
    Arc<Mutex<Projection>>,
);
impl fmt::Debug for WorkSubmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkSubmission([one-shot])")
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WorkControl {
    ReadArtifact {
        record: AgentWorkRecord,
        profile: zephium_core::ids::ProfileId,
    },
    Stop {
        run: ContextRunId,
        reason: AgentRuntimeStopReason,
    },
    Review {
        record: AgentWorkRecord,
        decision: AgentWorkReviewDecision,
    },
    Reconcile,
}

#[derive(Clone)]
pub struct WorkCommand {
    projection: Arc<Mutex<Projection>>,
    control: WorkControl,
}
impl fmt::Debug for WorkCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.control.fmt(f)
    }
}

#[derive(Clone)]
pub struct WorkAttachment(Arc<Mutex<Option<ApplicationWork>>>);
impl fmt::Debug for WorkAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkAttachment([one-shot])")
    }
}

impl CallbackHandle {
    /// Lazily attaches the durable Work coordinator without creating an agent.
    pub fn attach_work(
        &self,
        journal: Arc<dyn AgentWorkJournalPort>,
        engine: crate::SharedEngine,
    ) -> Option<AgentWorkApplicationHandle> {
        self.attach_work_after(journal, engine, None)
    }

    /// Requests a new coordinator after an exact completed predecessor. The
    /// Shell independently checks original lifecycle/durable proof and all
    /// retained output/event lanes before replacing anything. No port reopens.
    pub fn attach_successor_work(
        &self,
        journal: Arc<dyn AgentWorkJournalPort>,
        engine: crate::SharedEngine,
        predecessor: &AgentWorkApplicationHandle,
    ) -> Option<AgentWorkApplicationHandle> {
        let projection = lock(&predecessor.projection);
        if !matches!(
            projection.snapshot.phase,
            AgentWorkApplicationPhase::Succeeded
                | AgentWorkApplicationPhase::Failed
                | AgentWorkApplicationPhase::Cancelled
                | AgentWorkApplicationPhase::WaitingForHuman
                | AgentWorkApplicationPhase::Reviewed
        ) {
            return None;
        }
        let run = projection.snapshot.run?;
        let record = projection
            .records
            .iter()
            .copied()
            .find(|record| record.key()[16..32] == run.bytes())?;
        drop(projection);
        self.attach_work_after(
            journal,
            engine,
            Some(WorkPredecessor {
                projection: predecessor.projection.clone(),
                record,
            }),
        )
    }

    fn attach_work_after(
        &self,
        journal: Arc<dyn AgentWorkJournalPort>,
        engine: crate::SharedEngine,
        predecessor: Option<WorkPredecessor>,
    ) -> Option<AgentWorkApplicationHandle> {
        let projection = Arc::new(Mutex::new(Projection {
            archived: None,
            extraction: None,
            snapshot: AgentWorkApplicationSnapshot {
                phase: AgentWorkApplicationPhase::Loading,
                run: None,
                failure: None,
                persistence_failure: None,
                last_review: None,
                recovery_audit: None,
                artifact: None,
                artifact_read: None,
            },
            records: Vec::new(),
            events: VecDeque::new(),
        }));
        let handle = AgentWorkApplicationHandle {
            callback: self.clone(),
            projection: projection.clone(),
            admission: Arc::new(AtomicBool::new(false)),
        };
        let mut actor = ApplicationWork::new(journal, engine, projection, self.clone());
        actor.predecessor = predecessor;
        if self.dispatch(Command::AttachWork(WorkAttachment(Arc::new(Mutex::new(
            Some(actor),
        ))))) {
            Some(handle)
        } else {
            None
        }
    }
}

struct WorkPredecessor {
    projection: Arc<Mutex<Projection>>,
    record: AgentWorkRecord,
}

struct ApplicationWake(CallbackHandle);
impl Wake for ApplicationWake {
    fn wake(self: Arc<Self>) {
        let _ = self.0.dispatch(Command::WorkWake);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let _ = self.0.dispatch(Command::WorkWake);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

struct ActiveWork {
    result_profile: Option<zephium_core::ids::ProfileId>,
    pending_event: Option<AgentWorkEvent>,
    needs_review: bool,
    handle: AgentWorkHandle,
    runtime: AgentRuntimeHandle,
    completion: AgentRuntimeCompletion,
    lifecycle: Option<Box<dyn AgentBrowserLifecycle>>,
    outcome: Option<AgentWorkOutcome>,
    native: Option<AgentNativeShutdownProof>,
    lifecycle_clean: Option<bool>,
}

struct RecoveryAuditFlight {
    proof: AgentAuditDeliveryProof,
    result: Arc<Mutex<Option<AgentAuditDeliverySettlement>>>,
    deadline: Instant,
    uncertain: bool,
}

#[derive(Clone)]
enum DurableRequest {
    Journal(AgentWorkJournalRequest),
    Artifact(AgentWorkArtifactRequest),
}
enum DurableReply {
    Journal(AgentWorkJournalReply),
    Artifact(AgentWorkArtifactReply),
}
type DurableResult = Result<DurableReply, AgentWorkJournalError>;

struct DurableSlot {
    result: Mutex<Option<DurableResult>>,
    ready: std::sync::Condvar,
}
impl DurableSlot {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            ready: std::sync::Condvar::new(),
        }
    }
    fn put(&self, result: DurableResult) {
        *lock(&self.result) = Some(result);
        self.ready.notify_all();
    }
    fn wait_until(&self, deadline: Instant) {
        let mut result = lock(&self.result);
        while result.is_none() && Instant::now() < deadline {
            result = self
                .ready
                .wait_timeout(result, deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
    }
}

struct DurableFlight {
    request: DurableRequest,
    result: Arc<DurableSlot>,
    deadline: Instant,
    uncertain: bool,
    reconciliations: u8,
    purpose: DurablePurpose,
}

#[derive(Clone, Copy)]
enum DurablePurpose {
    ArtifactPublish,
    ArtifactRead,
    Claim,
    Admit,
    Start,
    Terminal,
    Review,
}

pub(crate) struct ApplicationWork {
    predecessor: Option<WorkPredecessor>,
    engine: crate::SharedEngine,
    audit: Option<Arc<dyn AgentAuditPort>>,
    unstarted: Option<AgentWorkOutcome>,
    recovery_audit: Option<RecoveryAuditFlight>,
    audit_attempts: u8,
    journal: Arc<dyn AgentWorkJournalPort>,
    projection: Arc<Mutex<Projection>>,
    waker: Waker,
    owner: Option<AgentWorkIncarnation>,
    staged: Option<PreparedAgentWork>,
    active: Option<ActiveWork>,
    flight: Option<DurableFlight>,
    record: Option<AgentWorkRecord>,
    stopping: bool,
    used: bool,
    artifact_preparation_failed: bool,
}

impl ApplicationWork {
    /// A new coordinator can only replace the exact preceding closed owner.
    /// Decoded facts, a business-only outcome or a zero native count alone do
    /// not authorize replacement. The Shell also checks exact Engine/Store.
    pub(crate) fn accepts_predecessor(&self, previous: Option<&Self>) -> bool {
        let Some(expected) = &self.predecessor else {
            return previous.is_none();
        };
        let Some(previous) = previous else {
            return false;
        };
        if !Arc::ptr_eq(&expected.projection, &previous.projection)
            || previous.record != Some(expected.record)
            || expected.record.debt() != AgentWorkDebt::NONE
            || previous.flight.is_some()
            || previous.staged.is_some()
            || previous.unstarted.is_some()
            || previous.recovery_audit.is_some()
            || previous.artifact_preparation_failed
        {
            return false;
        }
        let Some(active) = &previous.active else {
            return false;
        };
        if active.lifecycle_clean != Some(true)
            || active.native.is_none()
            || active.lifecycle.is_some()
            || !active.completion.is_stopped()
            || active.pending_event.is_some()
            || active.handle.has_pending_events()
        {
            return false;
        }
        let expected_phase = match (&active.outcome, expected.record.disposition()) {
            (Some(AgentWorkOutcome::Succeeded(success)), AgentWorkDisposition::Succeeded)
                if success.extraction().is_none() =>
            {
                AgentWorkApplicationPhase::Succeeded
            }
            (
                Some(AgentWorkOutcome::ClosedUnsuccessfully(closed)),
                AgentWorkDisposition::Failed,
            ) if matches!(
                closed.policy_settlement().closure().outcome(),
                AgentRunProgressOutcome::Failed(_)
            ) =>
            {
                AgentWorkApplicationPhase::Failed
            }
            (
                Some(AgentWorkOutcome::ClosedUnsuccessfully(closed)),
                AgentWorkDisposition::Cancelled,
            ) if matches!(
                closed.policy_settlement().closure().outcome(),
                AgentRunProgressOutcome::Cancelled(_)
            ) =>
            {
                AgentWorkApplicationPhase::Cancelled
            }
            (
                Some(AgentWorkOutcome::ClosedUnsuccessfully(closed)),
                AgentWorkDisposition::FreshAdmissionRequired
                | AgentWorkDisposition::Rejected
                | AgentWorkDisposition::FailedClosed,
            ) if closed.human_review().is_some() => AgentWorkApplicationPhase::Reviewed,
            (
                Some(AgentWorkOutcome::WaitingForHuman(waiting)),
                AgentWorkDisposition::WaitingForHuman,
            ) if matches!(
                waiting.policy_settlement().closure().outcome(),
                AgentRunProgressOutcome::Failed(AgentSupervisorFailure::PolicyDenied)
            ) =>
            {
                AgentWorkApplicationPhase::WaitingForHuman
            }
            _ => return false,
        };
        let projection = lock(&previous.projection);
        projection.snapshot.phase == expected_phase
            && projection.snapshot.persistence_failure.is_none()
            && projection.events.is_empty()
            && projection.extraction.is_none()
            && projection.archived.is_none()
    }

    /// Old handles remain terminal, but cannot retain quadratic copies of the
    /// process inventory. The new coordinator reloads the same durable Store;
    /// no execution or uncertain persistence owner is discarded here.
    pub(crate) fn retire_projection(&self) {
        lock(&self.projection).records = self.record.into_iter().collect();
    }
    pub(crate) fn belongs_to_engine(&self, engine: &crate::SharedEngine) -> bool {
        Arc::ptr_eq(&self.engine, engine)
    }
    pub(crate) fn belongs_to_store(&self, store: &crate::SharedStore) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.journal), Arc::as_ptr(store))
    }
    fn new(
        journal: Arc<dyn AgentWorkJournalPort>,
        engine: crate::SharedEngine,
        projection: Arc<Mutex<Projection>>,
        callback: CallbackHandle,
    ) -> Self {
        Self {
            predecessor: None,
            engine,
            audit: None,
            unstarted: None,
            recovery_audit: None,
            audit_attempts: 0,
            journal,
            projection,
            waker: Waker::from(Arc::new(ApplicationWake(callback))),
            owner: None,
            staged: None,
            active: None,
            flight: None,
            record: None,
            stopping: false,
            used: false,
            artifact_preparation_failed: false,
        }
    }

    pub(crate) fn take_attachment(attachment: &WorkAttachment) -> Option<Self> {
        lock(&attachment.0).take()
    }

    pub(crate) fn initialize(&mut self) {
        // Do not retain a chain of old handles after the exact Shell join.
        self.predecessor.take();
        self.dispatch(AgentWorkJournalRequest::Claim, DurablePurpose::Claim, 0);
    }

    pub(crate) fn refuse_attachment(&mut self) {
        self.fail(crate::contract_failure!());
    }

    pub(crate) fn admit(
        &mut self,
        submission: WorkSubmission,
        profile: Option<crate::AgentWorkProfileReadiness>,
    ) {
        if !Arc::ptr_eq(&self.projection, &submission.1) {
            return;
        }
        let Some(run) = lock(&submission.0).take() else {
            return;
        };
        if !Arc::ptr_eq(&run.engine, &self.engine)
            || !std::ptr::addr_eq(Arc::as_ptr(&run.audit), Arc::as_ptr(&self.journal))
        {
            self.fail(crate::contract_failure!());
            return;
        }
        if self.used || self.stopping {
            return;
        }
        self.used = true;
        self.audit = Some(run.audit.clone());
        let profile_valid = run.browser_profile.is_none_or(|binding| {
            profile == Some(crate::AgentWorkProfileReadiness::Ready(binding))
        });
        lock(&self.projection).snapshot.run = Some(run.run);
        self.staged = Some(run);
        if !profile_valid {
            self.stopping = true;
            self.fail(crate::contract_failure!());
            self.abort_staged();
        }
        self.poll();
    }

    fn dispatch(
        &mut self,
        request: AgentWorkJournalRequest,
        purpose: DurablePurpose,
        reconciliations: u8,
    ) {
        self.dispatch_request(DurableRequest::Journal(request), purpose, reconciliations);
    }

    fn dispatch_request(
        &mut self,
        request: DurableRequest,
        purpose: DurablePurpose,
        reconciliations: u8,
    ) {
        let result = Arc::new(DurableSlot::new());
        let sink = result.clone();
        let waker = self.waker.clone();
        let deadline = Instant::now() + Duration::from_secs(2);
        let flight = DurableFlight {
            request: request.clone(),
            result: result.clone(),
            deadline,
            uncertain: false,
            reconciliations,
            purpose,
        };
        self.flight = Some(flight);
        let dispatch = match request {
            DurableRequest::Journal(request) => self.journal.dispatch(
                request,
                Box::new(move |value| {
                    sink.put(value.map(DurableReply::Journal));
                    waker.wake();
                }),
            ),
            DurableRequest::Artifact(request) => self.journal.artifact(
                request,
                Box::new(move |value| {
                    sink.put(value.map(DurableReply::Artifact));
                    waker.wake();
                }),
            ),
        };
        if let Err(error) = dispatch {
            result.put(Err(error));
        }
    }

    fn durable_uncertain(&mut self, error: AgentWorkJournalError) {
        if self.active.is_none() {
            self.stopping = true;
            self.abort_staged();
        }
        let mut projection = lock(&self.projection);
        projection.snapshot.phase = AgentWorkApplicationPhase::PersistenceUncertain;
        projection.snapshot.persistence_failure = Some(error);
        drop(projection);
        self.stop_active(AgentRuntimeStopReason::Cancelled);
    }

    fn remember(&mut self, record: AgentWorkRecord) -> bool {
        zephium_agentic::remember_agent_work_record(&mut lock(&self.projection).records, record)
    }

    fn acknowledge(
        &mut self,
        purpose: DurablePurpose,
        request: DurableRequest,
        reply: DurableReply,
    ) -> Result<(), AgentWorkJournalError> {
        match (request, reply) {
            (DurableRequest::Journal(request), DurableReply::Journal(reply)) => {
                self.acknowledge_journal(purpose, request, reply)
            }
            (
                DurableRequest::Artifact(AgentWorkArtifactRequest::Publish(publication)),
                DurableReply::Artifact(AgentWorkArtifactReply::Published { record, descriptor }),
            ) if matches!(purpose, DurablePurpose::ArtifactPublish)
                && record == publication.mutation().next()
                && descriptor == publication.descriptor() =>
            {
                if !self.remember(record) {
                    return Err(AgentWorkJournalError::Capacity);
                }
                self.record = Some(record);
                lock(&self.projection).snapshot.artifact = Some(descriptor);
                self.finish_terminal(record);
                Ok(())
            }
            (
                DurableRequest::Artifact(AgentWorkArtifactRequest::Read {
                    record, profile, ..
                }),
                DurableReply::Artifact(AgentWorkArtifactReply::Read(result)),
            ) if matches!(purpose, DurablePurpose::ArtifactRead) => {
                if result.as_ref().is_some_and(|result| {
                    result.descriptor().key() != record.key()
                        || result.descriptor().profile() != profile
                }) {
                    return Err(AgentWorkJournalError::Conflict);
                }
                let mut projection = lock(&self.projection);
                if projection.archived.is_some() {
                    return Err(AgentWorkJournalError::Capacity);
                }
                projection.snapshot.artifact = result.as_ref().map(|result| result.descriptor());
                projection.snapshot.artifact_read = Some(Ok(result.is_some()));
                projection.archived = result;
                // A historical read changes only its own result lane, never
                // a prior failed/cancelled/successful execution disposition.
                Ok(())
            }
            _ => Err(AgentWorkJournalError::Conflict),
        }
    }

    fn finish_terminal(&mut self, record: AgentWorkRecord) {
        if record.disposition() == AgentWorkDisposition::Succeeded {
            if let Some(AgentWorkOutcome::Succeeded(success)) = self
                .active
                .as_mut()
                .and_then(|active| active.outcome.as_mut())
            {
                lock(&self.projection).extraction = success.take_extraction();
            }
        }
        lock(&self.projection).snapshot.phase = match record.disposition() {
            AgentWorkDisposition::Succeeded => AgentWorkApplicationPhase::Succeeded,
            AgentWorkDisposition::Failed => AgentWorkApplicationPhase::Failed,
            AgentWorkDisposition::Cancelled => AgentWorkApplicationPhase::Cancelled,
            AgentWorkDisposition::WaitingForHuman => AgentWorkApplicationPhase::WaitingForHuman,
            AgentWorkDisposition::NeedsApproval => AgentWorkApplicationPhase::NeedsReview,
            AgentWorkDisposition::FreshAdmissionRequired
            | AgentWorkDisposition::Rejected
            | AgentWorkDisposition::FailedClosed
                if record.debt() == AgentWorkDebt::NONE =>
            {
                AgentWorkApplicationPhase::Reviewed
            }
            _ => AgentWorkApplicationPhase::Recovery,
        };
        self.abort_staged();
    }

    fn acknowledge_journal(
        &mut self,
        purpose: DurablePurpose,
        request: AgentWorkJournalRequest,
        reply: AgentWorkJournalReply,
    ) -> Result<(), AgentWorkJournalError> {
        match (purpose, request, reply) {
            (
                DurablePurpose::Claim,
                AgentWorkJournalRequest::Claim,
                AgentWorkJournalReply::Claimed { owner, records },
            ) => {
                if records.len() > zephium_agentic::MAX_AGENT_WORK_INVENTORY
                    || records
                        .windows(2)
                        .any(|pair| pair[0].key() >= pair[1].key())
                    || records.iter().any(|record| {
                        !record.disposition().is_terminal() && record.incarnation() != owner
                    })
                {
                    return Err(AgentWorkJournalError::Conflict);
                }
                self.owner = Some(owner);
                let mut projection = lock(&self.projection);
                projection.records = records;
                projection.snapshot.phase = if self.stopping {
                    AgentWorkApplicationPhase::Recovery
                } else {
                    AgentWorkApplicationPhase::Ready
                };
                Ok(())
            }
            (
                purpose,
                AgentWorkJournalRequest::CompareAndSet(mutation),
                AgentWorkJournalReply::Record(Some(record)),
            ) if record == mutation.next() => {
                if !self.remember(record) {
                    return Err(AgentWorkJournalError::Capacity);
                }
                if !matches!(purpose, DurablePurpose::Review) {
                    self.record = Some(record);
                }
                match purpose {
                    DurablePurpose::Admit => {
                        let disposition = if self.stopping {
                            AgentWorkDisposition::FailedClosed
                        } else {
                            AgentWorkDisposition::Running
                        };
                        let next = AgentWorkJournalMutation::transition(record, disposition)?;
                        self.dispatch(
                            AgentWorkJournalRequest::CompareAndSet(next),
                            if self.stopping {
                                DurablePurpose::Terminal
                            } else {
                                DurablePurpose::Start
                            },
                            0,
                        );
                    }
                    DurablePurpose::Start => self.activate(),
                    DurablePurpose::Terminal => {
                        self.finish_terminal(record);
                    }
                    DurablePurpose::Review => {
                        if self.record.is_some_and(|prior| prior.key() == record.key()) {
                            self.record = Some(record);
                            self.finish_terminal(record);
                        }
                        lock(&self.projection).snapshot.last_review = Some(Ok(record));
                    }
                    DurablePurpose::Claim
                    | DurablePurpose::ArtifactPublish
                    | DurablePurpose::ArtifactRead => return Err(AgentWorkJournalError::Conflict),
                }
                Ok(())
            }
            _ => Err(AgentWorkJournalError::Conflict),
        }
    }

    pub(crate) fn poll(&mut self) {
        if self
            .staged
            .as_ref()
            .is_some_and(|staged| Instant::now() >= staged.deadline)
        {
            self.fail(AgentWorkFailure::Deadline);
            self.abort_staged();
        }
        let mut continue_audit = false;
        if let Some(flight) = self.flight.take() {
            let result = lock(&flight.result.result).take();
            match result {
                Some(Ok(reply)) => {
                    if let Err(error) =
                        self.acknowledge(flight.purpose, flight.request.clone(), reply)
                    {
                        self.refuse_durable_flight(flight, error);
                    } else {
                        lock(&self.projection).snapshot.persistence_failure = None;
                    }
                }
                Some(Err(error)) => {
                    self.refuse_durable_flight(flight, error);
                }
                None => {
                    if !flight.uncertain && Instant::now() >= flight.deadline {
                        self.refuse_durable_flight(flight, AgentWorkJournalError::Uncertain);
                    } else {
                        self.flight = Some(flight);
                    }
                }
            }
        }
        if let Some(mut flight) = self.recovery_audit.take() {
            let result = lock(&flight.result).take();
            if let Some(settlement) = result {
                let status = if settlement.proof() != flight.proof {
                    Err(AgentWorkFailure::Audit)
                } else if let Some(recovery) = self.recovery_mut() {
                    match recovery.settle_audit_reconciliation(settlement) {
                        Ok(AgentAuditDeliveryOutcome::Committed) => {
                            recovery.audit_reconciliation_status()
                        }
                        _ => Err(AgentWorkFailure::Audit),
                    }
                } else {
                    Err(crate::contract_failure!())
                };
                continue_audit = status.is_ok_and(|status| status.pending() > 0);
                lock(&self.projection).snapshot.recovery_audit = Some(status);
            } else {
                if Instant::now() >= flight.deadline {
                    flight.uncertain = true;
                    lock(&self.projection).snapshot.recovery_audit =
                        Some(Err(AgentWorkFailure::Audit));
                }
                self.recovery_audit = Some(flight);
            }
        }
        if let Some(active) = &mut self.active {
            while let Some(event) = active
                .pending_event
                .take()
                .or_else(|| active.handle.take_event())
            {
                if matches!(
                    event.kind(),
                    zephium_agent_controller::AgentWorkEventKind::NeedsHuman(_)
                ) {
                    active.needs_review = true;
                }
                let mut projection = lock(&self.projection);
                if projection.events.len() == zephium_agent_controller::MAX_AGENT_WORK_EVENTS {
                    active.pending_event = Some(event);
                    active
                        .runtime
                        .stop_and_seal(AgentRuntimeStopReason::Cancelled);
                    projection.snapshot.failure = Some(AgentWorkFailure::Backpressure);
                    break;
                }
                projection.events.push_back(event);
            }
            if active.completion.is_stopped() && active.lifecycle.is_some() {
                if let Some(lifecycle) = active.lifecycle.take() {
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        lifecycle.shutdown_until(Instant::now() + Duration::from_millis(100))
                    }));
                    match outcome {
                        Ok(AgentBrowserShutdownOutcome::Clean(native)) => {
                            active.native = Some(native);
                            active.lifecycle_clean = Some(true);
                        }
                        _ => {
                            active.lifecycle_clean = Some(false);
                        }
                    }
                }
            }
            // An unclean, deadline-bounded lifecycle join can return before
            // the worker's final Drop publishes its recovery owner. Consume
            // that exact late outcome on completion; never replace or lose it.
            if active.completion.is_stopped()
                && active.lifecycle.is_none()
                && active.outcome.is_none()
            {
                active.outcome = active.handle.take_outcome();
                if let Some(AgentWorkOutcome::Recovery(recovery)) = &active.outcome {
                    lock(&self.projection).snapshot.failure = Some(recovery.failure());
                } else if let Some(AgentWorkOutcome::ClosedUnsuccessfully(closed)) = &active.outcome
                {
                    lock(&self.projection).snapshot.failure = Some(closed.failure());
                }
            }
        }
        if continue_audit {
            self.reconcile_audit();
        }
        if self.flight.is_some() {
            return;
        }
        if let Some(owner) = self.owner {
            if self.record.is_none() && (self.staged.is_some() || self.unstarted.is_some()) {
                let admission = if let Some(staged) = &self.staged {
                    staged.controller.journal_admission(owner)
                } else if let Some(AgentWorkOutcome::Recovery(recovery)) = &self.unstarted {
                    recovery.journal_admission(owner)
                } else {
                    Err(crate::contract_failure!())
                };
                match admission {
                    Ok(mutation) => {
                        lock(&self.projection).snapshot.phase =
                            AgentWorkApplicationPhase::Admitting;
                        self.dispatch(
                            AgentWorkJournalRequest::CompareAndSet(mutation),
                            DurablePurpose::Admit,
                            0,
                        );
                    }
                    Err(error) => self.fail(error),
                }
                return;
            }
        }
        self.persist_execution_end();
    }

    fn activate(&mut self) {
        if self.stopping {
            self.abort_staged();
            return;
        }
        let Some(mut staged) = self.staged.take() else {
            self.fail(crate::contract_failure!());
            return;
        };
        if self.stopping || Instant::now() >= staged.deadline {
            drop(staged.controller);
            self.unstarted = staged.handle.take_outcome();
            self.fail(AgentWorkFailure::Deadline);
            return;
        }
        staged.handle.set_waker(self.waker.clone());
        let pending = match PendingAgentRuntime::spawn_suspended_with_controller(
            staged.runtime,
            staged.controller,
        ) {
            Ok(pending) => pending,
            Err(_) => {
                self.unstarted = staged.handle.take_outcome();
                self.fail(AgentWorkFailure::Shutdown);
                return;
            }
        };
        let sink = pending.native_event_sink();
        let browser = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            (staged.native)(sink)
        })) {
            Ok(Some(browser)) => browser,
            _ => {
                drop(pending);
                self.unstarted = staged.handle.take_outcome();
                self.fail(AgentWorkFailure::Context);
                return;
            }
        };
        let (runtime, completion, lifecycle) = pending.bind_browser_port(browser).into_parts();
        completion.set_waker(self.waker.clone());
        self.active = Some(ActiveWork {
            result_profile: staged.result_profile,
            pending_event: None,
            needs_review: false,
            handle: staged.handle,
            runtime,
            completion,
            lifecycle: Some(lifecycle),
            outcome: None,
            native: None,
            lifecycle_clean: None,
        });
        if let Some(active) = &self.active {
            if active.runtime.start_run().is_err() {
                self.fail(AgentWorkFailure::Shutdown);
                return;
            }
        }
        lock(&self.projection).snapshot.phase = AgentWorkApplicationPhase::Running;
    }

    fn persist_execution_end(&mut self) {
        if self.artifact_preparation_failed {
            return;
        }
        let Some(record) = self.record else {
            return;
        };
        // Stop/shutdown may race either side of the nonterminal review ACK.
        // This poll runs only with no durable flight; reconcile the exact ACK
        // first, then close review without rewriting the failed policy outcome.
        if self.stopping && record.disposition() == AgentWorkDisposition::NeedsApproval {
            if let Ok(mutation) =
                AgentWorkJournalMutation::transition(record, AgentWorkDisposition::FailedClosed)
            {
                lock(&self.projection).snapshot.phase = AgentWorkApplicationPhase::Closing;
                self.dispatch(
                    AgentWorkJournalRequest::CompareAndSet(mutation),
                    DurablePurpose::Terminal,
                    0,
                );
            }
            return;
        }
        if record.disposition() != AgentWorkDisposition::Running {
            return;
        }
        let mutation = if let Some(active) = &self.active {
            if active.lifecycle_clean.is_none() {
                return;
            }
            match (&active.outcome, &active.native, active.lifecycle_clean) {
                (Some(AgentWorkOutcome::Succeeded(policy)), Some(native), Some(true))
                    if lock(&self.projection).snapshot.failure.is_none() =>
                {
                    AgentWorkJournalMutation::completed(record, policy.policy_settlement(), native)
                }
                (
                    Some(AgentWorkOutcome::ClosedUnsuccessfully(closed)),
                    Some(native),
                    Some(true),
                ) => match closed.human_review().filter(|_| !self.stopping) {
                    Some(review) => AgentWorkJournalMutation::needs_approval_closed(
                        record,
                        closed.policy_settlement(),
                        native,
                        review,
                    ),
                    None => AgentWorkJournalMutation::closed_unsuccessfully(
                        record,
                        closed.policy_settlement(),
                        native,
                    ),
                },
                (Some(AgentWorkOutcome::WaitingForHuman(waiting)), Some(native), Some(true)) => {
                    AgentWorkHumanHandoff::try_new(
                        waiting.request().reason(),
                        waiting.request().observation(),
                    )
                    .and_then(|handoff| {
                        AgentWorkJournalMutation::waiting_for_human(
                            record,
                            waiting.policy_settlement(),
                            native,
                            handoff,
                        )
                    })
                }
                _ => AgentWorkJournalMutation::transition(
                    record,
                    if active.needs_review && !self.stopping {
                        AgentWorkDisposition::NeedsApproval
                    } else {
                        AgentWorkDisposition::RecoveryRequired
                    },
                ),
            }
        } else if self.stopping {
            AgentWorkJournalMutation::transition(record, AgentWorkDisposition::FailedClosed)
        } else {
            return;
        };
        match mutation {
            Ok(mutation) => {
                lock(&self.projection).snapshot.phase = AgentWorkApplicationPhase::Closing;
                if mutation.next().disposition() == AgentWorkDisposition::Succeeded {
                    if let Some(profile) = self
                        .active
                        .as_ref()
                        .and_then(|active| active.result_profile)
                    {
                        let publication = self
                            .active
                            .as_ref()
                            .and_then(|active| active.outcome.as_ref())
                            .and_then(|outcome| match outcome {
                                AgentWorkOutcome::Succeeded(success) => success.extraction(),
                                _ => None,
                            })
                            .ok_or(AgentWorkJournalError::Transition)
                            .and_then(|result| {
                                AgentWorkArtifactPublication::prepare(mutation, profile, result)
                            });
                        match publication {
                            Ok(publication) => self.dispatch_request(
                                DurableRequest::Artifact(AgentWorkArtifactRequest::Publish(
                                    Arc::new(publication),
                                )),
                                DurablePurpose::ArtifactPublish,
                                0,
                            ),
                            Err(error) => {
                                self.artifact_preparation_failed = true;
                                self.durable_uncertain(error);
                            }
                        }
                        return;
                    }
                }
                self.dispatch(
                    AgentWorkJournalRequest::CompareAndSet(mutation),
                    DurablePurpose::Terminal,
                    0,
                );
            }
            Err(error) => self.durable_uncertain(error),
        }
    }

    fn fail(&mut self, failure: AgentWorkFailure) {
        self.stopping = true;
        let mut projection = lock(&self.projection);
        projection.snapshot.failure = Some(failure);
        projection.snapshot.phase = AgentWorkApplicationPhase::Recovery;
        drop(projection);
        self.stop_active(AgentRuntimeStopReason::Cancelled);
    }

    fn refuse_durable_flight(&mut self, mut flight: DurableFlight, error: AgentWorkJournalError) {
        if matches!(flight.purpose, DurablePurpose::ArtifactRead) {
            // A read has no publication or execution debt. Refusal/timeout must
            // not consume future admission or replace an unread result. A late
            // callback retains only its original disconnected bounded slot.
            lock(&self.projection).snapshot.artifact_read = Some(Err(error));
        } else {
            flight.uncertain = true;
            self.flight = Some(flight);
            self.durable_uncertain(error);
        }
    }

    fn abort_staged(&mut self) {
        if let Some(staged) = self.staged.take() {
            let PreparedAgentWork {
                controller,
                mut handle,
                ..
            } = staged;
            // The existing controller Drop clears credentials/objective and
            // publishes its original recovery owner; do not discard that owner.
            drop(controller);
            self.unstarted = handle.take_outcome();
        }
    }

    fn recovery_mut(&mut self) -> Option<&mut zephium_agent_controller::AgentWorkRecovery> {
        let outcome = if let Some(active) = &mut self.active {
            active.outcome.as_mut()
        } else {
            self.unstarted.as_mut()
        };
        match outcome {
            Some(AgentWorkOutcome::Recovery(recovery)) => Some(recovery),
            _ => None,
        }
    }

    fn stop_active(&self, reason: AgentRuntimeStopReason) {
        if let Some(active) = &self.active {
            active.runtime.stop_and_seal(reason);
        }
    }

    pub(crate) fn control(&mut self, command: WorkCommand) {
        if !Arc::ptr_eq(&self.projection, &command.projection) {
            return;
        }
        match command.control {
            WorkControl::ReadArtifact { record, profile } => {
                let error = if self.stopping {
                    Some(AgentWorkJournalError::Shutdown)
                } else if self.flight.is_some()
                    || self.recovery_audit.is_some()
                    || lock(&self.projection).archived.is_some()
                {
                    Some(AgentWorkJournalError::Capacity)
                } else if !matches!(
                    lock(&self.projection).snapshot.phase,
                    AgentWorkApplicationPhase::Ready
                        | AgentWorkApplicationPhase::Succeeded
                        | AgentWorkApplicationPhase::Failed
                        | AgentWorkApplicationPhase::Cancelled
                        | AgentWorkApplicationPhase::WaitingForHuman
                ) || record.disposition() != AgentWorkDisposition::Succeeded
                    || !lock(&self.projection).records.contains(&record)
                {
                    Some(AgentWorkJournalError::Conflict)
                } else {
                    None
                };
                if let Some(error) = error {
                    lock(&self.projection).snapshot.artifact_read = Some(Err(error));
                } else if let Some(owner) = self.owner {
                    lock(&self.projection).snapshot.artifact_read = None;
                    self.dispatch_request(
                        DurableRequest::Artifact(AgentWorkArtifactRequest::Read {
                            owner,
                            record,
                            profile,
                        }),
                        DurablePurpose::ArtifactRead,
                        0,
                    );
                }
            }
            WorkControl::Stop { run, reason } => {
                // The first retained proof-bearing terminal CAS owns publication.
                // Its native/runtime/provider owners are already clean; a late
                // stop cannot rewrite that immutable outcome or disable reads.
                let terminal_owned = self
                    .record
                    .is_some_and(|record| record.disposition().is_terminal())
                    || self
                        .flight
                        .as_ref()
                        .is_some_and(|flight| match &flight.request {
                            DurableRequest::Journal(AgentWorkJournalRequest::CompareAndSet(
                                mutation,
                            )) => matches!(
                                mutation.next().disposition(),
                                AgentWorkDisposition::Succeeded
                                    | AgentWorkDisposition::Failed
                                    | AgentWorkDisposition::Cancelled
                                    | AgentWorkDisposition::FreshAdmissionRequired
                                    | AgentWorkDisposition::Rejected
                                    | AgentWorkDisposition::FailedClosed
                            ),
                            DurableRequest::Artifact(AgentWorkArtifactRequest::Publish(_)) => true,
                            _ => false,
                        });
                if lock(&self.projection).snapshot.run == Some(run) && !terminal_owned {
                    self.stopping = true;
                    self.stop_active(reason);
                    self.abort_staged();
                    if self.active.is_none() {
                        lock(&self.projection)
                            .snapshot
                            .failure
                            .get_or_insert(match reason {
                                AgentRuntimeStopReason::Cancelled => AgentWorkFailure::Cancelled,
                                AgentRuntimeStopReason::HumanTakeover => {
                                    AgentWorkFailure::HumanTakeover
                                }
                                AgentRuntimeStopReason::Suspend => {
                                    AgentWorkFailure::SuspendRequested
                                }
                                AgentRuntimeStopReason::PolicyRevoked => {
                                    AgentWorkFailure::PolicyRevoked
                                }
                            });
                    }
                    if self.flight.is_none() {
                        self.persist_execution_end();
                    }
                }
            }
            WorkControl::Review { record, decision } => {
                let result = if self.flight.is_some() || self.recovery_audit.is_some() {
                    Err(AgentWorkJournalError::Unavailable)
                } else if !lock(&self.projection).records.contains(&record)
                    || Some(record.incarnation()) != self.owner
                {
                    Err(AgentWorkJournalError::Conflict)
                } else {
                    AgentWorkJournalMutation::transition(
                        record,
                        match decision {
                            AgentWorkReviewDecision::AcceptFreshAdmission => {
                                AgentWorkDisposition::FreshAdmissionRequired
                            }
                            AgentWorkReviewDecision::Reject => AgentWorkDisposition::Rejected,
                        },
                    )
                };
                match result {
                    Ok(mutation) => self.dispatch(
                        AgentWorkJournalRequest::CompareAndSet(mutation),
                        DurablePurpose::Review,
                        0,
                    ),
                    Err(error) => lock(&self.projection).snapshot.last_review = Some(Err(error)),
                }
            }
            WorkControl::Reconcile => {
                if let Some(flight) = self.flight.take() {
                    if flight.uncertain && flight.reconciliations < 4 {
                        self.dispatch_request(
                            flight.request,
                            flight.purpose,
                            flight.reconciliations + 1,
                        );
                    } else {
                        self.flight = Some(flight);
                    }
                } else {
                    self.reconcile_audit();
                }
            }
        }
        self.poll();
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let durable = self
            .flight
            .as_ref()
            .filter(|flight| !flight.uncertain)
            .map(|flight| flight.deadline);
        let audit = self
            .recovery_audit
            .as_ref()
            .filter(|flight| !flight.uncertain)
            .map(|flight| flight.deadline);
        durable
            .into_iter()
            .chain(audit)
            .chain(self.staged.as_ref().map(|staged| staged.deadline))
            .min()
    }

    fn reconcile_audit(&mut self) {
        if self
            .recovery_audit
            .as_ref()
            .is_some_and(|flight| !flight.uncertain)
            || self.audit_attempts >= 4
        {
            return;
        }
        let Some(audit) = self.audit.clone() else {
            return;
        };
        let Some(recovery) = self.recovery_mut() else {
            return;
        };
        let delivery = match recovery.prepare_audit_reconciliation() {
            Ok(Some(delivery)) => delivery,
            Ok(None) => {
                lock(&self.projection).snapshot.recovery_audit =
                    Some(recovery.audit_reconciliation_status());
                return;
            }
            Err(error) => {
                lock(&self.projection).snapshot.recovery_audit = Some(Err(error));
                return;
            }
        };
        self.audit_attempts += 1;
        let proof = delivery.proof();
        let result = Arc::new(Mutex::new(None));
        let sink = result.clone();
        let wake = self.waker.clone();
        self.recovery_audit = Some(RecoveryAuditFlight {
            proof,
            result: result.clone(),
            deadline: Instant::now() + Duration::from_secs(2),
            uncertain: false,
        });
        match audit.append(
            delivery,
            Box::new(move |settlement| {
                *lock(&sink) = Some(settlement);
                wake.wake();
            }),
        ) {
            AgentAuditDispatch::Accepted(expected) if expected == proof => {}
            AgentAuditDispatch::Refused(settlement) => {
                *lock(&result) = Some(settlement);
            }
            _ => {
                if let Some(flight) = &mut self.recovery_audit {
                    flight.uncertain = true;
                }
                lock(&self.projection).snapshot.recovery_audit = Some(Err(AgentWorkFailure::Audit));
            }
        }
    }

    pub(crate) fn begin_shutdown(&mut self) {
        self.stopping = true;
        self.stop_active(AgentRuntimeStopReason::Cancelled);
        self.abort_staged();
    }

    pub(crate) fn shutdown_until(&mut self, deadline: Instant) -> bool {
        self.begin_shutdown();
        if let Some(active) = &mut self.active {
            if let Some(lifecycle) = active.lifecycle.take() {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    lifecycle.shutdown_until(deadline)
                })) {
                    Ok(AgentBrowserShutdownOutcome::Clean(native)) => {
                        active.native = Some(native);
                        active.lifecycle_clean = Some(true);
                    }
                    _ => active.lifecycle_clean = Some(false),
                }
                active.outcome = active.handle.take_outcome();
            }
        }
        // The accepted callback writes its own one-slot result before waking
        // the shell. Wait only on that slot, never on the shell's command queue.
        // This remains bounded by the original process-shutdown deadline.
        for _ in 0..4 {
            self.poll();
            let Some(flight) = &self.flight else {
                break;
            };
            if flight.uncertain || Instant::now() >= deadline {
                break;
            }
            flight.result.wait_until(deadline);
        }
        self.poll();
        self.flight.is_none()
            && !self.artifact_preparation_failed
            && self.recovery_audit.is_none()
            && self.unstarted.is_none()
            && self
                .active
                .as_ref()
                .is_none_or(|active| active.lifecycle_clean == Some(true))
    }
}
