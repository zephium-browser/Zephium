//! Persistent Work native page ownership. No run is stored in the page owner;
//! only admitted invocation/task envelopes carry temporary execution leases.

#[cfg(target_os = "macos")]
use super::agent_context::AgentPendingScreenshot;
#[cfg(target_os = "macos")]
use super::profiles::{
    profile_scoped_value, profile_value_is_isolated, MAX_PROFILE_PERSISTENCE_BINDINGS,
};
use super::{
    profiles::bind_profile_persistence_class,
    resources::{NativeResourceClass, NativeResourceLease},
    EngineHost,
};
use crate::agent_context_port::{
    work_browser_monotonic_now, WorkActionTask, WorkLifecycleTask, WorkObservationTask,
    WorkResourceGuard,
};
#[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
use crate::WorkResourceFailureCause as ResourceFailureCause;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use zephium_agentic::ContextOwnedViewport;
use zephium_agentic::{
    ContextId, ContextPortFailure, ContextProfileStorageClass, SemanticRuntimeCorrelation,
    SemanticRuntimePortFailure, SemanticSnapshot, WorkBrowserLeaseNativeDebt,
    WorkBrowserResourceNativeOutcome as Outcome, WorkBrowserResourceOperation as Operation,
    MAX_EXECUTING_CONTEXTS, MAX_LIVE_CONTEXTS,
};
use zephium_core::{ids::ProfileId, ports::engine::Partition};

const DRAIN_BUDGET: Duration = Duration::from_secs(5);
// Web pages may perform a bounded same-document URL finalization shortly after
// their exact native navigation finishes. Only an explicitly trusted document
// policy enters this native quiet-period fence. Exact documents pay no delay.
const DOCUMENT_FINALIZATION_QUIET_PERIOD: Duration = Duration::from_millis(500);
/// A page an action's admitted load committed but never reported loaded is
/// taken as loaded after this long.
const HAND_ON_COMMITTED_SETTLE: Duration = Duration::from_secs(4);
/// A site's first document that committed but keeps loading (a mail app with
/// a loading screen and open channels) is taken as loaded after this long.
const INITIAL_COMMITTED_SETTLE: Duration = Duration::from_secs(6);

struct DocumentFinalizationWake {
    timer: Option<crate::platform::imp::ContentPolicyTimeout>,
    ticket: crate::platform::work_document_navigation::WorkDocumentFinalizationTicket,
}

enum DocumentFinalizationProgress {
    Pending,
    Ready(zephium_agentic::ContextNavigationTarget),
    WakeUnavailable,
    Refused,
}

#[cfg(feature = "native-agentic-work-resource-probe")]
#[path = "work_resource_witness.rs"]
mod witness;

#[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
#[path = "work_resource_form_oracle.rs"]
mod form_oracle;

#[path = "work_resource_action.rs"]
mod action;
#[path = "work_frame_schedule.rs"]
mod frame_schedule;
#[path = "work_resource_human.rs"]
mod human;
#[path = "work_resource_media.rs"]
mod media;
#[path = "work_resource_navigation.rs"]
mod navigation;
#[path = "work_resource_observation.rs"]
mod observation;

#[path = "work_visual_store.rs"]
pub(crate) mod visual_store;

#[cfg(target_os = "macos")]
pub(super) struct AnonymousWorkStore {
    session: zephium_agentic::WeakWorkBrowserSession,
    visual: Option<visual_store::FreshNativeStore>,
    pub(super) profile: zephium_core::ids::ProfileId,
    pub(super) store: crate::platform::imp::WebsiteDataStore,
}

#[cfg(target_os = "macos")]
impl EngineHost {
    pub(super) fn anonymous_work_store(
        &mut self,
        session: &zephium_agentic::WorkBrowserSession,
    ) -> Result<crate::platform::imp::WebsiteDataStore, ContextPortFailure> {
        self.anonymous_work_stores.retain(|_, entry| {
            entry.session.is_current() || self.erasure_tombstones.contains(&entry.profile)
        });
        if !session.is_current() {
            return Err(ContextPortFailure::Stale);
        }
        if let Some(entry) = self.anonymous_work_stores.get(&session.id()) {
            return Ok(entry.store.clone());
        }
        if self.anonymous_work_stores.len() >= MAX_LIVE_CONTEXTS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let store = crate::platform::imp::new_ephemeral_data_store()
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        if self.anonymous_work_stores.values().any(|entry| {
            objc2::rc::Retained::as_ptr(&entry.store) == objc2::rc::Retained::as_ptr(&store)
        }) || self
            .macos_ephemeral_data_stores
            .values()
            .any(|other| objc2::rc::Retained::as_ptr(other) == objc2::rc::Retained::as_ptr(&store))
        {
            return Err(ContextPortFailure::NativeRefused);
        }
        let id = session.id();
        let profile = session.profile();
        if !session.register_retirement(Box::new(move || {
            dispatch2::DispatchQueue::main().exec_async(move || {
                super::best_effort_with(move |host| {
                    if !host.erasure_tombstones.contains(&profile) {
                        host.anonymous_work_stores.remove(&id);
                    }
                });
            });
        })) {
            return Err(ContextPortFailure::Stale);
        }
        let visual = visual_store::FreshNativeStore::after_native_allocation(session);
        self.anonymous_work_stores.insert(
            id,
            AnonymousWorkStore {
                session: session.downgrade(),
                visual,
                profile: session.profile(),
                store: store.clone(),
            },
        );
        Ok(store)
    }

    /// Work's persistent store for a durable profile. Pages of one run and of
    /// parallel runs share the one retained object, as tabs share a profile's.
    pub(super) fn work_data_store(
        &mut self,
        profile: ProfileId,
    ) -> Result<crate::platform::imp::WebsiteDataStore, ContextPortFailure> {
        if self.erasure_tombstones.contains(&profile)
            || self.profile_persistence_classes.get(&profile)
                != Some(&super::profiles::ProfilePersistenceClass::Durable)
        {
            return Err(ContextPortFailure::ProfileUnavailable);
        }
        if let Some(store) = self.work_data_stores.get(&profile) {
            return Ok(store.clone());
        }
        if self.work_data_stores.len() >= MAX_PROFILE_PERSISTENCE_BINDINGS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let store = crate::platform::imp::work_data_store(profile)
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        self.work_data_stores.insert(profile, store.clone());
        Ok(store)
    }

    /// Empties the profile's Work store; its pages keep running on it.
    pub(crate) fn clear_work_website_data(
        &mut self,
        profile: ProfileId,
        reply: std::sync::mpsc::Sender<bool>,
    ) {
        if self.erasure_tombstones.contains(&profile) {
            let _ = reply.send(false);
            return;
        }
        crate::platform::imp::clear_work_data_store(profile, reply);
    }
}

