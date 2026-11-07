//! Suspended runtime-worker ownership and fail-closed lifecycle shell.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_queue::ArrayQueue;
use thiserror::Error;
use tokio::sync::Notify;
use zephium_agentic::{
    AgentAuditCompletion, AgentAuditDeliverySettlement, AgentBrowserLifecycle, AgentBrowserPort,
    AgentBrowserShutdownOutcome, AgentNativeShutdownProof, AgentProviderShutdownProof,
    AgentRunPolicySettlement, ContextCookieTransferRequest, ContextDispatch, ContextNativeEvent,
    ContextNativeRequest, ContextNavigationReplacement, ContextRendererLoss,
    ContextResourceAuditId, ContextShutdownDispatch, SemanticActionNativeCompletion,
    SemanticActionNativeRequest, SemanticActionNativeSettlement, SemanticRuntimeInvocation,
    SemanticScreenshotNativeCapture, SemanticScreenshotNativeCompletion,
    SemanticScreenshotNativeFailure, SemanticScreenshotNativeRequest,
};

use crate::mailbox::{
    AgentAuditSink, AgentRuntimeMailbox, AgentRuntimeMailboxCleanClaimRefusal,
    AgentRuntimeMailboxItem, AgentRuntimeMailboxWake, SemanticActionSink,
};
use crate::{AgentRuntimeMailboxConfig, AgentRuntimeMailboxFault, NativeEventSink};

mod scoped;
#[cfg(test)]
mod scoped_tests;
pub use scoped::{
    AgentRuntimeScopedBinding, AgentRuntimeScopedBindingRefusal, AgentRuntimeScopedClaim,
    AgentRuntimeScopedCommitRefusal, AgentRuntimeScopedComposition, AgentRuntimeScopedController,
    AgentRuntimeScopedDrain, AgentRuntimeScopedDrained, AgentRuntimeScopedLifecycle,
    PendingScopedAgentRuntime,
};

const RUN_IDLE: u8 = 0;
const RUN_ACTIVE: u8 = 1;
const RUN_SEALED: u8 = 2;
// A controller clean-close claim is deliberately a two-step linearization.
// Cancellation and lifecycle shutdown can turn CLAIMING/CLAIMED into SEALED;
// only the moved proof may commit CLAIMED to SUCCEEDED.
const RUN_CLAIMING: u8 = 3;
const RUN_CLAIMED: u8 = 4;
const RUN_SUCCEEDED: u8 = 5;
const TERMINAL_CLASS_NONE: u8 = 0;
const TERMINAL_CLASS_ORDINARY: u8 = 1;
const TERMINAL_CLASS_CANCELLED: u8 = 2;
const TERMINAL_CLASS_SHUTDOWN: u8 = 3;
const STAGED_STOP_NONE: u8 = 0;
const STAGED_STOP_UNEXPECTED_NATIVE_EVENT: u8 = 1;
const STAGED_STOP_TERMINAL_OVERFLOW: u8 = 2;
const STAGED_STOP_SIGNAL_OVERFLOW: u8 = 3;
const STAGED_STOP_CLOSED_MAILBOX: u8 = 4;
// A spontaneous callback intake fault has no lifecycle caller from which to
// inherit an absolute deadline. It still gets one bounded, controller-visible
// reconciliation window before this host forcibly drops retained authority.
const CONTROLLER_FAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
// Provider TLS (reqwest, rustls, aws-lc) is polled inline on this worker; an
// unoptimized build overflowed the 2 MiB platform default mid-handshake.
const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

static RUNTIME_WORKER_HELD: AtomicBool = AtomicBool::new(false);
static EMERGENCY_WORKER_REAP: Mutex<Vec<RuntimeWorkerOwnership>> = Mutex::new(Vec::new());

#[cfg(test)]
static FORCE_REAPER_SPAWN_FAILURE: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static WORKER_EXIT_GATE: WorkerExitGate = WorkerExitGate::new();
#[cfg(test)]
static WORKER_POST_COMPLETION_GATE: WorkerExitGate = WorkerExitGate::new();

/// Minimum bounded control-command capacity for one staged run admission.
pub const MIN_AGENT_RUNTIME_COMMAND_CAPACITY: usize = 1;
/// Largest bounded control-command capacity accepted by this shell.
pub const MAX_AGENT_RUNTIME_COMMAND_CAPACITY: usize = 32;

/// Fixed configuration for one shell-owned agent runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentRuntimeConfig {
    mailbox: AgentRuntimeMailboxConfig,
    command_capacity: usize,
}

impl AgentRuntimeConfig {
    /// Conservative fixed configuration suitable for one production runtime.
    pub const STANDARD: Self = Self {
        mailbox: AgentRuntimeMailboxConfig::STANDARD,
        command_capacity: 4,
    };

    /// Creates one fixed, nonzero mailbox and command capacity configuration.
    pub const fn try_new(
        mailbox: AgentRuntimeMailboxConfig,
        command_capacity: usize,
    ) -> Option<Self> {
        if command_capacity < MIN_AGENT_RUNTIME_COMMAND_CAPACITY
            || command_capacity > MAX_AGENT_RUNTIME_COMMAND_CAPACITY
        {
            None
        } else {
            Some(Self {
                mailbox,
                command_capacity,
            })
        }
    }

    /// Native terminal/signal mailbox capacity configuration.
    pub const fn mailbox(self) -> AgentRuntimeMailboxConfig {
        self.mailbox
    }

    /// Maximum locally admitted control commands retained by the worker.
    pub const fn command_capacity(self) -> usize {
        self.command_capacity
    }
}

/// Content-free failure while starting the suspended worker.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RuntimeSpawnError {
    /// A scoped group does not match the work or its fixed capacity.
    #[error("agent runtime group does not admit this worker")]
    Group,
    /// Another Zephium agent runtime worker has not yet exited.
    #[error("an agent runtime worker is already running")]
    AlreadyRunning,
    /// The worker thread or current-thread Tokio runtime could not start.
    #[error("agent runtime worker is unavailable")]
    WorkerUnavailable,
}

/// Content-free reason this staged shell fail-stopped before controller wiring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentRuntimeStagedStopReason {
    /// A native callback arrived even though this shell dispatched no work.
    UnexpectedNativeEvent,
    /// An obligated native callback could not enter its reserved lane.
    TerminalMailboxOverflow,
    /// An unsolicited native signal could not enter its lane.
    SignalMailboxOverflow,
    /// Callback intake closed before the staged worker could reconcile it.
    ClosedMailbox,
}

impl AgentRuntimeStagedStopReason {
    const fn code(self) -> u8 {
        match self {
            Self::UnexpectedNativeEvent => STAGED_STOP_UNEXPECTED_NATIVE_EVENT,
            Self::TerminalMailboxOverflow => STAGED_STOP_TERMINAL_OVERFLOW,
            Self::SignalMailboxOverflow => STAGED_STOP_SIGNAL_OVERFLOW,
            Self::ClosedMailbox => STAGED_STOP_CLOSED_MAILBOX,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            STAGED_STOP_UNEXPECTED_NATIVE_EVENT => Some(Self::UnexpectedNativeEvent),
            STAGED_STOP_TERMINAL_OVERFLOW => Some(Self::TerminalMailboxOverflow),
            STAGED_STOP_SIGNAL_OVERFLOW => Some(Self::SignalMailboxOverflow),
            STAGED_STOP_CLOSED_MAILBOX => Some(Self::ClosedMailbox),
            _ => None,
        }
    }

    const fn from_mailbox_fault(fault: AgentRuntimeMailboxFault) -> Self {
        match fault {
            AgentRuntimeMailboxFault::Closed => Self::ClosedMailbox,
            AgentRuntimeMailboxFault::TerminalOverflow => Self::TerminalMailboxOverflow,
            AgentRuntimeMailboxFault::SignalOverflow => Self::SignalMailboxOverflow,
        }
    }
}

/// Strictly increasing, opaque local admission identity for one run.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AgentRunTicket(NonZeroU64);

impl AgentRunTicket {
    /// Numeric process-local identity for exact runtime joins.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for AgentRunTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentRunTicket([redacted])")
    }
}

/// Closed refusal when this staged shell cannot admit another run.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentRunAdmissionRefusal {
    /// A run is already active; the shell deliberately admits one at a time.
    #[error("an agent run is already active")]
    Active,
    /// Cancellation/shutdown permanently sealed run admission.
    #[error("agent runtime run admission is sealed")]
    Sealed,
    /// The fixed worker command mailbox is full.
    #[error("agent runtime command mailbox capacity was exhausted")]
    Capacity,
    /// Ticket identity exhausted and therefore sealed further admissions.
    #[error("agent runtime ticket identity was exhausted")]
    TicketExhausted,
}

/// Content-free projection of staged runtime state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentRunStatus {
    admitted: bool,
    cancelled: bool,
    sealed: bool,
    mailbox_fault: Option<AgentRuntimeMailboxFault>,
    staged_stop_reason: Option<AgentRuntimeStagedStopReason>,
}

impl AgentRunStatus {
    /// Whether the one staged run slot is currently admitted.
    pub const fn admitted(self) -> bool {
        self.admitted
    }

    /// Whether cancellation was requested for the exact current run tree.
    pub const fn cancelled(self) -> bool {
        self.cancelled
    }

    /// Whether future run admission is permanently sealed.
    pub const fn sealed(self) -> bool {
        self.sealed
    }

    /// Any sticky native callback intake fault.
    pub const fn mailbox_fault(self) -> Option<AgentRuntimeMailboxFault> {
        self.mailbox_fault
    }

    /// Why this pre-controller shell fail-stopped, if an unexpected callback arrived.
    pub const fn staged_stop_reason(self) -> Option<AgentRuntimeStagedStopReason> {
        self.staged_stop_reason
    }
}

/// One controller future, constructed and polled only by the named runtime worker.
///
/// This intentionally has no `Send` bound. The controller and its future never
/// cross to another task or executor after construction: the runtime owns one
/// current-thread Tokio worker for its complete lifetime.
pub type AgentRuntimeControllerFuture = Pin<Box<dyn Future<Output = ()> + 'static>>;

/// Affine browser capability owned by one runtime controller.
///
/// This wrapper is deliberately neither cloneable nor transferable to another
/// thread. It exposes the existing closed browser operations without exposing
/// the reference-counted port, so deadline-bounded lifecycle shutdown can
/// drop the controller future and release the browser on the named worker
/// before its permit is released. It is useful only to the controller future
/// receiving it from [`AgentRuntimeController::run`].
pub struct AgentRuntimeBrowser {
    port: Arc<dyn AgentBrowserPort>,
    not_send: PhantomData<Rc<()>>,
}

impl AgentRuntimeBrowser {
    fn new(port: Arc<dyn AgentBrowserPort>) -> Self {
        Self {
            port,
            not_send: PhantomData,
        }
    }

    /// Attempts to admit one exact lifecycle request to the bound browser.
    pub fn dispatch(&self, request: ContextNativeRequest) -> ContextDispatch {
        self.port.dispatch(request)
    }

    /// Attempts one bounded native-only cookie transfer.
    pub fn transfer_cookies(&self, request: ContextCookieTransferRequest) -> ContextDispatch {
        self.port.transfer_cookies(request)
    }

    /// Attempts one privacy-preserving native resource audit.
    pub fn audit_resources(&self, audit: ContextResourceAuditId) -> ContextDispatch {
        self.port.audit_resources(audit)
    }

    /// Permanently seals native admission and attempts the exact shutdown audit.
    pub fn seal_for_shutdown(&self, audit: ContextResourceAuditId) -> ContextShutdownDispatch {
        self.port.seal_for_shutdown(audit)
    }

    /// Attempts one already-encoded fixed semantic-runtime invocation.
    pub fn invoke_semantic(&self, invocation: SemanticRuntimeInvocation) -> ContextDispatch {
        self.port.invoke_semantic(invocation)
    }

    /// Attempts one already-authorized closed semantic action recipe.
    pub fn execute_semantic_action(
        &self,
        request: SemanticActionNativeRequest,
        completion: SemanticActionNativeCompletion,
    ) -> ContextDispatch {
        self.port.execute_semantic_action(request, completion)
    }

    /// Attempts one bounded viewport capture outside the cloneable event bus.
    pub fn capture_semantic_screenshot(
        &self,
        request: SemanticScreenshotNativeRequest,
        completion: SemanticScreenshotNativeCompletion,
    ) -> ContextDispatch {
        self.port.capture_semantic_screenshot(request, completion)
    }
}

impl fmt::Debug for AgentRuntimeBrowser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentRuntimeBrowser([capability, redacted])")
    }
}

/// Narrow controller seam for the one process-scoped agent runtime run.
///
/// A controller receives no runtime internals. It can wait for closed native
/// event vocabulary through [`AgentRuntimeWorker`] and use the affine bound
/// browser capability only through its existing closed operations. Returning
/// from this future, or unwinding while constructing or polling it, fail-closes
/// the one run and produces only an unclean lifecycle outcome.
///
/// This is deliberately not an application orchestration API and does not
/// offer multi-run reuse. One runtime owns exactly one process-scoped run; all
/// cancellation permanently seals further admission.
pub trait AgentRuntimeController: Send + 'static {
    /// Constructs the controller future on the runtime's named worker.
    fn run(
        self: Box<Self>,
        worker: AgentRuntimeWorker,
        browser: AgentRuntimeBrowser,
    ) -> AgentRuntimeControllerFuture;
}

enum RuntimeController {
    Legacy(Box<dyn AgentRuntimeController>),
    Scoped(Box<dyn AgentRuntimeScopedController>),
}

impl RuntimeController {
    fn run(
        self,
        worker: AgentRuntimeWorker,
        binding: StartupBinding,
    ) -> Option<AgentRuntimeControllerFuture> {
        match (self, binding) {
            (Self::Legacy(controller), StartupBinding::Browser(port)) => {
                Some(controller.run(worker, AgentRuntimeBrowser::new(port)))
            }
            (Self::Scoped(controller), StartupBinding::Scoped) => Some(controller.run(worker)),
            _ => None,
        }
    }
}

/// Closed event vocabulary delivered to one runtime controller.
///
/// Control and cancellation/shutdown observations take priority over all
/// native work. Terminal callback settlements take priority over unsolicited
/// signals. The values remain in their existing typed contracts; this enum
/// does not provide string, queue, handle, or platform escape hatches.
pub enum AgentRuntimeEvent {
    /// The sole run admission that was queued for this runtime.
    RunStarted(AgentRunTicket),
    /// A terminal lifecycle or semantic-runtime settlement from the native port.
    NativeTerminal(ContextNativeEvent),
    /// A terminal settlement from an exact semantic native action callback.
    SemanticActionTerminal(SemanticActionNativeSettlement),
    /// A terminal result from one exact native viewport-capture callback.
    SemanticScreenshotTerminal(
        Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>,
    ),
    /// A terminal settlement from an exact durable audit callback.
    AuditTerminal(AgentAuditDeliverySettlement),
    /// A page-initiated or otherwise unsolicited navigation replacement.
    NavigationReplaced(ContextNavigationReplacement),
    /// A native renderer-loss signal.
    RendererLost(ContextRendererLoss),
    /// Cancellation permanently sealed the sole process-scoped run.
    CancellationRequested,
    /// Lifecycle shutdown sealed the run while terminal callback intake drains.
    ShutdownRequested,
}

/// Trusted product reason for permanently revoking this run's automation.
/// A request is not a takeover/suspension acknowledgement: the controller
/// must reconcile dispatched work before a host can transfer control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum AgentRuntimeStopReason {
    /// The user cancelled this run.
    Cancelled = 1,
    /// The user requested exclusive human control.
    HumanTakeover = 2,
    /// The product requested resource suspension.
    Suspend = 3,
    /// Trusted policy authority was revoked.
    PolicyRevoked = 4,
}

impl AgentRuntimeStopReason {
    fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Cancelled),
            2 => Some(Self::HumanTakeover),
            3 => Some(Self::Suspend),
            4 => Some(Self::PolicyRevoked),
            _ => None,
        }
    }
}

impl fmt::Debug for AgentRuntimeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::RunStarted(_) => "RunStarted",
            Self::NativeTerminal(_) => "NativeTerminal",
            Self::SemanticActionTerminal(_) => "SemanticActionTerminal",
            Self::SemanticScreenshotTerminal(_) => "SemanticScreenshotTerminal",
            Self::AuditTerminal(_) => "AuditTerminal",
            Self::NavigationReplaced(_) => "NavigationReplaced",
            Self::RendererLost(_) => "RendererLost",
            Self::CancellationRequested => "CancellationRequested",
            Self::ShutdownRequested => "ShutdownRequested",
        };
        formatter.write_str(label)
    }
}

/// Content-free fatal mailbox condition observed by a controller capability.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentRuntimeWorkerFault {
    /// A native callback lane could no longer safely retain its obligations.
    #[error("agent runtime worker callback intake failed")]
    Mailbox(AgentRuntimeMailboxFault),
}

