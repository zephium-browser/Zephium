//! Single bounded worker for official HTTPS sources; idle workers block on IPC.
use crate::official_store::{OfficialStore, Package, Record, Schedule, Source, REVERT_AFTER};
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
/// The first check after a new bundle, so a list that changes several times a
/// day is not left at the bundle's copy for a whole day.
const FIRST_CHECK: u64 = 6 * 3600;

pub(crate) struct OfficialWorker {
    sender: Option<mpsc::SyncSender<Command>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    cancel: watch::Sender<bool>,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
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
            next.due = seed.identity.created_unix.saturating_add(FIRST_CHECK);
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
                                Ok(fetched) => finish_refresh(
                                    &mut store,
                                    &seed,
                                    &worker_shared,
                                    operation,
                                    fetched,
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
                                next.due = next_due(&next);
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
                                        Ok(fetched)
                                            if same_sources(&original, &fetched.sources) =>
                                        {
                                            CandidateRepairOutcome::Repaired
                                        }
                                        Ok(fetched) => match stage(&mut store, fetched) {
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
fn same_sources(package: &Package, sources: &[Source]) -> bool {
    package.sources.len() == sources.len()
        && package
            .sources
            .iter()
            .zip(sources)
            .all(|(a, b)| a.id == b.id && a.sha256 == b.sha256 && a.bytes == b.bytes)
}

/// One refresh: a descriptor for every list, which of them were checked now,
/// and which failed and kept their last verified copy.
struct Fetched {
    sources: Vec<Source>,
    checked: Vec<usize>,
    failed: Vec<(usize, (FailureKind, Option<u64>))>,
    /// Checked lists that returned an older copy than the one kept, with that
    /// copy's digest and how many checks in a row returned it.
    stale: Vec<(usize, [u8; 32], u8)>,
}

/// The worker's next wake-up: the earliest list due.
fn next_due(record: &Record) -> u64 {
    record
        .schedule
        .iter()
        .map(|schedule| schedule.due)
        .min()
        .unwrap_or(record.due)
}

/// Moves each list's schedule on: a checked list is due after its own
/// interval (daily lists with the record's jitter), a failed one backs off.
fn reschedule(next: &mut Record, fetched: &Fetched, now: u64) {
    for &index in &fetched.checked {
        let interval = fetched.sources[index].interval;
        let jitter = if interval >= DAY { next.jitter } else { 0 };
        let rollbacks = next.schedule[index].rollbacks;
        next.schedule[index] = Schedule {
            due: now.saturating_add(interval + jitter),
            rollbacks,
            ..Schedule::default()
        };
    }
    for &(index, sha256, seen) in &fetched.stale {
        let schedule = &mut next.schedule[index];
        schedule.stale = Some((sha256, seen));
        schedule.rollbacks = schedule.rollbacks.saturating_add(1);
    }
    for &(index, (_, retry)) in &fetched.failed {
        let schedule = &mut next.schedule[index];
        schedule.failure_streak = schedule.failure_streak.saturating_add(1).min(8);
        schedule.due = now.saturating_add(
            BACKOFF[usize::from(schedule.failure_streak.saturating_sub(1).min(3))]
                .max(retry.unwrap_or(0)),
        );
    }
    next.due = next_due(next);
}

async fn fetch_official_sources(
    store: &OfficialStore,
    client: &OfficialFilterClient,
    cancelled: &mut watch::Receiver<bool>,
    force: bool,
) -> Result<Fetched, (FailureKind, Option<u64>)> {
    let mut fetch =
        |source,
         validators: Option<zephium_update_transport::official_filters::FilterValidators>| {
            let client = client.clone();
            async move { client.fetch(source, validators.as_ref()).await }
        };
    fetch_sources(store, &mut fetch, cancelled, force).await
}

/// Checks every list that is due (all of them when none is, as for a manual
/// refresh, or with `force`). A list that is not due, or whose check fails,
/// keeps its last verified copy; one with no such copy fails the refresh.
async fn fetch_sources<F, Fut>(
    store: &OfficialStore,
    fetch: &mut F,
    cancelled: &mut watch::Receiver<bool>,
    force: bool,
) -> Result<Fetched, (FailureKind, Option<u64>)>
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
    let checked_at = now();
    let any_due = store
        .record
        .schedule
        .iter()
        .any(|schedule| schedule.due <= checked_at);
    let mut fetched = Fetched {
        sources: Vec::with_capacity(OfficialFilter::ALL.len()),
        checked: Vec::new(),
        failed: Vec::new(),
        stale: Vec::new(),
    };
    for (index, filter) in OfficialFilter::ALL.into_iter().enumerate() {
        let old = store.record.prior(filter);
        let cached = if force {
            None
        } else {
            old.and_then(|source| store.cached_bytes(source).ok())
        };
        let due = !any_due || store.record.schedule[index].due <= checked_at;
        if let (Some(old), Some(_), false) = (old, &cached, due) {
            fetched.sources.push(old.clone());
            continue;
        }
        match fetch_one(store, fetch, cancelled, filter, old, cached.is_some()).await {
            // An older copy than the one kept is a stale cache until the
            // same copy comes back REVERT_AFTER checks in a row.
            Ok(source)
                if cached.is_some() && old.is_some_and(|old| source.version < old.version) =>
            {
                let seen = match store.record.schedule[index].stale {
                    Some((sha256, seen)) if sha256 == source.sha256 => seen.saturating_add(1),
                    _ => 1,
                };
                if seen >= REVERT_AFTER {
                    fetched.sources.push(source);
                } else {
                    fetched
                        .sources
                        .push(old.expect("cached source descriptor").clone());
                    fetched.stale.push((index, source.sha256, seen));
                }
                fetched.checked.push(index);
            }
            Ok(source) => {
                fetched.sources.push(source);
                fetched.checked.push(index);
            }
            Err((FailureKind::Internal, retry)) => return Err((FailureKind::Internal, retry)),
            Err(failure) if cached.is_some() => {
                fetched
                    .sources
                    .push(old.expect("cached source descriptor").clone());
                fetched.failed.push((index, failure));
            }
            Err(failure) => return Err(failure),
        }
    }
    match fetched.failed.first() {
        // No list could be checked: the refresh failed.
        Some(&(_, failure)) if fetched.checked.is_empty() => Err(failure),
        _ => Ok(fetched),
    }
}

async fn fetch_one<F, Fut>(
    store: &OfficialStore,
    fetch: &mut F,
    cancelled: &mut watch::Receiver<bool>,
    filter: OfficialFilter,
    old: Option<&Source>,
    cached: bool,
) -> Result<Source, (FailureKind, Option<u64>)>
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
    let validators = old.filter(|_| cached).map(Source::validators);
    let mut attempt = 0;
    loop {
        if *cancelled.borrow() {
            return Err((FailureKind::Internal, None));
        }
        let response = tokio::select! {
            result=fetch(filter,if attempt==0 {validators.clone()}else{None})=>result.map_err(|e|(FailureKind::Transport,e.retry_after_seconds))?,
            _=cancelled.changed()=>return Err((FailureKind::Internal,None)),
        };
        match response {
            FilterResponse::NotModified
                if cached
                    && attempt == 0
                    && validators
                        .as_ref()
                        .is_some_and(|v| v.etag.is_some() || v.last_modified.is_some()) =>
            {
                return Ok(old.expect("cached source descriptor").clone());
            }
            FilterResponse::NotModified if attempt == 0 => {
                attempt = 1;
            }
            FilterResponse::NotModified => return Err((FailureKind::Target, None)),
            FilterResponse::Modified { bytes, validators } => {
                return store
                    .put_source(filter, &bytes, validators, old)
                    .map_err(|e| {
                        (
                            if e == crate::storage_io::StoreError::InvalidPackage {
                                FailureKind::Target
                            } else {
                                e.kind()
                            },
                            None,
                        )
                    });
            }
        }
    }
}
fn stage(
    store: &mut OfficialStore,
    fetched: Fetched,
) -> Result<Package, crate::storage_io::StoreError> {
    let now = now();
    let mut next = store.record.clone();
    reschedule(&mut next, &fetched, now);
    let package = store.new_package(fetched.sources, now)?;
    next.highest_revision = package.identity.revision;
    next.candidate = Some(package.clone());
    store.persist(next)?;
    Ok(package)
}
fn finish_refresh(
    store: &mut OfficialStore,
    seed: &ActivatedCatalog,
    shared: &Shared,
    operation: u64,
    fetched: Fetched,
) {
    if store
        .record
        .current
        .as_ref()
        .is_some_and(|p| same_sources(p, &fetched.sources))
    {
        let mut next = store.record.clone();
        if fetched.failed.is_empty() {
            next.checked = Some(now());
            next.verification = next.verification.saturating_add(1);
            next.failure_streak = 0;
        }
        reschedule(&mut next, &fetched, now());
        if let Some(current) = next.current.as_mut() {
            current.sources = fetched.sources;
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
    match stage(store, fetched) {
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
    use crate::official_store::{test_list, test_sources};
    use std::collections::VecDeque;
    use zephium_update_transport::official_filters::{FilterFetchFailure, FilterValidators};

    fn seed(created: u64) -> ActivatedCatalog {
        ActivatedCatalog {
            identity: CatalogIdentity {
                revision: 100,
                manifest_sha256: [1; 32],
                created_unix: created,
                expires_unix: created + 7 * DAY,
                source_count: 4,
                source_bytes: 1200000,
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
        let sources = test_sources(&store, &validators);
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
    fn a_fresh_restart_preserves_due_time_and_never_admits_a_startup_request() {
        let dir = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let root = dir.path().join("official");
        let seed = seed(now());
        let worker = OfficialWorker::start(root.clone(), seed.clone()).unwrap();
        let due = worker.next_refresh_unix();
        assert_eq!(due, seed.identity.created_unix + FIRST_CHECK);
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
        assert!(same_sources(&package, &result.sources));
        assert_eq!(result.checked, [0, 1, 2, 3]);
        assert_eq!(calls, vec![Some("\"stable\"".into()); 4]);
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
                bytes: test_list(OfficialFilter::EasyList),
                validators: FilterValidators::default(),
            },
            FilterResponse::NotModified,
            FilterResponse::NotModified,
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
        assert!(same_sources(&package, &result.sources));
        assert_eq!(calls, vec![false, false, true, true, true]);
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
        // Each list keeps its copy and fails; none was checked, so the refresh fails.
        assert_eq!(calls, 8);
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
        let mut bytes = test_list(OfficialFilter::EasyList);
        bytes.extend_from_slice(b"||new-ad.example.invalid^\n");
        let mut sources = original.sources.clone();
        sources[0] = store
            .put_source(
                OfficialFilter::EasyList,
                &bytes,
                FilterValidators::default(),
                Some(&original.sources[0]),
            )
            .unwrap();
        let fetched = Fetched {
            sources,
            checked: vec![0],
            failed: Vec::new(),
            stale: Vec::new(),
        };
        let candidate = stage(&mut store, fetched).unwrap();
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

    fn due_only(store: &mut OfficialStore, filter: OfficialFilter) {
        let mut next = store.record.clone();
        for (index, other) in OfficialFilter::ALL.into_iter().enumerate() {
            next.schedule[index].due = if other == filter { now() } else { now() + DAY };
        }
        store.persist(next).unwrap();
    }

    #[test]
    fn only_lists_that_are_due_are_checked() {
        let (_dir, mut store, package) = fixture();
        due_only(&mut store, OfficialFilter::UblockQuickFixes);
        let (_sender, mut cancelled) = watch::channel(false);
        let mut calls = Vec::new();
        let mut fetch = |filter, _: Option<FilterValidators>| {
            calls.push(filter);
            std::future::ready(Ok(FilterResponse::NotModified))
        };
        let result = runtime()
            .block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false))
            .unwrap();
        assert_eq!(calls, [OfficialFilter::UblockQuickFixes]);
        assert_eq!(result.checked, [3]);
        assert!(same_sources(&package, &result.sources));
    }

    #[test]
    fn a_failing_list_keeps_its_copy_while_the_others_update() {
        let (_dir, store, package) = fixture();
        let (_sender, mut cancelled) = watch::channel(false);
        let mut fetch = |filter, _: Option<FilterValidators>| {
            std::future::ready(if filter == OfficialFilter::UblockFilters {
                Err(FilterFetchFailure {
                    retry_after_seconds: Some(7200),
                })
            } else {
                Ok(FilterResponse::NotModified)
            })
        };
        let result = runtime()
            .block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false))
            .unwrap();
        assert!(same_sources(&package, &result.sources));
        assert_eq!(result.checked, [0, 1, 3]);
        assert_eq!(result.failed, [(2, (FailureKind::Transport, Some(7200)))]);
        let mut next = store.record.clone();
        let at = now();
        reschedule(&mut next, &result, at);
        assert_eq!(next.schedule[2].failure_streak, 1);
        assert_eq!(next.schedule[2].due, at + 7200);
        assert_eq!(next.schedule[3].due, at + 8 * 3600);
        assert_eq!(next.schedule[0].due, at + DAY + next.jitter);
        assert_eq!(next.due, at + 7200);
    }

    #[test]
    fn a_list_without_a_verified_copy_fails_the_refresh() {
        let (_dir, store, package) = fixture();
        let path = store
            .root_for_test()
            .join(crate::official_store::object_name_for_test(
                &package.sources[2].sha256,
            ));
        crate::storage_io::atomic_write(&path, b"corrupt").unwrap();
        let (_sender, mut cancelled) = watch::channel(false);
        let mut fetch = |filter, _: Option<FilterValidators>| {
            std::future::ready(if filter == OfficialFilter::UblockFilters {
                Err(FilterFetchFailure {
                    retry_after_seconds: None,
                })
            } else {
                Ok(FilterResponse::NotModified)
            })
        };
        assert!(matches!(
            runtime().block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false)),
            Err((FailureKind::Transport, None))
        ));
    }

    #[test]
    fn migrated_lists_are_revalidated_instead_of_downloaded_again() {
        let (_dir, mut store, package) = fixture();
        let mut next = store.record.clone();
        next.retained = package.sources[..2].to_vec();
        next.current = None;
        store.persist(next).unwrap();
        let (_sender, mut cancelled) = watch::channel(false);
        let mut calls = Vec::new();
        let mut fetch = |filter: OfficialFilter, validators: Option<FilterValidators>| {
            calls.push((filter, validators.is_some()));
            std::future::ready(Ok(if validators.is_some() {
                FilterResponse::NotModified
            } else {
                FilterResponse::Modified {
                    bytes: test_list(filter),
                    validators: FilterValidators::default(),
                }
            }))
        };
        let result = runtime()
            .block_on(fetch_sources(&store, &mut fetch, &mut cancelled, false))
            .unwrap();
        assert_eq!(
            calls,
            [
                (OfficialFilter::EasyList, true),
                (OfficialFilter::EasyPrivacy, true),
                (OfficialFilter::UblockFilters, false),
                (OfficialFilter::UblockQuickFixes, false),
            ]
        );
        assert_eq!(result.sources[..2], package.sources[..2]);
    }

    fn quick_fixes_dated(date: &str, extra: &str) -> Vec<u8> {
        let text = String::from_utf8(test_list(OfficialFilter::UblockQuickFixes)).unwrap();
        let text = text.replace(
            "! Last modified: Wed, 30 Sep 2026 00:01:00 +0000",
            &format!("! Last modified: {date}"),
        );
        format!("{text}{extra}").into_bytes()
    }

    fn check_quick_fixes(store: &mut OfficialStore, bytes: &[u8]) -> Fetched {
        due_only(store, OfficialFilter::UblockQuickFixes);
        let (_sender, mut cancelled) = watch::channel(false);
        let mut fetch = |_, _: Option<FilterValidators>| {
            std::future::ready(Ok(FilterResponse::Modified {
                bytes: bytes.to_vec(),
                validators: FilterValidators::default(),
            }))
        };
        let fetched = runtime()
            .block_on(fetch_sources(store, &mut fetch, &mut cancelled, false))
            .unwrap();
        let mut next = store.record.clone();
        reschedule(&mut next, &fetched, now());
        store.persist(next).unwrap();
        fetched
    }

    #[test]
    fn an_older_copy_is_kept_out_until_upstream_returns_it_three_times() {
        let (_dir, mut store, package) = fixture();
        let older = quick_fixes_dated(
            "Tue, 29 Sep 2026 00:01:00 +0000",
            "||reverted.example.invalid^\n",
        );
        for seen in 1..REVERT_AFTER {
            let fetched = check_quick_fixes(&mut store, &older);
            assert_eq!(fetched.sources[3], package.sources[3], "check {seen}");
            let schedule = store.record.schedule[3];
            assert_eq!(schedule.stale.map(|(_, n)| n), Some(seen));
            assert_eq!(schedule.rollbacks, u32::from(seen));
            assert_eq!(schedule.failure_streak, 0);
        }
        let fetched = check_quick_fixes(&mut store, &older);
        assert_eq!(fetched.sources[3].version, "202609290001");
        assert_eq!(store.record.schedule[3].stale, None);
        assert_eq!(
            store.record.schedule[3].rollbacks,
            u32::from(REVERT_AFTER - 1)
        );
    }

    #[test]
    fn a_different_older_copy_starts_the_count_again() {
        let (_dir, mut store, package) = fixture();
        let first = quick_fixes_dated("Tue, 29 Sep 2026 00:01:00 +0000", "||a.example.invalid^\n");
        let second = quick_fixes_dated("Tue, 29 Sep 2026 00:01:00 +0000", "||b.example.invalid^\n");
        check_quick_fixes(&mut store, &first);
        check_quick_fixes(&mut store, &first);
        let fetched = check_quick_fixes(&mut store, &second);
        assert_eq!(fetched.sources[3], package.sources[3]);
        assert_eq!(store.record.schedule[3].stale.map(|(_, n)| n), Some(1));
    }

    #[test]
    fn the_same_version_with_other_bytes_is_accepted() {
        let (_dir, mut store, package) = fixture();
        let changed = quick_fixes_dated(
            "Wed, 30 Sep 2026 00:01:00 +0000",
            "||new.example.invalid^\n",
        );
        let fetched = check_quick_fixes(&mut store, &changed);
        assert_eq!(fetched.sources[3].version, package.sources[3].version);
        assert_ne!(fetched.sources[3].sha256, package.sources[3].sha256);
        assert_eq!(store.record.schedule[3].rollbacks, 0);
    }
}
