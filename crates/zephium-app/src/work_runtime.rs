//! Product-owned dispatch and exact settlement over the ordinary Shell/Store.
//! Construction is dormant. Deserialized facts can never construct an attempt.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};
use zephium_core::{
    ids::ProfileId,
    work::{artifact::*, port::*, runtime::*, *},
};

type Active = std::sync::Mutex<Vec<(ProfileId, WorkId, WorkPlanNodeId)>>;
/// The app's active runs. Tests run many apps in one process, so each test
/// thread keeps its own.
fn active() -> &'static Active {
    #[cfg(not(test))]
    {
        static ACTIVE: Active = std::sync::Mutex::new(Vec::new());
        &ACTIVE
    }
    #[cfg(test)]
    {
        thread_local! {
            static ACTIVE: &'static Active = Box::leak(Box::default());
        }
        ACTIVE.with(|active| *active)
    }
}
struct Permit((ProfileId, WorkId, WorkPlanNodeId), &'static Active);
impl Permit {
    fn acquire(key: (ProfileId, WorkId, WorkPlanNodeId)) -> Result<Self, WorkError> {
        let registry = active();
        let mut active = registry.lock().map_err(|_| WorkError::Unavailable)?;
        if active.len() >= 4 || active.contains(&key) {
            return Err(WorkError::Capacity);
        }
        active.push(key);
        Ok(Self(key, registry))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.1.lock() {
            active.retain(|key| *key != self.0);
        }
    }
}

struct StartingAgentEntry {
    profile: ProfileId,
    work: WorkId,
    /// Tests isolate worker limits by their original test thread, as Permit does.
    group: &'static Active,
}
fn starting_agents() -> &'static Mutex<Vec<StartingAgentEntry>> {
    static STARTING: OnceLock<Mutex<Vec<StartingAgentEntry>>> = OnceLock::new();
    STARTING.get_or_init(Mutex::default)
}
fn has_starting_agent(profile: ProfileId, work: WorkId) -> bool {
    starting_agents()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|entry| entry.profile == profile && entry.work == work)
}
/// Bounded ownership established before automatic BeginAgent is dispatched.
/// Its absence can identify an unstarted agent without retaining failure records.
pub(crate) struct WorkStartingAgent {
    handle: crate::Handle,
    profile: ProfileId,
    work: WorkId,
}
impl WorkStartingAgent {
    pub(crate) fn acquire(
        handle: &crate::Handle,
        profile: ProfileId,
        work: WorkId,
    ) -> Result<Self, WorkError> {
        let group = active();
        let mut starting = starting_agents().lock().unwrap_or_else(|e| e.into_inner());
        if starting
            .iter()
            .filter(|entry| std::ptr::eq(entry.group, group))
            .count()
            >= 4
            || starting
                .iter()
                .any(|entry| entry.profile == profile && entry.work == work)
        {
            return Err(WorkError::Capacity);
        }
        starting.push(StartingAgentEntry {
            profile,
            work,
            group,
        });
        Ok(Self {
            handle: handle.clone(),
            profile,
            work,
        })
    }
}
impl Drop for WorkStartingAgent {
    fn drop(&mut self) {
        starting_agents()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|entry| entry.profile != self.profile || entry.work != self.work);
        crate::work_changes::publish(self.work);
        let _ = self.handle.dispatch(crate::Command::WorkChanged(
            zephium_ipc::work::WorkChangedV1 {
                profile: self.profile.to_string(),
                work: self.work,
            },
        ));
    }
}

/// Only live attempts are registered; these weak references cannot retain a run.
struct ActiveLifetime {
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
    /// None while Begin is admitted but its reply has not arrived yet.
    state: Option<Weak<Mutex<LifetimeState>>>,
    /// Stop can commit between Begin's callback and lifetime promotion.
    revoked: Option<WorkCancelCause>,
}
fn lifetimes() -> &'static Mutex<Vec<ActiveLifetime>> {
    static LIFETIMES: OnceLock<Mutex<Vec<ActiveLifetime>>> = OnceLock::new();
    LIFETIMES.get_or_init(Mutex::default)
}
/// Pending admission counts as a driver: a store reply can precede promotion
/// to a lifetime, and must never turn that new attempt into an orphan.
pub(crate) fn has_execution_driver(
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
) -> bool {
    has_starting_agent(profile, work)
        || lifetimes()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|entry| {
                entry.profile == profile
                    && entry.work == work
                    && entry.execution == execution
                    && entry
                        .state
                        .as_ref()
                        .is_none_or(|state| state.strong_count() > 0)
            })
}
/// A same-process orphan retains its durable facts and reservations, but the
/// projection cannot describe it as live just because abandonment failed to save.
pub(crate) fn project_driver_liveness(profile: ProfileId, projection: &mut WorkRuntimeProjection) {
    let registered = lifetimes().lock().unwrap_or_else(|e| e.into_inner());
    for execution in &projection.executions {
        if execution.status.terminal() || projection.interrupted.contains(&execution.id) {
            continue;
        }
        let unstarted = execution.authorization == WorkExecutionAuthorization::UserDirectedAgent
            && execution.status == WorkExecutionStatus::Approved
            && execution.attempts.is_empty()
            && !has_starting_agent(profile, projection.work.id)
            && !registered.iter().any(|entry| {
                entry.profile == profile
                    && entry.work == projection.work.id
                    && entry.execution == execution.id
                    && entry
                        .state
                        .as_ref()
                        .is_none_or(|state| state.strong_count() > 0)
            });
        if unstarted
            || execution.attempts.iter().any(|attempt| {
                attempt.status == WorkAttemptStatus::Running
                    && !registered.iter().any(|entry| {
                        entry.profile == profile
                            && entry.work == projection.work.id
                            && entry.execution == execution.id
                            && entry.attempt == attempt.id
                            && entry
                                .state
                                .as_ref()
                                .is_none_or(|state| state.strong_count() > 0)
                    })
            })
        {
            projection.interrupted.push(execution.id);
        }
    }
}
fn unregister_attempt(
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
) {
    lifetimes()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|entry| {
            !(entry.profile == profile
                && entry.work == work
                && entry.execution == execution
                && entry.attempt == attempt)
                && entry
                    .state
                    .as_ref()
                    .is_none_or(|state| state.strong_count() > 0)
        });
}
/// A successful durable publication revokes authority before waking the caller.
/// Existing native resources retain their original cleanup and accounting owners.
pub(crate) fn revoke_published(profile: ProfileId, projection: &WorkRuntimeProjection) {
    let revoked: Vec<_> = {
        let mut registered = lifetimes().lock().unwrap_or_else(|e| e.into_inner());
        registered.retain(|entry| {
            entry
                .state
                .as_ref()
                .is_none_or(|state| state.strong_count() > 0)
        });
        registered
            .iter_mut()
            .filter_map(|entry| {
                if entry.profile != profile || entry.work != projection.work.id {
                    return None;
                }
                let execution = projection
                    .executions
                    .iter()
                    .find(|e| e.id == entry.execution)?;
                let cause = if projection.interrupted.contains(&entry.execution) {
                    WorkCancelCause::Interrupted
                } else if matches!(
                    execution.status,
                    WorkExecutionStatus::CancelRequested | WorkExecutionStatus::Cancelled
                ) {
                    WorkCancelCause::Requested
                } else if execution.status.terminal() {
                    WorkCancelCause::ExecutionSettled
                } else {
                    return None;
                };
                let cause = *entry.revoked.get_or_insert(cause);
                Some((Lifetime(entry.state.as_ref()?.upgrade()?), cause))
            })
            .collect()
    };
    // Session retirement may call back into the host; never hold the registry.
    for (lifetime, cause) in revoked {
        lifetime.cancel(cause);
    }
}