/// Content-free reason a controller could not atomically claim success.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentRuntimeControllerTerminalRefusal {
    /// The requested proof class does not match the runtime's frozen scope.
    #[error("agent runtime controller terminal claim has the wrong scope")]
    Scope,
    /// The run was no longer in its sole active controller state.
    #[error("agent runtime controller terminal claim is no longer active")]
    Inactive,
    /// Cancellation was observed before the claim linearized.
    #[error("agent runtime controller terminal claim was cancelled")]
    Cancelled,
    /// Lifecycle shutdown was observed before the claim linearized.
    #[error("agent runtime controller terminal claim was shut down")]
    Shutdown,
    /// Callback intake faulted while the claim was closing ingress.
    #[error("agent runtime controller terminal claim observed a mailbox fault")]
    MailboxFault,
    /// A terminal callback remained queued at the claim point.
    #[error("agent runtime controller terminal claim observed terminal callback debt")]
    TerminalDebt,
    /// An unsolicited native signal remained queued at the claim point.
    #[error("agent runtime controller terminal claim observed native signal debt")]
    SignalDebt,
    /// A control command remained queued at the claim point.
    #[error("agent runtime controller terminal claim observed control debt")]
    ControlDebt,
}

/// Exact control-state class expected by one clean controller terminal claim.
///
/// The controller's business outcome remains its own closed vocabulary. This
/// class only prevents a cancelled or lifecycle-sealed runtime from being
/// misreported as an ordinary quiescent turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentRuntimeControllerTerminalClass {
    /// No runtime cancellation or lifecycle shutdown was observed.
    Ordinary,
    /// The controller drained after one user cancellation.
    Cancelled,
    /// The controller drained during lifecycle shutdown.
    Shutdown,
}

impl AgentRuntimeControllerTerminalClass {
    const fn code(self) -> u8 {
        match self {
            Self::Ordinary => TERMINAL_CLASS_ORDINARY,
            Self::Cancelled => TERMINAL_CLASS_CANCELLED,
            Self::Shutdown => TERMINAL_CLASS_SHUTDOWN,
        }
    }

    const fn from_code(code: u8) -> Option<Self> {
        match code {
            TERMINAL_CLASS_ORDINARY => Some(Self::Ordinary),
            TERMINAL_CLASS_CANCELLED => Some(Self::Cancelled),
            TERMINAL_CLASS_SHUTDOWN => Some(Self::Shutdown),
            _ => None,
        }
    }
}

/// Move-only proof that the current runtime accepted a clean controller close.
///
/// A business-only commit is not lifecycle success. A lifecycle commit also
/// consumes independent native and policy/audit proofs. Dropping an
/// uncommitted claim revokes it.
#[must_use]
pub struct AgentRuntimeControllerTerminalClaim {
    inner: Arc<RuntimeInner>,
    ticket: AgentRunTicket,
    committed: bool,
}

impl AgentRuntimeControllerTerminalClaim {
    /// Returns the exact sole run ticket bound to this claim.
    pub const fn ticket(&self) -> AgentRunTicket {
        self.ticket
    }

    /// Commits the previously linearized clean controller terminal claim.
    pub fn commit(mut self) {
        self.commit_inner(None, None);
    }

    /// Commits terminal policy/audit consumption and exact native shutdown.
    ///
    /// The runtime retains the move-only native proof until its worker returns
    /// normally and is joined. Timeout, unwind, callback debt or mailbox fault
    /// still preclude clean lifecycle shutdown. The policy settlement is
    /// constructor-closed evidence, not permission to run another task.
    pub fn commit_with_shutdown(
        mut self,
        native: AgentNativeShutdownProof,
        policy: AgentRunPolicySettlement,
        provider: AgentProviderShutdownProof,
    ) {
        self.commit_inner(
            Some(RuntimeShutdownClosure {
                native,
                _policy: policy,
                _provider: provider,
            }),
            None,
        );
    }

    fn commit_inner(
        &mut self,
        closure: Option<RuntimeShutdownClosure>,
        scoped: Option<scoped::ScopedClosure>,
    ) {
        let _gate = recover_lock(&self.inner.terminal_claim_gate);
        // `self` is the sole move-only owner created by CLAIMING->CLAIMED.
        // Lifecycle control cannot revoke CLAIMED and no other API can create
        // or consume this proof, so this is an infallible typestate edge.
        *recover_lock(&self.inner.shutdown_closure) = closure;
        *recover_lock(&self.inner.scoped_closure) = scoped;
        self.inner.run_state.store(RUN_SUCCEEDED, Ordering::Release);
        self.inner
            .terminal_claim_class
            .store(TERMINAL_CLASS_NONE, Ordering::Release);
        self.committed = true;
    }
}

impl Drop for AgentRuntimeControllerTerminalClaim {
    fn drop(&mut self) {
        if !self.committed {
            self.inner.fail_success_claim();
        }
    }
}

impl fmt::Debug for AgentRuntimeControllerTerminalClaim {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentRuntimeControllerTerminalClaim([move-only, redacted])")
    }
}

/// Move-only capability for the one controller running on the named worker.
///
/// It intentionally exposes neither worker queues nor scheduler primitives,
/// runtime state ownership, platform handles, provider/store ports, nor
/// content-string APIs. It is useful only to the controller future receiving
/// it from [`AgentRuntimeController::run`].
pub struct AgentRuntimeWorker {
    inner: Arc<RuntimeInner>,
    cancellation_delivered: bool,
    shutdown_delivered: bool,
    // A controller future is intentionally affine to the one current-thread
    // Tokio worker. This marker prevents moving the capability into a spawned
    // task or another executor even though its private bookkeeping is Arc'd.
    not_send: PhantomData<Rc<()>>,
}

impl AgentRuntimeWorker {
    /// The first trusted product stop request, if one was admitted.
    pub fn stop_reason(&self) -> Option<AgentRuntimeStopReason> {
        AgentRuntimeStopReason::from_code(self.inner.stop_reason.load(Ordering::Acquire))
    }
    /// Waits without a lost-wake race for the next closed controller event.
    ///
    /// Shutdown and cancellation win over every other event once, then a
    /// queued run admission, then terminal callbacks, and finally unsolicited
    /// signals. Their sticky state remains visible through [`Self::status`]
    /// without repeatedly hiding already-obligated terminal debt. A sticky
    /// mailbox fault requests bounded cooperative shutdown before it is
    /// returned, so terminal debt remains observable until final fail-closed
    /// draining.
    pub async fn next_event(&mut self) -> Result<AgentRuntimeEvent, AgentRuntimeWorkerFault> {
        loop {
            let mut notified = std::pin::pin!(self.inner.control_wake.notified());
            notified.as_mut().enable();

            if self.inner.shutdown_requested.load(Ordering::Acquire) && !self.shutdown_delivered {
                self.shutdown_delivered = true;
                return Ok(AgentRuntimeEvent::ShutdownRequested);
            }
            if self.inner.cancelled.load(Ordering::Acquire)
                && !self.cancellation_delivered
                && !self.shutdown_delivered
            {
                // Cooperative shutdown publishes this flag before cancellation,
                // so a controller that observes cancellation from lifecycle
                // teardown must re-check and receive the stronger one-shot
                // shutdown event instead.
                if self.inner.shutdown_requested.load(Ordering::Acquire) {
                    continue;
                }
                self.cancellation_delivered = true;
                return Ok(AgentRuntimeEvent::CancellationRequested);
            }
            if !self.inner.cancelled.load(Ordering::Acquire)
                && !self.inner.shutdown_requested.load(Ordering::Acquire)
            {
                if let Some(command) = self.inner.commands.pop() {
                    let WorkerCommand::Start(ticket) = command;
                    return Ok(AgentRuntimeEvent::RunStarted(ticket));
                }
            }
            if let Some(item) = self.inner.mailbox.try_pop_terminal() {
                return Ok(controller_event_from_mailbox(item));
            }
            if let Some(fault) = self.inner.mailbox.fault() {
                // Do not close intake or destroy retained callback debt here.
                // The next turn delivers the stronger shutdown control event;
                // a cooperating controller can then cancel and reconcile its
                // outstanding terminal obligations before the host deadline.
                self.inner.request_fault_shutdown();
                if !self.shutdown_delivered {
                    continue;
                }
                return Err(AgentRuntimeWorkerFault::Mailbox(fault));
            }
            if let Some(item) = self.inner.mailbox.try_pop_signal() {
                return Ok(controller_event_from_mailbox(item));
            }

            let mailbox_wait = self.inner.mailbox.wait_for_work();
            tokio::pin!(mailbox_wait);
            tokio::select! {
                biased;
                _ = &mut notified => {}
                _ = &mut mailbox_wait => {}
            }
        }
    }

    /// Waits for a retained event while reconciling an already-accepted
    /// terminal obligation.
    ///
    /// Unlike [`Self::next_event`], a sticky mailbox fault is observed through
    /// [`Self::status`] but does not preempt queued or future terminal callback
    /// settlement. This narrow cleanup-only API is not a general event loop:
    /// it exists so a controller can drain the exact audit callback it already
    /// transferred before returning opaque recovery.
    pub async fn next_event_for_terminal_cleanup(&mut self) -> AgentRuntimeEvent {
        loop {
            let mut notified = std::pin::pin!(self.inner.control_wake.notified());
            notified.as_mut().enable();
            if self.inner.shutdown_requested.load(Ordering::Acquire) && !self.shutdown_delivered {
                self.shutdown_delivered = true;
                return AgentRuntimeEvent::ShutdownRequested;
            }
            if self.inner.cancelled.load(Ordering::Acquire)
                && !self.cancellation_delivered
                && !self.shutdown_delivered
            {
                if self.inner.shutdown_requested.load(Ordering::Acquire) {
                    continue;
                }
                self.cancellation_delivered = true;
                return AgentRuntimeEvent::CancellationRequested;
            }
            if let Some(item) = self.inner.mailbox.try_pop_terminal() {
                return controller_event_from_mailbox(item);
            }
            if let Some(item) = self.inner.mailbox.try_pop_signal() {
                return controller_event_from_mailbox(item);
            }
            let mailbox_wait = self.inner.mailbox.wait_for_cleanup_work();
            tokio::pin!(mailbox_wait);
            tokio::select! {
                biased;
                _ = &mut notified => {}
                _ = &mut mailbox_wait => {}
            }
        }
    }

    /// Removes one already-retained mailbox item after a terminal claim was
    /// refused. The closed event enum preserves any move-only settlement for
    /// opaque controller recovery; it never exposes a queue or mailbox handle.
    pub fn try_drain_terminal_claim_refusal_event(&mut self) -> Option<AgentRuntimeEvent> {
        self.inner
            .mailbox
            .try_pop()
            .map(controller_event_from_mailbox)
    }

    /// Returns the exact ticket for the sole admitted run, if one exists.
    pub fn current_run_ticket(&self) -> Option<AgentRunTicket> {
        NonZeroU64::new(self.inner.current_ticket.load(Ordering::Acquire)).map(AgentRunTicket)
    }

    /// Returns the content-free current process-scoped run status.
    pub fn status(&self) -> AgentRunStatus {
        self.inner.status()
    }

    /// Returns the absolute host deadline for cooperative shutdown, if any.
    ///
    /// After [`AgentRuntimeEvent::ShutdownRequested`], a controller uses this
    /// only to bound cancellation, callback reconciliation, and audit work;
    /// reaching it does not authorize a clean outcome or a new effect.
    pub fn shutdown_deadline(&self) -> Option<Instant> {
        self.inner.shutdown_deadline()
    }

    /// Creates one exact move-only semantic-action completion callback.
    pub fn semantic_action_completion(&self) -> SemanticActionNativeCompletion {
        self.inner.mailbox.semantic_action_sink().completion()
    }

    /// Creates one exact move-only semantic-screenshot completion callback.
    pub fn semantic_screenshot_completion(&self) -> SemanticScreenshotNativeCompletion {
        self.inner.mailbox.semantic_screenshot_sink().completion()
    }

    /// Creates one exact move-only durable-audit completion callback.
    pub fn audit_completion(&self) -> AgentAuditCompletion {
        self.inner.mailbox.audit_sink().completion()
    }

    /// Atomically closes callback ingress and claims a controller success.
    ///
    /// This waits for callbacks which entered before ingress closed, then
    /// rejects success if cancellation, lifecycle shutdown, a mailbox fault,
    /// a queued terminal settlement, or an unsolicited native signal won the
    /// race. The returned proof must be committed only when the controller's
    /// own policy closure is also irreversible; dropping it fails closed.
    pub async fn try_claim_controller_terminal(
        &mut self,
        class: AgentRuntimeControllerTerminalClass,
    ) -> Result<AgentRuntimeControllerTerminalClaim, AgentRuntimeControllerTerminalRefusal> {
        if self.inner.scope.is_some() {
            return Err(AgentRuntimeControllerTerminalRefusal::Scope);
        }
        self.claim_terminal(class).await
    }

    async fn claim_terminal(
        &mut self,
        class: AgentRuntimeControllerTerminalClass,
    ) -> Result<AgentRuntimeControllerTerminalClaim, AgentRuntimeControllerTerminalRefusal> {
        // Even a control-class refusal is a terminal controller decision: seal
        // ingress and wait preexisting writers before returning it so the
        // caller can synchronously retain every queued move-only settlement.
        let began = self.inner.begin_terminal_claim(class);
        let mailbox = self.inner.mailbox.try_claim_clean_quiescence().await;
        match mailbox {
            Ok(()) => {}
            Err(AgentRuntimeMailboxCleanClaimRefusal::Fault) => {
                self.inner.fail_success_claim();
                return Err(AgentRuntimeControllerTerminalRefusal::MailboxFault);
            }
            Err(AgentRuntimeMailboxCleanClaimRefusal::TerminalDebt) => {
                self.inner.fail_success_claim();
                return Err(AgentRuntimeControllerTerminalRefusal::TerminalDebt);
            }
            Err(AgentRuntimeMailboxCleanClaimRefusal::SignalDebt) => {
                self.inner.fail_success_claim();
                return Err(AgentRuntimeControllerTerminalRefusal::SignalDebt);
            }
        }
        began?;
        if !self.inner.commands.is_empty() {
            self.inner.fail_success_claim();
            return Err(AgentRuntimeControllerTerminalRefusal::ControlDebt);
        }
        self.inner.finish_terminal_claim(class)?;
        let ticket = self.current_run_ticket().ok_or_else(|| {
            self.inner.fail_success_claim();
            AgentRuntimeControllerTerminalRefusal::Inactive
        })?;
        Ok(AgentRuntimeControllerTerminalClaim {
            inner: Arc::clone(&self.inner),
            ticket,
            committed: false,
        })
    }
}

impl fmt::Debug for AgentRuntimeWorker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentRuntimeWorker([capability, redacted])")
    }
}

fn controller_event_from_mailbox(item: AgentRuntimeMailboxItem) -> AgentRuntimeEvent {
    match item {
        AgentRuntimeMailboxItem::NativeTerminal(event) => AgentRuntimeEvent::NativeTerminal(event),
        AgentRuntimeMailboxItem::SemanticActionTerminal(settlement) => {
            AgentRuntimeEvent::SemanticActionTerminal(settlement)
        }
        AgentRuntimeMailboxItem::SemanticScreenshotTerminal(settlement) => {
            AgentRuntimeEvent::SemanticScreenshotTerminal(settlement)
        }
        AgentRuntimeMailboxItem::AuditTerminal(settlement) => {
            AgentRuntimeEvent::AuditTerminal(settlement)
        }
        AgentRuntimeMailboxItem::NavigationReplaced(replacement) => {
            AgentRuntimeEvent::NavigationReplaced(replacement)
        }
        AgentRuntimeMailboxItem::RendererLost(loss) => AgentRuntimeEvent::RendererLost(loss),
        #[cfg(test)]
        AgentRuntimeMailboxItem::TestTerminal | AgentRuntimeMailboxItem::TestSignal => {
            // The public controller vocabulary deliberately has no test-only
            // variants. Test-only mailbox markers are consumed only by the
            // mailbox's own unit tests and cannot reach a production host.
            unreachable!("test-only mailbox marker cannot enter controller host")
        }
    }
}

enum WorkerCommand {
    Start(AgentRunTicket),
}

#[derive(Clone)]
enum StartupBinding {
    Browser(Arc<dyn AgentBrowserPort>),
    Scoped,
}

enum StartupState {
    Suspended,
    Bound(StartupBinding),
    Stop,
}

struct StartupGate {
    state: Mutex<StartupState>,
    changed: Notify,
}

impl StartupGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(StartupState::Suspended),
            changed: Notify::new(),
        }
    }

    fn bind(&self, browser: Arc<dyn AgentBrowserPort>) {
        let mut state = recover_lock(&self.state);
        *state = StartupState::Bound(StartupBinding::Browser(browser));
        self.changed.notify_waiters();
    }

    fn bind_scoped(&self) {
        let mut state = recover_lock(&self.state);
        *state = StartupState::Bound(StartupBinding::Scoped);
        self.changed.notify_waiters();
    }

    fn stop(&self) {
        let mut state = recover_lock(&self.state);
        *state = StartupState::Stop;
        self.changed.notify_waiters();
    }

    async fn wait(&self) -> Option<StartupBinding> {
        loop {
            let mut notified = std::pin::pin!(self.changed.notified());
            notified.as_mut().enable();
            let decision = {
                let state = recover_lock(&self.state);
                match &*state {
                    StartupState::Suspended => None,
                    StartupState::Bound(binding) => Some(Some(binding.clone())),
                    StartupState::Stop => Some(None),
                }
            };
            if let Some(startup) = decision {
                return startup;
            }
            notified.await;
        }
    }
}

