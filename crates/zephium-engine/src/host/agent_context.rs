#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

//! Main-thread ownership for production agent-browser native contexts.
//!
//! This module is deliberately a separate identity island. An owned context
//! never enters ordinary tab, session, stage, or extension registries. The
//! shell-facing port carries only the closed `zephium-agentic` vocabulary;
//! this owner retains every native and profile obligation until exact close
//! or process shutdown.

#[cfg(target_os = "macos")]
mod favicon;
#[cfg(all(target_os = "macos", feature = "native-agentic-foreground-probe"))]
#[path = "agent_foreground_probe.rs"]
mod foreground_probe;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::Arc;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::time::{Duration, Instant};

use zephium_agentic::{
    ContextNativeEvent, ContextNativeResourceCounts, ContextNativeResourceSnapshot,
    ContextPortFailure, SemanticOrigin,
};

#[cfg(target_os = "windows")]
use zephium_agentic::{
    ContextCancellationSettlement, ContextCapabilities, ContextConstructionProof,
    ContextConstructionRequest, ContextConstructionSettlement, ContextConstructionSource,
    ContextCookieTransferDirection, ContextCookieTransferFailure, ContextCookieTransferId,
    ContextCookieTransferOutcome, ContextCookieTransferSettlement, ContextId, ContextJoin,
    ContextNativeRequest, ContextNavigationRedirectPolicy, ContextNavigationRequest,
    ContextNavigationSettlement, ContextNavigationTarget, ContextOperationJoin,
    ContextOperationKind, ContextOwnedViewport, ContextProfileLease, ContextProfileLeasePurpose,
    ContextProfileStorageClass, ContextTransitionRequest, ContextTransitionSettlement,
    MAX_LIVE_CONTEXTS, MAX_PENDING_COOKIE_TRANSFERS,
};
#[cfg(target_os = "macos")]
use zephium_agentic::{
    ContextCancellationSettlement, ContextCapabilities, ContextConstructionProof,
    ContextConstructionRequest, ContextConstructionSettlement, ContextConstructionSource,
    ContextId, ContextJoin, ContextNativeRequest, ContextNavigationRedirectPolicy,
    ContextNavigationRequest, ContextNavigationSettlement, ContextNavigationTarget,
    ContextOperationJoin, ContextOperationKind, ContextOwnedViewport, ContextProfileLease,
    ContextProfileLeasePurpose, ContextProfileStorageClass, ContextTransitionRequest,
    ContextTransitionSettlement, FrameId, SemanticActionNativeFailure, SemanticFrameTrust,
    SemanticInvocationId, SemanticRuntimePortFailure, SemanticRuntimeSettlement,
    SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure, SemanticScreenshotRequestId,
    SemanticSnapshotGeneration, MAX_LIVE_CONTEXTS,
};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use zephium_core::ports::engine::Partition;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::profiles::bind_profile_persistence_class;
#[cfg(target_os = "macos")]
use super::profiles::{
    profile_scoped_value, profile_value_is_isolated, MAX_PROFILE_PERSISTENCE_BINDINGS,
};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::resources::{NativeResourceClass, NativeResourceLease};
use super::EngineHost;
#[cfg(target_os = "macos")]
use crate::agent_context_port::AgentActionTask;
use crate::agent_context_port::AgentContextTask;
#[cfg(target_os = "macos")]
use crate::agent_context_port::AgentScreenshotTask;

#[cfg(target_os = "macos")]
const AGENT_PAGE_LOAD_COMMIT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(target_os = "windows")]
const AGENT_PAGE_LOAD_COMMIT_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(target_os = "windows")]
const AGENT_SUSPEND_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(target_os = "macos")]
const AGENT_SEMANTIC_RUNTIME_TIMEOUT: Duration = Duration::from_secs(15);

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentReplacementAdvance {
    #[cfg(target_os = "macos")]
    Direct,
    Navigation,
    Full,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentReplacementRejoin {
    NotPending,
    Ready,
    DeferredLocation,
    DeferredRendererLoss,
}

#[cfg(target_os = "macos")]
pub(super) struct AgentPendingScreenshot {
    id: SemanticScreenshotRequestId,
    context: ContextJoin,
    snapshot_generation: SemanticSnapshotGeneration,
    capture_deadline: Instant,
    pub(super) cancelled: Arc<AtomicBool>,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    task: AgentScreenshotTask,
}

#[cfg(target_os = "macos")]
impl AgentPendingScreenshot {
    fn complete(
        self,
        outcome: Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>,
    ) {
        if outcome.is_err() {
            self.cancelled.store(true, Ordering::Release);
        }
        let Self {
            id: _,
            context: _,
            snapshot_generation: _,
            capture_deadline: _,
            cancelled: _,
            watchdog,
            task,
        } = self;
        drop(watchdog);
        task.complete(outcome);
    }
}

#[cfg(target_os = "macos")]
struct AgentPendingNavigation {
    operation: ContextOperationJoin,
    target: ContextNavigationTarget,
    redirect_policy: Option<ContextNavigationRedirectPolicy>,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    task: AgentContextTask,
}

#[cfg(target_os = "macos")]
struct AgentPendingRecovery {
    operation: ContextOperationJoin,
    expected: Option<ContextNavigationTarget>,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    task: AgentContextTask,
}

#[cfg(target_os = "macos")]
struct AgentContextRetirement {
    navigation_clean: bool,
    content_policy_clean: bool,
    semantic_clean: bool,
    #[cfg(feature = "native-agentic-foreground-probe")]
    rendering_clean: bool,
}

#[cfg(target_os = "macos")]
impl AgentContextRetirement {
    const fn is_clean(&self) -> bool {
        #[cfg(feature = "native-agentic-foreground-probe")]
        if !self.rendering_clean {
            return false;
        }
        self.navigation_clean && self.content_policy_clean && self.semantic_clean
    }
}

#[cfg(target_os = "macos")]
impl AgentPendingNavigation {
    fn complete(self, outcome: Result<ContextNavigationTarget, ContextPortFailure>) {
        let Self {
            operation,
            target: _,
            redirect_policy: _,
            watchdog,
            task,
        } = self;
        drop(watchdog);
        match ContextNavigationSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::NavigationSettled(settlement)),
            Err(_) => task.refuse(ContextPortFailure::NativeRefused),
        }
    }
}

#[cfg(target_os = "macos")]
impl AgentPendingRecovery {
    fn complete(self, outcome: Result<(), ContextPortFailure>) {
        let Self {
            operation,
            expected: _,
            watchdog,
            task,
        } = self;
        drop(watchdog);
        match ContextTransitionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::TransitionSettled(settlement)),
            Err(_) => task.refuse(ContextPortFailure::NativeRefused),
        }
    }
}

/// Exact native owner for one run-owned macOS context.
///
/// Field order is part of teardown correctness: the policy registration is
/// retired before the page, and the page is destroyed before its capacity
/// lease can be reissued.
#[cfg(target_os = "macos")]
pub(super) struct AgentOwnedContext {
    join: ContextJoin,
    native_view_origin: ContextJoin,
    capabilities: ContextCapabilities,
    profile_lease: ContextProfileLease,
    committed_target: Option<ContextNavigationTarget>,
    pending_navigation: Option<AgentPendingNavigation>,
    pending_recovery: Option<AgentPendingRecovery>,
    pending_screenshot: Option<AgentPendingScreenshot>,
    renderer_lost: bool,
    renderer_loss_rejoin_pending: bool,
    renderer_loss_deferred_for_replacement: bool,
    navigation_replacement_rejoin_pending: bool,
    last_semantic_invocation: Option<SemanticInvocationId>,
    semantic_snapshot_generation: Option<SemanticSnapshotGeneration>,
    content_policy_registration: Option<crate::platform::imp::ContentPolicyRegistration>,
    event_emitter: crate::agent_context_port::AgentContextCallbackGuard,
    #[cfg(feature = "native-agentic-foreground-probe")]
    rendering_probe: Option<foreground_probe::AgentForegroundRendering>,
    #[cfg(feature = "native-agentic-foreground-probe")]
    rendering_probe_attempted: bool,
    favicon: Option<favicon::AgentFaviconWatch>,
    view: crate::platform::imp::AgentOwnedView,
    native_resource: Option<NativeResourceLease>,
}

#[cfg(target_os = "macos")]
impl AgentOwnedContext {
    fn new(
        join: ContextJoin,
        capabilities: ContextCapabilities,
        profile_lease: ContextProfileLease,
        content_policy_registration: crate::platform::imp::ContentPolicyRegistration,
        event_emitter: crate::agent_context_port::AgentContextCallbackGuard,
        view: crate::platform::imp::AgentOwnedView,
        native_resource: NativeResourceLease,
    ) -> Self {
        Self {
            join,
            native_view_origin: join,
            capabilities,
            profile_lease,
            committed_target: None,
            pending_navigation: None,
            pending_recovery: None,
            pending_screenshot: None,
            renderer_lost: false,
            renderer_loss_rejoin_pending: false,
            renderer_loss_deferred_for_replacement: false,
            navigation_replacement_rejoin_pending: false,
            last_semantic_invocation: None,
            semantic_snapshot_generation: None,
            content_policy_registration: Some(content_policy_registration),
            event_emitter,
            #[cfg(feature = "native-agentic-foreground-probe")]
            rendering_probe: None,
            #[cfg(feature = "native-agentic-foreground-probe")]
            rendering_probe_attempted: false,
            favicon: None,
            view,
            native_resource: Some(native_resource),
        }
    }

    pub(super) fn profile(&self) -> zephium_core::ids::ProfileId {
        self.join.identity().profile()
    }

    fn rejoin_navigation_replacement(
        &mut self,
        requested: ContextJoin,
        advance: AgentReplacementAdvance,
    ) -> Result<AgentReplacementRejoin, ContextPortFailure> {
        if !self.navigation_replacement_rejoin_pending {
            return Ok(AgentReplacementRejoin::NotPending);
        }
        if !replacement_rejoin_matches(self.join, requested, advance) {
            return Err(ContextPortFailure::Stale);
        }
        let deferred_location = self
            .view
            .navigation()
            .acknowledge_location_replacement()
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        self.join = requested;
        self.navigation_replacement_rejoin_pending = false;
        if self.renderer_lost {
            if !self.renderer_loss_deferred_for_replacement || deferred_location {
                return Err(ContextPortFailure::NativeRefused);
            }
            self.renderer_loss_deferred_for_replacement = false;
            self.renderer_loss_rejoin_pending = true;
            return Ok(AgentReplacementRejoin::DeferredRendererLoss);
        }
        Ok(if deferred_location {
            AgentReplacementRejoin::DeferredLocation
        } else {
            AgentReplacementRejoin::Ready
        })
    }

    pub(super) fn view(&self) -> &wry::WebView {
        self.view.view()
    }

    pub(super) fn replace_content_policy_registration(
        &mut self,
        registration: crate::platform::imp::ContentPolicyRegistration,
    ) -> Option<crate::platform::imp::ContentPolicyRegistration> {
        self.content_policy_registration.replace(registration)
    }

    fn pending_operation_for_audit(&self) -> Option<bool> {
        let lifecycle_pending = self.pending_navigation.is_some()
            || self.pending_recovery.is_some()
            || self.pending_screenshot.is_some();
        let semantic_pending = self.view.semantic_pending_for_audit()?;
        if lifecycle_pending && semantic_pending {
            None
        } else {
            Some(lifecycle_pending || semantic_pending)
        }
    }

    fn is_consistent_with_key(&self, id: ContextId) -> bool {
        let identity = self.join.identity();
        let pending_operation = self
            .pending_navigation
            .as_ref()
            .map(|pending| pending.operation)
            .or_else(|| {
                self.pending_recovery
                    .as_ref()
                    .map(|pending| pending.operation)
            });
        identity.id() == id
            && self.native_view_origin.identity() == identity
            && self.capabilities.kind() == identity.kind()
            && self.profile_lease.identity() == identity
            && self.profile_lease.purpose() == ContextProfileLeasePurpose::Owned
            && (!self.renderer_loss_rejoin_pending || self.renderer_lost)
            && (!self.renderer_loss_deferred_for_replacement
                || (self.renderer_lost
                    && self.navigation_replacement_rejoin_pending
                    && !self.renderer_loss_rejoin_pending))
            && !(self.renderer_loss_deferred_for_replacement && self.renderer_loss_rejoin_pending)
            && self
                .view
                .navigation()
                .location_state_for_audit()
                .is_some_and(|(callback, dirty, replacement)| {
                    replacement == self.navigation_replacement_rejoin_pending
                        && (!self.renderer_lost || (!callback && !dirty))
                })
            && (self.pending_navigation.is_none() || self.pending_recovery.is_none())
            && (self.pending_navigation.is_none() || self.pending_screenshot.is_none())
            && (self.pending_recovery.is_none() || self.pending_screenshot.is_none())
            && self
                .pending_navigation
                .as_ref()
                .is_none_or(|_| !self.renderer_lost)
            && self.pending_recovery.as_ref().is_none_or(|pending| {
                self.renderer_lost
                    && !self.renderer_loss_rejoin_pending
                    && pending.operation.context() == self.join
                    && pending.operation.kind() == ContextOperationKind::Recover
            })
            && self.view.navigation().matches_for_audit(
                pending_operation,
                self.renderer_lost && self.pending_recovery.is_none(),
            )
            && self.pending_navigation.as_ref().is_none_or(|pending| {
                pending.operation.context() == self.join
                    && pending.operation.kind() == ContextOperationKind::Navigate
            })
            && self.pending_screenshot.as_ref().is_none_or(|pending| {
                pending.context == self.join
                    && self.semantic_snapshot_generation == Some(pending.snapshot_generation)
                    && !self.renderer_lost
            })
            && self.pending_operation_for_audit().is_some()
            && self.semantic_snapshot_generation.is_none_or(|_| {
                self.committed_target.is_some()
                    && self.last_semantic_invocation.is_some()
                    && !self.renderer_lost
            })
            && self.native_resource.is_some()
            && self.content_policy_registration.is_some()
    }

    fn retire(mut self, pending_failure: ContextPortFailure) -> AgentContextRetirement {
        #[cfg(feature = "native-agentic-foreground-probe")]
        let rendering_clean = self.retire_foreground_probe();
        crate::platform::imp::stop_loading(self.view.view());
        let mut navigation_clean = true;
        if let Some(pending) = self.pending_navigation.take() {
            let disarmed = self.view.navigation().disarm(pending.operation);
            if disarmed {
                pending.complete(Err(pending_failure));
            } else {
                navigation_clean = false;
                pending.complete(Err(ContextPortFailure::NativeRefused));
            }
        }
        if let Some(pending) = self.pending_recovery.take() {
            let disarmed = self
                .view
                .navigation()
                .settle_recovery(pending.operation, false);
            if disarmed {
                pending.complete(Err(pending_failure));
            } else {
                navigation_clean = false;
                pending.complete(Err(ContextPortFailure::NativeRefused));
            }
        }
        if let Some(pending) = self.pending_screenshot.take() {
            pending.complete(Err(map_context_failure_to_screenshot(pending_failure)));
        }
        let content_policy_clean = self
            .content_policy_registration
            .take()
            .is_some_and(|registration| registration.retire().is_ok());
        let semantic_clean = self.view.retire_semantic_runtime();
        drop(self);
        AgentContextRetirement {
            navigation_clean,
            content_policy_clean,
            semantic_clean,
            #[cfg(feature = "native-agentic-foreground-probe")]
            rendering_clean,
        }
    }
}

#[cfg(target_os = "windows")]
struct AgentPendingNavigation {
    operation: ContextOperationJoin,
    target: ContextNavigationTarget,
    redirect_policy: Option<ContextNavigationRedirectPolicy>,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    task: AgentContextTask,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl AgentPendingNavigation {
    fn accepts_committed_target(&self, committed: &ContextNavigationTarget) -> bool {
        committed == &self.target
            || self
                .redirect_policy
                .as_ref()
                .is_some_and(|policy| policy.allows(committed))
    }
}

#[cfg(target_os = "windows")]
struct AgentPendingRecovery {
    operation: ContextOperationJoin,
    expected: Option<ContextNavigationTarget>,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    task: AgentContextTask,
}

#[cfg(target_os = "windows")]
struct AgentPendingSuspend {
    operation: ContextOperationJoin,
    claim: crate::platform::agent_suspension::AgentSuspendClaim,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    task: AgentContextTask,
}

#[cfg(target_os = "windows")]
pub(super) struct AgentPendingCookieTransfer {
    id: ContextCookieTransferId,
    destination: ContextId,
    destination_profile: zephium_core::ids::ProfileId,
    watchdog: crate::platform::imp::ContentPolicyTimeout,
    transfer: crate::platform::imp::WindowsAgentCookieTransfer,
    task: AgentContextTask,
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentNativeSuspendState {
    Active,
    Suspended,
    Uncertain(ContextOperationJoin),
}

#[cfg(target_os = "windows")]
struct AgentContextRetirement {
    navigation_clean: bool,
    content_policy_clean: bool,
    semantic_clean: bool,
    native_clean: bool,
}

#[cfg(target_os = "windows")]
impl AgentContextRetirement {
    const fn is_clean(&self) -> bool {
        self.navigation_clean
            && self.content_policy_clean
            && self.semantic_clean
            && self.native_clean
    }
}

#[cfg(target_os = "windows")]
impl AgentPendingNavigation {
    fn complete(self, outcome: Result<ContextNavigationTarget, ContextPortFailure>) {
        let Self {
            operation,
            target: _,
            redirect_policy: _,
            watchdog,
            task,
        } = self;
        drop(watchdog);
        match ContextNavigationSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::NavigationSettled(settlement)),
            Err(_) => task.refuse(ContextPortFailure::NativeRefused),
        }
    }
}

#[cfg(target_os = "windows")]
impl AgentPendingRecovery {
    fn complete(self, outcome: Result<(), ContextPortFailure>) {
        let Self {
            operation,
            expected: _,
            watchdog,
            task,
        } = self;
        drop(watchdog);
        match ContextTransitionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::TransitionSettled(settlement)),
            Err(_) => task.refuse(ContextPortFailure::NativeRefused),
        }
    }
}

#[cfg(target_os = "windows")]
impl AgentPendingSuspend {
    fn complete(self, outcome: Result<(), ContextPortFailure>) {
        let Self {
            operation,
            claim: _,
            watchdog,
            task,
        } = self;
        drop(watchdog);
        match ContextTransitionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::TransitionSettled(settlement)),
            Err(_) => task.refuse(ContextPortFailure::NativeRefused),
        }
    }
}