/// Compact responsibility, exact approved scope and completion contract. This
/// is deliberately move-only and not serializable. Executors disclose only the
/// selected textual fields through their own provider policy boundary.
#[must_use]
pub struct WorkNodeAttempt {
    screenshots: Arc<crate::work_screenshots::consent::RunScreenshots>,
    lifetime: Lifetime,
    basis_revision: Arc<Mutex<WorkRevision>>,
    owner: WorkRuntimeSessionId,
    progress: Arc<Mutex<Option<zephium_ipc::work::WorkSignalV1>>>,
    pages: Pages,
    page_wake: Arc<PageProjectionWake>,
    handle: crate::Handle,
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
    node: WorkPlanNode,
    decisions: Vec<zephium_core::work::planning::PlanningAnswer>,
    dependencies: Vec<WorkArtifactV1>,
    spec: WorkNodeExecutionSpec,
    settled: bool,
    _permit: Permit,
}
impl WorkNodeAttempt {
    fn publish_driver_loss(&self) {
        crate::work_changes::publish(self.work);
        let _ = self.handle.dispatch(crate::Command::WorkChanged(
            zephium_ipc::work::WorkChangedV1 {
                profile: self.profile.to_string(),
                work: self.work,
            },
        ));
    }
    pub(crate) async fn runtime_projection(&self) -> Result<WorkRuntimeProjection, WorkError> {
        read(&self.handle, self.profile, self.work).await
    }

    pub(crate) async fn begin_child(
        &self,
        node: WorkPlanNodeId,
    ) -> Result<WorkNodeAttempt, WorkError> {
        if self.cancellation_requested().await? || Instant::now() >= self.deadline() {
            return Err(WorkError::Unavailable);
        }
        let state = self.runtime_projection().await?;
        WorkRuntimeService::new(self.handle.clone())
            .begin(
                self.profile,
                self.work,
                state.work.revision,
                self.execution,
                node,
                Some((self.attempt, self.deadline())),
            )
            .await
    }

    pub(crate) fn retain_child_artifacts(
        &mut self,
        artifacts: Vec<WorkArtifactV1>,
    ) -> Result<(), WorkError> {
        let mut combined = self.dependencies.clone();
        for artifact in artifacts {
            if self.node.dependencies.contains(&artifact.node) {
                if combined.iter().any(|a| a.id == artifact.id) {
                    return Err(WorkError::Invalid);
                }
                combined.push(artifact);
            }
        }
        self.dependencies = compact_dependency_artifacts(combined)?;
        Ok(())
    }
    /// Pure observation; retains neither the attempt nor a browser/resource.
    pub fn observer(&self) -> WorkAttemptObserver {
        WorkAttemptObserver {
            attempt: self.attempt,
            execution: self.execution,
            profile: self.profile,
            work: self.work,
            progress: Arc::downgrade(&self.progress),
            pages: Arc::downgrade(&self.pages),
            page_wake: self.page_wake.clone(),
        }
    }
    pub fn record_activity(&self, activity: zephium_ipc::work::WorkActivityV1) {
        self.probe().record_activity(activity);
    }
    /// A cloneable observation of this live attempt for step runners: it can
    /// read cancellation, report activity and commit steps, never settle or
    /// restart the attempt.
    pub fn probe(&self) -> WorkAttemptProbe {
        WorkAttemptProbe {
            no_usage_charge: matches!(&self.spec.capability, WorkCapability::Agent { grant } if grant.no_usage_charge),
            screenshots: self.screenshots.clone(),
            lifetime: self.lifetime.clone(),
            basis_revision: self.basis_revision.clone(),
            owner: self.owner,
            progress: self.progress.clone(),
            pages: self.pages.clone(),
            page_wake: self.page_wake.clone(),
            handle: self.handle.clone(),
            profile: self.profile,
            work: self.work,
            execution: self.execution,
            attempt: self.attempt,
            node: self.node.id,
        }
    }
    pub(crate) fn mint_artifact(
        &self,
        draft: WorkArtifactDraft,
    ) -> Result<WorkArtifactV1, WorkError> {
        self.mint_marked_artifact(draft, false)
    }
    /// `general_knowledge` marks an object answered from the model's own
    /// knowledge; it may then stand without evidence.
    pub(crate) fn mint_marked_artifact(
        &self,
        draft: WorkArtifactDraft,
        general_knowledge: bool,
    ) -> Result<WorkArtifactV1, WorkError> {
        let expected = self
            .node
            .outputs
            .iter()
            .find(|o| o.name == draft.output)
            .ok_or(WorkError::Invalid)?;
        let artifact = WorkArtifactV1 {
            revises: None,
            part: None,
            version: 1,
            id: WorkArtifactId::generate(),
            execution: self.execution,
            node: self.node.id,
            attempt: self.attempt,
            output: draft.output,
            title: draft.title,
            data: draft.data,
            evidence: draft.evidence,
            review: expected.review,
            presentation: WorkArtifactPresentationV1::Automatic,
            general_knowledge,
        };
        artifact.validate()?;
        Ok(artifact)
    }
    pub fn profile(&self) -> ProfileId {
        self.profile
    }
    pub fn work(&self) -> WorkId {
        self.work
    }
    /// Durable changes to this attempt's work from now on, to wait on instead of polling.
    pub(crate) fn changes(&self) -> crate::work_changes::WorkChanges {
        crate::work_changes::watch(self.work)
    }
    pub fn execution(&self) -> WorkExecutionId {
        self.execution
    }
    pub fn attempt(&self) -> WorkAttemptId {
        self.attempt
    }
    pub fn node(&self) -> &WorkPlanNode {
        &self.node
    }
    pub fn decisions(&self) -> &[zephium_core::work::planning::PlanningAnswer] {
        &self.decisions
    }
    pub fn disclosure_objective(&self) -> Result<String, WorkError> {
        assignment_objective(&self.node.objective, &self.decisions)
    }
    /// Only successful outputs of this node's approved direct dependencies,
    /// from this exact execution. No sibling graph or provider transcript.
    pub fn dependency_artifacts(&self) -> &[WorkArtifactV1] {
        &self.dependencies
    }
    pub fn specification(&self) -> &WorkNodeExecutionSpec {
        &self.spec
    }
    pub fn deadline(&self) -> Instant {
        self.lifetime.deadline()
    }

    pub(crate) async fn read_evidence(
        &self,
        link: WorkEvidenceLink,
    ) -> Result<WorkEvidencePreviewV1, WorkError> {
        match request(
            &self.handle,
            self.profile,
            WorkRequest::ReadEvidence {
                id: self.work,
                link,
            },
        )
        .await?
        {
            WorkReply::Evidence(preview) => Ok(preview),
            _ => Err(WorkError::Invalid),
        }
    }

    /// Authoritative cancellation comes from durable intent. Delayed transient
    /// signals cannot cancel or restart an attempt. Owner changes fail closed.
    pub async fn cancellation_requested(&self) -> Result<bool, WorkError> {
        self.probe().cancellation_requested().await
    }

    /// Called by a trusted typed adapter only after its original worker and
    /// required resource ownership have settled. Unknown outcomes have no
    /// apparent zero-cost refund. User IPC cannot call this operation.
    pub async fn settle(
        self,
        result: WorkAdapterResult,
    ) -> Result<WorkRuntimeProjection, WorkError> {
        self.settle_owned(result)
            .await
            .map(WorkNodeSettlement::into_projection)
    }

    /// Preserve the original acknowledged publication, or the person's durable
    /// cancellation of this exact owned attempt, for an owning parent. Decoded
    /// projections alone cannot construct this move-only receipt.
    pub async fn settle_owned(
        self,
        result: WorkAdapterResult,
    ) -> Result<WorkNodeSettlement, WorkError> {
        self.settle_with_provider_evidence(result, None).await
    }