struct CompletionState {
    stopped: Mutex<bool>,
    changed: Condvar,
    waker: Mutex<Option<std::task::Waker>>,
}

impl CompletionState {
    fn new() -> Self {
        Self {
            stopped: Mutex::new(false),
            changed: Condvar::new(),
            waker: Mutex::new(None),
        }
    }

    fn mark_stopped(&self) {
        let mut stopped = recover_lock(&self.stopped);
        *stopped = true;
        let waker = recover_lock(&self.waker).take();
        self.changed.notify_all();
        drop(stopped);
        Self::wake(waker);
    }

    fn set_waker(&self, waker: std::task::Waker) {
        let stopped = recover_lock(&self.stopped);
        if *stopped {
            drop(stopped);
            Self::wake(Some(waker));
        } else {
            *recover_lock(&self.waker) = Some(waker);
        }
    }

    fn wake(waker: Option<std::task::Waker>) {
        if let Some(waker) = waker {
            let _notified = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| waker.wake()));
        }
    }

    fn is_stopped(&self) -> bool {
        *recover_lock(&self.stopped)
    }

    fn wait_until(&self, deadline: Instant) -> bool {
        let mut stopped = recover_lock(&self.stopped);
        while !*stopped {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let timeout = deadline.saturating_duration_since(now);
            let result = match self.changed.wait_timeout(stopped, timeout) {
                Ok(result) => result,
                Err(error) => error.into_inner(),
            };
            stopped = result.0;
            if result.1.timed_out() && !*stopped {
                return false;
            }
        }
        true
    }
}

struct RuntimeShutdownClosure {
    native: AgentNativeShutdownProof,
    _policy: AgentRunPolicySettlement,
    _provider: AgentProviderShutdownProof,
}

struct RuntimeInner {
    scope: Option<AgentRuntimeScopedBinding>,
    scoped_closure: Mutex<Option<scoped::ScopedClosure>>,
    mailbox: AgentRuntimeMailbox,
    commands: ArrayQueue<WorkerCommand>,
    control_wake: Notify,
    run_state: AtomicU8,
    next_ticket: AtomicU64,
    current_ticket: AtomicU64,
    cancelled: AtomicBool,
    stop_reason: AtomicU8,
    shutdown_requested: AtomicBool,
    shutdown_deadline: Mutex<Option<Instant>>,
    // Serializes the final CLAIMING->CLAIMED transition against lifecycle
    // cancellation/shutdown publication. Once CLAIMED, the returned move-only
    // proof is the clean-terminal linearization point.
    terminal_claim_gate: Mutex<()>,
    terminal_claim_class: AtomicU8,
    // One optional content-free terminal record; no background work or queue.
    shutdown_closure: Mutex<Option<RuntimeShutdownClosure>>,
    controller_returned: AtomicBool,
    fault_shutdown_requested: AtomicBool,
    staged_stop_reason: AtomicU8,
    completion: CompletionState,
    joined: Arc<WorkerJoinCompletion>,
}

impl RuntimeInner {
    fn status(&self) -> AgentRunStatus {
        let run_state = self.run_state.load(Ordering::Acquire);
        AgentRunStatus {
            admitted: run_state == RUN_ACTIVE
                || run_state == RUN_CLAIMING
                || run_state == RUN_CLAIMED,
            cancelled: self.cancelled.load(Ordering::Acquire),
            sealed: run_state != RUN_IDLE && run_state != RUN_ACTIVE
                || self.shutdown_requested.load(Ordering::Acquire),
            mailbox_fault: self.mailbox.fault(),
            staged_stop_reason: AgentRuntimeStagedStopReason::from_code(
                self.staged_stop_reason.load(Ordering::Acquire),
            ),
        }
    }

    fn seal_and_cancel(&self) {
        self.seal_and_stop(AgentRuntimeStopReason::Cancelled);
    }

    fn seal_and_stop(&self, reason: AgentRuntimeStopReason) {
        let _gate = recover_lock(&self.terminal_claim_gate);
        if self.seal_for_control(false) {
            // First admitted stop wins; later requests cannot rewrite history.
            let _ = self.stop_reason.compare_exchange(
                0,
                reason as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            self.cancelled.store(true, Ordering::Release);
        }
        self.control_wake.notify_waiters();
    }

    fn request_cooperative_shutdown_until(&self, deadline: Instant) {
        let _gate = recover_lock(&self.terminal_claim_gate);
        let mut retained_deadline = recover_lock(&self.shutdown_deadline);
        match *retained_deadline {
            Some(current) if current <= deadline => {}
            _ => *retained_deadline = Some(deadline),
        }
        if matches!(
            self.run_state.load(Ordering::Acquire),
            RUN_CLAIMED | RUN_SUCCEEDED
        ) {
            // A moved terminal proof already linearized the run's business
            // outcome, but the host must still retain an absolute reap bound
            // in case a malicious controller holds that proof and never
            // returns. Do not rewrite semantic shutdown/cancel flags here.
            drop(retained_deadline);
            self.control_wake.notify_waiters();
            return;
        }
        if self.seal_for_control(true) {
            self.shutdown_requested.store(true, Ordering::Release);
            self.cancelled.store(true, Ordering::Release);
        }
        drop(retained_deadline);
        self.control_wake.notify_waiters();
    }

    fn request_fault_shutdown(&self) {
        if self
            .fault_shutdown_requested
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let now = Instant::now();
        let deadline = now
            .checked_add(CONTROLLER_FAULT_DRAIN_TIMEOUT)
            .unwrap_or(now);
        self.request_cooperative_shutdown_until(deadline);
    }

    fn shutdown_deadline(&self) -> Option<Instant> {
        *recover_lock(&self.shutdown_deadline)
    }

    fn request_final_shutdown(&self) {
        if self.clean_terminal_claimed() {
            self.mailbox.close_clean();
            return;
        }
        self.request_cooperative_shutdown_until(Instant::now());
        self.mailbox.close();
    }

    async fn close_and_drain_for_finalization(&self) {
        if self.clean_terminal_claimed() {
            self.mailbox.close_and_drain_clean().await;
        } else {
            self.mailbox.close_and_drain().await;
        }
    }

    fn clean_terminal_claimed(&self) -> bool {
        matches!(
            self.run_state.load(Ordering::Acquire),
            RUN_CLAIMED | RUN_SUCCEEDED
        )
    }

    fn seal_for_control(&self, shutdown: bool) -> bool {
        loop {
            let current = self.run_state.load(Ordering::Acquire);
            match current {
                RUN_IDLE | RUN_ACTIVE => match self.run_state.compare_exchange(
                    current,
                    RUN_SEALED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => return true,
                    Err(_) => continue,
                },
                RUN_CLAIMING => match AgentRuntimeControllerTerminalClass::from_code(
                    self.terminal_claim_class.load(Ordering::Acquire),
                ) {
                    // Repeating the same cancellation class cannot revoke an
                    // already-linearizing clean cancelled terminal. Lifecycle
                    // shutdown is stronger and deliberately does revoke it.
                    Some(AgentRuntimeControllerTerminalClass::Cancelled) if !shutdown => {
                        return true;
                    }
                    Some(AgentRuntimeControllerTerminalClass::Shutdown) => return true,
                    _ => match self.run_state.compare_exchange(
                        RUN_CLAIMING,
                        RUN_SEALED,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    ) {
                        Ok(_) => return true,
                        Err(_) => continue,
                    },
                },
                RUN_SEALED => return true,
                // A late user cancellation loses to the terminal proof. A
                // lifecycle shutdown still publishes its force-drop deadline
                // while preserving the proof's eventual commit.
                RUN_CLAIMED => return shutdown,
                RUN_SUCCEEDED => return false,
                _ => return false,
            }
        }
    }

    fn begin_terminal_claim(
        &self,
        class: AgentRuntimeControllerTerminalClass,
    ) -> Result<(), AgentRuntimeControllerTerminalRefusal> {
        let expected = match class {
            AgentRuntimeControllerTerminalClass::Ordinary => {
                if self.shutdown_requested.load(Ordering::Acquire) {
                    return Err(AgentRuntimeControllerTerminalRefusal::Shutdown);
                }
                if self.cancelled.load(Ordering::Acquire) {
                    return Err(AgentRuntimeControllerTerminalRefusal::Cancelled);
                }
                RUN_ACTIVE
            }
            AgentRuntimeControllerTerminalClass::Cancelled => {
                if self.shutdown_requested.load(Ordering::Acquire) {
                    return Err(AgentRuntimeControllerTerminalRefusal::Shutdown);
                }
                if !self.cancelled.load(Ordering::Acquire) {
                    return Err(AgentRuntimeControllerTerminalRefusal::Inactive);
                }
                RUN_SEALED
            }
            AgentRuntimeControllerTerminalClass::Shutdown => {
                if !self.shutdown_requested.load(Ordering::Acquire) {
                    return Err(AgentRuntimeControllerTerminalRefusal::Inactive);
                }
                RUN_SEALED
            }
        };
        self.terminal_claim_class
            .store(class.code(), Ordering::Release);
        self.run_state
            .compare_exchange(expected, RUN_CLAIMING, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| {
                self.terminal_claim_class
                    .store(TERMINAL_CLASS_NONE, Ordering::Release);
                self.terminal_refusal()
            })
    }

    fn fail_success_claim(&self) {
        let _ = self.run_state.compare_exchange(
            RUN_CLAIMING,
            RUN_SEALED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.terminal_claim_class
            .store(TERMINAL_CLASS_NONE, Ordering::Release);
        let _ = self.run_state.compare_exchange(
            RUN_CLAIMED,
            RUN_SEALED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.control_wake.notify_waiters();
    }

    fn finish_terminal_claim(
        &self,
        class: AgentRuntimeControllerTerminalClass,
    ) -> Result<(), AgentRuntimeControllerTerminalRefusal> {
        let _gate = recover_lock(&self.terminal_claim_gate);
        match class {
            AgentRuntimeControllerTerminalClass::Ordinary
                if self.shutdown_requested.load(Ordering::Acquire) =>
            {
                self.fail_success_claim();
                return Err(AgentRuntimeControllerTerminalRefusal::Shutdown);
            }
            AgentRuntimeControllerTerminalClass::Ordinary
                if self.cancelled.load(Ordering::Acquire) =>
            {
                self.fail_success_claim();
                return Err(AgentRuntimeControllerTerminalRefusal::Cancelled);
            }
            AgentRuntimeControllerTerminalClass::Cancelled
                if self.shutdown_requested.load(Ordering::Acquire)
                    || !self.cancelled.load(Ordering::Acquire) =>
            {
                self.fail_success_claim();
                return Err(self.terminal_refusal());
            }
            AgentRuntimeControllerTerminalClass::Shutdown
                if !self.shutdown_requested.load(Ordering::Acquire) =>
            {
                self.fail_success_claim();
                return Err(self.terminal_refusal());
            }
            _ => {}
        }
        self.run_state
            .compare_exchange(
                RUN_CLAIMING,
                RUN_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| self.terminal_refusal())
    }

    fn terminal_refusal(&self) -> AgentRuntimeControllerTerminalRefusal {
        if self.shutdown_requested.load(Ordering::Acquire) {
            AgentRuntimeControllerTerminalRefusal::Shutdown
        } else if self.cancelled.load(Ordering::Acquire) {
            AgentRuntimeControllerTerminalRefusal::Cancelled
        } else if self.mailbox.fault().is_some() {
            AgentRuntimeControllerTerminalRefusal::MailboxFault
        } else {
            AgentRuntimeControllerTerminalRefusal::Inactive
        }
    }

    fn record_staged_stop(&self, reason: AgentRuntimeStagedStopReason) {
        let _ = self.staged_stop_reason.compare_exchange(
            STAGED_STOP_NONE,
            reason.code(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Cloneable observer for logical worker-loop completion, not OS-thread exit.
#[derive(Clone)]
pub struct AgentRuntimeCompletion {
    inner: Arc<RuntimeInner>,
}

impl AgentRuntimeCompletion {
    /// Registers one replaceable content-free application wake. Registration
    /// racing logical completion cannot lose the wake; it is not a lifecycle proof.
    /// The waker must enqueue work without waiting for this worker to join.
    pub fn set_waker(&self, waker: std::task::Waker) {
        self.inner.completion.set_waker(waker);
    }
    /// Whether the worker loop stopped and published its completion signal.
    /// Runtime/thread-local teardown can remain; only lifecycle join proves exit.
    pub fn is_stopped(&self) -> bool {
        self.inner.completion.is_stopped()
    }
}

impl fmt::Debug for AgentRuntimeCompletion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentRuntimeCompletion")
            .field("stopped", &self.is_stopped())
            .finish()
    }
}

/// Cloneable shell handle for admission, cancellation, and callback sinks.
#[derive(Clone)]
pub struct AgentRuntimeHandle {
    inner: Arc<RuntimeInner>,
}

impl AgentRuntimeHandle {
    /// Admits the sole staged run slot and returns its opaque ticket.
    pub fn start_run(&self) -> Result<AgentRunTicket, AgentRunAdmissionRefusal> {
        match self.inner.run_state.compare_exchange(
            RUN_IDLE,
            RUN_ACTIVE,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {}
            Err(RUN_ACTIVE) => return Err(AgentRunAdmissionRefusal::Active),
            Err(_) => return Err(AgentRunAdmissionRefusal::Sealed),
        }

        let ticket = match allocate_ticket(&self.inner.next_ticket) {
            Some(ticket) => ticket,
            None => {
                self.inner.run_state.store(RUN_SEALED, Ordering::Release);
                return Err(AgentRunAdmissionRefusal::TicketExhausted);
            }
        };
        self.inner
            .current_ticket
            .store(ticket.get(), Ordering::Release);
        if self
            .inner
            .commands
            .push(WorkerCommand::Start(ticket))
            .is_err()
        {
            let _ = self.inner.run_state.compare_exchange(
                RUN_ACTIVE,
                RUN_IDLE,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            self.inner.current_ticket.store(0, Ordering::Release);
            return Err(AgentRunAdmissionRefusal::Capacity);
        }
        // Shared control has nested lifecycle/watchdog and controller event
        // waiters. An older lifecycle waiter can remain enabled while the
        // controller runs, so waking only one can strand this queued Start.
        // Every consumer registers before inspecting durable control state;
        // broadcast makes that state visible without relying on a wake permit.
        self.inner.control_wake.notify_waiters();
        Ok(ticket)
    }

    /// Permanently seals future admission and requests cancellation.
    pub fn cancel_and_seal(&self) {
        self.inner.seal_and_cancel();
    }

    /// Requests trusted revocation without granting native takeover authority.
    pub fn stop_and_seal(&self, reason: AgentRuntimeStopReason) {
        self.inner.seal_and_stop(reason);
    }

    /// Returns a content-free projection of the staged shell state.
    pub fn status(&self) -> AgentRunStatus {
        self.inner.status()
    }
}

impl fmt::Debug for AgentRuntimeHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentRuntimeHandle")
            .field("status", &self.status())
            .finish()
    }
}

/// Move-only suspended runtime before the browser port is transferred.
///
/// The native event sink is available in this state while no browser port has
/// entered the runtime. Because this staged shell dispatches no native work,
/// any callback received before or after binding is consumed, recorded as an
/// unexpected event, and fail-stops the worker rather than becoming orphaned
/// work. A later controller replaces that deliberate safety boundary.
pub struct PendingAgentRuntime {
    inner: Arc<RuntimeInner>,
    gate: Arc<StartupGate>,
    worker: Option<RuntimeWorkerOwnership>,
    bound: bool,
}

impl PendingAgentRuntime {
    /// Starts one named current-thread Tokio worker behind a startup gate.
    pub fn spawn_suspended(config: AgentRuntimeConfig) -> Result<Self, RuntimeSpawnError> {
        Self::spawn_suspended_inner(config, None, None, None)
    }

    /// Starts the suspended worker with one controller moved to that worker.
    ///
    /// The controller is not constructed or polled until
    /// [`Self::bind_browser_port`] transfers the exact browser port and opens
    /// the startup gate. The ordinary [`Self::spawn_suspended`] constructor
    /// remains the safe staged default.
    pub fn spawn_suspended_with_controller(
        config: AgentRuntimeConfig,
        controller: Box<dyn AgentRuntimeController>,
    ) -> Result<Self, RuntimeSpawnError> {
        Self::spawn_suspended_inner(
            config,
            Some(RuntimeController::Legacy(controller)),
            None,
            None,
        )
    }

    fn spawn_suspended_inner(
        config: AgentRuntimeConfig,
        controller: Option<RuntimeController>,
        scope: Option<AgentRuntimeScopedBinding>,
        group: Option<&AgentRuntimeWorkerGroup>,
    ) -> Result<Self, RuntimeSpawnError> {
        let permit = match group {
            Some(group) => group.acquire(scope.as_ref().ok_or(RuntimeSpawnError::Group)?)?,
            None => acquire_worker_permit()?,
        };
        let inner = Arc::new(RuntimeInner {
            scope,
            scoped_closure: Mutex::new(None),
            mailbox: AgentRuntimeMailbox::new(config.mailbox),
            commands: ArrayQueue::new(config.command_capacity),
            control_wake: Notify::new(),
            run_state: AtomicU8::new(RUN_IDLE),
            next_ticket: AtomicU64::new(1),
            current_ticket: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            stop_reason: AtomicU8::new(0),
            shutdown_requested: AtomicBool::new(false),
            shutdown_deadline: Mutex::new(None),
            terminal_claim_gate: Mutex::new(()),
            terminal_claim_class: AtomicU8::new(TERMINAL_CLASS_NONE),
            shutdown_closure: Mutex::new(None),
            controller_returned: AtomicBool::new(false),
            fault_shutdown_requested: AtomicBool::new(false),
            staged_stop_reason: AtomicU8::new(STAGED_STOP_NONE),
            completion: CompletionState::new(),
            joined: Arc::new(WorkerJoinCompletion::new()),
        });
        let gate = Arc::new(StartupGate::new());
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let worker_inner = Arc::clone(&inner);
        let worker_gate = Arc::clone(&gate);
        let worker_permit = Arc::clone(&permit);
        let worker = thread::Builder::new()
            .name("zephium-agent-runtime".to_owned())
            .stack_size(WORKER_STACK_BYTES)
            .spawn(move || {
                worker_main(
                    worker_inner,
                    worker_gate,
                    startup_sender,
                    worker_permit,
                    controller,
                )
            })
            .map_err(|_| RuntimeSpawnError::WorkerUnavailable)?;
        let joined = inner.joined.clone();
        let worker = RuntimeWorkerOwnership {
            worker,
            permit,
            joined,
        };
        match startup_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                inner,
                gate,
                worker: Some(worker),
                bound: false,
            }),
            Ok(Err(())) | Err(_) => {
                gate.stop();
                worker.join();
                Err(RuntimeSpawnError::WorkerUnavailable)
            }
        }
    }

    /// Returns the native event sink before ownership of the browser port moves.
    pub fn native_event_sink(&self) -> NativeEventSink {
        self.inner.mailbox.native_event_sink()
    }

    /// Returns the semantic-action callback sink for the in-crate controller.
    #[allow(dead_code)]
    pub(crate) fn semantic_action_sink(&self) -> SemanticActionSink {
        self.inner.mailbox.semantic_action_sink()
    }

    /// Returns the audit-delivery callback sink for the in-crate controller.
    #[allow(dead_code)]
    pub(crate) fn audit_sink(&self) -> AgentAuditSink {
        self.inner.mailbox.audit_sink()
    }

    /// Transfers the browser port exactly once and releases the worker gate.
    pub fn bind_browser_port(
        mut self,
        browser: Arc<dyn AgentBrowserPort>,
    ) -> AgentRuntimeComposition {
        self.gate.bind(browser);
        self.bound = true;
        let handle = AgentRuntimeHandle {
            inner: Arc::clone(&self.inner),
        };
        let completion = AgentRuntimeCompletion {
            inner: Arc::clone(&self.inner),
        };
        let lifecycle = RuntimeLifecycle {
            inner: Arc::clone(&self.inner),
            worker: self.worker.take(),
        };
        AgentRuntimeComposition {
            handle,
            completion,
            lifecycle: Box::new(lifecycle),
        }
    }
}

impl Drop for PendingAgentRuntime {
    fn drop(&mut self) {
        if self.bound {
            return;
        }
        self.gate.stop();
        self.inner.request_final_shutdown();
        if let Some(worker) = self.worker.take() {
            worker.join();
        }
    }
}

/// Completed runtime assembly after exact browser-port transfer.
pub struct AgentRuntimeComposition {
    handle: AgentRuntimeHandle,
    completion: AgentRuntimeCompletion,
    lifecycle: Box<dyn AgentBrowserLifecycle>,
}

impl AgentRuntimeComposition {
    /// Consumes the constructor-closed assembly into its three owned ports.
    pub fn into_parts(
        self,
    ) -> (
        AgentRuntimeHandle,
        AgentRuntimeCompletion,
        Box<dyn AgentBrowserLifecycle>,
    ) {
        (self.handle, self.completion, self.lifecycle)
    }
}

impl fmt::Debug for AgentRuntimeComposition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentRuntimeComposition")
            .field("handle", &self.handle)
            .field("completion", &self.completion)
            .field("lifecycle", &"AgentBrowserLifecycle([redacted])")
            .finish()
    }
}

