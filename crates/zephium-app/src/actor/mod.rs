//! Actor ownership, worker lifetime, and terminal shutdown.

mod mailbox;
#[cfg(test)]
mod tests;

use mailbox::CommandQueueInner;
pub(super) use mailbox::{CommandQueue, TimerWake, TryPushError};

use std::sync::mpsc::{sync_channel, Receiver, TrySendError};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::thread;
use std::thread::JoinHandle;

use crate::shell::{
    Shell, ShellPorts, END_TO_END_SHUTDOWN_TIMEOUT, MAINTENANCE_INTERVAL, MAX_OPERATION_ID_BYTES,
};
use crate::store_reads::{run as run_store_reader, StoreReadQueue, StoreReaderStopGuard};
#[cfg(feature = "agentic-browser")]
use crate::AgentLifecycle;
use crate::{
    Command, ContentPolicyStatusQueryOutcome, EmitFn, SharedBlocker, SharedChrome, SharedEngine,
    SharedStore, ShellTerminalFailureCallback, ShutdownOutcome,
};
use zephium_core::ids::{ItemId, ProfileId};
use zephium_ipc::BlockerStatusView;

const FAILED_SPAWN_CLEANUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
type WorkerTask = Box<dyn FnOnce() + Send + 'static>;

trait PendingAgentLifecycle: Send + 'static {
    type Failure;

    #[cfg(feature = "agentic-browser")]
    fn into_shell_lifecycle(self) -> Option<AgentLifecycle>;

    fn into_spawn_failure(self, error: SpawnError, worker_cleanup_proven: bool) -> Self::Failure;
}

struct NoAgentLifecycle;

impl PendingAgentLifecycle for NoAgentLifecycle {
    type Failure = SpawnFailure;

    #[cfg(feature = "agentic-browser")]
    fn into_shell_lifecycle(self) -> Option<AgentLifecycle> {
        None
    }

    fn into_spawn_failure(self, error: SpawnError, worker_cleanup_proven: bool) -> Self::Failure {
        SpawnFailure::new(error, worker_cleanup_proven)
    }
}

#[cfg(feature = "agentic-browser")]
struct PendingAgentBrowserLifecycle(AgentLifecycle);

#[cfg(feature = "agentic-browser")]
impl PendingAgentLifecycle for PendingAgentBrowserLifecycle {
    type Failure = AgenticSpawnFailure;

    fn into_shell_lifecycle(self) -> Option<AgentLifecycle> {
        Some(self.0)
    }

    fn into_spawn_failure(self, error: SpawnError, worker_cleanup_proven: bool) -> Self::Failure {
        AgenticSpawnFailure::new(error, self.0, worker_cleanup_proven)
    }
}

struct ShellHandoff<Agent = NoAgentLifecycle> {
    engine: SharedEngine,
    store: SharedStore,
    blocker: SharedBlocker,
    agent_lifecycle: Agent,
    terminal_failure: ShellTerminalFailureCallback,
    chrome: SharedChrome,
    emit: EmitFn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActorStartupState {
    Pending,
    Waiting,
    Admitted,
    Started,
    Cancelled,
}

/// Exact composition-root admission between guarded Shell construction and the
/// first external port call. Cancellation may overtake admission until the
/// actor consumes it, closing the concurrent terminal-start window.
struct ActorStartupGate {
    state: Mutex<ActorStartupState>,
    changed: Condvar,
}

impl ActorStartupGate {
    fn new() -> Self {
        Self {
            state: Mutex::new(ActorStartupState::Pending),
            changed: Condvar::new(),
        }
    }

    fn admit(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(
            *state,
            ActorStartupState::Pending | ActorStartupState::Waiting
        ) {
            return false;
        }
        *state = ActorStartupState::Admitted;
        self.changed.notify_one();
        true
    }

    fn cancel(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !matches!(
            *state,
            ActorStartupState::Pending | ActorStartupState::Waiting | ActorStartupState::Admitted
        ) {
            return false;
        }
        *state = ActorStartupState::Cancelled;
        self.changed.notify_one();
        true
    }

    /// Makes an already-admitted transition irrevocable for the compatibility
    /// `spawn` API. The suspended composition path deliberately does not call
    /// this: its terminal coordinator may still overtake admission until the
    /// actor consumes the gate.
    fn commit_admission(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match *state {
            ActorStartupState::Admitted => {
                *state = ActorStartupState::Started;
                self.changed.notify_one();
                true
            }
            ActorStartupState::Started => true,
            ActorStartupState::Pending
            | ActorStartupState::Waiting
            | ActorStartupState::Cancelled => false,
        }
    }

    fn wait_for_admission(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            match *state {
                ActorStartupState::Pending => {
                    *state = ActorStartupState::Waiting;
                    self.changed.notify_all();
                }
                ActorStartupState::Waiting => {
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                ActorStartupState::Admitted => {
                    *state = ActorStartupState::Started;
                    return true;
                }
                ActorStartupState::Started => return true,
                ActorStartupState::Cancelled => return false,
            }
        }
    }

    #[cfg(test)]
    fn wait_until_waiting(&self, deadline: std::time::Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *state == ActorStartupState::Pending {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (next, timeout) = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = next;
            if timeout.timed_out() && *state == ActorStartupState::Pending {
                return false;
            }
        }
        *state == ActorStartupState::Waiting
    }
}

pub struct Handle {
    pub(super) queue: CommandQueue,
    pub(super) workers: Arc<WorkerThreads>,
    startup: Arc<ActorStartupGate>,
    pub(super) counted: bool,
}

pub struct ShutdownRequest {
    deadline: std::time::Instant,
    receiver: Receiver<ShutdownOutcome>,
    pub(super) workers: Arc<WorkerThreads>,
}

#[must_use = "the bounded status request must be received or deliberately dropped"]
pub struct ContentPolicyStatusRequest {
    receiver: Receiver<ContentPolicyStatusQueryOutcome>,
}

#[must_use = "the bounded focused status request must be received or deliberately dropped"]
pub struct FocusedContentPolicyStatusRequest {
    receiver: Receiver<BlockerStatusView>,
}

#[derive(Default)]
pub(super) struct WorkerThreads {
    state: Mutex<WorkerThreadState>,
    work_pending: crate::work_authoring::Pending,
}

#[derive(Default)]
struct WorkerThreadState {
    actor: Option<WorkerThread>,
    timer: Option<WorkerThread>,
    store_reader: Option<WorkerThread>,
}

struct WorkerThread {
    join: JoinHandle<()>,
    exited: Receiver<()>,
}

#[derive(Debug)]
pub enum SpawnError {
    Actor(std::io::Error),
    ActorHandoff(std::io::Error),
    Timer(std::io::Error),
    StoreReader(std::io::Error),
}

/// Application-composition failure, with proof of whether the helper workers
/// admitted before the failure were reaped inside the bounded cleanup budget.
pub struct SpawnFailure {
    error: SpawnError,
    worker_cleanup_proven: bool,
}

impl SpawnFailure {
    fn new(error: SpawnError, worker_cleanup_proven: bool) -> Self {
        Self {
            error,
            worker_cleanup_proven,
        }
    }