    /// Recover only the original agent's durable terminal bookkeeping. Store
    /// decides whether a published reply is complete or effects remain unknown;
    /// this carries no replacement result, artifacts, or execution permission.
    pub(crate) async fn recover_owned(mut self) -> Result<WorkNodeSettlement, WorkError> {
        if !matches!(&self.spec.capability, WorkCapability::Agent { .. }) {
            return Err(WorkError::Invalid);
        }
        let reply = request(
            &self.handle,
            self.profile,
            WorkRequest::RuntimeAbandon {
                id: self.work,
                execution: self.execution,
                attempt: self.attempt,
            },
        )
        .await?;
        let WorkReply::Runtime(projection) = reply else {
            return Err(WorkError::Invalid);
        };
        if projection.work.id != self.work
            || projection.work.profile != self.profile
            || !projection
                .owners
                .iter()
                .any(|entry| entry.execution == self.execution && entry.owner == self.owner)
            || !projection.executions.iter().any(|execution| {
                execution.id == self.execution
                    && execution.status.terminal()
                    && execution.attempts.iter().any(|attempt| {
                        attempt.id == self.attempt
                            && attempt.node == self.node.id
                            && attempt.status != WorkAttemptStatus::Running
                    })
            })
        {
            return Err(WorkError::Invalid);
        }
        self.settled = true;
        Ok(WorkNodeSettlement {
            profile: self.profile,
            work: self.work,
            execution: self.execution,
            attempt: self.attempt,
            node: self.node.id,
            projection: *projection,
        })
    }

    pub(crate) async fn settle_with_provider_evidence(
        mut self,
        result: WorkAdapterResult,
        mut provider_evidence: Option<WorkProviderSearchRecordV1>,
    ) -> Result<WorkNodeSettlement, WorkError> {
        if result.status == WorkAttemptStatus::Running {
            return Err(WorkError::Invalid);
        }
        let mut status = result.status;
        let mut drafts = Some(result.artifacts);
        let mut artifacts = Vec::new();
        // Retry only persistence after a definite CAS conflict. Never repeat
        // adapter execution, change the attempt, or retry an uncertain write.
        for _ in 0..4 {
            let state = read(&self.handle, self.profile, self.work).await?;
            // Stop closes the logical run before native cleanup necessarily
            // finishes. Its original owner may observe that terminal receipt,
            // but cannot publish a late result or replace unknown accounting.
            if state
                .owners
                .iter()
                .any(|entry| entry.execution == self.execution && entry.owner == self.owner)
                && state.executions.iter().any(|execution| {
                    execution.id == self.execution
                        && execution.status == WorkExecutionStatus::Cancelled
                        && execution.attempts.iter().any(|attempt| {
                            attempt.id == self.attempt
                                && attempt.node == self.node.id
                                && attempt.status != WorkAttemptStatus::Running
                        })
                })
            {
                self.settled = true;
                return Ok(WorkNodeSettlement {
                    profile: self.profile,
                    work: self.work,
                    execution: self.execution,
                    attempt: self.attempt,
                    node: self.node.id,
                    projection: state,
                });
            }
            if let Some(drafts) = drafts.take() {
                artifacts = drafts
                    .into_iter()
                    .map(|draft| self.mint_artifact(draft))
                    .collect::<Result<Vec<_>, WorkError>>()?;
            }
            let update = if let Some(evidence) = &provider_evidence {
                WorkRuntimeUpdate::SettleProviderSearch {
                    execution: self.execution,
                    attempt: self.attempt,
                    status,
                    usage: result.usage,
                    artifacts: artifacts.clone(),
                    evidence: Box::new(evidence.clone()),
                }
            } else {
                WorkRuntimeUpdate::Settle {
                    execution: self.execution,
                    attempt: self.attempt,
                    status,
                    usage: result.usage,
                    artifacts: artifacts.clone(),
                    intervention: result.intervention.clone(),
                }
            };
            match request(
                &self.handle,
                self.profile,
                WorkRequest::RuntimeUpdate {
                    id: self.work,
                    expected: state.work.revision,
                    update,
                },
            )
            .await
            {
                Ok(WorkReply::Runtime(state)) => {
                    if let Some(record) = &provider_evidence {
                        probe_source_icons(&self.handle, self.profile, record);
                    }
                    self.settled = true;
                    return Ok(WorkNodeSettlement {
                        profile: self.profile,
                        work: self.work,
                        execution: self.execution,
                        attempt: self.attempt,
                        node: self.node.id,
                        projection: *state,
                    });
                }
                Err(WorkError::Conflict) => continue,
                Err(error @ (WorkError::Invalid | WorkError::Capacity))
                    if matches!(&self.spec.capability, WorkCapability::Agent { .. }) =>
                {
                    crate::work_trace::record(format_args!(
                        "work: phase=attempt stage=terminal_write_refused error={error:?}"
                    ));
                    return self.recover_owned().await;
                }
                // A definite capacity refusal rolled back publication. Preserve
                // the original worker's known usage with a small failed fact;
                // never repeat provider work or treat an uncertain write this way.
                Err(WorkError::Capacity)
                    if status == WorkAttemptStatus::Succeeded && result.usage.is_some() =>
                {
                    status = WorkAttemptStatus::Failed;
                    artifacts.clear();
                    provider_evidence = None;
                }
                Err(error) => return Err(error),
                Ok(_) => return Err(WorkError::Invalid),
            }
        }
        if matches!(&self.spec.capability, WorkCapability::Agent { .. }) {
            crate::work_trace::record(format_args!(
                "work: phase=attempt stage=terminal_write_refused error=Conflict"
            ));
            self.recover_owned().await
        } else {
            Err(WorkError::Conflict)
        }
    }
}

/// Fresh settlement of the original owned attempt, including a durable Stop
/// acknowledged by that attempt after its worker drains. This is not an
/// execution capability; serialization loses its original ownership.
#[must_use]
pub struct WorkNodeSettlement {
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
    node: WorkPlanNodeId,
    projection: WorkRuntimeProjection,
}
impl WorkNodeSettlement {
    pub fn profile(&self) -> ProfileId {
        self.profile
    }
    pub fn work(&self) -> WorkId {
        self.work
    }
    pub fn execution(&self) -> WorkExecutionId {
        self.execution
    }
    pub fn attempt(&self) -> WorkAttemptId {
        self.attempt
    }
    pub fn node(&self) -> WorkPlanNodeId {
        self.node
    }
    pub fn projection(&self) -> &WorkRuntimeProjection {
        &self.projection
    }
    pub fn into_projection(self) -> WorkRuntimeProjection {
        self.projection
    }
}
/// The person-facing view of one browser step's page: its address and the
/// newest bounded frame. Replaced in place, dropped with the attempt.
#[derive(Clone)]
pub struct WorkPageFrame {
    pub step: WorkStepId,
    pub url: String,
    pub live: bool,
    pub frame: Option<Arc<zephium_agentic::WorkBrowserFrame>>,
}
type Pages = Arc<Mutex<BTreeMap<WorkStepId, WorkPageFrame>>>;

fn publish_page_frame(
    pages: &Pages,
    wake: &PageProjectionWake,
    step: WorkStepId,
    url: &str,
    frame: Option<Arc<zephium_agentic::WorkBrowserFrame>>,
) {
    let url: String = url.chars().take(2048).collect();
    let changed = if let Ok(mut pages) = pages.lock() {
        if pages.len() >= 16 && !pages.contains_key(&step) {
            let Some(settled) = pages.values().find(|page| !page.live).map(|page| page.step) else {
                return;
            };
            pages.remove(&settled);
        }
        // Existing one-second activity sampling handles ordinary replacements.
        // Full application invalidation is bounded to the first usable picture
        // and settlement, independent of how many native frames arrive.
        let changed = frame.is_some() && pages.get(&step).is_none_or(|prior| prior.frame.is_none());
        pages.insert(
            step,
            WorkPageFrame {
                step,
                url,
                live: true,
                frame,
            },
        );
        changed
    } else {
        false
    };
    // Never notify while holding the page data lock: consumers can snapshot now.
    if changed {
        wake.notify();
    }
}

fn settle_page_frame(pages: &Pages, wake: &PageProjectionWake, step: WorkStepId) {
    let changed = if let Ok(mut pages) = pages.lock() {
        if let Some(page) = pages.get_mut(&step) {
            let changed = page.live;
            page.live = false;
            changed
        } else {
            false
        }
    } else {
        false
    };
    if changed {
        wake.notify();
    }
}

#[derive(Default)]
struct PageProjectionWake {
    pending: std::sync::atomic::AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}
