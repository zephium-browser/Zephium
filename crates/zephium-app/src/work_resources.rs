//! Application-owned retained-page substrate, not product/run admission.
//!
//! The original owner has no actor-runtime, journal or Store admission authority.
//! Its opt-in child supplies only the common controller's narrow lease facade;
//! Its private application child joins scoped/durable actor admission; selected
//! profile construction and Shell attachment live in the bounded product child.
#![allow(dead_code)] // Other private resource primitives await explicit product joins.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, TryLockError, Weak};
use std::task::Wake;
use zephium_agentic::*;
use zephium_core::ids::ProfileId;

#[cfg(feature = "work-execution")]
#[path = "work_resources_controller.rs"]
mod controller;

#[cfg(feature = "work-execution")]
#[path = "work_resources_navigation.rs"]
mod navigation;
#[cfg(feature = "work-execution")]
use navigation::PendingNavigation;

#[cfg(feature = "work-execution")]
#[path = "work_resources_action.rs"]
mod action;
#[cfg(feature = "work-execution")]
use action::PendingAction;

#[cfg(feature = "work-execution")]
#[path = "work_resources_application.rs"]
mod application;

#[cfg(feature = "work-execution")]
#[path = "work_resources_product.rs"]
pub(super) mod product;

#[cfg(feature = "work-execution")]
#[path = "work_resources_shutdown.rs"]
mod shutdown;

#[cfg(feature = "work-execution")]
#[path = "work_resources_wait.rs"]
mod wait;

#[cfg(feature = "work-execution-probe")]
#[path = "work_resources_probe.rs"]
pub mod probe;
#[cfg(feature = "work-execution-probe")]
#[path = "work_resources_snapshot_probe.rs"]
mod snapshot_probe;

type NativeSink = Arc<dyn Fn(ContextNativeEvent) + Send + Sync>;
type NativeFactory = Box<dyn FnOnce(NativeSink) -> Option<Arc<dyn AgentBrowserPort>>>;
type WakeApplication = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Refusal {
    Uncertain,
    Busy,
    Consumed,
    Core(WorkBrowserResourceError),
    NativeAdmission(ContextPortFailure),
}
impl From<WorkBrowserResourceError> for Refusal {
    fn from(error: WorkBrowserResourceError) -> Self {
        Self::Core(error)
    }
}

struct Notifications {
    pending: AtomicBool,
    failed: AtomicBool,
    wake: WakeApplication,
    #[cfg(feature = "work-execution")]
    actors: Mutex<Vec<std::sync::Weak<controller::LeaseSignal>>>,
    #[cfg(feature = "work-execution")]
    epoch: wait::NotificationEpoch,
}
impl Notifications {
    fn publish(&self) -> bool {
        // State/terminal publication always precedes this coalesced wake.
        if !self.pending.swap(true, Ordering::AcqRel)
            && !matches!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.wake)())),
                Ok(true)
            )
        {
            self.failed.store(true, Ordering::Release);
        }
        #[cfg(feature = "work-execution")]
        self.publish_actor_wakes();
        #[cfg(feature = "work-execution")]
        if !self.epoch.publish() {
            self.failed.store(true, Ordering::Release);
        }
        !self.failed.load(Ordering::Acquire)
    }
}
impl Wake for Notifications {
    fn wake(self: Arc<Self>) {
        // The core reporter contains this panic and marks its exact resource
        // uncertain too. No native admission can ignore a failed stable wake.
        assert!(self.publish(), "Work resource notification failed");
    }
}