    pub fn error(&self) -> &SpawnError {
        &self.error
    }

    /// Whether every app-owned helper worker admitted before the failure was
    /// observed exited and joined before the rollback deadline.
    pub fn worker_cleanup_proven(&self) -> bool {
        self.worker_cleanup_proven
    }
}

impl std::fmt::Debug for SpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpawnFailure")
            .field("error", &self.error)
            .field("worker_cleanup_proven", &self.worker_cleanup_proven)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for SpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

impl std::error::Error for SpawnFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Lossless agentic application-composition failure.
///
/// This is distinct from [`SpawnFailure`] so the ordinary spawn API cannot
/// accidentally acquire or discard an agent lifecycle. The move-only owner is
/// returned untouched; the composition root must consume it away from a
/// native UI/event-loop thread.
#[cfg(feature = "agentic-browser")]
#[must_use = "recover and explicitly dispose the returned agent lifecycle owner"]
pub struct AgenticSpawnFailure {
    error: SpawnError,
    agent_lifecycle: AgentLifecycle,
    worker_cleanup_proven: bool,
}

#[cfg(feature = "agentic-browser")]
impl AgenticSpawnFailure {
    fn new(
        error: SpawnError,
        agent_lifecycle: AgentLifecycle,
        worker_cleanup_proven: bool,
    ) -> Self {
        Self {
            error,
            agent_lifecycle,
            worker_cleanup_proven,
        }
    }

    /// Concrete helper-worker construction or handoff failure.
    pub fn error(&self) -> &SpawnError {
        &self.error
    }

    /// Whether every admitted app helper worker was reaped by the rollback deadline.
    pub fn worker_cleanup_proven(&self) -> bool {
        self.worker_cleanup_proven
    }

    /// Recovers the failure and the unique, never-settled agent lifecycle owner.
    pub fn into_parts(self) -> (SpawnError, AgentLifecycle) {
        (self.error, self.agent_lifecycle)
    }
}

#[cfg(feature = "agentic-browser")]
impl std::fmt::Debug for AgenticSpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgenticSpawnFailure")
            .field("error", &self.error)
            .field("worker_cleanup_proven", &self.worker_cleanup_proven)
            .finish_non_exhaustive()
    }
}

#[cfg(feature = "agentic-browser")]
impl std::fmt::Display for AgenticSpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(formatter)
    }
}

#[cfg(feature = "agentic-browser")]
impl std::error::Error for AgenticSpawnFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Actor(error) => write!(formatter, "could not start shell actor: {error}"),
            Self::ActorHandoff(error) => {
                write!(formatter, "could not hand the shell to its actor: {error}")
            }
            Self::Timer(error) => write!(formatter, "could not start shell timer: {error}"),
            Self::StoreReader(error) => {
                write!(formatter, "could not start shell storage reader: {error}")
            }
        }
    }
}

impl std::error::Error for SpawnError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Actor(error)
            | Self::ActorHandoff(error)
            | Self::Timer(error)
            | Self::StoreReader(error) => Some(error),
        }
    }
}

fn spawn_worker(name: &'static str, task: WorkerTask) -> std::io::Result<WorkerThread> {
    let (exited, exit_proof) = sync_channel(1);
    let join = thread::Builder::new().name(name.into()).spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
        // The task's stack and owned ports have unwound before this signal.
        // Re-raise a panic so the retained JoinHandle still reports failure.
        let _ = exited.send(());
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    })?;
    Ok(WorkerThread {
        join,
        exited: exit_proof,
    })
}

fn join_worker_until(slot: &mut Option<WorkerThread>, deadline: std::time::Instant) -> bool {
    let Some(worker) = slot.as_mut() else {
        return true;
    };
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() || worker.exited.recv_timeout(remaining).is_err() {
        return false;
    }
    while !worker.join.is_finished() && std::time::Instant::now() < deadline {
        thread::yield_now();
    }
    if !worker.join.is_finished() {
        return false;
    }
    slot.take().is_some_and(|worker| worker.join.join().is_ok())
}

impl WorkerThreads {
    #[cfg(test)]
    pub(super) fn actor_and_timer_stopped(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.actor.is_none() && state.timer.is_none()
    }

    fn install_actor(&self, worker: WorkerThread) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .actor = Some(worker);
    }

    fn install_timer(&self, worker: WorkerThread) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .timer = Some(worker);
    }

    fn install_store_reader(&self, worker: WorkerThread) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .store_reader = Some(worker);
    }

    fn join_until(&self, deadline: std::time::Instant) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let actor_clean = join_worker_until(&mut state.actor, deadline);
        let timer_clean = join_worker_until(&mut state.timer, deadline);
        let store_reader_clean = join_worker_until(&mut state.store_reader, deadline);
        actor_clean && timer_clean && store_reader_clean
    }
}

fn cleanup_failed_workers(
    queue: &CommandQueue,
    store_reads: &StoreReadQueue,
    workers: &WorkerThreads,
) -> bool {
    let deadline = std::time::Instant::now() + FAILED_SPAWN_CLEANUP_TIMEOUT;
    let queue_clean = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for pending in queue.close_and_drain() {
            finish_unprocessed_command(pending, ShutdownOutcome::Unclean);
        }
    }))
    .is_ok();
    let reads_clean =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store_reads.stop())).is_ok();
    let workers_clean = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        workers.join_until(deadline)
    }))
    .unwrap_or(false);
    let clean = queue_clean && reads_clean && workers_clean;
    if !clean {
        crate::diagnostic!(
            "startup: app helper-worker cleanup was not proven after construction failed"
        );
    }
    clean
}

impl ShutdownRequest {
    pub fn deadline(&self) -> std::time::Instant {
        self.deadline
    }