impl PageProjectionWake {
    fn notify(&self) {
        use std::sync::atomic::Ordering;
        if self.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let wake = self.waker.lock().ok().and_then(|slot| slot.clone());
        if let Some(wake) = wake {
            // Projection notifications do not alter native or execution state.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| wake.wake()));
        }
    }
    fn consume(&self) {
        self.pending
            .swap(false, std::sync::atomic::Ordering::AcqRel);
    }
}
#[derive(Clone)]
pub struct WorkAttemptObserver {
    attempt: WorkAttemptId,
    execution: WorkExecutionId,
    profile: ProfileId,
    work: WorkId,
    progress: std::sync::Weak<Mutex<Option<zephium_ipc::work::WorkSignalV1>>>,
    pages: std::sync::Weak<Mutex<BTreeMap<WorkStepId, WorkPageFrame>>>,
    page_wake: Arc<PageProjectionWake>,
}
impl WorkAttemptObserver {
    /// Retains only this attempt's bounded page pictures. The consumer must
    /// bound the number of retained attempts; this owns no execution or browser.
    pub fn retain_pages(&self) -> Option<WorkPageFrames> {
        Some(WorkPageFrames {
            pages: self.pages.upgrade()?,
            page_wake: self.page_wake.clone(),
            progress: self.progress.clone(),
            profile: self.profile,
            work: self.work,
        })
    }
    pub fn attempt(&self) -> WorkAttemptId {
        self.attempt
    }
    pub fn execution(&self) -> WorkExecutionId {
        self.execution
    }
    pub fn is_alive(&self) -> bool {
        self.progress.strong_count() > 0
    }
    /// At most one replaceable transient signal; idle observers own no timer.
    pub fn latest(&self) -> Option<zephium_ipc::work::WorkSignalV1> {
        self.progress.upgrade()?.lock().ok()?.clone()
    }
    /// Pages this attempt has opened, newest frame each.
    pub fn pages(&self) -> Vec<WorkPageFrame> {
        self.pages
            .upgrade()
            .and_then(|pages| {
                pages
                    .lock()
                    .ok()
                    .map(|pages| pages.values().cloned().collect())
            })
            .unwrap_or_default()
    }
}
/// A bounded page-data mailbox that survives the producer's final update and
/// destruction, so a slower UI poll can still consume its last picture.
pub struct WorkPageFrames {
    pages: Pages,
    page_wake: Arc<PageProjectionWake>,
    progress: std::sync::Weak<Mutex<Option<zephium_ipc::work::WorkSignalV1>>>,
    profile: ProfileId,
    work: WorkId,
}

#[cfg(test)]
mod page_mailbox_tests {
    use super::*;

    struct CountingWake {
        calls: std::sync::atomic::AtomicUsize,
        pages: Pages,
    }
    impl std::task::Wake for CountingWake {
        fn wake(self: Arc<Self>) {
            assert!(
                self.pages.try_lock().is_ok(),
                "notifications must release the producer lock"
            );
            self.calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
    #[test]
    fn first_picture_and_final_state_wake_without_invalidating_each_frame() {
        use std::sync::atomic::Ordering;
        let pages = Arc::new(Mutex::new(BTreeMap::new()));
        let wake = Arc::new(PageProjectionWake::default());
        let mailbox = WorkPageFrames {
            pages: pages.clone(),
            page_wake: wake.clone(),
            progress: std::sync::Weak::new(),
            profile: ProfileId::generate(),
            work: WorkId::generate(),
        };
        let counter = Arc::new(CountingWake {
            calls: std::sync::atomic::AtomicUsize::new(0),
            pages: pages.clone(),
        });
        let step = WorkStepId::generate();
        let frame = |generation| {
            Some(Arc::new(zephium_agentic::WorkBrowserFrame {
                generation,
                width: 1,
                height: 1,
                png: Arc::new(vec![generation as u8]),
            }))
        };
        publish_page_frame(&pages, &wake, step, "https://fixture.invalid/", None);
        assert!(mailbox.register_projection_waker(counter.clone().into()));
        assert!(!mailbox.register_projection_waker(counter.clone().into()));
        assert_eq!(counter.calls.load(Ordering::Acquire), 0);
        publish_page_frame(&pages, &wake, step, "https://fixture.invalid/", frame(1));
        for generation in 2..5 {
            publish_page_frame(
                &pages,
                &wake,
                step,
                "https://fixture.invalid/",
                frame(generation),
            );
        }
        assert_eq!(counter.calls.load(Ordering::Acquire), 1);
        assert_eq!(mailbox.pages()[0].frame.as_ref().unwrap().generation, 4);
        publish_page_frame(&pages, &wake, step, "https://fixture.invalid/", frame(4));
        assert_eq!(counter.calls.load(Ordering::Acquire), 1);
        publish_page_frame(&pages, &wake, step, "https://fixture.invalid/", frame(5));
        assert_eq!(counter.calls.load(Ordering::Acquire), 1);
        settle_page_frame(&pages, &wake, step);
        assert_eq!(counter.calls.load(Ordering::Acquire), 2);
        settle_page_frame(&pages, &wake, step);
        assert_eq!(counter.calls.load(Ordering::Acquire), 2);
        assert_eq!(mailbox.pages()[0].frame.as_ref().unwrap().generation, 5);
    }

    #[test]
    fn final_page_picture_survives_producer_exit_without_retaining_execution() {
        let step = WorkStepId::generate();
        let progress = Arc::new(Mutex::new(None));
        let pages = Arc::new(Mutex::new(BTreeMap::from([(
            step,
            WorkPageFrame {
                step,
                url: "https://fixture.invalid/consent".into(),
                live: true,
                frame: None,
            },
        )])));
        let weak_pages = Arc::downgrade(&pages);
        let observer = WorkAttemptObserver {
            attempt: WorkAttemptId::generate(),
            execution: WorkExecutionId::generate(),
            profile: ProfileId::generate(),
            work: WorkId::generate(),
            progress: Arc::downgrade(&progress),
            pages: weak_pages.clone(),
            page_wake: Arc::new(PageProjectionWake::default()),
        };
        let mailbox = observer.retain_pages().unwrap();
        assert!(mailbox.is_alive());
        assert!(mailbox.pages()[0].frame.is_none());
        // The result arrives and the producer exits entirely between UI polls.
        {
            let mut pages = pages.lock().unwrap();
            let page = pages.get_mut(&step).unwrap();
            page.frame = Some(Arc::new(zephium_agentic::WorkBrowserFrame {
                generation: 2,
                width: 1,
                height: 1,
                png: Arc::new(vec![2]),
            }));
            page.live = false;
        }
        drop(pages);
        drop(progress);
        drop(observer);
        assert!(!mailbox.is_alive());
        let final_pages = mailbox.pages();
        assert!(!final_pages[0].live);
        assert_eq!(final_pages[0].frame.as_ref().unwrap().generation, 2);
        drop(mailbox);
        assert!(weak_pages.upgrade().is_none());
    }
}
impl WorkPageFrames {
    /// Installs one data-only application invalidation receiver. This receiver
    /// retains no producer, execution lease, resource, native page, or timer.
    pub fn register_projection_waker(&self, waker: std::task::Waker) -> bool {
        let pending = {
            let Ok(mut slot) = self.page_wake.waker.lock() else {
                return false;
            };
            if slot.is_some() {
                return false;
            }
            *slot = Some(waker.clone());
            self.page_wake
                .pending
                .load(std::sync::atomic::Ordering::Acquire)
        };
        if pending {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| waker.wake()));
        }
        true
    }
    pub fn scope(&self) -> (ProfileId, WorkId) {
        (self.profile, self.work)
    }
    pub fn is_alive(&self) -> bool {
        self.progress.strong_count() > 0
    }
    pub fn pages(&self) -> Vec<WorkPageFrame> {
        self.page_wake.consume();
        self.pages
            .lock()
            .map(|pages| pages.values().cloned().collect())
            .unwrap_or_default()
    }
}
/// Closed reason an attempt must stop; a development log fact, never text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkCancelCause {
    /// Another owner or attempt holds this execution now.
    Unowned,
    /// The store marked the execution interrupted.
    Interrupted,
    /// The person asked to stop.
    Requested,
    /// The execution already settled.
    ExecutionSettled,
    /// Some attempt of this execution is no longer running or succeeded.
    AttemptSettled,
    /// The attempt's own deadline passed.
    Deadline,
    /// The durable state could not be read.
    Unreadable,
}
/// The attempt's deadline and the anonymous browser scope bound to it. Time
/// spent waiting for the person, or with a person on one of its pages, does
/// not count against the attempt.
#[derive(Clone)]
struct Lifetime(Arc<Mutex<LifetimeState>>);
struct LifetimeState {
    deadline: Instant,
    session: zephium_agentic::WorkBrowserSession,
    closed: bool,
    cancellation: tokio::sync::watch::Sender<Option<WorkCancelCause>>,
    work: WorkId,
    /// Pages a person holds now, and since when the first of them was held.
    held: u32,
    held_since: Option<Instant>,
}
impl LifetimeState {
    fn deadline(&self) -> Instant {
        self.held_since
            .map_or(self.deadline, |since| self.deadline + since.elapsed())
    }
}
impl Lifetime {
    fn new(
        profile: ProfileId,
        work: WorkId,
        execution: WorkExecutionId,
        attempt: WorkAttemptId,
        deadline: Instant,
    ) -> Self {
        let lifetime = Self(Arc::new(Mutex::new(LifetimeState {
            deadline,
            session: zephium_agentic::WorkBrowserSession::new(profile, work, deadline),
            closed: false,
            cancellation: tokio::sync::watch::channel(None).0,
            work,
            held: 0,
            held_since: None,
        })));
        let mut registered = lifetimes().lock().unwrap_or_else(|e| e.into_inner());
        registered.retain(|entry| {
            entry
                .state
                .as_ref()
                .is_none_or(|state| state.strong_count() > 0)
        });
        let entry = registered
            .iter_mut()
            .find(|entry| {
                entry.profile == profile
                    && entry.work == work
                    && entry.execution == execution
                    && entry.attempt == attempt
            })
            .expect("the exact Begin was registered before dispatch");
        entry.state = Some(Arc::downgrade(&lifetime.0));
        let revoked = entry.revoked;
        drop(registered);
        if let Some(cause) = revoked {
            lifetime.cancel(cause);
        }
        lifetime
    }
    fn state(&self) -> std::sync::MutexGuard<'_, LifetimeState> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn deadline(&self) -> Instant {
        self.state().deadline()
    }
    /// The browser scope for a page about to open. A scope whose deadline the
    /// attempt has since outlived is replaced; pages already open keep theirs.
    fn session(&self) -> zephium_agentic::WorkBrowserSession {
        let mut state = self.state();
        let deadline = state.deadline();
        if !state.closed && !state.session.is_current() && Instant::now() < deadline {
            state.session = zephium_agentic::WorkBrowserSession::new(
                state.session.profile(),
                state.work,
                deadline,
            );
        }
        state.session.clone()
    }
    fn close(&self) {
        let session = {
            let mut state = self.state();
            state.closed = true;
            state.session.clone()
        };
        session.close();
    }
    fn cancel(&self, cause: WorkCancelCause) {
        self.close();
        self.state().cancellation.send_if_modified(|current| {
            if current.is_some() {
                return false;
            }
            *current = Some(cause);
            true
        });
    }
    /// Gives back the time the attempt spent waiting for the person.
    fn extend(&self, waited: Duration) {
        let mut state = self.state();
        if !state.closed {
            state.deadline += waited;
        }
    }
    fn hold(&self) {
        let mut state = self.state();
        state.held += 1;
        state.held_since.get_or_insert_with(Instant::now);
    }
    fn release(&self) {
        let mut state = self.state();
        state.held = state.held.saturating_sub(1);
        if state.held == 0 {
            if let Some(since) = state.held_since.take() {
                state.deadline += since.elapsed();
            }
        }
    }
}
/// A person holds one of the attempt's pages: the attempt's deadline stands
/// still until this is dropped. The page's own deadline is unchanged.
#[must_use]
pub struct WorkPersonHold(Lifetime);
impl Drop for WorkPersonHold {
    fn drop(&mut self) {
        self.0.release();
    }
}
#[derive(Clone)]
pub struct WorkAttemptProbe {
    no_usage_charge: bool,
    screenshots: Arc<crate::work_screenshots::consent::RunScreenshots>,
    lifetime: Lifetime,
    basis_revision: Arc<Mutex<WorkRevision>>,
    owner: WorkRuntimeSessionId,
    progress: Arc<Mutex<Option<zephium_ipc::work::WorkSignalV1>>>,
    pages: Pages,
    page_wake: Arc<PageProjectionWake>,
    handle: crate::Handle,
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
    node: WorkPlanNodeId,
}
impl WorkAttemptProbe {
    /// Original admitted pricing fact; it grants no page or provider authority.
    pub fn no_usage_charge(&self) -> bool {
        self.no_usage_charge
    }

