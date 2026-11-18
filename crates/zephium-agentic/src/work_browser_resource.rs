//! Work-owned browser identity and revocable, run-bound native lease protocol.
//!
//! This is not a task, account, navigation or effect capability. The trusted
//! application must supply those separately. No native adapter is enabled by
//! this functional core; unsupported adapters cannot manufacture drain proof.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;
use ulid::Ulid;
use zephium_core::ids::ProfileId;

use crate::{
    AgentPolicyInstant, ContextId, ContextNavigationTarget, ContextPortFailure,
    ContextProfileStorageClass, ContextRunId, MAX_EXECUTING_CONTEXTS, MAX_LIVE_CONTEXTS,
    MAX_PENDING_NATIVE_CONTEXT_TASKS,
};

pub use zephium_core::ids::WorkId;
crate::context::durable_id!(
    WorkBrowserResourceId,
    "Durable identity of one browser resource, independent of every actor run."
);

/// Persistent, non-executing identity. Restoring it restores no native lease.
#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBrowserResourceIdentity {
    work: WorkId,
    resource: WorkBrowserResourceId,
    profile: ProfileId,
    context: ContextId,
}

impl WorkBrowserResourceIdentity {
    /// Owning Work, not the current actor run.
    pub const fn work(self) -> WorkId {
        self.work
    }
    /// Stable browser resource identity.
    pub const fn resource(self) -> WorkBrowserResourceId {
        self.resource
    }
    /// Profile retained for the entire resource lifetime.
    pub const fn profile(self) -> ProfileId {
        self.profile
    }
    /// Stable native-page identity, never replaced by a lease identity.
    pub const fn context(self) -> ContextId {
        self.context
    }
}

impl fmt::Debug for WorkBrowserResourceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WorkBrowserResourceIdentity([redacted])")
    }
}

// The private allocation prevents an identically reconstructed registry from
// accepting another registry's receipts. It is never serialized or exposed.
#[derive(Clone)]
struct Authority(Arc<()>);
impl PartialEq for Authority {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for Authority {}
impl fmt::Debug for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// Exact process-local incarnation of a persistent resource; not a capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkBrowserResourceJoin {
    identity: WorkBrowserResourceIdentity,
    authority: Authority,
    incarnation: u64,
}
impl WorkBrowserResourceJoin {
    /// Persistent ownership coordinates.
    pub const fn identity(&self) -> WorkBrowserResourceIdentity {
        self.identity
    }
}

/// Exact run-bound lease coordinates. No page ownership is transferred.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkBrowserExecutionLease {
    resource: WorkBrowserResourceJoin,
    run: ContextRunId,
    generation: u64,
    deadline: AgentPolicyInstant,
}
impl WorkBrowserExecutionLease {
    /// Same persistent resource throughout this lease and its retirement.
    pub const fn resource(&self) -> &WorkBrowserResourceJoin {
        &self.resource
    }
    /// Actor run holding this exact execution lease only.
    pub const fn run(&self) -> ContextRunId {
        self.run
    }
    /// Original absolute deadline; observations and renewals cannot reset it.
    pub const fn deadline(&self) -> AgentPolicyInstant {
        self.deadline
    }
}

/// Resource lifecycle, independent of presentation and full run completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkBrowserResourcePhase {
    /// Native construction has not yet proven its result.
    Constructing,
    /// Native resource is retained with no executing actor lease.
    Retained,
    /// Agent admission is sealed while native human presentation is pending.
    PresentingHuman,
    /// Only the person may interact with this exact retained page.
    PresentedHuman,
    /// Native hiding and document rebinding must finish before agent admission.
    ContinuingAfterHuman,
    /// One exact native lease binding is awaiting acknowledgement.
    Acquiring,
    /// The exact acknowledged execution lease is active.
    Leased,
    /// Native acquisition finished after expiry or shutdown admission sealing;
    /// execution is forbidden and its exact lease still requires revocation.
    RevocationRequired,
    /// Execution admission is closed; exact native drain remains owed.
    Revoking,
    /// This resource has uncertain native state and cannot execute again.
    Quarantined,
    /// Native destruction and all resource callbacks remain owed.
    Destroying,
    /// Exact destruction or synchronous non-construction proves native absence;
    /// persistent metadata remains and old callback debt may still be retained.
    Destroyed,
}

/// Content-free cause retained without poisoning unrelated resource rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkBrowserResourceFailure {
    /// The exact native adapter refused or could not prove its operation.
    NativeRefused,
    /// A native terminal did not match its requested operation class.
    Contract,
    /// Lease-owned native work or the retained page was not proven drained.
    DrainUnproven,
    /// An accepted native callback did not arrive within the owner's budget.
    CallbackUncertain,
    /// Trusted monotonic time regressed for this resource.
    ClockRegression,
}

/// Functional-core refusal. No error grants retry or native authority.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum WorkBrowserResourceError {
    /// No matching resource or private authority exists.
    #[error("stale Work browser resource authority")]
    Stale,
    /// A resource/context identifier is already present.
    #[error("duplicate Work browser resource identity")]
    Duplicate,
    /// Resource or execution capacity is retained by other exact owners.
    #[error("Work browser resource capacity exhausted")]
    Capacity,
    /// Required phase or callback disposition is absent.
    #[error("Work browser resource phase mismatch")]
    Phase,
    /// The immutable lease deadline was reached or is invalid.
    #[error("Work browser execution lease expired")]
    Expired,
    /// This resource's native state is uncertain.
    #[error("Work browser resource is quarantined")]
    Quarantined,
    /// An exact native callback is still owned.
    #[error("Work browser resource callback is pending")]
    Pending,
    /// Monotonic identity space cannot wrap or be reset.
    #[error("Work browser resource identity exhausted")]
    Exhausted,
    /// Work shutdown has permanently closed acquisition/construction.
    #[error("Work browser resource admission is sealed")]
    Sealed,
    /// The frozen initial source cannot be observed through the HTTP(S) model.
    #[error("Work browser resource source is unsupported")]
    Source,
}

/// Closed operation classes; none grants page-level or model authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkBrowserResourceOperation {
    /// Optional bounded native media read on the drained same page.
    ReadMedia,
    /// Construct one extension-free resource under its selected profile.
    Construct,
    /// Bind one run lease to the existing exact page.
    Acquire,
    /// Revoke and drain only the exact run lease, retaining the page.
    Revoke,
    /// Explicitly destroy the exact resource and its resource-owned channels.
    Destroy,
    /// Present the drained page under an explicit bounded human-control request.
    PresentHuman,
    /// Hide the human page and bind its final document without starting an actor.
    ContinueAfterHuman,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OperationJoin {
    resource: WorkBrowserResourceJoin,
    sequence: u64,
    kind: WorkBrowserResourceOperation,
    lease: Option<WorkBrowserExecutionLease>,
}