    /// Waits only until the process-wide shutdown deadline. Shutdown callers
    /// must never regain an unbounded wait merely by choosing this shorthand.
    pub fn recv(&self) -> Result<ShutdownOutcome, std::sync::mpsc::RecvTimeoutError> {
        self.recv_until_deadline()
    }

    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<ShutdownOutcome, std::sync::mpsc::RecvTimeoutError> {
        let now = std::time::Instant::now();
        let deadline = now
            .checked_add(timeout)
            .unwrap_or(self.deadline)
            .min(self.deadline);
        self.receiver
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .map(|outcome| self.finish(outcome, deadline))
    }

    pub fn recv_until_deadline(
        &self,
    ) -> Result<ShutdownOutcome, std::sync::mpsc::RecvTimeoutError> {
        self.receiver
            .recv_timeout(
                self.deadline
                    .saturating_duration_since(std::time::Instant::now()),
            )
            .map(|outcome| self.finish(outcome, self.deadline))
    }

    fn finish(&self, outcome: ShutdownOutcome, deadline: std::time::Instant) -> ShutdownOutcome {
        if outcome == ShutdownOutcome::RetryableFailure {
            return outcome;
        }
        if self.workers.join_until(deadline) {
            outcome
        } else {
            crate::diagnostic!(
                "shutdown: shell, timer and storage-reader worker joins were not proven before the shared deadline"
            );
            // A clean native/store acknowledgement is insufficient when the
            // owning shell or timer thread did not terminate under the same
            // process-boundary budget.
            ShutdownOutcome::Unclean
        }
    }
}

impl ContentPolicyStatusRequest {
    /// Waits at most `timeout` for the actor-ordered status. Timeout, actor
    /// exit, and failed admission all become the explicit `Unavailable`
    /// result; callers never need an unbounded wait to inspect policy state.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> ContentPolicyStatusQueryOutcome {
        self.receiver
            .recv_timeout(timeout)
            .unwrap_or(ContentPolicyStatusQueryOutcome::Unavailable)
    }
}

impl FocusedContentPolicyStatusRequest {
    /// Waits at most `timeout` for the actor-ordered focused-profile view.
    /// Queue rejection, timeout, and actor exit return revision-zero
    /// `Unavailable`, which cannot regress a real projection.
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> BlockerStatusView {
        self.receiver
            .recv_timeout(timeout)
            .unwrap_or_else(|_| BlockerStatusView::unavailable())
    }
}

/// Non-owning ingress for callbacks retained by an engine or another shell
/// dependency. It deliberately does not count as a public owner: otherwise
/// Shell -> Engine -> callback Handle -> queue keeps both actor threads alive
/// forever after the application releases its managed Handle.
#[derive(Clone)]
pub struct CallbackHandle {
    pub(super) queue: Weak<CommandQueueInner>,
}

impl CallbackHandle {
    /// Sticky retained state is polled after every Shell command. A full queue
    /// already owns that progress; a sealed queue owns the shutdown barrier,
    /// which waits on the original resource notification epoch instead. Only
    /// a permanently closed actor has lost the application wake route.
    #[cfg(feature = "work-execution")]
    pub(crate) fn wake_retained_work(&self) -> bool {
        let Some(inner) = self.queue.upgrade() else {
            return false;
        };
        !matches!(
            (CommandQueue { inner }).try_push(Command::WorkWake),
            Err(TryPushError::Closed(_))
        )
    }
    pub fn dispatch(&self, command: Command) -> bool {
        let Some(inner) = self.queue.upgrade() else {
            return false;
        };
        CommandQueue { inner }.try_push(command).is_ok()
    }
}

impl Clone for Handle {
    fn clone(&self) -> Self {
        let counted = self.queue.retain_handle();
        Self {
            queue: self.queue.clone(),
            workers: self.workers.clone(),
            startup: self.startup.clone(),
            counted,
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // The actor owns an internal queue reference so native callbacks can
        // re-enter it, but that reference must not keep the actor and ticker
        // alive after every public owner has gone away.
        if self.counted && self.queue.release_handle() {
            self.startup.cancel();
        }
    }
}

impl Handle {
    pub fn work_call(
        &self,
        profile: zephium_core::ids::ProfileId,
        call: zephium_ipc::work::WorkCallV1,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(call.into_request()?, Some(profile))
    }

    pub fn work_authoring_command(
        &self,
        profile: zephium_core::ids::ProfileId,
        command: zephium_ipc::work::WorkAuthoringCommandV1,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(command.into_request()?, Some(profile))
    }

    pub fn work_query(
        &self,
        profile: zephium_core::ids::ProfileId,
        query: zephium_ipc::work::WorkQueryV1,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(query.into_request()?, Some(profile))
    }

    /// Revision-checked runtime user intent, bound to the displayed profile.
    /// Store reconciliation never returns a live worker capability.
    pub fn work_command(
        &self,
        profile: zephium_core::ids::ProfileId,
        command: zephium_ipc::work::WorkCommandV1,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(command.into_request()?, Some(profile))
    }

    pub fn work_projection(
        &self,
        profile: zephium_core::ids::ProfileId,
        id: zephium_core::work::WorkId,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(
            zephium_core::work::port::WorkRequest::RuntimeRead { id },
            Some(profile),
        )
    }

    pub fn work_evidence(
        &self,
        profile: zephium_core::ids::ProfileId,
        id: zephium_core::work::WorkId,
        link: zephium_core::work::artifact::WorkEvidenceLink,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(
            zephium_core::work::port::WorkRequest::ReadEvidence { id, link },
            Some(profile),
        )
    }

    /// Author a persistent Work under the actor-selected regular profile. This
    /// is a typed Rust intent seam, not a raw manifest or execution endpoint.
    pub fn work_document(
        &self,
        intent: crate::WorkIntent,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        self.submit_work_document(intent.into_request()?, None)
    }

    pub(crate) fn submit_work_document(
        &self,
        request: zephium_core::work::port::WorkRequest,
        owner: Option<zephium_core::ids::ProfileId>,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        let (submission, receiver) = crate::work_authoring::WorkDocumentSubmission::prepare_bound(
            &self.workers.work_pending,
            request,
            owner,
        )?;
        self.queue_work_document(submission, receiver)
    }

    #[cfg(feature = "work-runtime")]
    pub(crate) fn submit_owned_work_runtime(
        &self,
        request: zephium_core::work::port::WorkRequest,
        owner: zephium_core::ids::ProfileId,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        let (submission, receiver) = crate::work_authoring::WorkDocumentSubmission::prepare_pinned(
            &self.workers.work_pending,
            request,
            owner,
        )?;
        self.queue_work_document(submission, receiver)
    }