    pub(crate) async fn wait_image_cancelled(&self) {
        let mut receiver = self.lifetime.state().cancellation.subscribe();
        loop {
            if receiver.borrow_and_update().is_some() {
                return;
            }
            if receiver.changed().await.is_err() {
                return;
            }
        }
    }

    pub(crate) fn screenshot_choices(
        &self,
    ) -> &Arc<crate::work_screenshots::consent::RunScreenshots> {
        &self.screenshots
    }

    #[cfg(feature = "work-execution")]
    pub async fn admit_read_page(
        &self,
        step: WorkStepId,
    ) -> Result<crate::RetainedPageAdmission, WorkError> {
        if self.cancellation_requested().await? || Instant::now() >= self.deadline() {
            return Err(WorkError::Invalid);
        }
        let state = self.runtime_projection().await?;
        let execution = state
            .executions
            .iter()
            .find(|execution| execution.id == self.execution)
            .ok_or(WorkError::NotFound)?;
        if execution.authorization != WorkExecutionAuthorization::UserDirectedAgent
            || !execution.steps.iter().any(|fact| {
                fact.id == step
                    && fact.status == WorkStepStatus::Running
                    && matches!(
                        fact.kind,
                        WorkStepKindV1::Read { .. } | WorkStepKindV1::Search { .. }
                    )
            })
        {
            return Err(WorkError::Invalid);
        }
        Ok(crate::RetainedPageAdmission {
            profile: self.profile,
            work: self.work,
            execution: self.execution,
            attempt: self.attempt,
            step,
            workers: execution.spec.limits.max_workers.min(3),
            deadline: self.deadline(),
        })
    }
    pub fn browser_session(&self) -> zephium_agentic::WorkBrowserSession {
        self.lifetime.session()
    }
    pub fn profile(&self) -> ProfileId {
        self.profile
    }
    pub fn work(&self) -> WorkId {
        self.work
    }
    pub fn execution(&self) -> WorkExecutionId {
        self.execution
    }
    pub fn attempt(&self) -> WorkAttemptId {
        self.attempt
    }
    pub fn node(&self) -> WorkPlanNodeId {
        self.node
    }
    pub fn deadline(&self) -> Instant {
        self.lifetime.deadline()
    }
    /// Time spent waiting for the person does not count against the attempt.
    pub(crate) fn resume_after_wait(&self, waited: Duration) {
        self.lifetime.extend(waited);
    }
    /// A person took one of this attempt's pages; the attempt's deadline
    /// stands still until the returned hold is dropped.
    pub fn hold_for_person(&self) -> WorkPersonHold {
        self.lifetime.hold();
        WorkPersonHold(self.lifetime.clone())
    }
    /// The person let the run keep going: its limits grow durably first.
    pub async fn extend_limits(&self, limits: WorkExecutionLimits) -> Result<(), WorkError> {
        self.commit_step(WorkRuntimeUpdate::ExtendLimits {
            execution: self.execution,
            attempt: self.attempt,
            limits,
        })
        .await
        .map(|_| ())
    }
    /// Publishes optional immutable media only for this exact still-live read.
    pub async fn put_record_media(
        &self,
        media: zephium_core::work::media::WorkRecordMediaRefV1,
        png: Vec<u8>,
    ) -> Result<zephium_core::work::media::WorkRecordMediaRefV1, WorkError> {
        if media.profile != self.profile
            || media.work != self.work
            || media.execution != self.execution
            || media.attempt != self.attempt
            || self.cached_cancellation().is_some()
        {
            return Err(WorkError::Conflict);
        }
        match request(
            &self.handle,
            self.profile,
            WorkRequest::PutRecordMedia { media, png },
        )
        .await?
        {
            WorkReply::RecordMediaStored(media) => Ok(media),
            _ => Err(WorkError::Invalid),
        }
    }
    /// Durably records conservative possible image disclosure before any
    /// image request may dispatch. Permission/source checks remain independent.
    /// A missing, late, stopped or wrong-owner acknowledgement refuses the send.
    pub async fn record_image_disclosure(
        &self,
        step: WorkStepId,
        disclosure: WorkPageImageDisclosureV1,
    ) -> Result<(), WorkError> {
        if self.cached_cancellation().is_some() {
            return Err(WorkError::Conflict);
        }
        let projection = self
            .commit_step(WorkRuntimeUpdate::MarkImageDisclosure {
                execution: self.execution,
                attempt: self.attempt,
                step,
                disclosure,
            })
            .await?;
        let recorded = projection
            .executions
            .iter()
            .find(|execution| execution.id == self.execution)
            .filter(|execution| execution.status == WorkExecutionStatus::Running)
            .filter(|execution| {
                execution.attempts.iter().any(|attempt| {
                    attempt.id == self.attempt && attempt.status == WorkAttemptStatus::Running
                })
            })
            .and_then(|execution| {
                execution.steps.iter().find(|read| {
                    read.id == step
                        && read.status == WorkStepStatus::Running
                        && matches!(read.kind, WorkStepKindV1::Read { .. })
                })
            })
            .and_then(|read| read.measurements)
            .and_then(|measurement| measurement.image_disclosure)
            .is_some_and(|recorded| recorded.merge(disclosure) == recorded);
        if !recorded || self.cancellation_requested().await? {
            return Err(WorkError::Conflict);
        }
        Ok(())
    }
    /// Persists the observed public title of a running page-read step.
    pub async fn record_page_title(&self, step: WorkStepId, title: &str) -> Result<(), WorkError> {
        self.commit_step(WorkRuntimeUpdate::PageTitle {
            execution: self.execution,
            attempt: self.attempt,
            step,
            title: title.to_owned(),
        })
        .await
        .map(|_| ())
    }
    /// Records the newest frame of one browser step's page, bounded per attempt.
    pub fn record_page_frame(
        &self,
        step: WorkStepId,
        url: &str,
        frame: Option<Arc<zephium_agentic::WorkBrowserFrame>>,
    ) {
        publish_page_frame(&self.pages, &self.page_wake, step, url, frame);
    }
    /// The page's step settled: keep its last frame, drop its live mark.
    pub fn settle_page(&self, step: WorkStepId) {
        settle_page_frame(&self.pages, &self.page_wake, step);
    }
    pub fn record_activity(&self, activity: zephium_ipc::work::WorkActivityV1) {
        if let Ok(mut signal) = self.progress.lock() {
            *signal = Some(zephium_ipc::work::WorkSignalV1 {
                version: 1,
                owner: self.owner,
                profile: self.profile.to_string(),
                work: self.work,
                basis_revision: *self
                    .basis_revision
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()),
                execution: self.execution,
                node: self.node,
                attempt: self.attempt,
                activity,
            });
        }
    }
    pub async fn runtime_projection(&self) -> Result<WorkRuntimeProjection, WorkError> {
        read(&self.handle, self.profile, self.work).await
    }
    /// Durable changes to this attempt's work from now on, to wait on instead of polling.
    pub(crate) fn changes(&self) -> crate::work_changes::WorkChanges {
        crate::work_changes::watch(self.work)
    }
    pub async fn cancellation_requested(&self) -> Result<bool, WorkError> {
        self.cancellation_cause().await.map(|cause| cause.is_some())
    }
    /// Asks the store to stop this execution as the person's Stop would, so
    /// the attempt settles as stopped rather than failed.
    pub(crate) async fn request_stop(&self) -> Result<(), WorkError> {
        let mut conflicts = 0;
        loop {
            let state = self.runtime_projection().await?;
            let command = WorkRequest::RuntimeCommand {
                id: self.work,
                expected: state.work.revision,
                command: WorkCommandId::generate(),
                intent: WorkRuntimeIntent::Cancel {
                    execution: self.execution,
                    intervention: None,
                },
            };
            let submitted = admitted(|| {
                self.handle
                    .submit_work_document(command.clone(), Some(self.profile))
            })
            .await?;
            let reply = tokio::time::timeout(Duration::from_secs(10), submitted)
                .await
                .map_err(|_| WorkError::OutcomeUnknown)?;
            match reply {
                Err(WorkError::Conflict) if conflicts < 2 => conflicts += 1,
                Err(error) => return Err(error),
                Ok(_) => return Ok(()),
            }
        }
    }
    /// A monotonic revocation already observed from durable state, without I/O.
    pub fn cached_cancellation(&self) -> Option<WorkCancelCause> {
        *self.lifetime.state().cancellation.borrow()
    }
    /// Why this attempt must stop, from durable revocation or a fresh read;
    /// None while it still owns a running execution.
    pub async fn cancellation_cause(&self) -> Result<Option<WorkCancelCause>, WorkError> {
        if let Some(cause) = self.cached_cancellation() {
            return Ok(Some(cause));
        }
        let state = read(&self.handle, self.profile, self.work).await?;
        let execution = state
            .executions
            .iter()
            .find(|e| e.id == self.execution)
            .ok_or(WorkError::NotFound)?;
        let owned = state
            .owners
            .iter()
            .any(|entry| entry.execution == self.execution && entry.owner == self.owner)
            && execution.attempts.iter().any(|attempt| {
                attempt.id == self.attempt
                    && attempt.node == self.node
                    && attempt.status == WorkAttemptStatus::Running
            });
        if owned && !state.interrupted.contains(&self.execution) {
            // Only the original attempt advances its observation basis after a
            // fresh durable read. Historical projections never mint an observer.
            if let Ok(mut basis) = self.basis_revision.lock() {
                *basis = state.work.revision;
            }
            if let Ok(mut signal) = self.progress.lock() {
                if let Some(signal) = signal.as_mut() {
                    signal.basis_revision = state.work.revision;
                }
            }
        }
        let cause = if !owned {
            Some(WorkCancelCause::Unowned)
        } else if state.interrupted.contains(&self.execution) {
            Some(WorkCancelCause::Interrupted)
        } else if execution.status == WorkExecutionStatus::CancelRequested {
            Some(WorkCancelCause::Requested)
        } else if execution.status.terminal() {
            Some(WorkCancelCause::ExecutionSettled)
        } else if execution.attempts.iter().any(|attempt| {
            !matches!(
                attempt.status,
                WorkAttemptStatus::Running | WorkAttemptStatus::Succeeded
            )
        }) {
            Some(WorkCancelCause::AttemptSettled)
        } else {
            None
        };
        if let Some(cause) = cause {
            self.lifetime.cancel(cause);
        }
        Ok(cause)
    }
    /// Waits for durable revocation even while an adapter future does not poll.
    /// The publication signal also wins over a stalled durable read. A fallback
    /// handles store changes made outside the shell without a busy polling loop.
    pub async fn wait_cancelled(&self) -> WorkCancelCause {
        let mut cancelled = self.lifetime.state().cancellation.subscribe();
        let mut changes = self.changes();
        loop {
            if let Some(cause) = *cancelled.borrow_and_update() {
                return cause;
            }
            tokio::select! {
                biased;
                _ = cancelled.changed() => continue,
                result = self.cancellation_cause() => {
                    if let Ok(Some(cause)) = result {
                        return cause;
                    }
                }
            }
            tokio::select! {
                biased;
                _ = cancelled.changed() => {},
                _ = changes.next(Duration::from_millis(100)) => {},
            }
        }
    }
    pub(crate) async fn read_evidence(
        &self,
        link: WorkEvidenceLink,
    ) -> Result<WorkEvidencePreviewV1, WorkError> {
        match request(
            &self.handle,
            self.profile,
            WorkRequest::ReadEvidence {
                id: self.work,
                link,
            },
        )
        .await?
        {
            WorkReply::Evidence(preview) => Ok(preview),
            _ => Err(WorkError::Invalid),
        }
    }
    /// Commit one step under the live attempt. Persistence retries only a
    /// definite CAS conflict; a capacity refusal downgrades a successful step
    /// to a failed one without its payload rather than losing the record.
    pub(crate) async fn commit_step(
        &self,
        mut update: WorkRuntimeUpdate,
    ) -> Result<WorkRuntimeProjection, WorkError> {
        // Parallel parts commit to the same run at once: a lost race reads
        // the fresh revision and tries again after a short, growing pause.
        for attempt in 0..COMMIT_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(5 * u64::from(attempt))).await;
            }
            let state = read(&self.handle, self.profile, self.work).await?;
            match request(
                &self.handle,
                self.profile,
                WorkRequest::RuntimeUpdate {
                    id: self.work,
                    expected: state.work.revision,
                    update: update.clone(),
                },
            )
            .await
            {
                Ok(WorkReply::Runtime(state)) => {
                    if let WorkRuntimeUpdate::SettleStep {
                        evidence: Some(record),
                        ..
                    }
                    | WorkRuntimeUpdate::BeginStep {
                        evidence: Some(record),
                        ..
                    } = &update
                    {
                        probe_source_icons(&self.handle, self.profile, record);
                    }
                    return Ok(*state);
                }
                Err(WorkError::Conflict) => continue,
                Err(WorkError::Capacity) => match &mut update {
                    WorkRuntimeUpdate::SettleStep {
                        status,
                        artifacts,
                        evidence,
                        file,
                        note,
                        ..
                    } if *status == WorkStepStatus::Succeeded => {
                        *status = WorkStepStatus::Failed;
                        artifacts.clear();
                        *evidence = None;
                        *file = None;
                        *note = Some(KEPT_NOTHING.into());
                    }
                    WorkRuntimeUpdate::BeginStep {
                        step,
                        artifacts,
                        evidence,
                        file,
                        ..
                    } if !artifacts.is_empty() => {
                        step.status = WorkStepStatus::Failed;
                        step.artifacts.clear();
                        step.evidence = None;
                        step.note = Some(KEPT_NOTHING.into());
                        artifacts.clear();
                        *evidence = None;
                        *file = None;
                    }
                    _ => return Err(WorkError::Capacity),
                },
                Err(error) => return Err(error),
                Ok(_) => return Err(WorkError::Invalid),
            }
        }
        Err(WorkError::Conflict)
    }
}
/// Tries one runtime commit gets against concurrent commits of its run.
const COMMIT_ATTEMPTS: u32 = 12;