/// Code-owned retry policy; the original Work deadline always remains binding.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkBrowserConstructionAttempt {
    /// Original page-loading window.
    #[default]
    Initial,
    /// One longer retry after a native page-loading timeout.
    SlowPageRetry,
}
impl WorkBrowserConstructionAttempt {
    /// Closed per-attempt ceiling, intersected with the original deadline.
    pub const fn budget(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Self::Initial => 30,
            Self::SlowPageRetry => 60,
        })
    }
}

#[derive(Debug, Default)]
struct ConstructionSettings {
    anonymous_session: Option<crate::WorkBrowserSession>,
    work_website_data: bool,
    redirect: Option<crate::WorkBrowserRedirect>,
    unreachable: Option<crate::WorkBrowserUnreachable>,
    window: Option<(WorkBrowserConstructionAttempt, std::time::Instant)>,
}

/// Move-only exact native request, minted after publishing its pending owner.
#[must_use]
#[derive(Debug)]
pub struct WorkBrowserResourceRequest {
    construction: Option<Box<ConstructionSettings>>,
    media: Option<Box<media::MediaRead>>,
    operation: Box<OperationJoin>,
    storage: ContextProfileStorageClass,
    isolated_public: bool,
    document: Option<Arc<ContextNavigationTarget>>,
    document_policy: crate::WorkBrowserDocumentPolicy,
    delivery: Option<Box<delivery::DeliveryDispatch>>,
    health: Option<Box<WorkBrowserResourceHealthReporter>>,
    human: Option<Box<human::HumanWindow>>,
}
impl WorkBrowserResourceRequest {
    /// Binds trusted retry policy and the original absolute Work deadline once.
    pub fn with_construction_window(
        mut self,
        attempt: WorkBrowserConstructionAttempt,
        deadline: std::time::Instant,
    ) -> Result<Self, Box<Self>> {
        if self.operation() != WorkBrowserResourceOperation::Construct
            || self.construction_window().is_some()
            || deadline <= std::time::Instant::now()
            || (attempt == WorkBrowserConstructionAttempt::SlowPageRetry && !self.isolated_public)
        {
            return Err(Box::new(self));
        }
        self.construction
            .get_or_insert_with(Default::default)
            .window = Some((attempt, deadline));
        Ok(self)
    }
    /// Exact construction-only policy; ordinary lifecycle successors have none.
    pub fn construction_window(
        &self,
    ) -> Option<(WorkBrowserConstructionAttempt, std::time::Instant)> {
        self.construction
            .as_ref()
            .and_then(|settings| settings.window)
    }

    /// Attaches attempt-owned storage only to exact anonymous construction.
    pub fn with_anonymous_session(
        mut self,
        session: crate::WorkBrowserSession,
    ) -> Result<Self, Box<Self>> {
        let identity = self.resource().identity();
        if self.operation() != WorkBrowserResourceOperation::Construct
            || !self.isolated_public
            || self.anonymous_session().is_some()
            || self.work_website_data()
            || !session.admits(identity.profile(), identity.work())
        {
            return Err(Box::new(self));
        }
        self.construction
            .get_or_insert_with(Default::default)
            .anonymous_session = Some(session);
        Ok(self)
    }
    /// Optional anonymous storage scope; never profile authentication.
    pub fn anonymous_session(&self) -> Option<&crate::WorkBrowserSession> {
        self.construction
            .as_ref()
            .and_then(|settings| settings.anonymous_session.as_ref())
    }

    /// Opens the page in Work's own persistent website data for its profile:
    /// separate from the profile's store and from every run's anonymous one.
    pub fn with_work_website_data(mut self) -> Result<Self, Box<Self>> {
        if self.operation() != WorkBrowserResourceOperation::Construct
            || !self.isolated_public
            || self.anonymous_session().is_some()
            || self.work_website_data()
        {
            return Err(Box::new(self));
        }
        self.construction
            .get_or_insert_with(Default::default)
            .work_website_data = true;
        Ok(self)
    }
    /// Whether construction uses Work's own persistent website data.
    pub fn work_website_data(&self) -> bool {
        self.construction
            .as_ref()
            .is_some_and(|settings| settings.work_website_data)
    }

    /// Where the native gate keeps a public document's refused redirect,
    /// for exact public construction only.
    pub fn with_redirect(
        mut self,
        redirect: crate::WorkBrowserRedirect,
    ) -> Result<Self, Box<Self>> {
        if self.operation() != WorkBrowserResourceOperation::Construct
            || !self.isolated_public
            || !self.document_policy.is_public()
            || self.redirect().is_some()
        {
            return Err(Box::new(self));
        }
        self.construction
            .get_or_insert_with(Default::default)
            .redirect = Some(redirect);
        Ok(self)
    }
    /// Where the native view says its main-frame load could not reach the
    /// site, for any construction.
    pub fn with_unreachable(
        mut self,
        unreachable: crate::WorkBrowserUnreachable,
    ) -> Result<Self, Box<Self>> {
        if self.operation() != WorkBrowserResourceOperation::Construct
            || self.unreachable().is_some()
        {
            return Err(Box::new(self));
        }
        self.construction
            .get_or_insert_with(Default::default)
            .unreachable = Some(unreachable);
        Ok(self)
    }
    /// The unreachable slot of a construction, if one was bound.
    pub fn unreachable(&self) -> Option<&crate::WorkBrowserUnreachable> {
        self.construction
            .as_ref()
            .and_then(|settings| settings.unreachable.as_ref())
    }
    /// The refused-redirect slot of a public construction, if one was bound.
    pub fn redirect(&self) -> Option<&crate::WorkBrowserRedirect> {
        self.construction
            .as_ref()
            .and_then(|settings| settings.redirect.as_ref())
    }