pub(super) struct WorkNativeResource {
    media_pending: bool,
    media_batch: Option<std::rc::Rc<media::Batch>>,
    media_wake: Option<observation::ObservationWake>,
    pub(super) guard: Arc<WorkResourceGuard>,
    construction: Option<WorkLifecycleTask>,
    construction_presentation: Option<crate::platform::imp::WorkObservationPresentation>,
    human_presentation: Option<crate::platform::imp::WorkHumanPresentation>,
    human_wake: Option<crate::platform::imp::ContentPolicyTimeout>,
    human_progress: Option<zephium_agentic::WorkBrowserHumanProgress>,
    /// Handed over without a presentation, for a decision made elsewhere.
    human_unpresented: bool,
    /// A continued sign-in waiting for its site's cookies to reach the
    /// person's profile; the page goes idle only after that copy, so the
    /// copy never runs under the page's own suspension.
    #[cfg(target_os = "windows")]
    pub(super) human_return: Option<(WorkLifecycleTask, zephium_agentic::ContextNavigationTarget)>,
    revocation: Option<WorkLifecycleTask>,
    destruction: Option<WorkLifecycleTask>,
    watchdog: Option<crate::platform::imp::ContentPolicyTimeout>,
    observation: Option<observation::WorkObservation>,
    action: Option<action::WorkAction>,
    reading_presentation: Option<crate::platform::imp::WorkObservationPresentation>,
    presentation_wake: Option<observation::ObservationWake>,
    frame_generation: u64,
    frame_in_flight: Arc<std::sync::atomic::AtomicBool>,
    frame_rendering_pending: Arc<std::sync::atomic::AtomicBool>,
    frame_schedule: frame_schedule::WorkFrameSchedule,
    frame_initial_requested: bool,
    frame_wake: Option<crate::platform::imp::ContentPolicyTimeout>,
    navigation: Option<navigation::WorkNavigation>,
    history_back: Option<navigation::WorkHistoryBack>,
    #[cfg(target_os = "macos")]
    pub(super) screenshot: Option<AgentPendingScreenshot>,
    document_finalization_wake: Option<DocumentFinalizationWake>,
    document_finalization_ready:
        Option<crate::platform::work_document_navigation::WorkDocumentFinalizationTicket>,
    hand_on_wake: Option<crate::platform::imp::ContentPolicyTimeout>,
    #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
    form_oracle: form_oracle::State,
    #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
    pub(super) form_actions: crate::platform::form_qualification::ActionAudit,
    pub(super) last_invocation: u64,
    /// The document a look last completed on: a document's first look waits
    /// longer for a heavy app still starting its own scripts.
    pub(super) looked_document:
        Option<crate::platform::work_document_navigation::WorkDocumentStamp>,
    document_started: bool,
    retirement_clean: bool,
    deadline_expired: bool,
    // Pair wall-clock expiry with its exact lifecycle class. Destruction can
    // overtake construction, so diagnostics must not infer this from whichever
    // task fields happen to remain populated when the deadline wins.
    lifecycle_deadline: Option<(Instant, Operation)>,
    content_policy: Option<crate::platform::imp::ContentPolicyRegistration>,
    pub(super) view: Option<crate::platform::imp::AgentOwnedView>,
    native_resource: Option<NativeResourceLease>,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    witness: Option<witness::RenderingHolder>,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    witness_attempted: bool,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    witness_admission: Option<witness::Admission>,
    #[cfg(feature = "native-agentic-work-resource-probe")]
    presentation_observations: u16,
}
impl WorkNativeResource {
    #[cfg(feature = "native-agentic-work-resource-probe")]
    fn record_construction_failure(&self, cause: &'static str) {
        self.guard.record_construction_evidence(|| {
            // Reserved before sampling: duplicate failure causes never enter
            // this closure or read a second native URL.
            let current =
                self.view
                    .as_ref()
                    .zip(self.guard.document())
                    .and_then(|(view, expected)| {
                        view.work_navigation().map(|gate| {
                            crate::platform::imp::current_document_evidence(
                                view.view(),
                                gate,
                                expected,
                            )
                        })
                    });
            crate::agent_context_port::resource_witness::ConstructionEvidence {
                cause,
                port_failure: None,
                navigation: self
                    .view
                    .as_ref()
                    .and_then(|view| view.work_navigation())
                    .and_then(|gate| gate.construction_evidence()),
                document_started: self.document_started,
                deadline_expired: self.deadline_expired,
                guard_healthy: self.guard.is_healthy(),
                current_document: current
                    .as_ref()
                    .map_or_else(|| self.ready(), |(ready, _)| *ready),
                current_components: current.map(|(_, evidence)| evidence),
                semantic_pending: self
                    .view
                    .as_ref()
                    .and_then(|view| view.semantic_pending_for_audit()),
            }
        });
    }
    fn unconstructed(guard: Arc<WorkResourceGuard>, native_resource: NativeResourceLease) -> Self {
        Self {
            guard,
            construction: None,
            construction_presentation: None,
            human_presentation: None,
            human_wake: None,
            human_progress: None,
            human_unpresented: false,
            #[cfg(target_os = "windows")]
            human_return: None,
            revocation: None,
            destruction: None,
            watchdog: None,
            observation: None,
            action: None,
            reading_presentation: None,
            presentation_wake: None,
            media_pending: false,
            media_batch: None,
            media_wake: None,
            frame_generation: 0,
            frame_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            frame_rendering_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            frame_schedule: frame_schedule::WorkFrameSchedule::default(),
            frame_initial_requested: false,
            frame_wake: None,
            navigation: None,
            history_back: None,
            #[cfg(target_os = "macos")]
            screenshot: None,
            document_finalization_wake: None,
            document_finalization_ready: None,
            hand_on_wake: None,
            #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
            form_oracle: form_oracle::State::default(),
            #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
            form_actions: crate::platform::form_qualification::ActionAudit::default(),
            last_invocation: 0,
            looked_document: None,
            document_started: false,
            retirement_clean: true,
            deadline_expired: false,
            lifecycle_deadline: None,
            content_policy: None,
            view: None,
            native_resource: Some(native_resource),
            #[cfg(feature = "native-agentic-work-resource-probe")]
            witness: None,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            witness_attempted: false,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            witness_admission: None,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            presentation_observations: 0,
        }
    }
    pub(super) fn guard(&self) -> Arc<WorkResourceGuard> {
        self.guard.clone()
    }
    pub(super) fn profile(&self) -> ProfileId {
        self.guard.resource().identity().profile()
    }
    pub(super) fn view(&self) -> Option<&wry::WebView> {
        self.view.as_ref().map(|view| view.view())
    }
    pub(super) fn replace_content_policy_registration(
        &mut self,
        registration: crate::platform::imp::ContentPolicyRegistration,
    ) -> Option<crate::platform::imp::ContentPolicyRegistration> {
        self.content_policy.replace(registration)
    }
    fn history_back_pending(&self) -> bool {
        self.history_back.is_some()
    }
    pub(super) fn pending(&self) -> bool {
        self.media_pending
            || self.construction.is_some()
            || self.revocation.is_some()
            || self.destruction.is_some()
            || self.observation.is_some()
            || self.action.is_some()
            || self.navigation.is_some()
            || self.history_back_pending()
            || {
                #[cfg(target_os = "macos")]
                {
                    self.screenshot.is_some()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    self.human_return.is_some()
                }
            }
    }
    fn schedule_document_finalization_wake(
        &mut self,
        ticket: crate::platform::work_document_navigation::WorkDocumentFinalizationTicket,
    ) -> bool {
        if self.document_finalization_wake.is_some() || self.document_finalization_ready.is_some() {
            return false;
        }
        let guard = self.guard.clone();
        let rejected = guard.clone();
        let wake_ticket = ticket.clone();
        let timer = crate::platform::imp::schedule_content_policy_timeout(
            DOCUMENT_FINALIZATION_QUIET_PERIOD,
            move || {
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.wake_work_document_finalization(&guard, wake_ticket)
                }) {
                    rejected.fail();
                }
            },
        );
        let Some(timer) = timer else {
            return false;
        };
        self.document_finalization_wake = Some(DocumentFinalizationWake {
            timer: Some(timer),
            ticket,
        });
        true
    }
    fn progress_document_finalization(
        &mut self,
        gate: &crate::platform::work_document_navigation::WorkDocumentNavigation,
    ) -> DocumentFinalizationProgress {
        let Some(ticket) = self.document_finalization_ready.take() else {
            if self.document_finalization_wake.is_some() {
                return DocumentFinalizationProgress::Pending;
            }
            return match gate
                .finalization_ticket()
                .filter(|ticket| self.schedule_document_finalization_wake(ticket.clone()))
            {
                Some(_) => DocumentFinalizationProgress::Pending,
                None => DocumentFinalizationProgress::WakeUnavailable,
            };
        };
        let outcome = self.view.as_ref().map_or(Err(()), |view| {
            gate.finalize_after_quiet_period(ticket, || {
                crate::platform::imp::current_url(view.view())
            })
        });
        match outcome {
            Ok(Some(effective)) => DocumentFinalizationProgress::Ready(effective),
            Ok(None) => match gate
                .finalization_ticket()
                .filter(|ticket| self.schedule_document_finalization_wake(ticket.clone()))
            {
                Some(_) => DocumentFinalizationProgress::Pending,
                None => DocumentFinalizationProgress::WakeUnavailable,
            },
            Err(()) => DocumentFinalizationProgress::Refused,
        }
    }
    /// A document an action's admitted load put in place (a saved consent)
    /// settles like a load of its own: a page that never reports it finished
    /// is taken as loaded after a short wait, then the quiet period and the
    /// location sample make it ready.
    fn progress_hand_on(&mut self) {
        if self.navigation.is_some() || self.history_back_pending() {
            return;
        }
        let Some(gate) = self
            .view
            .as_ref()
            .and_then(|view| view.work_navigation())
            .cloned()
        else {
            return;
        };
        // The construction's own load is finalized by the construction; only
        // its committed-but-never-finished wait is settled here.
        let constructing = self.construction.is_some();
        if !constructing && !gate.handed_on() {
            return;
        }
        let (committed, settle) = if constructing {
            (gate.committed_unfinished_since(), INITIAL_COMMITTED_SETTLE)
        } else {
            (gate.hand_on_committed_since(), HAND_ON_COMMITTED_SETTLE)
        };
        if let Some(since) = committed {
            if since.elapsed() >= settle {
                gate.accept_committed_load();
                self.hand_on_wake = None;
            } else if self.hand_on_wake.is_none() {
                let guard = self.guard.clone();
                self.hand_on_wake = crate::platform::imp::schedule_content_policy_timeout(
                    settle.saturating_sub(since.elapsed()) + Duration::from_millis(100),
                    move || {
                        let rejected = guard.clone();
                        if !crate::host::try_with_agent_context_terminal(move |host| {
                            host.progress_work_resource(&guard)
                        }) {
                            rejected.fail();
                        }
                    },
                );
            }
        }
        if !constructing && gate.finalization_pending() {
            self.hand_on_wake = None;
            match self.progress_document_finalization(&gate) {
                DocumentFinalizationProgress::Ready(_) | DocumentFinalizationProgress::Pending => {}
                DocumentFinalizationProgress::WakeUnavailable
                | DocumentFinalizationProgress::Refused => self.guard.fail(),
            }
        }
    }
    pub(super) fn resident(&self) -> bool {
        self.view.is_some()
    }
    fn destruction_drained(&self) -> bool {
        self.retirement_clean
            && self.view.is_none()
            && self.guard.callbacks_drained()
            && !self.frame_in_flight.load(Ordering::Acquire)
            && self.observation.is_none()
            && self.action.is_none()
            && self.navigation.is_none()
            && !self.history_back_pending()
            && {
                #[cfg(target_os = "macos")]
                {
                    self.screenshot.is_none()
                }
                #[cfg(not(target_os = "macos"))]
                {
                    true
                }
            }
    }
    fn retire_construction(&mut self) {
        self.retire_construction_presentation();
        if self
            .lifecycle_deadline
            .is_some_and(|(_, operation)| operation == Operation::Construct)
        {
            self.watchdog = None;
            self.lifecycle_deadline = None;
        }
        if let Some(task) = self.construction.take() {
            task.complete(Outcome::Refused);
        }
    }
    fn present_construction(&mut self) -> bool {
        use crate::platform::imp::{PresentationState, WorkObservationPresentation};
        let Some((deadline, Operation::Construct)) = self.lifecycle_deadline else {
            return false;
        };
        let Some(view) = &self.view else {
            return false;
        };
        if self.construction_presentation.is_some() {
            return false;
        }
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        let prepared = {
            let guard = self.guard.clone();
            WorkObservationPresentation::prepare(view.view(), deadline, move |failure| {
                guard.record_failure_cause(ResourceFailureCause::ConstructionPresentation(failure));
            })
        };
        #[cfg(not(feature = "native-agentic-work-lifetime-diagnostic"))]
        let prepared = WorkObservationPresentation::prepare(view.view(), deadline);
        let Ok(presentation) = prepared else {
            return false;
        };
        // Retain the native owner before exposing the loading page beneath chrome.
        self.construction_presentation = Some(presentation);
        self.construction_presentation
            .as_mut()
            .is_some_and(|presentation| {
                matches!(
                    presentation.present(),
                    PresentationState::Ready | PresentationState::Acquiring
                )
            })
    }
    fn retire_construction_presentation(&mut self) -> bool {
        let retired = self
            .construction_presentation
            .as_mut()
            .is_none_or(|presentation| {
                presentation.retire() == crate::platform::imp::PresentationState::Retired
            });
        if retired {
            self.construction_presentation = None;
        } else {
            #[cfg(target_os = "windows")]
            if self.frame_rendering_pending.load(Ordering::Acquire)
                && self
                    .construction_presentation
                    .as_mut()
                    .is_some_and(|presentation| {
                        presentation.poll() == crate::platform::imp::PresentationState::Retiring
                    })
            {
                // Original CapturePreview rendering debt is pending, not a
                // failed native retirement. Its callback wakes this same owner.
                return false;
            }
            self.retirement_clean = false;
            self.guard.fail();
        }
        retired
    }
    fn prepare_destruction(&mut self) -> bool {
        // Settle terminals this host still owns before waiting for their
        // physical delivery barriers. Ingress-owned queued tasks and accepted
        // reads remain independently owed; refusal is not a drain shortcut.
        self.retire_construction();
        self.cancel_observation(SemanticRuntimePortFailure::Shutdown);
        self.cancel_action();
        #[cfg(target_os = "macos")]
        if let Some(screenshot) = &self.screenshot {
            // Native capture owns physical capacity until its callback returns.
            // Cancellation prevents disclosure but does not counterfeit drain.
            screenshot.cancelled.store(true, Ordering::Release);
        }
        if let Some(navigation) = self.navigation.take() {
            navigation.refuse(ContextPortFailure::Shutdown);
        }
        if let Some(history) = self.history_back.take() {
            history.refuse(ContextPortFailure::Shutdown);
        }
        if let Some(task) = self.revocation.take() {
            task.complete(Outcome::Refused);
        }
        self.retire_page();
        self.destruction_drained()
    }
    pub(super) fn consistent(&self, id: ContextId) -> bool {
        self.guard.resource().identity().context() == id
            && self.native_resource.is_some()
            && self.retirement_clean
            && self.guard.is_healthy()
            && self.view.as_ref().is_none_or(|view| {
                view.work_navigation().is_some() && view.semantic_pending_for_audit().is_some()
            })
            && (self.view.is_none() || self.content_policy.is_some())
    }
    pub(super) fn ready(&self) -> bool {
        let Some(view) = self.view.as_ref() else {
            return false;
        };
        let Some(gate) = view.work_navigation() else {
            return false;
        };
        if self.guard.document().is_none() {
            return gate.bootstrap_ready();
        }
        gate.ready(crate::platform::imp::current_url(view.view()).as_deref())
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    fn deadline_failure_cause(&self, operation: Operation) -> ResourceFailureCause {
        Self::deadline_failure_cause_for_gate(
            operation,
            self.view.as_ref().and_then(|view| view.work_navigation()),
        )
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    fn deadline_failure_cause_for_gate(
        operation: Operation,
        gate: Option<&crate::platform::work_document_navigation::WorkDocumentNavigation>,
    ) -> ResourceFailureCause {
        use crate::WorkResourceDeadlineStage as Stage;

        let stage = match operation {
            Operation::Construct => {
                gate.map_or(Stage::ConstructionNativeSetup, |gate| gate.deadline_stage())
            }
            Operation::Revoke => Stage::RevocationDrain,
            Operation::Destroy => Stage::DestructionDrain,
            Operation::Acquire
            | Operation::PresentHuman
            | Operation::ContinueAfterHuman
            | Operation::ReadMedia => Stage::Unattributed,
        };
        ResourceFailureCause::LifecycleDeadline(stage)
    }
    /// One bounded frame for the canvas after a settled observation or action.
    /// Never more than one capture in flight, never faster than four a second.
    pub(super) fn capture_frame(&mut self) {
        self.frame_schedule.request();
        self.frame_rendering_pending.store(true, Ordering::Release);
        #[cfg(target_os = "windows")]
        if let Some(presentation) = self
            .reading_presentation
            .as_mut()
            .or(self.construction_presentation.as_mut())
        {
            presentation.retain_frame_capture(self.frame_rendering_pending.clone());
        }
        self.progress_frame_capture();
    }

    fn request_initial_frame(&mut self) {
        if self.frame_initial_requested
            || self.construction.is_none()
            || !self.guard.construction_current()
        {
            return;
        }
        let committed = self
            .view
            .as_ref()
            .and_then(|view| view.work_navigation())
            .and_then(|gate| gate.preview_document_stamp())
            .is_some();
        let visible = self
            .construction_presentation
            .as_ref()
            .is_some_and(crate::platform::imp::WorkObservationPresentation::visible_for_audit);
        if !committed || !visible {
            return;
        }
        self.frame_initial_requested = true;
        // One finite native paint opportunity, independent of semantic readiness.
        self.frame_schedule
            .request_after(Instant::now() + Duration::from_millis(100));
        self.frame_rendering_pending.store(true, Ordering::Release);
        #[cfg(target_os = "windows")]
        if let Some(presentation) = &mut self.construction_presentation {
            presentation.retain_frame_capture(self.frame_rendering_pending.clone());
        }
    }

    fn progress_frame_capture(&mut self) {
        use frame_schedule::FrameOpportunity;
        let in_flight = self.frame_in_flight.load(Ordering::Acquire);
        if !self.guard.is_healthy() || self.destruction.is_some() {
            self.frame_schedule.discard();
            self.frame_wake = None;
        }
        if self.construction.is_some()
            && !in_flight
            && self.ready()
            && self
                .view
                .as_ref()
                .is_some_and(|view| view.semantic_pending_for_audit() == Some(false))
        {
            // A fast document owes no new visual work. If semantic readiness
            // wins before dispatch, preserve construction's existing fast path;
            // its first read will request the ordinary settled picture.
            self.frame_schedule.discard();
            self.frame_wake = None;
        }
        if !self.frame_schedule.requested() {
            self.frame_rendering_pending
                .store(in_flight, Ordering::Release);
            return;
        }
        // An admitted operation owns its own completion wake. Capture the
        // newest settled state when it returns, rather than an intermediate
        // document or action whose native callback is still outstanding.
        if self.observation.is_some()
            || self.action.is_some()
            || self.navigation.is_some()
            || self.history_back_pending()
        {
            return;
        }
        let visible = self
            .reading_presentation
            .as_ref()
            .or(self.construction_presentation.as_ref())
            .is_some_and(crate::platform::imp::WorkObservationPresentation::visible_for_audit);
        if !visible || self.view.is_none() {
            self.frame_schedule.discard();
            self.frame_wake = None;
            self.frame_rendering_pending
                .store(in_flight, Ordering::Release);
            return;
        }
        let now = Instant::now();
        match self.frame_schedule.opportunity(now, in_flight) {
            FrameOpportunity::Idle | FrameOpportunity::Busy => return,
            FrameOpportunity::Wait(delay) => {
                if self.frame_wake.is_none() {
                    let guard = self.guard.clone();
                    self.frame_wake =
                        crate::platform::imp::schedule_content_policy_timeout(delay, move || {
                            let id = guard.resource().identity().context();
                            let _ = crate::host::try_with_agent_context_terminal(move |host| {
                                if let Some(resource) = host
                                    .work_resources
                                    .get_mut(&id)
                                    .filter(|resource| Arc::ptr_eq(&resource.guard, &guard))
                                {
                                    resource.frame_wake = None;
                                }
                                host.progress_work_resource(&guard);
                            });
                        });
                    if self.frame_wake.is_none() {
                        self.frame_schedule.discard();
                        self.frame_rendering_pending.store(false, Ordering::Release);
                    }
                }
                return;
            }
            FrameOpportunity::Capture => {}
        }
        self.frame_wake = None;
        self.frame_schedule.discard();
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let Some(gate) = view.work_navigation().cloned() else {
            self.frame_rendering_pending
                .store(in_flight, Ordering::Release);
            return;
        };
        let stamp = gate.preview_document_stamp();
        let Some(stamp) = stamp else {
            self.frame_rendering_pending
                .store(in_flight, Ordering::Release);
            return;
        };
        let id = self.guard.resource().identity().context();
        let generation = self.frame_generation.saturating_add(1);
        let in_flight = self.frame_in_flight.clone();
        let guard = self.guard.clone();
        #[cfg(all(
            target_os = "windows",
            feature = "native-agentic-work-lifetime-diagnostic"
        ))]
        guard.bind_frame_capture_diagnostic(&in_flight);
        in_flight.store(true, Ordering::Release);
        #[cfg(target_os = "windows")]
        let frame_rendering_guard = self
            .reading_presentation
            .as_mut()
            .or(self.construction_presentation.as_mut())
            .map(|presentation| {
                presentation.retain_frame_capture(self.frame_rendering_pending.clone())
            });
        #[cfg(all(
            target_os = "windows",
            feature = "native-agentic-work-lifetime-diagnostic"
        ))]
        if generation == 1 {
            view.diagnose_work_cookie_disk();
        }
        let dispatched = crate::platform::imp::capture_work_frame(view.view(), move |encoded| {
            let document_current = gate.preview_document_stamp() == Some(stamp);
            #[cfg(target_os = "windows")]
            let projection_current = guard.frame_projection_current();
            #[cfg(not(target_os = "windows"))]
            let projection_current = guard.is_healthy();
            if projection_current && document_current {
                if let Some((width, height, png)) = encoded {
                    super::work_frames::store(
                        id,
                        zephium_agentic::WorkBrowserFrame {
                            generation,
                            width,
                            height,
                            png: Arc::new(png),
                        },
                    );
                    // Windows finishes byte encoding after the observation's
                    // normal application wake. Publish this replacement through
                    // the original resource receiver as soon as it is ready.
                    guard.notify_frame_projection();
                }
            }
            #[cfg(target_os = "windows")]
            drop(frame_rendering_guard);
            in_flight.store(false, Ordering::Release);
            notify_work_resource(guard);
        });
        if dispatched {
            self.frame_generation = generation;
            self.frame_schedule.started(now);
            #[cfg(all(
                target_os = "windows",
                feature = "native-agentic-work-lifetime-diagnostic"
            ))]
            self.guard.notify_frame_capture_dispatched_diagnostic();
        } else {
            self.frame_in_flight.store(false, Ordering::Release);
            self.frame_rendering_pending.store(false, Ordering::Release);
        }
    }
    fn retire_page(&mut self) -> bool {
        #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
        if !form_oracle::before_retire(self) {
            return false;
        }
        if let Some(media) = &self.media_batch {
            media.cancel();
            return false;
        }
        #[cfg(target_os = "windows")]
        if self.frame_in_flight.load(Ordering::Acquire) {
            // Revoke the transferred reading/action fences, but preserve the
            // original capture's rendering opportunity and native debt.
            self.retire_reading_presentation();
            self.retire_observation_presentation();
            self.retire_action_presentation();
            return false;
        }
        self.watchdog = None;
        super::work_frames::clear(self.guard.resource().identity().context());
        if !self.retire_human_presentation()
            || !self.retire_construction_presentation()
            || !self.retire_reading_presentation()
            || !self.retire_observation_presentation()
            || !self.retire_action_presentation()
        {
            return false;
        }
        #[cfg(feature = "native-agentic-work-resource-probe")]
        if !self.retire_witness() {
            self.retirement_clean = false;
            self.guard.fail();
            return false;
        }
        let Some(view) = self.view.as_mut() else {
            return self.retirement_clean;
        };
        #[cfg(target_os = "windows")]
        if !view.cancel_work_storage()
            || !view.work_native_activity_drained()
            || self.frame_in_flight.load(Ordering::Acquire)
        {
            return false;
        }
        // Destruction only. Lease retirement never enters this method.
        crate::platform::imp::stop_loading(view.view());
        self.retirement_clean &= view.work_navigation().is_some_and(|gate| gate.retire());
        self.retirement_clean &= view.retire_semantic_runtime();
        self.retirement_clean &= self
            .content_policy
            .take()
            .is_some_and(|registration| registration.retire().is_ok());
        #[cfg(target_os = "windows")]
        if self.retirement_clean {
            let native_profile = view
                .work_native_profile()
                .unwrap_or(self.guard.resource().identity().profile());
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            if let Some(target) = self.guard.document() {
                super::work_windows::diagnose_work_cookie_before_close(view, target);
            }
            if let Err(debt) = view.close() {
                let debt = super::OwnedWindowsCleanupDebt::new(debt, self.native_resource.take());
                super::queue_windows_cleanup_debt(native_profile, debt);
                self.retirement_clean = false;
                self.guard.fail();
            }
        }
        if self.retirement_clean {
            self.view = None;
        } else {
            self.guard.fail();
        }
        self.retirement_clean
    }
}

