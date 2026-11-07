//! Application-owned single-resource admission behind the bounded Shell adapter.
//!
//! A retained native row is not successor permission. This owner alone sequences
//! original Store acknowledgements, the common controller and scoped worker,
//! and keeps the page/cleanup owner across fresh, explicitly supplied actors.

use super::shutdown::RetainedNativeShutdown;
use super::*;
use std::collections::VecDeque;
use std::task::Waker;
use std::time::{Duration, Instant};
use zephium_agent_controller::*;
use zephium_agent_provider_transport::AgentProviderTransportConfig;
use zephium_agent_runtime::*;

#[path = "work_resources_human.rs"]
mod human;
pub(in crate::work_resources) use human::HumanStep;
pub use human::{RetainedHumanPhase, RetainedHumanResume, RetainedHumanSnapshot};

#[cfg(feature = "work-execution-probe")]
#[path = "work_resources_public_actor.rs"]
mod public_qualification;

/// Trusted, dormant preparation. The callback receives only a newly acquired
/// read binding through the narrow facade, never the native resource owner.
pub(super) type PrepareActor = Box<
    dyn FnOnce(
            Box<dyn AgentWorkRetainedBrowser>,
            Arc<dyn AgentAuditPort>,
            Option<AgentWorkWaitingForHuman>,
        ) -> Result<StagedActor, AgentWorkFailure>
        + Send,
>;

pub(super) struct ActorRequest {
    pub(super) run: ContextRunId,
    pub(super) deadline: AgentPolicyInstant,
    pub(super) prepare: PrepareActor,
}