    /// Attaches one stable resource-health observer to original construction.
    /// Other operations and repeated attachment return the request losslessly.
    pub fn track_resource_health(mut self) -> Result<(Self, WorkBrowserResourceHealth), Box<Self>> {
        if self.operation.kind != WorkBrowserResourceOperation::Construct || self.health.is_some() {
            return Err(Box::new(self));
        }
        let (observer, reporter) = health::track(self.operation.resource.clone());
        self.health = Some(Box::new(reporter));
        Ok((self, observer))
    }
    /// Transfers the stable reporting owner once to the trusted native resource.
    /// Unhandled observers fail closed when the original request is consumed.
    pub fn take_resource_health_reporter(&mut self) -> Option<WorkBrowserResourceHealthReporter> {
        self.health.take().map(|reporter| *reporter)
    }
    /// Exact persistent and process-local resource coordinates.
    pub const fn resource(&self) -> &WorkBrowserResourceJoin {
        &self.operation.resource
    }
    /// Closed requested transition.
    pub const fn operation(&self) -> WorkBrowserResourceOperation {
        self.operation.kind
    }
    /// Exact lease when acquiring or revoking; destruction may retain old debt.
    pub const fn lease(&self) -> Option<&WorkBrowserExecutionLease> {
        self.operation.lease.as_ref()
    }
    /// Immutable selected-profile persistence class; never inferred by a model.
    pub const fn storage(&self) -> ContextProfileStorageClass {
        self.storage
    }
    /// Storage separate from the profile's cookies: a run's anonymous store,
    /// a fresh one, or Work's own (`work_website_data`).
    pub const fn isolated_public(&self) -> bool {
        self.isolated_public
    }
    /// Admission-frozen initial document, absent for an empty resource. Native
    /// construction must install selected-profile policy before loading it and
    /// refuse redirects, substitutions and any later page navigation.
    pub fn document(&self) -> Option<&ContextNavigationTarget> {
        self.document.as_deref()
    }
    /// Immutable construction policy, never supplied by a page or model.
    pub const fn document_policy(&self) -> crate::WorkBrowserDocumentPolicy {
        self.document_policy
    }
    /// Frozen native presentation operands; absent from ordinary actor operations.
    pub fn human_region(&self) -> Option<WorkBrowserHumanRegion> {
        self.human.as_ref().and_then(|human| human.region)
    }
    /// Native document-change reporting without page content or URL disclosure.
    pub fn human_progress(&self) -> Option<WorkBrowserHumanProgress> {
        self.human.as_ref().map(|human| human.progress.clone())
    }
    /// Original absolute human wait deadline, never renewed by presentation.
    pub fn human_deadline(&self) -> Option<AgentPolicyInstant> {
        self.human.as_ref().map(|human| human.deadline)
    }
    /// A sign-in presentation: navigation may cross sites until it settles back.
    pub fn human_sign_in(&self) -> bool {
        self.human.as_ref().is_some_and(|human| human.sign_in)
    }
    /// Exact source whose origin bounds human navigation; it grants no actor scope.
    pub fn human_source(&self) -> Option<&ContextNavigationTarget> {
        self.human.as_ref().map(|human| human.source.as_ref())
    }
    /// Native proof that the page is hidden and its final document is frozen.
    pub fn complete_human_document(
        self,
        effective: ContextNavigationTarget,
    ) -> WorkBrowserResourceCompletion {
        let mut completion = self.complete(WorkBrowserResourceNativeOutcome::HumanContinued);
        completion.effective_document = Some(Arc::new(effective));
        completion
    }
    /// Attest the frozen native location after the exact initial navigation.
    /// The original operation owner binds the requested/effective lineage.
    pub fn complete_document(
        self,
        effective: ContextNavigationTarget,
    ) -> WorkBrowserResourceCompletion {
        let mut completion = self.complete(WorkBrowserResourceNativeOutcome::Constructed);
        completion.effective_document = Some(Arc::new(effective));
        completion
    }
    /// Transfers the optional exact delivery-barrier owner to the native task.
    /// It must stay beside the original task permit until the terminal callback
    /// returns. Leaving it unhandled fails closed, including on older adapters.
    pub fn take_lease_delivery_completion(&mut self) -> Option<WorkBrowserLeaseDeliveryCompletion> {
        self.delivery.as_mut()?.completion.take()
    }
    /// Consume the native request to settle once. The trusted adapter must
    /// inspect its original physical owners; constructing a value is not proof.
    pub fn complete(
        self,
        outcome: WorkBrowserResourceNativeOutcome,
    ) -> WorkBrowserResourceCompletion {
        WorkBrowserResourceCompletion {
            operation: *self.operation,
            outcome,
            delivery: self.delivery.map(|delivery| delivery.binding),
            effective_document: None,
        }
    }
}

/// Lease-owned native debt only. Resource-owned page/profile/document channels
/// remain live and counted separately; policy/provider/audit debt is not native.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkBrowserLeaseNativeDebt {
    /// Accepted lease tasks not yet consumed by the native executor.
    pub queued_tasks: u8,
    /// Lease observation requests still physically owned.
    pub observations: u8,
    /// Native actions without an exact terminal receipt.
    pub actions: u8,
    /// Lease navigations or their terminal ownership still in flight.
    pub navigations: u8,
    /// Physical captures or cancellation receipts still owned.
    pub captures: u8,
    /// Completion callbacks still able to refer to this lease.
    pub callbacks: u8,
}
impl WorkBrowserLeaseNativeDebt {
    fn is_empty(self) -> bool {
        self == Self::default()
    }
    fn bounded(self) -> bool {
        [
            self.queued_tasks,
            self.observations,
            self.actions,
            self.navigations,
            self.captures,
            self.callbacks,
        ]
        .into_iter()
        .all(|count| usize::from(count) <= MAX_PENDING_NATIVE_CONTEXT_TASKS)
    }
}

/// Trusted native facts, bound by consuming the exact original request.
/// Unsupported/refused/unknown facts must never be represented as success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkBrowserResourceNativeOutcome {
    /// Optional media terminal arrived; original resource remains owned.
    MediaRead,
    /// Exact page/profile/channel construction and ownership were attested.
    Constructed,
    /// Same exact page was bound to the requested lease, without recreation.
    Acquired,
    /// Admission is sealed; report exact lease debt and unchanged resource
    /// ownership. A missing page is not successful lease-only retirement.
    Revoked {
        /// Exact lease-owned debt from the native adapter.
        debt: WorkBrowserLeaseNativeDebt,
        /// Original page/profile/resource ownership still exists unchanged.
        resource_retained: bool,
    },
    /// Exact page, retained profile lease, resource channels and callbacks
    /// retired. This never means erasing the selected profile's stored data.
    Destroyed,
    /// No exact successful native result could be proven.
    Refused,
    /// Exact drained page is presented in the requested human-owned region.
    HumanPresented,
    /// Native human input is retired and the current document is frozen.
    HumanContinued,
}

/// Non-cloneable native terminal; ordinary audits cannot mint this receipt.
#[must_use]
#[derive(Debug)]
pub struct WorkBrowserResourceCompletion {
    operation: OperationJoin,
    outcome: WorkBrowserResourceNativeOutcome,
    delivery: Option<delivery::DeliveryBinding>,
    effective_document: Option<Arc<ContextNavigationTarget>>,
}

/// Exact ended native lease, not physical terminal-callback return, run success,
/// resource destruction or permission to start a successor. Core `Retained` is
/// bookkeeping only; delivery proof and fresh product/policy/account admission
/// remain independently required.
#[must_use]
#[derive(Debug)]
pub struct WorkBrowserLeaseEnded {
    lease: WorkBrowserExecutionLease,
    delivery: Option<delivery::DeliveryBinding>,
}
impl WorkBrowserLeaseEnded {
    /// Ended lease and its still-owned resource.
    pub const fn lease(&self) -> &WorkBrowserExecutionLease {
        &self.lease
    }
}