/// Exact native owner for one run-owned Windows automation surface.
///
/// The view never enters tab, stage, session, suspension, or extension maps.
/// Explicit WebView2 close runs before the accounting lease can be released;
/// a failed close transfers that exact lease into the host cleanup-debt band.
#[cfg(target_os = "windows")]
pub(super) struct AgentOwnedContext {
    join: ContextJoin,
    native_view_origin: ContextJoin,
    capabilities: ContextCapabilities,
    profile_lease: ContextProfileLease,
    committed_target: Option<ContextNavigationTarget>,
    pending_navigation: Option<AgentPendingNavigation>,
    pending_recovery: Option<AgentPendingRecovery>,
    pending_suspend: Option<AgentPendingSuspend>,
    pending_cookie_transfer: Option<ContextCookieTransferId>,
    cookie_contaminated: bool,
    late_suspend_claim: Option<(
        ContextOperationJoin,
        crate::platform::agent_suspension::AgentSuspendClaim,
    )>,
    suspend_state: AgentNativeSuspendState,
    renderer_lost: bool,
    renderer_loss_rejoin_pending: bool,
    renderer_loss_deferred_for_replacement: bool,
    navigation_replacement_rejoin_pending: bool,
    content_policy_registration: Option<crate::platform::imp::ContentPolicyRegistration>,
    event_emitter: crate::agent_context_port::AgentContextCallbackGuard,
    view: crate::platform::imp::AgentOwnedView,
    cleanup_profile: zephium_core::ids::ProfileId,
    native_close_attempted: bool,
    native_terminal_failure: Arc<dyn Fn(&'static str) + Send + Sync>,
    native_resource: Option<NativeResourceLease>,
}

#[cfg(target_os = "windows")]
impl AgentOwnedContext {
    #[cfg_attr(
        target_os = "windows",
        expect(
            clippy::too_many_arguments,
            reason = "Construction carries independent exact native identity, policy, cleanup and resource owners."
        )
    )]
    fn new(
        join: ContextJoin,
        capabilities: ContextCapabilities,
        profile_lease: ContextProfileLease,
        content_policy_registration: crate::platform::imp::ContentPolicyRegistration,
        event_emitter: crate::agent_context_port::AgentContextCallbackGuard,
        view: crate::platform::imp::AgentOwnedView,
        native_terminal_failure: Arc<dyn Fn(&'static str) + Send + Sync>,
        native_resource: NativeResourceLease,
    ) -> Self {
        Self {
            join,
            native_view_origin: join,
            capabilities,
            profile_lease,
            committed_target: None,
            pending_navigation: None,
            pending_recovery: None,
            pending_suspend: None,
            pending_cookie_transfer: None,
            cookie_contaminated: false,
            late_suspend_claim: None,
            suspend_state: AgentNativeSuspendState::Active,
            renderer_lost: false,
            renderer_loss_rejoin_pending: false,
            renderer_loss_deferred_for_replacement: false,
            navigation_replacement_rejoin_pending: false,
            content_policy_registration: Some(content_policy_registration),
            event_emitter,
            view,
            cleanup_profile: join.identity().profile(),
            native_close_attempted: false,
            native_terminal_failure,
            native_resource: Some(native_resource),
        }
    }

    pub(super) fn profile(&self) -> zephium_core::ids::ProfileId {
        self.join.identity().profile()
    }

    fn rejoin_navigation_replacement(
        &mut self,
        requested: ContextJoin,
        advance: AgentReplacementAdvance,
    ) -> Result<AgentReplacementRejoin, ContextPortFailure> {
        if !self.navigation_replacement_rejoin_pending {
            return Ok(AgentReplacementRejoin::NotPending);
        }
        if !replacement_rejoin_matches(self.join, requested, advance) {
            return Err(ContextPortFailure::Stale);
        }
        let deferred_location = self
            .view
            .navigation()
            .acknowledge_location_replacement()
            .map_err(|_| ContextPortFailure::NativeRefused)?;
        self.join = requested;
        self.navigation_replacement_rejoin_pending = false;
        if self.renderer_lost {
            if !self.renderer_loss_deferred_for_replacement || deferred_location {
                return Err(ContextPortFailure::NativeRefused);
            }
            self.renderer_loss_deferred_for_replacement = false;
            self.renderer_loss_rejoin_pending = true;
            return Ok(AgentReplacementRejoin::DeferredRendererLoss);
        }
        Ok(if deferred_location {
            AgentReplacementRejoin::DeferredLocation
        } else {
            AgentReplacementRejoin::Ready
        })
    }

    pub(super) fn view(&self) -> &wry::WebView {
        self.view.view()
    }

    pub(super) fn permits_content_policy_install(&self) -> bool {
        !self.renderer_lost
            && self.pending_suspend.is_none()
            && self.pending_cookie_transfer.is_none()
            && !self.cookie_contaminated
            && self.late_suspend_claim.is_none()
            && self.suspend_state == AgentNativeSuspendState::Active
    }

    pub(super) fn replace_content_policy_registration(
        &mut self,
        registration: crate::platform::imp::ContentPolicyRegistration,
    ) -> Option<crate::platform::imp::ContentPolicyRegistration> {
        self.content_policy_registration.replace(registration)
    }

    fn pending_operation_for_audit(&self) -> Option<bool> {
        if matches!(self.suspend_state, AgentNativeSuspendState::Uncertain(_)) {
            return None;
        }
        let lifecycle_pending = self.pending_navigation.is_some()
            || self.pending_recovery.is_some()
            || self.pending_suspend.is_some()
            || self.pending_cookie_transfer.is_some();
        let semantic_pending = self.view.semantic_pending_for_audit()?;
        let lifecycle_count = usize::from(self.pending_navigation.is_some())
            + usize::from(self.pending_recovery.is_some())
            + usize::from(self.pending_suspend.is_some())
            + usize::from(self.pending_cookie_transfer.is_some());
        if lifecycle_count > 1 || (lifecycle_pending && semantic_pending) {
            None
        } else {
            Some(lifecycle_pending || semantic_pending)
        }
    }

    fn suspended_for_audit(&self) -> Option<bool> {
        if self.pending_suspend.is_some() || self.renderer_lost {
            return self.renderer_lost.then_some(false);
        }
        match self.suspend_state {
            AgentNativeSuspendState::Active => Some(false),
            AgentNativeSuspendState::Suspended => Some(true),
            AgentNativeSuspendState::Uncertain(_) => None,
        }
    }

    fn is_consistent_with_key(&self, id: ContextId) -> bool {
        let identity = self.join.identity();
        let pending_operation = self
            .pending_navigation
            .as_ref()
            .map(|pending| pending.operation)
            .or_else(|| {
                self.pending_recovery
                    .as_ref()
                    .map(|pending| pending.operation)
            });
        identity.id() == id
            && self.native_view_origin.identity() == identity
            && self.capabilities.kind() == identity.kind()
            && self.profile_lease.identity() == identity
            && self.profile_lease.purpose() == ContextProfileLeasePurpose::Owned
            && (!self.renderer_loss_rejoin_pending || self.renderer_lost)
            && (!self.renderer_loss_deferred_for_replacement
                || (self.renderer_lost
                    && self.navigation_replacement_rejoin_pending
                    && !self.renderer_loss_rejoin_pending))
            && !(self.renderer_loss_deferred_for_replacement && self.renderer_loss_rejoin_pending)
            && self
                .view
                .navigation()
                .location_state_for_audit()
                .is_some_and(|(callback, dirty, replacement)| {
                    replacement == self.navigation_replacement_rejoin_pending
                        && (!self.renderer_lost || (!callback && !dirty))
                })
            && (self.pending_navigation.is_none() || self.pending_recovery.is_none())
            && (self.pending_navigation.is_none() || self.pending_suspend.is_none())
            && (self.pending_recovery.is_none() || self.pending_suspend.is_none())
            && (self.pending_cookie_transfer.is_none()
                || (self.pending_navigation.is_none()
                    && self.pending_recovery.is_none()
                    && self.pending_suspend.is_none()
                    && self.committed_target.is_none()
                    && !self.cookie_contaminated
                    && !self.renderer_lost
                    && self.suspend_state == AgentNativeSuspendState::Active))
            && (!self.cookie_contaminated || self.pending_cookie_transfer.is_none())
            && self
                .pending_suspend
                .as_ref()
                .is_none_or(|_| self.late_suspend_claim.is_none())
            && self
                .late_suspend_claim
                .as_ref()
                .is_none_or(|(operation, _)| {
                    matches!(
                        self.suspend_state,
                        AgentNativeSuspendState::Uncertain(expected) if expected == *operation
                    )
                })
            && (!matches!(
                self.suspend_state,
                AgentNativeSuspendState::Active | AgentNativeSuspendState::Suspended
            ) || self.late_suspend_claim.is_none())
            && (!self.renderer_lost
                || (self.pending_suspend.is_none()
                    && self.suspend_state == AgentNativeSuspendState::Active))
            && self.pending_navigation.as_ref().is_none_or(|_| {
                !self.renderer_lost && self.suspend_state == AgentNativeSuspendState::Active
            })
            && self.pending_recovery.as_ref().is_none_or(|pending| {
                self.renderer_lost
                    && !self.renderer_loss_rejoin_pending
                    && pending.operation.context() == self.join
                    && pending.operation.kind() == ContextOperationKind::Recover
            })
            && self.view.navigation().matches_for_audit(
                pending_operation,
                self.renderer_lost && self.pending_recovery.is_none(),
            )
            && self.pending_navigation.as_ref().is_none_or(|pending| {
                pending.operation.context() == self.join
                    && pending.operation.kind() == ContextOperationKind::Navigate
            })
            && self.pending_suspend.as_ref().is_none_or(|pending| {
                pending.operation.context() == self.join
                    && pending.operation.kind() == ContextOperationKind::Suspend
                    && self.suspend_state == AgentNativeSuspendState::Active
            })
            && self.pending_operation_for_audit().is_some()
            && !self.native_close_attempted
            && self.native_resource.is_some()
            && self.content_policy_registration.is_some()
    }

    fn close_native(&mut self) -> bool {
        if self.native_close_attempted {
            return false;
        }
        self.native_close_attempted = true;
        match self.view.close() {
            Ok(()) => true,
            Err(debt) => {
                let debt = super::OwnedWindowsCleanupDebt::new(debt, self.native_resource.take());
                if !debt.accounted_as_debt() {
                    (self.native_terminal_failure)(
                        "Windows agent cleanup debt exceeded native resource accounting",
                    );
                }
                super::dispatch::queue_windows_cleanup_debt(self.cleanup_profile, debt);
                false
            }
        }
    }

    fn retire(mut self, pending_failure: ContextPortFailure) -> AgentContextRetirement {
        if self.pending_cookie_transfer.take().is_some() {
            (self.native_terminal_failure)(
                "Windows agent context retired before its cookie transfer terminal",
            );
        }
        crate::platform::imp::stop_loading(self.view.view());
        let mut navigation_clean = true;
        if let Some(pending) = self.pending_navigation.take() {
            let disarmed = self.view.navigation().disarm(pending.operation);
            navigation_clean &= disarmed;
            pending.complete(Err(if disarmed {
                pending_failure
            } else {
                ContextPortFailure::NativeRefused
            }));
        }
        if let Some(pending) = self.pending_recovery.take() {
            let disarmed = self
                .view
                .navigation()
                .settle_recovery(pending.operation, false);
            navigation_clean &= disarmed;
            pending.complete(Err(if disarmed {
                pending_failure
            } else {
                ContextPortFailure::NativeRefused
            }));
        }
        if let Some(pending) = self.pending_suspend.take() {
            pending.claim.retire();
            pending.complete(Err(pending_failure));
        }
        if let Some((_, claim)) = self.late_suspend_claim.take() {
            claim.retire();
        }
        let content_policy_clean = self
            .content_policy_registration
            .take()
            .is_some_and(|registration| registration.retire().is_ok());
        let semantic_clean = self.view.retire_semantic_runtime();
        let native_clean = self.close_native();
        AgentContextRetirement {
            navigation_clean,
            content_policy_clean,
            semantic_clean,
            native_clean,
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for AgentOwnedContext {
    fn drop(&mut self) {
        if self.pending_cookie_transfer.is_some() {
            (self.native_terminal_failure)(
                "Windows agent cookie transfer escaped explicit host retirement",
            );
        }
        if let Some(pending) = self.pending_suspend.as_ref() {
            pending.claim.retire();
        }
        if let Some((_, claim)) = self.late_suspend_claim.as_ref() {
            claim.retire();
        }
        let policy_cleanup_failed = self
            .content_policy_registration
            .take()
            .is_some_and(|registration| registration.retire().is_err());
        if policy_cleanup_failed {
            (self.native_terminal_failure)(
                "Windows agent content-policy registration escaped explicit retirement",
            );
        }
        if self.view.semantic_is_live() && !self.view.retire_semantic_runtime() {
            (self.native_terminal_failure)(
                "Windows agent semantic runtime escaped explicit retirement",
            );
        }
        let _ = self.close_native();
    }
}

impl EngineHost {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn admit_navigation_replacement_rejoin(
        &mut self,
        task: AgentContextTask,
        requested: ContextJoin,
        advance: AgentReplacementAdvance,
        close_terminal: bool,
    ) -> Option<(AgentContextTask, bool)> {
        let id = requested.identity().id();
        let outcome = self
            .agent_contexts
            .get_mut(&id)
            .ok_or(ContextPortFailure::Stale)
            .and_then(|binding| binding.rejoin_navigation_replacement(requested, advance));
        match outcome {
            Ok(AgentReplacementRejoin::NotPending) => Some((task, false)),
            Ok(AgentReplacementRejoin::Ready) => Some((task, true)),
            Ok(AgentReplacementRejoin::DeferredLocation) => {
                task.refuse(ContextPortFailure::Stale);
                self.retry_deferred_owned_agent_location_check(id);
                None
            }
            Ok(AgentReplacementRejoin::DeferredRendererLoss) => {
                if close_terminal {
                    return Some((task, true));
                }
                let emitter = self
                    .agent_contexts
                    .get(&id)
                    .map(|binding| binding.event_emitter.clone());
                task.refuse(ContextPortFailure::Stale);
                if let Some(emitter) = emitter {
                    emitter.emit_renderer_lost(requested);
                } else {
                    self.fail_agent_context_invariant(
                        "agent-context deferred renderer loss lost its native owner",
                    );
                }
                None
            }
            Err(failure) => {
                if failure == ContextPortFailure::NativeRefused {
                    self.fail_agent_context_invariant(
                        "agent-context replacement rejoin contradicted native observer state",
                    );
                }
                task.refuse(failure);
                None
            }
        }
    }

    pub(crate) fn handle_agent_context_task(&mut self, task: AgentContextTask) {
        if let Some(audit) = task.audit() {
            self.settle_agent_context_audit(task, audit);
            return;
        }
        #[cfg(target_os = "windows")]
        if task.cookie().is_some() {
            self.start_windows_agent_cookie_transfer(task);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        if task.cookie().is_some() {
            task.refuse(ContextPortFailure::Unsupported);
            return;
        }
        #[cfg(target_os = "macos")]
        if task.is_semantic() {
            self.start_owned_agent_semantic_invocation(task);
            return;
        }
        #[cfg(not(target_os = "macos"))]
        if task.is_semantic() {
            task.refuse(ContextPortFailure::Unsupported);
            return;
        }

        let Some(request) = task.request().cloned() else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        #[cfg(all(target_os = "macos", feature = "native-agentic-foreground-probe"))]
        if !self.foreground_probe_allows_lifecycle(&request) {
            task.refuse(ContextPortFailure::Unsupported);
            return;
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        match request {
            ContextNativeRequest::Construct(request) => {
                let operation = request.operation();
                let outcome = self.construct_owned_agent_context(request, task.callback_guard());
                self.complete_agent_construction(task, operation, outcome);
            }
            ContextNativeRequest::Navigate(request) => {
                self.start_owned_agent_navigation(task, request);
            }
            ContextNativeRequest::Transition(request)
                if request.operation().kind() == ContextOperationKind::Recover =>
            {
                self.start_owned_agent_recovery(task, request);
            }
            #[cfg(target_os = "windows")]
            ContextNativeRequest::Transition(request)
                if request.operation().kind() == ContextOperationKind::Suspend =>
            {
                self.start_owned_agent_suspend(task, request);
            }
            #[cfg(target_os = "windows")]
            ContextNativeRequest::Transition(request)
                if request.operation().kind() == ContextOperationKind::Resume =>
            {
                self.resume_owned_agent_context(task, request);
            }
            ContextNativeRequest::Transition(request)
                if request.operation().kind() == ContextOperationKind::Close =>
            {
                self.close_owned_agent_context(task, request);
            }
            ContextNativeRequest::Cancel(request) => {
                let current = request.current();
                let Some((task, replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
                    task,
                    current,
                    AgentReplacementAdvance::Full,
                    false,
                ) else {
                    return;
                };
                let outcome = self.cancel_owned_agent_context(current, replacement_rejoined);
                task.complete(ContextNativeEvent::CancellationSettled(
                    ContextCancellationSettlement::new(current, outcome),
                ));
            }
            ContextNativeRequest::Transition(_) => {
                task.refuse(ContextPortFailure::Unsupported);
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = request;
            task.refuse(ContextPortFailure::Unsupported);
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn handle_agent_screenshot_task(&mut self, task: AgentScreenshotTask) {
        let retained = task.request().is_some_and(|request| {
            self.work_resources
                .contains_key(&request.context().identity().id())
        });
        if retained {
            self.start_work_resource_screenshot(task);
            return;
        }
        self.start_owned_agent_screenshot(task);
    }

    #[cfg(target_os = "macos")]
    fn start_work_resource_screenshot(&mut self, mut task: AgentScreenshotTask) {
        let Some(request) = task.request() else {
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let context = request.context();
        let id = context.identity().id();
        let request_id = request.id();
        let snapshot_generation = request.snapshot_generation();
        let admitted_at = task.admitted_at();
        let Some(capture_window_millis) = request
            .deadline()
            .millis()
            .checked_sub(request.requested_at().millis())
        else {
            task.refuse(SemanticScreenshotNativeFailure::TimedOut);
            return;
        };
        let capture_window = Duration::from_millis(capture_window_millis);
        let elapsed = Instant::now().saturating_duration_since(admitted_at);
        if capture_window.is_zero() || elapsed >= capture_window {
            task.refuse(SemanticScreenshotNativeFailure::TimedOut);
            return;
        }

        let failure = match self.work_resources.get(&id) {
            None => Some(SemanticScreenshotNativeFailure::Stale),
            Some(resource)
                if resource.guard.resource().identity().context() != context.identity().id() =>
            {
                Some(SemanticScreenshotNativeFailure::Stale)
            }
            Some(resource) if !resource.guard.is_healthy() || !resource.ready() => {
                Some(SemanticScreenshotNativeFailure::Shutdown)
            }
            Some(resource) if resource.pending() => {
                Some(SemanticScreenshotNativeFailure::ResourceExhausted)
            }
            Some(resource) if snapshot_generation.get() != resource.last_invocation => {
                Some(SemanticScreenshotNativeFailure::Stale)
            }
            Some(resource)
                if resource
                    .view
                    .as_ref()
                    .is_none_or(|view| view.semantic_pending_for_audit() != Some(false)) =>
            {
                Some(SemanticScreenshotNativeFailure::NotReady)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }

        let callback_guard = task.callback_guard();
        let timeout_guard = callback_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            capture_window.saturating_sub(elapsed),
            move || {
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_work_resource_screenshot(
                        id,
                        request_id,
                        Err(SemanticScreenshotNativeFailure::TimedOut),
                    );
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let Some(request) = task.take_request() else {
            drop(watchdog);
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let Some(physical) = task.take_physical() else {
            drop(watchdog);
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let pending = AgentPendingScreenshot {
            id: request_id,
            context,
            snapshot_generation,
            capture_deadline: admitted_at
                .checked_add(capture_window)
                .unwrap_or(admitted_at),
            cancelled: cancelled.clone(),
            watchdog,
            task,
        };
        let Some(resource) = self.work_resources.get_mut(&id) else {
            pending.complete(Err(SemanticScreenshotNativeFailure::Stale));
            return;
        };
        let Some(view) = resource.view.as_ref() else {
            pending.complete(Err(SemanticScreenshotNativeFailure::Shutdown));
            return;
        };
        let Some(document_gate) = view.work_navigation().cloned() else {
            pending.complete(Err(SemanticScreenshotNativeFailure::Stale));
            return;
        };
        let Some(document) = document_gate.observation_stamp(context) else {
            pending.complete(Err(SemanticScreenshotNativeFailure::Stale));
            return;
        };
        resource.screenshot = Some(pending);

        let native_guard = callback_guard.clone();
        let panic_guard = callback_guard.clone();
        let dispatched = view.dispatch_screenshot(
            request,
            admitted_at,
            cancelled,
            move |outcome| {
                drop(physical);
                let outcome = if document_gate.observation_stamp(context) == Some(document) {
                    outcome
                } else {
                    Err(SemanticScreenshotNativeFailure::Stale)
                };
                let rejected = native_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_work_resource_screenshot(id, request_id, outcome);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
            move || panic_guard.callback_dispatch_rejected(),
        );
        if let Err(failure) = dispatched {
            self.finish_work_resource_screenshot(id, request_id, Err(failure));
        }
    }

    #[cfg(target_os = "macos")]
    fn finish_work_resource_screenshot(
        &mut self,
        id: ContextId,
        request_id: SemanticScreenshotRequestId,
        outcome: Result<
            crate::platform::semantic_pixel_privacy::ModelCapture<SemanticScreenshotNativeCapture>,
            SemanticScreenshotNativeFailure,
        >,
    ) {
        let Some(pending) = self.work_resources.get_mut(&id).and_then(|resource| {
            (resource
                .screenshot
                .as_ref()
                .is_some_and(|pending| pending.id == request_id))
            .then(|| resource.screenshot.take())
            .flatten()
        }) else {
            return;
        };
        let outcome = outcome.and_then(|proof| {
            let resource = self
                .work_resources
                .get(&id)
                .ok_or(SemanticScreenshotNativeFailure::Stale)?;
            resource.guard.record_model_capture(
                request_id,
                pending.context,
                pending.capture_deadline,
                proof,
            )
        });
        pending.complete(outcome);
        if let Some(resource) = self.work_resources.get(&id) {
            let guard = resource.guard.clone();
            self.progress_work_resource(&guard);
        }
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn handle_agent_action_task(&mut self, task: AgentActionTask) {
        self.start_owned_agent_action(task);
    }

    fn settle_agent_context_audit(
        &mut self,
        task: AgentContextTask,
        _audit: zephium_agentic::ContextResourceAuditId,
    ) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let binding_count = u8::try_from(self.agent_contexts.len()).ok();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let binding_count = binding_count
            .and_then(|count| usize::from(count).checked_add(self.work_resources.len()))
            .and_then(|count| u8::try_from(count).ok());
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let binding_count = Some(0);

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let resident_view_count = u8::try_from(
            self.agent_contexts
                .values()
                .filter(|binding| !binding.renderer_lost)
                .count(),
        )
        .ok();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let resident_view_count = resident_view_count
            .and_then(|count| {
                usize::from(count).checked_add(
                    self.work_resources
                        .values()
                        .filter(|resource| resource.resident())
                        .count(),
                )
            })
            .and_then(|count| u8::try_from(count).ok());
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let resident_view_count = Some(0);

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let pending_operations = self
            .agent_contexts
            .values()
            .try_fold(0usize, |count, binding| {
                let pending = binding.pending_operation_for_audit()?;
                count.checked_add(usize::from(pending))
            })
            .and_then(|count| u8::try_from(count).ok());
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let pending_operations = pending_operations
            .and_then(|count| {
                usize::from(count).checked_add(
                    self.work_resources
                        .values()
                        .filter(|resource| resource.pending())
                        .count(),
                )
            })
            .and_then(|count| u8::try_from(count).ok());
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let pending_operations = Some(0);

        #[cfg(target_os = "windows")]
        let suspended_view_count = self
            .agent_contexts
            .values()
            .try_fold(0usize, |count, binding| {
                let suspended = binding.suspended_for_audit()?;
                count.checked_add(usize::from(suspended))
            })
            .and_then(|count| u8::try_from(count).ok());
        #[cfg(not(target_os = "windows"))]
        let suspended_view_count = Some(0);

        let admission_counts = task.admission_counts();
        #[cfg(all(target_os = "macos", feature = "native-agentic-foreground-probe"))]
        let visible_surfaces = self
            .agent_contexts
            .values()
            .filter(|binding| binding.foreground_probe_visible())
            .count();
        #[cfg(all(target_os = "macos", feature = "native-agentic-work-resource-probe"))]
        let visible_surfaces = visible_surfaces
            + self
                .work_resources
                .values()
                .filter(|resource| resource.witness_visible())
                .count();
        #[cfg(not(all(target_os = "macos", feature = "native-agentic-foreground-probe")))]
        let visible_surfaces = 0usize;
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let visible_surfaces = visible_surfaces
            + self
                .work_resources
                .values()
                .filter(|resource| resource.observation_visible())
                .count();
        let queued_request_tasks = admission_counts
            .map(|(pending, _)| pending)
            .and_then(|pending| pending.checked_sub(1))
            .zip(pending_operations)
            .and_then(|(pending, operations)| pending.checked_sub(usize::from(operations)))
            .and_then(|pending| u8::try_from(pending).ok());
        let pending_captures =
            admission_counts.and_then(|(_, captures)| u8::try_from(captures).ok());
        let queued_tasks = queued_request_tasks
            .zip(crate::host::agent_context_terminal_depth_for_audit())
            .and_then(|(requests, terminals)| {
                usize::from(requests)
                    .checked_add(terminals)
                    .and_then(|queued| u8::try_from(queued).ok())
            });

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let bindings_consistent = self
            .agent_contexts
            .iter()
            .all(|(id, binding)| binding.is_consistent_with_key(*id));
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let bindings_consistent = bindings_consistent
            && self
                .work_resources
                .iter()
                .all(|(id, resource)| resource.consistent(*id))
            && task.work_ingress_matches(
                self.work_resources
                    .values()
                    .map(|resource| resource.guard())
                    .collect(),
            );
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let bindings_consistent = true;

        #[cfg(target_os = "windows")]
        let cookie_bindings_consistent = self.agent_cookie_transfers.len()
            <= MAX_PENDING_COOKIE_TRANSFERS
            && self.agent_cookie_transfers.iter().all(|(id, pending)| {
                pending.id == *id
                    && pending.task.cookie().is_some_and(|(request, _)| {
                        request.id() == *id
                            && request.destination().identity().id() == pending.destination
                            && request.destination().identity().profile()
                                == pending.destination_profile
                    })
                    && self
                        .agent_contexts
                        .get(&pending.destination)
                        .is_some_and(|binding| {
                            binding.pending_cookie_transfer == Some(*id)
                                && binding.profile() == pending.destination_profile
                        })
            })
            && self
                .agent_contexts
                .values()
                .filter(|binding| binding.pending_cookie_transfer.is_some())
                .count()
                == self.agent_cookie_transfers.len()
            && self
                .agent_cookie_transfers
                .values()
                .enumerate()
                .all(|(index, pending)| {
                    self.agent_cookie_transfers
                        .values()
                        .skip(index + 1)
                        .all(|candidate| {
                            candidate.destination_profile != pending.destination_profile
                        })
                });
        #[cfg(not(target_os = "windows"))]
        let cookie_bindings_consistent = true;

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let resource_count_matches = binding_count.is_some_and(|binding_count| {
            self.native_resources
                .count_for_audit(NativeResourceClass::AgentContext)
                == Some(usize::from(binding_count))
        });
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let resource_count_matches = true;

        let outcome = match (
            binding_count,
            resident_view_count,
            pending_operations,
            suspended_view_count,
            pending_captures,
            queued_tasks,
        ) {
            (
                Some(binding_count),
                Some(resident_view_count),
                Some(pending_operations),
                Some(suspended_view_count),
                Some(pending_captures),
                Some(queued_tasks),
            ) if !self.native_resource_accounting_failed
                && self.native_resources.is_healthy()
                && bindings_consistent
                && cookie_bindings_consistent
                && resource_count_matches =>
            {
                ContextNativeResourceSnapshot::try_new(ContextNativeResourceCounts {
                    known_bindings: binding_count,
                    resident_views: resident_view_count,
                    owned_reservations: binding_count,
                    borrowed_leases: 0,
                    visible_surfaces: u8::try_from(visible_surfaces)
                        .map_err(|_| ContextPortFailure::NativeRefused)
                        .unwrap_or(u8::MAX),
                    suspended_views: suspended_view_count,
                    pending_operations,
                    pending_captures,
                    queued_tasks,
                })
                .map_err(|_| ContextPortFailure::NativeRefused)
            }
            _ => Err(ContextPortFailure::NativeRefused),
        };
        task.complete_audit(outcome);
    }

    #[cfg(target_os = "macos")]
    fn construct_owned_agent_context(
        &mut self,
        request: ContextConstructionRequest,
        callback_guard: crate::agent_context_port::AgentContextCallbackGuard,
    ) -> Result<ContextConstructionProof, ContextPortFailure> {
        if request.source() != ContextConstructionSource::Owned
            || request.profile_lease().purpose() != ContextProfileLeasePurpose::Owned
        {
            return Err(ContextPortFailure::Unsupported);
        }
        let viewport = request
            .owned_viewport()
            .filter(|viewport| *viewport == ContextOwnedViewport::STANDARD)
            .ok_or(ContextPortFailure::NativeRefused)?;
        let join = request.operation().context();
        let identity = join.identity();
        let id = identity.id();
        let profile = identity.profile();
        if self.agent_contexts.contains_key(&id) || self.work_resources.contains_key(&id) {
            return Err(ContextPortFailure::Stale);
        }
        if self.agent_contexts.len() + self.work_resources.len() >= MAX_LIVE_CONTEXTS
            || (!self.work_resources.is_empty()
                && self.agent_contexts.len() + self.work_execution_reservations()
                    >= zephium_agentic::MAX_EXECUTING_CONTEXTS)
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if self.erasure_tombstones.contains(&profile) {
            return Err(ContextPortFailure::ProfileUnavailable);
        }

        let partition = match request.profile_lease().storage_class() {
            ContextProfileStorageClass::Durable => Partition::Persistent(profile),
            ContextProfileStorageClass::Ephemeral => Partition::Ephemeral(profile),
        };
        if !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition) {
            return Err(ContextPortFailure::ProfileUnavailable);
        }
        let content_policy = self
            .applied_content_policy(profile)
            .ok_or(ContextPortFailure::ProfileUnavailable)?;
        if self.native_resource_accounting_failed || !self.native_resources.is_healthy() {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if self
            .native_resources
            .count_for_audit(NativeResourceClass::AgentContext)
            .is_some_and(|count| count >= NativeResourceClass::AgentContext.limit())
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        let mut native_resource = self
            .native_resources
            .try_acquire(NativeResourceClass::TransientConstruction)
            .map_err(|error| self.map_agent_resource_failure(error))?;

        let ephemeral_store = match request.profile_lease().storage_class() {
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
                    self.macos_ephemeral_data_stores.remove(&profile);
                    return Err(ContextPortFailure::ProfileUnavailable);
                }
                Some(store)
            }
        };

        let view_origin = join;
        let navigation_guard = callback_guard.clone();
        let location_guard = callback_guard.clone();
        let renderer_guard = callback_guard.clone();
        let invariant_guard = callback_guard.clone();
        let panic_guard = callback_guard.clone();
        let view = crate::platform::imp::build_owned_agent_view(
            &self.parent,
            viewport,
            profile,
            request.profile_lease().storage_class(),
            ephemeral_store.as_ref(),
            crate::platform::imp::AgentOwnedViewCallbacks::new(
                move |terminal| {
                    let rejected = navigation_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_navigation_terminal(id, terminal);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || {
                    let rejected = location_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_location_check(id, view_origin);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || {
                    let emitter = renderer_guard.clone();
                    let rejected = renderer_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_renderer_lost(id, view_origin, emitter);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || invariant_guard.callback_dispatch_rejected(),
                move || panic_guard.callback_dispatch_rejected(),
            ),
        )
        .map_err(map_owned_view_construction_failure)?;
        let content_policy_registration =
            crate::platform::imp::install_content_policy_on_view(view.view(), &content_policy)
                .map_err(|failure| {
                    if failure == zephium_core::blocker::ContentRuleApplyFailure::NativeCleanup {
                        self.fail_content_policy_retirement();
                    }
                    ContextPortFailure::NativeRefused
                })?;
        if let Err(error) = native_resource.reclassify(NativeResourceClass::AgentContext) {
            if content_policy_registration.retire().is_err() {
                self.fail_content_policy_retirement();
            }
            drop(view);
            return Err(self.map_agent_resource_failure(error));
        }

        let binding = AgentOwnedContext::new(
            join,
            request.capabilities(),
            request.profile_lease(),
            content_policy_registration,
            callback_guard,
            view,
            native_resource,
        );
        match self.agent_contexts.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(binding);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                let retirement = binding.retire(ContextPortFailure::NativeRefused);
                self.record_agent_context_retirement(retirement);
                self.fail_agent_context_invariant(
                    "agent-context identity became occupied during native construction",
                );
                return Err(ContextPortFailure::NativeRefused);
            }
        }
        Ok(ContextConstructionProof::MacOsOwnedSelectedProfileExtensionFree)
    }

    #[cfg(target_os = "macos")]
    fn start_owned_agent_semantic_invocation(&mut self, mut task: AgentContextTask) {
        let Some(invocation) = task.take_semantic_invocation() else {
            self.fail_agent_context_invariant(
                "agent-context semantic task lost its exact invocation",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        let correlation = invocation.correlation();
        if task.semantic_correlation() != Some(&correlation) {
            self.fail_agent_context_invariant(
                "agent-context semantic task correlation changed during admission",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }

        let context = invocation.frame().context();
        let id = context.identity().id();
        let Some((task, _replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
            task,
            context,
            AgentReplacementAdvance::Direct,
            false,
        ) else {
            return;
        };
        let failure = match self.agent_contexts.get(&id) {
            None => Some(SemanticRuntimePortFailure::Stale),
            #[cfg(feature = "native-agentic-foreground-probe")]
            Some(binding) if !binding.foreground_probe_semantic_ready(context) => {
                Some(SemanticRuntimePortFailure::NotReady)
            }
            Some(binding) if binding.join != context => Some(SemanticRuntimePortFailure::Stale),
            Some(binding) if binding.renderer_lost => {
                Some(SemanticRuntimePortFailure::RendererLost)
            }
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_screenshot.is_some() =>
            {
                Some(SemanticRuntimePortFailure::NotReady)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Observe) =>
            {
                Some(SemanticRuntimePortFailure::Unsupported)
            }
            Some(_)
                if invocation.frame().frame() != FrameId::MAIN
                    || invocation.frame().trust() != SemanticFrameTrust::SameOrigin =>
            {
                Some(SemanticRuntimePortFailure::Unsupported)
            }
            Some(binding) => {
                let expected_origin = binding
                    .committed_target
                    .as_ref()
                    .and_then(|target| SemanticOrigin::parse(target.as_url().as_str()).ok());
                let expected_generation = binding
                    .semantic_snapshot_generation
                    .map_or(Some(SemanticSnapshotGeneration::INITIAL), |generation| {
                        generation.next()
                    });
                if expected_origin.as_ref() != Some(invocation.frame().origin())
                    || expected_generation != Some(invocation.snapshot_generation())
                    || binding
                        .last_semantic_invocation
                        .is_some_and(|last| invocation.invocation().get() <= last.get())
                {
                    Some(SemanticRuntimePortFailure::Stale)
                } else if !binding.view.navigation().location_stable_for_result() {
                    // Committed navigation is not native load completion. Do
                    // not run a semantic request before the exact location
                    // observer has drained; no snapshot generation is spent.
                    Some(SemanticRuntimePortFailure::NotReady)
                } else {
                    None
                }
            }
        };
        if let Some(failure) = failure {
            let settlement = SemanticRuntimeSettlement::try_new(correlation, Err(failure));
            match settlement {
                Ok(settlement) => task.complete(ContextNativeEvent::SemanticRuntimeSettled(
                    Box::new(settlement),
                )),
                Err(_) => {
                    self.fail_agent_context_invariant(
                        "agent-context semantic refusal violated correlation",
                    );
                    task.refuse(ContextPortFailure::NativeRefused);
                }
            }
            return;
        }

        let invocation_id = invocation.invocation();
        let snapshot_generation = invocation.snapshot_generation();
        let timeout_guard = task.callback_guard();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            AGENT_SEMANTIC_RUNTIME_TIMEOUT,
            move || {
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.timeout_owned_agent_semantic_invocation(id, invocation_id);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            match SemanticRuntimeSettlement::try_new(
                correlation,
                Err(SemanticRuntimePortFailure::Transport),
            ) {
                Ok(settlement) => task.complete(ContextNativeEvent::SemanticRuntimeSettled(
                    Box::new(settlement),
                )),
                Err(_) => {
                    self.fail_agent_context_invariant(
                        "agent-context semantic timeout lost exact correlation",
                    );
                    task.refuse(ContextPortFailure::NativeRefused);
                }
            }
            return;
        };
        let callback_guard = task.callback_guard();
        let callback_correlation = correlation.clone();
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        #[cfg(feature = "native-agentic-foreground-probe")]
        if !binding.admit_foreground_probe_semantic(&invocation) {
            task.refuse(ContextPortFailure::ResourceExhausted);
            return;
        }
        let result_navigation = binding.view.navigation().clone();
        #[cfg(feature = "native-agentic-foreground-probe")]
        let result_rendering = binding
            .rendering_probe
            .as_ref()
            .and_then(|probe| probe.lease().cloned());
        let dispatched = binding.view.dispatch_semantic(invocation, move |outcome| {
            drop(watchdog);
            let outcome = if result_navigation.location_stable_for_result() {
                outcome
            } else {
                Err(SemanticRuntimePortFailure::DocumentReplaced)
            };
            #[cfg(feature = "native-agentic-foreground-probe")]
            let outcome = if foreground_probe::semantic_ready(result_rendering.as_ref(), context) {
                outcome
            } else {
                Err(SemanticRuntimePortFailure::NotReady)
            };
            match SemanticRuntimeSettlement::try_new(callback_correlation, outcome) {
                Ok(settlement) => task.complete(ContextNativeEvent::SemanticRuntimeSettled(
                    Box::new(settlement),
                )),
                Err(_) => {
                    callback_guard.callback_dispatch_rejected();
                    task.refuse(ContextPortFailure::NativeRefused);
                }
            }
        });
        if dispatched.is_ok() {
            binding.last_semantic_invocation = Some(invocation_id);
            binding.semantic_snapshot_generation = Some(snapshot_generation);
        }
    }

    #[cfg(target_os = "macos")]
    fn start_owned_agent_action(&mut self, mut task: AgentActionTask) {
        let Some(request) = task.request() else {
            self.fail_agent_context_invariant("agent-context action task lost its exact request");
            task.refuse(SemanticActionNativeFailure::Transport);
            return;
        };
        let frame = request.frame().clone();
        let context = frame.context();
        let id = context.identity().id();
        let attempt = request.attempt();
        let admitted_at = task.admitted_at();
        let Some(execution_millis) = request
            .deadline()
            .millis()
            .checked_sub(request.requested_at().millis())
        else {
            task.refuse(SemanticActionNativeFailure::TimedOut);
            return;
        };
        let execution_window = Duration::from_millis(execution_millis);
        let elapsed = Instant::now().saturating_duration_since(admitted_at);
        if execution_window.is_zero() || elapsed >= execution_window {
            task.refuse(SemanticActionNativeFailure::TimedOut);
            return;
        }
        if !zephium_agentic::AGENT_BROWSER_SNAPSHOT_ACTION_KINDS.contains(&request.kind()) {
            task.refuse(SemanticActionNativeFailure::UnsupportedInteraction);
            return;
        }

        let rejoin = self
            .agent_contexts
            .get_mut(&id)
            .ok_or(ContextPortFailure::Stale)
            .and_then(|binding| {
                binding.rejoin_navigation_replacement(context, AgentReplacementAdvance::Direct)
            });
        match rejoin {
            Ok(AgentReplacementRejoin::NotPending | AgentReplacementRejoin::Ready) => {}
            Ok(AgentReplacementRejoin::DeferredLocation) => {
                task.refuse(SemanticActionNativeFailure::StaleReference);
                self.retry_deferred_owned_agent_location_check(id);
                return;
            }
            Ok(AgentReplacementRejoin::DeferredRendererLoss) => {
                let emitter = self
                    .agent_contexts
                    .get(&id)
                    .map(|binding| binding.event_emitter.clone());
                task.refuse(SemanticActionNativeFailure::StaleReference);
                if let Some(emitter) = emitter {
                    emitter.emit_renderer_lost(context);
                } else {
                    self.fail_agent_context_invariant(
                        "agent-context action rejoin lost deferred renderer owner",
                    );
                }
                return;
            }
            Err(failure) => {
                task.refuse(map_context_failure_to_action(failure));
                return;
            }
        }

        let failure = match self.agent_contexts.get(&id) {
            None => Some(SemanticActionNativeFailure::StaleReference),
            Some(binding) if binding.join != context => {
                Some(SemanticActionNativeFailure::StaleReference)
            }
            Some(binding) if binding.renderer_lost => {
                Some(SemanticActionNativeFailure::RendererLost)
            }
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_screenshot.is_some() =>
            {
                Some(SemanticActionNativeFailure::ResourceExhausted)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Act) =>
            {
                Some(SemanticActionNativeFailure::UnsupportedInteraction)
            }
            Some(binding)
                if frame.frame() != FrameId::MAIN
                    || frame.trust() != SemanticFrameTrust::SameOrigin
                    || binding.committed_target.is_none()
                    || binding.last_semantic_invocation
                        != Some(request.checkpoint_invocation())
                    || binding.semantic_snapshot_generation
                        != Some(request.checkpoint_snapshot()) =>
            {
                Some(SemanticActionNativeFailure::StaleReference)
            }
            Some(binding) if binding.view.semantic_pending_for_audit() != Some(false) => {
                Some(SemanticActionNativeFailure::ResourceExhausted)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }

        let timeout_guard = task.callback_guard();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            execution_window.saturating_sub(elapsed),
            move || {
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.timeout_owned_agent_action(id, attempt);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(SemanticActionNativeFailure::Transport);
            return;
        };
        let Some(binding) = self.agent_contexts.get(&id) else {
            drop(watchdog);
            task.refuse(SemanticActionNativeFailure::StaleReference);
            return;
        };
        let callback_guard = task.callback_guard();
        let Some(request) = task.take_request() else {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "agent-context action request changed during admission",
            );
            task.refuse(SemanticActionNativeFailure::Transport);
            return;
        };
        binding
            .view
            .dispatch_semantic_action(request, admitted_at, move |settlement| {
                drop(watchdog);
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    task.complete(settlement);
                }))
                .is_err()
                {
                    callback_guard.callback_dispatch_rejected();
                }
            });
    }

    #[cfg(target_os = "macos")]
    fn timeout_owned_agent_action(
        &mut self,
        id: ContextId,
        attempt: zephium_agentic::SemanticActionAttemptId,
    ) {
        let matched = self
            .agent_contexts
            .get(&id)
            .and_then(|binding| binding.view.semantic())
            .is_some_and(|semantic| semantic.timeout_action(attempt));
        if !matched {
            self.fail_agent_context_invariant(
                "agent-context action timeout lost its exact pending invocation",
            );
        }
    }

    #[cfg(target_os = "macos")]
    fn start_owned_agent_screenshot(&mut self, mut task: AgentScreenshotTask) {
        let Some(request) = task.request() else {
            self.fail_agent_context_invariant(
                "agent-context screenshot task lost its exact request",
            );
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let context = request.context();
        let id = context.identity().id();
        let request_id = request.id();
        let snapshot_generation = request.snapshot_generation();
        let admitted_at = task.admitted_at();
        let Some(capture_window_millis) = request
            .deadline()
            .millis()
            .checked_sub(request.requested_at().millis())
        else {
            task.refuse(SemanticScreenshotNativeFailure::TimedOut);
            return;
        };
        let capture_window = Duration::from_millis(capture_window_millis);
        let elapsed = Instant::now().saturating_duration_since(admitted_at);
        if capture_window.is_zero() || elapsed >= capture_window {
            task.refuse(SemanticScreenshotNativeFailure::TimedOut);
            return;
        }

        let rejoin = self
            .agent_contexts
            .get_mut(&id)
            .ok_or(ContextPortFailure::Stale)
            .and_then(|binding| {
                binding.rejoin_navigation_replacement(context, AgentReplacementAdvance::Direct)
            });
        match rejoin {
            Ok(AgentReplacementRejoin::NotPending | AgentReplacementRejoin::Ready) => {}
            Ok(AgentReplacementRejoin::DeferredLocation) => {
                task.refuse(SemanticScreenshotNativeFailure::Stale);
                self.retry_deferred_owned_agent_location_check(id);
                return;
            }
            Ok(AgentReplacementRejoin::DeferredRendererLoss) => {
                let emitter = self
                    .agent_contexts
                    .get(&id)
                    .map(|binding| binding.event_emitter.clone());
                task.refuse(SemanticScreenshotNativeFailure::Stale);
                if let Some(emitter) = emitter {
                    emitter.emit_renderer_lost(context);
                } else {
                    self.fail_agent_context_invariant(
                        "agent-context screenshot rejoin lost deferred renderer owner",
                    );
                }
                return;
            }
            Err(failure) => {
                if failure == ContextPortFailure::NativeRefused {
                    self.fail_agent_context_invariant(
                        "agent-context screenshot rejoin contradicted native observer state",
                    );
                }
                task.refuse(map_context_failure_to_screenshot(failure));
                return;
            }
        }

        let failure = match self.agent_contexts.get(&id) {
            None => Some(SemanticScreenshotNativeFailure::Stale),
            Some(binding) if binding.join != context => {
                Some(SemanticScreenshotNativeFailure::Stale)
            }
            Some(binding) if binding.renderer_lost => {
                Some(SemanticScreenshotNativeFailure::RendererLost)
            }
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_screenshot.is_some() =>
            {
                Some(SemanticScreenshotNativeFailure::ResourceExhausted)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Observe) =>
            {
                Some(SemanticScreenshotNativeFailure::Unsupported)
            }
            Some(binding)
                if binding.committed_target.is_none()
                    || binding.semantic_snapshot_generation != Some(snapshot_generation) =>
            {
                Some(SemanticScreenshotNativeFailure::Stale)
            }
            Some(binding) if binding.view.semantic_pending_for_audit() != Some(false) => {
                Some(SemanticScreenshotNativeFailure::NotReady)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }

        let callback_guard = task.callback_guard();
        let timeout_guard = callback_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            capture_window.saturating_sub(elapsed),
            move || {
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_screenshot(
                        id,
                        request_id,
                        Err(SemanticScreenshotNativeFailure::TimedOut),
                    );
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let Some(request) = task.take_request() else {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "agent-context screenshot request changed during admission",
            );
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let Some(physical) = task.take_physical() else {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "agent-context screenshot lost its physical capacity permit",
            );
            task.refuse(SemanticScreenshotNativeFailure::Transport);
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let pending = AgentPendingScreenshot {
            id: request_id,
            context,
            snapshot_generation,
            capture_deadline: admitted_at
                .checked_add(capture_window)
                .unwrap_or(admitted_at),
            cancelled: cancelled.clone(),
            watchdog,
            task,
        };
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            pending.complete(Err(SemanticScreenshotNativeFailure::Stale));
            return;
        };
        if binding.pending_screenshot.is_some() {
            pending.complete(Err(SemanticScreenshotNativeFailure::Transport));
            self.fail_agent_context_invariant(
                "agent-context screenshot admission observed an existing capture",
            );
            return;
        }
        binding.pending_screenshot = Some(pending);

        let native_guard = callback_guard.clone();
        let panic_guard = callback_guard.clone();
        let result_navigation = binding.view.navigation().clone();
        let dispatched = binding.view.dispatch_screenshot(
            request,
            admitted_at,
            cancelled,
            move |outcome| {
                drop(physical);
                let outcome = if result_navigation.location_stable_for_result() {
                    outcome
                } else {
                    Err(SemanticScreenshotNativeFailure::Stale)
                };
                let rejected = native_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_screenshot(id, request_id, outcome);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
            move || panic_guard.callback_dispatch_rejected(),
        );
        if let Err(failure) = dispatched {
            self.finish_owned_agent_screenshot(id, request_id, Err(failure));
        }
    }

    #[cfg(target_os = "macos")]
    fn finish_owned_agent_screenshot(
        &mut self,
        id: ContextId,
        request_id: SemanticScreenshotRequestId,
        outcome: Result<
            crate::platform::semantic_pixel_privacy::ModelCapture<SemanticScreenshotNativeCapture>,
            SemanticScreenshotNativeFailure,
        >,
    ) {
        let Some(pending) = self.agent_contexts.get_mut(&id).and_then(|binding| {
            (binding
                .pending_screenshot
                .as_ref()
                .is_some_and(|pending| pending.id == request_id))
            .then(|| binding.pending_screenshot.take())
            .flatten()
        }) else {
            return;
        };
        pending.complete(
            outcome.map(crate::platform::semantic_pixel_privacy::ModelCapture::into_capture),
        );
    }

    #[cfg(target_os = "macos")]
    fn timeout_owned_agent_semantic_invocation(
        &mut self,
        id: ContextId,
        invocation: SemanticInvocationId,
    ) {
        let Some(binding) = self.agent_contexts.get(&id) else {
            return;
        };
        let Some(semantic) = binding.view.semantic() else {
            self.fail_agent_context_invariant(
                "agent-context semantic timeout lost its runtime registration",
            );
            return;
        };
        let _ = semantic.timeout(invocation);
    }

    #[cfg(target_os = "macos")]
    fn start_owned_agent_navigation(
        &mut self,
        task: AgentContextTask,
        request: ContextNavigationRequest,
    ) {
        if request.document_policy() != zephium_agentic::WorkBrowserDocumentPolicy::Exact {
            task.refuse(ContextPortFailure::Unsupported);
            return;
        }
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let target = request.target().clone();
        let redirect_policy = request.redirect_policy().cloned();
        let Some((task, replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
            task,
            requested,
            AgentReplacementAdvance::Navigation,
            false,
        ) else {
            return;
        };
        let failure = match self.agent_contexts.get(&id) {
            None => Some(ContextPortFailure::Stale),
            Some(binding) if binding.renderer_lost => Some(ContextPortFailure::Stale),
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_screenshot.is_some() =>
            {
                Some(ContextPortFailure::ResourceExhausted)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Navigate) =>
            {
                Some(ContextPortFailure::Unsupported)
            }
            Some(binding)
                if !replacement_rejoined && !navigation_successor(binding.join, requested) =>
            {
                Some(ContextPortFailure::Stale)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }

        // The functional core already advanced navigation/frame authority
        // before dispatch. Retain that successor even if native admission
        // below refuses, so a later close or cancellation cannot rejoin an
        // obsolete document generation.
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        binding.join = requested;
        binding.semantic_snapshot_generation = None;
        if binding.view.prepare_semantic_document_load().is_err() {
            self.fail_agent_context_invariant(
                "agent-context navigation could not rotate its semantic document world",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }

        let terminal_claimed = Arc::new(AtomicBool::new(false));
        let timeout_claim = terminal_claimed.clone();
        let callback_guard = task.callback_guard();
        let timeout_guard = callback_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            AGENT_PAGE_LOAD_COMMIT_TIMEOUT,
            move || {
                if timeout_claim
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_navigation(
                        id,
                        operation,
                        Err(ContextPortFailure::TimedOut),
                    );
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };

        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            drop(watchdog);
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        let armed = match redirect_policy.clone() {
            Some(policy) => binding.view.navigation().arm_with_redirect_policy(
                operation,
                target.clone(),
                policy,
                terminal_claimed.clone(),
            ),
            None => {
                binding
                    .view
                    .navigation()
                    .arm(operation, target.clone(), terminal_claimed.clone())
            }
        };
        if armed.is_err() {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "agent-context navigation gate retained a contradictory operation",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        binding.pending_navigation = Some(AgentPendingNavigation {
            operation,
            target,
            redirect_policy,
            watchdog,
            task,
        });
        let load_failed = binding
            .view
            .view()
            .load_url(request.target().as_url().as_str())
            .is_err();
        if load_failed && !terminal_claimed.swap(true, Ordering::AcqRel) {
            self.finish_owned_agent_navigation(
                id,
                operation,
                Err(ContextPortFailure::NativeRefused),
            );
        }
    }

    #[cfg(target_os = "macos")]
    fn start_owned_agent_recovery(
        &mut self,
        task: AgentContextTask,
        request: ContextTransitionRequest,
    ) {
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let failure = match self.agent_contexts.get(&id) {
            None => Some(ContextPortFailure::Stale),
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_screenshot.is_some() =>
            {
                Some(ContextPortFailure::ResourceExhausted)
            }
            Some(binding) if !binding.renderer_lost => Some(ContextPortFailure::Stale),
            Some(binding) if !binding.renderer_loss_rejoin_pending => {
                Some(ContextPortFailure::Stale)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Recover) =>
            {
                Some(ContextPortFailure::Unsupported)
            }
            Some(binding) if !double_full_successor(binding.join, requested) => {
                Some(ContextPortFailure::Stale)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }

        // Renderer loss and begin-recovery each advance the complete core
        // join. Retain that exact successor before any fallible native work so
        // later cancellation/close cannot rejoin the dead document.
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        binding.join = requested;
        binding.renderer_loss_rejoin_pending = false;
        binding.semantic_snapshot_generation = None;
        if binding.view.prepare_semantic_document_load().is_err() {
            self.fail_agent_context_invariant(
                "agent-context recovery could not rotate its semantic document world",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        let profile = binding.profile();
        let storage_class = binding.profile_lease.storage_class();
        let expected = binding.committed_target.clone();
        let ephemeral_store = match storage_class {
            ContextProfileStorageClass::Durable => None,
            ContextProfileStorageClass::Ephemeral => {
                let Some(store) = self.macos_ephemeral_data_stores.get(&profile).cloned() else {
                    task.refuse(ContextPortFailure::ProfileUnavailable);
                    return;
                };
                Some(store)
            }
        };
        let attestation = self.agent_contexts.get(&id).map_or(
            Err(crate::platform::imp::AgentOwnedViewConstructionError::Native),
            |binding| {
                binding
                    .view
                    .attest(profile, storage_class, ephemeral_store.as_ref())
            },
        );
        if let Err(failure) = attestation {
            task.refuse(map_owned_view_construction_failure(failure));
            return;
        }

        let terminal_claimed = Arc::new(AtomicBool::new(false));
        let timeout_claim = terminal_claimed.clone();
        let callback_guard = task.callback_guard();
        let timeout_guard = callback_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            AGENT_PAGE_LOAD_COMMIT_TIMEOUT,
            move || {
                if timeout_claim
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_recovery(
                        id,
                        operation,
                        Err(ContextPortFailure::TimedOut),
                    );
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };

        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            drop(watchdog);
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        if binding
            .view
            .navigation()
            .arm_recovery(operation, expected.clone(), terminal_claimed.clone())
            .is_err()
        {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "agent-context recovery gate did not retain exact renderer loss",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        binding.pending_recovery = Some(AgentPendingRecovery {
            operation,
            expected: expected.clone(),
            watchdog,
            task,
        });
        let load_failed = match expected {
            Some(_) => binding.view.view().reload().is_err(),
            None => binding.view.view().load_url("about:blank").is_err(),
        };
        if load_failed && !terminal_claimed.swap(true, Ordering::AcqRel) {
            self.finish_owned_agent_recovery(id, operation, Err(ContextPortFailure::NativeRefused));
        }
    }

    #[cfg(target_os = "macos")]
    fn on_owned_agent_navigation_terminal(
        &mut self,
        id: ContextId,
        terminal: crate::platform::imp::AgentNavigationTerminal,
    ) {
        let operation = terminal.operation();
        let outcome = terminal.into_outcome();
        if operation.context().identity().id() != id {
            self.fail_agent_context_invariant(
                "agent-context navigation callback crossed native context identity",
            );
            return;
        }
        match operation.kind() {
            ContextOperationKind::Navigate => {
                self.finish_owned_agent_navigation(id, operation, outcome);
            }
            ContextOperationKind::Recover => {
                self.finish_owned_agent_recovery(id, operation, outcome);
            }
            _ => self.fail_agent_context_invariant(
                "agent-context page-load callback carried an unsupported operation",
            ),
        }
    }

    #[cfg(target_os = "macos")]
    fn on_owned_agent_location_check(&mut self, id: ContextId, view_origin: ContextJoin) {
        let mut invariant_failure = None;
        let mut replacement = None;
        {
            let Some(binding) = self.agent_contexts.get_mut(&id) else {
                return;
            };
            if binding.native_view_origin != view_origin || binding.renderer_lost {
                return;
            }
            if binding.pending_navigation.is_some() || binding.pending_recovery.is_some() {
                if binding.view.navigation().defer_location_check().is_err() {
                    invariant_failure = Some(
                        "agent-context location callback could not defer across page-load work",
                    );
                }
            } else {
                let sampled = crate::platform::imp::current_url(binding.view.view())
                    .and_then(|url| ContextNavigationTarget::parse(&url).ok());
                match (binding.committed_target.as_ref(), sampled) {
                    (Some(previous), Some(current)) if *previous == current => {
                        if binding
                            .view
                            .navigation()
                            .finish_location_check(false)
                            .is_err()
                        {
                            invariant_failure = Some(
                                "agent-context unchanged location lost its callback ownership",
                            );
                        }
                    }
                    (Some(previous), Some(current))
                        if navigation_targets_share_origin(previous, &current) =>
                    {
                        if binding
                            .view
                            .navigation()
                            .finish_location_check(true)
                            .is_err()
                        {
                            invariant_failure =
                                Some("agent-context replacement lost its callback ownership");
                        } else {
                            let prior = binding.join;
                            let pending_screenshot = binding.pending_screenshot.take();
                            binding.committed_target = Some(current.clone());
                            binding.navigation_replacement_rejoin_pending = true;
                            binding.last_semantic_invocation = None;
                            binding.semantic_snapshot_generation = None;
                            replacement = Some((
                                prior,
                                current,
                                pending_screenshot,
                                binding.event_emitter.clone(),
                            ));
                        }
                    }
                    (Some(_), Some(_)) | (None, _) | (_, None) => {
                        crate::platform::imp::stop_loading(binding.view.view());
                        if binding
                            .view
                            .navigation()
                            .finish_location_check(false)
                            .is_err()
                        {
                            invariant_failure =
                                Some("agent-context invalid location lost its callback ownership");
                        } else {
                            invariant_failure = Some(
                                "agent-context native location escaped its committed same-origin target",
                            );
                        }
                    }
                }
            }
        }
        if let Some(message) = invariant_failure {
            self.fail_agent_context_invariant(message);
            return;
        }
        if let Some((prior, target, pending_screenshot, emitter)) = replacement {
            if let Some(pending) = pending_screenshot {
                pending.complete(Err(SemanticScreenshotNativeFailure::Stale));
            }
            emitter.emit_navigation_replaced(prior, target);
        }
    }

    #[cfg(target_os = "macos")]
    fn retry_deferred_owned_agent_location_check(&mut self, id: ContextId) {
        let queued = self.agent_contexts.get(&id).and_then(|binding| {
            if binding.renderer_lost
                || binding.pending_navigation.is_some()
                || binding.pending_recovery.is_some()
            {
                return None;
            }
            match binding.view.navigation().request_deferred_location_check() {
                Ok(true) => Some((binding.native_view_origin, binding.event_emitter.clone())),
                Ok(false) => None,
                Err(()) => Some((binding.native_view_origin, binding.event_emitter.clone())),
            }
        });
        let Some((view_origin, emitter)) = queued else {
            return;
        };
        let rejected = emitter.clone();
        if !crate::host::try_with_agent_context_terminal(move |host| {
            host.on_owned_agent_location_check(id, view_origin);
        }) {
            rejected.callback_dispatch_rejected();
        }
    }

    #[cfg(target_os = "macos")]
    fn on_owned_agent_renderer_lost(
        &mut self,
        id: ContextId,
        view_origin: ContextJoin,
        emitter: crate::agent_context_port::AgentContextCallbackGuard,
    ) {
        let (
            prior,
            pending_navigation,
            pending_recovery,
            pending_screenshot,
            navigation_clean,
            emit_loss,
        ) = {
            let Some(binding) = self.agent_contexts.get_mut(&id) else {
                return;
            };
            let recovering = binding.pending_recovery.is_some();
            if binding.native_view_origin != view_origin || (binding.renderer_lost && !recovering) {
                return;
            }
            crate::platform::imp::stop_loading(binding.view.view());
            let prior = binding.join;
            let pending_navigation = binding.pending_navigation.take();
            let pending_recovery = binding.pending_recovery.take();
            let pending_screenshot = binding.pending_screenshot.take();
            let pending_operation = pending_navigation
                .as_ref()
                .map(|pending| pending.operation)
                .or_else(|| pending_recovery.as_ref().map(|pending| pending.operation));
            let mutually_exclusive = pending_navigation.is_none() || pending_recovery.is_none();
            let disarmed = mutually_exclusive
                && pending_operation
                    .is_none_or(|operation| binding.view.navigation().disarm(operation));
            let navigation_clean =
                disarmed && binding.view.navigation().matches_for_audit(None, true);
            let emit_loss =
                pending_recovery.is_none() && !binding.navigation_replacement_rejoin_pending;
            binding.renderer_lost = true;
            binding.renderer_loss_rejoin_pending = emit_loss;
            binding.renderer_loss_deferred_for_replacement =
                pending_recovery.is_none() && binding.navigation_replacement_rejoin_pending;
            (
                prior,
                pending_navigation,
                pending_recovery,
                pending_screenshot,
                navigation_clean,
                emit_loss,
            )
        };

        if let Some(pending) = pending_navigation {
            pending.complete(Err(ContextPortFailure::NativeRefused));
        }
        if let Some(pending) = pending_recovery {
            pending.complete(Err(ContextPortFailure::NativeRefused));
        }
        if let Some(pending) = pending_screenshot {
            pending.complete(Err(SemanticScreenshotNativeFailure::RendererLost));
        }
        if !navigation_clean {
            self.fail_agent_context_invariant(
                "agent-context renderer loss did not retire exact navigation state",
            );
        }
        if emit_loss {
            emitter.emit_renderer_lost(prior);
        }
    }

    #[cfg(target_os = "macos")]
    fn finish_owned_agent_navigation(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
        outcome: Result<crate::platform::imp::AgentNavigationCommit, ContextPortFailure>,
    ) {
        let Some((pending, outcome, invariant_failed)) = (|| {
            let binding = self.agent_contexts.get_mut(&id)?;
            if binding
                .pending_navigation
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_navigation.take()?;
            if outcome.is_err() {
                crate::platform::imp::stop_loading(binding.view.view());
            }
            let mut invariant_failed = !binding.view.navigation().disarm(operation);
            let mut outcome = match outcome {
                Ok(crate::platform::imp::AgentNavigationCommit::Web(committed))
                    if pending.accepts_committed_target(&committed) =>
                {
                    if !invariant_failed {
                        binding.committed_target = Some(committed.clone());
                    }
                    Ok(committed)
                }
                Ok(crate::platform::imp::AgentNavigationCommit::Web(_))
                | Ok(crate::platform::imp::AgentNavigationCommit::Bootstrap) => {
                    invariant_failed = true;
                    Err(ContextPortFailure::NativeRefused)
                }
                Err(failure) => Err(failure),
            };
            if invariant_failed {
                outcome = Err(ContextPortFailure::NativeRefused);
            }
            Some((pending, outcome, invariant_failed))
        })() else {
            return;
        };
        if invariant_failed {
            self.fail_agent_context_invariant(
                "agent-context navigation terminal lost its exact gate or target",
            );
        }
        let committed = outcome.is_ok();
        pending.complete(outcome);
        self.retry_deferred_owned_agent_location_check(id);
        if committed {
            self.start_agent_favicon(id);
        }
    }

    #[cfg(target_os = "macos")]
    fn finish_owned_agent_recovery(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
        outcome: Result<crate::platform::imp::AgentNavigationCommit, ContextPortFailure>,
    ) {
        let Some((pending, outcome, invariant_failed)) = (|| {
            let binding = self.agent_contexts.get_mut(&id)?;
            if binding
                .pending_recovery
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_recovery.take()?;
            let (mut outcome, mut invariant_failed) = match outcome {
                Ok(crate::platform::imp::AgentNavigationCommit::Web(committed))
                    if pending.expected.as_ref() == Some(&committed) =>
                {
                    (Ok(()), false)
                }
                Ok(crate::platform::imp::AgentNavigationCommit::Bootstrap)
                    if pending.expected.is_none() =>
                {
                    (Ok(()), false)
                }
                Ok(crate::platform::imp::AgentNavigationCommit::Web(_))
                | Ok(crate::platform::imp::AgentNavigationCommit::Bootstrap) => {
                    (Err(ContextPortFailure::NativeRefused), true)
                }
                Err(failure) => (Err(failure), false),
            };
            let applied = outcome.is_ok();
            if !binding
                .view
                .navigation()
                .settle_recovery(operation, applied)
            {
                invariant_failed = true;
                outcome = Err(ContextPortFailure::NativeRefused);
            }
            binding.renderer_lost = outcome.is_err();
            if outcome.is_err() {
                crate::platform::imp::stop_loading(binding.view.view());
            }
            Some((pending, outcome, invariant_failed))
        })() else {
            return;
        };
        if invariant_failed {
            self.fail_agent_context_invariant(
                "agent-context recovery terminal lost its exact gate or target",
            );
        }
        pending.complete(outcome);
        self.retry_deferred_owned_agent_location_check(id);
    }

    #[cfg(target_os = "macos")]
    fn close_owned_agent_context(
        &mut self,
        task: AgentContextTask,
        request: ContextTransitionRequest,
    ) {
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let Some((task, _replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
            task,
            requested,
            AgentReplacementAdvance::Full,
            true,
        ) else {
            return;
        };
        let outcome = self
            .agent_contexts
            .get(&id)
            .map(|binding| {
                same_or_full_successor(binding.join, requested)
                    || (binding.renderer_loss_rejoin_pending
                        && double_full_successor(binding.join, requested))
            })
            .filter(|matches| *matches)
            .ok_or(ContextPortFailure::Stale)
            .and_then(|_| {
                #[cfg(feature = "native-agentic-foreground-probe")]
                if !self
                    .agent_contexts
                    .get_mut(&id)
                    .is_some_and(AgentOwnedContext::retire_foreground_probe)
                {
                    return Err(ContextPortFailure::ResourceExhausted);
                }
                let binding = self
                    .agent_contexts
                    .remove(&id)
                    .ok_or(ContextPortFailure::NativeRefused)?;
                let retirement = binding.retire(ContextPortFailure::Cancelled);
                if self.record_agent_context_retirement(retirement) {
                    Ok(())
                } else {
                    Err(ContextPortFailure::NativeRefused)
                }
            });
        match ContextTransitionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::TransitionSettled(settlement)),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "agent-context close settlement violated its closed operation contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn cancel_owned_agent_context(
        &mut self,
        current: ContextJoin,
        replacement_rejoined: bool,
    ) -> Result<(), ContextPortFailure> {
        let id = current.identity().id();
        let (pending_navigation, pending_recovery, pending_screenshot, disarmed) = {
            let binding = self
                .agent_contexts
                .get_mut(&id)
                .ok_or(ContextPortFailure::Stale)?;
            let rejoins = if replacement_rejoined {
                binding.join == current
            } else if binding.renderer_loss_rejoin_pending {
                full_successor(binding.join, current)
                    || double_full_successor(binding.join, current)
            } else {
                full_successor(binding.join, current)
            };
            if !rejoins {
                return Err(ContextPortFailure::Stale);
            }
            #[cfg(feature = "native-agentic-foreground-probe")]
            let _ = binding.retire_foreground_probe();
            let location_sealed = binding.view.navigation().seal_location_observation();
            crate::platform::imp::stop_loading(binding.view.view());
            binding
                .view
                .semantic()
                .ok_or(ContextPortFailure::NativeRefused)?
                .cancel();
            binding.join = current;
            binding.semantic_snapshot_generation = None;
            binding.renderer_loss_rejoin_pending = false;
            binding.renderer_loss_deferred_for_replacement = false;
            binding.navigation_replacement_rejoin_pending = false;
            let pending_navigation = binding.pending_navigation.take();
            let pending_recovery = binding.pending_recovery.take();
            let pending_screenshot = binding.pending_screenshot.take();
            let mutually_exclusive = (pending_navigation.is_none() || pending_recovery.is_none())
                && (pending_navigation.is_none() || pending_screenshot.is_none())
                && (pending_recovery.is_none() || pending_screenshot.is_none());
            let navigation_disarmed = pending_navigation
                .as_ref()
                .is_none_or(|pending| binding.view.navigation().disarm(pending.operation));
            let recovery_disarmed = pending_recovery.as_ref().is_none_or(|pending| {
                binding
                    .view
                    .navigation()
                    .settle_recovery(pending.operation, false)
            });
            if pending_recovery.is_some() {
                binding.renderer_lost = true;
            }
            (
                pending_navigation,
                pending_recovery,
                pending_screenshot,
                mutually_exclusive && navigation_disarmed && recovery_disarmed && location_sealed,
            )
        };
        if let Some(pending) = pending_navigation {
            if disarmed {
                pending.complete(Err(ContextPortFailure::Cancelled));
            } else {
                self.fail_agent_context_invariant(
                    "agent-context cancellation lost its exact navigation gate",
                );
                pending.complete(Err(ContextPortFailure::NativeRefused));
            }
        }
        if let Some(pending) = pending_recovery {
            if disarmed {
                pending.complete(Err(ContextPortFailure::Cancelled));
            } else {
                self.fail_agent_context_invariant(
                    "agent-context cancellation lost its exact recovery gate",
                );
                pending.complete(Err(ContextPortFailure::NativeRefused));
            }
        }
        if let Some(pending) = pending_screenshot {
            pending.complete(Err(SemanticScreenshotNativeFailure::Cancelled));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn complete_agent_construction(
        &mut self,
        task: AgentContextTask,
        operation: zephium_agentic::ContextOperationJoin,
        outcome: Result<ContextConstructionProof, ContextPortFailure>,
    ) {
        match ContextConstructionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::ConstructionSettled(settlement)),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "agent-context construction settlement violated its closed operation contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn map_agent_resource_failure(
        &mut self,
        error: super::resources::NativeResourceAdmissionError,
    ) -> ContextPortFailure {
        if error == super::resources::NativeResourceAdmissionError::AccountingInvariant {
            self.fail_agent_context_invariant("agent-context native resource accounting failed");
        }
        ContextPortFailure::ResourceExhausted
    }

    #[cfg(target_os = "macos")]
    fn fail_agent_context_invariant(&mut self, message: &'static str) {
        self.native_resource_accounting_failed = true;
        (self.native_terminal_failure)(message);
    }

    #[cfg(target_os = "macos")]
    fn record_agent_context_retirement(&mut self, retirement: AgentContextRetirement) -> bool {
        #[cfg(feature = "native-agentic-foreground-probe")]
        if !retirement.rendering_clean {
            self.fail_agent_context_invariant("agent-context rendering owner did not drain");
        }
        if !retirement.content_policy_clean {
            self.fail_content_policy_retirement();
        } else if !retirement.semantic_clean {
            self.fail_agent_context_invariant(
                "agent-context teardown did not retire its semantic runtime",
            );
        } else if !retirement.navigation_clean {
            self.fail_agent_context_invariant(
                "agent-context teardown lost its exact native navigation gate",
            );
        }
        retirement.is_clean()
    }

    #[cfg(target_os = "macos")]
    pub(super) fn has_agent_context_for_profile(
        &self,
        profile: zephium_core::ids::ProfileId,
    ) -> bool {
        self.agent_contexts
            .values()
            .any(|binding| binding.profile() == profile)
            || self
                .work_resources
                .values()
                .any(|resource| resource.profile() == profile)
    }

    /// Physically destroys all remaining private contexts during shutdown.
    ///
    /// A non-empty cohort means the shell crossed its lifecycle barrier
    /// without exact Close settlements, so teardown proceeds but clean
    /// shutdown is refused.
    #[cfg(target_os = "macos")]
    pub(super) fn force_shutdown_agent_contexts(&mut self) -> bool {
        let shell_was_quiescent =
            self.force_shutdown_work_resources() && self.agent_contexts.is_empty();
        let contexts = std::mem::take(&mut self.agent_contexts);
        let mut native_clean = true;
        for (_id, binding) in contexts {
            #[cfg(feature = "native-agentic-foreground-probe")]
            let mut binding = binding;
            #[cfg(feature = "native-agentic-foreground-probe")]
            if !binding.retire_foreground_probe() {
                // A forced shutdown never proves clean shell closure. Keep
                // the exact undrained presentation/context capacity owned
                // until process teardown rather than reporting a false zero.
                self.agent_contexts.insert(_id, binding);
                native_clean = false;
                continue;
            }
            let retirement = binding.retire(ContextPortFailure::Shutdown);
            native_clean &= self.record_agent_context_retirement(retirement);
        }
        shell_was_quiescent && native_clean
    }
}

#[cfg(target_os = "windows")]
impl EngineHost {
    fn construct_owned_agent_context(
        &mut self,
        request: ContextConstructionRequest,
        callback_guard: crate::agent_context_port::AgentContextCallbackGuard,
    ) -> Result<ContextConstructionProof, ContextPortFailure> {
        if request.source() != ContextConstructionSource::Owned
            || request.profile_lease().purpose() != ContextProfileLeasePurpose::Owned
        {
            return Err(ContextPortFailure::Unsupported);
        }
        let viewport = request
            .owned_viewport()
            .filter(|viewport| *viewport == ContextOwnedViewport::STANDARD)
            .ok_or(ContextPortFailure::NativeRefused)?;
        let join = request.operation().context();
        let identity = join.identity();
        let id = identity.id();
        let profile = identity.profile();
        if self.agent_contexts.contains_key(&id) {
            return Err(ContextPortFailure::Stale);
        }
        if self.agent_contexts.len() >= MAX_LIVE_CONTEXTS {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if self.erasure_tombstones.contains(&profile) {
            return Err(ContextPortFailure::ProfileUnavailable);
        }
        let partition = match request.profile_lease().storage_class() {
            ContextProfileStorageClass::Durable => Partition::Persistent(profile),
            ContextProfileStorageClass::Ephemeral => Partition::Ephemeral(profile),
        };
        if !bind_profile_persistence_class(&mut self.profile_persistence_classes, partition) {
            return Err(ContextPortFailure::ProfileUnavailable);
        }
        let content_policy = self
            .applied_content_policy(profile)
            .ok_or(ContextPortFailure::ProfileUnavailable)?;
        self.collect_pending_windows_cleanup_debts();
        if self.native_resource_accounting_failed
            || !self.native_resources.is_healthy()
            || self.windows_view_admission_blocked(profile)
            || !self.windows_profile_process_group_capacity_allows(profile)
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }
        if self
            .native_resources
            .count_for_audit(NativeResourceClass::AgentContext)
            .is_some_and(|count| count >= NativeResourceClass::AgentContext.limit())
        {
            return Err(ContextPortFailure::ResourceExhausted);
        }

        let storage_root = match request.profile_lease().storage_class() {
            ContextProfileStorageClass::Durable => self.profiles_root.clone(),
            ContextProfileStorageClass::Ephemeral => self.private_runtime.root().to_owned(),
        };
        let expected_user_data_folder =
            crate::erasure::prepare_profile_directory(&storage_root, profile)
                .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
        let now = Instant::now();
        let deadline = now
            .checked_add(Duration::from_secs(5))
            .ok_or(ContextPortFailure::NativeRefused)?;
        self.ensure_windows_profile_environment_at_path(
            profile,
            deadline,
            expected_user_data_folder.clone(),
            request.profile_lease().storage_class() == ContextProfileStorageClass::Durable,
        )
        .map_err(map_windows_profile_environment_failure)?;
        // Durable environments admit human extensions from startup. Work
        // always uses its existing, separate automation subprofile.
        let selected = false;
        if !selected && self.agent_cookie_quarantined_profiles.contains(&profile) {
            return Err(ContextPortFailure::CookieTransferFailed);
        }
        let owned_profile = if selected {
            crate::platform::imp::AgentOwnedProfile::Selected
        } else {
            crate::platform::imp::AgentOwnedProfile::automation(profile)
        };

        let environment = self
            .environments
            .get(&profile)
            .cloned()
            .ok_or(ContextPortFailure::ProfileUnavailable)?;
        let (process_id, process_generation) = self
            .capture_windows_environment(profile, environment.clone())
            .map_err(|_| ContextPortFailure::ProfileUnavailable)?;
        let mut native_resource = Some(
            self.native_resources
                .try_acquire(NativeResourceClass::TransientConstruction)
                .map_err(|error| self.map_agent_resource_failure(error))?,
        );

        let view_origin = join;
        let navigation_guard = callback_guard.clone();
        let location_guard = callback_guard.clone();
        let renderer_guard = callback_guard.clone();
        let browser_guard = callback_guard.clone();
        let invariant_guard = callback_guard.clone();
        let panic_guard = callback_guard.clone();
        let parent = super::ParentHandle(self.parent.0);
        let built = crate::platform::imp::build_owned_agent_view(
            &parent,
            viewport,
            &environment,
            owned_profile,
            request.profile_lease().storage_class(),
            &expected_user_data_folder,
            deadline,
            request.profile_lease().storage_class() == ContextProfileStorageClass::Durable,
            crate::platform::imp::AgentOwnedViewCallbacks::new(
                move |terminal| {
                    let rejected = navigation_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_navigation_terminal(id, terminal);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || {
                    let rejected = location_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_location_check(id, view_origin);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || {
                    let emitter = renderer_guard.clone();
                    let rejected = renderer_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_renderer_lost(id, view_origin, emitter);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || {
                    let emitter = browser_guard.clone();
                    let rejected = browser_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_renderer_lost(id, view_origin, emitter);
                        host.on_profile_process_exit(profile, process_id, process_generation);
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || invariant_guard.callback_dispatch_rejected(),
                move || panic_guard.callback_dispatch_rejected(),
            ),
        );

        let construction_debts = wry::pending_webview2_cleanup_debts();
        if !construction_debts.is_empty() {
            if built.is_ok() || construction_debts.len() != 1 {
                self.fail_windows_cleanup_invariant();
            }
            for debt in construction_debts {
                let resource = native_resource.take().or_else(|| {
                    self.native_resources
                        .try_acquire(NativeResourceClass::TeardownDebt)
                        .ok()
                });
                let debt = super::OwnedWindowsCleanupDebt::new(debt, resource);
                if !debt.accounted_as_debt() {
                    self.native_resource_accounting_failed = true;
                }
                self.retain_windows_cleanup_debt(profile, debt);
            }
        }
        if wry::webview2_cleanup_overflowed() {
            self.fail_windows_cleanup_invariant();
        }
        self.collect_pending_windows_cleanup_debts();
        let (mut view, proof) = match built {
            Ok(view) if !self.windows_view_admission_blocked(profile) => view,
            Ok((mut view, _)) => {
                self.close_unpublished_windows_agent_view(profile, &mut view, &mut native_resource);
                return Err(ContextPortFailure::ResourceExhausted);
            }
            Err(failure) => return Err(map_owned_view_construction_failure(failure)),
        };
        let controller_process = match crate::platform::imp::browser_process(view.view()) {
            Ok(process) => process,
            Err(_) => {
                self.close_unpublished_windows_agent_view(profile, &mut view, &mut native_resource);
                self.quarantine_unverifiable_windows_profile(profile);
                return Err(ContextPortFailure::NativeRefused);
            }
        };
        if controller_process.id() != process_id || !controller_process.is_running() {
            self.close_unpublished_windows_agent_view(profile, &mut view, &mut native_resource);
            self.quarantine_unverifiable_windows_profile(profile);
            return Err(ContextPortFailure::NativeRefused);
        }
        let content_policy_registration = match crate::platform::imp::install_content_policy_on_view(
            view.view(),
            &content_policy,
        ) {
            Ok(registration) => registration,
            Err(failure) => {
                self.close_unpublished_windows_agent_view(profile, &mut view, &mut native_resource);
                if failure == zephium_core::blocker::ContentRuleApplyFailure::NativeCleanup {
                    self.fail_content_policy_retirement();
                }
                return Err(ContextPortFailure::NativeRefused);
            }
        };
        let Some(resource) = native_resource.as_mut() else {
            let _ = content_policy_registration.retire();
            self.close_unpublished_windows_agent_view(profile, &mut view, &mut native_resource);
            self.fail_agent_context_invariant(
                "Windows agent construction lost its native resource lease",
            );
            return Err(ContextPortFailure::NativeRefused);
        };
        if let Err(error) = resource.reclassify(NativeResourceClass::AgentContext) {
            if content_policy_registration.retire().is_err() {
                self.fail_content_policy_retirement();
            }
            self.close_unpublished_windows_agent_view(profile, &mut view, &mut native_resource);
            return Err(self.map_agent_resource_failure(error));
        }
        let Some(native_resource) = native_resource.take() else {
            self.fail_agent_context_invariant(
                "Windows agent construction lost its reclassified resource lease",
            );
            return Err(ContextPortFailure::NativeRefused);
        };
        let binding = AgentOwnedContext::new(
            join,
            request.capabilities(),
            request.profile_lease(),
            content_policy_registration,
            callback_guard,
            view,
            self.native_terminal_failure.clone(),
            native_resource,
        );
        match self.agent_contexts.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(binding);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                let retirement = binding.retire(ContextPortFailure::NativeRefused);
                self.record_agent_context_retirement(retirement);
                self.fail_agent_context_invariant(
                    "Windows agent-context identity became occupied during native construction",
                );
                return Err(ContextPortFailure::NativeRefused);
            }
        }
        Ok(proof)
    }

    /// Starts one selected-profile-to-automation-profile transfer.
    ///
    /// Source and destination native authorities are derived from private host
    /// registries and re-attested on this UI thread. Neither COM owner nor any
    /// cookie field crosses the engine boundary.
    fn start_windows_agent_cookie_transfer(&mut self, task: AgentContextTask) {
        let Some((request, admitted_at)) = task.cookie() else {
            self.fail_agent_context_invariant(
                "Windows agent cookie task lost its exact admitted request",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        let request = request.clone();
        let transfer_id = request.id();
        let destination = request.destination();
        let destination_id = destination.identity().id();
        let destination_profile = destination.identity().profile();
        let Some(terminal_deadline) = crate::platform::imp::map_cookie_transfer_deadline(
            request.window(),
            admitted_at,
            Instant::now(),
        ) else {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::TimedOut,
            );
            return;
        };

        if request.direction() != ContextCookieTransferDirection::SelectedProfileToOwned {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::SourceUnavailable,
            );
            return;
        }
        if self.agent_cookie_transfers.contains_key(&transfer_id)
            || self.agent_cookie_transfers.len() >= MAX_PENDING_COOKIE_TRANSFERS
            || self
                .agent_cookie_transfers
                .values()
                .any(|pending| pending.destination_profile == destination_profile)
        {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::DestinationUnavailable,
            );
            return;
        }
        if self.erasure_tombstones.contains(&destination_profile)
            || self
                .agent_cookie_quarantined_profiles
                .contains(&destination_profile)
        {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::DestinationUnavailable,
            );
            return;
        }

        let Some(expected_environment) = self.environments.get(&destination_profile).cloned()
        else {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::SourceUnavailable,
            );
            return;
        };
        let destination_ready = self
            .agent_contexts
            .get(&destination_id)
            .is_some_and(|binding| {
                binding.join == destination
                    && binding.profile_lease == request.destination_profile_lease()
                    && binding
                        .capabilities
                        .contains(zephium_agentic::ContextCapability::ImportCookies)
                    && binding.committed_target.is_none()
                    && binding.pending_navigation.is_none()
                    && binding.pending_recovery.is_none()
                    && binding.pending_suspend.is_none()
                    && binding.pending_cookie_transfer.is_none()
                    && !binding.cookie_contaminated
                    && binding.late_suspend_claim.is_none()
                    && binding.suspend_state == AgentNativeSuspendState::Active
                    && !binding.renderer_lost
                    && binding.view.semantic_pending_for_audit() == Some(false)
            });
        if !destination_ready {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::DestinationUnavailable,
            );
            return;
        }

        let source =
            match self.selected_profile_cookie_source(destination_profile, &expected_environment) {
                Ok(source) => source,
                Err(failure) => {
                    self.complete_windows_agent_cookie_refusal(task, failure);
                    return;
                }
            };
        let destination_authority = self
            .agent_contexts
            .get(&destination_id)
            .ok_or(ContextCookieTransferFailure::DestinationUnavailable)
            .and_then(|binding| {
                binding
                    .view
                    .cookie_destination(&expected_environment)
                    .map_err(|_| ContextCookieTransferFailure::DestinationUnavailable)
            });
        let (destination_manager, destination_native_profile) = match destination_authority {
            Ok(authority) => authority,
            Err(failure) => {
                self.complete_windows_agent_cookie_refusal(task, failure);
                return;
            }
        };
        let Some(watchdog_duration) = terminal_deadline.checked_duration_since(Instant::now())
        else {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::TimedOut,
            );
            return;
        };
        if watchdog_duration.is_zero() {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::TimedOut,
            );
            return;
        }

        let timeout_guard = task.callback_guard();
        let Some(watchdog) =
            crate::platform::imp::schedule_content_policy_timeout(watchdog_duration, move || {
                let rejected = timeout_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.timeout_windows_agent_cookie_transfer(transfer_id);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            })
        else {
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::DestinationUnavailable,
            );
            return;
        };

        let Some(binding) = self.agent_contexts.get_mut(&destination_id) else {
            drop(watchdog);
            self.complete_windows_agent_cookie_refusal(
                task,
                ContextCookieTransferFailure::DestinationUnavailable,
            );
            return;
        };
        binding.pending_cookie_transfer = Some(transfer_id);

        let terminal_guard = task.callback_guard();
        let panic_guard = task.callback_guard();
        let transfer = crate::platform::imp::WindowsAgentCookieTransfer::start(
            source,
            destination_manager,
            destination_native_profile,
            &request,
            admitted_at,
            move |terminal| {
                let rejected = terminal_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_windows_agent_cookie_transfer(transfer_id, terminal);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
            move || panic_guard.callback_dispatch_rejected(),
        );
        let transfer = match transfer {
            Ok(transfer) => transfer,
            Err(failure) => {
                drop(watchdog);
                if let Some(binding) = self.agent_contexts.get_mut(&destination_id) {
                    if binding.pending_cookie_transfer == Some(transfer_id) {
                        binding.pending_cookie_transfer = None;
                    }
                }
                self.complete_windows_agent_cookie_refusal(task, failure);
                return;
            }
        };

        let pending = AgentPendingCookieTransfer {
            id: transfer_id,
            destination: destination_id,
            destination_profile,
            watchdog,
            transfer,
            task,
        };
        match self.agent_cookie_transfers.entry(transfer_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(pending);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                if let Some(binding) = self.agent_contexts.get_mut(&destination_id) {
                    if binding.pending_cookie_transfer == Some(transfer_id) {
                        binding.pending_cookie_transfer = None;
                    }
                }
                let _ = pending
                    .transfer
                    .cancel(ContextCookieTransferFailure::Cancelled);
                self.fail_agent_context_invariant(
                    "Windows agent cookie identity became occupied during native admission",
                );
                pending.task.refuse(ContextPortFailure::NativeRefused);
            }
        }
    }

    fn complete_windows_agent_cookie_refusal(
        &mut self,
        task: AgentContextTask,
        failure: ContextCookieTransferFailure,
    ) {
        let Some((request, _)) = task.cookie() else {
            self.fail_agent_context_invariant(
                "Windows agent cookie refusal lost its exact admitted request",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        let settlement = ContextCookieTransferSettlement::try_new(
            request.clone(),
            ContextCookieTransferOutcome::Refused(failure),
        );
        match settlement {
            Ok(settlement) => task.complete(ContextNativeEvent::CookieTransferSettled(Box::new(
                settlement,
            ))),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "Windows agent cookie refusal violated its closed request contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
    }

    fn timeout_windows_agent_cookie_transfer(&mut self, id: ContextCookieTransferId) {
        let Some(pending) = self.agent_cookie_transfers.get(&id) else {
            return;
        };
        let accepted = pending
            .transfer
            .cancel(ContextCookieTransferFailure::TimedOut);
        if !accepted && !pending.transfer.is_terminal() {
            self.fail_agent_context_invariant(
                "Windows agent cookie watchdog lost its exact native transfer",
            );
        }
    }

    fn finish_windows_agent_cookie_transfer(
        &mut self,
        id: ContextCookieTransferId,
        terminal: crate::platform::imp::WindowsAgentCookieTerminal,
    ) {
        let Some(pending) = self.agent_cookie_transfers.remove(&id) else {
            self.fail_agent_context_invariant(
                "Windows agent cookie terminal did not match an active transfer",
            );
            return;
        };
        let AgentPendingCookieTransfer {
            id: retained_id,
            destination,
            destination_profile,
            watchdog,
            transfer,
            task,
        } = pending;
        drop(watchdog);
        let native_terminal = transfer.is_terminal();
        drop(transfer);

        let outcome = terminal.outcome();
        let cleanup = terminal.cleanup();
        let cleanup_consistent = matches!(
            (outcome, cleanup),
            (
                ContextCookieTransferOutcome::Applied(_) | ContextCookieTransferOutcome::Refused(_),
                crate::platform::imp::WindowsAgentCookieCleanup::NotRequired
            ) | (
                ContextCookieTransferOutcome::Partial { .. },
                crate::platform::imp::WindowsAgentCookieCleanup::Proven
                    | crate::platform::imp::WindowsAgentCookieCleanup::Unproven
            )
        );
        let request = task.cookie().map(|(request, _)| request.clone());
        let task_consistent = request.as_ref().is_some_and(|request| {
            retained_id == id
                && request.id() == id
                && request.destination().identity().id() == destination
                && request.destination().identity().profile() == destination_profile
        });
        let binding_consistent = self
            .agent_contexts
            .get_mut(&destination)
            .is_some_and(|binding| {
                if binding.pending_cookie_transfer != Some(id)
                    || binding.profile() != destination_profile
                {
                    return false;
                }
                binding.pending_cookie_transfer = None;
                if matches!(outcome, ContextCookieTransferOutcome::Partial { .. }) {
                    binding.cookie_contaminated = true;
                }
                true
            });
        if matches!(outcome, ContextCookieTransferOutcome::Partial { .. })
            && cleanup != crate::platform::imp::WindowsAgentCookieCleanup::Proven
        {
            self.agent_cookie_quarantined_profiles
                .insert(destination_profile);
        }
        if !native_terminal || !cleanup_consistent || !task_consistent || !binding_consistent {
            self.fail_agent_context_invariant(
                "Windows agent cookie terminal violated host transaction ownership",
            );
        }

        let Some(request) = request else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        match ContextCookieTransferSettlement::try_new(request, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::CookieTransferSettled(Box::new(
                settlement,
            ))),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "Windows agent cookie terminal violated its bounded result contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
        self.retry_deferred_owned_agent_location_check(destination);
    }

    fn close_unpublished_windows_agent_view(
        &mut self,
        profile: zephium_core::ids::ProfileId,
        view: &mut crate::platform::imp::AgentOwnedView,
        native_resource: &mut Option<NativeResourceLease>,
    ) -> bool {
        let semantic_clean = view.retire_semantic_runtime();
        if !semantic_clean {
            self.fail_agent_context_invariant(
                "unpublished Windows agent semantic runtime did not retire exactly",
            );
        }
        let native_clean = match view.close() {
            Ok(()) => true,
            Err(debt) => {
                let debt = super::OwnedWindowsCleanupDebt::new(debt, native_resource.take());
                if !debt.accounted_as_debt() {
                    self.native_resource_accounting_failed = true;
                }
                self.retain_windows_cleanup_debt(profile, debt);
                false
            }
        };
        self.collect_pending_windows_cleanup_debts();
        semantic_clean && native_clean
    }

    fn start_owned_agent_navigation(
        &mut self,
        task: AgentContextTask,
        request: ContextNavigationRequest,
    ) {
        if request.document_policy() != zephium_agentic::WorkBrowserDocumentPolicy::Exact {
            task.refuse(ContextPortFailure::Unsupported);
            return;
        }
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let target = request.target().clone();
        let redirect_policy = request.redirect_policy().cloned();
        let Some((task, replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
            task,
            requested,
            AgentReplacementAdvance::Navigation,
            false,
        ) else {
            return;
        };
        let failure = match self.agent_contexts.get(&id) {
            None => Some(ContextPortFailure::Stale),
            Some(binding) if binding.renderer_lost => Some(ContextPortFailure::Stale),
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_suspend.is_some()
                    || binding.pending_cookie_transfer.is_some() =>
            {
                Some(ContextPortFailure::ResourceExhausted)
            }
            Some(binding) if binding.cookie_contaminated => {
                Some(ContextPortFailure::CookieTransferFailed)
            }
            Some(binding) if binding.suspend_state != AgentNativeSuspendState::Active => {
                Some(ContextPortFailure::NativeRefused)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Navigate) =>
            {
                Some(ContextPortFailure::Unsupported)
            }
            Some(binding)
                if !replacement_rejoined && !navigation_successor(binding.join, requested) =>
            {
                Some(ContextPortFailure::Stale)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        binding.join = requested;
        if binding.view.prepare_semantic_document_load().is_err() {
            self.fail_agent_context_invariant(
                "Windows agent navigation could not rotate its semantic document world",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        let terminal_claimed = Arc::new(AtomicBool::new(false));
        let timeout_claim = terminal_claimed.clone();
        let timeout_guard = task.callback_guard();
        let rejected_guard = timeout_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            AGENT_PAGE_LOAD_COMMIT_TIMEOUT,
            move || {
                if timeout_claim
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                let rejected = rejected_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_navigation(
                        id,
                        operation,
                        Err(ContextPortFailure::TimedOut),
                    );
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            drop(watchdog);
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        let armed = match redirect_policy.clone() {
            Some(policy) => binding.view.navigation().arm_with_redirect_policy(
                operation,
                target.clone(),
                policy,
                terminal_claimed.clone(),
            ),
            None => {
                binding
                    .view
                    .navigation()
                    .arm(operation, target.clone(), terminal_claimed.clone())
            }
        };
        if armed.is_err() {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "Windows agent navigation gate retained a contradictory operation",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        binding.pending_navigation = Some(AgentPendingNavigation {
            operation,
            target,
            redirect_policy,
            watchdog,
            task,
        });
        let load_failed = binding
            .view
            .view()
            .load_url(request.target().as_url().as_str())
            .is_err();
        if load_failed && !terminal_claimed.swap(true, Ordering::AcqRel) {
            self.finish_owned_agent_navigation(
                id,
                operation,
                Err(ContextPortFailure::NativeRefused),
            );
        }
    }

    fn start_owned_agent_recovery(
        &mut self,
        task: AgentContextTask,
        request: ContextTransitionRequest,
    ) {
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let failure = match self.agent_contexts.get(&id) {
            None => Some(ContextPortFailure::Stale),
            Some(binding)
                if binding.pending_navigation.is_some()
                    || binding.pending_recovery.is_some()
                    || binding.pending_suspend.is_some()
                    || binding.pending_cookie_transfer.is_some() =>
            {
                Some(ContextPortFailure::ResourceExhausted)
            }
            Some(binding) if binding.cookie_contaminated => {
                Some(ContextPortFailure::CookieTransferFailed)
            }
            Some(binding) if binding.suspend_state != AgentNativeSuspendState::Active => {
                Some(ContextPortFailure::NativeRefused)
            }
            Some(binding) if !binding.renderer_lost || !binding.renderer_loss_rejoin_pending => {
                Some(ContextPortFailure::Stale)
            }
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Recover) =>
            {
                Some(ContextPortFailure::Unsupported)
            }
            Some(binding) if !double_full_successor(binding.join, requested) => {
                Some(ContextPortFailure::Stale)
            }
            Some(_) => None,
        };
        if let Some(failure) = failure {
            task.refuse(failure);
            return;
        }
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        binding.join = requested;
        binding.renderer_loss_rejoin_pending = false;
        if binding.view.prepare_semantic_document_load().is_err() {
            self.fail_agent_context_invariant(
                "Windows agent recovery could not rotate its semantic document world",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        let expected = binding.committed_target.clone();
        if let Err(failure) = binding.view.attest() {
            task.refuse(map_owned_view_construction_failure(failure));
            return;
        }
        let terminal_claimed = Arc::new(AtomicBool::new(false));
        let timeout_claim = terminal_claimed.clone();
        let timeout_guard = task.callback_guard();
        let rejected_guard = timeout_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            AGENT_PAGE_LOAD_COMMIT_TIMEOUT,
            move || {
                if timeout_claim
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_err()
                {
                    return;
                }
                let rejected = rejected_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_recovery(
                        id,
                        operation,
                        Err(ContextPortFailure::TimedOut),
                    );
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            drop(watchdog);
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        if binding
            .view
            .navigation()
            .arm_recovery(operation, expected.clone(), terminal_claimed.clone())
            .is_err()
        {
            drop(watchdog);
            self.fail_agent_context_invariant(
                "Windows agent recovery gate did not retain exact renderer loss",
            );
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        }
        binding.pending_recovery = Some(AgentPendingRecovery {
            operation,
            expected: expected.clone(),
            watchdog,
            task,
        });
        let load_failed = match expected {
            Some(_) => binding.view.view().reload().is_err(),
            None => binding.view.view().load_url("about:blank").is_err(),
        };
        if load_failed && !terminal_claimed.swap(true, Ordering::AcqRel) {
            self.finish_owned_agent_recovery(id, operation, Err(ContextPortFailure::NativeRefused));
        }
    }

    fn start_owned_agent_suspend(
        &mut self,
        task: AgentContextTask,
        request: ContextTransitionRequest,
    ) {
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let Some((task, replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
            task,
            requested,
            AgentReplacementAdvance::Full,
            false,
        ) else {
            return;
        };
        let structural_failure = match self.agent_contexts.get(&id) {
            None => Some(ContextPortFailure::Stale),
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Suspend) =>
            {
                Some(ContextPortFailure::Unsupported)
            }
            Some(binding) if !replacement_rejoined && !full_successor(binding.join, requested) => {
                Some(ContextPortFailure::Stale)
            }
            Some(_) => None,
        };
        if let Some(failure) = structural_failure {
            task.refuse(failure);
            return;
        }
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };

        // The core advances every join coordinate before native work. Retain
        // that exact successor even when watchdog or COM admission refuses.
        binding.join = requested;
        let native_failure = if binding.renderer_lost {
            Some(ContextPortFailure::Stale)
        } else if binding.pending_navigation.is_some()
            || binding.pending_recovery.is_some()
            || binding.pending_suspend.is_some()
            || binding.pending_cookie_transfer.is_some()
        {
            Some(ContextPortFailure::ResourceExhausted)
        } else if binding.cookie_contaminated {
            Some(ContextPortFailure::CookieTransferFailed)
        } else if binding.suspend_state != AgentNativeSuspendState::Active
            || binding.late_suspend_claim.is_some()
        {
            Some(ContextPortFailure::NativeRefused)
        } else if binding.pending_operation_for_audit() != Some(false) {
            Some(ContextPortFailure::ResourceExhausted)
        } else {
            None
        };
        if let Some(failure) = native_failure {
            task.refuse(failure);
            return;
        }
        let claim = crate::platform::agent_suspension::AgentSuspendClaim::new();
        let timeout_claim = claim.clone();
        let timeout_guard = task.callback_guard();
        let rejected_guard = timeout_guard.clone();
        let Some(watchdog) = crate::platform::imp::schedule_content_policy_timeout(
            AGENT_SUSPEND_TIMEOUT,
            move || {
                if !timeout_claim.timeout() {
                    return;
                }
                let rejected = rejected_guard.clone();
                if !crate::host::try_with_agent_context_terminal(move |host| {
                    host.finish_owned_agent_suspend_timeout(id, operation);
                }) {
                    rejected.callback_dispatch_rejected();
                }
            },
        ) else {
            task.refuse(ContextPortFailure::NativeRefused);
            return;
        };
        let callback_claim = claim.clone();
        let callback_guard = task.callback_guard();
        let panic_guard = callback_guard.clone();
        binding.pending_suspend = Some(AgentPendingSuspend {
            operation,
            claim,
            watchdog,
            task,
        });
        let native_refused = binding
            .view
            .try_suspend(
                move |native_succeeded| {
                    let disposition = callback_claim.native_completed();
                    let rejected = callback_guard.clone();
                    if !crate::host::try_with_agent_context_terminal(move |host| {
                        host.on_owned_agent_suspend_native(
                            id,
                            operation,
                            disposition,
                            native_succeeded,
                        );
                    }) {
                        rejected.callback_dispatch_rejected();
                    }
                },
                move || panic_guard.callback_dispatch_rejected(),
            )
            .is_err();
        if native_refused {
            self.finish_owned_agent_suspend_immediate_failure(id, operation);
        }
    }

    fn finish_owned_agent_suspend_timeout(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
    ) {
        let Some(pending) = self.agent_contexts.get_mut(&id).and_then(|binding| {
            if binding
                .pending_suspend
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_suspend.take()?;
            binding.late_suspend_claim = Some((operation, pending.claim.clone()));
            binding.suspend_state = AgentNativeSuspendState::Uncertain(operation);
            Some(pending)
        }) else {
            return;
        };
        pending.complete(Err(ContextPortFailure::TimedOut));
    }

    fn finish_owned_agent_suspend_immediate_failure(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
    ) {
        let Some((pending, outcome)) = (|| {
            let binding = self.agent_contexts.get_mut(&id)?;
            if binding
                .pending_suspend
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_suspend.take()?;
            pending.claim.retire();
            let outcome = match binding.view.attest_suspension_state() {
                Ok(true) => match binding.view.resume_and_attest_active() {
                    Ok(true) => {
                        binding.suspend_state = AgentNativeSuspendState::Active;
                        Err(ContextPortFailure::NativeRefused)
                    }
                    Ok(false) | Err(_) => {
                        binding.suspend_state = AgentNativeSuspendState::Uncertain(operation);
                        Err(ContextPortFailure::NativeRefused)
                    }
                },
                Ok(false) => {
                    binding.suspend_state = AgentNativeSuspendState::Active;
                    Err(ContextPortFailure::NativeRefused)
                }
                Err(_) => {
                    binding.suspend_state = AgentNativeSuspendState::Uncertain(operation);
                    Err(ContextPortFailure::NativeRefused)
                }
            };
            Some((pending, outcome))
        })() else {
            return;
        };
        pending.complete(outcome);
        self.retry_deferred_owned_agent_location_check(id);
    }

    fn on_owned_agent_suspend_native(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
        disposition: crate::platform::agent_suspension::AgentSuspendNativeDisposition,
        native_succeeded: bool,
    ) {
        use crate::platform::agent_suspension::AgentSuspendNativeDisposition as Disposition;

        match disposition {
            Disposition::Retired => {}
            Disposition::Duplicate => self.fail_agent_context_invariant(
                "Windows agent suspend callback completed more than once",
            ),
            Disposition::Reconcile => {
                self.reconcile_owned_agent_suspend(id, operation);
            }
            Disposition::Terminal => {
                let pending_matches = self.agent_contexts.get(&id).is_some_and(|binding| {
                    binding
                        .pending_suspend
                        .as_ref()
                        .is_some_and(|pending| pending.operation == operation)
                });
                if pending_matches {
                    self.finish_owned_agent_suspend_native_terminal(
                        id,
                        operation,
                        native_succeeded,
                    );
                } else if self.agent_contexts.get(&id).is_some_and(|binding| {
                    binding.suspend_state == AgentNativeSuspendState::Uncertain(operation)
                }) {
                    // A cancellation can retire the external task after the
                    // native callback won but before this queued terminal ran.
                    self.reconcile_owned_agent_suspend(id, operation);
                }
            }
        }
    }

    fn finish_owned_agent_suspend_native_terminal(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
        native_succeeded: bool,
    ) {
        let Some((pending, outcome)) = (|| {
            let binding = self.agent_contexts.get_mut(&id)?;
            if binding
                .pending_suspend
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_suspend.take()?;
            let outcome = match binding.view.attest_suspension_state() {
                Ok(true) if native_succeeded => {
                    binding.suspend_state = AgentNativeSuspendState::Suspended;
                    Ok(())
                }
                Ok(true) => match binding.view.resume_and_attest_active() {
                    Ok(true) => {
                        binding.suspend_state = AgentNativeSuspendState::Active;
                        Err(ContextPortFailure::NativeRefused)
                    }
                    Ok(false) | Err(_) => {
                        binding.suspend_state = AgentNativeSuspendState::Uncertain(operation);
                        Err(ContextPortFailure::NativeRefused)
                    }
                },
                Ok(false) => {
                    binding.suspend_state = AgentNativeSuspendState::Active;
                    Err(ContextPortFailure::NativeRefused)
                }
                Err(_) => {
                    binding.suspend_state = AgentNativeSuspendState::Uncertain(operation);
                    Err(ContextPortFailure::NativeRefused)
                }
            };
            Some((pending, outcome))
        })() else {
            return;
        };
        pending.complete(outcome);
        self.retry_deferred_owned_agent_location_check(id);
    }

    fn reconcile_owned_agent_suspend(&mut self, id: ContextId, operation: ContextOperationJoin) {
        let reconciled = self.agent_contexts.get_mut(&id).is_some_and(|binding| {
            if binding.pending_suspend.is_some()
                || binding.suspend_state != AgentNativeSuspendState::Uncertain(operation)
                || binding
                    .late_suspend_claim
                    .as_ref()
                    .is_none_or(|(expected, _)| *expected != operation)
            {
                return false;
            }
            let _completed_claim = binding.late_suspend_claim.take();
            match binding.view.resume_and_attest_active() {
                Ok(true) => {
                    binding.suspend_state = AgentNativeSuspendState::Active;
                    true
                }
                Ok(false) | Err(_) => false,
            }
        });
        if self.agent_contexts.contains_key(&id) && !reconciled {
            self.fail_agent_context_invariant(
                "Windows agent late suspend callback could not restore active state",
            );
        } else if reconciled {
            self.retry_deferred_owned_agent_location_check(id);
        }
    }

    fn resume_owned_agent_context(
        &mut self,
        task: AgentContextTask,
        request: ContextTransitionRequest,
    ) {
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let structural_failure = match self.agent_contexts.get(&id) {
            None => Some(ContextPortFailure::Stale),
            Some(binding)
                if !binding
                    .capabilities
                    .contains(zephium_agentic::ContextCapability::Suspend) =>
            {
                Some(ContextPortFailure::Unsupported)
            }
            Some(binding) if !full_successor(binding.join, requested) => {
                Some(ContextPortFailure::Stale)
            }
            Some(_) => None,
        };
        if let Some(failure) = structural_failure {
            task.refuse(failure);
            return;
        }
        let Some(binding) = self.agent_contexts.get_mut(&id) else {
            task.refuse(ContextPortFailure::Stale);
            return;
        };
        binding.join = requested;
        let outcome = if binding.renderer_lost {
            Err(ContextPortFailure::Stale)
        } else if binding.pending_navigation.is_some()
            || binding.pending_recovery.is_some()
            || binding.pending_suspend.is_some()
            || binding.pending_cookie_transfer.is_some()
        {
            Err(ContextPortFailure::ResourceExhausted)
        } else if binding.cookie_contaminated {
            Err(ContextPortFailure::CookieTransferFailed)
        } else if binding.suspend_state != AgentNativeSuspendState::Suspended
            || binding.late_suspend_claim.is_some()
        {
            Err(ContextPortFailure::NativeRefused)
        } else {
            match binding.view.resume_and_attest_active() {
                Ok(true) => {
                    binding.suspend_state = AgentNativeSuspendState::Active;
                    Ok(())
                }
                Ok(false) => {
                    binding.suspend_state = AgentNativeSuspendState::Suspended;
                    Err(ContextPortFailure::NativeRefused)
                }
                Err(_) => {
                    binding.suspend_state = AgentNativeSuspendState::Uncertain(operation);
                    Err(ContextPortFailure::NativeRefused)
                }
            }
        };
        match ContextTransitionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::TransitionSettled(settlement)),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "Windows agent resume settlement violated its closed operation contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
        self.retry_deferred_owned_agent_location_check(id);
    }

    fn on_owned_agent_navigation_terminal(
        &mut self,
        id: ContextId,
        terminal: crate::platform::imp::AgentNavigationTerminal,
    ) {
        let operation = terminal.operation();
        let outcome = terminal.into_outcome();
        if operation.context().identity().id() != id {
            self.fail_agent_context_invariant(
                "Windows agent navigation callback crossed context identity",
            );
            return;
        }
        match operation.kind() {
            ContextOperationKind::Navigate => {
                self.finish_owned_agent_navigation(id, operation, outcome)
            }
            ContextOperationKind::Recover => {
                self.finish_owned_agent_recovery(id, operation, outcome)
            }
            _ => self.fail_agent_context_invariant(
                "Windows agent page-load callback carried an unsupported operation",
            ),
        }
    }

    #[cfg(target_os = "windows")]
    fn on_owned_agent_location_check(&mut self, id: ContextId, view_origin: ContextJoin) {
        let mut invariant_failure = None;
        let mut replacement = None;
        {
            let Some(binding) = self.agent_contexts.get_mut(&id) else {
                return;
            };
            if binding.native_view_origin != view_origin || binding.renderer_lost {
                return;
            }
            if binding.pending_navigation.is_some()
                || binding.pending_recovery.is_some()
                || binding.pending_suspend.is_some()
                || binding.pending_cookie_transfer.is_some()
                || binding.suspend_state != AgentNativeSuspendState::Active
                || binding.late_suspend_claim.is_some()
            {
                if binding.view.navigation().defer_location_check().is_err() {
                    invariant_failure = Some(
                        "Windows agent location callback could not defer across lifecycle work",
                    );
                }
            } else {
                let sampled = crate::platform::imp::current_url(binding.view.view())
                    .and_then(|url| ContextNavigationTarget::parse(&url).ok());
                match (binding.committed_target.as_ref(), sampled) {
                    (Some(previous), Some(current)) if *previous == current => {
                        if binding
                            .view
                            .navigation()
                            .finish_location_check(false)
                            .is_err()
                        {
                            invariant_failure = Some(
                                "Windows agent unchanged location lost its callback ownership",
                            );
                        }
                    }
                    (Some(previous), Some(current))
                        if navigation_targets_share_origin(previous, &current) =>
                    {
                        if binding
                            .view
                            .navigation()
                            .finish_location_check(true)
                            .is_err()
                        {
                            invariant_failure =
                                Some("Windows agent replacement lost its callback ownership");
                        } else {
                            let prior = binding.join;
                            binding.committed_target = Some(current.clone());
                            binding.navigation_replacement_rejoin_pending = true;
                            replacement = Some((prior, current, binding.event_emitter.clone()));
                        }
                    }
                    (Some(_), Some(_)) | (None, _) | (_, None) => {
                        crate::platform::imp::stop_loading(binding.view.view());
                        if binding
                            .view
                            .navigation()
                            .finish_location_check(false)
                            .is_err()
                        {
                            invariant_failure =
                                Some("Windows agent invalid location lost its callback ownership");
                        } else {
                            invariant_failure = Some(
                                "Windows agent native location escaped its committed same-origin target",
                            );
                        }
                    }
                }
            }
        }
        if let Some(message) = invariant_failure {
            self.fail_agent_context_invariant(message);
            return;
        }
        if let Some((prior, target, emitter)) = replacement {
            emitter.emit_navigation_replaced(prior, target);
        }
    }

    #[cfg(target_os = "windows")]
    fn retry_deferred_owned_agent_location_check(&mut self, id: ContextId) {
        let queued = self.agent_contexts.get(&id).and_then(|binding| {
            if binding.renderer_lost
                || binding.pending_navigation.is_some()
                || binding.pending_recovery.is_some()
                || binding.pending_suspend.is_some()
                || binding.pending_cookie_transfer.is_some()
                || binding.suspend_state != AgentNativeSuspendState::Active
                || binding.late_suspend_claim.is_some()
            {
                return None;
            }
            match binding.view.navigation().request_deferred_location_check() {
                Ok(true) => Some((binding.native_view_origin, binding.event_emitter.clone())),
                Ok(false) => None,
                Err(()) => Some((binding.native_view_origin, binding.event_emitter.clone())),
            }
        });
        let Some((view_origin, emitter)) = queued else {
            return;
        };
        let rejected = emitter.clone();
        if !crate::host::try_with_agent_context_terminal(move |host| {
            host.on_owned_agent_location_check(id, view_origin);
        }) {
            rejected.callback_dispatch_rejected();
        }
    }

    fn on_owned_agent_renderer_lost(
        &mut self,
        id: ContextId,
        view_origin: ContextJoin,
        emitter: crate::agent_context_port::AgentContextCallbackGuard,
    ) {
        let (
            prior,
            pending_navigation,
            pending_recovery,
            pending_suspend,
            pending_cookie_transfer,
            navigation_clean,
            emit_loss,
        ) = {
            let Some(binding) = self.agent_contexts.get_mut(&id) else {
                return;
            };
            let recovering = binding.pending_recovery.is_some();
            if binding.native_view_origin != view_origin || (binding.renderer_lost && !recovering) {
                return;
            }
            crate::platform::imp::stop_loading(binding.view.view());
            let prior = binding.join;
            let pending_navigation = binding.pending_navigation.take();
            let pending_recovery = binding.pending_recovery.take();
            let pending_suspend = binding.pending_suspend.take();
            let pending_cookie_transfer = binding.pending_cookie_transfer;
            if let Some(pending) = pending_suspend.as_ref() {
                pending.claim.retire();
            }
            if let Some((_, claim)) = binding.late_suspend_claim.take() {
                claim.retire();
            }
            let pending_operation = pending_navigation
                .as_ref()
                .map(|pending| pending.operation)
                .or_else(|| pending_recovery.as_ref().map(|pending| pending.operation));
            let lifecycle_count = usize::from(pending_navigation.is_some())
                + usize::from(pending_recovery.is_some())
                + usize::from(pending_suspend.is_some())
                + usize::from(pending_cookie_transfer.is_some());
            let disarmed = lifecycle_count <= 1
                && pending_operation
                    .is_none_or(|operation| binding.view.navigation().disarm(operation));
            let navigation_clean =
                disarmed && binding.view.navigation().matches_for_audit(None, true);
            let emit_loss =
                pending_recovery.is_none() && !binding.navigation_replacement_rejoin_pending;
            binding.suspend_state = AgentNativeSuspendState::Active;
            binding.renderer_lost = true;
            binding.renderer_loss_rejoin_pending = emit_loss;
            binding.renderer_loss_deferred_for_replacement =
                pending_recovery.is_none() && binding.navigation_replacement_rejoin_pending;
            (
                prior,
                pending_navigation,
                pending_recovery,
                pending_suspend,
                pending_cookie_transfer,
                navigation_clean,
                emit_loss,
            )
        };
        if let Some(pending) = pending_navigation {
            pending.complete(Err(ContextPortFailure::NativeRefused));
        }
        if let Some(pending) = pending_recovery {
            pending.complete(Err(ContextPortFailure::NativeRefused));
        }
        if let Some(pending) = pending_suspend {
            pending.complete(Err(ContextPortFailure::NativeRefused));
        }
        if let Some(transfer_id) = pending_cookie_transfer {
            let cancelled = self
                .agent_cookie_transfers
                .get(&transfer_id)
                .is_some_and(|pending| {
                    pending.destination == id
                        && (pending
                            .transfer
                            .cancel(ContextCookieTransferFailure::Cancelled)
                            || pending.transfer.is_terminal())
                });
            if !cancelled {
                self.fail_agent_context_invariant(
                    "Windows renderer loss could not cancel its cookie transfer",
                );
            }
        }
        if !navigation_clean {
            self.fail_agent_context_invariant(
                "Windows agent renderer loss did not retire exact navigation state",
            );
        }
        if emit_loss {
            emitter.emit_renderer_lost(prior);
        }
    }

    fn finish_owned_agent_navigation(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
        outcome: Result<crate::platform::imp::AgentNavigationCommit, ContextPortFailure>,
    ) {
        let Some((pending, outcome, invariant_failed)) = (|| {
            let binding = self.agent_contexts.get_mut(&id)?;
            if binding
                .pending_navigation
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_navigation.take()?;
            if outcome.is_err() {
                crate::platform::imp::stop_loading(binding.view.view());
            }
            let mut invariant_failed = !binding.view.navigation().disarm(operation);
            let mut outcome = match outcome {
                Ok(crate::platform::imp::AgentNavigationCommit::Web(committed))
                    if pending.accepts_committed_target(&committed) =>
                {
                    if !invariant_failed {
                        binding.committed_target = Some(committed.clone());
                    }
                    Ok(committed)
                }
                Ok(crate::platform::imp::AgentNavigationCommit::Web(_))
                | Ok(crate::platform::imp::AgentNavigationCommit::Bootstrap) => {
                    invariant_failed = true;
                    Err(ContextPortFailure::NativeRefused)
                }
                Err(failure) => Err(failure),
            };
            if invariant_failed {
                outcome = Err(ContextPortFailure::NativeRefused);
            }
            Some((pending, outcome, invariant_failed))
        })() else {
            return;
        };
        if invariant_failed {
            self.fail_agent_context_invariant(
                "Windows agent navigation terminal lost its exact gate or target",
            );
        }
        pending.complete(outcome);
        self.retry_deferred_owned_agent_location_check(id);
    }

    fn finish_owned_agent_recovery(
        &mut self,
        id: ContextId,
        operation: ContextOperationJoin,
        outcome: Result<crate::platform::imp::AgentNavigationCommit, ContextPortFailure>,
    ) {
        let Some((pending, outcome, invariant_failed)) = (|| {
            let binding = self.agent_contexts.get_mut(&id)?;
            if binding
                .pending_recovery
                .as_ref()
                .is_none_or(|pending| pending.operation != operation)
            {
                return None;
            }
            let pending = binding.pending_recovery.take()?;
            let (mut outcome, mut invariant_failed) = match outcome {
                Ok(crate::platform::imp::AgentNavigationCommit::Web(committed))
                    if pending.expected.as_ref() == Some(&committed) =>
                {
                    (Ok(()), false)
                }
                Ok(crate::platform::imp::AgentNavigationCommit::Bootstrap)
                    if pending.expected.is_none() =>
                {
                    (Ok(()), false)
                }
                Ok(crate::platform::imp::AgentNavigationCommit::Web(_))
                | Ok(crate::platform::imp::AgentNavigationCommit::Bootstrap) => {
                    (Err(ContextPortFailure::NativeRefused), true)
                }
                Err(failure) => (Err(failure), false),
            };
            let applied = outcome.is_ok();
            if !binding
                .view
                .navigation()
                .settle_recovery(operation, applied)
            {
                invariant_failed = true;
                outcome = Err(ContextPortFailure::NativeRefused);
            }
            binding.renderer_lost = outcome.is_err();
            if outcome.is_err() {
                crate::platform::imp::stop_loading(binding.view.view());
            }
            Some((pending, outcome, invariant_failed))
        })() else {
            return;
        };
        if invariant_failed {
            self.fail_agent_context_invariant(
                "Windows agent recovery terminal lost its exact gate or target",
            );
        }
        pending.complete(outcome);
        self.retry_deferred_owned_agent_location_check(id);
    }

    fn close_owned_agent_context(
        &mut self,
        task: AgentContextTask,
        request: ContextTransitionRequest,
    ) {
        let operation = request.operation();
        let requested = operation.context();
        let id = requested.identity().id();
        let Some((task, _replacement_rejoined)) = self.admit_navigation_replacement_rejoin(
            task,
            requested,
            AgentReplacementAdvance::Full,
            true,
        ) else {
            return;
        };
        let outcome = self
            .agent_contexts
            .get(&id)
            .map(|binding| {
                same_or_full_successor(binding.join, requested)
                    || (binding.renderer_loss_rejoin_pending
                        && double_full_successor(binding.join, requested))
            })
            .filter(|matches| *matches)
            .ok_or(ContextPortFailure::Stale)
            .and_then(|_| {
                if self
                    .agent_contexts
                    .get(&id)
                    .is_some_and(|binding| binding.pending_cookie_transfer.is_some())
                {
                    return Err(ContextPortFailure::ResourceExhausted);
                }
                let binding = self
                    .agent_contexts
                    .remove(&id)
                    .ok_or(ContextPortFailure::NativeRefused)?;
                let retirement = binding.retire(ContextPortFailure::Cancelled);
                self.collect_pending_windows_cleanup_debts();
                if self.record_agent_context_retirement(retirement) {
                    Ok(())
                } else {
                    Err(ContextPortFailure::NativeRefused)
                }
            });
        match ContextTransitionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::TransitionSettled(settlement)),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "Windows agent close settlement violated its closed operation contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
    }

    fn cancel_owned_agent_context(
        &mut self,
        current: ContextJoin,
        replacement_rejoined: bool,
    ) -> Result<(), ContextPortFailure> {
        let id = current.identity().id();
        let (
            pending_navigation,
            pending_recovery,
            pending_suspend,
            pending_cookie_transfer,
            mut disarmed,
        ) = {
            let binding = self
                .agent_contexts
                .get_mut(&id)
                .ok_or(ContextPortFailure::Stale)?;
            let rejoins = if replacement_rejoined {
                binding.join == current
            } else if binding.renderer_loss_rejoin_pending {
                full_successor(binding.join, current)
                    || double_full_successor(binding.join, current)
            } else {
                full_successor(binding.join, current)
            };
            if !rejoins {
                return Err(ContextPortFailure::Stale);
            }
            crate::platform::imp::stop_loading(binding.view.view());
            binding.join = current;
            binding.renderer_loss_rejoin_pending = false;
            binding.renderer_loss_deferred_for_replacement = false;
            binding.navigation_replacement_rejoin_pending = false;
            let location_sealed = binding.view.navigation().seal_location_observation();
            let pending_navigation = binding.pending_navigation.take();
            let pending_recovery = binding.pending_recovery.take();
            let pending_suspend = binding.pending_suspend.take();
            let pending_cookie_transfer = binding.pending_cookie_transfer;
            let lifecycle_count = usize::from(pending_navigation.is_some())
                + usize::from(pending_recovery.is_some())
                + usize::from(pending_suspend.is_some())
                + usize::from(pending_cookie_transfer.is_some());
            let navigation_disarmed = pending_navigation
                .as_ref()
                .is_none_or(|pending| binding.view.navigation().disarm(pending.operation));
            let recovery_disarmed = pending_recovery.as_ref().is_none_or(|pending| {
                binding
                    .view
                    .navigation()
                    .settle_recovery(pending.operation, false)
            });
            if pending_recovery.is_some() {
                binding.suspend_state = AgentNativeSuspendState::Active;
                binding.renderer_lost = true;
            }
            let suspend_disarmed = pending_suspend.as_ref().is_none_or(|pending| {
                let _ = pending.claim.cancel();
                if binding.late_suspend_claim.is_some() {
                    return false;
                }
                binding.late_suspend_claim = Some((pending.operation, pending.claim.clone()));
                binding.suspend_state = AgentNativeSuspendState::Uncertain(pending.operation);
                true
            });
            (
                pending_navigation,
                pending_recovery,
                pending_suspend,
                pending_cookie_transfer,
                lifecycle_count <= 1
                    && navigation_disarmed
                    && recovery_disarmed
                    && suspend_disarmed
                    && location_sealed,
            )
        };
        if let Some(transfer_id) = pending_cookie_transfer {
            let cookie_disarmed =
                self.agent_cookie_transfers
                    .get(&transfer_id)
                    .is_some_and(|pending| {
                        pending.destination == id
                            && (pending
                                .transfer
                                .cancel(ContextCookieTransferFailure::Cancelled)
                                || pending.transfer.is_terminal())
                    });
            disarmed &= cookie_disarmed;
        }
        if let Some(pending) = pending_navigation {
            pending.complete(Err(if disarmed {
                ContextPortFailure::Cancelled
            } else {
                ContextPortFailure::NativeRefused
            }));
        }
        if let Some(pending) = pending_recovery {
            pending.complete(Err(if disarmed {
                ContextPortFailure::Cancelled
            } else {
                ContextPortFailure::NativeRefused
            }));
        }
        if let Some(pending) = pending_suspend {
            pending.complete(Err(if disarmed {
                ContextPortFailure::Cancelled
            } else {
                ContextPortFailure::NativeRefused
            }));
        }
        if !disarmed {
            self.fail_agent_context_invariant(
                "Windows agent cancellation lost its exact native lifecycle gate",
            );
            return Err(ContextPortFailure::NativeRefused);
        }
        Ok(())
    }

    fn complete_agent_construction(
        &mut self,
        task: AgentContextTask,
        operation: ContextOperationJoin,
        outcome: Result<ContextConstructionProof, ContextPortFailure>,
    ) {
        match ContextConstructionSettlement::try_new(operation, outcome) {
            Ok(settlement) => task.complete(ContextNativeEvent::ConstructionSettled(settlement)),
            Err(_) => {
                self.fail_agent_context_invariant(
                    "Windows agent construction settlement violated its closed contract",
                );
                task.refuse(ContextPortFailure::NativeRefused);
            }
        }
    }

    fn map_agent_resource_failure(
        &mut self,
        error: super::resources::NativeResourceAdmissionError,
    ) -> ContextPortFailure {
        if error == super::resources::NativeResourceAdmissionError::AccountingInvariant {
            self.fail_agent_context_invariant("Windows agent native resource accounting failed");
        }
        ContextPortFailure::ResourceExhausted
    }

    fn fail_agent_context_invariant(&mut self, message: &'static str) {
        self.native_resource_accounting_failed = true;
        (self.native_terminal_failure)(message);
    }

    fn record_agent_context_retirement(&mut self, retirement: AgentContextRetirement) -> bool {
        if !retirement.content_policy_clean {
            self.fail_content_policy_retirement();
        } else if !retirement.semantic_clean {
            self.fail_agent_context_invariant(
                "Windows agent semantic runtime did not retire exactly",
            );
        } else if !retirement.navigation_clean {
            self.fail_agent_context_invariant(
                "Windows agent teardown lost its exact native navigation gate",
            );
        }
        retirement.is_clean()
    }

    pub(super) fn has_agent_context_for_profile(
        &self,
        profile: zephium_core::ids::ProfileId,
    ) -> bool {
        self.has_windows_agent_view_or_transfer_for_profile(profile)
            || self
                .work_resources
                .values()
                .any(|resource| resource.profile() == profile)
    }

    pub(super) fn has_windows_agent_view_or_transfer_for_profile(
        &self,
        profile: zephium_core::ids::ProfileId,
    ) -> bool {
        self.agent_contexts
            .values()
            .any(|binding| binding.profile() == profile)
            || self
                .agent_cookie_transfers
                .values()
                .any(|pending| pending.destination_profile == profile)
    }

    pub(super) fn force_shutdown_agent_contexts(&mut self) -> bool {
        let shell_was_quiescent = self.force_shutdown_work_resources()
            && self.agent_contexts.is_empty()
            && self.agent_cookie_transfers.is_empty();
        let transfers = std::mem::take(&mut self.agent_cookie_transfers);
        for (id, pending) in transfers {
            let binding_detached = self
                .agent_contexts
                .get_mut(&pending.destination)
                .is_some_and(|binding| {
                    if binding.pending_cookie_transfer != Some(id) {
                        return false;
                    }
                    binding.pending_cookie_transfer = None;
                    true
                });
            if !binding_detached {
                self.fail_agent_context_invariant(
                    "Windows shutdown lost an active cookie destination binding",
                );
            }
            let _ = pending
                .transfer
                .cancel(ContextCookieTransferFailure::Shutdown);
            drop(pending);
        }
        let contexts = std::mem::take(&mut self.agent_contexts);
        let mut native_clean = true;
        for (_, binding) in contexts {
            let retirement = binding.retire(ContextPortFailure::Shutdown);
            native_clean &= self.record_agent_context_retirement(retirement);
        }
        self.collect_pending_windows_cleanup_debts();
        shell_was_quiescent && native_clean && !self.windows_cleanup_invariant_failed
    }
}

#[cfg(target_os = "windows")]
fn map_windows_profile_environment_failure(
    failure: super::construction::WindowsProfileEnvironmentFailure,
) -> ContextPortFailure {
    use super::construction::WindowsProfileEnvironmentFailure as Failure;
    match failure {
        Failure::Busy => ContextPortFailure::ProfileBusy,
        Failure::Mismatch => ContextPortFailure::ProfileUnavailable,
        Failure::Construction => ContextPortFailure::NativeRefused,
    }
}

#[cfg(target_os = "windows")]
fn map_owned_view_construction_failure(
    failure: crate::platform::imp::AgentOwnedViewConstructionError,
) -> ContextPortFailure {
    match failure {
        crate::platform::imp::AgentOwnedViewConstructionError::Storage => {
            ContextPortFailure::ProfileUnavailable
        }
        crate::platform::imp::AgentOwnedViewConstructionError::ExtensionIsolation => {
            ContextPortFailure::ExtensionIsolationUnproven
        }
        crate::platform::imp::AgentOwnedViewConstructionError::Native => {
            ContextPortFailure::NativeRefused
        }
    }
}

#[cfg(target_os = "macos")]
fn map_owned_view_construction_failure(
    failure: crate::platform::imp::AgentOwnedViewConstructionError,
) -> ContextPortFailure {
    match failure {
        crate::platform::imp::AgentOwnedViewConstructionError::Storage => {
            ContextPortFailure::ProfileUnavailable
        }
        crate::platform::imp::AgentOwnedViewConstructionError::ExtensionIsolation => {
            ContextPortFailure::ExtensionIsolationUnproven
        }
        crate::platform::imp::AgentOwnedViewConstructionError::Native => {
            ContextPortFailure::NativeRefused
        }
    }
}

#[cfg(target_os = "macos")]
const fn map_context_failure_to_screenshot(
    failure: ContextPortFailure,
) -> SemanticScreenshotNativeFailure {
    match failure {
        ContextPortFailure::Unsupported => SemanticScreenshotNativeFailure::Unsupported,
        ContextPortFailure::ResourceExhausted => SemanticScreenshotNativeFailure::ResourceExhausted,
        ContextPortFailure::Cancelled => SemanticScreenshotNativeFailure::Cancelled,
        ContextPortFailure::TimedOut => SemanticScreenshotNativeFailure::TimedOut,
        ContextPortFailure::Stale => SemanticScreenshotNativeFailure::Stale,
        ContextPortFailure::Shutdown => SemanticScreenshotNativeFailure::Shutdown,
        ContextPortFailure::ProfileUnavailable
        | ContextPortFailure::ProfileBusy
        | ContextPortFailure::ExtensionIsolationUnproven
        | ContextPortFailure::CookieTransferFailed
        | ContextPortFailure::NativeRefused => SemanticScreenshotNativeFailure::Transport,
    }
}

#[cfg(target_os = "macos")]
const fn map_context_failure_to_action(failure: ContextPortFailure) -> SemanticActionNativeFailure {
    match failure {
        ContextPortFailure::Unsupported => SemanticActionNativeFailure::UnsupportedInteraction,
        ContextPortFailure::ResourceExhausted => SemanticActionNativeFailure::ResourceExhausted,
        ContextPortFailure::Cancelled => SemanticActionNativeFailure::Cancelled,
        ContextPortFailure::TimedOut => SemanticActionNativeFailure::TimedOut,
        ContextPortFailure::Stale => SemanticActionNativeFailure::StaleReference,
        ContextPortFailure::Shutdown => SemanticActionNativeFailure::Shutdown,
        ContextPortFailure::ProfileUnavailable
        | ContextPortFailure::ProfileBusy
        | ContextPortFailure::ExtensionIsolationUnproven
        | ContextPortFailure::CookieTransferFailed
        | ContextPortFailure::NativeRefused => SemanticActionNativeFailure::Transport,
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn navigation_targets_share_origin(
    prior: &ContextNavigationTarget,
    current: &ContextNavigationTarget,
) -> bool {
    SemanticOrigin::parse(prior.as_url().as_str())
        .ok()
        .zip(SemanticOrigin::parse(current.as_url().as_str()).ok())
        .is_some_and(|(prior, current)| prior == current)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn full_successor(prior: ContextJoin, current: ContextJoin) -> bool {
    prior.identity() == current.identity()
        && prior.frame() == current.frame()
        && prior.context_generation().next() == Some(current.context_generation())
        && prior.navigation_epoch().next() == Some(current.navigation_epoch())
        && prior.frame_generation().next() == Some(current.frame_generation())
        && prior.cancellation_generation().next() == Some(current.cancellation_generation())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn double_full_successor(prior: ContextJoin, current: ContextJoin) -> bool {
    prior.identity() == current.identity()
        && prior.frame() == current.frame()
        && prior
            .context_generation()
            .next()
            .and_then(|generation| generation.next())
            == Some(current.context_generation())
        && prior
            .navigation_epoch()
            .next()
            .and_then(|epoch| epoch.next())
            == Some(current.navigation_epoch())
        && prior
            .frame_generation()
            .next()
            .and_then(|generation| generation.next())
            == Some(current.frame_generation())
        && prior
            .cancellation_generation()
            .next()
            .and_then(|generation| generation.next())
            == Some(current.cancellation_generation())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn navigation_successor(prior: ContextJoin, current: ContextJoin) -> bool {
    prior.identity() == current.identity()
        && prior.frame() == current.frame()
        && prior.context_generation() == current.context_generation()
        && prior.navigation_epoch().next() == Some(current.navigation_epoch())
        && prior.frame_generation().next() == Some(current.frame_generation())
        && prior.cancellation_generation() == current.cancellation_generation()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn double_navigation_successor(prior: ContextJoin, current: ContextJoin) -> bool {
    prior.identity() == current.identity()
        && prior.frame() == current.frame()
        && prior.context_generation() == current.context_generation()
        && prior
            .navigation_epoch()
            .next()
            .and_then(|epoch| epoch.next())
            == Some(current.navigation_epoch())
        && prior
            .frame_generation()
            .next()
            .and_then(|generation| generation.next())
            == Some(current.frame_generation())
        && prior.cancellation_generation() == current.cancellation_generation()
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn replacement_then_full_successor(prior: ContextJoin, current: ContextJoin) -> bool {
    prior.identity() == current.identity()
        && prior.frame() == current.frame()
        && prior.context_generation().next() == Some(current.context_generation())
        && prior
            .navigation_epoch()
            .next()
            .and_then(|epoch| epoch.next())
            == Some(current.navigation_epoch())
        && prior
            .frame_generation()
            .next()
            .and_then(|generation| generation.next())
            == Some(current.frame_generation())
        && prior.cancellation_generation().next() == Some(current.cancellation_generation())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn replacement_rejoin_matches(
    prior: ContextJoin,
    current: ContextJoin,
    advance: AgentReplacementAdvance,
) -> bool {
    match advance {
        #[cfg(target_os = "macos")]
        AgentReplacementAdvance::Direct => navigation_successor(prior, current),
        AgentReplacementAdvance::Navigation => double_navigation_successor(prior, current),
        AgentReplacementAdvance::Full => replacement_then_full_successor(prior, current),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn same_or_full_successor(prior: ContextJoin, current: ContextJoin) -> bool {
    prior == current || full_successor(prior, current)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    fn owned_context() -> (
        zephium_agentic::ContextRegistry,
        zephium_agentic::ContextId,
        zephium_agentic::ContextOperationJoin,
    ) {
        let identity = zephium_agentic::ContextIdentity::new(
            zephium_agentic::ContextId::generate(),
            zephium_agentic::ContextRunId::generate(),
            zephium_core::ids::ProfileId::from(3),
            zephium_agentic::ContextKind::Owned,
        );
        let capabilities = zephium_agentic::ContextCapabilities::try_new(
            zephium_agentic::ContextKind::Owned,
            &[
                zephium_agentic::ContextCapability::Navigate,
                zephium_agentic::ContextCapability::Recover,
            ],
        )
        .expect("capabilities");
        let mut registry = zephium_agentic::ContextRegistry::new();
        registry.reserve(identity, capabilities).expect("reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                zephium_agentic::ContextOperationId::new(1).expect("operation"),
            )
            .expect("context");
        (registry, identity.id(), construction)
    }

    #[test]
    fn owned_context_source_has_no_ordinary_identity_projection() {
        let source = include_str!("agent_context.rs");
        for forbidden in [
            concat!("Item", "Id"),
            concat!("self.", "views"),
            concat!("self.", "partitions"),
            concat!("self.", "stages"),
            concat!("extension_document_", "authority"),
            concat!("extension_browser_", "surfaces"),
            concat!("navigation_", "snapshots"),
        ] {
            assert!(
                !source.contains(forbidden),
                "forbidden projection: {forbidden}"
            );
        }
        assert!(source.contains("profile_lease: ContextProfileLease"));
        assert!(source.contains("native_resource: Option<NativeResourceLease>"));
        let owner = source
            .split_once("pub(super) struct AgentOwnedContext {")
            .expect("owned context declaration")
            .1
            .split_once("\n}")
            .expect("owned context declaration end")
            .0;
        let registration = owner
            .find("content_policy_registration:")
            .expect("policy registration");
        let view = owner
            .find("view: crate::platform::imp::AgentOwnedView")
            .expect("native view");
        let resource = owner
            .find("native_resource: Option<NativeResourceLease>")
            .expect("native resource");
        assert!(registration < view && view < resource);
    }

    #[test]
    fn windows_owned_construction_proves_identity_before_publication() {
        let source = include_str!("agent_context.rs");
        let windows = source
            .split_once("#[cfg(target_os = \"windows\")]\nimpl EngineHost {")
            .expect("Windows owner implementation")
            .1;
        let environment = windows
            .find("ensure_windows_profile_environment_at_path(")
            .expect("profile environment proof");
        let transient_resource = windows
            .find("try_acquire(NativeResourceClass::TransientConstruction)")
            .expect("transient resource");
        let build = windows
            .find("build_owned_agent_view(")
            .expect("private native view build");
        let cleanup_import = windows
            .find("pending_webview2_cleanup_debts()")
            .expect("construction cleanup import");
        let process = windows
            .find("browser_process(view.view())")
            .expect("exact process readback");
        let policy = windows
            .find("install_content_policy_on_view(")
            .expect("content policy");
        let reclassify = windows
            .find("resource.reclassify(NativeResourceClass::AgentContext)")
            .expect("resource reclassification");
        let publish = windows
            .find("self.agent_contexts.entry(id)")
            .expect("private owner publication");
        assert!(environment < transient_resource);
        assert!(transient_resource < build);
        assert!(build < cleanup_import);
        assert!(cleanup_import < process);
        assert!(process < policy);
        assert!(policy < reclassify);
        assert!(reclassify < publish);
    }

    #[test]
    fn windows_owned_failures_close_before_releasing_or_quarantining() {
        let source = include_str!("agent_context.rs");
        let windows = source
            .split_once("#[cfg(target_os = \"windows\")]\nimpl EngineHost {")
            .expect("Windows owner implementation")
            .1;
        let process_failure = windows
            .split_once("let controller_process = match")
            .expect("process readback branch")
            .1
            .split_once("let content_policy_registration = match")
            .expect("process readback branch end")
            .0;
        let close = process_failure
            .find("close_unpublished_windows_agent_view")
            .expect("explicit native close");
        let quarantine = process_failure
            .find("quarantine_unverifiable_windows_profile")
            .expect("profile quarantine");
        let terminal = process_failure
            .find("return Err(ContextPortFailure::NativeRefused)")
            .expect("typed refusal");
        assert!(close < quarantine && quarantine < terminal);

        let helper = windows
            .split_once("fn close_unpublished_windows_agent_view(")
            .expect("unpublished close helper")
            .1
            .split_once("fn start_owned_agent_navigation(")
            .expect("unpublished close helper end")
            .0;
        let native_close = helper.find("view.close()").expect("WebView2 close");
        let transfer = helper
            .find("OwnedWindowsCleanupDebt::new(debt, native_resource.take())")
            .expect("exact lease transfer");
        let retain = helper
            .find("retain_windows_cleanup_debt(profile, debt)")
            .expect("bounded cleanup debt retention");
        let collect = helper
            .find("collect_pending_windows_cleanup_debts()")
            .expect("global cleanup debt import");
        assert!(native_close < transfer && transfer < retain && retain < collect);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn close_and_cancellation_accept_only_the_exact_core_generation_step() {
        let (mut ready, id, construction) = owned_context();
        ready
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction");
        let prior = ready.join(id).expect("join");
        let cancelled = ready.cancel_run(id, prior).expect("cancellation");
        assert!(super::full_successor(prior, cancelled));
        assert!(!super::same_or_full_successor(cancelled, prior));

        let close = ready
            .begin_close(
                id,
                zephium_agentic::ContextOperationId::new(2).expect("operation"),
            )
            .expect("close");
        assert!(super::full_successor(cancelled, close.context()));

        let (mut faulted, id, construction) = owned_context();
        faulted
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Refused,
            )
            .expect("refused construction");
        let prior = faulted.join(id).expect("join");
        let close = faulted
            .begin_close(
                id,
                zephium_agentic::ContextOperationId::new(3).expect("operation"),
            )
            .expect("fault cleanup");
        assert_eq!(prior, close.context());
        assert!(super::same_or_full_successor(prior, close.context()));
        assert!(!super::full_successor(prior, close.context()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn navigation_accepts_only_the_core_navigation_and_frame_successor() {
        let (mut registry, id, construction) = owned_context();
        registry
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction");
        let prior = registry.join(id).expect("join");
        let navigation = registry
            .begin_navigation(
                id,
                zephium_agentic::ContextOperationId::new(7).expect("operation"),
            )
            .expect("navigation");
        assert!(super::navigation_successor(prior, navigation.context()));
        assert!(!super::navigation_successor(navigation.context(), prior));

        let cancelled = registry
            .cancel_run(id, navigation.context())
            .expect("cancellation");
        assert!(!super::navigation_successor(prior, cancelled));
        assert!(super::full_successor(navigation.context(), cancelled));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn replacement_rejoin_shapes_match_exact_followup_transitions() {
        let (mut registry, id, construction) = owned_context();
        registry
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction");
        let prior = registry.join(id).expect("join");
        let replacement = registry
            .observe_navigation_replacement(id, prior)
            .expect("replacement");
        assert!(super::replacement_rejoin_matches(
            prior,
            replacement,
            super::AgentReplacementAdvance::Direct,
        ));
        let navigation = registry
            .begin_navigation(
                id,
                zephium_agentic::ContextOperationId::new(8).expect("operation"),
            )
            .expect("navigation");
        assert!(super::replacement_rejoin_matches(
            prior,
            navigation.context(),
            super::AgentReplacementAdvance::Navigation,
        ));
        assert!(!super::replacement_rejoin_matches(
            prior,
            navigation.context(),
            super::AgentReplacementAdvance::Direct,
        ));

        let (mut registry, id, construction) = owned_context();
        registry
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction");
        let prior = registry.join(id).expect("join");
        let replacement = registry
            .observe_navigation_replacement(id, prior)
            .expect("replacement");
        let cancelled = registry.cancel_run(id, replacement).expect("cancellation");
        assert!(super::replacement_rejoin_matches(
            prior,
            cancelled,
            super::AgentReplacementAdvance::Full,
        ));
        assert!(!super::replacement_rejoin_matches(
            prior,
            cancelled,
            super::AgentReplacementAdvance::Navigation,
        ));
    }

    #[test]
    fn page_location_replacement_is_limited_to_the_committed_origin() {
        let prior =
            zephium_agentic::ContextNavigationTarget::parse("https://example.test:8443/start")
                .expect("prior");
        let same = zephium_agentic::ContextNavigationTarget::parse(
            "https://example.test:8443/history?step=2#done",
        )
        .expect("same origin");
        let port_change =
            zephium_agentic::ContextNavigationTarget::parse("https://example.test:9443/history")
                .expect("different port");
        let scheme_change =
            zephium_agentic::ContextNavigationTarget::parse("http://example.test:8443/history")
                .expect("different scheme");
        assert!(super::navigation_targets_share_origin(&prior, &same));
        assert!(!super::navigation_targets_share_origin(
            &prior,
            &port_change
        ));
        assert!(!super::navigation_targets_share_origin(
            &prior,
            &scheme_change
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn renderer_loss_rejoins_only_the_exact_one_or_two_core_steps() {
        let (mut registry, id, construction) = owned_context();
        registry
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction");
        let prior = registry.join(id).expect("join");
        let lost = registry.renderer_lost(id, prior).expect("renderer loss");
        assert!(super::full_successor(prior, lost));
        assert!(!super::double_full_successor(prior, lost));

        let close = registry
            .begin_close(
                id,
                zephium_agentic::ContextOperationId::new(9).expect("operation"),
            )
            .expect("close after loss");
        assert!(super::double_full_successor(prior, close.context()));
        assert!(!super::full_successor(prior, close.context()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn renderer_recovery_is_the_exact_double_full_successor() {
        let (mut registry, id, construction) = owned_context();
        registry
            .settle_construction(
                id,
                construction,
                zephium_agentic::ContextSettlement::Applied,
            )
            .expect("construction");
        let prior = registry.join(id).expect("join");
        let lost = registry.renderer_lost(id, prior).expect("renderer loss");
        let recovery = registry
            .begin_recovery(
                id,
                zephium_agentic::ContextOperationId::new(10).expect("operation"),
            )
            .expect("recovery");
        assert!(super::full_successor(prior, lost));
        assert!(super::full_successor(lost, recovery.context()));
        assert!(super::double_full_successor(prior, recovery.context()));
        assert!(!super::full_successor(prior, recovery.context()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recovery_rejoins_before_fallible_native_work_and_reuses_the_view() {
        let recovery = include_str!("agent_context.rs")
            .split_once("fn start_owned_agent_recovery(")
            .expect("recovery owner")
            .1
            .split_once("fn on_owned_agent_navigation_terminal(")
            .expect("recovery owner end")
            .0;
        let rejoin = recovery.find("binding.join = requested").expect("rejoin");
        let rotate = recovery
            .find("binding.view.prepare_semantic_document_load()")
            .expect("semantic epoch rotation");
        let reattest = recovery.find(".attest(profile").expect("reattest");
        let watchdog = recovery
            .find("schedule_content_policy_timeout(")
            .expect("watchdog");
        assert!(rejoin < rotate && rotate < reattest && reattest < watchdog);
        assert!(!recovery.contains("build_owned_agent_view("));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn navigation_revokes_old_semantic_authority_before_other_fallible_native_work() {
        let navigation = include_str!("agent_context.rs")
            .split_once("fn start_owned_agent_navigation(")
            .expect("navigation owner")
            .1
            .split_once("fn start_owned_agent_recovery(")
            .expect("navigation owner end")
            .0;
        let rejoin = navigation.find("binding.join = requested").expect("rejoin");
        let rotate = navigation
            .find("binding.view.prepare_semantic_document_load()")
            .expect("semantic epoch rotation");
        let watchdog = navigation
            .find("schedule_content_policy_timeout(")
            .expect("watchdog");
        let native_gate = navigation
            .find("let armed = match redirect_policy.clone()")
            .expect("native gate");
        assert!(rejoin < rotate && rotate < watchdog && watchdog < native_gate);
    }
}