struct RuntimeLifecycle {
    inner: Arc<RuntimeInner>,
    worker: Option<RuntimeWorkerOwnership>,
}

impl AgentBrowserLifecycle for RuntimeLifecycle {
    fn shutdown_until(mut self: Box<Self>, deadline: Instant) -> AgentBrowserShutdownOutcome {
        if join_worker_until(&self.inner, &mut self.worker, deadline) && self.inner.scope.is_none()
        {
            if let Some(closure) = recover_lock(&self.inner.shutdown_closure).take() {
                return AgentBrowserShutdownOutcome::Clean(closure.native);
            }
        }
        AgentBrowserShutdownOutcome::Unclean
    }
}

fn join_worker_until(
    inner: &RuntimeInner,
    worker: &mut Option<RuntimeWorkerOwnership>,
    deadline: Instant,
) -> bool {
    inner.request_cooperative_shutdown_until(deadline);
    let completed = inner.completion.wait_until(deadline);
    // Logical completion precedes Tokio/runtime/TLS teardown. Only the existing
    // reaper may perform a potentially blocking OS-thread join; its original
    // acknowledgement, not mark_stopped or is_finished, proves actual exit.
    if let Some(worker) = worker.take() {
        schedule_reap(worker);
    }
    completed
        && inner.joined.wait_until(deadline)
        && Instant::now() < deadline
        && inner.controller_returned.load(Ordering::Acquire)
        && inner.run_state.load(Ordering::Acquire) == RUN_SUCCEEDED
        && inner.mailbox.fault().is_none()
}

impl Drop for RuntimeLifecycle {
    fn drop(&mut self) {
        // If an application drops this staged lifecycle rather than consuming
        // it, admission is sealed immediately and the worker receives a
        // deadline-bounded request to close callback intake. The reaper joins
        // that worker, but this remains Unclean: only a later controller can
        // establish the native-zero audit proof required to claim a clean
        // browser shutdown.
        if let Some(worker) = self.worker.take() {
            self.inner
                .request_cooperative_shutdown_until(Instant::now());
            schedule_reap(worker);
        }
    }
}

fn schedule_reap(worker: RuntimeWorkerOwnership) {
    // Deadline expiry must not make the UI-thread caller wait indefinitely.
    // The reaper owns only a worker which has already been cancelled and
    // sealed. At this handoff callback intake may still be open while the
    // worker reaches controller completion or its retained shutdown deadline;
    // that same worker performs the final close. The public outcome remains
    // Unclean until a later controller can offer an audited native-zero proof.
    let handoff = Arc::new(Mutex::new(Some(worker)));
    let reaper_handoff = Arc::clone(&handoff);
    if spawn_reaper(reaper_handoff).is_err() {
        // Ownership stays bounded by the exclusive reservation or its three
        // group slots. A failed reaper never releases a slot before join.
        let worker = take_worker_ownership(&handoff);
        let mut emergency = recover_lock(&EMERGENCY_WORKER_REAP);
        if let Some(worker) = worker {
            emergency.push(worker);
        }
    }
}

fn spawn_reaper(handoff: Arc<Mutex<Option<RuntimeWorkerOwnership>>>) -> Result<(), ()> {
    #[cfg(test)]
    if FORCE_REAPER_SPAWN_FAILURE.load(Ordering::Acquire) {
        return Err(());
    }
    thread::Builder::new()
        .name("zephium-agent-runtime-reaper".to_owned())
        .spawn(move || {
            if let Some(worker) = take_worker_ownership(&handoff) {
                worker.join();
            }
        })
        .map(|_| ())
        .map_err(|_| ())
}

/// Starts a suspended runtime without transferring a browser port.
pub fn spawn_suspended(
    config: AgentRuntimeConfig,
) -> Result<PendingAgentRuntime, RuntimeSpawnError> {
    PendingAgentRuntime::spawn_suspended(config)
}

/// Starts a suspended runtime whose controller is moved to its named worker.
///
/// The controller remains dormant until [`PendingAgentRuntime::bind_browser_port`]
/// transfers the browser port. This runtime still owns exactly one
/// process-scoped run and permanently seals admission after cancellation;
/// it is not a multi-run controller pool.
pub fn spawn_suspended_with_controller(
    config: AgentRuntimeConfig,
    controller: Box<dyn AgentRuntimeController>,
) -> Result<PendingAgentRuntime, RuntimeSpawnError> {
    PendingAgentRuntime::spawn_suspended_with_controller(config, controller)
}

fn worker_main(
    inner: Arc<RuntimeInner>,
    gate: Arc<StartupGate>,
    startup_sender: mpsc::SyncSender<Result<(), ()>>,
    permit: Arc<WorkerPermit>,
    controller: Option<RuntimeController>,
) {
    // This is intentionally scoped across every actual worker operation,
    // including startup failure and post-loop thread teardown. The permit
    // therefore cannot release merely because lifecycle completion became
    // observable; it releases only when this worker thread returns.
    let _permit_until_worker_exit = permit;
    let runtime = match tokio::runtime::Builder::new_current_thread()
        // Created only for an explicitly admitted agent worker. Provider and
        // local transport futures share this reactor; no second executor.
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = startup_sender.send(Err(()));
            inner.completion.mark_stopped();
            return;
        }
    };
    let _ = startup_sender.send(Ok(()));
    runtime.block_on(worker_loop(inner, gate, controller));
}

async fn worker_loop(
    inner: Arc<RuntimeInner>,
    gate: Arc<StartupGate>,
    mut controller: Option<RuntimeController>,
) {
    // Legacy browser authority stays in this one worker. A scoped controller
    // receives no native port; its captured facade cannot transfer the original
    // Work resource owner's port/sink/destruction ownership into this substrate.
    // Without it, this remains the conservative staged shell: every callback
    // is unexpected and forces a recorded fail-stop.
    let mut browser: Option<Arc<dyn AgentBrowserPort>> = None;
    loop {
        let mut notified = std::pin::pin!(inner.control_wake.notified());
        notified.as_mut().enable();
        if inner.shutdown_requested.load(Ordering::Acquire) {
            inner.close_and_drain_for_finalization().await;
            break;
        }
        if inner.cancelled.load(Ordering::Acquire) {
            inner.request_final_shutdown();
            inner.close_and_drain_for_finalization().await;
            break;
        }
        // A controller-owned start must remain queued until it is bound so the
        // controller observes that one exact admission. The staged shell has
        // no controller and deliberately consumes it as a no-op.
        if controller.is_none() {
            while let Some(command) = inner.commands.pop() {
                match command {
                    WorkerCommand::Start(ticket) => {
                        let _ticket = ticket;
                    }
                }
            }
        }
        if inner.shutdown_requested.load(Ordering::Acquire) {
            inner.close_and_drain_for_finalization().await;
            break;
        }
        if browser.is_none() {
            let startup_wait = gate.wait();
            tokio::pin!(startup_wait);
            if controller.is_some() {
                tokio::select! {
                    biased;
                    _ = &mut notified => {}
                    startup = &mut startup_wait => match startup {
                        Some(binding) => {
                            if let Some(controller) = controller.take() {
                                run_controller_no_unwind(
                                    &inner,
                                    controller,
                                    AgentRuntimeWorker {
                                        inner: Arc::clone(&inner),
                                        cancellation_delivered: false,
                                        shutdown_delivered: false,
                                        not_send: PhantomData,
                                    },
                                    binding,
                                )
                                .await;
                                // A controller return or unwind never grants
                                // an implicit clean outcome. Seal, drain, and
                                // release the browser only on this worker.
                                inner.request_final_shutdown();
                                inner.close_and_drain_for_finalization().await;
                                break;
                            }
                        }
                        None => {
                            inner.request_final_shutdown();
                            inner.close_and_drain_for_finalization().await;
                            break;
                        }
                    },
                }
            } else {
                let mailbox_wait = inner.mailbox.wait_for_work();
                tokio::pin!(mailbox_wait);
                tokio::select! {
                    biased;
                    _ = &mut notified => {}
                    wake = &mut mailbox_wait => {
                        fail_staged_mailbox_wake(&inner, wake).await;
                        break;
                    }
                    startup = &mut startup_wait => match startup {
                        Some(StartupBinding::Browser(port)) => browser = Some(port),
                        Some(StartupBinding::Scoped) => {
                            inner.request_final_shutdown();
                            inner.mailbox.close_and_drain().await;
                            break;
                        }
                        None => {
                            inner.request_final_shutdown();
                            inner.mailbox.close_and_drain().await;
                            break;
                        }
                    }
                }
            }
        } else {
            let mailbox_wait = inner.mailbox.wait_for_work();
            tokio::pin!(mailbox_wait);
            tokio::select! {
                biased;
                _ = &mut notified => {}
                wake = &mut mailbox_wait => {
                    fail_staged_mailbox_wake(&inner, wake).await;
                    break;
                }
            }
        }
    }
    drop(browser);
    gate.stop();
    #[cfg(test)]
    WORKER_EXIT_GATE.wait();
    inner.completion.mark_stopped();
    #[cfg(test)]
    WORKER_POST_COMPLETION_GATE.wait();
}

async fn run_controller_no_unwind(
    inner: &RuntimeInner,
    controller: RuntimeController,
    worker: AgentRuntimeWorker,
    binding: StartupBinding,
) {
    let mut future = match catch_unwind(AssertUnwindSafe(|| controller.run(worker, binding))) {
        Ok(Some(future)) => future,
        Ok(None) | Err(_) => return,
    };
    // Lifecycle shutdown is stronger than controller cooperation, but it does
    // not immediately destroy controller-owned active work. It first grants
    // the controller the lifecycle caller's exact deadline to observe the
    // one-shot shutdown event, cancel external work, and drain terminal/audit
    // callbacks. A spontaneous mailbox fault receives the same bounded path
    // with this host's fixed deadline. Only expiry force-drops a
    // non-cooperative controller; forced expiry remains unclean.
    let returned = {
        let controller_done = poll_controller_without_unwind(future.as_mut());
        tokio::pin!(controller_done);
        loop {
            // Register before observing shutdown state or its deadline so a
            // concurrent lifecycle/fault request cannot tighten the bound
            // between observation and wait registration.
            let mut control = std::pin::pin!(inner.control_wake.notified());
            control.as_mut().enable();
            if inner.mailbox.fault().is_some() {
                inner.request_fault_shutdown();
            }

            if let Some(deadline) = inner.shutdown_deadline() {
                if Instant::now() >= deadline {
                    break false;
                }
                let deadline_wait =
                    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
                tokio::pin!(deadline_wait);

                if inner.mailbox.fault().is_none() {
                    let mailbox_fault = inner.mailbox.wait_for_fault();
                    tokio::pin!(mailbox_fault);
                    tokio::select! {
                        biased;
                        returned = &mut controller_done => break returned,
                        _ = &mut deadline_wait => break false,
                        _ = &mut mailbox_fault => inner.request_fault_shutdown(),
                        _ = &mut control => {}
                    }
                } else {
                    tokio::select! {
                        biased;
                        returned = &mut controller_done => break returned,
                        _ = &mut deadline_wait => break false,
                        _ = &mut control => {}
                    }
                }
                continue;
            }

            let mailbox_fault = inner.mailbox.wait_for_fault();
            tokio::pin!(mailbox_fault);
            tokio::select! {
                biased;
                returned = &mut controller_done => break returned,
                _ = &mut mailbox_fault => inner.request_fault_shutdown(),
                _ = &mut control => {}
            }
        }
    };
    // A malicious or faulty controller may also panic while its future is
    // being dropped. Treat that exactly like a polling panic: the caller below
    // still seals and drains this runtime on the same worker.
    let dropped = catch_unwind(AssertUnwindSafe(|| drop(future))).is_ok();
    inner
        .controller_returned
        .store(returned && dropped, Ordering::Release);
    if !returned || !dropped {
        recover_lock(&inner.shutdown_closure).take();
        recover_lock(&inner.scoped_closure).take();
    }
}

fn poll_controller_without_unwind<'future>(
    mut future: Pin<&'future mut (dyn Future<Output = ()> + 'static)>,
) -> impl Future<Output = bool> + 'future {
    std::future::poll_fn(move |context| {
        match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
            Ok(std::task::Poll::Ready(())) => std::task::Poll::Ready(true),
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Err(_) => std::task::Poll::Ready(false),
        }
    })
}

async fn fail_staged_mailbox_wake(inner: &RuntimeInner, wake: AgentRuntimeMailboxWake) {
    // `wait_for_work` deliberately reports terminal debt before a sticky
    // fault so the eventual drain remains terminal-first. Its wake label is
    // not, however, the complete diagnostic state: saturation may already
    // have made successful execution impossible. Prefer that sticky fault so
    // a terminal overflow is never misreported as an ordinary callback.
    let reason = match inner.mailbox.fault() {
        Some(fault) => AgentRuntimeStagedStopReason::from_mailbox_fault(fault),
        None => match wake {
            AgentRuntimeMailboxWake::TerminalReady | AgentRuntimeMailboxWake::SignalReady => {
                AgentRuntimeStagedStopReason::UnexpectedNativeEvent
            }
            AgentRuntimeMailboxWake::Fault(fault) => {
                AgentRuntimeStagedStopReason::from_mailbox_fault(fault)
            }
        },
    };
    inner.record_staged_stop(reason);
    inner.request_final_shutdown();
    inner.mailbox.close_and_drain().await;
}