/// Exact bounded state transition delivered to the trusted application.
#[must_use]
#[derive(Debug)]
pub enum WorkBrowserResourceEvent {
    /// Resource construction is acknowledged without actor authority.
    Retained(WorkBrowserResourceJoin),
    /// Native presentation acknowledged; no execution lease exists.
    HumanPresented(WorkBrowserResourceJoin),
    /// Human input retired and document rebound; fresh actor admission is separate.
    HumanContinued(WorkBrowserResourceJoin),
    /// An exact native lease binding was acknowledged.
    Acquired(WorkBrowserExecutionLease),
    /// Exact binding exists but was not activated because expiry or shutdown
    /// won. The original native lease remains owned for explicit revocation.
    RevocationRequired(WorkBrowserExecutionLease),
    /// Only the named native lease drained; its resource remains retained.
    LeaseEnded(WorkBrowserLeaseEnded),
    /// Native uncertainty blocks only this exact resource.
    Quarantined(WorkBrowserResourceFailure),
    /// Exact resource destruction acknowledged.
    Destroyed(WorkBrowserResourceJoin),
    /// An old exact callback settled while destruction owned the resource.
    /// This cannot restore the old lease or alter the destruction result.
    DebtSettled(WorkBrowserResourceJoin),
    /// Exact synchronous non-admission, not a native completion or success.
    AdmissionRefused {
        /// Resource whose unmodified request was returned.
        resource: WorkBrowserResourceJoin,
        /// Closed request class that transferred no new native obligation.
        operation: WorkBrowserResourceOperation,
        /// Native adapter's exact admission failure.
        failure: ContextPortFailure,
    },
}

struct Resource {
    join: WorkBrowserResourceJoin,
    storage: ContextProfileStorageClass,
    isolated_public: bool,
    anonymous_read_authority: Option<WorkBrowserAnonymousReadAuthority>,
    anonymous_read_generation: Option<u64>,
    phase: WorkBrowserResourcePhase,
    pending: Option<OperationJoin>,
    destruction: Option<OperationJoin>,
    destruction_attempted: bool,
    lease: Option<WorkBrowserExecutionLease>,
    failure: Option<WorkBrowserResourceFailure>,
    last_tick: AgentPolicyInstant,
    document: Option<Arc<ContextNavigationTarget>>,
    document_policy: crate::WorkBrowserDocumentPolicy,
    effective_document: Option<Arc<ContextNavigationTarget>>,
    current_requested_document: Option<Arc<ContextNavigationTarget>>,
    admission_document: Option<Arc<ContextNavigationTarget>>,
    admission_epoch: crate::NavigationEpoch,
    navigation_epoch: crate::NavigationEpoch,
    frame_generation: crate::FrameGeneration,
    document_available: bool,
    observed: bool,
    navigation: Option<navigation::NavigationJoin>,
    action: Option<action::ActionJoin>,
    observation_sequence: u16,
    observation: Option<observation::ObservationJoin>,
    human: Option<Box<human::HumanWindow>>,
}
impl Drop for Resource {
    fn drop(&mut self) {
        if let Some(proof) = &self.anonymous_read_authority {
            proof.revoke(WorkBrowserAnonymousReadStatus::Closed);
        }
    }
}
impl Resource {
    fn quarantine(&mut self, failure: WorkBrowserResourceFailure) {
        if let Some(proof) = &self.anonymous_read_authority {
            proof.revoke(WorkBrowserAnonymousReadStatus::Closed);
        }
        self.failure.get_or_insert(failure);
        self.phase = WorkBrowserResourcePhase::Quarantined;
        // Keep both callback and execution-capacity debt. Quarantine is not
        // native drain, destruction or permission to recycle capacity.
    }
    fn tick(&mut self, now: AgentPolicyInstant) -> Result<(), WorkBrowserResourceError> {
        if now < self.last_tick {
            self.quarantine(WorkBrowserResourceFailure::ClockRegression);
            return Err(WorkBrowserResourceError::Quarantined);
        }
        self.last_tick = now;
        Ok(())
    }
}

/// Explicit opt-in, profile-bound Work resource authority. Pure state only:
/// no native object, worker, timer, provider, page data or implicit renewal.
/// The native adapter must additionally enforce the shared process ceiling.
pub struct WorkBrowserResources {
    work: WorkId,
    profile: ProfileId,
    authority: Authority,
    rows: BTreeMap<WorkBrowserResourceId, Resource>,
    sequence: u64,
    sealed: bool,
    native_shutdown_started: bool,
}

