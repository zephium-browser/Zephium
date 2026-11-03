//! Single bounded worker for official HTTPS sources; idle workers block on IPC.
use crate::official_store::{OfficialStore, Package, Source};
use crate::types::*;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use zephium_update_transport::official_filters::{
    FilterResponse, OfficialFilter, OfficialFilterClient,
};

const DAY: u64 = 86400;
const BACKOFF: [u64; 4] = [900, 3600, 21600, 86400];

pub(crate) struct OfficialWorker {
    sender: Option<mpsc::SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    cancel: watch::Sender<bool>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Told after each published status, outside the state lock.
    waker: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
struct State {
    snapshot: StatusSnapshot,
    initial: Option<ActivatedCatalog>,
    candidate: Option<ActivatedCatalog>,
    candidate_identity: Option<CatalogIdentity>,
    due: u64,
    verification: u64,
    freshness_deadline: u64,
    busy: bool,
    sealed: bool,
    operation: u64,
    repairs: u8,
}
enum Command {
    Refresh(u64),
    Commit(
        CatalogIdentity,
        Box<dyn FnOnce(CandidateCommitOutcome) + Send>,
    ),
    Reject(
        CatalogIdentity,
        FailureKind,
        Box<dyn FnOnce(CandidateRejectOutcome) + Send>,
    ),
    Repair(
        CatalogIdentity,
        Box<dyn FnOnce(CandidateRepairOutcome) + Send>,
    ),
}
impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn publish(&self, record: &crate::official_store::Record, status: UpdateStatus) {
        let mut state = self.lock();
        let Some(revision) = state.snapshot.revision.checked_add(1) else {
            state.sealed = true;
            state.snapshot.status = UpdateStatus::Shutdown;
            self.changed.notify_all();
            return;
        };
        state.snapshot = StatusSnapshot {
            revision,
            last_refresh_attempt_unix: record.attempted,
            status,
        };
        state.due = record.due;
        state.verification = record.verification;
        state.freshness_deadline = freshness_deadline(record, &state.snapshot.status);
        state.candidate_identity = record.candidate.as_ref().map(|p| p.identity.clone());
        state.busy = false;
        self.changed.notify_all();
        drop(state);
        let waker = self
            .waker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(wake) = waker {
            wake();
        }
    }
}