/// Sources a search admitted get their site icons without being opened.
fn probe_source_icons(
    handle: &crate::Handle,
    profile: ProfileId,
    record: &WorkProviderSearchRecordV1,
) {
    let origins: Vec<String> = record
        .evidence
        .citations
        .iter()
        .map(|citation| citation.url.clone())
        .collect();
    if !origins.is_empty() {
        let _ = handle.dispatch(crate::Command::ProbeFavicons { profile, origins });
    }
}
/// A step whose result the execution record had no room left to keep.
const KEPT_NOTHING: &str = "The work has grown too large to keep this result";
impl Drop for WorkNodeAttempt {
    fn drop(&mut self) {
        self.lifetime.close();
        unregister_attempt(self.profile, self.work, self.execution, self.attempt);
        if let Ok(mut progress) = self.progress.lock() {
            progress.take();
        }
        if !self.settled {
            self.publish_driver_loss();
            // No detached future/thread. If admission itself fails, the retained
            // running fact still requires recovery; no success is manufactured.
            let _ = self.handle.submit_owned_work_runtime(
                WorkRequest::RuntimeAbandon {
                    id: self.work,
                    execution: self.execution,
                    attempt: self.attempt,
                },
                self.profile,
            );
        }
    }
}

/// Adapter result data. Evidence links must be obtained from the adapter's
/// original durable source publication. They must not be invented by a model.
pub struct WorkArtifactDraft {
    pub output: String,
    pub title: String,
    pub data: WorkArtifactDataV1,
    pub evidence: Vec<WorkEvidenceLink>,
}
pub struct WorkAdapterResult {
    pub status: WorkAttemptStatus,
    pub usage: Option<WorkUsage>,
    pub artifacts: Vec<WorkArtifactDraft>,
    /// Closed reason automation stopped for a person; never page text.
    pub intervention: Option<WorkInterventionV1>,
}