/// Crate-private, move-only admission from the actual permanently sealed row
/// owner. It is not native zero, and cannot be reconstructed from durable IDs.
pub(crate) struct WorkBrowserNativeShutdownAdmission {
    _authority: Authority,
}
impl WorkBrowserResources {
    /// The trusted application supplies its actual durable Work/profile owner.
    /// This constructor is not persistence, account or native-profile proof.
    pub fn new(work: WorkId, profile: ProfileId) -> Self {
        Self {
            work,
            profile,
            authority: Authority(Arc::new(())),
            rows: BTreeMap::new(),
            sequence: 0,
            sealed: false,
            native_shutdown_started: false,
        }
    }
    fn next(&mut self) -> Result<u64, WorkBrowserResourceError> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or(WorkBrowserResourceError::Exhausted)?;
        Ok(self.sequence)
    }
    fn row_mut(
        &mut self,
        join: &WorkBrowserResourceJoin,
    ) -> Result<&mut Resource, WorkBrowserResourceError> {
        self.rows
            .get_mut(&join.identity.resource)
            .filter(|row| row.join == *join)
            .ok_or(WorkBrowserResourceError::Stale)
    }
    /// Reserve one resource and its exact construction callback before dispatch.
    pub fn construct(
        &mut self,
        resource: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        self.construct_source(
            resource,
            context,
            storage,
            None,
            crate::WorkBrowserDocumentPolicy::Exact,
            now,
            false,
            None,
        )
    }
    /// Reserve one exact initial document at the trusted application edge.
    /// This is task-authored source admission, not model navigation authority.
    /// Redirects and successor navigation are unsupported in this first slice.
    pub fn construct_document(
        &mut self,
        resource: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        document: ContextNavigationTarget,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        self.construct_document_with_policy(
            resource,
            context,
            storage,
            document,
            crate::WorkBrowserDocumentPolicy::Exact,
            now,
        )
    }
    /// Trusted opt-in initial finalization. This grants no successor navigation.
    #[allow(clippy::too_many_arguments)]
    pub fn construct_document_with_policy(
        &mut self,
        resource: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        document: ContextNavigationTarget,
        policy: crate::WorkBrowserDocumentPolicy,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        self.construct_document_with_isolation(
            resource, context, storage, document, policy, false, now,
        )
    }
    /// Trusted public-discovery admission; the isolation operand is frozen in
    /// every lifecycle request and cannot change when a worker is replaced.
    #[allow(clippy::too_many_arguments)]
    pub fn construct_document_with_isolation(
        &mut self,
        resource: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        document: ContextNavigationTarget,
        policy: crate::WorkBrowserDocumentPolicy,
        isolated_public: bool,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        crate::SemanticOrigin::parse(document.as_url().as_str())
            .map_err(|_| WorkBrowserResourceError::Source)?;
        if !policy.admits_request(&document) {
            return Err(WorkBrowserResourceError::Source);
        }
        self.construct_source(
            resource,
            context,
            storage,
            Some(Arc::new(document)),
            policy,
            now,
            isolated_public,
            None,
        )
    }
    /// Freezes an original anonymous read proof exactly once, before native
    /// construction. This does not attest signed-out storage or pixel authority.
    #[allow(clippy::too_many_arguments)]
    pub fn construct_document_with_original_read(
        &mut self,
        resource: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        document: ContextNavigationTarget,
        policy: crate::WorkBrowserDocumentPolicy,
        isolated_public: bool,
        authority: WorkBrowserAnonymousReadAuthority,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        crate::SemanticOrigin::parse(document.as_url().as_str())
            .map_err(|_| WorkBrowserResourceError::Source)?;
        if !isolated_public || !policy.admits_request(&document) {
            return Err(WorkBrowserResourceError::Source);
        }
        self.construct_source(
            resource,
            context,
            storage,
            Some(Arc::new(document)),
            policy,
            now,
            isolated_public,
            Some(authority),
        )
    }
    /// Closed current status of the original resource proof. This is not
    /// account state, a screenshot grant, or permission to replace the proof.
    pub fn anonymous_read_status(
        &self,
        resource: &WorkBrowserResourceJoin,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserAnonymousReadStatus, WorkBrowserResourceError> {
        let row = self
            .rows
            .get(&resource.identity.resource)
            .filter(|row| row.join == *resource)
            .ok_or(WorkBrowserResourceError::Stale)?;
        Ok(row
            .anonymous_read_authority
            .as_ref()
            .map_or(WorkBrowserAnonymousReadStatus::Absent, |proof| {
                proof.status(now)
            }))
    }
    #[allow(clippy::too_many_arguments)]
    fn construct_source(
        &mut self,
        resource: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        document: Option<Arc<ContextNavigationTarget>>,
        document_policy: crate::WorkBrowserDocumentPolicy,
        now: AgentPolicyInstant,
        isolated_public: bool,
        anonymous_read_authority: Option<WorkBrowserAnonymousReadAuthority>,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        if self.sealed {
            return Err(WorkBrowserResourceError::Sealed);
        }
        if self.rows.contains_key(&resource)
            || self
                .rows
                .values()
                .any(|row| row.join.identity.context == context)
        {
            return Err(WorkBrowserResourceError::Duplicate);
        }
        if self.rows.len() >= MAX_LIVE_CONTEXTS {
            return Err(WorkBrowserResourceError::Capacity);
        }
        let sequence = self.next()?;
        if let Some(proof) = &anonymous_read_authority {
            if !document
                .as_ref()
                .is_some_and(|document| proof.claim_resource(context, self.profile, document, now))
            {
                return Err(WorkBrowserResourceError::Source);
            }
        }
        let join = WorkBrowserResourceJoin {
            identity: WorkBrowserResourceIdentity {
                work: self.work,
                resource,
                profile: self.profile,
                context,
            },
            authority: self.authority.clone(),
            incarnation: sequence,
        };
        let operation = OperationJoin {
            resource: join.clone(),
            sequence,
            kind: WorkBrowserResourceOperation::Construct,
            lease: None,
        };
        self.rows.insert(
            resource,
            Resource {
                join,
                storage,
                isolated_public,
                anonymous_read_authority,
                anonymous_read_generation: None,
                phase: WorkBrowserResourcePhase::Constructing,
                pending: Some(operation.clone()),
                destruction: None,
                destruction_attempted: false,
                lease: None,
                failure: None,
                last_tick: now,
                document: document.clone(),
                document_policy,
                effective_document: None,
                current_requested_document: None,
                admission_document: document.clone(),
                admission_epoch: crate::NavigationEpoch::INITIAL,
                navigation_epoch: crate::NavigationEpoch::INITIAL,
                frame_generation: crate::FrameGeneration::INITIAL,
                document_available: false,
                observed: false,
                navigation: None,
                action: None,
                observation_sequence: 0,
                observation: None,
                human: None,
            },
        );
        Ok(WorkBrowserResourceRequest {
            construction: None,
            media: None,
            operation: Box::new(operation),
            storage,
            isolated_public,
            document,
            document_policy,
            delivery: None,
            health: None,
            human: None,
        })
    }
    /// Reserve one run-bound native lease without transferring page ownership.
    /// The only offered capability here is lifecycle binding, not page access.
    pub fn acquire(
        &mut self,
        resource: &WorkBrowserResourceJoin,
        run: ContextRunId,
        now: AgentPolicyInstant,
        deadline: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        if self.sealed {
            return Err(WorkBrowserResourceError::Sealed);
        }
        if deadline <= now {
            return Err(WorkBrowserResourceError::Expired);
        }
        if self.rows.values().filter(|row| row.lease.is_some()).count() >= MAX_EXECUTING_CONTEXTS {
            return Err(WorkBrowserResourceError::Capacity);
        }
        let sequence = self.next()?;
        let row = self.row_mut(resource)?;
        if row.failure.is_some() {
            return Err(WorkBrowserResourceError::Quarantined);
        }
        if row.phase != WorkBrowserResourcePhase::Retained
            || row.pending.is_some()
            || row.lease.is_some()
            || row.observation.is_some()
            || row.navigation.is_some()
            || row.action.is_some()
        {
            return Err(WorkBrowserResourceError::Phase);
        }
        row.tick(now)?;
        let lease = WorkBrowserExecutionLease {
            resource: resource.clone(),
            run,
            generation: sequence,
            deadline,
        };
        let operation = OperationJoin {
            resource: resource.clone(),
            sequence,
            kind: WorkBrowserResourceOperation::Acquire,
            lease: Some(lease.clone()),
        };
        if let Some(proof) = &row.anonymous_read_authority {
            row.anonymous_read_generation = proof.acquire(run, now).then_some(sequence);
        }
        row.lease = Some(lease);
        row.observed = false;
        row.phase = WorkBrowserResourcePhase::Acquiring;
        row.pending = Some(operation.clone());
        Ok(WorkBrowserResourceRequest {
            construction: None,
            media: None,
            operation: Box::new(operation),
            storage: row.storage,
            isolated_public: row.isolated_public,
            document: row.document.clone(),
            document_policy: row.document_policy,
            delivery: None,
            health: None,
            human: None,
        })
    }
    /// Check exact current native-lease membership and immutable deadline.
    /// This is necessary but never sufficient for any page operation.
    pub fn admits_lease(
        &mut self,
        lease: &WorkBrowserExecutionLease,
        now: AgentPolicyInstant,
    ) -> Result<(), WorkBrowserResourceError> {
        if self.sealed {
            return Err(WorkBrowserResourceError::Sealed);
        }
        let row = self.row_mut(&lease.resource)?;
        if row.lease.as_ref() != Some(lease) {
            return Err(WorkBrowserResourceError::Stale);
        }
        row.tick(now)?;
        if row.failure.is_some() {
            return Err(WorkBrowserResourceError::Quarantined);
        }
        if now >= lease.deadline {
            return Err(WorkBrowserResourceError::Expired);
        }
        if row.phase != WorkBrowserResourcePhase::Leased || row.pending.is_some() {
            return Err(WorkBrowserResourceError::Phase);
        }
        Ok(())
    }
    /// Seal this exact lease before dispatching its native revoke/drain request.
    /// Expiry does not erase this cleanup owner or grant a replacement lease.
    pub fn revoke(
        &mut self,
        lease: &WorkBrowserExecutionLease,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        let row = self.row_mut(&lease.resource)?;
        if row.lease.as_ref() != Some(lease) {
            return Err(WorkBrowserResourceError::Stale);
        }
        if !matches!(
            row.phase,
            WorkBrowserResourcePhase::Leased | WorkBrowserResourcePhase::RevocationRequired
        ) || row.pending.is_some()
        {
            return Err(WorkBrowserResourceError::Phase);
        }
        let operation = OperationJoin {
            resource: lease.resource.clone(),
            sequence: lease.generation,
            kind: WorkBrowserResourceOperation::Revoke,
            lease: Some(lease.clone()),
        };
        row.phase = WorkBrowserResourcePhase::Revoking;
        row.pending = Some(operation.clone());
        Ok(WorkBrowserResourceRequest {
            construction: None,
            media: None,
            operation: Box::new(operation),
            storage: row.storage,
            isolated_public: row.isolated_public,
            document: row.document.clone(),
            document_policy: row.document_policy,
            delivery: None,
            health: None,
            human: None,
        })
    }
    /// Revokes the exact lease and additionally tracks physical delivery of its
    /// terminal callback. The native adapter must implement the separate return
    /// barrier; receiving `LeaseEnded` or polling Pending never establishes it.
    /// No new operation, callback, worker or native capacity is allocated.
    pub fn revoke_with_delivery(
        &mut self,
        lease: &WorkBrowserExecutionLease,
    ) -> Result<
        (WorkBrowserResourceRequest, WorkBrowserLeaseDeliveryTicket),
        WorkBrowserResourceError,
    > {
        let mut request = self.revoke(lease)?;
        let (delivery, ticket) = delivery::track(lease.clone());
        request.delivery = Some(Box::new(delivery));
        Ok((request, ticket))
    }
    /// Quarantine one resource without discarding callback or capacity debt.
    /// A late exact terminal can settle debt but cannot unquarantine the page.
    pub fn quarantine(
        &mut self,
        resource: &WorkBrowserResourceJoin,
    ) -> Result<(), WorkBrowserResourceError> {
        let row = self.row_mut(resource)?;
        if row.phase == WorkBrowserResourcePhase::Destroyed {
            return Err(WorkBrowserResourceError::Phase);
        }
        row.quarantine(WorkBrowserResourceFailure::CallbackUncertain);
        Ok(())
    }
    /// Explicit resource destruction is separate from lease completion. One
    /// reserved cleanup slot remains usable even with an uncertain callback or
    /// exhausted acquisition identities. It never erases that callback debt.
    pub fn destroy(
        &mut self,
        resource: &WorkBrowserResourceJoin,
    ) -> Result<WorkBrowserResourceRequest, WorkBrowserResourceError> {
        let row = self.row_mut(resource)?;
        if row.destruction_attempted {
            return Err(WorkBrowserResourceError::Phase);
        }
        if !matches!(
            row.phase,
            WorkBrowserResourcePhase::Retained
                | WorkBrowserResourcePhase::Quarantined
                | WorkBrowserResourcePhase::PresentingHuman
                | WorkBrowserResourcePhase::PresentedHuman
                | WorkBrowserResourcePhase::ContinuingAfterHuman
        ) {
            return Err(WorkBrowserResourceError::Phase);
        }
        let operation = OperationJoin {
            resource: resource.clone(),
            sequence: resource.incarnation,
            kind: WorkBrowserResourceOperation::Destroy,
            lease: row.lease.clone(),
        };
        if let Some(proof) = &row.anonymous_read_authority {
            proof.revoke(WorkBrowserAnonymousReadStatus::Closed);
        }
        row.phase = WorkBrowserResourcePhase::Destroying;
        row.destruction_attempted = true;
        row.destruction = Some(operation.clone());
        Ok(WorkBrowserResourceRequest {
            construction: None,
            media: None,
            operation: Box::new(operation),
            storage: row.storage,
            isolated_public: row.isolated_public,
            document: row.document.clone(),
            document_policy: row.document_policy,
            delivery: None,
            health: None,
            human: None,
        })
    }
    /// Settle the exact owned terminal. Wrong authority/phase cannot be replaced
    /// by a zero audit, guessed emptiness or a different resource's receipt.
    pub fn settle_at(
        &mut self,
        completion: WorkBrowserResourceCompletion,
        now: AgentPolicyInstant,
    ) -> Result<WorkBrowserResourceEvent, WorkBrowserResourceError> {
        let sealed = self.sealed;
        let row = self.row_mut(&completion.operation.resource)?;
        if row.destruction.as_ref() != Some(&completion.operation)
            && row.pending.as_ref() != Some(&completion.operation)
        {
            return Err(WorkBrowserResourceError::Stale);
        }
        // A clock failure revokes execution but cannot discard the exact native
        // terminal or block physical cleanup. Native absence is not a clean
        // time/accounting claim; its original failure remains retained.
        let phase_before_tick = row.phase;
        let _ = row.tick(now);
        if matches!(
            phase_before_tick,
            WorkBrowserResourcePhase::Destroying | WorkBrowserResourcePhase::Destroyed
        ) {
            row.phase = phase_before_tick;
        }
        if row.destruction.as_ref() == Some(&completion.operation) {
            row.destruction = None;
            if completion.outcome == WorkBrowserResourceNativeOutcome::Destroyed {
                row.phase = WorkBrowserResourcePhase::Destroyed;
                row.lease = None;
                return Ok(WorkBrowserResourceEvent::Destroyed(row.join.clone()));
            }
            row.quarantine(
                if completion.outcome == WorkBrowserResourceNativeOutcome::Refused {
                    WorkBrowserResourceFailure::NativeRefused
                } else {
                    WorkBrowserResourceFailure::Contract
                },
            );
            return Ok(WorkBrowserResourceEvent::Quarantined(
                row.failure.unwrap_or(WorkBrowserResourceFailure::Contract),
            ));
        }
        if row.pending.as_ref() != Some(&completion.operation) {
            return Err(WorkBrowserResourceError::Stale);
        }
        row.pending = None;
        if matches!(
            row.phase,
            WorkBrowserResourcePhase::Destroying | WorkBrowserResourcePhase::Destroyed
        ) {
            return Ok(WorkBrowserResourceEvent::DebtSettled(row.join.clone()));
        }
        if let Some(failure) = row.failure {
            row.phase = WorkBrowserResourcePhase::Quarantined;
            return Ok(WorkBrowserResourceEvent::Quarantined(failure));
        }
        if matches!(
            completion.operation.kind,
            WorkBrowserResourceOperation::PresentHuman
                | WorkBrowserResourceOperation::ContinueAfterHuman
        ) {
            return Ok(human::settle(row, completion, now, sealed));
        }
        let failure = match (completion.operation.kind, completion.outcome) {
            (
                WorkBrowserResourceOperation::ReadMedia,
                WorkBrowserResourceNativeOutcome::MediaRead,
            ) => {
                row.phase = WorkBrowserResourcePhase::Retained;
                return Ok(WorkBrowserResourceEvent::Retained(row.join.clone()));
            }
            (
                WorkBrowserResourceOperation::Construct,
                WorkBrowserResourceNativeOutcome::Constructed,
            ) => {
                let effective = completion.effective_document.or_else(|| {
                    (row.document_policy == crate::WorkBrowserDocumentPolicy::Exact)
                        .then(|| row.document.clone())
                        .flatten()
                });
                let valid = match (&row.document, &effective) {
                    (Some(requested), Some(effective)) => row
                        .document_policy
                        .admits_final_document(requested, effective),
                    (None, None) => row.document_policy == crate::WorkBrowserDocumentPolicy::Exact,
                    _ => false,
                };
                if !valid {
                    row.quarantine(WorkBrowserResourceFailure::Contract);
                    return Ok(WorkBrowserResourceEvent::Quarantined(
                        WorkBrowserResourceFailure::Contract,
                    ));
                }
                row.effective_document = effective;
                row.document_available = row.effective_document.is_some();
                row.phase = WorkBrowserResourcePhase::Retained;
                return Ok(WorkBrowserResourceEvent::Retained(row.join.clone()));
            }
            (WorkBrowserResourceOperation::Acquire, WorkBrowserResourceNativeOutcome::Acquired) => {
                if let Some(lease) = row.lease.as_ref() {
                    if sealed || now >= lease.deadline {
                        row.phase = WorkBrowserResourcePhase::RevocationRequired;
                        return Ok(WorkBrowserResourceEvent::RevocationRequired(lease.clone()));
                    }
                    row.phase = WorkBrowserResourcePhase::Leased;
                    return Ok(WorkBrowserResourceEvent::Acquired(lease.clone()));
                }
                WorkBrowserResourceFailure::Contract
            }
            (
                WorkBrowserResourceOperation::Revoke,
                WorkBrowserResourceNativeOutcome::Revoked {
                    debt,
                    resource_retained,
                },
            ) => {
                if debt.bounded()
                    && debt.is_empty()
                    && resource_retained
                    && row.observation.is_none()
                    && row.navigation.is_none()
                    && row.action.is_none()
                {
                    if let Some(lease) = row.lease.take() {
                        row.phase = WorkBrowserResourcePhase::Retained;
                        return Ok(WorkBrowserResourceEvent::LeaseEnded(
                            WorkBrowserLeaseEnded {
                                lease,
                                delivery: completion.delivery,
                            },
                        ));
                    }
                }
                WorkBrowserResourceFailure::DrainUnproven
            }
            (_, WorkBrowserResourceNativeOutcome::Refused) => {
                WorkBrowserResourceFailure::NativeRefused
            }
            _ => WorkBrowserResourceFailure::Contract,
        };
        row.quarantine(failure);
        Ok(WorkBrowserResourceEvent::Quarantined(failure))
    }
    /// Account a returned unmodified request after synchronous non-admission.
    /// No callback is synthesized. A rejected revoke/destroy cannot erase the
    /// already-existing native resource or its preceding lease obligations.
    pub fn dispatch_refused(
        &mut self,
        request: WorkBrowserResourceRequest,
        failure: ContextPortFailure,
    ) -> Result<WorkBrowserResourceEvent, WorkBrowserResourceError> {
        let row = self.row_mut(&request.operation.resource)?;
        let operation = request.operation.kind;
        if row.destruction.as_ref() == Some(request.operation.as_ref()) {
            row.destruction = None;
            row.quarantine(WorkBrowserResourceFailure::NativeRefused);
        } else {
            if row.pending.as_ref() != Some(request.operation.as_ref()) {
                return Err(WorkBrowserResourceError::Stale);
            }
            row.pending = None;
            if operation == WorkBrowserResourceOperation::Acquire {
                // The returned request proves that this lease never acquired
                // native ownership, even if a clock/control race quarantined
                // the resource while dispatch was being decided.
                row.lease = None;
            }
            if matches!(
                row.phase,
                WorkBrowserResourcePhase::Destroying | WorkBrowserResourcePhase::Destroyed
            ) {
                // Cleanup already owns/retired the resource; settle only debt.
            } else if operation == WorkBrowserResourceOperation::Construct {
                if let Some(proof) = &row.anonymous_read_authority {
                    proof.revoke(WorkBrowserAnonymousReadStatus::Closed);
                }
                // Exact non-admission proves absence without pretending the
                // prior clock/control ambiguity or task failure disappeared.
                row.phase = WorkBrowserResourcePhase::Destroyed;
            } else if row.failure.is_some() {
                row.phase = WorkBrowserResourcePhase::Quarantined;
            } else {
                match operation {
                    WorkBrowserResourceOperation::Construct => {
                        row.phase = WorkBrowserResourcePhase::Destroyed
                    }
                    WorkBrowserResourceOperation::Acquire
                    | WorkBrowserResourceOperation::ReadMedia => {
                        row.phase = WorkBrowserResourcePhase::Retained;
                    }
                    WorkBrowserResourceOperation::Revoke
                    | WorkBrowserResourceOperation::PresentHuman
                    | WorkBrowserResourceOperation::ContinueAfterHuman
                    | WorkBrowserResourceOperation::Destroy => {
                        row.quarantine(WorkBrowserResourceFailure::NativeRefused)
                    }
                }
            }
        }
        Ok(WorkBrowserResourceEvent::AdmissionRefused {
            resource: row.join.clone(),
            operation,
            failure,
        })
    }
    /// Read-only exact phase. It does not disclose page or profile contents.
    pub fn phase(
        &self,
        resource: &WorkBrowserResourceJoin,
    ) -> Result<WorkBrowserResourcePhase, WorkBrowserResourceError> {
        self.rows
            .get(&resource.identity.resource)
            .filter(|row| row.join == *resource)
            .map(|row| row.phase)
            .ok_or(WorkBrowserResourceError::Stale)
    }
    /// Permanently forbid new construction/acquisition. Existing native leases
    /// still require explicit revocation/drain and every resource destruction.
    pub fn seal(&mut self) {
        for row in self.rows.values() {
            if let Some(proof) = &row.anonymous_read_authority {
                proof.revoke(WorkBrowserAnonymousReadStatus::Closed);
            }
        }
        self.sealed = true;
    }
    /// True only after sealing and exact destruction of every retained resource.
    /// This is local registry closure, never the engine's global shutdown proof.
    pub fn is_quiescent(&self) -> bool {
        self.sealed
            && self.rows.values().all(|row| {
                row.phase == WorkBrowserResourcePhase::Destroyed
                    && row.pending.is_none()
                    && row.destruction.is_none()
                    && row.lease.is_none()
                    && row.observation.is_none()
                    && row.navigation.is_none()
                    && row.action.is_none()
            })
    }
    /// Admits the existing global native seal/audit protocol once, only after
    /// this original registry is permanently sealed and every resource and
    /// logical callback is drained. The application must additionally prove
    /// physical callback/health-owner retirement and use its original port.
    /// No native-zero proof exists until that protocol accepts an exact audit.
    pub fn begin_native_shutdown(
        &mut self,
    ) -> Result<crate::AgentNativeShutdownCoordinator, WorkBrowserResourceError> {
        if self.native_shutdown_started {
            return Err(WorkBrowserResourceError::Sealed);
        }
        if !self.is_quiescent() {
            return Err(WorkBrowserResourceError::Phase);
        }
        self.native_shutdown_started = true;
        Ok(
            crate::AgentNativeShutdownCoordinator::from_retained_registry(
                WorkBrowserNativeShutdownAdmission {
                    _authority: self.authority.clone(),
                },
            ),
        )
    }
    /// Release runtime bookkeeping only after exact resource destruction and
    /// every retained callback. The returned durable identity is not deleted.
    pub fn reap(
        &mut self,
        resource: &WorkBrowserResourceJoin,
    ) -> Result<WorkBrowserResourceIdentity, WorkBrowserResourceError> {
        let row = self.row_mut(resource)?;
        if row.pending.is_some()
            || row.destruction.is_some()
            || row.observation.is_some()
            || row.navigation.is_some()
            || row.action.is_some()
        {
            return Err(WorkBrowserResourceError::Pending);
        }
        if row.phase != WorkBrowserResourcePhase::Destroyed || row.lease.is_some() {
            return Err(WorkBrowserResourceError::Phase);
        }
        self.rows
            .remove(&resource.identity.resource)
            .map(|row| row.join.identity)
            .ok_or(WorkBrowserResourceError::Stale)
    }
}