impl OfficialWorker {
    pub(crate) fn start(root: PathBuf, seed: ActivatedCatalog) -> Result<Self, std::io::Error> {
        let initial_now = now();
        let mut store = OfficialStore::open(root, seed.identity.revision, initial_now)
            .map_err(std::io::Error::other)?;
        let mut next = store.record.clone();
        let newer_bundle = seed.identity.revision > next.highest_revision;
        next.highest_revision = next.highest_revision.max(seed.identity.revision);
        if next
            .current
            .as_ref()
            .is_some_and(|p| p.identity.created_unix < seed.identity.created_unix)
        {
            next.previous = next.current.take();
            next.checked = None;
            next.due = seed.identity.created_unix.saturating_add(DAY + next.jitter);
        }
        if next
            .current
            .as_ref()
            .is_some_and(|package| !store.intact(package))
        {
            next.current = next.previous.take().filter(|package| store.intact(package));
            next.checked = None;
            next.due = initial_now;
        }
        if next
            .candidate
            .as_ref()
            .is_some_and(|package| !store.intact(package))
        {
            next.candidate = None;
            next.due = initial_now;
        }
        let installed = next
            .current
            .as_ref()
            .filter(|p| p.identity.created_unix >= seed.identity.created_unix)
            .map(|p| &p.identity)
            .unwrap_or(&seed.identity);
        if next
            .candidate
            .as_ref()
            .is_some_and(|p| p.identity.revision <= installed.revision)
        {
            next.candidate = None;
        }

        if next.current.is_none() && (next.attempted.is_none() || newer_bundle) {
            next.due = seed.identity.created_unix.saturating_add(DAY + next.jitter);
        }
        store.persist(next).map_err(std::io::Error::other)?;
        let initial = chosen_current(&store, &seed);
        let candidate = store.record.candidate.as_ref().map(|p| store.catalog(p));
        let freshness_deadline = store
            .record
            .checked
            .unwrap_or(initial.identity.created_unix)
            .saturating_add(DAY + store.record.jitter);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                snapshot: StatusSnapshot {
                    revision: 1,
                    last_refresh_attempt_unix: store.record.attempted,
                    status: UpdateStatus::Ready(availability(&store, &seed)),
                },
                initial: Some(initial),
                candidate,
                candidate_identity: store.record.candidate.as_ref().map(|p| p.identity.clone()),
                due: store.record.due,
                verification: store.record.verification,
                freshness_deadline,
                busy: false,
                sealed: false,
                operation: 0,
                repairs: 0,
            }),
            changed: Condvar::new(),
            waker: Mutex::new(None),
        });
        let (sender, receiver) = mpsc::sync_channel(2);
        let (cancel, mut cancelled) = watch::channel(false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = OfficialFilterClient::new().map_err(std::io::Error::other)?;
        let worker_shared = shared.clone();
        let thread = thread::Builder::new()
            .name("official-filter-updates".into())
            .spawn(move || {
                while let Ok(command) = receiver.recv() {
                    if *cancelled.borrow() {
                        abandon(command);
                        break;
                    }
                    match command {
                        Command::Refresh(operation) => {
                            let mut next = store.record.clone();
                            next.attempted = Some(now());
                            next.due = now()
                                .saturating_add(BACKOFF[usize::from(next.failure_streak.min(3))]);
                            let result = store
                                .persist(next)
                                .map_err(|_| (FailureKind::Storage, None))
                                .and_then(|_| {
                                    runtime.block_on(fetch_official_sources(
                                        &store,
                                        &client,
                                        &mut cancelled,
                                        false,
                                    ))
                                });
                            match result {
                                Ok(sources) => finish_refresh(
                                    &mut store,
                                    &seed,
                                    &worker_shared,
                                    operation,
                                    sources,
                                ),
                                Err((failure, retry)) => failed_refresh(
                                    &mut store,
                                    &seed,
                                    &worker_shared,
                                    operation,
                                    failure,
                                    retry,
                                ),
                            }
                        }
                        Command::Commit(identity, done) => {
                            if store
                                .record
                                .candidate
                                .as_ref()
                                .is_some_and(|p| p.identity == identity)
                                && identity.expires_unix <= now()
                            {
                                let mut next = store.record.clone();
                                next.rejected = next.candidate.take();
                                next.due = now();
                                let failure = if store.persist(next).is_ok() {
                                    FailureKind::Manifest
                                } else {
                                    FailureKind::Storage
                                };
                                let operation = worker_shared.lock().operation;
                                worker_shared.publish(
                                    &store.record,
                                    UpdateStatus::Failed {
                                        operation,
                                        failure,
                                        current: Some(availability(&store, &seed)),
                                    },
                                );
                                worker_shared.lock().candidate = None;
                                done(CandidateCommitOutcome::Failed(failure));
                                continue;
                            }
                            let outcome = if store
                                .record
                                .candidate
                                .as_ref()
                                .is_some_and(|p| p.identity == identity && store.intact(p))
                            {
                                let mut next = store.record.clone();
                                next.previous = next.current.take();
                                next.current = next.candidate.take();
                                next.checked = Some(now());
                                next.verification = next.verification.saturating_add(1);
                                next.failure_streak = 0;
                                next.due = now().saturating_add(DAY + next.jitter);
                                next.rejected = None;
                                match store.persist(next) {
                                    Ok(()) => {
                                        let _ = store.collect();
                                        worker_shared.publish(
                                            &store.record,
                                            UpdateStatus::Ready(availability(&store, &seed)),
                                        );
                                        CandidateCommitOutcome::Committed
                                    }
                                    Err(_) => {
                                        let operation = worker_shared.lock().operation;
                                        worker_shared.publish(
                                            &store.record,
                                            UpdateStatus::Failed {
                                                operation,
                                                failure: FailureKind::Storage,
                                                current: Some(availability(&store, &seed)),
                                            },
                                        );
                                        CandidateCommitOutcome::Failed(FailureKind::Storage)
                                    }
                                }
                            } else {
                                CandidateCommitOutcome::Failed(FailureKind::Rollback)
                            };
                            {
                                let mut state = worker_shared.lock();
                                state.busy = false;
                                if store.record.candidate.is_none() {
                                    state.candidate = None;
                                }
                            }
                            done(outcome);
                        }
                        Command::Reject(identity, failure, done) => {
                            let outcome = if store
                                .record
                                .candidate
                                .as_ref()
                                .is_some_and(|p| p.identity == identity)
                            {
                                let mut next = store.record.clone();
                                next.rejected = next.candidate.take();
                                next.failure_streak = next.failure_streak.saturating_add(1).min(8);
                                next.due = now().saturating_add(
                                    BACKOFF
                                        [usize::from(next.failure_streak.saturating_sub(1).min(3))],
                                );
                                match store.persist(next) {
                                    Ok(()) => {
                                        let _ = store.collect();
                                        let operation = worker_shared.lock().operation;
                                        worker_shared.publish(
                                            &store.record,
                                            UpdateStatus::Failed {
                                                operation,
                                                failure,
                                                current: Some(availability(&store, &seed)),
                                            },
                                        );
                                        CandidateRejectOutcome::Rejected
                                    }
                                    Err(_) => CandidateRejectOutcome::Failed(FailureKind::Storage),
                                }
                            } else {
                                CandidateRejectOutcome::Failed(FailureKind::Rollback)
                            };
                            {
                                let mut state = worker_shared.lock();
                                state.busy = false;
                                if store.record.candidate.is_none() {
                                    state.candidate = None;
                                }
                            }
                            done(outcome);
                        }
                        Command::Repair(identity, done) => {
                            let outcome = if let Some(original) = store
                                .record
                                .candidate
                                .clone()
                                .filter(|p| p.identity == identity)
                            {
                                let intact = original
                                    .sources
                                    .iter()
                                    .all(|source| store.cached_bytes(source).is_ok());
                                if intact {
                                    CandidateRepairOutcome::Repaired
                                } else {
                                    match runtime.block_on(fetch_official_sources(
                                        &store,
                                        &client,
                                        &mut cancelled,
                                        true,
                                    )) {
                                        Ok(sources) if same_sources(&original, &sources) => {
                                            CandidateRepairOutcome::Repaired
                                        }
                                        Ok(sources) => match stage(&mut store, sources) {
                                            Ok(package) => CandidateRepairOutcome::Superseded(
                                                store.catalog(&package),
                                            ),
                                            Err(_) => {
                                                CandidateRepairOutcome::Failed(FailureKind::Storage)
                                            }
                                        },
                                        Err((failure, _)) => {
                                            CandidateRepairOutcome::Failed(failure)
                                        }
                                    }
                                }
                            } else {
                                CandidateRepairOutcome::Failed(FailureKind::Rollback)
                            };
                            let operation = worker_shared.lock().operation;
                            worker_shared.publish(
                                &store.record,
                                UpdateStatus::Refreshing {
                                    operation,
                                    current: Some(availability(&store, &seed)),
                                },
                            );
                            done(outcome);
                        }
                    }
                }
                for command in receiver.try_iter() {
                    abandon(command);
                }
                let mut state = worker_shared.lock();
                state.sealed = true;
                state.busy = false;
                state.snapshot.status = UpdateStatus::Shutdown;
                state.snapshot.revision = state.snapshot.revision.saturating_add(1);
                worker_shared.changed.notify_all();
            })?;
        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            shared,
            cancel,
        })
    }

    pub(crate) fn status(&self) -> StatusSnapshot {
        self.check_thread();
        self.shared.lock().snapshot.clone()
    }
    fn check_thread(&self) {
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            let mut state = self.shared.lock();
            if !state.sealed {
                state.sealed = true;
                state.snapshot.revision = state.snapshot.revision.saturating_add(1);
                state.snapshot.status = UpdateStatus::Shutdown;
                self.shared.changed.notify_all();
            }
        }
    }
    pub(crate) fn freshness(&self) -> (u64, bool) {
        let state = self.shared.lock();
        (state.verification, state.freshness_deadline <= now())
    }
    pub(crate) fn next_refresh_unix(&self) -> u64 {
        self.shared.lock().due
    }
    pub(crate) fn observe_and_take_catalogs(
        &self,
    ) -> (
        StatusSnapshot,
        Option<ActivatedCatalog>,
        Option<ActivatedCatalog>,
    ) {
        self.check_thread();
        let mut state = self.shared.lock();
        (
            state.snapshot.clone(),
            state.initial.take(),
            state.candidate.take(),
        )
    }
    pub(crate) fn request_refresh(&self) -> RefreshAdmission {
        let mut state = self.shared.lock();
        if state.sealed {
            return RefreshAdmission::Shutdown;
        }
        if state.busy
            || state.candidate_identity.is_some()
            || state
                .snapshot
                .last_refresh_attempt_unix
                .is_some_and(|last| now() < last.saturating_add(60))
        {
            return RefreshAdmission::Busy;
        }
        let Some(operation) = state.operation.checked_add(1) else {
            state.sealed = true;
            return RefreshAdmission::Shutdown;
        };
        if self
            .sender
            .as_ref()
            .is_none_or(|s| s.try_send(Command::Refresh(operation)).is_err())
        {
            return RefreshAdmission::Busy;
        }
        state.operation = operation;
        state.busy = true;
        let current = availability_from_status(&state.snapshot.status);
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.snapshot.status = UpdateStatus::Refreshing { operation, current };
        self.shared.changed.notify_all();
        RefreshAdmission::Accepted(operation)
    }
    pub(crate) fn commit_candidate(
        &self,
        identity: CatalogIdentity,
        done: Box<dyn FnOnce(CandidateCommitOutcome) + Send>,
    ) -> CandidateCommitDispatch {
        match self.send_candidate(&identity, Command::Commit(identity.clone(), done)) {
            Ok(()) => CandidateCommitDispatch::Scheduled,
            Err(true) => CandidateCommitDispatch::Terminal,
            Err(false) => CandidateCommitDispatch::Rejected,
        }
    }
    pub(crate) fn reject_candidate(
        &self,
        identity: CatalogIdentity,
        failure: FailureKind,
        _reason: CandidateRejectionReason,
        done: Box<dyn FnOnce(CandidateRejectOutcome) + Send>,
    ) -> CandidateRejectDispatch {
        match self.send_candidate(&identity, Command::Reject(identity.clone(), failure, done)) {
            Ok(()) => CandidateRejectDispatch::Scheduled,
            Err(true) => CandidateRejectDispatch::Terminal,
            Err(false) => CandidateRejectDispatch::Rejected,
        }
    }
    pub(crate) fn repair_candidate(
        &self,
        identity: CatalogIdentity,
        done: Box<dyn FnOnce(CandidateRepairOutcome) + Send>,
        explicit: bool,
    ) -> CandidateRepairDispatch {
        {
            let state = self.shared.lock();
            if state.repairs >= 3 || (!explicit && state.repairs > 0) {
                return CandidateRepairDispatch::LimitReached;
            }
        }
        match self.send_candidate(&identity, Command::Repair(identity.clone(), done)) {
            Ok(()) => {
                let mut state = self.shared.lock();
                state.repairs += 1;
                CandidateRepairDispatch::Scheduled {
                    operation: state.operation,
                }
            }
            Err(true) => CandidateRepairDispatch::Terminal,
            Err(false) => CandidateRepairDispatch::Rejected,
        }
    }
    fn send_candidate(&self, identity: &CatalogIdentity, command: Command) -> Result<(), bool> {
        let mut state = self.shared.lock();
        if state.sealed {
            return Err(true);
        }
        if state.busy || state.candidate_identity.as_ref() != Some(identity) {
            return Err(false);
        }
        if self
            .sender
            .as_ref()
            .is_none_or(|sender| sender.try_send(command).is_err())
        {
            return Err(false);
        }
        state.busy = true;
        Ok(())
    }
    pub(crate) fn set_change_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self
            .shared
            .waker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(wake);
    }
    pub(crate) fn wait_for_change(&self, revision: u64, timeout: Duration) -> WaitForStatus {
        let state = self.shared.lock();
        let (state, _) = self
            .shared
            .changed
            .wait_timeout_while(state, timeout, |s| s.snapshot.revision <= revision)
            .unwrap_or_else(|p| p.into_inner());
        if state.snapshot.revision > revision {
            WaitForStatus::Changed(state.snapshot.clone())
        } else {
            WaitForStatus::TimedOut
        }
    }
    pub(crate) fn shutdown(mut self, timeout: Duration) -> ShutdownOutcome {
        self.shared.lock().sealed = true;
        let _ = self.cancel.send(true);
        self.sender.take();
        let deadline = Instant::now() + timeout;
        let Some(handle) = self.thread.take() else {
            return ShutdownOutcome::Unavailable;
        };
        while !handle.is_finished() && Instant::now() < deadline {
            thread::park_timeout(Duration::from_millis(1));
        }
        if handle.is_finished() && handle.join().is_ok() {
            ShutdownOutcome::Complete
        } else {
            ShutdownOutcome::TimedOut
        }
    }
}
impl Drop for OfficialWorker {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        self.sender.take();
    }
}