struct Resource {
    join: WorkBrowserResourceJoin,
    health: Mutex<WorkBrowserResourceHealth>,
    failed: AtomicBool,
    flights: AtomicUsize,
    reads: AtomicUsize,
    navigations: AtomicUsize,
    actions: AtomicUsize,
    orphaned_actions: AtomicUsize,
    reusable: AtomicBool,
    slots: Mutex<Vec<OwnedSlot>>,
    facade: Mutex<Option<WorkBrowserExecutionLease>>,
}
impl Resource {
    fn current(&self) -> bool {
        if self.health.is_poisoned()
            || self.facade.is_poisoned()
            || self.slots.is_poisoned()
            || self
                .lock_local(&self.slots)
                .is_ok_and(|slots| slots.iter().any(OwnedSlot::is_poisoned))
        {
            self.fail();
        }
        !self.failed.load(Ordering::Acquire)
            && self
                .lock_local(&self.health)
                .is_ok_and(|health| health.snapshot() == WorkBrowserResourceHealthState::Current)
    }
    fn fail(&self) {
        self.failed.store(true, Ordering::Release);
    }
    fn refusal(&self) -> Refusal {
        self.fail();
        Refusal::Uncertain
    }
    fn lock_local<'a, T>(&self, mutex: &'a Mutex<T>) -> Result<MutexGuard<'a, T>, Refusal> {
        mutex.lock().map_err(|_| self.refusal())
    }
    fn retain(&self, slot: OwnedSlot) -> Result<(), Refusal> {
        let mut slots = self.lock_local(&self.slots)?;
        let mut retired = Vec::new();
        let mut index = 0;
        while index < slots.len() {
            if slots[index].finished(self) {
                retired.push(slots.swap_remove(index));
            } else {
                index += 1;
            }
        }
        // One lifecycle, one document operation (read/navigation/action), and one
        // overtaking destruction maximum; the core enforces the exclusion.
        let result = if slots.len() >= 3 {
            self.fail();
            Err(Refusal::Busy)
        } else {
            slots.push(slot);
            Ok(())
        };
        drop(slots);
        // Delivery listeners may own arbitrary Waker destructors. Never reap
        // their last slot while holding the application collection mutex.
        drop(retired);
        result
    }
}
struct Shared {
    // Original application/native owner; never extracted or rebound to actors.
    port: Arc<dyn AgentBrowserPort>,
    rows: Mutex<WorkBrowserResources>,
    resources: Mutex<BTreeMap<WorkBrowserResourceId, Arc<Resource>>>,
    notifications: Arc<Notifications>,
}
impl Shared {
    fn refusal(&self) -> Refusal {
        self.notifications.failed.store(true, Ordering::Release);
        Refusal::Uncertain
    }
    fn global_current(&self) -> bool {
        // Existing facades do not traverse the map. They must still observe a
        // poisoned original owner before admitting or accepting another read.
        if self.rows.is_poisoned() || self.resources.is_poisoned() {
            self.refusal();
        }
        #[cfg(feature = "work-execution")]
        if self.notifications.actors.is_poisoned() {
            self.refusal();
        }
        !self.notifications.failed.load(Ordering::Acquire)
    }
    fn lock_resources(
        &self,
    ) -> Result<MutexGuard<'_, BTreeMap<WorkBrowserResourceId, Arc<Resource>>>, Refusal> {
        self.resources.lock().map_err(|_| self.refusal())
    }
    fn lock_rows(&self) -> Result<std::sync::MutexGuard<'_, WorkBrowserResources>, Refusal> {
        self.rows.lock().map_err(|_| self.refusal())
    }
    fn current(&self, resource: &Resource) -> Result<(), Refusal> {
        if !self.global_current() || !resource.current() {
            Err(Refusal::Uncertain)
        } else {
            Ok(())
        }
    }
    fn resource(&self, join: &WorkBrowserResourceJoin) -> Result<Arc<Resource>, Refusal> {
        let resources = self.lock_resources()?;
        resources
            .get(&join.identity().resource())
            .filter(|row| &row.join == join)
            .cloned()
            .ok_or(Refusal::Core(WorkBrowserResourceError::Stale))
    }
}

/// Survives every actor and retains the original registry, native port and sink.
/// Private until scoped runtime + durable admission are independently joined.
struct WorkResourceOwner {
    shared: Arc<Shared>,
    native_events: mpsc::Receiver<ContextNativeEvent>,
}
impl WorkResourceOwner {
    fn new(
        work: WorkId,
        profile: ProfileId,
        wake: WakeApplication,
        factory: NativeFactory,
    ) -> Option<Self> {
        let notifications = Arc::new(Notifications {
            pending: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            wake,
            #[cfg(feature = "work-execution")]
            actors: Mutex::new(Vec::with_capacity(MAX_LIVE_CONTEXTS)),
            #[cfg(feature = "work-execution")]
            epoch: wait::NotificationEpoch::default(),
        });
        // Resource-local events use their exact sticky observer, not this lane.
        // Only original global audit events belong here. Overflow/unexpected
        // legacy events poison this owner; they never select an actor mailbox.
        let (tx, native_events) = mpsc::sync_channel(2);
        let sink_notifications = notifications.clone();
        let sink: NativeSink = Arc::new(move |event| {
            if !matches!(
                event,
                ContextNativeEvent::ResourceAuditSettled(_)
                    | ContextNativeEvent::ShutdownAuditSettled(_)
            ) || tx.try_send(event).is_err()
            {
                sink_notifications.failed.store(true, Ordering::Release);
            }
            sink_notifications.publish();
        });
        let port = factory(sink)?;
        Some(Self {
            shared: Arc::new(Shared {
                port,
                rows: Mutex::new(WorkBrowserResources::new(work, profile)),
                resources: Mutex::new(BTreeMap::new()),
                notifications,
            }),
            native_events,
        })
    }