fn allocate_ticket(counter: &AtomicU64) -> Option<AgentRunTicket> {
    loop {
        let current = counter.load(Ordering::Acquire);
        let ticket = NonZeroU64::new(current)?;
        let next = current.checked_add(1)?;
        match counter.compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(AgentRunTicket(ticket)),
            Err(_) => continue,
        }
    }
}

fn recover_lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(error) => error.into_inner(),
    }
}

fn take_worker_ownership(
    handoff: &Mutex<Option<RuntimeWorkerOwnership>>,
) -> Option<RuntimeWorkerOwnership> {
    recover_lock(handoff).take()
}

struct RuntimeWorkerOwnership {
    worker: JoinHandle<()>,
    permit: Arc<WorkerPermit>,
    joined: Arc<WorkerJoinCompletion>,
}

impl RuntimeWorkerOwnership {
    fn join(self) -> bool {
        let joined = self.worker.join().is_ok();
        // Keep the lifecycle owner's permit alive until the actual worker has
        // been joined. The worker itself holds the other Arc until its thread
        // returns, so either ordering preserves process-wide exclusion.
        drop(self.permit);
        self.joined.finish(joined);
        joined
    }
}

/// Original join acknowledgement. It can be published only after the owner's
/// OS-thread join returns, including runtime and thread-local destruction.
struct WorkerJoinCompletion {
    result: Mutex<Option<bool>>,
    changed: Condvar,
}
impl WorkerJoinCompletion {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            changed: Condvar::new(),
        }
    }
    fn finish(&self, joined: bool) {
        *recover_lock(&self.result) = Some(joined);
        self.changed.notify_all();
    }
    fn wait_until(&self, deadline: Instant) -> bool {
        let mut result = recover_lock(&self.result);
        loop {
            if let Some(joined) = *result {
                return joined;
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return false;
            };
            result = match self.changed.wait_timeout(result, remaining) {
                Ok((result, _)) => result,
                Err(error) => error.into_inner().0,
            };
        }
    }
}

#[cfg(test)]
struct WorkerExitGate {
    held: Mutex<bool>,
    changed: Condvar,
}

#[cfg(test)]
impl WorkerExitGate {
    const fn new() -> Self {
        Self {
            held: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn hold(&self) {
        *recover_lock(&self.held) = true;
    }

    fn release(&self) {
        *recover_lock(&self.held) = false;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut held = recover_lock(&self.held);
        while *held {
            held = match self.changed.wait(held) {
                Ok(held) => held,
                Err(error) => error.into_inner(),
            };
        }
    }
}

struct WorkerPermit(Option<Arc<WorkerGroup>>);

/// One exclusive process reservation split into at most three scoped workers.
#[derive(Clone)]
pub struct AgentRuntimeWorkerGroup(Arc<WorkerGroup>);

struct WorkerGroup {
    _permit: Arc<WorkerPermit>,
    work: zephium_agentic::WorkId,
    capacity: u8,
    active: AtomicU8,
}

impl AgentRuntimeWorkerGroup {
    /// Reserves the existing exclusive runtime slot for one trusted Work group.
    pub fn try_new(work: zephium_agentic::WorkId, capacity: u8) -> Result<Self, RuntimeSpawnError> {
        if !(1..=3).contains(&capacity) {
            return Err(RuntimeSpawnError::Group);
        }
        Ok(Self(Arc::new(WorkerGroup {
            _permit: acquire_worker_permit()?,
            work,
            capacity,
            active: AtomicU8::new(0),
        })))
    }

    fn acquire(
        &self,
        scope: &AgentRuntimeScopedBinding,
    ) -> Result<Arc<WorkerPermit>, RuntimeSpawnError> {
        if scope.work() != self.0.work {
            return Err(RuntimeSpawnError::Group);
        }
        if retry_emergency_reapers() {
            return Err(RuntimeSpawnError::AlreadyRunning);
        }
        self.0
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < self.0.capacity).then_some(active + 1)
            })
            .map_err(|_| RuntimeSpawnError::Group)?;
        Ok(Arc::new(WorkerPermit(Some(self.0.clone()))))
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        match &self.0 {
            Some(group) => {
                group.active.fetch_sub(1, Ordering::AcqRel);
            }
            None => RUNTIME_WORKER_HELD.store(false, Ordering::Release),
        }
    }
}

fn acquire_worker_permit() -> Result<Arc<WorkerPermit>, RuntimeSpawnError> {
    if retry_emergency_reapers() {
        return Err(RuntimeSpawnError::AlreadyRunning);
    }
    if RUNTIME_WORKER_HELD
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(RuntimeSpawnError::AlreadyRunning);
    }
    Ok(Arc::new(WorkerPermit(None)))
}