#[cfg(target_os = "macos")]
fn resource_callback(guard: Arc<WorkResourceGuard>) {
    notify_work_resource(guard);
}
pub(crate) fn notify_work_resource(guard: Arc<WorkResourceGuard>) {
    if !guard.begin_notification() {
        return;
    }
    let Some(permit) = guard.notification_permit() else {
        return;
    };
    // Coalesced one-slot resource notification, after the original WebKit
    // callback unwinds. This is a callback barrier, not a readiness delay.
    let dispatch_guard = guard.clone();
    dispatch_guard.dispatch_notification(move || {
        guard.consume_notification();
        let rejected = guard.clone();
        if !crate::host::try_with_agent_context_terminal(move |host| {
            host.progress_work_resource(&guard)
        }) {
            rejected.fail();
        }
        drop(permit);
    });
}
#[derive(Clone)]
struct LifecycleDeadline {
    operation: Operation,
    lease: Option<zephium_agentic::WorkBrowserExecutionLease>,
}
impl LifecycleDeadline {
    fn from_task(task: &WorkLifecycleTask) -> Option<Self> {
        let request = task.request()?;
        Some(Self::from_request(request))
    }
    fn from_request(request: &zephium_agentic::WorkBrowserResourceRequest) -> Self {
        Self {
            operation: request.operation(),
            lease: request.lease().cloned(),
        }
    }
    fn matches_request(&self, request: &zephium_agentic::WorkBrowserResourceRequest) -> bool {
        request.operation() == self.operation && request.lease() == self.lease.as_ref()
    }
    fn matches(&self, task: &WorkLifecycleTask) -> bool {
        task.request()
            .is_some_and(|request| self.matches_request(request))
    }
}
fn timeout(
    guard: Arc<WorkResourceGuard>,
    deadline: LifecycleDeadline,
    duration: Duration,
) -> Option<crate::platform::imp::ContentPolicyTimeout> {
    crate::platform::imp::schedule_content_policy_timeout(duration, move || {
        let rejected = guard.clone();
        if !crate::host::try_with_agent_context_terminal(move |host| {
            host.expire_work_resource(&guard, &deadline)
        }) {
            rejected.fail();
        }
    })
}