    fn queue_work_document(
        &self,
        submission: crate::WorkDocumentSubmission,
        receiver: crate::WorkDocumentRequest,
    ) -> Result<crate::WorkDocumentRequest, zephium_core::work::WorkError> {
        match self.queue.try_push(Command::WorkDocument(submission)) {
            Ok(()) => Ok(receiver),
            Err(TryPushError::Full(_)) => Err(zephium_core::work::WorkError::Capacity),
            Err(TryPushError::Sealed(_) | TryPushError::Closed(_)) => {
                Err(zephium_core::work::WorkError::Shutdown)
            }
        }
    }

    pub fn web_extension_target(
        &self,
        tab: Option<zephium_core::ids::ItemId>,
    ) -> std::sync::mpsc::Receiver<Option<crate::shell::WebExtensionTarget>> {
        let (reply, receiver) = sync_channel(1);
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self
            .queue
            .try_push(Command::ResolveWebExtensionTarget { tab, reply })
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    pub fn web_extension_status(
        &self,
        profile: zephium_core::ids::ProfileId,
    ) -> std::sync::mpsc::Receiver<
        Vec<(
            zephium_core::ids::ExtensionInstallId,
            crate::shell::WebExtensionStatus,
        )>,
    > {
        let (reply, receiver) = sync_channel(1);
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self
            .queue
            .try_push(Command::WebExtensionStatus { profile, reply })
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    #[cfg(test)]
    pub(super) fn new(queue: CommandQueue) -> Self {
        Self::with_workers(
            queue,
            Arc::new(WorkerThreads::default()),
            Arc::new(ActorStartupGate::new()),
        )
    }

    fn with_workers(
        queue: CommandQueue,
        workers: Arc<WorkerThreads>,
        startup: Arc<ActorStartupGate>,
    ) -> Self {
        let counted = queue.retain_handle();
        Self {
            queue,
            workers,
            startup,
            counted,
        }
    }

    /// Authorizes the guarded actor to enter its first external composition
    /// port. Desktop calls this only after publishing this Handle and clearing
    /// every temporary rollback owner. The transition is exactly once.
    #[must_use = "startup admission must be observed because cancellation is terminal"]
    pub fn admit_startup(&self) -> bool {
        self.startup.admit()
    }

    fn commit_startup_admission(&self) -> bool {
        self.startup.commit_admission()
    }

    #[cfg(test)]
    pub(super) fn wait_until_startup_suspended(&self, deadline: std::time::Instant) -> bool {
        self.startup.wait_until_waiting(deadline)
    }

    /// Returns a non-owning callback ingress. Long-lived dependencies owned
    /// by the shell must use this instead of retaining a strong Handle.
    pub fn callback_handle(&self) -> CallbackHandle {
        CallbackHandle {
            queue: Arc::downgrade(&self.queue.inner),
        }
    }

    /// Attempts to enqueue without ever blocking the caller. Native WebView
    /// callbacks and window messages can execute on the UI thread; applying
    /// backpressure there can deadlock an ordered shutdown that is waiting for
    /// main-thread destruction. Overload is therefore bounded and fail-closed.
    pub fn dispatch(&self, cmd: Command) -> bool {
        self.queue.try_push(cmd).is_ok()
    }

    /// Admits a user mutation with a stable process-local identity. A `true`
    /// return is an exact FIFO ownership transfer; the queue will neither
    /// coalesce nor evict the wrapper after reporting acceptance.
    pub fn dispatch_operation(&self, operation_id: String, command: Command) -> bool {
        if operation_id.is_empty()
            || operation_id.len() > MAX_OPERATION_ID_BYTES
            || !tracked_operation_command(&command)
        {
            return false;
        }
        self.dispatch(Command::Operation {
            operation_id,
            command: Box::new(command),
        })
    }

    /// Requests an actor-ordered snapshot of one profile's content policy.
    ///
    /// This is intentionally an in-process API. The desktop does not expose
    /// it to raw page content, and callers must use the request's bounded
    /// receive method instead of blocking a native UI thread indefinitely.
    pub fn content_policy_status(&self, profile: ProfileId) -> ContentPolicyStatusRequest {
        let (reply, receiver) = sync_channel(1);
        let command = Command::ContentPolicyStatus { profile, reply };
        match self.queue.try_push(command) {
            Ok(()) => {}
            Err(
                TryPushError::Full(command)
                | TryPushError::Sealed(command)
                | TryPushError::Closed(command),
            ) => {
                finish_unprocessed_command(command, ShutdownOutcome::Unclean);
            }
        }
        ContentPolicyStatusRequest { receiver }
    }

    /// Requests the focused profile's revisioned diagnostics without exposing
    /// a caller-selected profile identity.
    pub fn element_picker(
        &self,
        context: zephium_ipc::BlockerSiteContext,
        action: zephium_ipc::BlockerPickerAction,
    ) -> Receiver<Option<zephium_ipc::BlockerPickerView>> {
        let (reply, receiver) = sync_channel(1);
        let command = Command::ElementPicker {
            context: Box::new(context),
            action,
            reply,
        };
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self.queue.try_push(command)
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }
    pub fn blocker_statistics(
        &self,
        profile: ProfileId,
    ) -> std::sync::mpsc::Receiver<Option<zephium_ipc::BlockerStatsView>> {
        let (reply, receiver) = sync_channel(1);
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self
            .queue
            .try_push(Command::BlockerStatistics { profile, reply })
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }
    pub fn focused_content_policy_status(&self) -> FocusedContentPolicyStatusRequest {
        let (reply, receiver) = sync_channel(1);
        let command = Command::FocusedContentPolicyStatus { reply };
        match self.queue.try_push(command) {
            Ok(()) => {}
            Err(
                TryPushError::Full(command)
                | TryPushError::Sealed(command)
                | TryPushError::Closed(command),
            ) => {
                finish_unprocessed_command(command, ShutdownOutcome::Unclean);
            }
        }
        FocusedContentPolicyStatusRequest { receiver }
    }

    /// Selects the current browser profile on the actor, never from UI/model IDs.
    #[cfg(feature = "work-execution")]
    pub fn work_profile_binding(&self) -> crate::AgentWorkProfileRequest {
        let (reply, receiver) = sync_channel(1);
        let command = Command::WorkProfileBinding { reply };
        match self.queue.try_push(command) {
            Ok(()) => {}
            Err(
                TryPushError::Full(command)
                | TryPushError::Sealed(command)
                | TryPushError::Closed(command),
            ) => {
                finish_unprocessed_command(command, ShutdownOutcome::Unclean);
            }
        }
        crate::AgentWorkProfileRequest(receiver)
    }

    /// The focused window's open tabs of `profile`, for consented context;
    /// an unprocessed request disconnects the receiver.
    pub fn window_tabs(
        &self,
        profile: ProfileId,
    ) -> std::sync::mpsc::Receiver<Vec<crate::TabMetadata>> {
        let (reply, receiver) = sync_channel(1);
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self.queue.try_push(Command::WindowTabs { profile, reply })
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    /// Tab titles and URLs for context admission; an unprocessed request
    /// disconnects the receiver instead of inventing an empty answer.
    pub fn tab_metadata(
        &self,
        profile: ProfileId,
        ids: Vec<ItemId>,
    ) -> std::sync::mpsc::Receiver<Vec<crate::TabMetadata>> {
        let (reply, receiver) = sync_channel(1);
        let command = Command::TabMetadata {
            profile,
            ids,
            reply,
        };
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self.queue.try_push(command)
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    /// Admits desktop-read bytes as a Media resource of the focused profile.
    pub fn import_media(
        &self,
        profile: ProfileId,
        import: zephium_core::resources::MediaImport,
    ) -> Receiver<zephium_core::resources::ResourceReply> {
        let (sender, receiver) = sync_channel(1);
        let done = crate::ResourceCompletion::new(move |reply| {
            let _ = sender.send(reply);
        });
        let command = Command::ImportMedia {
            expected_profile: profile,
            import: Box::new(import),
            done,
        };
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self.queue.try_push(command)
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    /// One profile-checked resource read or mutation, settled by the actor.
    pub fn resource_call(
        &self,
        profile: ProfileId,
        call: zephium_core::resources::ResourceCall,
    ) -> Receiver<zephium_core::resources::ResourceReply> {
        let (sender, receiver) = sync_channel(1);
        let done = crate::ResourceCompletion::new(move |reply| {
            let _ = sender.send(reply);
        });
        let command = Command::ResourceCall {
            expected_profile: profile,
            call: Arc::new(call),
            done,
        };
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self.queue.try_push(command)
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    /// One profile-checked notes call, settled by the notes service.
    pub fn note_call(
        &self,
        profile: ProfileId,
        call: zephium_core::notes::NoteCall,
    ) -> Receiver<zephium_core::notes::NoteReply> {
        let (sender, receiver) = sync_channel(1);
        let done = crate::NoteCompletion::new(move |reply| {
            let _ = sender.send(reply);
        });
        let command = Command::NoteCall {
            expected_profile: profile,
            call: Arc::new(call),
            done,
        };
        if let Err(
            TryPushError::Full(command)
            | TryPushError::Sealed(command)
            | TryPushError::Closed(command),
        ) = self.queue.try_push(command)
        {
            finish_unprocessed_command(command, ShutdownOutcome::Unclean);
        }
        receiver
    }

    /// Requests an ordered shutdown without waiting for queue capacity on the
    /// caller (normally the UI thread). Only `RetryableFailure` leaves the
    /// actor and native engine available for another attempt.
    pub fn shutdown(&self) -> ShutdownRequest {
        self.shutdown_with_deadline(self.shutdown_deadline())
    }

    /// Creates the single absolute deadline for composition-owned preflight
    /// and the actor's complete ordered shutdown.
    ///
    /// A composition root that owns a weak-callback producer must stop and
    /// join it before calling [`Self::shutdown_with_deadline`] with this exact
    /// value. This prevents late submissions from racing service retirement
    /// without restarting the process-wide shutdown budget.
    pub fn shutdown_deadline(&self) -> std::time::Instant {
        std::time::Instant::now() + END_TO_END_SHUTDOWN_TIMEOUT
    }

    /// Requests ordered shutdown under a caller-started absolute deadline.
    ///
    /// Deadlines beyond Zephium's fixed process budget are clamped; callers
    /// cannot extend teardown by supplying a later instant.
    pub fn shutdown_with_deadline(&self, deadline: std::time::Instant) -> ShutdownRequest {
        let (ack, done) = sync_channel(1);
        let deadline = deadline.min(self.shutdown_deadline());
        // The one process-boundary budget may start before actor admission.
        // Time spent stopping composition-owned callback producers or behind
        // accepted FIFO work must not be hidden by restarting the clock here.
        // A terminal request may overtake desktop admission. Wake the actor in
        // cancelled mode before publishing the barrier so it can drain that
        // exact request without entering an external startup port.
        self.startup.cancel();
        let command = Command::Shutdown { deadline, ack };
        match self.queue.try_push(command) {
            Ok(()) => {}
            // Normal admission reserves one slot for this barrier, so Full is
            // an invariant failure rather than a reason to block a native
            // close callback. A live prior barrier is likewise retryable by
            // the coordinator; a permanently closed actor is terminal.
            Err(TryPushError::Full(command) | TryPushError::Sealed(command)) => {
                finish_unprocessed_command(command, ShutdownOutcome::RetryableFailure);
            }
            Err(TryPushError::Closed(command)) => {
                finish_unprocessed_command(command, ShutdownOutcome::Unclean);
            }
        }
        ShutdownRequest {
            deadline,
            receiver: done,
            workers: self.workers.clone(),
        }
    }
}

fn finish_unprocessed_command(command: Command, outcome: ShutdownOutcome) {
    match command {
        Command::Shutdown { ack, .. } => {
            let _ = ack.send(outcome);
        }
        Command::ContentPolicyStatus { reply, .. } => {
            let _ = reply.send(ContentPolicyStatusQueryOutcome::Unavailable);
        }
        Command::ElementPicker { reply, .. } => {
            let _ = reply.try_send(None);
        }
        Command::BlockerStatistics { reply, .. } => {
            let _ = reply.try_send(None);
        }
        Command::FocusedContentPolicyStatus { reply } => {
            let _ = reply.send(BlockerStatusView::unavailable());
        }
        #[cfg(feature = "work-execution")]
        Command::WorkProfileBinding { reply } => {
            let _ = reply.send(crate::AgentWorkProfileReadiness::Unavailable);
        }
        Command::ResolveWebExtensionTarget { reply, .. } => {
            let _ = reply.try_send(None);
        }
        Command::WebExtensionStatus { reply, .. } => {
            let _ = reply.try_send(Vec::new());
        }
        _ => {}
    }
}

fn tracked_operation_command(command: &Command) -> bool {
    matches!(
        command,
        Command::Open
            | Command::ShowBrowserPage(_)
            | Command::WorkPaneShow { .. }
            | Command::WorkPaneHide
            | Command::SetTabEssential { .. }
            | Command::KeepSite(_)
            | Command::RenameFocusedProfile(_)
            | Command::Activate(_)
            | Command::Close(_)
            | Command::Navigate { .. }
            | Command::Reload(_)
            | Command::AnswerPageRequest { .. }
            | Command::GoBack(_)
            | Command::GoForward(_)
            | Command::SplitWith { .. }
            | Command::Unsplit
            | Command::LeaveSplit(_)
            | Command::TabAction { .. }
            | Command::DropTab { .. }
            | Command::DividerRelease { .. }
            | Command::Run(_)
            | Command::RunSearchAction { .. }
            | Command::InvokeExtensionAction { .. }
            | Command::RespondToPagePermissionPrompt { .. }
            | Command::StopMediaCapture { .. }
            | Command::OpenUrl { .. }
            | Command::SetAppSetting { .. }
            | Command::Focus(_)
            | Command::DeleteProfile(_)
            | Command::RetryContentPolicy { .. }
            | Command::SetFocusedContentBlockerEnabled(_)
            | Command::ChangeBlockerSite { .. }
            | Command::RetryFocusedContentPolicy { .. }
            | Command::RefreshContentBlockerSources
    )
}

struct ActorExitGuard(CommandQueue);

impl Drop for ActorExitGuard {
    fn drop(&mut self) {
        // Also runs during unwinding. Without this guard a panic in a port
        // implementation strands the ticker and lets callers enqueue into a
        // queue that will never again be drained. A barrier abandoned by an
        // exiting actor is terminal: no retry can revive this queue.
        for pending in self.0.close_and_drain() {
            finish_unprocessed_command(pending, ShutdownOutcome::Unclean);
        }
    }
}

struct ShellExitGuard(Shell);

impl std::ops::Deref for ShellExitGuard {
    type Target = Shell;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ShellExitGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ShellExitGuard {
    fn drop(&mut self) {
        if self.0.is_shutdown() {
            return;
        }
        // A panic or last-handle queue close can bypass Command::Shutdown.
        // Signal the composition root first so its independent hard deadline
        // is armed even if a damaged cleanup port never returns. The correlated
        // shutdown request remains queued until ActorExitGuard terminalizes it.
        self.0
            .report_terminal_failure(crate::ShellTerminalFailure::ActorExitedUnexpectedly);
        // Run every independent terminal barrier before ActorExitGuard drains
        // waiters, under one absolute deadline and without retrying a process
        // whose authoritative actor state has already been lost.
        let deadline = std::time::Instant::now() + END_TO_END_SHUTDOWN_TIMEOUT;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.0.cleanup_after_unexpected_exit_until(deadline)
        })) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                crate::diagnostic!(
                    "shutdown: complete cleanup was not proven during unexpected shell exit"
                )
            }
        }
    }
}

pub fn spawn(
    engine: SharedEngine,
    store: SharedStore,
    blocker: SharedBlocker,
    terminal_failure: ShellTerminalFailureCallback,
    chrome: SharedChrome,
    emit: EmitFn,
) -> Result<Handle, SpawnFailure> {
    let handle = spawn_suspended(engine, store, blocker, terminal_failure, chrome, emit)?;
    let admitted = handle.admit_startup();
    debug_assert!(admitted, "new Shell startup gate must be pending");
    let committed = handle.commit_startup_admission();
    debug_assert!(committed, "compatibility startup admission must commit");
    Ok(handle)
}

/// Starts and immediately admits a Shell that owns the complete agent browser lifecycle.
///
/// New composition roots should prefer [`spawn_agentic_suspended`] so the
/// handle can be published before any external startup port is entered.
#[cfg(feature = "agentic-browser")]
pub fn spawn_agentic(
    engine: SharedEngine,
    store: SharedStore,
    blocker: SharedBlocker,
    agent_lifecycle: AgentLifecycle,
    terminal_failure: ShellTerminalFailureCallback,
    chrome: SharedChrome,
    emit: EmitFn,
) -> Result<Handle, AgenticSpawnFailure> {
    let handle = spawn_agentic_suspended(
        engine,
        store,
        blocker,
        agent_lifecycle,
        terminal_failure,
        chrome,
        emit,
    )?;
    let admitted = handle.admit_startup();
    debug_assert!(admitted, "new agentic Shell startup gate must be pending");
    let committed = handle.commit_startup_admission();
    debug_assert!(committed, "agentic compatibility admission must commit");
    Ok(handle)
}

/// Starts helper workers and transfers the composition ports into a guarded
/// Shell while keeping every external actor port suspended.
///
/// The composition root must publish the returned Handle, clear every
/// temporary rollback owner, and then call [`Handle::admit_startup`]. Calling
/// [`Handle::shutdown`] first cancels admission and still drives the ordered
/// terminal barrier without entering startup ports.
pub fn spawn_suspended(
    engine: SharedEngine,
    store: SharedStore,
    blocker: SharedBlocker,
    terminal_failure: ShellTerminalFailureCallback,
    chrome: SharedChrome,
    emit: EmitFn,
) -> Result<Handle, SpawnFailure> {
    spawn_suspended_with_worker_spawner(
        ShellHandoff {
            engine,
            store,
            blocker,
            agent_lifecycle: NoAgentLifecycle,
            terminal_failure,
            chrome,
            emit,
        },
        spawn_worker,
    )
}

/// Starts a guarded Shell and transfers the move-only agent lifecycle while
/// keeping every external actor port suspended.
///
/// Every construction refusal returns the owner through
/// [`AgenticSpawnFailure`]. After success the Shell consumes the agent
/// lifecycle before terminal Store and engine teardown.
#[cfg(feature = "agentic-browser")]
pub fn spawn_agentic_suspended(
    engine: SharedEngine,
    store: SharedStore,
    blocker: SharedBlocker,
    agent_lifecycle: AgentLifecycle,
    terminal_failure: ShellTerminalFailureCallback,
    chrome: SharedChrome,
    emit: EmitFn,
) -> Result<Handle, AgenticSpawnFailure> {
    spawn_suspended_with_worker_spawner(
        ShellHandoff {
            engine,
            store,
            blocker,
            agent_lifecycle: PendingAgentBrowserLifecycle(agent_lifecycle),
            terminal_failure,
            chrome,
            emit,
        },
        spawn_worker,
    )
}

#[cfg(test)]
fn spawn_with_worker_spawner(
    ports: ShellHandoff<NoAgentLifecycle>,
    worker_spawner: impl FnMut(&'static str, WorkerTask) -> std::io::Result<WorkerThread>,
) -> Result<Handle, SpawnFailure> {
    let handle = spawn_suspended_with_worker_spawner(ports, worker_spawner)?;
    let admitted = handle.admit_startup();
    debug_assert!(admitted, "new Shell startup gate must be pending");
    let committed = handle.commit_startup_admission();
    debug_assert!(committed, "compatibility startup admission must commit");
    Ok(handle)
}

#[cfg(all(test, feature = "agentic-browser"))]
fn spawn_agentic_with_worker_spawner(
    ports: ShellHandoff<PendingAgentBrowserLifecycle>,
    worker_spawner: impl FnMut(&'static str, WorkerTask) -> std::io::Result<WorkerThread>,
) -> Result<Handle, AgenticSpawnFailure> {
    let handle = spawn_suspended_with_worker_spawner(ports, worker_spawner)?;
    let admitted = handle.admit_startup();
    debug_assert!(admitted, "new agentic Shell startup gate must be pending");
    let committed = handle.commit_startup_admission();
    debug_assert!(committed, "agentic compatibility admission must commit");
    Ok(handle)
}

#[cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]
fn spawn_suspended_with_worker_spawner<Agent: PendingAgentLifecycle>(
    ports: ShellHandoff<Agent>,
    mut worker_spawner: impl FnMut(&'static str, WorkerTask) -> std::io::Result<WorkerThread>,
) -> Result<Handle, Agent::Failure> {
    let ShellHandoff {
        engine,
        store,
        blocker,
        agent_lifecycle,
        terminal_failure,
        chrome,
        emit,
    } = ports;
    // A hostile page can generate title/load/navigation events much faster
    // than projections can be persisted. Backpressure bounds memory instead
    // of letting the actor queue grow without limit.
    let queue = CommandQueue::new();
    let workers = Arc::new(WorkerThreads::default());
    let startup = Arc::new(ActorStartupGate::new());
    let handle = Handle::with_workers(queue.clone(), workers.clone(), startup.clone());
    let store_reads = StoreReadQueue::new();
    let store_reader_task: WorkerTask = Box::new({
        let reader_store = store.clone();
        let reader_queue = store_reads.clone();
        let callback = handle.callback_handle();
        move || run_store_reader(reader_store, reader_queue, callback)
    });
    let store_reader = match worker_spawner("zephium-store-reader", store_reader_task) {
        Ok(worker) => worker,
        Err(error) => {
            let cleanup_proven = cleanup_failed_workers(&queue, &store_reads, &workers);
            return Err(
                agent_lifecycle.into_spawn_failure(SpawnError::StoreReader(error), cleanup_proven)
            );
        }
    };
    workers.install_store_reader(store_reader);
    let (shell_handoff, shell_receiver) = sync_channel::<ShellHandoff<Agent>>(1);
    let actor_queue = queue.clone();
    let actor_store_reads = store_reads.clone();
    let actor_startup = startup;
    let actor_task: WorkerTask = Box::new(move || {
        let _exit_guard = ActorExitGuard(actor_queue.clone());
        let shell_store_reads = actor_store_reads.clone();
        let _store_reader_guard = StoreReaderStopGuard::new(actor_store_reads);
        let Ok(handoff) = shell_receiver.recv() else {
            return;
        };
        let ShellHandoff {
            engine,
            store,
            blocker,
            agent_lifecycle,
            terminal_failure,
            chrome,
            emit,
        } = handoff;
        let ports = ShellPorts::new(engine, store, blocker, terminal_failure, chrome, emit);
        #[cfg(feature = "agentic-browser")]
        let ports = ports.with_agent_lifecycle(agent_lifecycle.into_shell_lifecycle());
        #[cfg(not(feature = "agentic-browser"))]
        drop(agent_lifecycle);
        let shell = Shell::with_store_reads_deferred_blocker_catalog(
            ports,
            shell_store_reads,
            #[cfg(test)]
            false,
        );
        let mut shell = ShellExitGuard(shell);
        // No external composition port is entered until the complete Shell is
        // guarded and desktop has published its authoritative Handle and
        // cleared every temporary rollback owner.
        if !actor_startup.wait_for_admission() {
            shell.attach_queue_for_terminal_cleanup(actor_queue.clone());
            while let Some(command) = actor_queue.recv() {
                if matches!(command, Command::Shutdown { .. }) {
                    shell.handle(command);
                    break;
                }
                finish_unprocessed_command(command, ShutdownOutcome::Unclean);
            }
            return;
        }
        // A panic in the initial blocker snapshot now runs only after the
        // composition root can route the early terminal signal to this exact
        // managed Shell.
        shell.initialize_blocker_catalog();
        shell.attach_queue(actor_queue.clone());
        while let Some(command) = actor_queue.recv() {
            shell.handle(command);
            if shell.is_shutdown() || shell.terminal_failure_handoff_panicked() {
                break;
            }
        }
    });
    let actor = match worker_spawner("zephium-shell", actor_task) {
        Ok(actor) => actor,
        Err(error) => {
            drop(shell_handoff);
            let cleanup_proven = cleanup_failed_workers(&queue, &store_reads, &workers);
            return Err(
                agent_lifecycle.into_spawn_failure(SpawnError::Actor(error), cleanup_proven)
            );
        }
    };
    workers.install_actor(actor);
    let timer_queue = queue.clone();
    let timer_task: WorkerTask = Box::new(move || {
        let mut maintenance_deadline = std::time::Instant::now() + MAINTENANCE_INTERVAL;
        loop {
            match timer_queue.wait_for_timer(maintenance_deadline) {
                #[cfg(feature = "work-execution")]
                TimerWake::Work => match timer_queue.try_push(Command::WorkWake) {
                    Ok(()) | Err(TryPushError::Sealed(_)) => {}
                    Err(TryPushError::Full(_)) => timer_queue.schedule_work(Some(
                        std::time::Instant::now() + std::time::Duration::from_millis(25),
                    )),
                    Err(TryPushError::Closed(_)) => break,
                },
                TimerWake::Maintenance => {
                    maintenance_deadline = std::time::Instant::now() + MAINTENANCE_INTERVAL;
                    match timer_queue.try_push(Command::Tick) {
                        Ok(()) | Err(TryPushError::Full(_)) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::Focus => match timer_queue.try_push(Command::FocusWake) {
                    Ok(()) | Err(TryPushError::Sealed(_)) => {}
                    Err(TryPushError::Full(_)) => timer_queue.schedule_focus(Some(
                        std::time::Instant::now() + std::time::Duration::from_millis(25),
                    )),
                    Err(TryPushError::Closed(_)) => break,
                },
                TimerWake::Persist => match timer_queue.try_push(Command::Persist) {
                    Ok(()) | Err(TryPushError::Sealed(_)) => {}
                    Err(TryPushError::Full(_)) => timer_queue.schedule_persist(
                        std::time::Instant::now() + std::time::Duration::from_millis(25),
                    ),
                    Err(TryPushError::Closed(_)) => break,
                },
                TimerWake::Favicon { id, attempt } => {
                    match timer_queue.try_push(Command::FaviconPoll { id, attempt }) {
                        Ok(()) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Full(_)) => timer_queue.schedule_favicon(
                            id,
                            attempt,
                            std::time::Instant::now() + std::time::Duration::from_millis(25),
                        ),
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::Presentation {
                    id,
                    navigation,
                    hard_deadline,
                } => {
                    match timer_queue.try_push(Command::PresentationFallback {
                        id,
                        navigation,
                        hard_deadline,
                    }) {
                        Ok(()) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Full(_)) => timer_queue.retry_presentation(
                            id,
                            navigation,
                            std::time::Instant::now() + std::time::Duration::from_millis(25),
                            hard_deadline,
                        ),
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::DiscardProbe { id, probe } => {
                    match timer_queue.try_push(Command::DiscardProbeTimeout { id, probe }) {
                        Ok(()) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Full(_)) => timer_queue.schedule_discard_probe(
                            id,
                            probe,
                            std::time::Instant::now() + std::time::Duration::from_millis(25),
                        ),
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::ViewCapacity { id } => {
                    match timer_queue.try_push(Command::ViewCapacityRetry(id)) {
                        Ok(()) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Full(_)) => timer_queue.schedule_view_capacity(
                            id,
                            std::time::Instant::now() + std::time::Duration::from_millis(25),
                        ),
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::ProfileDeletion {
                    profile,
                    generation,
                } => match timer_queue.try_push(Command::ProfileDeletionRetry {
                    profile,
                    generation,
                }) {
                    Ok(()) | Err(TryPushError::Sealed(_)) => {}
                    Err(TryPushError::Full(_)) => timer_queue.schedule_profile_deletion(
                        profile,
                        generation,
                        std::time::Instant::now() + std::time::Duration::from_millis(25),
                    ),
                    Err(TryPushError::Closed(_)) => break,
                },
                TimerWake::BlockerPreference { profile, token } => {
                    match timer_queue.try_push(Command::BlockerPreferenceRetry { profile, token }) {
                        Ok(()) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Full(_)) => timer_queue
                            .schedule_blocker_preference_reconciliation(
                                profile,
                                token,
                                std::time::Instant::now() + std::time::Duration::from_millis(25),
                            ),
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::BlockerCatalog { operation, attempt } => {
                    match timer_queue.try_push(Command::BlockerCatalogPoll { operation, attempt }) {
                        Ok(()) | Err(TryPushError::Sealed(_)) => {}
                        Err(TryPushError::Full(_)) => timer_queue.schedule_blocker_catalog_poll(
                            operation,
                            attempt,
                            std::time::Instant::now() + std::time::Duration::from_millis(25),
                        ),
                        Err(TryPushError::Closed(_)) => break,
                    }
                }
                TimerWake::PagePermission {
                    profile,
                    item,
                    request,
                } => match timer_queue.try_push(Command::PagePermissionTimeout {
                    profile,
                    item,
                    request,
                }) {
                    Ok(()) | Err(TryPushError::Sealed(_)) => {}
                    Err(TryPushError::Full(_)) => timer_queue.schedule_page_permission(
                        profile,
                        item,
                        request,
                        std::time::Instant::now() + std::time::Duration::from_millis(25),
                    ),
                    Err(TryPushError::Closed(_)) => break,
                },
                TimerWake::Stopped => break,
            }
        }
    });
    let timer = match worker_spawner("zephium-timer", timer_task) {
        Ok(timer) => timer,
        Err(error) => {
            // Wake the actor waiter before joining it. Any agent lifecycle
            // owner is still local and has not crossed the handoff boundary.
            drop(shell_handoff);
            let cleanup_proven = cleanup_failed_workers(&queue, &store_reads, &workers);
            return Err(
                agent_lifecycle.into_spawn_failure(SpawnError::Timer(error), cleanup_proven)
            );
        }
    };
    workers.install_timer(timer);

    // Every fallible worker construction completed before the unique owner
    // crosses into the actor. Shell itself is constructed only after receipt,
    // so a disconnected handoff returns the owner as a first-class field
    // without calling any lifecycle method on this setup thread.
    let handoff = ShellHandoff {
        engine,
        store,
        blocker,
        agent_lifecycle,
        terminal_failure,
        chrome,
        emit,
    };
    match shell_handoff.try_send(handoff) {
        Ok(()) => {}
        Err(TrySendError::Full(handoff) | TrySendError::Disconnected(handoff)) => {
            let ShellHandoff {
                engine,
                store,
                blocker,
                agent_lifecycle,
                terminal_failure,
                chrome,
                emit,
            } = handoff;
            let cleanup_proven = cleanup_failed_workers(&queue, &store_reads, &workers);
            // These cloneable composition ports are not returned, but keep a
            // faulty destructor from preventing recovery of the unique owner.
            for clean in [
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(engine))).is_ok(),
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(store))).is_ok(),
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(blocker))).is_ok(),
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(terminal_failure)))
                    .is_ok(),
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(chrome))).is_ok(),
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(emit))).is_ok(),
            ] {
                if !clean {
                    crate::diagnostic!(
                        "startup: a composition port panicked while its handoff was rolled back"
                    );
                }
            }
            return Err(agent_lifecycle.into_spawn_failure(
                SpawnError::ActorHandoff(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "shell actor exited before accepting its unique owner",
                )),
                cleanup_proven,
            ));
        }
    }
    Ok(handle)
}