#[path = "work_browser_human.rs"]
mod human;
pub use human::{
    brand_family, same_work_human_site, WorkBrowserHumanCheck, WorkBrowserHumanCheckState,
    WorkBrowserHumanProgress, WorkBrowserHumanRegion, WorkBrowserHumanTime,
    MAX_WORK_HUMAN_WAIT_MILLIS,
};

#[path = "work_browser_observation.rs"]
mod observation;
pub use observation::{
    WorkBrowserObservationCapability, WorkBrowserObservationCompletion,
    WorkBrowserObservationCompletionCallback, WorkBrowserObservationDispatch,
    WorkBrowserObservationEvent, WorkBrowserObservationRequest, WorkBrowserReadBinding,
};

#[path = "work_browser_navigation.rs"]
mod navigation;
pub use navigation::{
    WorkBrowserHistoryBackCompletionCallback, WorkBrowserHistoryBackDispatch,
    WorkBrowserHistoryBackRequest, WorkBrowserNavigationCompletion,
    WorkBrowserNavigationCompletionCallback, WorkBrowserNavigationDispatch,
    WorkBrowserNavigationEvent, WorkBrowserNavigationPreparation, WorkBrowserNavigationRequest,
};

#[path = "work_browser_action.rs"]
mod action;
pub use action::{
    WorkBrowserActionCompletion, WorkBrowserActionCompletionCallback,
    WorkBrowserActionCompletionOwner, WorkBrowserActionDeliveryCompletion,
    WorkBrowserActionDeliveryTicket, WorkBrowserActionDispatch, WorkBrowserActionEvent,
    WorkBrowserActionRefusal, WorkBrowserActionRequest,
};