pub struct WorkRuntimeService {
    handle: crate::Handle,
}
struct PendingBegin<'a> {
    handle: &'a crate::Handle,
    profile: ProfileId,
    work: WorkId,
    execution: WorkExecutionId,
    attempt: WorkAttemptId,
    armed: bool,
}
impl Drop for PendingBegin<'_> {
    fn drop(&mut self) {
        if self.armed {
            unregister_attempt(self.profile, self.work, self.execution, self.attempt);
            crate::work_changes::publish(self.work);
            let _ = self.handle.dispatch(crate::Command::WorkChanged(
                zephium_ipc::work::WorkChangedV1 {
                    profile: self.profile.to_string(),
                    work: self.work,
                },
            ));
            let _ = self.handle.submit_owned_work_runtime(
                WorkRequest::RuntimeAbandon {
                    id: self.work,
                    execution: self.execution,
                    attempt: self.attempt,
                },
                self.profile,
            );
        }
    }
}
impl WorkRuntimeService {
    pub fn new(handle: crate::Handle) -> Self {
        Self { handle }
    }
    /// The user has already approved the exact plan and concrete scopes. This
    /// is a fresh dispatch, not resumption by deserialization. Store independently
    /// checks its incarnation, deadline, dependency readiness and reservation.
    pub async fn begin_node(
        &self,
        profile: ProfileId,
        work: WorkId,
        expected: WorkRevision,
        execution: WorkExecutionId,
        node: WorkPlanNodeId,
    ) -> Result<WorkNodeAttempt, WorkError> {
        self.begin(profile, work, expected, execution, node, None)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn begin(
        &self,
        profile: ProfileId,
        work: WorkId,
        expected: WorkRevision,
        execution: WorkExecutionId,
        node: WorkPlanNodeId,
        parent: Option<(WorkAttemptId, Instant)>,
    ) -> Result<WorkNodeAttempt, WorkError> {
        let attempt = WorkAttemptId::generate();
        {
            let mut registered = lifetimes().lock().unwrap_or_else(|e| e.into_inner());
            registered.retain(|entry| {
                entry
                    .state
                    .as_ref()
                    .is_none_or(|state| state.strong_count() > 0)
            });
            registered.push(ActiveLifetime {
                profile,
                work,
                execution,
                attempt,
                state: None,
                revoked: None,
            });
        }
        let mut pending = PendingBegin {
            handle: &self.handle,
            profile,
            work,
            execution,
            attempt,
            armed: true,
        };
        // Approval already exists: every failure from here must notify the
        // store, including refusal by the process-wide worker limit.
        let permit = Permit::acquire((profile, work, node))?;
        let submitted = Instant::now();
        let reply = request(
            &self.handle,
            profile,
            WorkRequest::RuntimeUpdate {
                id: work,
                expected,
                update: match parent {
                    Some((parent, _)) => WorkRuntimeUpdate::BeginChild {
                        execution,
                        attempt,
                        node,
                        parent,
                    },
                    None => WorkRuntimeUpdate::Begin {
                        execution,
                        attempt,
                        node,
                    },
                },
            },
        )
        .await?;
        let WorkReply::RuntimeStarted {
            projection,
            remaining_millis,
        } = reply
        else {
            return Err(WorkError::Invalid);
        };
        let execution_fact = projection
            .executions
            .iter()
            .find(|e| e.id == execution)
            .ok_or(WorkError::Invalid)?;
        let spec = execution_fact
            .spec
            .nodes
            .iter()
            .find(|n| n.node == node)
            .cloned()
            .ok_or(WorkError::Invalid)?;
        let node_plan = projection
            .work
            .plan
            .as_ref()
            .and_then(|p| p.draft.nodes.iter().find(|n| n.id == node))
            .cloned()
            .ok_or(WorkError::Invalid)?;
        let dependencies = compact_dependency_artifacts(
            execution_fact
                .artifacts
                .iter()
                .filter(|artifact| node_plan.dependencies.contains(&artifact.node))
                .cloned()
                .collect(),
        )?;
        // Subtract queue/Store time conservatively by anchoring at submission;
        // neither callback latency nor a new worker renews the deadline.
        let deadline = submitted
            + Duration::from_millis(u64::from(remaining_millis))
                .min(Duration::from_secs(u64::from(spec.limits.timeout_seconds)));
        let deadline = parent.map_or(deadline, |(_, parent_deadline)| {
            deadline.min(parent_deadline)
        });
        let owned = WorkNodeAttempt {
            screenshots: Arc::new(crate::work_screenshots::consent::RunScreenshots::new(
                profile,
            )),
            lifetime: Lifetime::new(profile, work, execution, attempt, deadline),
            basis_revision: Arc::new(Mutex::new(projection.work.revision)),
            owner: projection
                .owners
                .iter()
                .find(|entry| entry.execution == execution)
                .ok_or(WorkError::Invalid)?
                .owner,
            progress: Arc::new(Mutex::new(None)),
            pages: Arc::new(Mutex::new(BTreeMap::new())),
            page_wake: Arc::new(PageProjectionWake::default()),
            handle: self.handle.clone(),
            profile,
            work,
            execution,
            attempt,
            node: node_plan,
            decisions: decision_context(&projection.work),
            dependencies,
            spec,
            settled: false,
            _permit: permit,
        };
        pending.armed = false;
        if Instant::now() >= deadline {
            return Err(WorkError::Unavailable);
        }
        Ok(owned)
    }
}
async fn request(
    handle: &crate::Handle,
    profile: ProfileId,
    request: WorkRequest,
) -> Result<WorkReply, WorkError> {
    let submitted = admitted(|| handle.submit_owned_work_runtime(request.clone(), profile)).await?;
    let response = tokio::time::timeout(Duration::from_secs(10), submitted)
        .await
        .map_err(|_| WorkError::OutcomeUnknown)??;
    if response.profile != profile {
        return Err(WorkError::ProfileUnavailable);
    }
    Ok(response.reply)
}
/// The store admits a few Work documents at a time across the app. A
/// refusal at that bound sent nothing: a run's reads and writes wait for a
/// place in line instead of failing their step.
pub(crate) async fn admitted<T>(
    mut submit: impl FnMut() -> Result<T, WorkError>,
) -> Result<T, WorkError> {
    const WAIT: Duration = Duration::from_secs(8);
    let started = Instant::now();
    let mut delay = Duration::from_millis(10);
    loop {
        match submit() {
            Err(WorkError::Capacity) if started.elapsed() < WAIT => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(200));
            }
            result => {
                let waited = started.elapsed();
                if result.is_err() || waited >= Duration::from_millis(500) {
                    crate::work_trace::record(format_args!(
                        "work: phase=store_admission waited_ms={} refused={:?}",
                        waited.as_millis(),
                        result.as_ref().err()
                    ));
                }
                return result;
            }
        }
    }
}
pub(crate) fn decision_context(
    work: &WorkSnapshot,
) -> Vec<zephium_core::work::planning::PlanningAnswer> {
    work.current_questions()
        .filter_map(|q| {
            q.answer
                .as_ref()
                .map(|answer| zephium_core::work::planning::PlanningAnswer {
                    question: q.prompt.clone(),
                    answer: answer.clone(),
                })
        })
        .collect()
}