fn retry_emergency_reapers() -> bool {
    let retained_workers = std::mem::take(&mut *recover_lock(&EMERGENCY_WORKER_REAP));
    let retained = !retained_workers.is_empty();
    if retained {
        for worker in retained_workers {
            schedule_reap(worker);
        }
    }
    retained
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use tokio::sync::oneshot;

    use crate::{MIN_AGENT_RUNTIME_SIGNAL_CAPACITY, MIN_AGENT_RUNTIME_TERMINAL_CAPACITY};
    use zephium_agentic::{
        decode_semantic_snapshot, encode_semantic_observation, AgentAccountAttestationId,
        AgentAccountScope, AgentActiveModelCall, AgentContextAccountBinding, AgentEffectScope,
        AgentModelCallBudget, AgentModelCallId, AgentModelCallReceipt, AgentModelCallRequest,
        AgentModelCallSettlement, AgentModelCallUnaccountedSettlement, AgentModelUsageAccounting,
        AgentPlanLeaseBinding, AgentPlanLeaseId, AgentPlanNodeAuthority, AgentPlanNodeId,
        AgentPlanNodeScope, AgentPolicyInstant, AgentRunBudget, AgentRunManifest,
        AgentRunManifestId, AgentRunPolicy, AgentRunScope, ContextCapabilities, ContextCapability,
        ContextCookieTransferRequest, ContextDispatch, ContextId, ContextIdentity, ContextKind,
        ContextNativeEvent, ContextNativeRequest, ContextOperationId, ContextPortFailure,
        ContextRegistry, ContextRendererLoss, ContextResourceAuditId,
        ContextResourceAuditSettlement, ContextRunId, ContextSettlement, ContextShutdownDispatch,
        FrameId, SemanticActionNativeCompletion, SemanticActionNativeRequest,
        SemanticDecodeContext, SemanticEffectClass, SemanticFrameJoin, SemanticFrameTrust,
        SemanticInvocationId, SemanticModelDeliverySettlement, SemanticModelEncodingBudget,
        SemanticObservationAssembler, SemanticObservationBudget, SemanticObservationId,
        SemanticObservationRequest, SemanticOrigin, SemanticRuntimeInvocation,
        SemanticScreenshotNativeCompletion, SemanticScreenshotNativeRequest, SemanticSensitivity,
        SemanticSnapshotGeneration, SemanticTokenizerRevision,
    };

    use super::*;

    static RUNTIME_TEST_SERIALIZER: Mutex<()> = Mutex::new(());

    struct CompletionWakeCounter(AtomicUsize);
    impl std::task::Wake for CompletionWakeCounter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[test]
    fn completion_wake_registration_racing_exit_is_never_lost() {
        for _ in 0..32 {
            let state = Arc::new(CompletionState::new());
            let wake = Arc::new(CompletionWakeCounter(AtomicUsize::new(0)));
            let exit = state.clone();
            let thread = std::thread::spawn(move || exit.mark_stopped());
            state.set_waker(std::task::Waker::from(wake.clone()));
            thread.join().unwrap();
            assert!(state.is_stopped());
            assert_eq!(wake.0.load(Ordering::Acquire), 1);
            state.mark_stopped();
            assert_eq!(wake.0.load(Ordering::Acquire), 1);
        }
    }

    #[test]
    fn completion_wake_panic_does_not_strand_worker_exit() {
        struct PanickingWake;
        impl std::task::Wake for PanickingWake {
            fn wake(self: Arc<Self>) {
                panic!("synthetic wake panic");
            }
        }
        let state = CompletionState::new();
        state.set_waker(std::task::Waker::from(Arc::new(PanickingWake)));
        state.mark_stopped();
        assert!(state.wait_until(Instant::now()));
    }

    struct RecordingPort {
        calls: AtomicUsize,
    }

    impl RecordingPort {
        fn calls(&self) -> usize {
            self.calls.load(Ordering::Acquire)
        }
    }

    impl AgentBrowserPort for RecordingPort {
        fn dispatch(&self, _request: ContextNativeRequest) -> ContextDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextDispatch::Unsupported
        }

        fn transfer_cookies(&self, _request: ContextCookieTransferRequest) -> ContextDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextDispatch::Unsupported
        }

        fn audit_resources(&self, _audit: ContextResourceAuditId) -> ContextDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextDispatch::Unsupported
        }

        fn seal_for_shutdown(&self, _audit: ContextResourceAuditId) -> ContextShutdownDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextShutdownDispatch::SealedWithoutAudit(ContextPortFailure::Shutdown)
        }

        fn invoke_semantic(&self, _invocation: SemanticRuntimeInvocation) -> ContextDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextDispatch::Unsupported
        }

        fn execute_semantic_action(
            &self,
            _request: SemanticActionNativeRequest,
            _completion: SemanticActionNativeCompletion,
        ) -> ContextDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextDispatch::Unsupported
        }

        fn capture_semantic_screenshot(
            &self,
            _request: SemanticScreenshotNativeRequest,
            _completion: SemanticScreenshotNativeCompletion,
        ) -> ContextDispatch {
            self.calls.fetch_add(1, Ordering::AcqRel);
            ContextDispatch::Unsupported
        }
    }

    pub(super) fn runtime_test_guard() -> MutexGuard<'static, ()> {
        recover_lock(&RUNTIME_TEST_SERIALIZER)
    }

    struct ControllerReport {
        worker_name: Option<String>,
        current_ticket: Option<u64>,
        admitted: bool,
        events: Vec<&'static str>,
    }

    struct RecordingController {
        sender: mpsc::Sender<ControllerReport>,
        expected_events: usize,
    }

    impl AgentRuntimeController for RecordingController {
        fn run(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            let sender = self.sender.clone();
            let expected_events = self.expected_events;
            let worker_name = std::thread::current().name().map(str::to_owned);
            let current_ticket = worker.current_run_ticket().map(AgentRunTicket::get);
            let admitted = worker.status().admitted();
            Box::pin(async move {
                let mut events = Vec::with_capacity(expected_events);
                for _ in 0..expected_events {
                    let label = match worker.next_event().await {
                        Ok(AgentRuntimeEvent::RunStarted(_)) => "RunStarted",
                        Ok(AgentRuntimeEvent::NativeTerminal(_)) => "NativeTerminal",
                        Ok(AgentRuntimeEvent::SemanticActionTerminal(_)) => {
                            "SemanticActionTerminal"
                        }
                        Ok(AgentRuntimeEvent::SemanticScreenshotTerminal(_)) => {
                            "SemanticScreenshotTerminal"
                        }
                        Ok(AgentRuntimeEvent::AuditTerminal(_)) => "AuditTerminal",
                        Ok(AgentRuntimeEvent::NavigationReplaced(_)) => "NavigationReplaced",
                        Ok(AgentRuntimeEvent::RendererLost(_)) => "RendererLost",
                        Ok(AgentRuntimeEvent::CancellationRequested) => "CancellationRequested",
                        Ok(AgentRuntimeEvent::ShutdownRequested) => "ShutdownRequested",
                        Err(AgentRuntimeWorkerFault::Mailbox(_)) => "MailboxFault",
                    };
                    events.push(label);
                }
                let _ = sender.send(ControllerReport {
                    worker_name,
                    current_ticket,
                    admitted,
                    events,
                });
            })
        }
    }

    struct EarlyExitController;

    pub(super) struct LateStartController {
        pub(super) waiting: mpsc::Sender<()>,
        pub(super) started: mpsc::Sender<Option<AgentRunTicket>>,
    }

    impl LateStartController {
        fn wait_for_start(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
        ) -> AgentRuntimeControllerFuture {
            Box::pin(async move {
                let mut event = Box::pin(worker.next_event());
                std::future::poll_fn(|cx| {
                    // Signal only after the real inner waiter has registered
                    // and returned Pending under the older lifecycle waiters.
                    assert!(event.as_mut().poll(cx).is_pending());
                    std::task::Poll::Ready(())
                })
                .await;
                self.waiting.send(()).unwrap();
                let ticket = match event.await {
                    Ok(AgentRuntimeEvent::RunStarted(ticket)) => Some(ticket),
                    _ => None,
                };
                let _ = self.started.send(ticket);
            })
        }
    }

    impl AgentRuntimeController for LateStartController {
        fn run(
            self: Box<Self>,
            worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            self.wait_for_start(worker)
        }
    }

    impl AgentRuntimeScopedController for LateStartController {
        fn run(self: Box<Self>, worker: AgentRuntimeWorker) -> AgentRuntimeControllerFuture {
            self.wait_for_start(worker)
        }
    }

    #[test]
    fn bound_controller_observes_late_start_without_an_unrelated_wake() {
        let _guard = runtime_test_guard();
        let (waiting, pending_event) = mpsc::channel();
        let (started, observed_start) = mpsc::channel();
        let pending = PendingAgentRuntime::spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(LateStartController { waiting, started }),
        )
        .unwrap();
        let (handle, _completion, lifecycle) = pending
            .bind_browser_port(Arc::new(RecordingPort {
                calls: AtomicUsize::new(0),
            }))
            .into_parts();
        pending_event.recv_timeout(Duration::from_secs(2)).unwrap();
        let ticket = handle.start_run().unwrap();
        let observed = observed_start.recv_timeout(Duration::from_millis(200));
        // Always clean up before asserting, including the negative control.
        assert!(matches!(
            lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)),
            AgentBrowserShutdownOutcome::Unclean
        ));
        assert_eq!(observed, Ok(Some(ticket)));
    }

    impl AgentRuntimeController for EarlyExitController {
        fn run(
            self: Box<Self>,
            _worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            Box::pin(async {})
        }
    }

    struct PanicController;

    impl AgentRuntimeController for PanicController {
        fn run(
            self: Box<Self>,
            _worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            Box::pin(async {
                panic!("controller test panic");
            })
        }
    }

    struct FaultIgnoringController {
        started: mpsc::Sender<()>,
    }

    impl AgentRuntimeController for FaultIgnoringController {
        fn run(
            self: Box<Self>,
            worker: AgentRuntimeWorker,
            browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            let _ = self.started.send(());
            Box::pin(async move {
                let _retained_outside_capability_loop = (worker, browser);
                std::future::pending::<()>().await;
            })
        }
    }

    struct EventWaitingController {
        ready: mpsc::Sender<()>,
        settled: mpsc::Sender<()>,
    }

    impl AgentRuntimeController for EventWaitingController {
        fn run(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            let ready = self.ready.clone();
            let settled = self.settled.clone();
            Box::pin(async move {
                let _ = ready.send(());
                if matches!(
                    worker.next_event().await,
                    Ok(AgentRuntimeEvent::NativeTerminal(
                        ContextNativeEvent::ResourceAuditSettled(_)
                    ))
                ) {
                    let _ = settled.send(());
                }
            })
        }
    }

    struct ShutdownDrainController {
        policy: AgentRunPolicy,
        active_call: AgentActiveModelCall,
        active: mpsc::Sender<()>,
        shutdown: mpsc::Sender<PolicyDrainReport>,
        reconciled: mpsc::Sender<()>,
        published: oneshot::Receiver<()>,
    }

    struct PolicyDrainReport {
        receipt: AgentModelCallReceipt,
        pending_model_calls: usize,
        reserved_model_tokens: u64,
        reserved_cost_micro_usd: u64,
    }

    impl AgentRuntimeController for ShutdownDrainController {
        fn run(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            let Self {
                mut policy,
                active_call,
                active: active_sender,
                shutdown: shutdown_sender,
                reconciled: reconciled_sender,
                published,
            } = *self;
            Box::pin(async move {
                let mut active = false;
                let mut shutdown = false;
                let mut active_call = Some(active_call);
                loop {
                    match worker.next_event().await {
                        Ok(AgentRuntimeEvent::RunStarted(_)) => {
                            active = true;
                            let _ = active_sender.send(());
                        }
                        Ok(AgentRuntimeEvent::ShutdownRequested) => {
                            let Some(active_call) = active_call.take() else {
                                return;
                            };
                            let Ok(receipt) = policy.settle_model_call_unaccounted(
                                active_call,
                                AgentModelCallUnaccountedSettlement::ProviderFailed,
                            ) else {
                                return;
                            };
                            shutdown = true;
                            let accounting = policy.accounting();
                            let _ = shutdown_sender.send(PolicyDrainReport {
                                receipt,
                                pending_model_calls: policy.pending_model_calls(),
                                reserved_model_tokens: accounting.reserved_model_tokens(),
                                reserved_cost_micro_usd: accounting.reserved_cost_micro_usd(),
                            });
                        }
                        Ok(AgentRuntimeEvent::NativeTerminal(
                            ContextNativeEvent::ResourceAuditSettled(_),
                        )) if active && shutdown => {
                            let _ = reconciled_sender.send(());
                            let _ = published.await;
                            return;
                        }
                        Ok(_) | Err(_) => return,
                    }
                }
            })
        }
    }

    struct FaultDrainController {
        release: oneshot::Receiver<()>,
        ready: mpsc::Sender<()>,
        drained: mpsc::Sender<usize>,
        expected_terminal_count: usize,
    }

    impl AgentRuntimeController for FaultDrainController {
        fn run(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            let ready = self.ready.clone();
            let drained = self.drained.clone();
            let expected_terminal_count = self.expected_terminal_count;
            let release = self.release;
            Box::pin(async move {
                let _ = ready.send(());
                if release.await.is_err() {
                    return;
                }
                let mut shutdown = false;
                let mut terminal_count = 0;
                loop {
                    match worker.next_event().await {
                        Ok(AgentRuntimeEvent::ShutdownRequested) => {
                            shutdown = true;
                            if terminal_count == expected_terminal_count {
                                let _ = drained.send(terminal_count);
                                return;
                            }
                        }
                        Ok(AgentRuntimeEvent::NativeTerminal(
                            ContextNativeEvent::ResourceAuditSettled(_),
                        )) => {
                            terminal_count += 1;
                            if shutdown && terminal_count == expected_terminal_count {
                                let _ = drained.send(terminal_count);
                                return;
                            }
                        }
                        Ok(_) | Err(_) => return,
                    }
                }
            })
        }
    }

    struct DropSignal(Option<mpsc::Sender<Instant>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(Instant::now());
            }
        }
    }

    struct DeadlineIgnoringController {
        entered: mpsc::Sender<()>,
        shutdown: mpsc::Sender<Instant>,
        dropped: mpsc::Sender<Instant>,
    }

    impl AgentRuntimeController for DeadlineIgnoringController {
        fn run(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
            browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            let entered = self.entered.clone();
            let shutdown = self.shutdown.clone();
            let dropped = self.dropped.clone();
            Box::pin(async move {
                let _ = entered.send(());
                if !matches!(
                    worker.next_event().await,
                    Ok(AgentRuntimeEvent::ShutdownRequested)
                ) {
                    return;
                }
                let Some(deadline) = worker.shutdown_deadline() else {
                    return;
                };
                let _ = shutdown.send(deadline);
                let _retained_outside_capability_loop = (worker, browser);
                let _drop_signal = DropSignal(Some(dropped));
                std::future::pending::<()>().await;
            })
        }
    }

    fn controller_test_inner() -> Arc<RuntimeInner> {
        Arc::new(RuntimeInner {
            scope: None,
            scoped_closure: Mutex::new(None),
            mailbox: AgentRuntimeMailbox::new(AgentRuntimeMailboxConfig::STANDARD),
            commands: ArrayQueue::new(MIN_AGENT_RUNTIME_COMMAND_CAPACITY),
            control_wake: Notify::new(),
            run_state: AtomicU8::new(RUN_IDLE),
            next_ticket: AtomicU64::new(1),
            current_ticket: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            stop_reason: AtomicU8::new(0),
            shutdown_requested: AtomicBool::new(false),
            shutdown_deadline: Mutex::new(None),
            terminal_claim_gate: Mutex::new(()),
            terminal_claim_class: AtomicU8::new(TERMINAL_CLASS_NONE),
            shutdown_closure: Mutex::new(None),
            controller_returned: AtomicBool::new(false),
            fault_shutdown_requested: AtomicBool::new(false),
            staged_stop_reason: AtomicU8::new(STAGED_STOP_NONE),
            completion: CompletionState::new(),
            joined: Arc::new(WorkerJoinCompletion::new()),
        })
    }

    fn closed_shutdown_evidence() -> (AgentNativeShutdownProof, AgentRunPolicySettlement) {
        use zephium_agentic::*;
        let context = controller_test_context();
        let root = AgentPlanNodeId::generate();
        let origin = SemanticOrigin::parse("https://shutdown.invalid").expect("origin");
        let effects = AgentEffectScope::try_new(&[SemanticEffectClass::Read]).expect("effects");
        let budget = AgentRunBudget::try_new(1, 1, 1, 1).expect("budget");
        let profile = context.identity().profile();
        let manifest = AgentRunManifest::try_new(
            AgentRunManifestId::generate(),
            context.identity().owner(),
            AgentRunScope::try_new(
                vec![profile],
                vec![AgentAccountScope::Anonymous],
                vec![origin.clone()],
                SemanticSensitivity::Public,
                effects,
                Vec::new(),
            )
            .expect("scope"),
            budget,
            AgentPolicyInstant::from_millis(1),
            AgentPolicyInstant::from_millis(100),
            vec![AgentPlanNodeScope::new(
                root,
                AgentPlanNodeAuthority::try_new(
                    vec![profile],
                    vec![AgentAccountScope::Anonymous],
                    vec![origin],
                    SemanticSensitivity::Public,
                    effects,
                )
                .expect("authority"),
                budget,
                AgentPolicyInstant::from_millis(99),
            )],
        )
        .expect("manifest");
        let mut supervisor = AgentRunSupervisor::new(
            AgentSupervisorId::new(1).expect("id"),
            AgentDelegationTopology::try_new(&manifest, vec![AgentDelegationSpec::new(root, None)])
                .expect("topology"),
        );
        let accounting =
            AgentRunAccountingMetrics::try_new(&manifest, &supervisor).expect("accounting");
        let mut progress =
            AgentRunProgressMetrics::try_new(&manifest, &supervisor).expect("progress");
        let actions =
            AgentRunActionPerformanceMetrics::try_new(&manifest, &supervisor).expect("actions");
        let inputs = AgentRunProviderInputMetrics::try_new(&manifest, &supervisor).expect("inputs");
        let mut audit = AgentAuditLedger::try_new(&manifest, &supervisor).expect("audit");
        let mut record = |supervisor: &AgentRunSupervisor, value: u64| {
            let event = audit
                .record_current(
                    supervisor,
                    root,
                    AgentAuditEventId::new(value).expect("id"),
                    AgentPolicyInstant::from_millis(value),
                )
                .expect("event");
            progress.record_event(event).expect("progress");
        };
        record(&supervisor, 1);
        let execution = supervisor
            .start(root, AgentSupervisorAttemptId::new(1).expect("id"))
            .expect("start");
        record(&supervisor, 2);
        supervisor
            .complete(execution, AgentSupervisorCompletion::Succeeded)
            .expect("complete");
        record(&supervisor, 3);
        let closure = AgentRunMetricClosure::try_close(
            &manifest,
            &supervisor,
            &accounting,
            &progress,
            &actions,
            &inputs,
        )
        .expect("closure");
        audit.seal_for_shutdown().expect("seal");
        let delivery = audit
            .begin_delivery(
                AgentAuditDeliveryId::new(1).expect("id"),
                MAX_AGENT_AUDIT_DELIVERY_EVENTS,
            )
            .expect("delivery");
        audit
            .settle_delivery(
                delivery
                    .proof()
                    .settle(AgentAuditDeliveryOutcome::Committed),
            )
            .expect("commit");
        let policy = AgentRunPolicy::try_new(
            manifest,
            vec![AgentPlanLeaseBinding::new(
                AgentPlanLeaseId::generate(),
                root,
            )],
        )
        .expect("policy");
        let settled = policy
            .settle_metric_closure(closure, &accounting, audit)
            .expect("settled");
        let mut contexts = ContextRegistry::new();
        contexts.seal_for_shutdown().expect("seal");
        let mut profiles = ContextProfileLeaseRegistry::new();
        profiles.seal_for_shutdown().expect("seal");
        let mut cookies = ContextCookieTransferRegistry::new();
        cookies.seal_for_shutdown().expect("seal");
        let mut executions = SemanticActionExecutionCoordinator::new();
        executions.seal();
        let mut settlements = SemanticActionSettlementCoordinator::new();
        settlements.seal();
        let mut screenshots = SemanticScreenshotCoordinator::new();
        screenshots.seal_for_shutdown();
        let mut native =
            AgentNativeShutdownCoordinator::try_new(AgentNativeShutdownResources::new(
                contexts,
                profiles,
                cookies,
                executions,
                settlements,
                screenshots,
            ))
            .expect("cohort");
        let id = ContextResourceAuditId::new(1).expect("id");
        native.begin_port_seal(id).expect("seal");
        native
            .account_port_seal(id, ContextShutdownDispatch::AuditScheduled)
            .expect("dispatch");
        native
            .settle_shutdown_audit(ContextShutdownAuditSettlement::new(
                id,
                Ok(
                    ContextNativeResourceSnapshot::try_new(ContextNativeResourceCounts {
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
                    .expect("zero"),
                ),
            ))
            .expect("audit");
        (native.finish().expect("native proof"), settled)
    }

    struct ClosedController(u8);

    impl AgentRuntimeController for ClosedController {
        fn run(
            self: Box<Self>,
            mut worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            Box::pin(async move {
                assert!(matches!(
                    worker.next_event().await,
                    Ok(AgentRuntimeEvent::RunStarted(_))
                ));
                let claim = worker
                    .try_claim_controller_terminal(AgentRuntimeControllerTerminalClass::Ordinary)
                    .await
                    .expect("claim");
                let (_native, _policy) = closed_shutdown_evidence();
                // Native-zero and policy closure alone are insufficient:
                // only the real provider transport can supply its third proof.
                claim.commit();
                match self.0 {
                    0 => {}
                    1 => panic!("test controller panic after closed evidence"),
                    _ => std::future::pending::<()>().await,
                }
            })
        }
    }

    #[test]
    fn lifecycle_cannot_infer_provider_drain_from_native_policy_or_worker_exit() {
        let _guard = runtime_test_guard();
        for mode in 0..3 {
            let pending = PendingAgentRuntime::spawn_suspended_with_controller(
                AgentRuntimeConfig::STANDARD,
                Box::new(ClosedController(mode)),
            )
            .expect("runtime");
            let composition = pending.bind_browser_port(Arc::new(RecordingPort {
                calls: AtomicUsize::new(0),
            }));
            let (handle, completion, lifecycle) = composition.into_parts();
            handle.start_run().expect("run");
            // Wait until commit; do not race shutdown against initial admission.
            for _ in 0..200 {
                if handle.inner.run_state.load(Ordering::Acquire) == RUN_SUCCEEDED {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(
                handle.inner.run_state.load(Ordering::Acquire),
                RUN_SUCCEEDED
            );
            if mode < 2 {
                wait_stopped(&completion);
            }
            let outcome = lifecycle.shutdown_until(Instant::now() + Duration::from_millis(50));
            assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
            assert_eq!(
                handle.inner.controller_returned.load(Ordering::Acquire),
                mode == 0
            );
            wait_stopped(&completion);
            let next = spawn_after_true_worker_exit();
            drop(next);
        }
    }

    fn controller_test_worker(inner: Arc<RuntimeInner>) -> AgentRuntimeWorker {
        AgentRuntimeWorker {
            inner,
            cancellation_delivered: false,
            shutdown_delivered: false,
            not_send: PhantomData,
        }
    }

    fn active_controller_test_inner() -> Arc<RuntimeInner> {
        let inner = controller_test_inner();
        inner.run_state.store(RUN_ACTIVE, Ordering::Release);
        inner.current_ticket.store(1, Ordering::Release);
        inner
    }

    #[test]
    fn terminal_claim_closes_racing_ingress_and_commits_only_once() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller claim runtime");
        runtime.block_on(async {
            let inner = active_controller_test_inner();
            let mut worker = controller_test_worker(Arc::clone(&inner));
            let claim = worker
                .try_claim_controller_terminal(AgentRuntimeControllerTerminalClass::Ordinary)
                .await
                .expect("empty active controller claims terminal");
            assert_eq!(claim.ticket().get(), 1);
            assert!(inner.mailbox.publish_test_terminal().is_err());
            claim.commit();
            assert!(inner.status().sealed());
            assert!(inner.mailbox.fault().is_none());
        });
    }

    #[test]
    fn terminal_claim_rejects_queued_callback_debt_before_clean_publication() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller claim runtime");
        runtime.block_on(async {
            let inner = active_controller_test_inner();
            inner
                .mailbox
                .publish_test_terminal()
                .expect("bounded terminal debt queues");
            let mut worker = controller_test_worker(Arc::clone(&inner));
            assert!(matches!(
                worker
                    .try_claim_controller_terminal(AgentRuntimeControllerTerminalClass::Ordinary)
                    .await,
                Err(AgentRuntimeControllerTerminalRefusal::TerminalDebt)
            ));
            assert!(inner.status().sealed());
            assert!(inner.mailbox.try_pop_terminal().is_some());
        });
    }

    #[test]
    fn terminal_claim_rejects_queued_native_signal_debt_before_clean_publication() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller claim runtime");
        runtime.block_on(async {
            let inner = active_controller_test_inner();
            inner
                .mailbox
                .publish_test_signal()
                .expect("bounded signal debt queues");
            let mut worker = controller_test_worker(Arc::clone(&inner));
            assert!(matches!(
                worker
                    .try_claim_controller_terminal(AgentRuntimeControllerTerminalClass::Ordinary)
                    .await,
                Err(AgentRuntimeControllerTerminalRefusal::SignalDebt)
            ));
            // Production signal values use the same closed controller event
            // path; this mailbox-private marker only proves claim rejection.
        });
    }

    #[test]
    fn cancelled_and_shutdown_terminal_claims_preserve_their_exact_control_class() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller claim runtime");
        runtime.block_on(async {
            let cancelled = active_controller_test_inner();
            cancelled.seal_and_cancel();
            let mut worker = controller_test_worker(Arc::clone(&cancelled));
            let claim = worker
                .try_claim_controller_terminal(AgentRuntimeControllerTerminalClass::Cancelled)
                .await
                .expect("cancelled drain claims clean terminal");
            // Repeated user cancellation cannot revoke an equivalent claim.
            cancelled.seal_and_cancel();
            claim.commit();
            assert!(cancelled.status().cancelled());
            assert!(cancelled.mailbox.fault().is_none());

            let shutdown = active_controller_test_inner();
            let deadline = Instant::now() + Duration::from_secs(1);
            shutdown.request_cooperative_shutdown_until(deadline);
            let mut worker = controller_test_worker(Arc::clone(&shutdown));
            let claim = worker
                .try_claim_controller_terminal(AgentRuntimeControllerTerminalClass::Shutdown)
                .await
                .expect("shutdown drain claims clean terminal");
            // A tightened lifecycle deadline remains available to reap a
            // controller which maliciously holds the move-only proof.
            shutdown.request_cooperative_shutdown_until(Instant::now());
            assert!(shutdown.shutdown_deadline().is_some());
            claim.commit();
            assert!(shutdown.status().mailbox_fault().is_none());
        });
    }

    fn controller_test_context() -> zephium_agentic::ContextJoin {
        let identity = ContextIdentity::new(
            ContextId::generate(),
            ContextRunId::generate(),
            31_u128.into(),
            ContextKind::Owned,
        );
        let capabilities = ContextCapabilities::try_new(
            ContextKind::Owned,
            &[ContextCapability::Observe, ContextCapability::Act],
        )
        .expect("controller test capabilities");
        let mut registry = ContextRegistry::new();
        registry
            .reserve(identity, capabilities)
            .expect("controller test reserve");
        let construction = registry
            .begin_context(
                identity.id(),
                ContextOperationId::new(1).expect("controller test operation"),
            )
            .expect("controller test construction");
        registry
            .settle_construction(identity.id(), construction, ContextSettlement::Applied)
            .expect("controller test settlement");
        registry
            .join(identity.id())
            .expect("controller test current join")
    }

    fn active_model_call_for_shutdown_test() -> (AgentRunPolicy, AgentActiveModelCall) {
        const ISSUED_AT: u64 = 1;
        const NOW: u64 = 10;
        const EXPIRES_AT: u64 = 100;

        let context = controller_test_context();
        let origin = SemanticOrigin::parse("https://runtime-shutdown.invalid")
            .expect("test origin is canonical");
        let effects = AgentEffectScope::try_new(&[SemanticEffectClass::Read])
            .expect("test effect scope is nonempty");
        let budget = AgentRunBudget::try_new(4, 2_000, 1_000, 1).expect("test run budget");
        let scope = AgentRunScope::try_new(
            vec![context.identity().profile()],
            vec![AgentAccountScope::Anonymous],
            vec![origin.clone()],
            SemanticSensitivity::Sensitive,
            effects,
            Vec::new(),
        )
        .expect("test run scope");
        let authority = AgentPlanNodeAuthority::try_new(
            vec![context.identity().profile()],
            vec![AgentAccountScope::Anonymous],
            vec![origin.clone()],
            SemanticSensitivity::Sensitive,
            effects,
        )
        .expect("test node authority");
        let node = AgentPlanNodeId::generate();
        let lease = AgentPlanLeaseId::generate();
        let manifest = AgentRunManifest::try_new(
            AgentRunManifestId::generate(),
            context.identity().owner(),
            scope,
            budget,
            AgentPolicyInstant::from_millis(ISSUED_AT),
            AgentPolicyInstant::from_millis(EXPIRES_AT),
            vec![AgentPlanNodeScope::new(
                node,
                authority,
                budget,
                AgentPolicyInstant::from_millis(EXPIRES_AT - 1),
            )],
        )
        .expect("test manifest");
        let mut policy =
            AgentRunPolicy::try_new(manifest, vec![AgentPlanLeaseBinding::new(lease, node)])
                .expect("test policy");

        let frame = SemanticFrameJoin::try_new(
            context,
            FrameId::MAIN,
            context.frame_generation(),
            origin,
            SemanticFrameTrust::SameOrigin,
        )
        .expect("test main frame");
        let snapshot = decode_semantic_snapshot(
            SemanticDecodeContext::new(
                SemanticInvocationId::new(1).expect("test invocation identity"),
                frame,
                SemanticSnapshotGeneration::INITIAL,
            ),
            br#"{"v":1,"i":1,"g":1,"c":"complete","n":[{"k":1,"r":"document","o":16}]}"#,
        )
        .expect("test semantic snapshot");
        let observation = SemanticObservationAssembler::new(
            SemanticObservationRequest::initial(
                SemanticObservationId::new(1).expect("test observation identity"),
                context,
                SemanticObservationBudget::INITIAL_FILTERED,
            ),
            snapshot,
        )
        .expect("test observation assembly")
        .finish()
        .expect("test observation completion");
        let tokenizer = SemanticTokenizerRevision::try_new("runtime-shutdown-v1".to_owned())
            .expect("test tokenizer revision");
        let payload = encode_semantic_observation(
            &observation,
            SemanticModelEncodingBudget::INITIAL_CONSERVATIVE,
        )
        .expect("test observation encoding")
        .admit_conservative_utf8(&tokenizer)
        .expect("test observation token admission");
        let request = AgentModelCallRequest::new(
            AgentModelCallId::new(1).expect("test model-call identity"),
            lease,
            AgentContextAccountBinding::new(
                AgentAccountAttestationId::generate(),
                context,
                AgentAccountScope::Anonymous,
                AgentPolicyInstant::from_millis(NOW),
            ),
            AgentModelCallBudget::try_new(10, 20, 30).expect("test model-call budget"),
            AgentPolicyInstant::from_millis(NOW),
        );
        let admission = policy
            .prepare_observation_input(request, &observation, &payload)
            .expect("test model-call reservation");
        let acknowledgement = payload
            .settle_delivery(SemanticModelDeliverySettlement::Committed)
            .expect("test model input commitment");
        let active_call = policy
            .commit_observation_input(admission, &acknowledgement)
            .expect("test active model call");
        assert_eq!(policy.pending_model_calls(), 1);
        assert!(policy.accounting().reserved_model_tokens() > 0);
        (policy, active_call)
    }

    struct ForcedReaperSpawnFailure;

    impl ForcedReaperSpawnFailure {
        fn enable() -> Self {
            FORCE_REAPER_SPAWN_FAILURE.store(true, Ordering::Release);
            Self
        }
    }

    impl Drop for ForcedReaperSpawnFailure {
        fn drop(&mut self) {
            FORCE_REAPER_SPAWN_FAILURE.store(false, Ordering::Release);
        }
    }

    struct HeldWorkerExitGate;

    impl HeldWorkerExitGate {
        fn enable() -> Self {
            WORKER_EXIT_GATE.hold();
            Self
        }

        fn release(self) {
            WORKER_EXIT_GATE.release();
        }
    }

    impl Drop for HeldWorkerExitGate {
        fn drop(&mut self) {
            WORKER_EXIT_GATE.release();
        }
    }

    pub(super) fn wait_stopped(completion: &AgentRuntimeCompletion) {
        for _ in 0..300 {
            if completion.is_stopped() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(completion.is_stopped());
    }

    fn spawn_after_true_worker_exit() -> PendingAgentRuntime {
        for _ in 0..40 {
            match spawn_suspended(AgentRuntimeConfig::STANDARD) {
                Ok(next) => return next,
                Err(RuntimeSpawnError::AlreadyRunning) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(RuntimeSpawnError::WorkerUnavailable | RuntimeSpawnError::Group) => {
                    panic!("test worker infrastructure is unavailable")
                }
            }
        }
        panic!("worker permit did not release after the worker exited")
    }

    #[test]
    fn controller_starts_on_the_named_worker_after_bind_and_receives_queued_start_once() {
        let _guard = runtime_test_guard();
        let (sender, receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(RecordingController {
                sender,
                expected_events: 1,
            }),
        )
        .expect("controller worker starts");
        let handle_before_bind = AgentRuntimeHandle {
            inner: Arc::clone(&pending.inner),
        };
        let ticket = handle_before_bind.start_run().expect("one queued start");
        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        let report = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller starts after bind");
        assert_eq!(report.worker_name.as_deref(), Some("zephium-agent-runtime"));
        assert_eq!(report.current_ticket, Some(ticket.get()));
        assert!(report.admitted);
        assert_eq!(report.events, ["RunStarted"]);
        wait_stopped(&completion);
        assert!(handle.status().cancelled());
        assert!(handle.status().sealed());
        assert!(matches!(
            lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)),
            AgentBrowserShutdownOutcome::Unclean
        ));
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn controller_events_are_closed_real_and_terminal_first() {
        let inner = controller_test_inner();
        let sink = inner.mailbox.native_event_sink();
        let audit = ContextResourceAuditId::new(1).expect("controller test audit identity");
        // Intentionally enqueue the unsolicited signal first: controller
        // delivery must drain the separate terminal lane before it.
        assert!(sink
            .publish(ContextNativeEvent::RendererLost(ContextRendererLoss::new(
                controller_test_context(),
            )))
            .is_ok());
        assert!(sink
            .publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
            ))
            .is_ok());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller test runtime");
        runtime.block_on(async {
            let mut worker = controller_test_worker(Arc::clone(&inner));
            assert!(matches!(
                worker.next_event().await,
                Ok(AgentRuntimeEvent::NativeTerminal(
                    ContextNativeEvent::ResourceAuditSettled(_)
                ))
            ));
            assert!(matches!(
                worker.next_event().await,
                Ok(AgentRuntimeEvent::RendererLost(_))
            ));
        });
    }

    #[test]
    fn cancellation_is_one_shot_and_leaves_terminal_debt_observable() {
        let inner = controller_test_inner();
        let audit = ContextResourceAuditId::new(1).expect("controller test audit identity");
        assert!(inner
            .mailbox
            .native_event_sink()
            .publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Cancelled)),
            ))
            .is_ok());
        inner.seal_and_cancel();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller test runtime");
        runtime.block_on(async {
            let mut worker = controller_test_worker(Arc::clone(&inner));
            assert!(matches!(
                worker.next_event().await,
                Ok(AgentRuntimeEvent::CancellationRequested)
            ));
            assert!(matches!(
                worker.next_event().await,
                Ok(AgentRuntimeEvent::NativeTerminal(
                    ContextNativeEvent::ResourceAuditSettled(_)
                ))
            ));
            assert!(worker.status().cancelled());
            assert!(worker.status().sealed());
        });
    }

    #[test]
    fn lifecycle_shutdown_event_is_not_downgraded_to_cancellation() {
        let inner = controller_test_inner();
        inner.request_cooperative_shutdown_until(Instant::now());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller test runtime");
        runtime.block_on(async {
            let mut worker = controller_test_worker(Arc::clone(&inner));
            assert!(matches!(
                worker.next_event().await,
                Ok(AgentRuntimeEvent::ShutdownRequested)
            ));
            assert!(worker.status().cancelled());
            assert!(worker.status().sealed());
        });
    }

    #[test]
    fn shutdown_deadline_keeps_the_earliest_request_and_wakes_the_worker() {
        let inner = controller_test_inner();
        let initial = Instant::now() + Duration::from_secs(2);
        let earlier = Instant::now() + Duration::from_secs(1);
        let worker = controller_test_worker(Arc::clone(&inner));

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller test runtime");
        runtime.block_on(async {
            let mut first_wake = std::pin::pin!(inner.control_wake.notified());
            first_wake.as_mut().enable();
            inner.request_cooperative_shutdown_until(initial);
            tokio::time::timeout(Duration::from_millis(100), &mut first_wake)
                .await
                .expect("initial shutdown request wakes worker");
            assert_eq!(worker.shutdown_deadline(), Some(initial));

            let mut tighter_wake = std::pin::pin!(inner.control_wake.notified());
            tighter_wake.as_mut().enable();
            inner.request_cooperative_shutdown_until(earlier);
            tokio::time::timeout(Duration::from_millis(100), &mut tighter_wake)
                .await
                .expect("earlier deadline wakes worker for recomputation");
            assert_eq!(worker.shutdown_deadline(), Some(earlier));
        });
    }

    #[test]
    fn shutdown_event_never_precedes_deadline_publication_under_racing_requests() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller test runtime");
        runtime.block_on(async {
            for _ in 0..64 {
                let inner = controller_test_inner();
                let deadline = Instant::now() + Duration::from_secs(1);
                let requester_inner = Arc::clone(&inner);
                let requester = thread::spawn(move || {
                    requester_inner.request_cooperative_shutdown_until(deadline);
                });
                let mut worker = controller_test_worker(inner);
                assert!(matches!(
                    tokio::time::timeout(Duration::from_millis(100), worker.next_event())
                        .await
                        .expect("shutdown event must not lose its wake"),
                    Ok(AgentRuntimeEvent::ShutdownRequested)
                ));
                assert_eq!(worker.shutdown_deadline(), Some(deadline));
                requester.join().expect("shutdown requester joins");
            }
        });
    }

    #[test]
    fn lifecycle_shutdown_settles_active_model_call_and_retains_callback_reconciliation() {
        let _guard = runtime_test_guard();
        let (active_sender, active_receiver) = mpsc::channel();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let (reconciled_sender, reconciled_receiver) = mpsc::channel();
        let (published_sender, published_receiver) = oneshot::channel();
        let (policy, active_call) = active_model_call_for_shutdown_test();
        let pending = spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(ShutdownDrainController {
                policy,
                active_call,
                active: active_sender,
                shutdown: shutdown_sender,
                reconciled: reconciled_sender,
                published: published_receiver,
            }),
        )
        .expect("controller worker starts");
        let sink = pending.native_event_sink();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        let _ticket = handle.start_run().expect("active run admission");
        active_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller owns the active model-call obligation");

        let publisher = thread::spawn(move || {
            let report = shutdown_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("controller settles active call on lifecycle shutdown");
            let audit =
                ContextResourceAuditId::new(1).expect("reconciliation audit identity is nonzero");
            let published = sink.publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
            ));
            let _ = published_sender.send(());
            published.map(|()| report)
        });
        let outcome = lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1));
        assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
        let report = publisher
            .join()
            .expect("callback publisher joins")
            .expect("callback intake remains open for reconciliation");
        assert_eq!(
            report.receipt.settlement(),
            AgentModelCallSettlement::ProviderFailed
        );
        assert_eq!(
            report.receipt.usage_accounting(),
            AgentModelUsageAccounting::ReservationCeiling
        );
        assert_eq!(report.pending_model_calls, 0);
        assert_eq!(report.reserved_model_tokens, 0);
        assert_eq!(report.reserved_cost_micro_usd, 0);
        reconciled_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller reconciles retained terminal callback");
        assert!(completion.is_stopped());
        assert!(handle.status().cancelled());
        assert!(handle.status().sealed());
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn mailbox_fault_allows_bounded_controller_terminal_drain_before_final_close() {
        let _guard = runtime_test_guard();
        let mailbox = AgentRuntimeMailboxConfig::try_new(
            MIN_AGENT_RUNTIME_TERMINAL_CAPACITY,
            MIN_AGENT_RUNTIME_SIGNAL_CAPACITY,
        )
        .expect("published test capacities are valid");
        let config = AgentRuntimeConfig::try_new(mailbox, MIN_AGENT_RUNTIME_COMMAND_CAPACITY)
            .expect("published command capacity is valid");
        let (release_sender, release_receiver) = oneshot::channel();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (drained_sender, drained_receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            config,
            Box::new(FaultDrainController {
                release: release_receiver,
                ready: ready_sender,
                drained: drained_sender,
                expected_terminal_count: MIN_AGENT_RUNTIME_TERMINAL_CAPACITY,
            }),
        )
        .expect("controller worker starts");
        let sink = pending.native_event_sink();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller waits outside the mailbox");

        for value in 1..=MIN_AGENT_RUNTIME_TERMINAL_CAPACITY {
            let audit = ContextResourceAuditId::new(value as u64)
                .expect("terminal test audit identity is nonzero");
            assert!(sink
                .publish(ContextNativeEvent::ResourceAuditSettled(
                    ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
                ))
                .is_ok());
        }
        let overflow_audit =
            ContextResourceAuditId::new((MIN_AGENT_RUNTIME_TERMINAL_CAPACITY + 1) as u64)
                .expect("overflow audit identity is nonzero");
        assert_eq!(
            sink.publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(
                    overflow_audit,
                    Err(ContextPortFailure::Shutdown),
                ),
            )),
            Err(AgentRuntimeMailboxFault::TerminalOverflow)
        );
        release_sender
            .send(())
            .expect("controller release channel remains live");
        assert_eq!(
            drained_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("controller drains retained terminal debt"),
            MIN_AGENT_RUNTIME_TERMINAL_CAPACITY
        );
        wait_stopped(&completion);
        assert_eq!(
            handle.status().mailbox_fault(),
            Some(AgentRuntimeMailboxFault::TerminalOverflow)
        );
        assert!(matches!(
            lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)),
            AgentBrowserShutdownOutcome::Unclean
        ));
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn dropping_a_reaped_lifecycle_preserves_its_published_deadline() {
        let inner = controller_test_inner();
        let deadline = Instant::now() + Duration::from_secs(1);
        inner.request_cooperative_shutdown_until(deadline);
        drop(RuntimeLifecycle {
            inner: Arc::clone(&inner),
            worker: None,
        });
        assert_eq!(inner.shutdown_deadline(), Some(deadline));
    }

    #[test]
    fn noncooperative_controller_is_forced_only_after_lifecycle_deadline() {
        let _guard = runtime_test_guard();
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(DeadlineIgnoringController {
                entered: entered_sender,
                shutdown: shutdown_sender,
                dropped: dropped_sender,
            }),
        )
        .expect("controller worker starts");
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let weak_port = Arc::downgrade(&port);
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("noncooperative controller is running");

        let shutdown = thread::spawn(move || {
            lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1))
        });
        let published_deadline = shutdown_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller observes the published lifecycle deadline");
        assert!(matches!(
            shutdown.join().expect("lifecycle thread joins"),
            AgentBrowserShutdownOutcome::Unclean
        ));
        let dropped_at = dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("host force-drops after deadline");
        assert!(dropped_at >= published_deadline);
        wait_stopped(&completion);
        assert!(handle.status().sealed());
        assert!(weak_port.upgrade().is_none());
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn mailbox_fault_tightens_an_existing_lifecycle_deadline() {
        let _guard = runtime_test_guard();
        let mailbox = AgentRuntimeMailboxConfig::try_new(
            MIN_AGENT_RUNTIME_TERMINAL_CAPACITY,
            MIN_AGENT_RUNTIME_SIGNAL_CAPACITY,
        )
        .expect("published test capacities are valid");
        let config = AgentRuntimeConfig::try_new(mailbox, MIN_AGENT_RUNTIME_COMMAND_CAPACITY)
            .expect("published command capacity is valid");
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (shutdown_sender, shutdown_receiver) = mpsc::channel();
        let (dropped_sender, dropped_receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            config,
            Box::new(DeadlineIgnoringController {
                entered: entered_sender,
                shutdown: shutdown_sender,
                dropped: dropped_sender,
            }),
        )
        .expect("controller worker starts");
        let sink = pending.native_event_sink();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("noncooperative controller is running");

        let long_deadline = Instant::now() + Duration::from_secs(5);
        let shutdown = thread::spawn(move || lifecycle.shutdown_until(long_deadline));
        let published_deadline = shutdown_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller observes the long lifecycle deadline");
        assert_eq!(published_deadline, long_deadline);

        for value in 1..=MIN_AGENT_RUNTIME_TERMINAL_CAPACITY {
            let audit = ContextResourceAuditId::new(value as u64)
                .expect("terminal test audit identity is nonzero");
            assert!(sink
                .publish(ContextNativeEvent::ResourceAuditSettled(
                    ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
                ))
                .is_ok());
        }
        let overflow_audit =
            ContextResourceAuditId::new((MIN_AGENT_RUNTIME_TERMINAL_CAPACITY + 1) as u64)
                .expect("overflow audit identity is nonzero");
        assert_eq!(
            sink.publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(
                    overflow_audit,
                    Err(ContextPortFailure::Shutdown),
                ),
            )),
            Err(AgentRuntimeMailboxFault::TerminalOverflow)
        );
        let dropped_at = dropped_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("fault deadline force-drops noncooperative controller");
        assert!(matches!(
            shutdown.join().expect("lifecycle thread joins"),
            AgentBrowserShutdownOutcome::Unclean
        ));
        assert!(dropped_at < published_deadline);
        assert_eq!(
            handle.status().mailbox_fault(),
            Some(AgentRuntimeMailboxFault::TerminalOverflow)
        );
        assert!(completion.is_stopped());
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn controller_wait_has_no_lost_wake_when_callback_races_registration() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("controller test runtime");
        runtime.block_on(async {
            for value in 1..=16 {
                let inner = controller_test_inner();
                let sink = inner.mailbox.native_event_sink();
                let mut worker = controller_test_worker(inner);
                let producer = tokio::spawn(async move {
                    tokio::task::yield_now().await;
                    let audit =
                        ContextResourceAuditId::new(value).expect("controller test audit identity");
                    sink.publish(ContextNativeEvent::ResourceAuditSettled(
                        ContextResourceAuditSettlement::new(
                            audit,
                            Err(ContextPortFailure::Shutdown),
                        ),
                    ))
                });
                let event = tokio::time::timeout(Duration::from_millis(100), worker.next_event())
                    .await
                    .expect("controller wait must not lose a callback");
                assert!(matches!(
                    event,
                    Ok(AgentRuntimeEvent::NativeTerminal(
                        ContextNativeEvent::ResourceAuditSettled(_)
                    ))
                ));
                assert!(producer.await.expect("producer joins").is_ok());
            }
        });
    }

    #[test]
    fn controller_event_wait_is_not_starved_by_the_fault_monitor() {
        let _guard = runtime_test_guard();
        let (ready_sender, ready_receiver) = mpsc::channel();
        let (settled_sender, settled_receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(EventWaitingController {
                ready: ready_sender,
                settled: settled_sender,
            }),
        )
        .expect("controller worker starts");
        let sink = pending.native_event_sink();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (_handle, completion, lifecycle) = composition.into_parts();
        ready_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller has begun waiting for an event");

        let audit = ContextResourceAuditId::new(1).expect("event test audit identity is nonzero");
        assert!(sink
            .publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
            ))
            .is_ok());
        settled_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("event wait must wake despite the fault monitor");
        wait_stopped(&completion);
        assert!(matches!(
            lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)),
            AgentBrowserShutdownOutcome::Unclean
        ));
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    struct DeepStackController {
        report: mpsc::Sender<(Option<String>, u64)>,
    }

    #[inline(never)]
    fn consume_stack(frames: usize) -> u64 {
        let mut frame = [0u8; 64 * 1024];
        std::hint::black_box(&mut frame);
        if frames == 0 {
            return u64::from(frame[0]);
        }
        consume_stack(frames - 1) + u64::from(std::hint::black_box(frame[frame.len() - 1]))
    }

    impl AgentRuntimeController for DeepStackController {
        fn run(
            self: Box<Self>,
            _worker: AgentRuntimeWorker,
            _browser: AgentRuntimeBrowser,
        ) -> AgentRuntimeControllerFuture {
            Box::pin(async move {
                // Three times the 2 MiB platform default a provider handshake overflowed.
                let depth = consume_stack(6 * 1024 * 1024 / (64 * 1024));
                let _ = self
                    .report
                    .send((std::thread::current().name().map(str::to_owned), depth));
            })
        }
    }

    #[test]
    fn worker_stack_admits_controller_frames_beyond_the_platform_default() {
        let _guard = runtime_test_guard();
        let (report, reported) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(DeepStackController { report }),
        )
        .expect("controller worker starts");
        let (_handle, completion, lifecycle) = pending
            .bind_browser_port(Arc::new(RecordingPort {
                calls: AtomicUsize::new(0),
            }))
            .into_parts();
        let observed = reported.recv_timeout(Duration::from_secs(5));
        wait_stopped(&completion);
        drop(lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)));
        assert_eq!(observed, Ok((Some("zephium-agent-runtime".to_owned()), 0)));
    }

    #[test]
    fn controller_exit_and_panic_release_browser_and_worker_permit() {
        let _guard = runtime_test_guard();
        for controller in [
            Box::new(EarlyExitController) as Box<dyn AgentRuntimeController>,
            Box::new(PanicController) as Box<dyn AgentRuntimeController>,
        ] {
            let pending = spawn_suspended_with_controller(AgentRuntimeConfig::STANDARD, controller)
                .expect("controller worker starts");
            let port = Arc::new(RecordingPort {
                calls: AtomicUsize::new(0),
            });
            let weak_port = Arc::downgrade(&port);
            let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
            let (handle, completion, lifecycle) = composition.into_parts();
            wait_stopped(&completion);
            assert!(handle.status().cancelled());
            assert!(handle.status().sealed());
            assert!(weak_port.upgrade().is_none());
            assert!(matches!(
                lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)),
                AgentBrowserShutdownOutcome::Unclean
            ));
            let next = spawn_after_true_worker_exit();
            drop(next);
        }
    }

    #[test]
    fn lifecycle_shutdown_forcibly_drops_noncooperative_controller() {
        let _guard = runtime_test_guard();
        struct ReleasePostCompletion;
        impl Drop for ReleasePostCompletion {
            fn drop(&mut self) {
                WORKER_POST_COMPLETION_GATE.release();
            }
        }
        WORKER_POST_COMPLETION_GATE.hold();
        let post_completion = ReleasePostCompletion;
        let worker_exit_gate = HeldWorkerExitGate::enable();
        let (started_sender, started_receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            AgentRuntimeConfig::STANDARD,
            Box::new(FaultIgnoringController {
                started: started_sender,
            }),
        )
        .expect("controller worker starts");
        let sink = pending.native_event_sink();
        let audit = ContextResourceAuditId::new(1).expect("controller test audit identity");
        assert!(sink
            .publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
            ))
            .is_ok());
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let weak_port = Arc::downgrade(&port);
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller has started outside the event loop");
        let outcome = lifecycle.shutdown_until(Instant::now());
        assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
        assert!(!completion.is_stopped());
        worker_exit_gate.release();
        assert!(completion
            .inner
            .completion
            .wait_until(Instant::now() + Duration::from_secs(1)));
        assert!(handle.status().sealed());
        assert_eq!(
            handle.status().mailbox_fault(),
            Some(AgentRuntimeMailboxFault::Closed)
        );
        assert!(weak_port.upgrade().is_none());
        drop(post_completion);
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn mailbox_fault_forcibly_drops_a_controller_that_never_polls_events() {
        let _guard = runtime_test_guard();
        let mailbox = AgentRuntimeMailboxConfig::try_new(
            MIN_AGENT_RUNTIME_TERMINAL_CAPACITY,
            MIN_AGENT_RUNTIME_SIGNAL_CAPACITY,
        )
        .expect("published test capacities are valid");
        let config = AgentRuntimeConfig::try_new(mailbox, MIN_AGENT_RUNTIME_COMMAND_CAPACITY)
            .expect("published command capacity is valid");
        let (started_sender, started_receiver) = mpsc::channel();
        let pending = spawn_suspended_with_controller(
            config,
            Box::new(FaultIgnoringController {
                started: started_sender,
            }),
        )
        .expect("controller worker starts");
        let sink = pending.native_event_sink();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let weak_port = Arc::downgrade(&port);
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("controller has begun waiting outside next_event");

        for value in 1..=MIN_AGENT_RUNTIME_TERMINAL_CAPACITY {
            let audit = ContextResourceAuditId::new(value as u64)
                .expect("terminal test audit identity is nonzero");
            assert!(sink
                .publish(ContextNativeEvent::ResourceAuditSettled(
                    ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown),),
                ))
                .is_ok());
        }
        let overflow_audit =
            ContextResourceAuditId::new((MIN_AGENT_RUNTIME_TERMINAL_CAPACITY + 1) as u64)
                .expect("overflow test audit identity is nonzero");
        assert_eq!(
            sink.publish(ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(
                    overflow_audit,
                    Err(ContextPortFailure::Shutdown),
                ),
            )),
            Err(AgentRuntimeMailboxFault::TerminalOverflow)
        );

        wait_stopped(&completion);
        assert!(handle.status().cancelled());
        assert!(handle.status().sealed());
        assert_eq!(
            handle.status().mailbox_fault(),
            Some(AgentRuntimeMailboxFault::TerminalOverflow)
        );
        assert!(weak_port.upgrade().is_none());
        assert!(matches!(
            lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1)),
            AgentBrowserShutdownOutcome::Unclean
        ));
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn suspended_worker_does_not_touch_a_port_and_drop_before_bind_joins() {
        let _guard = runtime_test_guard();
        let start = Instant::now();
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let native = pending.native_event_sink();
        let joined = pending.inner.joined.clone();
        drop(native);
        drop(pending);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(joined.wait_until(Instant::now() + Duration::from_secs(1)));
    }

    #[test]
    fn ownership_admits_one_run_and_shutdown_never_claims_clean() {
        let _guard = runtime_test_guard();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let composition = pending.bind_browser_port(Arc::clone(&port) as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        let ticket = handle.start_run().expect("first run admission");
        assert_eq!(ticket.get(), 1);
        assert_eq!(handle.start_run(), Err(AgentRunAdmissionRefusal::Active));
        handle.cancel_and_seal();
        let status = handle.status();
        assert!(!status.admitted());
        assert!(status.cancelled());
        assert!(status.sealed());
        assert_eq!(port.calls(), 0);
        let outcome = lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1));
        assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
        assert!(completion.is_stopped());
        assert_eq!(port.calls(), 0);
    }

    #[test]
    fn expired_deadline_is_unclean_and_stop_request_remains_sticky() {
        let _guard = runtime_test_guard();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        let outcome = lifecycle.shutdown_until(Instant::now());
        assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
        assert!(handle.status().sealed());
        wait_stopped(&completion);
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn logical_completion_cannot_extend_shutdown_through_unfinished_thread_teardown() {
        let _guard = runtime_test_guard();
        struct ReleasePostCompletion;
        impl Drop for ReleasePostCompletion {
            fn drop(&mut self) {
                WORKER_POST_COMPLETION_GATE.release();
            }
        }
        WORKER_POST_COMPLETION_GATE.hold();
        let release = ReleasePostCompletion;
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).unwrap();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let (handle, completion, lifecycle) = pending.bind_browser_port(port).into_parts();
        handle.stop_and_seal(AgentRuntimeStopReason::Cancelled);
        wait_stopped(&completion);
        assert!(RUNTIME_WORKER_HELD.load(Ordering::Acquire));
        let (sent, received) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_millis(20);
        let caller = std::thread::spawn(move || {
            sent.send(lifecycle.shutdown_until(deadline)).unwrap();
        });
        let outcome = received.recv_timeout(Duration::from_millis(200));
        assert!(RUNTIME_WORKER_HELD.load(Ordering::Acquire));
        drop(release);
        caller.join().unwrap();
        assert!(
            matches!(outcome, Ok(AgentBrowserShutdownOutcome::Unclean)),
            "logical completion must not make caller join unfinished teardown: {outcome:?}"
        );
        drop(spawn_after_true_worker_exit());
    }

    #[test]
    fn failed_reaper_spawn_retains_the_worker_without_extending_the_deadline() {
        let _guard = runtime_test_guard();
        let forced_failure = ForcedReaperSpawnFailure::enable();
        let worker_exit_gate = HeldWorkerExitGate::enable();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (_handle, completion, lifecycle) = composition.into_parts();
        let start = Instant::now();
        let outcome = lifecycle.shutdown_until(Instant::now());
        assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
        assert!(start.elapsed() < Duration::from_secs(1));
        // The lifecycle's Arc travels into the emergency bundle before this
        // method returns, so admission remains closed across the handoff even
        // if the worker races to exit immediately afterwards.
        assert!(RUNTIME_WORKER_HELD.load(Ordering::Acquire));
        assert!(!recover_lock(&EMERGENCY_WORKER_REAP).is_empty());
        assert!(!completion.is_stopped());

        worker_exit_gate.release();
        wait_stopped(&completion);
        assert!(RUNTIME_WORKER_HELD.load(Ordering::Acquire));
        assert!(matches!(
            spawn_suspended(AgentRuntimeConfig::STANDARD),
            Err(RuntimeSpawnError::AlreadyRunning)
        ));
        drop(forced_failure);
        let next = spawn_after_true_worker_exit();
        assert!(recover_lock(&EMERGENCY_WORKER_REAP).is_empty());
        drop(next);
    }

    #[test]
    fn public_debug_is_content_free() {
        let _guard = runtime_test_guard();
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let debug = format!("{:#?}", AgentRuntimeConfig::STANDARD);
        assert!(!debug.contains("http"));
        drop(pending);
    }

    #[test]
    fn process_wide_worker_permit_covers_pending_bound_and_true_worker_exit() {
        let _guard = runtime_test_guard();
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("first worker starts");
        assert!(matches!(
            spawn_suspended(AgentRuntimeConfig::STANDARD),
            Err(RuntimeSpawnError::AlreadyRunning)
        ));

        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (_handle, completion, lifecycle) = composition.into_parts();
        assert!(matches!(
            spawn_suspended(AgentRuntimeConfig::STANDARD),
            Err(RuntimeSpawnError::AlreadyRunning)
        ));
        let outcome = lifecycle.shutdown_until(Instant::now() + Duration::from_secs(1));
        assert!(matches!(outcome, AgentBrowserShutdownOutcome::Unclean));
        assert!(completion.is_stopped());

        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn terminal_overflow_reason_wins_while_terminal_debt_is_ready() {
        let config = AgentRuntimeMailboxConfig::try_new(
            MIN_AGENT_RUNTIME_TERMINAL_CAPACITY,
            MIN_AGENT_RUNTIME_SIGNAL_CAPACITY,
        )
        .expect("published test capacities are valid");
        let mailbox = AgentRuntimeMailbox::new(config);
        let sink = mailbox.native_event_sink();
        for value in 1..=MIN_AGENT_RUNTIME_TERMINAL_CAPACITY {
            let audit =
                ContextResourceAuditId::new(value as u64).expect("test audit identity is nonzero");
            let event = ContextNativeEvent::ResourceAuditSettled(
                ContextResourceAuditSettlement::new(audit, Err(ContextPortFailure::Shutdown)),
            );
            assert!(sink.publish(event).is_ok());
        }
        let overflow_audit =
            ContextResourceAuditId::new((MIN_AGENT_RUNTIME_TERMINAL_CAPACITY + 1) as u64)
                .expect("test overflow audit identity is nonzero");
        let overflow_event = ContextNativeEvent::ResourceAuditSettled(
            ContextResourceAuditSettlement::new(overflow_audit, Err(ContextPortFailure::Shutdown)),
        );
        assert_eq!(
            sink.publish(overflow_event),
            Err(AgentRuntimeMailboxFault::TerminalOverflow)
        );

        let inner = RuntimeInner {
            scope: None,
            scoped_closure: Mutex::new(None),
            mailbox,
            commands: ArrayQueue::new(MIN_AGENT_RUNTIME_COMMAND_CAPACITY),
            control_wake: Notify::new(),
            run_state: AtomicU8::new(RUN_IDLE),
            next_ticket: AtomicU64::new(1),
            current_ticket: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            stop_reason: AtomicU8::new(0),
            shutdown_requested: AtomicBool::new(false),
            shutdown_deadline: Mutex::new(None),
            terminal_claim_gate: Mutex::new(()),
            terminal_claim_class: AtomicU8::new(TERMINAL_CLASS_NONE),
            shutdown_closure: Mutex::new(None),
            controller_returned: AtomicBool::new(false),
            fault_shutdown_requested: AtomicBool::new(false),
            staged_stop_reason: AtomicU8::new(STAGED_STOP_NONE),
            completion: CompletionState::new(),
            joined: Arc::new(WorkerJoinCompletion::new()),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime starts");
        runtime.block_on(fail_staged_mailbox_wake(
            &inner,
            AgentRuntimeMailboxWake::TerminalReady,
        ));

        assert_eq!(
            inner.status().staged_stop_reason(),
            Some(AgentRuntimeStagedStopReason::TerminalMailboxOverflow)
        );
        assert!(inner.status().cancelled());
        assert!(inner.status().sealed());
        assert!(inner.mailbox.try_pop().is_none());
    }

    #[test]
    fn unexpected_real_native_event_is_consumed_and_fail_stops_the_staged_shell() {
        let _guard = runtime_test_guard();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let sink = pending.native_event_sink();
        let audit = ContextResourceAuditId::new(1).expect("nonzero audit identity");
        let event = ContextNativeEvent::ResourceAuditSettled(ContextResourceAuditSettlement::new(
            audit,
            Err(ContextPortFailure::Shutdown),
        ));
        let result = sink.publish(event);
        assert!(result.is_ok() || result == Err(AgentRuntimeMailboxFault::Closed));
        let composition = pending.bind_browser_port(Arc::clone(&port) as Arc<dyn AgentBrowserPort>);
        let (handle, completion, lifecycle) = composition.into_parts();
        wait_stopped(&completion);
        let status = handle.status();
        assert!(status.cancelled());
        assert!(status.sealed());
        assert_eq!(
            status.staged_stop_reason(),
            Some(AgentRuntimeStagedStopReason::UnexpectedNativeEvent)
        );
        assert_eq!(port.calls(), 0);
        drop(lifecycle);
        let next = spawn_after_true_worker_exit();
        drop(next);
    }

    #[test]
    fn dropping_lifecycle_reaps_the_worker_and_releases_the_permit() {
        let _guard = runtime_test_guard();
        let port = Arc::new(RecordingPort {
            calls: AtomicUsize::new(0),
        });
        let pending = spawn_suspended(AgentRuntimeConfig::STANDARD).expect("worker starts");
        let composition = pending.bind_browser_port(port as Arc<dyn AgentBrowserPort>);
        let (_handle, completion, lifecycle) = composition.into_parts();
        drop(lifecycle);
        wait_stopped(&completion);
        let next = spawn_after_true_worker_exit();
        drop(next);
    }
}