#[path = "work_browser_delivery.rs"]
mod delivery;
pub use delivery::{
    WorkBrowserLeaseDeliveryCompletion, WorkBrowserLeaseDeliveryNotification,
    WorkBrowserLeaseDeliveryPollError, WorkBrowserLeaseDeliveryProof,
    WorkBrowserLeaseDeliveryReceipt, WorkBrowserLeaseDeliveryRefusal,
    WorkBrowserLeaseDeliveryTicket,
};

#[path = "work_browser_health.rs"]
mod health;
pub use health::{
    WorkBrowserResourceHealth, WorkBrowserResourceHealthReporter, WorkBrowserResourceHealthState,
};

/// Move-only terminal callback for an accepted resource operation. A synchronous
/// refusal transfers no callback obligation and must not pretend to be success.
pub type WorkBrowserResourceCompletionCallback =
    Box<dyn FnOnce(WorkBrowserResourceCompletion) + Send + 'static>;

/// Lossless admission result. Rejected requests retain their exact owner so
/// the caller can account a synchronous refusal without a fabricated callback.
#[must_use]
#[derive(Debug)]
pub enum WorkBrowserResourceDispatch {
    /// The adapter owns exactly one completion callback for the consumed request.
    Scheduled,
    /// No callback/effect obligation transferred; the exact request is returned.
    Rejected {
        /// Original unmodified request, never a reconstructed replacement.
        request: Box<WorkBrowserResourceRequest>,
        /// Closed native admission failure.
        failure: ContextPortFailure,
    },
}

#[cfg(test)]
#[path = "work_browser_resource_tests.rs"]
mod tests;

#[path = "work_browser_media.rs"]
mod media;
pub use media::{
    WorkBrowserCropGeometry, WorkBrowserMediaFetchMode, WorkBrowserMediaProgress,
    WorkBrowserPublicAssetReply, WorkBrowserPublicAssetRequest, WorkBrowserRecordImage,
    WorkBrowserRecordImageOrigin, MAX_PUBLIC_RECORD_IMAGE_BYTES,
};

#[path = "work_browser_anonymous.rs"]
mod anonymous;
pub use anonymous::{
    WorkBrowserAnonymousReadAuthority, WorkBrowserAnonymousReadLoss, WorkBrowserAnonymousReadStatus,
};