fn abandon(command: Command) {
    match command {
        Command::Refresh(_) => {}
        Command::Commit(_, done) => done(CandidateCommitOutcome::Failed(FailureKind::Internal)),
        Command::Reject(_, _, done) => done(CandidateRejectOutcome::Failed(FailureKind::Internal)),
        Command::Repair(_, done) => done(CandidateRepairOutcome::Failed(FailureKind::Internal)),
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .unwrap_or(0)
}
fn chosen_current(store: &OfficialStore, seed: &ActivatedCatalog) -> ActivatedCatalog {
    store
        .record
        .current
        .as_ref()
        .filter(|p| p.identity.created_unix >= seed.identity.created_unix)
        .map(|p| store.catalog(p))
        .unwrap_or_else(|| seed.clone())
}
fn availability(store: &OfficialStore, seed: &ActivatedCatalog) -> CatalogAvailability {
    let identity = chosen_current(store, seed).identity;
    if store.record.checked.is_none_or(|checked| checked <= now())
        && store
            .record
            .checked
            .unwrap_or(identity.created_unix)
            .saturating_add(7 * DAY)
            > now()
    {
        CatalogAvailability::Fresh(identity)
    } else {
        CatalogAvailability::Stale(identity)
    }
}
fn availability_from_status(status: &UpdateStatus) -> Option<CatalogAvailability> {
    match status {
        UpdateStatus::Ready(value) => Some(value.clone()),
        UpdateStatus::Refreshing { current, .. } | UpdateStatus::Failed { current, .. } => {
            current.clone()
        }
        _ => None,
    }
}
fn same_sources(package: &Package, sources: &[Source; 2]) -> bool {
    package
        .sources
        .iter()
        .zip(sources)
        .all(|(a, b)| a.sha256 == b.sha256 && a.bytes == b.bytes)
}

async fn fetch_official_sources(
    store: &OfficialStore,
    client: &OfficialFilterClient,
    cancelled: &mut watch::Receiver<bool>,
    force: bool,
) -> Result<[Source; 2], (FailureKind, Option<u64>)> {
    let mut fetch =
        |source,
         validators: Option<zephium_update_transport::official_filters::FilterValidators>| {
            let client = client.clone();
            async move { client.fetch(source, validators.as_ref()).await }
        };
    fetch_sources(store, &mut fetch, cancelled, force).await
}

async fn fetch_sources<F, Fut>(
    store: &OfficialStore,
    fetch: &mut F,
    cancelled: &mut watch::Receiver<bool>,
    force: bool,
) -> Result<[Source; 2], (FailureKind, Option<u64>)>
where
    F: FnMut(
        OfficialFilter,
        Option<zephium_update_transport::official_filters::FilterValidators>,
    ) -> Fut,
    Fut: std::future::Future<
        Output = Result<
            FilterResponse,
            zephium_update_transport::official_filters::FilterFetchFailure,
        >,
    >,
{
    store.collect().map_err(|_| (FailureKind::Storage, None))?;
    let prior = store
        .record
        .candidate
        .as_ref()
        .or(store.record.rejected.as_ref())
        .or(store.record.current.as_ref());
    let mut output = Vec::with_capacity(2);
    for (index, source) in [OfficialFilter::EasyList, OfficialFilter::EasyPrivacy]
        .into_iter()
        .enumerate()
    {
        let old = prior.map(|p| &p.sources[index]);
        let cached = if force {
            None
        } else {
            old.and_then(|source| store.cached_bytes(source).ok())
        };
        let validators = old.filter(|_| cached.is_some()).map(Source::validators);
        let mut attempt = 0;
        loop {
            if *cancelled.borrow() {
                return Err((FailureKind::Internal, None));
            }
            let response = tokio::select! {
                result=fetch(source,if attempt==0 {validators.clone()}else{None})=>result.map_err(|e|(FailureKind::Transport,e.retry_after_seconds))?,
                _=cancelled.changed()=>return Err((FailureKind::Internal,None)),
            };
            match response {
                FilterResponse::NotModified
                    if cached.is_some()
                        && attempt == 0
                        && validators
                            .as_ref()
                            .is_some_and(|v| v.etag.is_some() || v.last_modified.is_some()) =>
                {
                    output.push(old.expect("cached source descriptor").clone());
                    break;
                }
                FilterResponse::NotModified if attempt == 0 => {
                    attempt = 1;
                }
                FilterResponse::NotModified => return Err((FailureKind::Target, None)),
                FilterResponse::Modified { bytes, validators } => {
                    output.push(
                        store
                            .put_source(index, &bytes, validators, old)
                            .map_err(|e| {
                                (
                                    if e == crate::storage_io::StoreError::InvalidPackage {
                                        FailureKind::Target
                                    } else {
                                        e.kind()
                                    },
                                    None,
                                )
                            })?,
                    );
                    break;
                }
            }
        }
    }
    output.try_into().map_err(|_| (FailureKind::Internal, None))
}
fn stage(
    store: &mut OfficialStore,
    sources: [Source; 2],
) -> Result<Package, crate::storage_io::StoreError> {
    let package = store.new_package(sources, now())?;
    let mut next = store.record.clone();
    next.highest_revision = package.identity.revision;
    next.candidate = Some(package.clone());
    next.due = now().saturating_add(DAY + next.jitter);
    store.persist(next)?;
    Ok(package)
}
fn finish_refresh(
    store: &mut OfficialStore,
    seed: &ActivatedCatalog,
    shared: &Shared,
    operation: u64,
    sources: [Source; 2],
) {
    if store
        .record
        .current
        .as_ref()
        .is_some_and(|p| same_sources(p, &sources))
    {
        let mut next = store.record.clone();
        next.checked = Some(now());
        next.verification = next.verification.saturating_add(1);
        next.failure_streak = 0;
        next.due = now().saturating_add(DAY + next.jitter);
        if let Some(current) = next.current.as_mut() {
            current.sources = sources;
        }
        next.rejected = None;
        if store.persist(next).is_err() {
            failed_refresh(store, seed, shared, operation, FailureKind::Storage, None);
            return;
        }
        shared.publish(
            &store.record,
            UpdateStatus::Ready(availability(store, seed)),
        );
        return;
    }
    match stage(store, sources) {
        Ok(package) => {
            let candidate = store.catalog(&package);
            {
                let mut state = shared.lock();
                state.candidate = Some(candidate);
                state.repairs = 0;
            }
            shared.publish(
                &store.record,
                UpdateStatus::Refreshing {
                    operation,
                    current: Some(availability(store, seed)),
                },
            );
        }
        Err(_) => failed_refresh(store, seed, shared, operation, FailureKind::Storage, None),
    }
}
fn failed_refresh(
    store: &mut OfficialStore,
    seed: &ActivatedCatalog,
    shared: &Shared,
    operation: u64,
    failure: FailureKind,
    retry: Option<u64>,
) {
    let mut next = store.record.clone();
    next.failure_streak = next.failure_streak.saturating_add(1).min(8);
    next.due = now().saturating_add(
        BACKOFF[usize::from(next.failure_streak.saturating_sub(1).min(3))].max(retry.unwrap_or(0)),
    );
    let failure = if store.persist(next).is_err() {
        FailureKind::Storage
    } else {
        failure
    };
    shared.publish(
        &store.record,
        UpdateStatus::Failed {
            operation,
            failure,
            current: Some(availability(store, seed)),
        },
    );
}

fn freshness_deadline(record: &crate::official_store::Record, status: &UpdateStatus) -> u64 {
    let created = availability_from_status(status)
        .map(|a| match a {
            CatalogAvailability::Fresh(i) | CatalogAvailability::Stale(i) => i.created_unix,
        })
        .unwrap_or(0);
    record
        .checked
        .unwrap_or(created)
        .saturating_add(DAY + record.jitter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use zephium_update_transport::official_filters::{FilterFetchFailure, FilterValidators};
    fn source(index: usize) -> Vec<u8> {
        let title = if index == 0 {
            "EasyList"
        } else {
            "EasyPrivacy"
        };
        let header = if index == 0 { "2.0" } else { "1.1" };
        let mut text=format!("[Adblock Plus {header}]\n! Version: 202609300001\n! Title: {title}\n! Homepage: https://easylist.to/\n");
        for n in 0..12000 {
            text.push_str(&format!("||ads{n}.example.invalid^\n"));
        }
        text.into_bytes()
    }
    fn seed(created: u64) -> ActivatedCatalog {
        ActivatedCatalog {
            identity: CatalogIdentity {
                revision: 100,
                manifest_sha256: [1; 32],
                created_unix: created,
                expires_unix: created + 7 * DAY,
                source_count: 2,
                source_bytes: 600000,
            },
            catalog: zephium_blocker::PolicyCatalog::eager(
                zephium_blocker::StaticPolicyCatalog::empty(),
            ),
        }
    }
    fn fixture() -> (tempfile::TempDir, OfficialStore, Package) {
        let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let mut store = OfficialStore::open(dir.path().join("official"), 100, now()).unwrap();
        let validators = FilterValidators {
            etag: Some("\"stable\"".into()),
            last_modified: None,
        };
        let sources = [
            store
                .put_source(0, &source(0), validators.clone(), None)
                .unwrap(),
            store.put_source(1, &source(1), validators, None).unwrap(),
        ];
        let package = store.new_package(sources, now()).unwrap();
        let mut record = store.record.clone();
        record.highest_revision = package.identity.revision;
        record.current = Some(package.clone());
        record.checked = Some(now());
        record.due = now() + DAY;
        store.persist(record).unwrap();
        (dir, store, package)
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn each_published_status_wakes_the_registered_caller_outside_the_lock() {
        let (_dir, store, _package) = fixture();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                snapshot: StatusSnapshot {
                    revision: 1,
                    last_refresh_attempt_unix: None,
                    status: UpdateStatus::Shutdown,
                },
                initial: None,
                candidate: None,
                candidate_identity: None,
                due: 0,
                verification: 0,
                freshness_deadline: 0,
                busy: false,
                sealed: false,
                operation: 0,
                repairs: 0,
            }),
            changed: Condvar::new(),
            waker: Mutex::new(None),
        });
        // Nothing is registered yet: publishing must not need a waker.
        shared.publish(&store.record, UpdateStatus::Shutdown);
        let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (seen, observed) = (wakes.clone(), Arc::downgrade(&shared));
        *shared.waker.lock().unwrap() = Some(Arc::new(move || {
            // The state lock is free while the caller is told.
            let shared = observed.upgrade().unwrap();
            assert!(shared.state.try_lock().is_ok());
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }));
        shared.publish(&store.record, UpdateStatus::Shutdown);
        shared.publish(&store.record, UpdateStatus::Shutdown);
        assert_eq!(wakes.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn a_fresh_restart_preserves_due_time_and_never_admits_a_startup_request() {
        let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = dir.path().join("official");
        let seed = seed(now());
        let worker = OfficialWorker::start(root.clone(), seed.clone()).unwrap();
        let due = worker.next_refresh_unix();
        assert!(
            (seed.identity.created_unix + DAY..=seed.identity.created_unix + DAY + 21600)
                .contains(&due)
        );
        assert!(matches!(worker.status().status, UpdateStatus::Ready(_)));
        assert_eq!(worker.status().last_refresh_attempt_unix, None);
        assert!(worker.observe_and_take_catalogs().1.is_some());
        assert!(worker.observe_and_take_catalogs().1.is_none());
        assert_eq!(
            worker.shutdown(Duration::from_secs(2)),
            ShutdownOutcome::Complete
        );
        let restarted = OfficialWorker::start(root, seed).unwrap();
        assert_eq!(restarted.next_refresh_unix(), due);
        assert_eq!(restarted.status().last_refresh_attempt_unix, None);
        assert_eq!(
            restarted.shutdown(Duration::from_secs(2)),
            ShutdownOutcome::Complete
        );
    }

    #[test]
    fn verified_conditional_hits_keep_exact_sources() {
        let (_dir, store, package) = fixture();
        let (_sender, mut cancelled) = watch::channel(false);
        let mut calls = Vec::new();
        let mut fetch = |_source, validators: Option<FilterValidators>| {
            calls.push(validators.unwrap().etag);
            std::future::ready(Ok(FilterResponse::NotModified))
        };
        let result = runtime()
            .block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false))
            .unwrap();
        assert!(same_sources(&package, &result));
        assert_eq!(calls, vec![Some("\"stable\"".into()); 2]);
    }

    #[test]
    fn corrupt_cache_cannot_use_304_and_gets_only_one_unconditional_retry() {
        let (_dir, store, package) = fixture();
        let path = store
            .root_for_test()
            .join(crate::official_store::object_name_for_test(
                &package.sources[0].sha256,
            ));
        crate::storage_io::atomic_write(&path, b"corrupt").unwrap();
        let mut responses = VecDeque::from([
            FilterResponse::NotModified,
            FilterResponse::Modified {
                bytes: source(0),
                validators: FilterValidators::default(),
            },
            FilterResponse::NotModified,
        ]);
        let mut calls = Vec::new();
        let mut fetch = |_, validators: Option<FilterValidators>| {
            calls.push(validators.is_some());
            std::future::ready(Ok(responses.pop_front().unwrap()))
        };
        let (_sender, mut cancelled) = watch::channel(false);
        let result = runtime()
            .block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false))
            .unwrap();
        assert!(same_sources(&package, &result));
        assert_eq!(calls, vec![false, false, true]);
        assert!(store.cached_bytes(&package.sources[0]).is_ok());
    }

    #[test]
    fn unsolicited_304_without_validators_is_retried_then_rejected() {
        let (_dir, mut store, _) = fixture();
        let mut next = store.record.clone();
        for source in &mut next.current.as_mut().unwrap().sources {
            source.etag = None;
        }
        store.persist(next).unwrap();
        let mut calls = 0;
        let mut fetch = |_, _: Option<FilterValidators>| {
            calls += 1;
            std::future::ready(Ok(FilterResponse::NotModified))
        };
        let (_sender, mut cancelled) = watch::channel(false);
        assert!(matches!(
            runtime().block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false)),
            Err((FailureKind::Target, None))
        ));
        assert_eq!(calls, 2);
    }

    #[test]
    fn cancellation_interrupts_a_pending_transport_future() {
        let (_dir, store, _) = fixture();
        let (sender, mut cancelled) = watch::channel(false);
        let mut fetch = |_, _: Option<FilterValidators>| {
            std::future::pending::<Result<FilterResponse, FilterFetchFailure>>()
        };
        let started = Instant::now();
        let result = runtime().block_on(async {
            let stop = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = sender.send(true);
            });
            let result = fetch_sources(&store, &mut fetch, &mut cancelled, false).await;
            stop.await.unwrap();
            result
        });
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn exact_candidate_commit_waits_for_explicit_authorization_and_preserves_previous() {
        let (dir, mut store, original) = fixture();
        let mut bytes = source(0);
        bytes.extend_from_slice(b"||new-ad.example.invalid^\n");
        let sources = [
            store
                .put_source(
                    0,
                    &bytes,
                    FilterValidators::default(),
                    Some(&original.sources[0]),
                )
                .unwrap(),
            original.sources[1].clone(),
        ];
        let candidate = stage(&mut store, sources).unwrap();
        drop(store);
        let worker = OfficialWorker::start(dir.path().join("official"), seed(1)).unwrap();
        let (status, initial, prepared) = worker.observe_and_take_catalogs();
        assert!(matches!(status.status, UpdateStatus::Ready(_)));
        assert_eq!(initial.unwrap().identity, original.identity);
        assert_eq!(prepared.unwrap().identity, candidate.identity);
        assert!(worker.observe_and_take_catalogs().2.is_none());
        assert_eq!(worker.request_refresh(), RefreshAdmission::Busy);
        let (tx, rx) = mpsc::sync_channel(1);
        assert_eq!(
            worker.commit_candidate(
                candidate.identity.clone(),
                Box::new(move |outcome| {
                    let _ = tx.send(outcome);
                })
            ),
            CandidateCommitDispatch::Scheduled
        );
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            CandidateCommitOutcome::Committed
        );
        assert_eq!(
            worker.shutdown(Duration::from_secs(2)),
            ShutdownOutcome::Complete
        );
        let store = OfficialStore::open(dir.path().join("official"), 100, now()).unwrap();
        assert_eq!(
            store.record.current.as_ref().unwrap().identity,
            candidate.identity
        );
        assert_eq!(
            store.record.previous.as_ref().unwrap().identity,
            original.identity
        );
    }
}