impl EngineHost {
    #[cfg(target_os = "macos")]
    pub(super) fn work_execution_reservations(&self) -> usize {
        self.work_resources
            .values()
            .filter(|resource| resource.guard.execution_reserved())
            .count()
    }
    fn wake_work_document_finalization(
        &mut self,
        guard: &Arc<WorkResourceGuard>,
        ticket: crate::platform::work_document_navigation::WorkDocumentFinalizationTicket,
    ) {
        let id = guard.resource().identity().context();
        let ready = self
            .work_resources
            .get_mut(&id)
            .filter(|resource| Arc::ptr_eq(&resource.guard, guard))
            .is_some_and(|resource| {
                let Some(mut wake) = resource.document_finalization_wake.take() else {
                    return false;
                };
                if wake.ticket != ticket {
                    resource.document_finalization_wake = Some(wake);
                    return false;
                }
                // The one-shot entered before the host turn. Dropping its
                // handle here cannot manufacture another wake.
                wake.timer = None;
                if resource
                    .document_finalization_ready
                    .replace(ticket)
                    .is_some()
                {
                    guard.fail();
                }
                true
            });
        if ready {
            self.progress_work_resource(guard);
        }
    }
    pub(crate) fn handle_work_lifecycle_task(&mut self, task: WorkLifecycleTask) {
        let Some(request) = task.request() else {
            return;
        };
        let operation = request.operation();
        let guard = task.guard();
        let id = guard.resource().identity().context();
        if operation == Operation::Construct {
            if !guard.construction_current() {
                task.complete(Outcome::Refused);
                return;
            }
            self.construct_work_resource(task);
            return;
        }
        if operation == Operation::Destroy
            && !self.work_resources.contains_key(&id)
            && !guard.callbacks_drained()
        {
            if self.agent_contexts.contains_key(&id) {
                task.complete(Outcome::Refused);
                return;
            }
            // Destroy overtook the original Construct before host entry.
            // Retain the cleanup task below under the same bounded native
            // reservation; an absent view is not absence of that constructor.
            let reservation = self
                .native_resources
                .try_acquire(NativeResourceClass::AgentContext);
            let Ok(reservation) = reservation else {
                task.complete(Outcome::Refused);
                return;
            };
            self.work_resources.insert(
                id,
                WorkNativeResource::unconstructed(guard.clone(), reservation),
            );
        }
        let execution_count = self.agent_contexts.len()
            + self
                .work_resources
                .values()
                .filter(|resource| resource.guard.execution_reserved())
                .count();
        let Some(resource) = self
            .work_resources
            .get_mut(&id)
            .filter(|resource| Arc::ptr_eq(&resource.guard, &guard))
        else {
            // A rejected/failed construction retains an ingress owner but may
            // have proven no native allocation. Destruction can attest absence.
            task.complete(
                if operation == Operation::Destroy && guard.callbacks_drained() {
                    Outcome::Destroyed
                } else {
                    Outcome::Refused
                },
            );
            return;
        };
        match operation {
            Operation::ReadMedia => {
                resource.handle_media(task);
            }
            Operation::PresentHuman | Operation::ContinueAfterHuman => {
                resource.handle_human(task, self.main_dispatch.clone());
            }
            Operation::Acquire => {
                let mut accepted = request.lease().is_some_and(|lease| {
                    work_browser_monotonic_now()
                        .is_some_and(|now| guard.acquisition_current(lease, now))
                }) && !self.erasure_tombstones.contains(&resource.profile())
                    && execution_count <= MAX_EXECUTING_CONTEXTS
                    && resource.ready()
                    && !resource.pending();
                #[cfg(target_os = "macos")]
                if accepted {
                    accepted = resource
                        .view
                        .as_mut()
                        .and_then(|view| {
                            view.work_navigation()
                                .and_then(|gate| gate.ready_target())
                                .map(|target| (view, target))
                        })
                        .is_some_and(|(view, target)| view.begin_history_lease(target).is_ok());
                }
                #[cfg(target_os = "windows")]
                if accepted {
                    accepted = resource
                        .view
                        .as_ref()
                        .is_some_and(|view| view.set_work_leased(true));
                }
                #[cfg(target_os = "windows")]
                if accepted {
                    let target = resource
                        .view
                        .as_ref()
                        .and_then(|view| view.work_navigation())
                        .and_then(|gate| gate.ready_target());
                    if let Some((view, target)) = resource.view.as_mut().zip(target) {
                        let current = guard.clone();
                        view.enroll_work_history_with_completion(target, true, move |healthy| {
                            let accepted = healthy
                                && current.is_healthy()
                                && task
                                    .request()
                                    .and_then(|request| request.lease())
                                    .is_some_and(|lease| {
                                        work_browser_monotonic_now().is_some_and(|now| {
                                            current.acquisition_current(lease, now)
                                        })
                                    });
                            task.complete(if accepted {
                                Outcome::Acquired
                            } else {
                                Outcome::Refused
                            });
                        });
                        return;
                    }
                    accepted = false;
                }
                task.complete(if accepted {
                    Outcome::Acquired
                } else {
                    Outcome::Refused
                });
            }
            Operation::Revoke
                if resource.revocation.is_none() && resource.destruction.is_none() =>
            {
                let deadline = LifecycleDeadline::from_task(&task);
                resource.revocation = Some(task);
                #[cfg(target_os = "macos")]
                if let Some(screenshot) = &resource.screenshot {
                    screenshot.cancelled.store(true, Ordering::Release);
                }
                resource.deadline_expired = false;
                resource.lifecycle_deadline = Instant::now()
                    .checked_add(DRAIN_BUDGET)
                    .map(|deadline| (deadline, Operation::Revoke));
                resource.watchdog =
                    deadline.and_then(|deadline| timeout(guard.clone(), deadline, DRAIN_BUDGET));
                if resource.watchdog.is_none() || resource.lifecycle_deadline.is_none() {
                    resource.deadline_expired = true;
                    guard.fail();
                }
                self.progress_work_resource(&guard);
            }
            Operation::Destroy if resource.destruction.is_none() => {
                let deadline = LifecycleDeadline::from_task(&task);
                resource.destruction = Some(task);
                resource.retire_page();
                resource.deadline_expired = false;
                resource.lifecycle_deadline = Instant::now()
                    .checked_add(DRAIN_BUDGET)
                    .map(|deadline| (deadline, Operation::Destroy));
                resource.watchdog =
                    deadline.and_then(|deadline| timeout(guard.clone(), deadline, DRAIN_BUDGET));
                if resource.watchdog.is_none() || resource.lifecycle_deadline.is_none() {
                    resource.deadline_expired = true;
                    guard.fail();
                }
                self.progress_work_resource(&guard);
            }
            _ => task.complete(Outcome::Refused),
        }
    }