pub(super) struct StagedActor {
    controller: Box<AgentWorkRetainedController>,
    handle: AgentWorkRetainedHandle,
    scope: AgentRuntimeScopedBinding,
    lease: WorkBrowserExecutionLease,
    audit: Arc<dyn AgentAuditPort>,
    config: AgentRuntimeConfig,
    deadline: Instant,
}
impl StagedActor {
    #[cfg(all(test, feature = "work-execution-probe"))]
    pub(super) fn for_probe(
        input: AgentWorkRunInput,
        browser: Box<dyn AgentWorkRetainedBrowser>,
        transport: zephium_agent_provider_transport::AgentProviderTransport,
        access: impl Into<AgentBrowserModelAccess>,
        audit: Arc<dyn AgentAuditPort>,
        task: Box<dyn AgentWorkTask>,
    ) -> Result<Self, AgentWorkFailure> {
        Self::for_probe_human(input, browser, transport, access, audit, task, None)
    }
    #[cfg(all(test, feature = "work-execution-probe"))]
    pub(super) fn for_probe_human(
        input: AgentWorkRunInput,
        browser: Box<dyn AgentWorkRetainedBrowser>,
        transport: zephium_agent_provider_transport::AgentProviderTransport,
        access: impl Into<AgentBrowserModelAccess>,
        audit: Arc<dyn AgentAuditPort>,
        task: Box<dyn AgentWorkTask>,
        waiting: Option<AgentWorkWaitingForHuman>,
    ) -> Result<Self, AgentWorkFailure> {
        let lease = browser.binding().lease().clone();
        let (controller, handle, scope) = if let Some(waiting) = waiting {
            AgentWorkRetainedController::try_new_after_human_for_probe(
                waiting,
                input,
                browser,
                transport,
                access,
                audit.clone(),
                task,
            )
        } else {
            AgentWorkRetainedController::try_new_for_probe(
                input,
                browser,
                transport,
                access,
                audit.clone(),
                task,
            )
        }?;
        let deadline = controller.deadline()?;
        Ok(Self {
            controller: Box::new(controller),
            handle,
            scope,
            lease,
            audit,
            config: AgentRuntimeConfig::STANDARD,
            deadline,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn try_new(
        input: AgentWorkRunInput,
        browser: Box<dyn AgentWorkRetainedBrowser>,
        config: AgentRuntimeConfig,
        provider: AgentProviderTransportConfig,
        access: AgentBrowserModelAccess,
        audit: Arc<dyn AgentAuditPort>,
        task: Box<dyn AgentWorkTask>,
        waiting: Option<AgentWorkWaitingForHuman>,
    ) -> Result<Self, AgentWorkFailure> {
        let lease = browser.binding().lease().clone();
        let (controller, handle, scope) = if let Some(waiting) = waiting {
            AgentWorkRetainedController::try_new_after_human(
                waiting,
                input,
                browser,
                provider,
                access,
                audit.clone(),
                task,
            )
        } else {
            AgentWorkRetainedController::try_new(
                input,
                browser,
                provider,
                access,
                audit.clone(),
                task,
            )
        }?;
        let deadline = controller.deadline()?;
        Ok(Self {
            controller: Box::new(controller),
            handle,
            scope,
            lease,
            audit,
            config,
            deadline,
        })
    }
}

struct ActiveActor {
    handle: AgentWorkRetainedHandle,
    runtime: AgentRuntimeHandle,
    completion: AgentRuntimeCompletion,
    lifecycle: Option<AgentRuntimeScopedLifecycle>,
    drained: Option<AgentRuntimeScopedDrained>,
    outcome: Option<AgentWorkRetainedOutcome>,
    accepted: Option<AgentRunPolicySettlement>,
    extraction: Option<Box<SemanticOwnedExtractionResult>>,
    pending_event: Option<AgentWorkEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AdmissionPhase {
    Constructing,
    Loading,
    NeedsReview,
    Reviewing,
    Ready,
    Acquiring,
    Admitting,
    Starting,
    Running,
    Closing,
    Terminal,
    Uncertain,
}

/// Original request and callback slot stay owned even after timeout/refusal.
/// There is no read-back shortcut or decoded-record admission. An uncertain
/// flight is resent only as the same exact request, after `deadline`.
struct JournalFlight {
    request: DurableRequest,
    result: Arc<Mutex<Option<Result<DurableReply, AgentWorkJournalError>>>>,
    deadline: Instant,
    phase: AdmissionPhase,
    uncertain: bool,
    reconciliations: u8,
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

type GroupReview = (AgentWorkRecord, Option<AgentWorkRecord>);

#[derive(Clone)]
pub(crate) struct RetainedWorkGroup {
    runtime: AgentRuntimeWorkerGroup,
    records: Arc<std::sync::Mutex<Vec<(ContextId, AgentWorkRecord)>>>,
    // Historical rows under review by a page of this group: the exact row and,
    // once acknowledged, the row the journal now holds.
    reviews: Arc<std::sync::Mutex<Vec<GroupReview>>>,
    failed: Arc<std::sync::atomic::AtomicBool>,
    sealed: Arc<std::sync::atomic::AtomicBool>,
}

impl RetainedWorkGroup {
    pub(crate) fn try_new(work: WorkId, capacity: u8) -> Result<Self, RuntimeSpawnError> {
        Ok(Self {
            runtime: AgentRuntimeWorkerGroup::try_new(work, capacity)?,
            records: Arc::new(std::sync::Mutex::new(Vec::new())),
            reviews: Arc::new(std::sync::Mutex::new(Vec::new())),
            failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sealed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    /// This page's run entered the group manifest.
    fn holds(&self, context: ContextId) -> bool {
        self.records
            .lock()
            .map_or(true, |records| records.iter().any(|(id, _)| *id == context))
    }
    /// Claims the review of `record` for one page, or returns the row a peer's
    /// acknowledged review left. `Err` while a peer's review is in flight.
    fn begin_review(
        &self,
        record: AgentWorkRecord,
    ) -> Result<Option<AgentWorkRecord>, AgentWorkJournalError> {
        let mut reviews = self
            .reviews
            .lock()
            .map_err(|_| AgentWorkJournalError::Conflict)?;
        match reviews.iter().find(|(expected, _)| *expected == record) {
            Some((_, Some(next))) => Ok(Some(*next)),
            Some((_, None)) => Err(AgentWorkJournalError::Unavailable),
            None if reviews.len() < MAX_DURABLE_AGENT_WORK_RUNS => {
                reviews.push((record, None));
                Ok(None)
            }
            None => Err(AgentWorkJournalError::Capacity),
        }
    }
    // An unacknowledged review stays claimed: its row is uncertain.
    fn settle_review(&self, expected: AgentWorkRecord, next: AgentWorkRecord) {
        if let Ok(mut reviews) = self.reviews.lock() {
            if let Some(entry) = reviews.iter_mut().find(|(row, _)| *row == expected) {
                entry.1 = Some(next);
            }
        }
    }
    fn remember(
        &self,
        context: ContextId,
        record: AgentWorkRecord,
    ) -> Result<(), AgentWorkJournalError> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| AgentWorkJournalError::Conflict)?;
        if let Some((_, prior)) = records.iter_mut().find(|(id, _)| *id == context) {
            if prior.key() != record.key() && !historical_record_admissible(*prior) {
                return Err(AgentWorkJournalError::Conflict);
            }
            *prior = record;
        } else if records.len() < 3 {
            records.push((context, record));
        } else {
            return Err(AgentWorkJournalError::Capacity);
        }
        Ok(())
    }
    fn admit(
        &self,
        context: ContextId,
        record: AgentWorkRecord,
    ) -> Result<(), AgentWorkJournalError> {
        if self.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(AgentWorkJournalError::Conflict);
        }
        self.remember(context, record)
    }
    fn owns_running(&self, owner: AgentWorkIncarnation, record: AgentWorkRecord) -> bool {
        !self.failed.load(std::sync::atomic::Ordering::Acquire)
            && record.incarnation() == owner
            && matches!(
                (record.disposition(), record.revision()),
                (AgentWorkDisposition::Admitted, 1) | (AgentWorkDisposition::Running, 2)
            )
            && self.records.lock().is_ok_and(|records| {
                records
                    .iter()
                    .any(|(_, expected)| expected.as_bytes()[16..] == record.as_bytes()[16..])
            })
    }
    fn fail(&self) {
        self.failed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn is_failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::Acquire)
    }
    /// A member took the native audit turn: the native group admits no page.
    pub(crate) fn seal(&self) {
        self.sealed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn is_sealed(&self) -> bool {
        self.sealed.load(std::sync::atomic::Ordering::Acquire)
    }
}

pub(super) struct RetainedWork {
    media: Option<MediaReadState>,
    media_result: Option<Vec<zephium_agentic::WorkBrowserRecordImage>>,
    runtime_group: Option<RetainedWorkGroup>,
    // Outlives the group itself: a page that left its group still audits
    // native state only when the Shell hands it the one audit turn.
    grouped: bool,
    group_shutdown: bool,
    human: Option<human::HumanHandoff>,
    prior_human: Option<AgentWorkWaitingForHuman>,
    human_generation: u32,
    owner: WorkResourceOwner,
    resource: WorkBrowserResourceJoin,
    journal: Arc<dyn AgentWorkJournalPort>,
    audit: Arc<dyn AgentAuditPort>,
    waker: Waker,
    incarnation: Option<AgentWorkIncarnation>,
    inventory: Vec<AgentWorkRecord>,
    phase: AdmissionPhase,
    failure: Option<AgentWorkFailure>,
    persistence_failure: Option<AgentWorkJournalError>,
    request: Option<ActorRequest>,
    acquisition: Option<PendingLifecycle>,
    construction: Option<PendingLifecycle>,
    staged: Option<StagedActor>,
    unstarted: Option<AgentWorkRetainedOutcome>,
    active: Option<ActiveActor>,
    flight: Option<JournalFlight>,
    record: Option<AgentWorkRecord>,
    terminal_usage: Option<(AgentWorkRecord, zephium_core::work::runtime::WorkUsage)>,
    result_profile: Option<zephium_core::ids::ProfileId>,
    artifact: Option<AgentWorkArtifactDescriptor>,
    archived: Option<AgentWorkArchivedExtraction>,
    artifact_read: Option<Result<bool, AgentWorkJournalError>>,
    last_review: Option<Result<AgentWorkRecord, AgentWorkJournalError>>,
    events: VecDeque<AgentWorkEvent>,
    stopping: bool,
    destruction: Option<PendingLifecycle>,
    destruction_settled: bool,
    destroyed: bool,
    unexpected_native: Option<ContextNativeEvent>,
    native_shutdown: Option<RetainedNativeShutdown>,
    /// Where the native gate saw a public first load redirected.
    redirect: Option<zephium_agentic::WorkBrowserRedirect>,
}
impl RetainedWork {
    /// The newest canvas frame of this resource's page, if the engine hosts one.
    pub(super) fn latest_frame(&self) -> Option<Arc<zephium_agentic::WorkBrowserFrame>> {
        self.owner.shared.port.latest_work_frame(&self.resource)
    }
    /// Consumes the original owner, not a replacement port or a decoded row.
    /// Store and audit must be the same original adapter allocation. Construction
    /// and selected-profile admission still belong to the trusted application.
    pub(super) fn new(
        owner: WorkResourceOwner,
        resource: WorkBrowserResourceJoin,
        journal: Arc<dyn AgentWorkJournalPort>,
        audit: Arc<dyn AgentAuditPort>,
    ) -> Result<Self, (WorkResourceOwner, Refusal)> {
        let valid = std::ptr::addr_eq(Arc::as_ptr(&journal), Arc::as_ptr(&audit))
            && owner
                .shared
                .lock_resources()
                .is_ok_and(|rows| rows.len() == 1)
            && owner.shared.resource(&resource).is_ok_and(|row| {
                owner.shared.current(&row).is_ok()
                    && row.reusable.load(Ordering::Acquire)
                    && row.flights.load(Ordering::Acquire) == 0
            })
            && owner
                .shared
                .lock_rows()
                .is_ok_and(|rows| rows.phase(&resource) == Ok(WorkBrowserResourcePhase::Retained));
        if !valid {
            return Err((owner, Refusal::Uncertain));
        }
        // Journal/progress use the same original application wake as native
        // resource notifications; no second callback route is substituted.
        let waker = owner.shared.notifications.clone().into();
        let mut work = Self::dormant(owner, resource, journal, audit, waker);
        work.rearm();
        if work.failure.is_some() {
            return Err((work.owner, Refusal::Uncertain));
        }
        work.dispatch(AgentWorkJournalRequest::Claim);
        Ok(work)
    }

    fn dormant(
        owner: WorkResourceOwner,
        resource: WorkBrowserResourceJoin,
        journal: Arc<dyn AgentWorkJournalPort>,
        audit: Arc<dyn AgentAuditPort>,
        waker: Waker,
    ) -> Self {
        Self {
            media: None,
            media_result: None,
            runtime_group: None,
            grouped: false,
            group_shutdown: false,
            human: None,
            prior_human: None,
            human_generation: 0,
            owner,
            resource,
            journal,
            audit,
            waker,
            incarnation: None,
            inventory: Vec::new(),
            phase: AdmissionPhase::Loading,
            failure: None,
            persistence_failure: None,
            request: None,
            acquisition: None,
            construction: None,
            staged: None,
            unstarted: None,
            active: None,
            flight: None,
            record: None,
            terminal_usage: None,
            result_profile: None,
            artifact: None,
            archived: None,
            artifact_read: None,
            last_review: None,
            events: VecDeque::with_capacity(MAX_AGENT_WORK_EVENTS),
            stopping: false,
            destruction: None,
            destruction_settled: false,
            destroyed: false,
            unexpected_native: None,
            native_shutdown: None,
            redirect: None,
        }
    }

    /// Shell has already checked original Store/Engine/profile before native
    /// construction. Retain its original operation through stop and timeout.
    pub(super) fn constructing(
        owner: WorkResourceOwner,
        pending: PendingLifecycle,
        journal: Arc<dyn AgentWorkJournalPort>,
        audit: Arc<dyn AgentAuditPort>,
    ) -> Self {
        let resource = pending.resource.join.clone();
        let exact = Arc::ptr_eq(&owner.shared, &pending.shared)
            && std::ptr::addr_eq(Arc::as_ptr(&journal), Arc::as_ptr(&audit))
            && owner
                .shared
                .lock_resources()
                .is_ok_and(|rows| rows.len() == 1)
            && owner
                .shared
                .resource(&resource)
                .is_ok_and(|row| Arc::ptr_eq(&row, &pending.resource));
        let waker = owner.shared.notifications.clone().into();
        let mut work = Self::dormant(owner, resource, journal, audit, waker);
        work.phase = AdmissionPhase::Constructing;
        work.construction = Some(pending);
        if !exact {
            work.fail(crate::contract_failure!());
        }
        work
    }

    pub(super) fn ready(&self) -> bool {
        if self
            .human
            .as_ref()
            .is_some_and(|human| human.phase != RetainedHumanPhase::ReadyToResume)
            || self.stopping
            || self.failure.is_some()
            || self.persistence_failure.is_some()
            || self.flight.is_some()
            || self.request.is_some()
            || self.acquisition.is_some()
            || self.staged.is_some()
            || self.unstarted.is_some()
            || !self.events.is_empty()
            || self.storage_full()
        {
            return false;
        }
        match self.phase {
            AdmissionPhase::Ready => self.active.is_none() && self.record.is_none(),
            AdmissionPhase::Terminal => self.active.as_ref().is_some_and(|active| {
                active.drained.is_some()
                    && active.lifecycle.is_none()
                    && active.completion.is_stopped()
                    && (active.outcome.is_some() || active.accepted.is_some())
                    && active.extraction.is_none()
                    && active.pending_event.is_none()
                    && !active.handle.has_pending_events()
                    && self
                        .record
                        .is_some_and(|record| record.debt() == AgentWorkDebt::NONE)
            }),
            _ => false,
        }
    }

    /// Exactly one fresh explicit request; no automatic actor restart or replay.
    /// A refusal returns the original task/credential closure without consuming it.
    pub(super) fn submit(
        &mut self,
        request: ActorRequest,
        now: AgentPolicyInstant,
    ) -> Result<(), ActorRequest> {
        if !self.ready()
            || request.deadline <= now
            || self
                .inventory
                .iter()
                .any(|prior| prior.as_bytes()[48..64] == request.run.bytes())
        {
            return Err(request);
        }
        let acquisition =
            match self
                .owner
                .acquire(&self.resource, request.run, now, request.deadline)
            {
                Ok(acquisition) => acquisition,
                Err(_) => return Err(request),
            };
        // Only this exact durable/scoped join retires the previous actor owner.
        if self
            .human
            .as_ref()
            .is_some_and(|human| human.phase == RetainedHumanPhase::ReadyToResume)
        {
            self.prior_human =
                self.active
                    .as_mut()
                    .and_then(|active| match active.outcome.take() {
                        Some(AgentWorkRetainedOutcome::WaitingForHuman(waiting)) => Some(waiting),
                        _ => None,
                    });
            self.human = None;
        }
        self.active.take();
        self.record = None;
        self.terminal_usage = None;
        self.result_profile = None;
        self.artifact = None;
        self.acquisition = Some(acquisition);
        self.request = Some(request);
        self.phase = AdmissionPhase::Acquiring;
        Ok(())
    }

    fn dispatch(&mut self, request: AgentWorkJournalRequest) {
        self.dispatch_attempt(DurableRequest::Journal(request), self.phase, 0);
    }

    fn dispatch_attempt(
        &mut self,
        request: DurableRequest,
        phase: AdmissionPhase,
        reconciliations: u8,
    ) {
        debug_assert!(self.flight.is_none());
        let result = Arc::new(Mutex::new(None));
        let sink = result.clone();
        let wake = self.waker.clone();
        self.flight = Some(JournalFlight {
            request: request.clone(),
            result: result.clone(),
            deadline: Instant::now() + Duration::from_secs(2),
            phase,
            uncertain: false,
            reconciliations,
        });
        let completion = move |reply| {
            match sink.lock() {
                Ok(mut slot) => *slot = Some(reply),
                Err(_) => return,
            }
            wake.wake();
        };
        let dispatched = match request {
            DurableRequest::Journal(request) => self.journal.dispatch(
                request,
                Box::new(move |reply| completion(reply.map(DurableReply::Journal))),
            ),
            DurableRequest::Artifact(request) => self.journal.artifact(
                request,
                Box::new(move |reply| completion(reply.map(DurableReply::Artifact))),
            ),
        };
        if let Err(error) = dispatched {
            if let Ok(mut slot) = result.lock() {
                *slot = Some(Err(error));
            }
        }
    }

    fn fail(&mut self, failure: AgentWorkFailure) {
        // Only a run in the group manifest can leave peers an uncertain row;
        // a page that failed before admission leaves its group healthy.
        if let Some(group) = self
            .runtime_group
            .as_ref()
            .filter(|group| group.holds(self.resource.identity().context()))
        {
            group.fail();
        }
        // A lost document whose first load the gate refused as a redirect
        // was redirected; its owner reads the destination from the slot.
        let failure = if failure == AgentWorkFailure::ContextLost
            && self
                .redirect
                .as_ref()
                .is_some_and(|redirect| redirect.destination().is_some())
        {
            AgentWorkFailure::Redirected
        } else {
            failure
        };
        self.failure.get_or_insert(failure);
        self.phase = AdmissionPhase::Uncertain;
        self.stopping = true;
        self.request.take();
        if let Some(staged) = self.staged.take() {
            let StagedActor {
                controller,
                mut handle,
                ..
            } = staged;
            drop(controller);
            self.unstarted = handle.take_outcome();
        }
        if let Some(active) = &self.active {
            active
                .runtime
                .stop_and_seal(AgentRuntimeStopReason::Cancelled);
        }
    }

    fn acknowledge(
        &mut self,
        request: &AgentWorkJournalRequest,
        reply: AgentWorkJournalReply,
        phase: AdmissionPhase,
    ) -> Result<(), AgentWorkJournalError> {
        match (request, reply) {
            (AgentWorkJournalRequest::Claim, AgentWorkJournalReply::Claimed { owner, records }) => {
                if records.len() > zephium_agentic::MAX_AGENT_WORK_INVENTORY
                    || records
                        .windows(2)
                        .any(|pair| pair[0].key() >= pair[1].key())
                {
                    // A malformed inventory leaves the claim itself unsettled.
                    self.inventory = records;
                    #[cfg(feature = "work-execution-probe")]
                    self.public_claim_refusal_diagnostic();
                    return Err(AgentWorkJournalError::Conflict);
                }
                if records.iter().any(|record| {
                    !(historical_record_admissible(*record)
                        || self
                            .runtime_group
                            .as_ref()
                            .is_some_and(|group| group.owns_running(owner, *record))
                        || (reviewable(record.disposition()) && record.incarnation() == owner))
                }) {
                    // Unreviewable recovery facts, or a failed group's rows,
                    // refuse this page. The claim itself settled and the page
                    // wrote nothing, so it closes like any page refused before
                    // admission instead of holding an unsettled Store request.
                    self.inventory = records;
                    #[cfg(feature = "work-execution-probe")]
                    self.public_claim_refusal_diagnostic();
                    self.fail(crate::contract_failure!());
                    return Ok(());
                }
                self.incarnation = Some(owner);
                self.inventory = records;
                self.phase = if self.stopping {
                    AdmissionPhase::Uncertain
                } else if self.inventory.iter().any(|record| {
                    !historical_record_admissible(*record)
                        && !self
                            .runtime_group
                            .as_ref()
                            .is_some_and(|group| group.owns_running(owner, *record))
                }) {
                    AdmissionPhase::NeedsReview
                } else {
                    AdmissionPhase::Ready
                };
            }
            (
                AgentWorkJournalRequest::CompareAndSet(mutation),
                AgentWorkJournalReply::Record(Some(record)),
            ) if record == mutation.next() => {
                if let Some(group) = &self.runtime_group {
                    group.remember(self.resource.identity().context(), record)?;
                }
                if !zephium_agentic::remember_agent_work_record(&mut self.inventory, record) {
                    return Err(AgentWorkJournalError::Capacity);
                }
                if phase == AdmissionPhase::Reviewing {
                    if !matches!(
                        record.disposition(),
                        AgentWorkDisposition::FreshAdmissionRequired
                            | AgentWorkDisposition::Rejected
                    ) {
                        return Err(AgentWorkJournalError::Transition);
                    }
                    if let (Some(group), Some(expected)) =
                        (&self.runtime_group, mutation.expected())
                    {
                        group.settle_review(expected, record);
                    }
                    self.last_review = Some(Ok(record));
                    self.phase = if self.stopping {
                        AdmissionPhase::Uncertain
                    } else if self
                        .inventory
                        .iter()
                        .all(|record| historical_record_admissible(*record))
                    {
                        AdmissionPhase::Ready
                    } else {
                        AdmissionPhase::NeedsReview
                    };
                    return Ok(());
                }
                self.record = Some(record);
                match (phase, record.disposition()) {
                    (AdmissionPhase::Admitting, AgentWorkDisposition::Admitted) => {
                        self.result_profile = mutation.result_profile();
                        self.phase = if self.stopping {
                            AdmissionPhase::Closing
                        } else {
                            AdmissionPhase::Starting
                        };
                        self.dispatch(AgentWorkJournalRequest::CompareAndSet(
                            AgentWorkJournalMutation::transition(
                                record,
                                if self.stopping {
                                    AgentWorkDisposition::FailedClosed
                                } else {
                                    AgentWorkDisposition::Running
                                },
                            )?,
                        ));
                    }
                    (AdmissionPhase::Starting, AgentWorkDisposition::Running) => {
                        if self.stopping {
                            self.phase = AdmissionPhase::Closing;
                            self.dispatch(AgentWorkJournalRequest::CompareAndSet(
                                AgentWorkJournalMutation::transition(
                                    record,
                                    AgentWorkDisposition::FailedClosed,
                                )?,
                            ));
                        } else {
                            self.activate();
                        }
                    }
                    (
                        AdmissionPhase::Closing,
                        AgentWorkDisposition::Succeeded
                        | AgentWorkDisposition::Failed
                        | AgentWorkDisposition::Cancelled
                        | AgentWorkDisposition::WaitingForHuman,
                    ) => {
                        self.phase = AdmissionPhase::Terminal;
                    }
                    (AdmissionPhase::Closing, AgentWorkDisposition::FailedClosed) => {
                        // Immutable no-replay classification, not scoped closure.
                        self.phase = AdmissionPhase::Uncertain;
                    }
                    (AdmissionPhase::Closing, AgentWorkDisposition::RecoveryRequired) => {
                        // The exact scoped controller ended in Recovery, so the
                        // durable row must remain nonterminal with its original
                        // conservative debt. This acknowledgement classifies
                        // the interruption; it is not a clean actor closure.
                        self.phase = AdmissionPhase::Uncertain;
                    }
                    _ => return Err(AgentWorkJournalError::Transition),
                }
            }
            _ => return Err(AgentWorkJournalError::Conflict),
        }
        Ok(())
    }

    pub(super) fn set_redirect(&mut self, redirect: zephium_agentic::WorkBrowserRedirect) {
        self.redirect = Some(redirect);
    }
    pub(super) fn set_runtime_group(&mut self, group: RetainedWorkGroup) {
        self.runtime_group = Some(group);
        self.grouped = true;
    }
    /// Releases this page's share of the group's runtime slot.
    pub(super) fn leave_group(&mut self) {
        self.runtime_group = None;
    }

    fn activate(&mut self) {
        let Some(staged) = self.staged.take() else {
            self.fail(crate::contract_failure!());
            return;
        };
        if self.stopping || staged.deadline <= Instant::now() {
            self.staged = Some(staged);
            self.fail(AgentWorkFailure::Deadline);
            return;
        }
        let StagedActor {
            controller,
            mut handle,
            scope,
            config,
            ..
        } = staged;
        handle.set_waker(self.waker.clone());
        let pending = match match &self.runtime_group {
            Some(group) => PendingScopedAgentRuntime::spawn_suspended_in_group(
                config,
                scope,
                controller,
                &group.runtime,
            ),
            None => PendingScopedAgentRuntime::spawn_suspended(config, scope, controller),
        } {
            Ok(pending) => pending,
            Err(_) => {
                self.unstarted = handle.take_outcome();
                self.fail(crate::contract_failure!());
                return;
            }
        };
        let (runtime, completion, lifecycle) = pending.bind().into_parts();
        completion.set_waker(self.waker.clone());
        let started = runtime.start_run().is_ok();
        self.active = Some(ActiveActor {
            handle,
            runtime,
            completion,
            lifecycle: Some(lifecycle),
            drained: None,
            outcome: None,
            accepted: None,
            extraction: None,
            pending_event: None,
        });
        self.phase = AdmissionPhase::Running;
        if !started {
            self.fail(crate::contract_failure!());
        }
    }

    pub(super) fn poll(&mut self, now: AgentPolicyInstant) {
        self.poll_before(now, None);
    }

    fn poll_before(&mut self, now: AgentPolicyInstant, deadline: Option<Instant>) {
        self.rearm();
        self.poll_human(now);
        self.poll_media(now);
        if let Some(mut construction) = self.construction.take() {
            match construction.poll(now) {
                Ok(None) => self.construction = Some(construction),
                Ok(Some(LifecycleResult::Event(WorkBrowserResourceEvent::Retained(resource))))
                    if resource == self.resource =>
                {
                    if !self.stopping {
                        self.phase = AdmissionPhase::Loading;
                        self.dispatch(AgentWorkJournalRequest::Claim);
                    }
                }
                _ => self.fail(AgentWorkFailure::ContextLost),
            }
        }
        // Check both original horizons before consuming a raced Running ACK.
        if self.staged.as_ref().is_some_and(|staged| {
            staged.deadline <= Instant::now() || staged.lease.deadline() <= now
        }) {
            self.fail(AgentWorkFailure::Deadline);
        }
        if let Some(mut flight) = self.flight.take() {
            let reply = flight
                .result
                .lock()
                .map(|mut slot| slot.take())
                .map_err(|_| ());
            #[cfg(test)]
            let reply = match reply {
                Ok(Some(Ok(_)))
                    if UNCONFIRMED_TERMINALS.load(Ordering::Acquire)
                        && matches!(
                            &flight.request,
                            DurableRequest::Journal(AgentWorkJournalRequest::CompareAndSet(mutation))
                                if mutation.next().disposition().is_terminal()
                        ) =>
                {
                    Ok(Some(Err(AgentWorkJournalError::Uncertain)))
                }
                reply => reply,
            };
            let mut retain = false;
            let result = match reply {
                Ok(Some(Ok(reply))) => match (&flight.request, reply) {
                    (DurableRequest::Journal(request), DurableReply::Journal(reply)) => {
                        self.acknowledge(request, reply, flight.phase)
                    }
                    (
                        DurableRequest::Artifact(AgentWorkArtifactRequest::Publish(publication)),
                        DurableReply::Artifact(AgentWorkArtifactReply::Published {
                            record,
                            descriptor,
                        }),
                    ) if record == publication.mutation().next()
                        && descriptor == publication.descriptor()
                        && self.result_profile == Some(descriptor.profile()) =>
                    {
                        self.acknowledge(
                            &AgentWorkJournalRequest::CompareAndSet(publication.mutation()),
                            AgentWorkJournalReply::Record(Some(record)),
                            flight.phase,
                        )
                        .map(|()| self.artifact = Some(descriptor))
                    }
                    (
                        DurableRequest::Artifact(AgentWorkArtifactRequest::Read {
                            record,
                            profile,
                            ..
                        }),
                        DurableReply::Artifact(AgentWorkArtifactReply::Read(archived)),
                    ) if archived.as_ref().is_none_or(|value| {
                        value.descriptor().key() == record.key()
                            && value.descriptor().profile() == *profile
                    }) =>
                    {
                        self.artifact_read = Some(Ok(archived.is_some()));
                        self.archived = archived;
                        Ok(())
                    }
                    _ => Err(AgentWorkJournalError::Conflict),
                },
                Ok(Some(Err(error))) => Err(error),
                Err(_) => Err(AgentWorkJournalError::Uncertain),
                _ if !flight.uncertain && Instant::now() >= flight.deadline => {
                    Err(AgentWorkJournalError::Uncertain)
                }
                _ => {
                    retain = true;
                    Ok(())
                }
            };
            if let Err(error) = result {
                if matches!(
                    flight.request,
                    DurableRequest::Artifact(AgentWorkArtifactRequest::Read { .. })
                ) {
                    // An archived read has no mutation or execution authority.
                    // Its failure must not relabel an already committed run.
                    self.artifact_read = Some(Err(error));
                    retain = false;
                } else {
                    if flight.phase == AdmissionPhase::Reviewing {
                        self.last_review = Some(Err(error));
                    }
                    self.persistence_failure.get_or_insert(error);
                    self.fail(crate::contract_failure!());
                    flight.uncertain = true;
                    flight.deadline =
                        Instant::now() + reconciliation_backoff(flight.reconciliations);
                    // Retain the exact unsettled request even after a malformed ACK.
                    retain = true;
                }
            }
            if retain {
                self.flight = Some(flight);
            } else {
                self.persistence_failure = None;
            }
        }
        // A late reply to the original request still settles it until the
        // resend is due. The resend is the same exact idempotent request; it
        // keeps the sticky stop and never lets native proof precede its ACK.
        if self
            .flight
            .as_ref()
            .is_some_and(|flight| flight.uncertain && Instant::now() >= flight.deadline)
        {
            self.reconcile();
        }

        if let Some(mut acquisition) = self.acquisition.take() {
            match acquisition.poll(now) {
                Ok(None) => self.acquisition = Some(acquisition),
                Ok(Some(LifecycleResult::Event(WorkBrowserResourceEvent::Acquired(lease)))) => {
                    if let Some(request) = self.request.take() {
                        let prepared = self
                            .owner
                            .retained_browser(lease.clone(), now)
                            .map_err(|_| AgentWorkFailure::ContextLost)
                            .and_then(|browser| {
                                (request.prepare)(
                                    Box::new(browser),
                                    self.audit.clone(),
                                    self.prior_human.take(),
                                )
                            });
                        match prepared {
                            Ok(staged)
                                if staged.lease == lease
                                    && std::ptr::addr_eq(
                                        Arc::as_ptr(&staged.audit),
                                        Arc::as_ptr(&self.audit),
                                    ) =>
                            {
                                let admission = self
                                    .incarnation
                                    .ok_or_else(|| crate::contract_failure!())
                                    .and_then(|owner| staged.controller.journal_admission(owner))
                                    .and_then(|admission| {
                                        if let Some(group) = &self.runtime_group {
                                            group
                                                .admit(
                                                    self.resource.identity().context(),
                                                    admission.next(),
                                                )
                                                .map_err(|_| crate::contract_failure!())?;
                                        }
                                        Ok(admission)
                                    });
                                self.staged = Some(staged);
                                match admission {
                                    Ok(admission) => {
                                        self.phase = AdmissionPhase::Admitting;
                                        self.dispatch(AgentWorkJournalRequest::CompareAndSet(
                                            admission,
                                        ));
                                    }
                                    Err(error) => self.fail(error),
                                }
                            }
                            Ok(staged) => {
                                self.staged = Some(staged);
                                self.fail(crate::contract_failure!());
                            }
                            Err(error) => self.fail(error),
                        }
                    }
                }
                _ => self.fail(AgentWorkFailure::ContextLost),
            }
        }
        let mut failure = None;
        if let Some(active) = &mut self.active {
            while let Some(event) = active
                .pending_event
                .take()
                .or_else(|| active.handle.take_event())
            {
                if self.events.len() == MAX_AGENT_WORK_EVENTS {
                    active.pending_event = Some(event);
                    failure = Some(AgentWorkFailure::Backpressure);
                    break;
                }
                self.events.push_back(event);
            }
            if active.completion.is_stopped() {
                if let Some(lifecycle) = active.lifecycle.take() {
                    let drain_deadline =
                        deadline.unwrap_or_else(|| Instant::now() + Duration::from_millis(100));
                    match lifecycle.drain_until(drain_deadline) {
                        AgentRuntimeScopedDrain::Drained(drained) => active.drained = Some(drained),
                        AgentRuntimeScopedDrain::Unproven => {
                            crate::diagnostic!("work: phase=page event=terminal_refused stage=scoped_drain; content=redacted");
                            failure = Some(crate::contract_failure!())
                        }
                    }
                }
                if active.outcome.is_none() && active.accepted.is_none() {
                    match active.handle.take_outcome() {
                        Some(AgentWorkRetainedOutcome::Accepted {
                            settlement,
                            extraction,
                        }) => {
                            active.accepted = Some(settlement);
                            active.extraction = Some(extraction);
                        }
                        Some(AgentWorkRetainedOutcome::Recovery(recovery)) => {
                            crate::diagnostic!(
                                "work: phase=page event=retained_recovery failure={:?} cleanup_failure={:?}; content=redacted",
                                recovery.failure(),
                                recovery.cleanup_failure()
                            );
                            active.outcome = Some(AgentWorkRetainedOutcome::Recovery(recovery));
                        }
                        outcome => active.outcome = outcome,
                    }
                }
                if let Some(AgentWorkRetainedOutcome::Recovery(recovery)) = &active.outcome {
                    failure = Some(recovery.failure());
                }
            }
        }
        if let Some(failure) = failure {
            self.fail(failure);
        }
        if self.phase == AdmissionPhase::Running && self.flight.is_none() {
            if let (Some(active), Some(record)) = (&mut self.active, self.record) {
                if let Some(drained) = &active.drained {
                    let policy = match &active.outcome {
                        Some(AgentWorkRetainedOutcome::ClosedUnsuccessfully(closed)) => {
                            Some(closed.policy_settlement())
                        }
                        Some(AgentWorkRetainedOutcome::WaitingForHuman(waiting)) => {
                            Some(waiting.policy_settlement())
                        }
                        _ => active.accepted,
                    };
                    let terminal = if policy == Some(drained.policy())
                        && drained.lease().resource() == &self.resource
                    {
                        if let Some(AgentWorkRetainedOutcome::WaitingForHuman(waiting)) =
                            active.outcome.as_ref()
                        {
                            AgentWorkHumanHandoff::try_new(
                                waiting.request().reason(),
                                waiting.request().observation(),
                            )
                            .and_then(|handoff| {
                                drained.work_human_terminal(&active.runtime, record, handoff)
                            })
                        } else {
                            drained.work_terminal(&active.runtime, record)
                        }
                    } else {
                        Err(AgentWorkJournalError::Transition)
                    };
                    match terminal {
                        Ok(terminal) => {
                            // Only the original equal policy/drain/resource join above
                            // can mint these descriptive usage facts. ACK gates exposure.
                            let policy = drained.policy();
                            let accounting = policy.accounting();
                            self.terminal_usage = u32::try_from(accounting.consumed_model_tokens())
                                .ok()
                                .zip(u32::try_from(accounting.consumed_cost_micro_usd()).ok())
                                .map(|(model_tokens, cost_micro_usd)| (terminal.next(),
                                    zephium_core::work::runtime::WorkUsage {
                                        model_tokens,
                                        cost_micro_usd,
                                        operations: accounting.consumed_operations(),
                                        accounting: if policy.model_usage_exact() {
                                            zephium_core::work::runtime::WorkUsageAccounting::Exact
                                        } else {
                                            zephium_core::work::runtime::WorkUsageAccounting::ConservativeReservation
                                        },
                                    }));
                            self.phase = AdmissionPhase::Closing;
                            if terminal.next().disposition() == AgentWorkDisposition::Succeeded
                                && self.result_profile.is_some()
                            {
                                let publication = self
                                    .result_profile
                                    .zip(active.extraction.as_deref())
                                    .ok_or(AgentWorkJournalError::Transition)
                                    .and_then(|(profile, result)| {
                                        AgentWorkArtifactPublication::prepare(
                                            terminal, profile, result,
                                        )
                                    });
                                #[cfg(test)]
                                let publication = publication.and_then(|publication| {
                                    if REFUSE_PUBLICATION.swap(false, Ordering::AcqRel) {
                                        Err(AgentWorkJournalError::Uncertain)
                                    } else {
                                        Ok(publication)
                                    }
                                });
                                match publication {
                                    Ok(publication) => self.dispatch_attempt(
                                        DurableRequest::Artifact(
                                            AgentWorkArtifactRequest::Publish(Arc::new(
                                                publication,
                                            )),
                                        ),
                                        self.phase,
                                        0,
                                    ),
                                    // Nothing reached the Store: the result cannot be
                                    // archived, so the run is no success. Classify its
                                    // exact Running row like a run stopped before it
                                    // could finish, through the same resent CAS.
                                    Err(error) => {
                                        crate::diagnostic!("work: phase=page event=terminal_refused stage=artifact_publication error={error:?}; content=redacted");
                                        self.fail(AgentWorkFailure::ArtifactPublication(error));
                                        match AgentWorkJournalMutation::transition(
                                            record,
                                            AgentWorkDisposition::FailedClosed,
                                        ) {
                                            Ok(mutation) => {
                                                self.phase = AdmissionPhase::Closing;
                                                self.dispatch(
                                                    AgentWorkJournalRequest::CompareAndSet(
                                                        mutation,
                                                    ),
                                                );
                                            }
                                            Err(error) => {
                                                self.persistence_failure = Some(error);
                                            }
                                        }
                                    }
                                }
                            } else {
                                self.dispatch(AgentWorkJournalRequest::CompareAndSet(terminal));
                            }
                        }
                        Err(error) => {
                            crate::diagnostic!("work: phase=page event=terminal_refused stage=journal_terminal error={error:?}; content=redacted");
                            self.fail(crate::contract_failure!())
                        }
                    }
                }
            }
        }
        // A scoped Recovery is the controller's final move-only outcome, but
        // carries no policy settlement that could authorize Failed, Cancelled
        // or Succeeded. Classify the exact Running row conservatively before
        // Store shutdown. The transition preserves UNKNOWN debt and remains
        // nonterminal, so it cannot be mistaken for a clean run or successor
        // admission merely because physical resource cleanup later succeeds.
        if self.flight.is_none()
            && self.active.as_ref().is_some_and(|active| {
                active.completion.is_stopped()
                    && active.lifecycle.is_none()
                    && matches!(active.outcome, Some(AgentWorkRetainedOutcome::Recovery(_)))
            })
        {
            if let Some(record) = self
                .record
                .filter(|record| record.disposition() == AgentWorkDisposition::Running)
            {
                match AgentWorkJournalMutation::transition(
                    record,
                    AgentWorkDisposition::RecoveryRequired,
                ) {
                    Ok(mutation) => {
                        self.phase = AdmissionPhase::Closing;
                        self.dispatch(AgentWorkJournalRequest::CompareAndSet(mutation));
                    }
                    Err(error) => {
                        self.persistence_failure.get_or_insert(error);
                    }
                }
            }
        }
        self.release_unadmitted_recovery();
        // A hard-deadline/spawn refusal can occur after Running ACK but before
        // an ActiveActor exists. Persist no-replay classification, not success
        // or revocation, from that exact acknowledged predecessor too.
        if self.stopping && self.active.is_none() && self.flight.is_none() {
            if let Some(record) = self.record.filter(|record| {
                matches!(
                    record.disposition(),
                    AgentWorkDisposition::Admitted | AgentWorkDisposition::Running
                )
            }) {
                match AgentWorkJournalMutation::transition(
                    record,
                    AgentWorkDisposition::FailedClosed,
                ) {
                    Ok(mutation) => {
                        self.phase = AdmissionPhase::Closing;
                        self.dispatch(AgentWorkJournalRequest::CompareAndSet(mutation));
                    }
                    Err(error) => {
                        self.persistence_failure = Some(error);
                    }
                }
            }
        }
    }

    /// A staged actor the journal never admitted leaves only its dormant
    /// controller's recovery: no row to classify, no native operation and,
    /// when its ledger recorded nothing, no audit to deliver. Holding it would
    /// keep the page from ever settling, and with it the group's native seat.
    fn release_unadmitted_recovery(&mut self) {
        if self.record.is_some() || self.flight.is_some() || self.persistence_failure.is_some() {
            return;
        }
        if let Some(AgentWorkRetainedOutcome::Recovery(recovery)) = self.unstarted.as_mut() {
            if recovery
                .audit_status()
                .is_ok_and(|status| status.pending() == 0 && status.in_flight() == 0)
            {
                drop(self.unstarted.take());
            }
        }
    }

    fn rearm(&mut self) {
        if self.unexpected_native.is_some() {
            return;
        }
        loop {
            match self.owner.poll_native_event() {
                Ok(None) => return,
                Ok(Some(event)) => {
                    let event = if let Some(shutdown) = &mut self.native_shutdown {
                        match shutdown.settle(event) {
                            Ok(()) => continue,
                            Err(event) => *event,
                        }
                    } else {
                        event
                    };
                    // Preserve any unadmitted/mismatched original global terminal.
                    self.unexpected_native = Some(event);
                    self.fail(AgentWorkFailure::ContextLost);
                    return;
                }
                Err(_) => {
                    self.fail(AgentWorkFailure::ContextLost);
                    return;
                }
            }
        }
    }

    pub(super) fn take_event(&mut self) -> Option<AgentWorkEvent> {
        self.events.pop_front()
    }

    pub(super) fn phase(&self) -> AdmissionPhase {
        self.phase
    }

    /// Live and debt-bearing runs fill the Store: no page may be admitted
    /// until the person settles recovery history.
    fn storage_full(&self) -> bool {
        zephium_agentic::outstanding_agent_work_runs(&self.inventory) >= MAX_DURABLE_AGENT_WORK_RUNS
    }

    pub(super) fn failures(&self) -> (Option<AgentWorkFailure>, Option<AgentWorkJournalError>) {
        // A proved unsuccessful actor outcome is not coordinator uncertainty.
        // Project its original cause without calling fail(), changing phase,
        // cancelling a drained actor, or interfering with the terminal CAS.
        let failure = self
            .failure
            .or_else(|| match self.active.as_ref()?.outcome.as_ref()? {
                AgentWorkRetainedOutcome::ClosedUnsuccessfully(closed) => Some(closed.failure()),
                _ => None,
            });
        let persistence = self.persistence_failure.or_else(|| {
            self.storage_full()
                .then_some(AgentWorkJournalError::Capacity)
        });
        (failure, persistence)
    }

    pub(super) fn record(&self) -> Option<AgentWorkRecord> {
        self.record
    }

    pub(super) fn usage(&self) -> Option<zephium_core::work::runtime::WorkUsage> {
        let (record, usage) = self.terminal_usage?;
        (self.phase == AdmissionPhase::Terminal && self.record == Some(record)).then_some(usage)
    }

    pub(super) fn artifact(&self) -> Option<AgentWorkArtifactDescriptor> {
        self.artifact
    }

    pub(super) fn records(&self) -> &[AgentWorkRecord] {
        &self.inventory
    }

    /// Exact human classification of a prior-process interruption. The original
    /// debt remains historical uncertainty; this never settles or replays it.
    pub(super) fn review(
        &mut self,
        record: AgentWorkRecord,
        decision: crate::AgentWorkReviewDecision,
    ) {
        let result = if self.phase != AdmissionPhase::NeedsReview
            || self.flight.is_some()
            || self.stopping
        {
            Err(AgentWorkJournalError::Unavailable)
        } else if !self.inventory.contains(&record)
            || Some(record.incarnation()) != self.incarnation
            || !reviewable(record.disposition())
            || self.record.is_some()
            || self.active.is_some()
            || self.acquisition.is_some()
            || self.staged.is_some()
        {
            Err(AgentWorkJournalError::Conflict)
        } else {
            AgentWorkJournalMutation::transition(
                record,
                match decision {
                    crate::AgentWorkReviewDecision::AcceptFreshAdmission => {
                        AgentWorkDisposition::FreshAdmissionRequired
                    }
                    crate::AgentWorkReviewDecision::Reject => AgentWorkDisposition::Rejected,
                },
            )
        };
        // Pages of one group claim from the same journal; one reviews a row,
        // its peers take the acknowledged row instead of racing the CAS.
        match result {
            Ok(mutation) => match self
                .runtime_group
                .as_ref()
                .map(|group| group.begin_review(record))
            {
                None | Some(Ok(None)) => {
                    self.last_review = None;
                    self.phase = AdmissionPhase::Reviewing;
                    self.dispatch(AgentWorkJournalRequest::CompareAndSet(mutation));
                }
                Some(Ok(Some(reviewed))) if reviewed.key() == record.key() => {
                    if let Some(row) = self.inventory.iter_mut().find(|row| **row == record) {
                        *row = reviewed;
                    }
                    self.last_review = Some(Ok(reviewed));
                    self.phase = if self
                        .inventory
                        .iter()
                        .all(|record| historical_record_admissible(*record))
                    {
                        AdmissionPhase::Ready
                    } else {
                        AdmissionPhase::NeedsReview
                    };
                }
                Some(Ok(Some(_))) => {
                    self.last_review = Some(Err(AgentWorkJournalError::Conflict));
                }
                Some(Err(error)) => self.last_review = Some(Err(error)),
            },
            Err(error) => self.last_review = Some(Err(error)),
        }
    }

    pub(super) fn last_review(&self) -> Option<Result<AgentWorkRecord, AgentWorkJournalError>> {
        self.last_review
    }

    pub(super) fn read_artifact(&mut self, record: AgentWorkRecord) -> bool {
        if self.flight.is_some()
            || !matches!(self.phase, AdmissionPhase::Ready | AdmissionPhase::Terminal)
            || !self.inventory.contains(&record)
            || record.disposition() != AgentWorkDisposition::Succeeded
            || self.archived.is_some()
        {
            return false;
        }
        let Some(owner) = self.incarnation else {
            return false;
        };
        self.artifact_read = None;
        self.dispatch_attempt(
            DurableRequest::Artifact(AgentWorkArtifactRequest::Read {
                owner,
                record,
                profile: self.resource.identity().profile(),
            }),
            self.phase,
            0,
        );
        true
    }

    pub(super) fn artifact_read(&self) -> Option<Result<bool, AgentWorkJournalError>> {
        self.artifact_read
    }

    pub(super) fn take_archived_extraction(&mut self) -> Option<AgentWorkArchivedExtraction> {
        self.archived.take()
    }

    pub(super) fn take_extraction(&mut self) -> Option<Box<SemanticOwnedExtractionResult>> {
        if self.phase != AdmissionPhase::Terminal
            || self.failure.is_some()
            || self.persistence_failure.is_some()
            || self.record?.disposition() != AgentWorkDisposition::Succeeded
            || (self.result_profile.is_some() && self.artifact.is_none())
        {
            return None;
        }
        self.active.as_mut()?.extraction.take()
    }

    /// Revokes actor execution only. It grants no rendering or human-input right.
    pub(super) fn cancel(&mut self) {
        if let Some(media) = &self.media {
            media.progress.cancel();
        }
        if self.active.is_none()
            || self.request.is_some()
            || self.acquisition.is_some()
            || self.staged.is_some()
        {
            self.fail(AgentWorkFailure::Cancelled);
        }
        if let Some(active) = &self.active {
            active
                .runtime
                .stop_and_seal(AgentRuntimeStopReason::Cancelled);
        }
    }

    /// Bounded reconciliation of the original exact idempotent request, due
    /// on its own after a backoff or asked for explicitly. Old callbacks keep
    /// their own disconnected slots; none can select a new worker or clear the
    /// sticky stop. A Store read is not used as an ACK.
    pub(super) fn reconcile(&mut self) -> bool {
        let Some(flight) = self.flight.take() else {
            return false;
        };
        if !flight.uncertain || flight.reconciliations >= MAX_RECONCILIATIONS {
            self.flight = Some(flight);
            return false;
        }
        self.dispatch_attempt(flight.request, flight.phase, flight.reconciliations + 1);
        true
    }

    /// Cleanup remains possible after any admission failure. This is not human
    /// takeover, persistence settlement, or a global native-zero proof.
    pub(super) fn begin_shutdown(&mut self) {
        self.stopping = true;
        self.request.take();
        self.cancel();
        if self.staged.is_some() {
            self.fail(AgentWorkFailure::Cancelled);
        }
    }

    pub(super) fn poll_shutdown(&mut self, now: AgentPolicyInstant) -> Result<bool, Refusal> {
        self.poll_shutdown_before(now, None)
    }

    fn poll_shutdown_before(
        &mut self,
        now: AgentPolicyInstant,
        deadline: Option<Instant>,
    ) -> Result<bool, Refusal> {
        if !self.stopping {
            return Err(Refusal::Busy);
        }
        self.poll_before(now, deadline);
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(false);
        }
        // A controller can end in Recovery while an exact native callback is
        // still owned by its abandoned operation slot. Drain only callbacks
        // that have actually arrived; an outstanding callback remains counted
        // and therefore continues to block reap/global shutdown below.
        self.owner.drain_abandoned(now)?;
        if self.destroyed {
            let native_clean = self.poll_native_shutdown(deadline)?;
            if native_clean && self.finally_unclean() {
                self.diagnose_classified_recovery_shutdown(deadline);
                return Err(Refusal::Uncertain);
            }
            return Ok(native_clean && self.local_shutdown_settled());
        }
        if self.construction.is_some()
            || self.acquisition.is_some()
            || self.staged.is_some()
            || self
                .active
                .as_ref()
                .is_some_and(|active| !active.completion.is_stopped() || active.lifecycle.is_some())
        {
            return Ok(false);
        }
        if self.destruction.is_none() && !self.destruction_settled {
            self.destruction = Some(self.owner.destroy(&self.resource)?);
        }
        if !self.destruction_settled {
            match self
                .destruction
                .as_mut()
                .ok_or(Refusal::Uncertain)?
                .poll(now)?
            {
                None => return Ok(false),
                Some(LifecycleResult::Event(WorkBrowserResourceEvent::Destroyed(_))) => {
                    self.destruction_settled = true;
                    self.destruction.take();
                }
                Some(_) => return Err(Refusal::Uncertain),
            }
        }
        match self.owner.reap_absent(&self.resource) {
            Ok(_) => {}
            Err(Refusal::Busy) => return Ok(false),
            Err(error) => return Err(error),
        }
        self.owner.seal_resources()?;
        self.destroyed = true;
        let native_clean = self.poll_native_shutdown(deadline)?;
        if native_clean && self.finally_unclean() {
            self.diagnose_classified_recovery_shutdown(deadline);
            return Err(Refusal::Uncertain);
        }
        Ok(native_clean && self.local_shutdown_settled())
    }

    fn diagnose_classified_recovery_shutdown(&self, deadline: Option<Instant>) {
        // Normal application polls also reach this final refusal. Report only
        // the process shutdown attempt, after its original native-zero proof.
        if deadline.is_some() {
            crate::diagnostic!(
                "shutdown: classified retained recovery remains unclean native_zero=true debt_unknown={}",
                self.record
                    .is_some_and(|record| record.debt() == AgentWorkDebt::UNKNOWN)
            );
        }
    }

    /// A final scoped Recovery cannot gain a policy settlement or a different
    /// runtime outcome inside this coordinator. Once its conservative Store
    /// classification is acknowledged there is no logical event left to wait
    /// for. Physical/native cleanup still runs first; the caller then receives
    /// an immediate unclean result instead of losing the remaining global
    /// shutdown budget to a notification that cannot make the run clean.
    fn final_scoped_recovery_is_classified(&self) -> bool {
        self.flight.is_none()
            && self.persistence_failure.is_none()
            && self.record.is_some_and(|record| {
                record.disposition() == AgentWorkDisposition::RecoveryRequired
            })
            && self.active.as_ref().is_some_and(|active| {
                active.completion.is_stopped()
                    && active.lifecycle.is_none()
                    && matches!(active.outcome, Some(AgentWorkRetainedOutcome::Recovery(_)))
            })
    }

    /// A staged actor that never ran keeps its recovery owner once the
    /// journal holds no unsettled request for it: either no row, or its row
    /// classified FailedClosed. Like a classified scoped recovery, nothing in
    /// this process can make it clean, so the page ends unclean, not never.
    fn final_unstarted_is_classified(&self) -> bool {
        self.unstarted.is_some()
            && self.flight.is_none()
            && self.persistence_failure.is_none()
            && self
                .record
                .is_none_or(|record| record.disposition().is_terminal())
    }

    fn finally_unclean(&self) -> bool {
        if self.unstarted.is_some() {
            self.final_unstarted_is_classified()
        } else {
            self.final_scoped_recovery_is_classified()
        }
    }

    fn local_shutdown_settled(&self) -> bool {
        self.destroyed
            && self.owner.locally_retired()
            && self.flight.is_none()
            && self.persistence_failure.is_none()
            && self.unexpected_native.is_none()
            && self.unstarted.is_none()
            && self
                .record
                .is_none_or(|record| record.disposition().is_terminal())
            && self
                .active
                .as_ref()
                .is_none_or(|active| active.drained.is_some())
    }

    pub(super) fn ready_for_group_shutdown(&self) -> bool {
        self.destroyed && self.owner.locally_retired()
    }
    pub(super) fn locally_closed(&self) -> bool {
        self.local_shutdown_settled() && !self.final_scoped_recovery_is_classified()
    }
    pub(super) fn allow_group_shutdown(&mut self) {
        self.group_shutdown = true;
    }

    fn native_audit_ready(&self) -> bool {
        self.local_shutdown_settled()
            || (self.destroyed
                && self.owner.locally_retired()
                && self.unexpected_native.is_none()
                && self.finally_unclean())
    }
    /// A grouped page that could start its native audit once given the turn.
    pub(super) fn awaits_group_audit(&self) -> bool {
        self.grouped
            && !self.group_shutdown
            && self.native_shutdown.is_none()
            && self.native_audit_ready()
    }
    /// Its audit is in flight, or it holds the turn and can start one.
    pub(super) fn holds_native_audit(&self) -> bool {
        self.native_shutdown.as_ref().map_or(
            self.grouped && self.group_shutdown && self.native_audit_ready(),
            RetainedNativeShutdown::in_flight,
        )
    }
    /// Its row cannot be classified in this process any more: its exact
    /// request stayed uncertain past every resend, or it failed with nothing
    /// to send. It keeps that request and its debt for a late acknowledgement
    /// or restart recovery, and so its native seat: its native audit never
    /// precedes the classification.
    pub(super) fn durably_unsettled(&self) -> bool {
        self.persistence_failure.is_some()
            && self.flight.as_ref().is_none_or(|flight| {
                flight.uncertain && flight.reconciliations >= MAX_RECONCILIATIONS
            })
    }
    /// Its audit ended; nothing further can close it cleanly for the group.
    pub(super) fn native_audit_settled(&self) -> bool {
        self.native_shutdown
            .as_ref()
            .is_some_and(|shutdown| !shutdown.in_flight())
    }

    fn poll_native_shutdown(&mut self, deadline: Option<Instant>) -> Result<bool, Refusal> {
        if self.grouped && !self.group_shutdown {
            return Ok(false);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) || !self.native_audit_ready()
        {
            return Ok(false);
        }
        if self.native_shutdown.is_none() {
            self.native_shutdown = Some(RetainedNativeShutdown::new(&self.owner)?);
        }
        self.native_shutdown
            .as_mut()
            .ok_or(Refusal::Uncertain)?
            .poll()
    }

    /// The caller's one absolute process deadline covers scoped worker, Store,
    /// resource callbacks and global native audit retries. The Shell command
    /// queue need not run: wait on the original callback epoch instead. Timeout
    /// never consumes unresolved owners or authorizes a fresh actor.
    pub(super) fn shutdown_until(
        &mut self,
        clock: &dyn TerraControllerClock,
        deadline: Instant,
    ) -> bool {
        self.begin_shutdown();
        let notifications = self.owner.shared.notifications.clone();
        loop {
            if Instant::now() >= deadline {
                return false;
            }
            // Snapshot before polling, not after: a final callback can race
            // the empty poll and the beginning of the blocking wait.
            let Ok(epoch) = notifications.epoch.snapshot() else {
                self.fail(AgentWorkFailure::ContextLost);
                return false;
            };
            let Ok(now) = clock.now() else {
                self.fail(crate::contract_failure!());
                return false;
            };
            if Instant::now() >= deadline {
                return false;
            }
            match self.poll_shutdown_before(now, Some(deadline)) {
                Ok(true) => return Instant::now() < deadline,
                Err(_) => return false,
                Ok(false) => {}
            }
            let wake_at = self
                .next_deadline()
                .map_or(deadline, |next| next.min(deadline));
            if notifications
                .epoch
                .wait_until_changed(epoch, wake_at)
                .is_err()
            {
                self.fail(AgentWorkFailure::ContextLost);
                return false;
            }
        }
    }

    pub(super) fn construction_timed_out(&self) -> bool {
        self.owner
            .shared
            .resource(&self.resource)
            .ok()
            .is_some_and(|resource| {
                resource
                    .health
                    .lock()
                    .is_ok_and(|health| health.construction_timed_out())
            })
    }

    pub(super) fn resource_destroyed(&self) -> bool {
        self.destroyed
    }

    /// Joins the existing application timer; no extra worker or polling thread.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.flight
            .as_ref()
            .filter(|flight| !flight.uncertain || flight.reconciliations < MAX_RECONCILIATIONS)
            .map(|flight| flight.deadline)
            .into_iter()
            .chain(self.staged.as_ref().map(|staged| staged.deadline))
            .chain(
                self.native_shutdown
                    .as_ref()
                    .and_then(RetainedNativeShutdown::next_deadline),
            )
            .min()
    }
}

/// Refuses the next terminal publication before it reaches the Store.
#[cfg(test)]
pub(in crate::work_resources) static REFUSE_PUBLICATION: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Reads every terminal journal acknowledgement as uncertain.
#[cfg(test)]
pub(in crate::work_resources) static UNCONFIRMED_TERMINALS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

const MAX_RECONCILIATIONS: u8 = 4;

/// Half a second, doubling: the last resend leaves within about 15 s of the
/// first uncertainty, past a Store that is only slow, not one that is gone.
fn reconciliation_backoff(reconciliations: u8) -> Duration {
    RECONCILIATION_BACKOFF * (1 << reconciliations.min(MAX_RECONCILIATIONS))
}
#[cfg(not(test))]
const RECONCILIATION_BACKOFF: Duration = Duration::from_millis(500);
#[cfg(test)]
const RECONCILIATION_BACKOFF: Duration = Duration::from_millis(20);

/// A record whose actor is gone and whose debt stays historical: a prior
/// process's interruption, or this process's own scoped recovery.
pub(super) fn reviewable(disposition: AgentWorkDisposition) -> bool {
    matches!(
        disposition,
        AgentWorkDisposition::Interrupted | AgentWorkDisposition::RecoveryRequired
    )
}
/// A fresh native resource may coexist with explicitly classified historical
/// uncertainty. This predicate never permits reuse of an active execution owner.
///
/// FailedClosed is the no-replay classification of an actor the journal
/// admitted but never started: stopped or out of time between the Admitted
/// and Running acknowledgements, or refused at spawn. It is terminal, so no
/// review can follow it, and its UNKNOWN debt stays history like a reviewed
/// row's. Refusing it would refuse every later claim in this Store for good.
fn historical_record_admissible(record: AgentWorkRecord) -> bool {
    record.disposition().is_terminal()
        && (record.debt() == AgentWorkDebt::NONE
            || matches!(
                record.disposition(),
                AgentWorkDisposition::FreshAdmissionRequired
                    | AgentWorkDisposition::Rejected
                    | AgentWorkDisposition::FailedClosed
            ))
}

struct MediaReadState {
    pending: PendingLifecycle,
    progress: zephium_agentic::WorkBrowserMediaProgress,
    deadline: AgentPolicyInstant,
    published: bool,
}
impl RetainedWork {
    pub(super) fn read_media(
        &mut self,
        plans: Vec<zephium_agentic::SemanticRecordMediaPlan>,
        fetch_mode: zephium_agentic::WorkBrowserMediaFetchMode,
        now: AgentPolicyInstant,
        deadline: AgentPolicyInstant,
    ) -> bool {
        if self.media.is_some()
            || self.stopping
            || self.phase != AdmissionPhase::Terminal
            || self
                .record
                .is_none_or(|record| record.disposition() != AgentWorkDisposition::Succeeded)
            || self.artifact.is_none()
        {
            return false;
        }
        let Ok((pending, progress)) =
            self.owner
                .media_lifecycle(&self.resource, plans, fetch_mode, now, deadline)
        else {
            return false;
        };
        progress.set_public_asset_waker(self.waker.clone());
        self.media = Some(MediaReadState {
            pending,
            progress,
            deadline,
            published: false,
        });
        true
    }
    fn poll_media(&mut self, now: AgentPolicyInstant) {
        let Some(mut media) = self.media.take() else {
            return;
        };
        if self.stopping || now >= media.deadline {
            media.progress.cancel();
            if !media.published {
                media.published = true;
                self.media_result = Some(Vec::new());
            }
        }
        match media.pending.poll(now) {
            Ok(None) => self.media = Some(media),
            Ok(Some(LifecycleResult::Event(WorkBrowserResourceEvent::Retained(resource))))
                if resource == self.resource =>
            {
                if !media.published {
                    self.media_result = Some(media.progress.take_result().unwrap_or_default());
                }
            }
            _ => {
                media.progress.cancel();
                if !media.published {
                    self.media_result = Some(Vec::new());
                }
            }
        }
    }
    pub(super) fn take_public_asset(
        &self,
    ) -> Option<zephium_agentic::WorkBrowserPublicAssetRequest> {
        self.media.as_ref()?.progress.take_public_asset()
    }
    pub(super) fn take_media_result(
        &mut self,
    ) -> Option<Vec<zephium_agentic::WorkBrowserRecordImage>> {
        self.media_result.take()
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    #[test]
    fn work_group_accepts_only_its_live_manifest_under_the_original_process_fence() {
        let _serial = crate::WORK_RUNTIME_TEST_SERIAL
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let group = RetainedWorkGroup::try_new(WorkId::generate(), 3).unwrap();
        let owner = AgentWorkIncarnation::generate();
        let mut bytes = [0; AGENT_WORK_RECORD_BYTES];
        bytes[0] = 1;
        bytes[1] = AgentWorkDisposition::Admitted as u8;
        bytes[2] = AgentWorkDebt::UNKNOWN.bits();
        bytes[15] = 1;
        bytes[16..32].copy_from_slice(&owner.bytes());
        bytes[47] = 1;
        let record = AgentWorkRecord::decode(bytes).unwrap();
        let context = ContextId::generate();
        assert!(!group.owns_running(owner, record));
        group.remember(context, record).unwrap();
        assert!(group.owns_running(owner, record));
        let running = record.transition(AgentWorkDisposition::Running).unwrap();
        assert!(group.owns_running(owner, running));
        assert!(!group.owns_running(AgentWorkIncarnation::generate(), running));
        let mut foreign = bytes;
        foreign[95] ^= 1;
        assert!(!group.owns_running(owner, AgentWorkRecord::decode(foreign).unwrap()));
        foreign = bytes;
        foreign[47] = 2;
        let foreign = AgentWorkRecord::decode(foreign).unwrap();
        assert!(!group.owns_running(owner, foreign));
        assert!(group.remember(context, foreign).is_err());
        assert!(!group.owns_running(
            owner,
            running
                .transition(AgentWorkDisposition::RecoveryRequired)
                .unwrap()
        ));
        group.fail();
        assert!(!group.owns_running(owner, running));
        assert!(group.admit(ContextId::generate(), foreign).is_err());
    }
}