pub(crate) fn assignment_objective(
    objective: &str,
    decisions: &[zephium_core::work::planning::PlanningAnswer],
) -> Result<String, WorkError> {
    let mut text = objective.to_owned();
    if !decisions.is_empty() {
        text.push_str(
            "\nUser decisions for this Work (task context, not additional permissions):\n",
        );
        text.push_str(&serde_json::to_string(decisions).map_err(|_| WorkError::Invalid)?);
    }
    if text.len() > MAX_WORK_TEXT_BYTES {
        return Err(WorkError::Capacity);
    }
    Ok(text)
}

async fn read(
    handle: &crate::Handle,
    profile: ProfileId,
    work: WorkId,
) -> Result<WorkRuntimeProjection, WorkError> {
    match request(handle, profile, WorkRequest::RuntimeRead { id: work }).await? {
        WorkReply::Runtime(state) => Ok(*state),
        _ => Err(WorkError::Invalid),
    }
}

/// Model-facing dependency projection only. Original Store publications and
/// all identity/evidence joins stay intact; the notice makes lost text explicit.
fn compact_dependency_artifacts(
    original: Vec<WorkArtifactV1>,
) -> Result<Vec<WorkArtifactV1>, WorkError> {
    const LIMIT: usize = 6 * 1024;
    const NOTICE: &str =
        "\n[Host-truncated dependency summary; complete artifact retained in Work.]";
    let fits = |items: &[WorkArtifactV1]| -> Result<bool, WorkError> {
        Ok(serde_json::to_vec(items)
            .map_err(|_| WorkError::Invalid)?
            .len()
            <= LIMIT)
    };
    if fits(&original)? {
        return Ok(original);
    }
    // Include the notice in the sizing projection even for complete summaries:
    // this makes the serialized prefix bound monotonic as allowance grows.
    // The returned projection labels only actual truncation.
    let project = |allowance: usize, sizing: bool| {
        original
            .iter()
            .cloned()
            .map(|mut artifact| {
                if let WorkArtifactDataV1::EvidenceCollection { summary, .. } = &mut artifact.data {
                    let prefix: String = summary.chars().take(allowance).collect();
                    let truncated = prefix.len() < summary.len();
                    *summary = prefix;
                    if truncated || sizing {
                        summary.push_str(NOTICE);
                    }
                }
                artifact
            })
            .collect::<Vec<_>>()
    };
    if !fits(&project(128, true))? {
        return Err(WorkError::Capacity);
    }
    let mut low = 128usize;
    let mut high = LIMIT;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if fits(&project(middle, true))? {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    Ok(project(low, false))
}

#[cfg(test)]
mod driver_liveness_tests {
    use super::*;

    #[test]
    fn work_pending_begin_drop_revokes_liveness_and_notifies_before_abandonment() {
        let profile = ProfileId::generate();
        let work = WorkId::generate();
        let execution = WorkExecutionId::generate();
        let attempt = WorkAttemptId::generate();
        let queue = crate::actor::CommandQueue::new();
        let handle = crate::Handle::new(queue.clone());
        let mut changes = crate::work_changes::watch(work);
        lifetimes().lock().unwrap().push(ActiveLifetime {
            profile,
            work,
            execution,
            attempt,
            state: None,
            revoked: Some(WorkCancelCause::Requested),
        });
        let pending = PendingBegin {
            handle: &handle,
            profile,
            work,
            execution,
            attempt,
            armed: true,
        };
        assert!(
            has_execution_driver(profile, work, execution),
            "a pending Begin is not an orphan"
        );
        assert!(!has_execution_driver(
            ProfileId::generate(),
            work,
            execution
        ));
        assert!(!has_execution_driver(
            profile,
            WorkId::generate(),
            execution
        ));
        assert!(!has_execution_driver(
            profile,
            work,
            WorkExecutionId::generate()
        ));
        let lifetime = Lifetime::new(
            profile,
            work,
            execution,
            attempt,
            Instant::now() + Duration::from_secs(30),
        );
        assert!(
            !lifetime.session().is_current(),
            "Stop before promotion cannot create fresh authority"
        );
        drop(pending);
        assert!(!has_execution_driver(profile, work, execution));
        assert!(changes.take(), "waiting runtimes observe failed promotion");
        assert!(
            matches!(queue.try_recv(), Some(crate::Command::WorkChanged(change))
            if change.profile == profile.to_string() && change.work == work),
            "the UI is notified even if the later abandonment cannot be delivered"
        );
        assert!(matches!(
            queue.try_recv(),
            Some(crate::Command::WorkDocument(_))
        ));
        assert!(queue.try_recv().is_none());
    }
}