    fn construct_work_resource(&mut self, task: WorkLifecycleTask) {
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        eprintln!("windows-work-construction: stage=shared_construct; content=redacted");
        let guard = task.guard();
        let original_deadline = guard.construction_deadline(Instant::now());
        if original_deadline.is_none_or(|deadline| deadline <= Instant::now()) {
            task.complete(Outcome::Refused);
            return;
        }
        let result = self.build_work_resource(guard.clone());
        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
        if let Err(failure) = &result {
            eprintln!("windows-work-construction: stage=shared_admission port_failure={failure:?}; content=redacted");
        }
        let Ok(mut resource) = result else {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            if let Err(failure) = &result {
                guard.record_failure_cause(ResourceFailureCause::NativeAdmission(*failure));
            }
            #[cfg(feature = "native-agentic-work-resource-probe")]
            guard.record_construction_evidence(|| {
                crate::agent_context_port::resource_witness::ConstructionEvidence {
                    cause: "build_refused",
                    port_failure: result.err(),
                    navigation: None,
                    document_started: false,
                    deadline_expired: false,
                    guard_healthy: guard.is_healthy(),
                    current_document: false,
                    current_components: None,
                    semantic_pending: None,
                }
            });
            task.complete(Outcome::Refused);
            return;
        };
        let deadline = LifecycleDeadline::from_task(&task);
        resource.construction = Some(task);
        resource.lifecycle_deadline =
            original_deadline.map(|deadline| (deadline, Operation::Construct));
        resource.watchdog = deadline.and_then(|ticket| {
            original_deadline.and_then(|deadline| {
                timeout(
                    guard.clone(),
                    ticket,
                    deadline.saturating_duration_since(Instant::now()),
                )
            })
        });
        if resource.watchdog.is_none() || resource.lifecycle_deadline.is_none() {
            resource.deadline_expired = true;
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(ResourceFailureCause::NativeAdmission(
                ContextPortFailure::ResourceExhausted,
            ));
            #[cfg(feature = "native-agentic-work-resource-probe")]
            resource.record_construction_failure("construction_watchdog");
            guard.fail();
        }
        self.work_resources
            .insert(guard.resource().identity().context(), resource);
        self.progress_work_resource(&guard);
    }