    // Caller drains until None. Empty is the re-arm point; clear then recheck
    // ensures a concurrent publication is observed or arranges the next wake.
    fn poll_native_event(&self) -> Result<Option<ContextNativeEvent>, Refusal> {
        self.shared.global_current(); // Cleanup events remain account-able after failure.
        match self.native_events.try_recv() {
            Ok(event) => return Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.shared
                    .notifications
                    .failed
                    .store(true, Ordering::Release);
                return Err(Refusal::Uncertain);
            }
        }
        self.shared
            .notifications
            .pending
            .swap(false, Ordering::AcqRel);
        // Drain/re-arm each stable observer too. Otherwise its initial pending
        // health wake could suppress an idle renderer failure after A exits.
        let resources: Vec<_> = self.shared.lock_resources()?.values().cloned().collect();
        let mut failure = None;
        for resource in resources {
            let state = match resource.lock_local(&resource.health) {
                Ok(mut health) => health.poll(),
                Err(error) => {
                    // A local fault cannot leave later healthy observers'
                    // pending signals armed and suppress their next idle wake.
                    failure = Some(error);
                    continue;
                }
            };
            if state == WorkBrowserResourceHealthState::Uncertain {
                resource.fail();
            }
        }
        match self.native_events.try_recv() {
            // Preserve a raced global terminal. The resource fault is sticky
            // and is reported on the next empty poll, never by losing this event.
            Ok(event) => Ok(Some(event)),
            Err(mpsc::TryRecvError::Empty) => failure.map_or(Ok(None), Err),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.shared
                    .notifications
                    .failed
                    .store(true, Ordering::Release);
                Err(Refusal::Uncertain)
            }
        }
    }

    fn construct(
        &self,
        id: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        target: ContextNavigationTarget,
        now: AgentPolicyInstant,
    ) -> Result<PendingLifecycle, Refusal> {
        self.construct_with_policy(
            id,
            context,
            storage,
            target,
            zephium_agentic::WorkBrowserDocumentPolicy::Exact,
            now,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn construct_with_policy(
        &self,
        id: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        target: ContextNavigationTarget,
        policy: zephium_agentic::WorkBrowserDocumentPolicy,
        now: AgentPolicyInstant,
    ) -> Result<PendingLifecycle, Refusal> {
        self.construct_isolated(
            id, context, storage, target, policy, false, false, None, None, None, None, None, now,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn construct_isolated(
        &self,
        id: WorkBrowserResourceId,
        context: ContextId,
        storage: ContextProfileStorageClass,
        target: ContextNavigationTarget,
        policy: zephium_agentic::WorkBrowserDocumentPolicy,
        isolated_public: bool,
        work_website_data: bool,
        anonymous_read_authority: Option<zephium_agentic::WorkBrowserAnonymousReadAuthority>,
        anonymous_session: Option<zephium_agentic::WorkBrowserSession>,
        redirect: Option<zephium_agentic::WorkBrowserRedirect>,
        unreachable: Option<zephium_agentic::WorkBrowserUnreachable>,
        construction_window: Option<(
            zephium_agentic::WorkBrowserConstructionAttempt,
            std::time::Instant,
        )>,
        now: AgentPolicyInstant,
    ) -> Result<PendingLifecycle, Refusal> {
        if !self.shared.global_current() {
            return Err(Refusal::Uncertain);
        }
        let request = if let Some(proof) = anonymous_read_authority {
            self.shared
                .lock_rows()?
                .construct_document_with_original_read(
                    id,
                    context,
                    storage,
                    target,
                    policy,
                    isolated_public,
                    proof,
                    now,
                )?
        } else {
            self.shared.lock_rows()?.construct_document_with_isolation(
                id,
                context,
                storage,
                target,
                policy,
                isolated_public,
                now,
            )?
        };
        let request = if work_website_data {
            request
                .with_work_website_data()
                .map_err(|_| Refusal::Uncertain)?
        } else {
            request
        };
        let request = match anonymous_session {
            Some(session) => request
                .with_anonymous_session(session)
                .map_err(|_| Refusal::Uncertain)?,
            None => request,
        };
        let request = match redirect {
            Some(redirect) => request
                .with_redirect(redirect)
                .map_err(|_| Refusal::Uncertain)?,
            None => request,
        };
        let request = match unreachable {
            Some(unreachable) => request
                .with_unreachable(unreachable)
                .map_err(|_| Refusal::Uncertain)?,
            None => request,
        };
        let request = match construction_window {
            Some((attempt, deadline)) => request
                .with_construction_window(attempt, deadline)
                .map_err(|_| Refusal::Uncertain)?,
            None => request,
        };
        let (request, mut health) = request
            .track_resource_health()
            .map_err(|_| Refusal::Uncertain)?;
        // This one immutable application wake predates native dispatch and is
        // never replaced by actor A/B's executor or mailbox.
        health.register(self.shared.notifications.clone().into());
        let resource = Arc::new(Resource {
            join: request.resource().clone(),
            health: Mutex::new(health),
            failed: AtomicBool::new(false),
            flights: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            navigations: AtomicUsize::new(0),
            actions: AtomicUsize::new(0),
            orphaned_actions: AtomicUsize::new(0),
            reusable: AtomicBool::new(false),
            slots: Mutex::new(Vec::with_capacity(3)),
            facade: Mutex::new(None),
        });
        self.shared.lock_resources()?.insert(id, resource.clone());
        PendingLifecycle::dispatch(self.shared.clone(), resource, request, None, None)
    }

    // Native-primitive acquisition only. Future product admission must also
    // consume scoped worker closure, fresh task/account and original Store ACK.
    fn acquire(
        &self,
        join: &WorkBrowserResourceJoin,
        run: ContextRunId,
        now: AgentPolicyInstant,
        deadline: AgentPolicyInstant,
    ) -> Result<PendingLifecycle, Refusal> {
        let resource = self.shared.resource(join)?;
        self.shared.current(&resource)?;
        if resource.flights.load(Ordering::Acquire) != 0
            || resource.orphaned_actions.load(Ordering::Acquire) != 0
            || !resource.reusable.load(Ordering::Acquire)
        {
            return Err(Refusal::Busy);
        }
        let request = self.shared.lock_rows()?.acquire(join, run, now, deadline)?;
        resource.reusable.store(false, Ordering::Release);
        PendingLifecycle::dispatch(self.shared.clone(), resource, request, None, None)
    }

    fn browser(
        &self,
        lease: WorkBrowserExecutionLease,
        now: AgentPolicyInstant,
    ) -> Result<LeaseBrowser, Refusal> {
        let resource = self.shared.resource(lease.resource())?;
        self.shared.current(&resource)?;
        self.shared.lock_rows()?.admits_lease(&lease, now)?;
        {
            let mut prior = resource.lock_local(&resource.facade)?;
            if prior.as_ref() == Some(&lease) {
                return Err(Refusal::Busy);
            }
            *prior = Some(lease.clone());
        }
        let retired = Arc::new(LeaseRetirement::new(&resource));
        Ok(LeaseBrowser {
            shared: self.shared.clone(),
            resource,
            lease,
            retired,
        })
    }

    fn destroy(&self, join: &WorkBrowserResourceJoin) -> Result<PendingLifecycle, Refusal> {
        let resource = self.shared.resource(join)?;
        if resource.failed.load(Ordering::Acquire)
            || !resource.current()
            || !matches!(
                self.shared.lock_rows()?.phase(join)?,
                WorkBrowserResourcePhase::Retained
                    | WorkBrowserResourcePhase::Destroyed
                    | WorkBrowserResourcePhase::PresentingHuman
                    | WorkBrowserResourcePhase::PresentedHuman
                    | WorkBrowserResourcePhase::ContinuingAfterHuman
            )
        {
            self.shared.lock_rows()?.quarantine(join)?;
        }
        let request = self.shared.lock_rows()?.destroy(join)?;
        PendingLifecycle::dispatch(self.shared.clone(), resource, request, None, None)
    }

    fn media_lifecycle(
        &self,
        join: &WorkBrowserResourceJoin,
        plans: Vec<zephium_agentic::SemanticRecordMediaPlan>,
        fetch_mode: zephium_agentic::WorkBrowserMediaFetchMode,
        now: AgentPolicyInstant,
        deadline: AgentPolicyInstant,
    ) -> Result<(PendingLifecycle, zephium_agentic::WorkBrowserMediaProgress), Refusal> {
        let resource = self.shared.resource(join)?;
        self.shared.current(&resource)?;
        if resource.flights.load(Ordering::Acquire) != 0
            || resource.orphaned_actions.load(Ordering::Acquire) != 0
            || !resource.reusable.load(Ordering::Acquire)
        {
            return Err(Refusal::Busy);
        }
        let (request, progress) = self
            .shared
            .lock_rows()?
            .read_record_media(join, plans, fetch_mode, now, deadline)?;
        resource.reusable.store(false, Ordering::Release);
        PendingLifecycle::dispatch(self.shared.clone(), resource, request, None, None)
            .map(|pending| (pending, progress))
    }

    fn human_lifecycle(
        &self,
        join: &WorkBrowserResourceJoin,
        step: application::HumanStep,
        now: AgentPolicyInstant,
        deadline: AgentPolicyInstant,
    ) -> Result<PendingLifecycle, Refusal> {
        let resource = self.shared.resource(join)?;
        self.shared.current(&resource)?;
        if resource.flights.load(Ordering::Acquire) != 0
            || resource.orphaned_actions.load(Ordering::Acquire) != 0
        {
            return Err(Refusal::Busy);
        }
        let request = match step {
            application::HumanStep::Present(..) | application::HumanStep::HandOver
                if !resource.reusable.load(Ordering::Acquire) =>
            {
                return Err(Refusal::Busy);
            }
            application::HumanStep::Present(region, sign_in) => self
                .shared
                .lock_rows()?
                .present_human_for(join, region, now, deadline, sign_in)?,
            application::HumanStep::HandOver => self
                .shared
                .lock_rows()?
                .hand_over_unpresented(join, now, deadline)?,
            application::HumanStep::Continue => {
                self.shared.lock_rows()?.continue_after_human(join, now)?
            }
        };
        resource.reusable.store(false, Ordering::Release);
        PendingLifecycle::dispatch(self.shared.clone(), resource, request, None, None)
    }

    fn seal_resources(&self) -> Result<(), Refusal> {
        self.shared.lock_rows()?.seal();
        Ok(())
    }

    fn reap_absent(
        &self,
        join: &WorkBrowserResourceJoin,
    ) -> Result<WorkBrowserResourceIdentity, Refusal> {
        let resource = self.shared.resource(join)?;
        if resource.flights.load(Ordering::Acquire) != 0
            || resource.orphaned_actions.load(Ordering::Acquire) != 0
            || !resource.lock_local(&resource.health)?.reporter_retired()
        {
            return Err(Refusal::Busy);
        }
        let identity = self.shared.lock_rows()?.reap(join)?;
        self.shared
            .lock_resources()?
            .remove(&join.identity().resource());
        Ok(identity)
    }

    fn shutdown_audit(
        &self,
        audit: ContextResourceAuditId,
    ) -> Result<ContextShutdownDispatch, Refusal> {
        self.seal_resources()?;
        // Original native cohort only. Returned/audited facts still need the
        // existing global coordinator and application lifecycle join.
        Ok(self.shared.port.seal_for_shutdown(audit))
    }

    fn locally_retired(&self) -> bool {
        // Descriptive local accounting only. No global native proof is minted.
        self.shared.global_current()
            && self
                .shared
                .lock_rows()
                .is_ok_and(|rows| rows.is_quiescent())
            && self.shared.lock_resources().is_ok_and(|resources| {
                resources.values().all(|resource| {
                    resource.flights.load(Ordering::Acquire) == 0
                        && resource.orphaned_actions.load(Ordering::Acquire) == 0
                        && resource.lock_local(&resource.health).is_ok_and(|health| {
                            health.reporter_retired()
                                && matches!(
                                    health.snapshot(),
                                    WorkBrowserResourceHealthState::Retired
                                        | WorkBrowserResourceHealthState::Uncertain
                                )
                        })
                })
            })
    }

    fn poll_health(
        &self,
        join: &WorkBrowserResourceJoin,
    ) -> Result<WorkBrowserResourceHealthState, Refusal> {
        let resource = self.shared.resource(join)?;
        let state = resource.lock_local(&resource.health)?.poll();
        if state == WorkBrowserResourceHealthState::Uncertain {
            resource.fail();
        }
        Ok(state)
    }

    fn drain_abandoned(&self, now: AgentPolicyInstant) -> Result<(), Refusal> {
        self.shared.drain_abandoned(now)
    }
}
impl Shared {
    fn drain_abandoned(&self, now: AgentPolicyInstant) -> Result<(), Refusal> {
        let resources: Vec<_> = self.lock_resources()?.values().cloned().collect();
        let mut failure = None;
        for resource in resources {
            let slots = match resource.lock_local(&resource.slots) {
                Ok(slots) => slots.clone(),
                Err(error) => {
                    failure = Some(error);
                    continue;
                }
            };
            for slot in slots {
                if let Err(error) = slot.drain_abandoned(self, &resource, now) {
                    failure = Some(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
impl Drop for WorkResourceOwner {
    fn drop(&mut self) {
        self.shared
            .notifications
            .failed
            .store(true, Ordering::Release);
        self.shared.notifications.publish();
    }
}

/// Exact per-facade retirement/failure arbitration, independent of later leases.
struct LeaseRetirement {
    state: AtomicU8,
    resource: Weak<Resource>,
}
impl LeaseRetirement {
    const ACTIVE: u8 = 0;
    const FAILED: u8 = 1;
    const RETIRED: u8 = 2;

    fn new(resource: &Arc<Resource>) -> Self {
        Self {
            state: AtomicU8::new(Self::ACTIVE),
            resource: Arc::downgrade(resource),
        }
    }
    fn is_retired(&self) -> bool {
        self.state.load(Ordering::Acquire) == Self::RETIRED
    }
    fn check_active(&self) -> Result<(), Refusal> {
        match self.state.load(Ordering::Acquire) {
            Self::ACTIVE => Ok(()),
            Self::FAILED => {
                self.fail();
                Err(Refusal::Uncertain)
            }
            _ => Err(Refusal::Core(WorkBrowserResourceError::Stale)),
        }
    }
    fn claim_failure(&self) -> Option<LeaseFailure<'_>> {
        // Failed and Retired are mutually exclusive and absorbing. Once a
        // failure claims Active, retirement cannot publish reusable, even if
        // this thread is paused before the exact resource failure is stored.
        match self.state.compare_exchange(
            Self::ACTIVE,
            Self::FAILED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) | Err(Self::FAILED) => Some(LeaseFailure(self)),
            Err(_) => None,
        }
    }
    fn fail(&self) -> bool {
        if let Some(failure) = self.claim_failure() {
            drop(failure);
            true
        } else {
            false
        }
    }
    fn retire(&self) -> Result<(), Refusal> {
        match self.state.compare_exchange(
            Self::ACTIVE,
            Self::RETIRED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(Self::FAILED) => {
                self.fail();
                Err(Refusal::Uncertain)
            }
            Err(_) => Err(Refusal::Consumed),
        }
    }
}

/// Exact failure publication owner, not a callback or native capability. Its
/// Drop cannot be delayed past successful retirement: Failed already won the
/// same atomic transition that retirement must consume. It only publishes the
/// sticky failure. Actor/lifecycle callers retain a strong Resource; signal
/// callers hold no owner lock when their temporary upgrade is dropped.
struct LeaseFailure<'a>(&'a LeaseRetirement);
impl Drop for LeaseFailure<'_> {
    fn drop(&mut self) {
        if let Some(resource) = self.0.resource.upgrade() {
            resource.fail();
        }
    }
}

/// Actor receives precisely bounded initial-read, revoke and health operations.
/// No port/registry extraction, construction, destruction, sealing or effects.
struct LeaseBrowser {
    shared: Arc<Shared>,
    resource: Arc<Resource>,
    lease: WorkBrowserExecutionLease,
    // Exact facade/lease marker; never inferred from the resource's later B phase.
    retired: Arc<LeaseRetirement>,
}
impl LeaseBrowser {
    fn refusal(&self) -> Refusal {
        if self.retired.fail() {
            Refusal::Uncertain
        } else {
            Refusal::Core(WorkBrowserResourceError::Stale)
        }
    }
    fn health(&self, now: AgentPolicyInstant) -> Result<(), Refusal> {
        self.retired.check_active()?;
        self.shared.current(&self.resource)?;
        self.shared.lock_rows()?.admits_lease(&self.lease, now)?;
        Ok(())
    }
    fn observe_initial(&self, now: AgentPolicyInstant) -> Result<PendingRead, Refusal> {
        self.health(now)?;
        let request = self.shared.lock_rows()?.observe_initial(&self.lease, now)?;
        PendingRead::dispatch(self.shared.clone(), self.resource.clone(), request)
    }
    fn revoke(&self) -> Result<PendingLifecycle, Refusal> {
        // Cleanup is permitted after health failure. It never invokes legacy
        // cancellation or resets the document/world/invocation ceiling.
        let (request, delivery) = self.shared.lock_rows()?.revoke_with_delivery(&self.lease)?;
        PendingLifecycle::dispatch(
            self.shared.clone(),
            self.resource.clone(),
            request,
            Some(delivery),
            Some(self.retired.clone()),
        )
    }
}
impl Drop for LeaseBrowser {
    fn drop(&mut self) {
        self.retired.fail();
    }
}

struct Flight<T> {
    receiver: mpsc::Receiver<T>,
    early: Option<T>,
    finished: bool,
    abandoned: bool,
    contradictory: bool,
    read: bool,
    navigation: bool,
    action: bool,
}
impl<T: Send + 'static> Flight<T> {
    fn new(
        shared: &Shared,
        resource: &Arc<Resource>,
        read: bool,
    ) -> (Self, Box<dyn FnOnce(T) + Send>) {
        // The functional registry separately admits at most one lifecycle and
        // one read per resource. This count cannot grow from page/model input.
        resource.flights.fetch_add(1, Ordering::AcqRel);
        if read {
            resource.reads.fetch_add(1, Ordering::AcqRel);
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let destination = Arc::downgrade(resource);
        let notifications = shared.notifications.clone();
        let callback = Box::new(move |terminal| {
            if sender.try_send(terminal).is_err() {
                if let Some(destination) = destination.upgrade() {
                    destination.fail();
                }
            }
            notifications.publish();
        });
        (
            Self {
                receiver,
                early: None,
                finished: false,
                abandoned: false,
                contradictory: false,
                read,
                navigation: false,
                action: false,
            },
            callback,
        )
    }
    fn rejected(&mut self, resource: &Resource) {
        match self.receiver.try_recv() {
            Err(mpsc::TryRecvError::Disconnected) => {}
            result => {
                self.contradictory = true;
                resource.fail();
                self.early = result.ok();
            }
        }
    }
    fn take(&mut self, resource: &Resource) -> Result<Option<T>, Refusal> {
        if self.finished {
            return Err(Refusal::Consumed);
        }
        if self.early.is_some() {
            return Ok(self.early.take());
        }
        match self.receiver.try_recv() {
            Ok(terminal) => Ok(Some(terminal)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                resource.fail();
                Err(Refusal::Uncertain)
            }
        }
    }
    fn finish(&mut self, resource: &Resource) {
        if !self.finished {
            self.finished = true;
            resource.flights.fetch_sub(1, Ordering::AcqRel);
            if self.read {
                resource.reads.fetch_sub(1, Ordering::AcqRel);
            }
            if self.navigation {
                resource.navigations.fetch_sub(1, Ordering::AcqRel);
            }
            if self.action {
                resource.actions.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

enum LifecycleResult {
    Event(WorkBrowserResourceEvent),
    Delivered(WorkBrowserLeaseDeliveryProof),
}
struct PendingLifecycle {
    shared: Arc<Shared>,
    resource: Arc<Resource>,
    slot: Arc<Mutex<LifecycleOperation>>,
}
struct LifecycleOperation {
    flight: Flight<WorkBrowserResourceCompletion>,
    refused: Option<(WorkBrowserResourceRequest, ContextPortFailure)>,
    delivery: Option<WorkBrowserLeaseDeliveryTicket>,
    ended: Option<WorkBrowserLeaseEnded>,
    retirement: Option<Arc<LeaseRetirement>>,
}
impl PendingLifecycle {
    fn dispatch(
        shared: Arc<Shared>,
        resource: Arc<Resource>,
        request: WorkBrowserResourceRequest,
        delivery: Option<WorkBrowserLeaseDeliveryTicket>,
        retirement: Option<Arc<LeaseRetirement>>,
    ) -> Result<Self, Refusal> {
        let (flight, callback) = Flight::new(&shared, &resource, false);
        let slot = Arc::new(Mutex::new(LifecycleOperation {
            flight,
            refused: None,
            delivery,
            ended: None,
            retirement,
        }));
        resource.retain(OwnedSlot::Lifecycle(slot.clone()))?;
        let cleanup = matches!(
            request.operation(),
            WorkBrowserResourceOperation::Revoke | WorkBrowserResourceOperation::Destroy
        );
        let mut result =
            if !cleanup && (!shared.global_current() || resource.failed.load(Ordering::Acquire)) {
                drop(callback);
                Ok(WorkBrowserResourceDispatch::Rejected {
                    request: Box::new(request),
                    failure: ContextPortFailure::Shutdown,
                })
            } else {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    shared.port.work_resource_lifecycle(request, callback)
                }))
            };
        // A synchronously returned construction was never installed. Retire its
        // callback-bearing reporter NOW, outside every application mutex. No
        // refused request stored/polled/dropped in a slot may carry that wake.
        if let Ok(WorkBrowserResourceDispatch::Rejected { request, .. }) = &mut result {
            drop(request.take_resource_health_reporter());
        }
        let mut state = resource.lock_local(&slot)?;
        state.refused = match result {
            Ok(WorkBrowserResourceDispatch::Scheduled) => None,
            Ok(WorkBrowserResourceDispatch::Rejected { request, failure }) => {
                state.flight.rejected(&resource);
                Some((*request, failure))
            }
            Err(_) => {
                resource.fail();
                None
            }
        };
        drop(state);
        Ok(Self {
            shared,
            resource,
            slot,
        })
    }
    fn poll(&mut self, now: AgentPolicyInstant) -> Result<Option<LifecycleResult>, Refusal> {
        self.resource
            .lock_local(&self.slot)?
            .poll(&self.shared, &self.resource, now)
    }
}
impl Drop for PendingLifecycle {
    fn drop(&mut self) {
        match self.slot.lock() {
            Ok(mut slot) if !slot.flight.finished => {
                slot.flight.abandoned = true;
                self.resource.fail();
            }
            Err(_) => self.resource.fail(),
            _ => {}
        }
    }
}
impl LifecycleOperation {
    fn poll(
        &mut self,
        shared: &Shared,
        resource: &Resource,
        now: AgentPolicyInstant,
    ) -> Result<Option<LifecycleResult>, Refusal> {
        if self.flight.finished {
            return Err(Refusal::Consumed);
        }
        // Native read callbacks can be physically returned but still queued in
        // their original application slot. Settle those exact reads before the
        // core's zero-read revocation terminal; never infer drain from order.
        if self.delivery.is_some()
            && (resource.reads.load(Ordering::Acquire) != 0
                || resource.navigations.load(Ordering::Acquire) != 0
                || resource.actions.load(Ordering::Acquire) != 0)
        {
            return Ok(None);
        }
        if self.ended.is_none() {
            let event = if let Some((request, failure)) = self.refused.take() {
                let event = shared.lock_rows()?.dispatch_refused(request, failure);
                if self.flight.contradictory {
                    let _ = event;
                    return self.drain_contradiction(shared, resource, now);
                }
                event?
            } else {
                if self.flight.contradictory {
                    return self.drain_contradiction(shared, resource, now);
                }
                let Some(terminal) = self.flight.take(resource)? else {
                    return Ok(None);
                };
                match shared.lock_rows()?.settle_at(terminal, now) {
                    Ok(event) => event,
                    Err(error) => {
                        resource.fail();
                        self.flight.finish(resource);
                        return Err(error.into());
                    }
                }
            };
            if let WorkBrowserResourceEvent::LeaseEnded(ended) = event {
                self.ended = Some(ended);
            } else {
                if matches!(
                    event,
                    WorkBrowserResourceEvent::Retained(_)
                        | WorkBrowserResourceEvent::AdmissionRefused {
                            operation: WorkBrowserResourceOperation::Acquire,
                            ..
                        }
                ) {
                    resource.reusable.store(true, Ordering::Release);
                }
                self.flight.finish(resource);
                return Ok(Some(LifecycleResult::Event(event)));
            }
        }
        let ticket = self.delivery.as_mut().ok_or(Refusal::Uncertain)?;
        let Some(receipt) = ticket.try_take().map_err(|_| resource.refusal())? else {
            return Ok(None);
        };
        let ended = self.ended.take().ok_or(Refusal::Uncertain)?;
        // Native terminal and one delivery receipt have both been accounted.
        // A failed health/proof join is uncertainty, not fictitious callback debt.
        self.flight.finish(resource);
        let proof = ended.join_delivery(receipt).map_err(|_| {
            resource.fail();
            Refusal::Uncertain
        })?;
        shared.current(resource)?;
        if let Some(retirement) = &self.retirement {
            retirement.retire()?;
        }
        resource.reusable.store(true, Ordering::Release);
        Ok(Some(LifecycleResult::Delivered(proof)))
    }
    fn drain_contradiction(
        &mut self,
        shared: &Shared,
        resource: &Resource,
        now: AgentPolicyInstant,
    ) -> Result<Option<LifecycleResult>, Refusal> {
        match self.flight.take(resource) {
            Ok(Some(terminal)) => {
                let _ = shared.lock_rows()?.settle_at(terminal, now);
                self.flight.finish(resource);
                Err(Refusal::Uncertain)
            }
            Ok(None) => Ok(None),
            Err(_) => {
                self.flight.finish(resource);
                Err(Refusal::Uncertain)
            }
        }
    }
}

struct PendingRead {
    shared: Arc<Shared>,
    resource: Arc<Resource>,
    slot: Arc<Mutex<ReadOperation>>,
}
struct ReadOperation {
    flight: Flight<WorkBrowserObservationCompletion>,
    refused: Option<(WorkBrowserObservationRequest, ContextPortFailure)>,
}
impl PendingRead {
    fn dispatch(
        shared: Arc<Shared>,
        resource: Arc<Resource>,
        request: WorkBrowserObservationRequest,
    ) -> Result<Self, Refusal> {
        let (flight, callback) = Flight::new(&shared, &resource, true);
        let slot = Arc::new(Mutex::new(ReadOperation {
            flight,
            refused: None,
        }));
        resource.retain(OwnedSlot::Read(slot.clone()))?;
        let result = if !shared.global_current() || resource.failed.load(Ordering::Acquire) {
            drop(callback);
            Ok(WorkBrowserObservationDispatch::Rejected {
                request: Box::new(request),
                failure: ContextPortFailure::Shutdown,
            })
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                shared.port.work_resource_observe(request, callback)
            }))
        };
        let mut state = resource.lock_local(&slot)?;
        state.refused = match result {
            Ok(WorkBrowserObservationDispatch::Scheduled) => None,
            Ok(WorkBrowserObservationDispatch::Rejected { request, failure }) => {
                state.flight.rejected(&resource);
                Some((*request, failure))
            }
            Err(_) => {
                resource.fail();
                None
            }
        };
        drop(state);
        Ok(Self {
            shared,
            resource,
            slot,
        })
    }
    fn poll(
        &mut self,
        now: AgentPolicyInstant,
    ) -> Result<Option<WorkBrowserObservationEvent>, Refusal> {
        self.resource
            .lock_local(&self.slot)?
            .poll(&self.shared, &self.resource, now)
    }
}
impl Drop for PendingRead {
    fn drop(&mut self) {
        match self.slot.lock() {
            Ok(mut slot) if !slot.flight.finished => {
                slot.flight.abandoned = true;
                self.resource.fail();
            }
            Err(_) => self.resource.fail(),
            _ => {}
        }
    }
}
impl ReadOperation {
    fn poll(
        &mut self,
        shared: &Shared,
        resource: &Resource,
        now: AgentPolicyInstant,
    ) -> Result<Option<WorkBrowserObservationEvent>, Refusal> {
        if let Some((request, failure)) = self.refused.take() {
            let result = shared.lock_rows()?.observation_dispatch_refused(request);
            if !self.flight.contradictory {
                self.flight.finish(resource);
                result?;
                return Err(Refusal::NativeAdmission(failure));
            }
        }
        let terminal = match self.flight.take(resource) {
            Ok(Some(terminal)) => terminal,
            Ok(None) => return Ok(None),
            Err(error) if self.flight.contradictory => {
                self.flight.finish(resource);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        // Check sticky native/global failure before accepting page contents.
        // Exact terminal still settles even after admission has been sealed.
        if shared.current(resource).is_err()
            && shared.lock_rows()?.phase(&resource.join)? != WorkBrowserResourcePhase::Destroyed
        {
            shared.lock_rows()?.quarantine(&resource.join)?;
        }
        let result = shared.lock_rows()?.settle_observation(terminal, now);
        self.flight.finish(resource);
        if self.flight.contradictory {
            return Err(Refusal::Uncertain);
        }
        Ok(Some(result?))
    }
}

#[derive(Clone)]
enum OwnedSlot {
    Lifecycle(Arc<Mutex<LifecycleOperation>>),
    Read(Arc<Mutex<ReadOperation>>),
    #[cfg(feature = "work-execution")]
    Navigation(Arc<Mutex<navigation::NavigationOperation>>),
    #[cfg(feature = "work-execution")]
    Action(Arc<Mutex<action::ActionOperation>>),
}
impl OwnedSlot {
    fn is_poisoned(&self) -> bool {
        match self {
            Self::Lifecycle(slot) => slot.is_poisoned(),
            Self::Read(slot) => slot.is_poisoned(),
            #[cfg(feature = "work-execution")]
            Self::Navigation(slot) => slot.is_poisoned(),
            #[cfg(feature = "work-execution")]
            Self::Action(slot) => slot.is_poisoned(),
        }
    }
    fn finished(&self, resource: &Resource) -> bool {
        // Called while the slots collection is held. Never wait for an operation
        // whose poll may recheck resource health (and the slots collection).
        match self {
            Self::Lifecycle(slot) => match slot.try_lock() {
                Ok(slot) => slot.flight.finished,
                Err(TryLockError::Poisoned(_)) => {
                    resource.fail();
                    false
                }
                Err(TryLockError::WouldBlock) => false,
            },
            Self::Read(slot) => match slot.try_lock() {
                Ok(slot) => slot.flight.finished,
                Err(TryLockError::Poisoned(_)) => {
                    resource.fail();
                    false
                }
                Err(TryLockError::WouldBlock) => false,
            },
            #[cfg(feature = "work-execution")]
            Self::Navigation(slot) => match slot.try_lock() {
                Ok(slot) => slot.flight.finished,
                Err(TryLockError::Poisoned(_)) => {
                    resource.fail();
                    false
                }
                Err(TryLockError::WouldBlock) => false,
            },
            #[cfg(feature = "work-execution")]
            Self::Action(slot) => match slot.try_lock() {
                Ok(slot) => slot.finished(),
                Err(TryLockError::Poisoned(_)) => {
                    resource.fail();
                    false
                }
                Err(TryLockError::WouldBlock) => false,
            },
        }
    }
    fn drain_abandoned(
        &self,
        shared: &Shared,
        resource: &Resource,
        now: AgentPolicyInstant,
    ) -> Result<(), Refusal> {
        match self {
            Self::Lifecycle(slot) => {
                let mut slot = resource.lock_local(slot)?;
                if slot.flight.abandoned && !slot.flight.finished {
                    let _ = slot.poll(shared, resource, now);
                }
            }
            Self::Read(slot) => {
                let mut slot = resource.lock_local(slot)?;
                if slot.flight.abandoned && !slot.flight.finished {
                    let _ = slot.poll(shared, resource, now);
                }
            }
            #[cfg(feature = "work-execution")]
            Self::Navigation(slot) => {
                let mut slot = resource.lock_local(slot)?;
                if slot.flight.abandoned && !slot.flight.finished {
                    // This only drains resource ownership after abandonment.
                    // The lost controller policy receipt remains recovery debt.
                    let _ = slot.poll(shared, resource, now);
                }
            }
            #[cfg(feature = "work-execution")]
            Self::Action(slot) => {
                let mut slot = resource.lock_local(slot)?;
                slot.drain_abandoned(shared, resource, now);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "work_resources_tests.rs"]
mod tests;