    fn build_work_resource(
        &mut self,
        guard: Arc<WorkResourceGuard>,
    ) -> Result<WorkNativeResource, ContextPortFailure> {
        let id = guard.resource().identity().context();
        let profile = guard.resource().identity().profile();
        if !guard.construction_current() {
            return Err(ContextPortFailure::Shutdown);
        }
        if self.agent_contexts.contains_key(&id) || self.work_resources.contains_key(&id) {
            return Err(ContextPortFailure::Stale);
        }
        if self.agent_contexts.len() + self.work_resources.len() >= MAX_LIVE_CONTEXTS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if self.erasure_tombstones.contains(&profile) {
            return Err(ContextPortFailure::ProfileUnavailable);
        }
        let partition = match guard.storage() {
            ContextProfileStorageClass::Durable => Partition::Persistent(profile),
            ContextProfileStorageClass::Ephemeral => Partition::Ephemeral(profile),
        };
        if !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition) {
            return Err(ContextPortFailure::ProfileUnavailable);
        }
        let policy = self
            .applied_content_policy(profile)
            .ok_or(ContextPortFailure::ProfileUnavailable)?;
        if self.native_resource_accounting_failed || !self.native_resources.is_healthy() {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let mut native_resource = self
            .native_resources
            .try_acquire(NativeResourceClass::TransientConstruction)
            .map_err(|_| ContextPortFailure::ResourceExhausted)?;
        // Anonymous sessions never borrow the user's profile cookie store.
        #[cfg(target_os = "macos")]
        let store = if guard.work_website_data() {
            Some(self.work_data_store(profile)?)
        } else if guard.isolated_public() {
            Some(match guard.anonymous_session() {
                Some(session) if session.admits(profile, guard.resource().identity().work()) => {
                    let store = self.anonymous_work_store(session)?;
                    if let Some(visual) = self
                        .anonymous_work_stores
                        .get(&session.id())
                        .and_then(|entry| entry.visual.clone())
                    {
                        guard.bind_fresh_native_store(visual)?;
                    }
                    store
                }
                Some(_) => return Err(ContextPortFailure::Stale),
                None => crate::platform::imp::new_ephemeral_data_store()
                    .map_err(|_| ContextPortFailure::NativeRefused)?,
            })
        } else {
            match guard.storage() {
                ContextProfileStorageClass::Durable => None,
                ContextProfileStorageClass::Ephemeral => {
                    if self.macos_ephemeral_data_stores.len() >= MAX_PROFILE_PERSISTENCE_BINDINGS
                        && !self.macos_ephemeral_data_stores.contains_key(&profile)
                    {
                        return Err(ContextPortFailure::ProfileUnavailable);
                    }
                    let store = profile_scoped_value(
                        &mut self.macos_ephemeral_data_stores,
                        profile,
                        crate::platform::imp::new_ephemeral_data_store,
                    )
                    .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
                    if !profile_value_is_isolated(
                        &self.macos_ephemeral_data_stores,
                        profile,
                        &store,
                        |left, right| {
                            objc2::rc::Retained::as_ptr(left) == objc2::rc::Retained::as_ptr(right)
                        },
                    ) {
                        return Err(ContextPortFailure::ProfileUnavailable);
                    }
                    Some(store)
                }
            }
        };
        #[cfg(target_os = "macos")]
        let legacy = guard.clone();
        #[cfg(target_os = "macos")]
        let location = guard.clone();
        #[cfg(target_os = "macos")]
        let renderer = guard.clone();
        #[cfg(target_os = "macos")]
        let invariant = guard.clone();
        #[cfg(target_os = "macos")]
        let panic = guard.clone();
        #[cfg(target_os = "macos")]
        let redirected = guard.clone();
        #[cfg(target_os = "macos")]
        let unreachable = guard.clone();
        // Publish/retain this exact native reservation before a fallible
        // constructor can allocate a partial view/delegate graph. An error
        // is not absence proof and must not return its capacity to another
        // Work or legacy context.
        native_resource
            .reclassify(NativeResourceClass::AgentContext)
            .map_err(|_| ContextPortFailure::ResourceExhausted)?;
        let mut resource = WorkNativeResource {
            media_pending: false,
            media_batch: None,
            media_wake: None,
            guard: guard.clone(),
            construction: None,
            construction_presentation: None,
            human_presentation: None,
            human_wake: None,
            human_progress: None,
            human_unpresented: false,
            #[cfg(target_os = "windows")]
            human_return: None,
            revocation: None,
            destruction: None,
            watchdog: None,
            observation: None,
            action: None,
            reading_presentation: None,
            presentation_wake: None,
            frame_generation: 0,
            frame_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            frame_rendering_pending: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            frame_schedule: frame_schedule::WorkFrameSchedule::default(),
            frame_initial_requested: false,
            frame_wake: None,
            navigation: None,
            history_back: None,
            #[cfg(target_os = "macos")]
            screenshot: None,
            document_finalization_wake: None,
            document_finalization_ready: None,
            hand_on_wake: None,
            #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
            form_oracle: form_oracle::State::default(),
            #[cfg(all(target_os = "macos", feature = "native-agentic-semantic-probe"))]
            form_actions: crate::platform::form_qualification::ActionAudit::default(),
            last_invocation: 0,
            looked_document: None,
            document_started: false,
            retirement_clean: false,
            deadline_expired: false,
            lifecycle_deadline: None,
            content_policy: None,
            view: None,
            native_resource: Some(native_resource),
            #[cfg(feature = "native-agentic-work-resource-probe")]
            witness: None,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            witness_attempted: false,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            witness_admission: None,
            #[cfg(feature = "native-agentic-work-resource-probe")]
            presentation_observations: 0,
        };
        #[cfg(target_os = "macos")]
        let credential_denied = guard.clone();
        #[cfg(target_os = "macos")]
        let callbacks = crate::platform::imp::AgentOwnedViewCallbacks::new(
            move |_| {
                legacy.fail();
                resource_callback(legacy.clone());
            },
            move || resource_callback(location.clone()),
            move || {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                renderer.record_failure_cause(ResourceFailureCause::RendererLost);
                renderer.fail();
                resource_callback(renderer.clone());
            },
            move || {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                invariant.record_failure_cause(ResourceFailureCause::SemanticNativeInvariant);
                invariant.fail();
                resource_callback(invariant.clone());
            },
            move || {
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                panic.record_failure_cause(ResourceFailureCause::SemanticNativeInvariant);
                panic.fail();
                resource_callback(panic.clone());
            },
        )
        .with_credential_denied(move || credential_denied.revoke_public_visual_lineage())
        .with_redirect(move |destination| {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            redirected.record_failure_cause(ResourceFailureCause::Redirected);
            redirected.record_redirect(destination);
            redirected.fail();
            resource_callback(redirected.clone());
        })
        .with_unreachable(move || {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            unreachable.record_failure_cause(ResourceFailureCause::Unreachable);
            unreachable.record_unreachable();
        });
        #[cfg(all(
            target_os = "macos",
            feature = "native-agentic-work-lifetime-diagnostic"
        ))]
        let callbacks = {
            let diagnostic = guard.clone();
            callbacks
                .with_failure_diagnostic(move |failure| diagnostic.record_failure_cause(failure))
        };
        #[cfg(target_os = "macos")]
        let view = crate::platform::imp::build_owned_work_view(
            &self.parent,
            ContextOwnedViewport::STANDARD,
            profile,
            if guard.work_website_data() {
                ContextProfileStorageClass::Durable
            } else if guard.isolated_public() {
                ContextProfileStorageClass::Ephemeral
            } else {
                guard.storage()
            },
            store.as_ref(),
            callbacks,
        );
        #[cfg(target_os = "windows")]
        let view = self.build_windows_work_view(&guard, &mut resource.native_resource);
        let Ok(view) = view else {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(ResourceFailureCause::NativeAdmission(
                ContextPortFailure::NativeRefused,
            ));
            #[cfg(feature = "native-agentic-work-resource-probe")]
            resource.record_construction_failure("view_build");
            guard.fail();
            return Ok(resource);
        };
        let registration =
            crate::platform::imp::install_content_policy_on_view(view.view(), &policy);
        resource.view = Some(view);
        let Ok(registration) = registration else {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            eprintln!("windows-work-construction: stage=content_policy_install refused=true; content=redacted");
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(ResourceFailureCause::NativeAdmission(
                ContextPortFailure::NativeRefused,
            ));
            #[cfg(feature = "native-agentic-work-resource-probe")]
            resource.record_construction_failure("content_policy_install");
            guard.fail();
            return Ok(resource);
        };
        resource.content_policy = Some(registration);
        resource.retirement_clean = true;
        Ok(resource)
    }

    pub(crate) fn progress_work_resource(&mut self, guard: &Arc<WorkResourceGuard>) {
        self.progress_work_action(guard);
        self.progress_work_observation(guard);
        self.progress_work_media(guard);
        let id = guard.resource().identity().context();
        let Some(resource) = self
            .work_resources
            .get_mut(&id)
            .filter(|resource| Arc::ptr_eq(&resource.guard, guard))
        else {
            return;
        };
        let expired_lifecycle = resource
            .lifecycle_deadline
            .and_then(|(deadline, operation)| (Instant::now() >= deadline).then_some(operation));
        if let Some(_operation) = expired_lifecycle {
            if _operation == Operation::Construct && resource.document_started {
                guard.construction_timed_out();
            }
            resource.deadline_expired = true;
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(resource.deadline_failure_cause(_operation));
            #[cfg(feature = "native-agentic-work-resource-probe")]
            if resource.construction.is_some() {
                resource.record_construction_failure("construction_deadline");
            }
            guard.fail();
        }
        if resource.destruction.is_some() {
            // Cancellation/destruction discards undispatched preview demand.
            // Already dispatched native capture debt still owns its renderer.
            resource.frame_schedule.discard();
            resource.frame_wake = None;
            resource.frame_rendering_pending.store(
                resource.frame_in_flight.load(Ordering::Acquire),
                Ordering::Release,
            );
            if resource.prepare_destruction() {
                let Some(mut resource) = self.work_resources.remove(&id) else {
                    guard.fail();
                    return;
                };
                resource.watchdog = None;
                resource.lifecycle_deadline = None;
                let task = resource.destruction.take();
                drop(resource);
                #[cfg(target_os = "windows")]
                self.release_work_profile_cookie_source(id);
                #[cfg(target_os = "windows")]
                self.retire_windows_anonymous_work_sessions();
                if let Some(task) = task {
                    task.complete(Outcome::Destroyed);
                }
            } else if !resource.retirement_clean || resource.deadline_expired {
                if let Some(task) = resource.destruction.take() {
                    task.complete(Outcome::Refused);
                }
            }
            return;
        }
        resource.progress_navigation(self.erasure_tombstones.contains(&resource.profile()));
        resource.progress_history_back(self.erasure_tombstones.contains(&resource.profile()));
        resource.progress_human(self.erasure_tombstones.contains(&resource.profile()));
        resource.progress_hand_on();
        resource.request_initial_frame();
        resource.progress_frame_capture();
        if resource
            .construction_presentation
            .as_mut()
            .is_some_and(|presentation| {
                !matches!(
                    presentation.poll(),
                    crate::platform::imp::PresentationState::Ready
                        | crate::platform::imp::PresentationState::Acquiring
                )
            })
        {
            guard.fail();
        }
        if resource
            .view
            .as_ref()
            .and_then(|view| view.work_navigation())
            .is_none_or(|gate| gate.failed())
        {
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(
                resource
                    .view
                    .as_ref()
                    .and_then(|view| view.work_navigation())
                    .and_then(|gate| gate.url_observation_failure())
                    .map_or(
                        ResourceFailureCause::UnattributedResourceFailure,
                        ResourceFailureCause::UrlObservationRefused,
                    ),
            );
            #[cfg(feature = "native-agentic-work-resource-probe")]
            if resource.construction.is_some() {
                resource.record_construction_failure("navigation_gate");
            }
            guard.fail();
        }
        if !guard.is_healthy() {
            resource.retire_human_presentation();
            resource.retire_construction_presentation();
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(ResourceFailureCause::UnattributedResourceFailure);
            #[cfg(feature = "native-agentic-work-resource-probe")]
            if resource.construction.is_some() {
                resource.record_construction_failure("native_health");
            }
            resource.watchdog = None;
            resource.document_finalization_wake = None;
            resource.document_finalization_ready = None;
            resource.lifecycle_deadline = None;
            if let Some(task) = resource.construction.take() {
                task.complete(Outcome::Refused);
            }
            if let Some(task) = resource.revocation.take() {
                task.complete(Outcome::Refused);
            }
            #[cfg(target_os = "windows")]
            if let Some((task, _)) = resource.human_return.take() {
                task.complete(Outcome::Refused);
            }
            return;
        }
        if resource.construction.is_some() {
            if !guard.construction_current() {
                resource.retire_construction_presentation();
                #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                guard.record_failure_cause(ResourceFailureCause::UnattributedResourceFailure);
                #[cfg(feature = "native-agentic-work-resource-probe")]
                resource.record_construction_failure("construction_authority");
                guard.fail();
                resource.watchdog = None;
                resource.lifecycle_deadline = None;
                if let Some(task) = resource.construction.take() {
                    task.complete(Outcome::Refused);
                }
                return;
            }
            if !resource.document_started {
                let Some(view) = resource.view.as_mut() else {
                    guard.fail();
                    return;
                };
                #[cfg(target_os = "windows")]
                view.progress_work_storage();
                let Some(gate) = view.work_navigation().cloned() else {
                    guard.fail();
                    return;
                };
                if !gate.bootstrap_ready() {
                    return;
                }
                #[cfg(target_os = "windows")]
                if !view.work_storage_ready() {
                    return;
                }
                if let Some(document) = guard.document() {
                    if !resource.present_construction() {
                        guard.fail();
                        resource.retire_construction();
                        return;
                    }
                    let Some(view) = resource.view.as_mut() else {
                        guard.fail();
                        resource.retire_construction();
                        return;
                    };
                    #[cfg(target_os = "windows")]
                    if !view.admit_work_store_page() {
                        return;
                    }
                    resource.document_started = true;
                    if view.prepare_semantic_document_load().is_err()
                        || gate
                            .arm_with_policy(document.clone(), guard.document_policy())
                            .is_err()
                        || view.view().load_url(document.as_url().as_str()).is_err()
                    {
                        #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                        guard.record_failure_cause(ResourceFailureCause::NativeAdmission(
                            ContextPortFailure::NativeRefused,
                        ));
                        #[cfg(feature = "native-agentic-work-resource-probe")]
                        resource.record_construction_failure("document_dispatch");
                        guard.fail();
                        resource.retire_construction();
                    }
                    return;
                }
            }
            if resource
                .view
                .as_ref()
                .is_some_and(|view| view.semantic_pending_for_audit() == Some(false))
            {
                let finalizing = resource
                    .view
                    .as_ref()
                    .and_then(|view| view.work_navigation())
                    .filter(|gate| gate.finalization_pending())
                    .cloned();
                // Drain the original finite visual request before retiring its
                // native owner. Keep the original construction watchdog and
                // deadline; no frame can renew construction or input authority.
                if resource.frame_rendering_pending.load(Ordering::Acquire) {
                    // Start the existing native quiet interval concurrently;
                    // never consume its ready ticket or sample a URL until the
                    // original visual capture has returned.
                    if let Some(gate) = &finalizing {
                        if resource.document_finalization_wake.is_none()
                            && resource.document_finalization_ready.is_none()
                            && !matches!(
                                resource.progress_document_finalization(gate),
                                DocumentFinalizationProgress::Pending
                            )
                        {
                            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                            guard.record_failure_cause(
                                ResourceFailureCause::DocumentFinalizationRefused,
                            );
                            guard.fail();
                            resource.retire_construction();
                        }
                    }
                    return;
                }
                if let Some(gate) = finalizing {
                    // No lease/read exists here. Require one native quiet
                    // period, then freeze one revision-fenced location sample
                    // under the original navigation identity.
                    match resource.progress_document_finalization(&gate) {
                        DocumentFinalizationProgress::Pending => return,
                        DocumentFinalizationProgress::Ready(effective)
                            if guard.construction_current() =>
                        {
                            let retired = resource.retire_construction_presentation();
                            #[cfg(target_os = "windows")]
                            let retired = retired
                                && resource
                                    .view
                                    .as_ref()
                                    .is_some_and(|view| view.set_work_leased(false));
                            resource.watchdog = None;
                            resource.lifecycle_deadline = None;
                            if let Some(task) = resource.construction.take() {
                                if retired {
                                    task.complete_document(effective);
                                } else {
                                    task.complete(Outcome::Refused);
                                }
                            }
                        }
                        DocumentFinalizationProgress::Ready(_)
                        | DocumentFinalizationProgress::WakeUnavailable
                        | DocumentFinalizationProgress::Refused => {
                            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
                            guard.record_failure_cause(
                                ResourceFailureCause::DocumentFinalizationRefused,
                            );
                            #[cfg(feature = "native-agentic-work-resource-probe")]
                            guard.record_construction_evidence(|| {
                                // A raced authoritative sample is never repeated
                                // for diagnostics. A stale quiet ticket consumed
                                // no sample and would have remained Pending.
                                crate::agent_context_port::resource_witness::ConstructionEvidence {
                                    cause: "document_finalization",
                                    port_failure: None,
                                    navigation: gate.construction_evidence(),
                                    document_started: resource.document_started,
                                    deadline_expired: resource.deadline_expired,
                                    guard_healthy: guard.is_healthy(),
                                    current_document: false,
                                    current_components: None,
                                    semantic_pending: resource
                                        .view
                                        .as_ref()
                                        .and_then(|view| view.semantic_pending_for_audit()),
                                }
                            });
                            guard.fail();
                            resource.retire_construction_presentation();
                            resource.watchdog = None;
                            resource.document_finalization_wake = None;
                            resource.document_finalization_ready = None;
                            resource.lifecycle_deadline = None;
                            if let Some(task) = resource.construction.take() {
                                task.complete(Outcome::Refused);
                            }
                        }
                    }
                    return;
                }
            }
            if resource.ready()
                && resource
                    .view
                    .as_ref()
                    .is_some_and(|view| view.semantic_pending_for_audit() == Some(false))
            {
                let retired = resource.retire_construction_presentation();
                #[cfg(target_os = "windows")]
                let retired = retired
                    && resource
                        .view
                        .as_ref()
                        .is_some_and(|view| view.set_work_leased(false));
                resource.watchdog = None;
                resource.lifecycle_deadline = None;
                if let Some(task) = resource.construction.take() {
                    task.complete(if retired {
                        Outcome::Constructed
                    } else {
                        Outcome::Refused
                    });
                }
            }
            return;
        }
        let frame_drained = {
            #[cfg(target_os = "windows")]
            {
                !resource.frame_rendering_pending.load(Ordering::Acquire)
            }
            #[cfg(not(target_os = "windows"))]
            {
                true
            }
        };
        // CapturePreview still owns a rendering opportunity after snapshot
        // delivery. Hiding its child or suspending its controller can strand
        // that native callback. The presenter revokes input immediately while
        // deferring its native hide until the exact capture flag drains.
        let presentation_retired = if resource.revocation.is_some() {
            resource.retire_reading_presentation()
        } else {
            true
        };
        if let Some(task) = resource.revocation.as_ref() {
            let drained = frame_drained
                && presentation_retired
                && task
                    .request()
                    .and_then(|request| request.lease())
                    .is_some_and(|lease| guard.lease_drained(lease))
                && resource.observation.is_none()
                && resource.action.is_none()
                && resource.navigation.is_none()
                && !resource.history_back_pending()
                && {
                    #[cfg(target_os = "macos")]
                    {
                        resource.screenshot.is_none()
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        true
                    }
                }
                && resource.ready()
                && resource
                    .view
                    .as_ref()
                    .is_some_and(|view| view.semantic_pending_for_audit() == Some(false));
            if drained {
                #[cfg(target_os = "windows")]
                if resource
                    .view
                    .as_ref()
                    .is_none_or(|view| !view.set_work_leased(false))
                {
                    guard.fail();
                    return;
                }
                #[cfg(target_os = "windows")]
                if resource
                    .view
                    .as_ref()
                    .is_none_or(|view| !view.work_native_activity_drained())
                    || resource.frame_in_flight.load(Ordering::Acquire)
                {
                    return;
                }
                resource.watchdog = None;
                resource.lifecycle_deadline = None;
                if let Some(task) = resource.revocation.take() {
                    task.complete(Outcome::Revoked {
                        debt: WorkBrowserLeaseNativeDebt::default(),
                        resource_retained: true,
                    });
                }
            }
        }
    }

    fn expire_work_resource(
        &mut self,
        guard: &Arc<WorkResourceGuard>,
        deadline: &LifecycleDeadline,
    ) {
        let Some(resource) = self
            .work_resources
            .get_mut(&guard.resource().identity().context())
            .filter(|resource| Arc::ptr_eq(&resource.guard, guard))
        else {
            return;
        };
        if resource
            .construction
            .as_ref()
            .is_some_and(|task| deadline.matches(task))
            || resource
                .revocation
                .as_ref()
                .is_some_and(|task| deadline.matches(task))
            || resource
                .destruction
                .as_ref()
                .is_some_and(|task| deadline.matches(task))
        {
            if deadline.operation == Operation::Construct && resource.document_started {
                guard.construction_timed_out();
            }
            resource.deadline_expired = true;
            #[cfg(all(
                target_os = "windows",
                feature = "native-agentic-work-lifetime-diagnostic"
            ))]
            if deadline.operation == Operation::Revoke {
                eprintln!("windows-work-revocation: guard_facts={:?} presentation_absent={} observation_absent={} action_absent={} navigation_absent={} history_absent={} document_ready={} semantic_pending={:?} native_activity_drained={} native_suspended={:?} frame_pending={}; content=redacted",
                    deadline.lease.as_ref().and_then(|lease| guard.revocation_drain_facts(lease)),
                    resource.reading_presentation.is_none(), resource.observation.is_none(), resource.action.is_none(), resource.navigation.is_none(), !resource.history_back_pending(), resource.ready(),
                    resource.view.as_ref().and_then(|view| view.semantic_pending_for_audit()),
                    resource.view.as_ref().is_some_and(|view| view.work_native_activity_drained()),
                    resource.view.as_ref().map(|view| view.is_suspended().ok()),
                    resource.frame_in_flight.load(Ordering::Acquire));
            }
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            guard.record_failure_cause(resource.deadline_failure_cause(deadline.operation));
            #[cfg(feature = "native-agentic-work-resource-probe")]
            if resource.construction.is_some() {
                resource.record_construction_failure("construction_deadline");
            }
            guard.fail();
            self.progress_work_resource(guard);
        }
    }

    pub(super) fn force_shutdown_work_resources(&mut self) -> bool {
        let was_empty = self.work_resources.is_empty();
        for resource in self.work_resources.values_mut() {
            resource.guard.fail();
            resource.retire_page();
        }
        // Forced teardown is never exact shell/application closure. Retain
        // unresolved task/resource reservations until the host itself drops.
        was_empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use zephium_agentic::{
        AgentPolicyInstant, ContextNavigationTarget, ContextRunId, WorkBrowserResourceId,
        WorkBrowserResources, WorkId,
    };
    #[test]
    fn overtaking_destroy_retains_no_view_owner_until_constructor_and_barrier_drain() {
        destroy_construction_schedule(false);
    }
    #[test]
    fn destroy_settles_host_retained_constructor_before_waiting_for_its_barrier() {
        destroy_construction_schedule(true);
    }
    #[test]
    fn destroy_settles_host_retained_tracked_revoke_before_waiting_for_read_and_delivery() {
        for read_before_wake in [false, true] {
            destroy_revocation_schedule(true, read_before_wake);
        }
    }
    #[test]
    fn destroy_settles_host_retained_legacy_revoke_before_waiting_for_read_and_delivery() {
        for read_before_wake in [false, true] {
            destroy_revocation_schedule(false, read_before_wake);
        }
    }
    fn destroy_revocation_schedule(tracked: bool, read_before_wake: bool) {
        use super::super::resources::NativeResourceLedger;
        use std::sync::atomic::{AtomicU8, Ordering};
        use zephium_agentic::{WorkBrowserObservationEvent, WorkBrowserResourceEvent};

        let tick = AgentPolicyInstant::from_millis;
        let rows = Arc::new(Mutex::new(WorkBrowserResources::new(
            WorkId::generate(),
            ProfileId::generate(),
        )));
        let construct = rows
            .lock()
            .unwrap()
            .construct_document(
                WorkBrowserResourceId::generate(),
                ContextId::generate(),
                ContextProfileStorageClass::Ephemeral,
                ContextNavigationTarget::parse("https://example.test/").unwrap(),
                tick(0),
            )
            .unwrap();
        let join = construct.resource().clone();
        let queue = Arc::new(Mutex::new(Vec::<Box<dyn FnOnce() + Send>>::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        // Native delivery intentionally contains receiver panics. Assert these
        // checks ran through normal return rather than swallowing test failures.
        let callback_checks = Arc::new(AtomicU8::new(0));
        let callback = || {
            let rows = rows.clone();
            let events = events.clone();
            Box::new(move |completion| {
                events
                    .lock()
                    .unwrap()
                    .push(rows.lock().unwrap().settle_at(completion, tick(1)).unwrap());
            }) as zephium_agentic::WorkBrowserResourceCompletionCallback
        };
        let queued = queue.clone();
        let construction = WorkLifecycleTask::construction_for_test(
            construct,
            callback(),
            Arc::new(move |task| {
                queued.lock().unwrap().push(task);
                true
            }),
        );
        let guard = construction.guard();
        let closure = construction.closure_for_test();
        construction.complete(Outcome::Constructed);
        let acquire = rows
            .lock()
            .unwrap()
            .acquire(&join, ContextRunId::generate(), tick(1), tick(1_000_000))
            .unwrap();
        let lease = acquire.lease().unwrap().clone();
        guard
            .lifecycle_for_test(acquire, callback())
            .complete(Outcome::Acquired);
        events.lock().unwrap().clear();

        // A real accepted read envelope is held before native execution. Its
        // result callback still belongs to this lease; there is no fabricated
        // zero counter, native view, platform timer or replacement host loop.
        let read = rows
            .lock()
            .unwrap()
            .observe_initial(&lease, tick(1))
            .unwrap();
        let read_rows = rows.clone();
        let read_guard = guard.clone();
        let read_checks = callback_checks.clone();
        let read = guard.observation_for_test(
            read,
            Box::new(move |completion| {
                assert!(matches!(
                    read_rows
                        .lock()
                        .unwrap()
                        .settle_observation(completion, tick(1))
                        .unwrap(),
                    WorkBrowserObservationEvent::DebtSettled
                ));
                assert!(!read_guard.callbacks_drained());
                read_guard.assert_shutdown_for_test(false);
                read_checks.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let (revoke, ticket) = if tracked {
            let (request, ticket) = rows.lock().unwrap().revoke_with_delivery(&lease).unwrap();
            (request, Some(ticket))
        } else {
            (rows.lock().unwrap().revoke(&lease).unwrap(), None)
        };
        let ticket = Arc::new(Mutex::new(ticket));
        let during_ticket = ticket.clone();
        let during_guard = guard.clone();
        let revoke_checks = callback_checks.clone();
        let core_callback = callback();
        let revocation = guard.lifecycle_for_test(
            revoke,
            Box::new(move |completion| {
                core_callback(completion);
                if let Some(ticket) = during_ticket.lock().unwrap().as_mut() {
                    assert!(ticket.try_take().unwrap().is_none());
                }
                assert!(!during_guard.callbacks_drained());
                assert!(during_guard.execution_reserved());
                during_guard.assert_shutdown_for_test(false);
                revoke_checks.fetch_add(1, Ordering::SeqCst);
            }),
        );
        rows.lock().unwrap().quarantine(&join).unwrap();
        let destroy = rows.lock().unwrap().destroy(&join).unwrap();
        let during_guard = guard.clone();
        let destroy_checks = callback_checks.clone();
        let core_callback = callback();
        let destruction = guard.lifecycle_for_test(
            destroy,
            Box::new(move |completion| {
                core_callback(completion);
                during_guard.assert_shutdown_for_test(false);
                destroy_checks.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let ledger = NativeResourceLedger::default();
        let reservation = ledger
            .try_acquire(NativeResourceClass::AgentContext)
            .unwrap();
        let mut resource = WorkNativeResource::unconstructed(guard.clone(), reservation);
        resource.revocation = Some(revocation);
        resource.destruction = Some(destruction);
        assert!(!resource.destruction_drained());
        assert_eq!(closure(), (false, Some(3)));

        // This is the same preparation path used by host destruction progress.
        // It must refuse the Revoke it owns before testing the delivery barrier.
        assert!(!resource.prepare_destruction());
        assert_eq!(callback_checks.load(Ordering::SeqCst), 1);
        assert!(resource.revocation.is_none());
        assert!(resource.view.is_none());
        assert_eq!(
            ledger.count_for_audit(NativeResourceClass::AgentContext),
            Some(1)
        );
        assert_eq!(closure(), (false, Some(3))); // read, Destroy, deferred wake
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [WorkBrowserResourceEvent::DebtSettled(_)]
        ));
        if let Some(ticket) = ticket.lock().unwrap().as_mut() {
            assert!(!ticket.try_take().unwrap().unwrap().returned());
        }
        assert_eq!(queue.lock().unwrap().len(), 1);
        let mut read = Some(read);
        if read_before_wake {
            read.take()
                .unwrap()
                .refuse(SemanticRuntimePortFailure::Cancelled);
            assert!(
                !resource.prepare_destruction(),
                "deferred delivery wake remains owned"
            );
            assert_eq!(closure(), (false, Some(2)));
        }
        queue.lock().unwrap().pop().unwrap()();
        if let Some(read) = read {
            assert!(
                !resource.prepare_destruction(),
                "read still physically owned"
            );
            assert_eq!(closure(), (false, Some(2)));
            assert!(resource.destruction.is_some());
            read.refuse(SemanticRuntimePortFailure::Cancelled);
        }
        assert!(resource.prepare_destruction());
        assert_eq!(callback_checks.load(Ordering::SeqCst), 2);
        assert_eq!(closure(), (false, Some(1)));
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [WorkBrowserResourceEvent::DebtSettled(_)]
        ));
        let destruction = resource.destruction.take().unwrap();
        drop(resource);
        assert!(ledger.is_quiescent());
        destruction.complete(Outcome::Destroyed);
        assert_eq!(callback_checks.load(Ordering::SeqCst), 3);
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [
                WorkBrowserResourceEvent::DebtSettled(_),
                WorkBrowserResourceEvent::Destroyed(_)
            ]
        ));
        assert_eq!(closure(), (true, Some(0)));
        rows.lock().unwrap().seal();
        assert!(rows.lock().unwrap().is_quiescent());
        assert!(!guard.admits(&lease, tick(1)));
        guard.assert_shutdown_for_test(true);
    }
    fn destroy_construction_schedule(constructor_at_host: bool) {
        use super::super::resources::NativeResourceLedger;
        use zephium_agentic::WorkBrowserResourceEvent;

        let tick = AgentPolicyInstant::from_millis;
        let rows = Arc::new(Mutex::new(WorkBrowserResources::new(
            WorkId::generate(),
            ProfileId::generate(),
        )));
        let construct = rows
            .lock()
            .unwrap()
            .construct_document(
                WorkBrowserResourceId::generate(),
                ContextId::generate(),
                ContextProfileStorageClass::Ephemeral,
                ContextNavigationTarget::parse("https://example.test/").unwrap(),
                tick(0),
            )
            .unwrap();
        let join = construct.resource().clone();
        let queue = Arc::new(Mutex::new(Vec::<Box<dyn FnOnce() + Send>>::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let callback = || {
            let rows = rows.clone();
            let events = events.clone();
            Box::new(move |completion| {
                events
                    .lock()
                    .unwrap()
                    .push(rows.lock().unwrap().settle_at(completion, tick(1)).unwrap());
            }) as zephium_agentic::WorkBrowserResourceCompletionCallback
        };
        let queued = queue.clone();
        let construction = WorkLifecycleTask::construction_for_test(
            construct,
            callback(),
            Arc::new(move |task| {
                queued.lock().unwrap().push(task);
                true
            }),
        );
        let guard = construction.guard();
        let closure = construction.closure_for_test();
        rows.lock().unwrap().quarantine(&join).unwrap();
        let destroy = rows.lock().unwrap().destroy(&join).unwrap();
        let destruction = construction.followup_for_test(destroy, callback());

        // The overtaking host task owns one bounded cleanup reservation, not
        // a page. The original Construct has not reached native execution.
        let ledger = NativeResourceLedger::default();
        assert!(ledger.is_quiescent());
        let reservation = ledger
            .try_acquire(NativeResourceClass::AgentContext)
            .unwrap();
        let mut resource = WorkNativeResource::unconstructed(guard.clone(), reservation);
        resource.destruction = Some(destruction);
        assert!(!guard.construction_current());
        assert!(resource.view.is_none());
        assert!(!resource.destruction_drained());
        assert!(events.lock().unwrap().is_empty());
        assert_eq!(closure(), (false, Some(2)));
        assert_eq!(
            ledger.count_for_audit(NativeResourceClass::AgentContext),
            Some(1)
        );

        // The actual stale-construction terminal cannot create a view or a
        // second reservation. Its queued physical callback barrier still
        // prevents Destroyed, even after core has accounted Construct debt.
        if constructor_at_host {
            resource.construction = Some(construction);
            resource.retire_construction();
            assert!(resource.construction.is_none());
        } else {
            resource.retire_construction();
            assert!(!resource.destruction_drained());
            construction.complete(Outcome::Refused);
        }
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [WorkBrowserResourceEvent::DebtSettled(_)]
        ));
        assert!(resource.view.is_none());
        assert_eq!(
            ledger.count_for_audit(NativeResourceClass::AgentContext),
            Some(1)
        );
        assert_eq!(queue.lock().unwrap().len(), 1);
        assert_eq!(closure(), (false, Some(2)));
        assert!(!resource.destruction_drained());

        // A missing host at the deferred wake is itself fail-closed. It may
        // quarantine this resource, but cannot erase the destruction owner.
        queue.lock().unwrap().pop().unwrap()();
        assert!(resource.destruction_drained());
        assert_eq!(closure(), (false, Some(1)));
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [WorkBrowserResourceEvent::DebtSettled(_)]
        ));
        let destruction = resource.destruction.take().unwrap();
        drop(resource);
        assert!(ledger.is_quiescent());
        destruction.complete(Outcome::Destroyed);
        assert!(matches!(
            events.lock().unwrap().as_slice(),
            [
                WorkBrowserResourceEvent::DebtSettled(_),
                WorkBrowserResourceEvent::Destroyed(_)
            ]
        ));
        assert_eq!(closure(), (true, Some(0)));
        rows.lock().unwrap().seal();
        assert!(rows.lock().unwrap().is_quiescent());
        assert!(!guard.construction_current());
    }
    #[test]
    fn cancelled_revoke_deadline_cannot_quarantine_a_successor_lease() {
        let tick = AgentPolicyInstant::from_millis;
        let mut rows = WorkBrowserResources::new(WorkId::generate(), ProfileId::generate());
        let request = rows
            .construct_document(
                WorkBrowserResourceId::generate(),
                ContextId::generate(),
                ContextProfileStorageClass::Ephemeral,
                ContextNavigationTarget::parse("https://example.test/").unwrap(),
                tick(0),
            )
            .unwrap();
        let resource = request.resource().clone();
        let _ = rows
            .settle_at(request.complete(Outcome::Constructed), tick(0))
            .unwrap();
        let mut deadlines = Vec::new();
        for _ in 0..2 {
            let request = rows
                .acquire(&resource, ContextRunId::generate(), tick(1), tick(100))
                .unwrap();
            let lease = request.lease().unwrap().clone();
            let _ = rows
                .settle_at(request.complete(Outcome::Acquired), tick(1))
                .unwrap();
            let revoke = rows.revoke(&lease).unwrap();
            for old in &deadlines {
                assert!(!LifecycleDeadline::matches_request(old, &revoke));
            }
            let deadline = LifecycleDeadline::from_request(&revoke);
            assert!(deadline.matches_request(&revoke));
            deadlines.push(deadline);
            let _ = rows
                .settle_at(
                    revoke.complete(Outcome::Revoked {
                        debt: WorkBrowserLeaseNativeDebt::default(),
                        resource_retained: true,
                    }),
                    tick(1),
                )
                .unwrap();
        }
        let destroy = rows.destroy(&resource).unwrap();
        assert!(deadlines
            .iter()
            .all(|deadline| !deadline.matches_request(&destroy)));
    }
    #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
    #[test]
    fn lifecycle_deadline_classification_uses_exact_operation_and_closed_gate_stage() {
        use crate::{WorkResourceDeadlineStage as Stage, WorkResourceFailureCause as Failure};

        assert_eq!(
            WorkNativeResource::deadline_failure_cause_for_gate(Operation::Construct, None),
            Failure::LifecycleDeadline(Stage::ConstructionNativeSetup)
        );
        let gate = crate::platform::work_document_navigation::WorkDocumentNavigation::default();
        assert_eq!(
            WorkNativeResource::deadline_failure_cause_for_gate(Operation::Construct, Some(&gate)),
            Failure::LifecycleDeadline(Stage::ConstructionBootstrap)
        );
        assert_eq!(
            WorkNativeResource::deadline_failure_cause_for_gate(Operation::Revoke, Some(&gate)),
            Failure::LifecycleDeadline(Stage::RevocationDrain)
        );
        assert_eq!(
            WorkNativeResource::deadline_failure_cause_for_gate(Operation::Destroy, Some(&gate)),
            Failure::LifecycleDeadline(Stage::DestructionDrain)
        );
        assert_eq!(
            WorkNativeResource::deadline_failure_cause_for_gate(Operation::Acquire, Some(&gate)),
            Failure::LifecycleDeadline(Stage::Unattributed)
        );
    }
    #[test]
    fn source_owner_retains_partial_construction_capacity_and_lease_retirement_never_cancels_document(
    ) {
        let source = include_str!("work_resource.rs");
        let build = source
            .split("fn build_work_resource(")
            .nth(1)
            .unwrap()
            .split("pub(crate) fn progress_work_resource")
            .next()
            .unwrap();
        assert!(
            build
                .find("reclassify(NativeResourceClass::AgentContext)")
                .unwrap()
                < build.find("build_owned_work_view(").unwrap()
        );
        assert!(build.contains("retirement_clean: false"));
        assert!(build.contains("return Ok(resource)"));
        let revoke = source
            .split("if let Some(task) = resource.revocation.as_ref()")
            .nth(1)
            .unwrap()
            .split("fn expire_work_resource")
            .next()
            .unwrap();
        for forbidden in [
            "stop_loading",
            ".cancel(",
            "prepare_semantic_document_load",
            "load_url(",
        ] {
            assert!(!revoke.contains(forbidden));
        }
        assert!(revoke.contains("guard.lease_drained(lease)"));
        assert!(revoke.contains("semantic_pending_for_audit() == Some(false)"));
    }
}
