//! Main-thread native download coordinator. Native operations own transport;
//! the Store owns durable metadata; bounded workers own blocking filesystem IO.

mod lifecycle;
#[cfg(target_os = "macos")]
#[path = "downloads/platform_macos.rs"]
mod platform;
#[cfg(target_os = "windows")]
#[path = "downloads/platform_windows.rs"]
mod platform;
mod recovery;
mod ui;
use platform::{Delegate, DialogLease, DirectoryPanel, Native, SavePanel, Source, Timer};

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
#[cfg(target_os = "macos")]
use std::rc::Weak;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use zephium_core::downloads::*;
use zephium_core::ids::{DownloadId, ProfileId};
use zephium_core::permissions::PageOrigin;
use zephium_core::ports::{engine::Partition, store::Store};

use super::download_files::{verify_file, Destination};
use super::permits::EventPermit;
use crate::navigation_epoch::{NavigationActivity, NavigationEpochTracker};

type SharedStore = Arc<dyn Store + Send + Sync>;
type Notify = Arc<dyn Fn(ProfileId) + Send + Sync>;
struct DestinationReply(Option<Box<dyn FnOnce(Option<PathBuf>)>>);
impl DestinationReply {
    fn new(done: impl FnOnce(Option<PathBuf>) + 'static) -> Self {
        Self(Some(Box::new(done)))
    }
    fn finish(mut self, path: Option<PathBuf>) {
        if let Some(done) = self.0.take() {
            done(path);
        }
    }
}
impl Drop for DestinationReply {
    fn drop(&mut self) {
        if let Some(done) = self.0.take() {
            done(None);
        }
    }
}
type DrainCompletion = Box<dyn FnOnce(bool) + Send>;
type DrainWaiter = (Option<ProfileId>, DrainCompletion);
const MAX_UI_CALLS: usize = 4;
// Include detached cleanup/persistence work in new-transfer admission. Completed
// records can leave the active map before slow filesystem cleanup returns.
const MAX_BACKGROUND_WORK: usize = 32;
const RECENT_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DecisionPhase {
    Admission,
    NativePicker,
    PreparingDestination,
    PersistingDestination,
    #[cfg(any(target_os = "windows", test))]
    ResumableInterruption,
}

#[derive(Clone, Copy)]
struct DecisionDeadline {
    phase: DecisionPhase,
    expires: Instant,
}
impl DecisionDeadline {
    fn new(phase: DecisionPhase, now: Instant) -> Self {
        let duration = match phase {
            // Filesystem calls can synchronously wait for native TCC/target
            // consent, just as a picker can wait for a person's choice.
            DecisionPhase::NativePicker | DecisionPhase::PreparingDestination => {
                Duration::from_secs(24 * 60 * 60)
            }
            DecisionPhase::Admission | DecisionPhase::PersistingDestination => {
                Duration::from_secs(30)
            }
            #[cfg(any(target_os = "windows", test))]
            DecisionPhase::ResumableInterruption => Duration::from_secs(5 * 60),
        };
        Self {
            phase,
            expires: now + duration,
        }
    }
    fn expired(self, now: Instant) -> bool {
        now > self.expires
    }
    fn prepared(
        self,
        state: DownloadState,
        cancelling: bool,
        terminal: bool,
        now: Instant,
    ) -> Option<Self> {
        (self.phase == DecisionPhase::PreparingDestination
            && state == DownloadState::Pending
            && !cancelling
            && !terminal
            && !self.expired(now))
        .then(|| Self::new(DecisionPhase::PersistingDestination, now))
    }
}

/// One native, user-admitted initial navigation may download before its child
/// is presented. Dropping an unconsumed admission also retires its empty tab.
pub(in crate::host) struct InitialDownload {
    #[cfg(target_os = "macos")]
    pub window: objc2::rc::Retained<objc2_app_kit::NSWindow>,
    pub on_started: Option<Box<dyn FnOnce()>>,
}
impl Drop for InitialDownload {
    fn drop(&mut self) {
        if let Some(done) = self.on_started.take() {
            done();
        }
    }
}

struct Transfer {
    partition: Partition,
    record: DownloadRecord,
    native: Native,
    _delegate: Delegate,
    source: Option<Source>,
    destination_reply: Option<DestinationReply>,
    destination: Option<Destination>,
    panel: Option<SavePanel>,
    panel_lease: Option<DialogLease>,
    on_started: Option<Box<dyn FnOnce()>>,
    authorized: bool,
    cancelling: bool,
    persisting_terminal: bool,
    deadline: DecisionDeadline,
}

enum Message {
    Preferences(DownloadId, DownloadStoreReply),
    Prepared(DownloadId, Result<Destination, DownloadError>),
    Persisted(DownloadId, u32, DownloadStoreReply),
    Finalized(DownloadId, Result<(PathBuf, FileIdentity), DownloadError>),
    #[cfg(target_os = "macos")]
    Cancelled(DownloadId),
    Cleaned,
    #[cfg(target_os = "windows")]
    RuntimeExited(bool),
    RecoveryProfiles(DownloadStoreReply),
    RecoveryPage(ProfileId, DownloadStoreReply),
    Recovered(
        ProfileId,
        Vec<(DownloadId, FileIdentity)>,
        Option<DownloadId>,
        Option<DownloadError>,
    ),
    RecoverySaved(ProfileId, DownloadId, FileIdentity, DownloadStoreReply),
    Ui(u64, DownloadStoreReply),
    Verified(u64, PathBuf, Result<(), DownloadError>),
    DirectorySelected(
        UiCall,
        DownloadPreferences,
        Result<(String, String), DownloadError>,
    ),
}
struct UiCall {
    partition: Partition,
    call: DownloadCall,
    done: DownloadCompletion,
}

pub(crate) struct Downloads {
    session: DownloadId,
    store: SharedStore,
    notify: Notify,
    active: RefCell<HashMap<DownloadId, Transfer>>,
    recent: RefCell<VecDeque<(Partition, DownloadRecord)>>,
    forgotten: RefCell<VecDeque<(ProfileId, DownloadId)>>,
    preferences: RefCell<HashMap<ProfileId, DownloadPreferences>>,
    calls: RefCell<HashMap<u64, UiCall>>,
    next_call: Cell<u64>,
    sender: mpsc::Sender<Message>,
    receiver: RefCell<mpsc::Receiver<Message>>,
    timer: RefCell<Option<Timer>>,
    stopping: Cell<bool>,
    retired: RefCell<HashSet<ProfileId>>,
    drained_profiles: RefCell<HashSet<ProfileId>>,
    waiters: RefCell<Vec<DrainWaiter>>,
    persistence_failed: Cell<bool>,
    recovery: RefCell<recovery::Recovery>,
    work: Cell<usize>,
    directory_panels: RefCell<HashMap<u64, (DirectoryPanel, DialogLease)>>,
    #[cfg(target_os = "windows")]
    retained_views: RefCell<Vec<super::ObservedView>>,
}

impl Downloads {
    /// A transfer or destination decision still owns this profile. Retaining
    /// its view costs less than risking a stalled or revoked native operation.
    #[cfg(target_os = "windows")]
    pub(in crate::host) fn has_profile_activity(&self, profile: ProfileId) -> bool {
        self.active.try_borrow().map_or(true, |active| {
            active
                .values()
                .any(|transfer| transfer.partition.profile() == profile)
        }) || self.calls.try_borrow().map_or(true, |calls| {
            calls
                .values()
                .any(|call| call.partition.profile() == profile)
        })
    }
    /// Admitted macOS transfers keep their WKDownload without the page; only
    /// an unanswered destination decision still depends on it.
    #[cfg(target_os = "macos")]
    pub(in crate::host) fn has_pending_decision(&self, profile: ProfileId) -> bool {
        self.active.try_borrow().map_or(true, |active| {
            active
                .values()
                .any(|transfer| transfer.partition.profile() == profile && !transfer.authorized)
        }) || self.calls.try_borrow().map_or(true, |calls| {
            calls
                .values()
                .any(|call| call.partition.profile() == profile)
        })
    }
    pub(crate) fn new(store: SharedStore, notify: Notify) -> Rc<Self> {
        let (sender, receiver) = mpsc::channel();
        let manager = Rc::new(Self {
            session: DownloadId::generate(),
            store,
            notify,
            active: RefCell::new(HashMap::new()),
            recent: RefCell::new(VecDeque::new()),
            forgotten: RefCell::new(VecDeque::new()),
            preferences: RefCell::new(HashMap::new()),
            calls: RefCell::new(HashMap::new()),
            next_call: Cell::new(1),
            sender,
            receiver: RefCell::new(receiver),
            timer: RefCell::new(None),
            stopping: Cell::new(false),
            retired: RefCell::new(HashSet::new()),
            drained_profiles: RefCell::new(HashSet::new()),
            waiters: RefCell::new(Vec::new()),
            persistence_failed: Cell::new(false),
            recovery: RefCell::default(),
            work: Cell::new(0),
            directory_panels: RefCell::new(HashMap::new()),
            #[cfg(target_os = "windows")]
            retained_views: RefCell::new(Vec::new()),
        });
        manager.initialize_recovery();
        manager
    }

    fn prepare(self: &Rc<Self>, id: DownloadId, path: PathBuf, expected_directory: Option<String>) {
        {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            if transfer.cancelling || !transfer.source.as_ref().is_some_and(Source::live) {
                drop(active);
                self.cancel(id, None);
                return;
            }
            transfer.authorized = true;
            transfer.source = None;
            transfer.deadline =
                DecisionDeadline::new(DecisionPhase::PreparingDestination, Instant::now());
        }
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if std::thread::Builder::new()
            .name("zephium-download-destination".into())
            .spawn(move || {
                let result = Destination::prepare(id, path, expected_directory);
                let _ = sender.send(Message::Prepared(id, result));
            })
            .is_err()
        {
            self.work.set(self.work.get() - 1);
            self.cancel(id, Some(DownloadError::Unavailable));
        }
    }

    fn persist(&self, id: DownloadId) {
        let entry = self
            .active
            .borrow()
            .get(&id)
            .map(|transfer| (transfer.partition, transfer.record.clone()));
        let Some((partition, record)) = entry else {
            return;
        };
        let revision = record.revision;
        self.work.set(self.work.get() + 1);
        if matches!(partition, Partition::Ephemeral(_)) {
            self.remember_volatile_cleanup(partition.profile(), &record);
            let _ = self
                .sender
                .send(Message::Persisted(id, revision, DownloadStoreReply::Saved));
            return;
        }
        let sender = self.sender.clone();
        if !self.store.download_call(
            partition.profile(),
            DownloadStoreCall::Save(Box::new(record)),
            Box::new(move |reply| {
                let _ = sender.send(Message::Persisted(id, revision, reply));
            }),
        ) {
            let _ = self.sender.send(Message::Persisted(
                id,
                revision,
                DownloadStoreReply::Error(DownloadError::Storage),
            ));
        }
    }

    fn native_finished(self: &Rc<Self>, id: DownloadId) {
        let transition = self.active.borrow().get(&id).and_then(|transfer| {
            native_terminal_transition(transfer.record.state, transfer.cancelling, true)
        });
        match transition {
            Some(DownloadState::Finalizing) => {}
            Some(state) => {
                self.terminal(
                    id,
                    state,
                    if state == DownloadState::Failed {
                        Some(DownloadError::Invalid)
                    } else {
                        None
                    },
                );
                return;
            }
            None => return,
        }

        let work = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            if transfer.cancelling || transfer.record.state != DownloadState::Receiving {
                return;
            }
            transfer.record.state = DownloadState::Finalizing;
            transfer.record.revision += 1;
            transfer.destination.take().map(|destination| {
                (
                    destination,
                    transfer.record.source.clone(),
                    transfer.partition.profile(),
                    transfer.native.clone(),
                )
            })
        };
        let Some((destination, source, profile, native)) = work else {
            self.terminal(id, DownloadState::Failed, Some(DownloadError::Destination));
            return;
        };
        // Native getters can pump callbacks. Finalizing is set before this
        // call, so a reentrant completion cannot start another publication.
        let expected_bytes = match platform::completion_bytes(&native) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.cleanup(destination);
                self.terminal(id, DownloadState::Failed, Some(error));
                return;
            }
        };
        (self.notify)(profile);
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if std::thread::Builder::new()
            .name("zephium-download-publish".into())
            .spawn(move || {
                let _ = sender.send(Message::Finalized(
                    id,
                    destination.finish(&source, expected_bytes),
                ));
            })
            .is_err()
        {
            self.work.set(self.work.get() - 1);
            self.terminal(id, DownloadState::Failed, Some(DownloadError::Unavailable));
        }
    }

    #[cfg(target_os = "windows")]
    fn native_failed(&self, id: DownloadId) {
        self.native_failed_with_error(id, DownloadError::Network);
    }

    #[cfg(target_os = "windows")]
    fn pause(&self, id: DownloadId, error: DownloadError) -> bool {
        let profile = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id).filter(|transfer| {
                transfer.record.state == DownloadState::Receiving
                    && !transfer.cancelling
                    && !transfer.persisting_terminal
                    && transfer.destination.is_some()
                    && retry_source_alive(&transfer.native.permit)
            }) else {
                return false;
            };
            transfer.record.state = DownloadState::Paused;
            transfer.record.error = Some(error);
            transfer.deadline =
                DecisionDeadline::new(DecisionPhase::ResumableInterruption, Instant::now());
            transfer.record.revision += 1;
            transfer.partition.profile()
        };
        self.persist(id);
        (self.notify)(profile);
        true
    }

    #[cfg(target_os = "windows")]
    fn note_native_error(&self, id: DownloadId, error: DownloadError) {
        if let Some(transfer) = self.active.borrow_mut().get_mut(&id).filter(|transfer| {
            !transfer.record.state.terminal()
                && transfer.record.state != DownloadState::Finalizing
                && !transfer.persisting_terminal
                && !transfer.cancelling
        }) {
            latch_native_error(&mut transfer.record.error, error);
        }
    }

    fn native_failed_with_error(&self, id: DownloadId, error: DownloadError) {
        let transition = self.active.borrow().get(&id).and_then(|transfer| {
            native_terminal_transition(transfer.record.state, transfer.cancelling, false)
        });
        if let Some(state) = transition {
            let state = if state == DownloadState::Failed && error == DownloadError::Cancelled {
                DownloadState::Cancelled
            } else {
                state
            };
            self.terminal(
                id,
                state,
                if state == DownloadState::Failed {
                    Some(error)
                } else {
                    None
                },
            );
        }
    }

    fn terminal(&self, id: DownloadId, state: DownloadState, error: Option<DownloadError>) {
        let (destination, on_started) = {
            let mut active = self.active.borrow_mut();
            let Some(transfer) = active.get_mut(&id) else {
                return;
            };
            if transfer.persisting_terminal {
                return;
            }
            transfer.record.writer_released = true;
            (transfer.record.state, transfer.record.error) =
                terminal_outcome(state, transfer.record.error, error);
            transfer.record.revision += 1;
            transfer.persisting_terminal = true;
            (transfer.destination.take(), transfer.on_started.take())
        };
        if let Some(started) = on_started {
            started();
        }
        if let Some(destination) = destination {
            // A terminal native callback proves it no longer writes payload.
            self.cleanup(destination);
        }
        self.persist(id);
    }

    fn tick(self: &Rc<Self>) {
        for _ in 0..64 {
            let message = self.receiver.borrow().try_recv().ok();
            let Some(message) = message else { break };
            self.message(message);
        }
        let mut cancel = Vec::new();
        let mut changed = Vec::new();
        let samples: Vec<_> = self
            .active
            .borrow()
            .iter()
            .filter(|(_, transfer)| transfer.record.state == DownloadState::Receiving)
            .map(|(id, transfer)| (*id, transfer.native.clone()))
            .collect();
        for (id, native) in samples {
            let Some((received, total)) = platform::progress(&native) else {
                continue;
            };
            if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
                if !transfer.cancelling && transfer.record.update_progress(received, total) {
                    changed.push(transfer.partition.profile());
                }
            }
        }
        for (id, transfer) in self.active.borrow().iter() {
            if transfer.record.state == DownloadState::Pending
                && (transfer.deadline.expired(Instant::now())
                    || (!transfer.authorized
                        && !transfer.source.as_ref().is_some_and(Source::live)))
            {
                cancel.push((*id, DownloadError::Unavailable));
            } else if transfer.record.state == DownloadState::Paused
                && transfer.deadline.expired(Instant::now())
            {
                cancel.push((*id, transfer.record.error.unwrap_or(DownloadError::Timeout)));
            }
        }
        for (id, error) in cancel {
            self.cancel(id, Some(error));
        }
        changed.sort();
        changed.dedup();
        for profile in changed {
            (self.notify)(profile);
        }
        #[cfg(target_os = "windows")]
        self.release_retained_views();
        if self.work.get() == 0 {
            let mut ready = Vec::new();
            {
                let mut waiters = self.waiters.borrow_mut();
                let mut index = 0;
                while index < waiters.len() {
                    let profile = waiters[index].0;
                    let active = self.active.borrow().values().any(|transfer| {
                        profile.is_none_or(|profile| transfer.partition.profile() == profile)
                    });
                    let calls = self.calls.borrow().values().any(|call| {
                        profile.is_none_or(|profile| call.partition.profile() == profile)
                    });
                    if !active && !calls && !self.recovery_busy(profile) {
                        ready.push(waiters.swap_remove(index));
                    } else {
                        index += 1;
                    }
                }
            }
            for (profile, done) in ready {
                self.recent
                    .borrow_mut()
                    .retain(|(owner, _)| profile.is_some_and(|profile| owner.profile() != profile));
                self.preferences
                    .borrow_mut()
                    .retain(|owner, _| profile.is_some_and(|profile| *owner != profile));
                let clean =
                    !self.persistence_failed.get() && self.finish_retirement_recovery(profile);
                if clean {
                    if let Some(profile) = profile {
                        self.drained_profiles.borrow_mut().insert(profile);
                    }
                }
                done(clean);
            }
        }
        if self.active.borrow().is_empty() && self.calls.borrow().is_empty() && self.work.get() == 0
        {
            if let Some(timer) = self.timer.borrow_mut().take() {
                platform::stop_timer(timer);
            }
        } else {
            self.ensure_timer();
        }
    }

    fn cleanup(&self, destination: Destination) {
        self.work.set(self.work.get() + 1);
        let sender = self.sender.clone();
        if std::thread::Builder::new()
            .name("zephium-download-cleanup".into())
            .spawn(move || {
                drop(destination);
                let _ = sender.send(Message::Cleaned);
            })
            .is_err()
        {
            self.work.set(self.work.get() - 1);
        }
    }

    fn message(self: &Rc<Self>, message: Message) {
        #[cfg(target_os = "macos")]
        let native_notice = matches!(message, Message::Cancelled(_));
        #[cfg(target_os = "windows")]
        let native_notice = matches!(message, Message::RuntimeExited(_));
        if !native_notice {
            self.work.set(self.work.get().saturating_sub(1));
        }
        match message {
            Message::RecoveryProfiles(reply) => self.recovery_profiles(reply),
            Message::RecoveryPage(profile, reply) => self.recovery_page(profile, reply),
            Message::Recovered(profile, records, next, error) => {
                self.recovered(profile, records, next, error)
            }
            Message::RecoverySaved(profile, id, identity, reply) => {
                self.recovery_saved(profile, id, identity, reply)
            }
            Message::Cleaned => {}
            #[cfg(target_os = "windows")]
            Message::RuntimeExited(clean) => self.runtime_exit_result(clean),
            Message::Preferences(id, DownloadStoreReply::Preferences(preferences)) => {
                self.choose_destination(id, preferences)
            }
            Message::Preferences(id, _) => self.cancel(id, Some(DownloadError::Storage)),
            Message::Prepared(id, result) => match result {
                Ok(destination) => {
                    let mut active = self.active.borrow_mut();
                    let now = Instant::now();
                    let Some(transfer) = active.get_mut(&id) else {
                        drop(active);
                        self.cleanup(destination);
                        return;
                    };
                    let Some(deadline) = transfer
                        .deadline
                        .prepared(
                            transfer.record.state,
                            transfer.cancelling,
                            transfer.persisting_terminal,
                            now,
                        )
                        .filter(|_| {
                            transfer.authorized
                                && transfer.destination_reply.is_some()
                                && transfer.destination.is_none()
                        })
                    else {
                        // Consent/work can finish after cancellation, failure,
                        // profile retirement or expiry. Never revive that request
                        // or create a new cleanup receipt from a stale result.
                        drop(active);
                        self.cleanup(destination);
                        return;
                    };
                    transfer.deadline = deadline;
                    transfer.record.filename = destination
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "download".into());
                    transfer.record.destination = Some(destination.path.clone());
                    transfer.record.staging = Some(destination.staging_path.clone());
                    transfer.record.staging_identity = Some(destination.staging_identity.clone());
                    transfer.destination = Some(destination);
                    transfer.record.revision += 1;
                    drop(active);
                    self.persist(id);
                }
                Err(error) => self.cancel(id, Some(error)),
            },
            Message::Persisted(id, revision, reply) => {
                let terminal = self
                    .active
                    .borrow()
                    .get(&id)
                    .filter(|transfer| transfer.record.revision == revision)
                    .map(|transfer| transfer.persisting_terminal);
                let Some(terminal) = terminal else { return };
                let saved = matches!(reply, DownloadStoreReply::Saved);
                if terminal {
                    let transfer = self.active.borrow_mut().remove(&id);
                    if let Some(mut transfer) = transfer {
                        if !saved {
                            self.persistence_failed.set(true);
                            transfer.record.error = Some(DownloadError::Storage);
                        }
                        let profile = transfer.partition.profile();
                        let mut recent = self.recent.borrow_mut();
                        recent.push_front((transfer.partition, transfer.record));
                        while recent.len() > RECENT_LIMIT {
                            if let Some((Partition::Ephemeral(owner), expired)) = recent.pop_back()
                            {
                                let mut forgotten = self.forgotten.borrow_mut();
                                forgotten.push_front((owner, expired.id));
                                while forgotten.len() > RECENT_LIMIT {
                                    forgotten.pop_back();
                                }
                            }
                        }
                        drop(recent);
                        (self.notify)(profile);
                        self.schedule_recovery(profile);
                    }
                } else if saved {
                    let start = {
                        let mut active = self.active.borrow_mut();
                        active
                            .get_mut(&id)
                            .filter(|transfer| !transfer.cancelling)
                            .and_then(|transfer| {
                                let destination = transfer.destination.as_ref()?;
                                let path = destination.payload();
                                let reply = transfer.destination_reply.take()?;
                                transfer.record.state = DownloadState::Receiving;
                                transfer.record.revision += 1;
                                Some((
                                    reply,
                                    path,
                                    transfer.partition.profile(),
                                    transfer.on_started.take(),
                                ))
                            })
                    };
                    if let Some((reply, path, profile, on_started)) = start {
                        reply.finish(Some(path));
                        if let Some(started) = on_started {
                            started();
                        }
                        (self.notify)(profile);
                    }
                } else {
                    self.cancel(id, Some(DownloadError::Storage));
                }
            }
            Message::Finalized(id, result) => match result {
                Ok((path, identity)) => {
                    if let Some(transfer) = self.active.borrow_mut().get_mut(&id) {
                        transfer.record.filename = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "download".into());
                        transfer.record.destination = Some(path);
                        transfer.record.received = identity.bytes;
                        transfer.record.total = Some(identity.bytes);
                        transfer.record.identity = Some(identity);
                        transfer.record.staging = None;
                        transfer.record.staging_identity = None;
                    }
                    self.terminal(id, DownloadState::Completed, None);
                }
                Err(error) => self.terminal(id, DownloadState::Failed, Some(error)),
            },
            #[cfg(target_os = "macos")]
            Message::Cancelled(id) => self.terminal(id, DownloadState::Cancelled, None),
            Message::Ui(token, reply) => self.ui_reply(token, reply),
            Message::Verified(token, path, result) => self.verified(token, path, result),
            Message::DirectorySelected(request, mut preferences, result) => {
                if self.stopping.get()
                    || self.retired.borrow().contains(&request.partition.profile())
                {
                    return;
                }
                match result {
                    Ok((path, identity)) => {
                        preferences.directory = Some(path);
                        preferences.directory_identity = Some(identity);
                        self.finish_directory_selection(request, preferences);
                    }
                    Err(error) => request.done.finish(DownloadResponse::Error { error }),
                }
            }
        }
    }
}

fn terminal_outcome(
    state: DownloadState,
    recorded: Option<DownloadError>,
    native: Option<DownloadError>,
) -> (DownloadState, Option<DownloadError>) {
    let error = recorded.or(native);
    let state = if state == DownloadState::Cancelled && error.is_some() {
        DownloadState::Failed
    } else {
        state
    };
    (state, error)
}

#[cfg(any(target_os = "windows", test))]
fn latch_native_error(recorded: &mut Option<DownloadError>, error: DownloadError) {
    recorded.get_or_insert(error);
}

#[cfg(any(target_os = "windows", test))]
fn retry_source_alive(permit: &EventPermit) -> bool {
    permit.active_token().is_some()
}

fn native_terminal_transition(
    state: DownloadState,
    cancelling: bool,
    finished: bool,
) -> Option<DownloadState> {
    if state.terminal() || state == DownloadState::Finalizing {
        return None;
    }
    Some(if cancelling {
        DownloadState::Cancelled
    } else if finished && state == DownloadState::Receiving {
        DownloadState::Finalizing
    } else {
        DownloadState::Failed
    })
}

#[cfg(test)]
mod state_tests {
    use super::*;
    #[test]
    fn cancellation_wins_before_publication_and_late_native_events_cannot_rewrite_finalization() {
        assert_eq!(
            native_terminal_transition(DownloadState::Receiving, true, true),
            Some(DownloadState::Cancelled)
        );
        assert_eq!(
            native_terminal_transition(DownloadState::Receiving, false, true),
            Some(DownloadState::Finalizing)
        );
        assert_eq!(
            native_terminal_transition(DownloadState::Pending, false, true),
            Some(DownloadState::Failed)
        );
        for state in [
            DownloadState::Finalizing,
            DownloadState::Completed,
            DownloadState::Cancelled,
            DownloadState::Failed,
            DownloadState::Interrupted,
        ] {
            for cancelled in [false, true] {
                for finished in [false, true] {
                    assert_eq!(native_terminal_transition(state, cancelled, finished), None);
                }
            }
        }
    }
    #[test]
    fn abandoned_destination_decisions_cancel_once() {
        let replies = Rc::new(Cell::new(0));
        let count = replies.clone();
        drop(DestinationReply::new(move |path| {
            assert!(path.is_none());
            count.set(count.get() + 1);
        }));
        assert_eq!(replies.get(), 1);
        let count = replies.clone();
        DestinationReply::new(move |path| {
            assert!(path.is_some());
            count.set(count.get() + 1);
        })
        .finish(Some(PathBuf::from("selected")));
        assert_eq!(replies.get(), 2);
    }

    #[test]
    fn paused_transfers_remain_cancellable_and_cannot_publish_without_resuming() {
        assert!(!DownloadState::Paused.terminal());
        assert_eq!(
            native_terminal_transition(DownloadState::Paused, true, false),
            Some(DownloadState::Cancelled)
        );
        assert_eq!(
            native_terminal_transition(DownloadState::Paused, false, true),
            Some(DownloadState::Failed)
        );
    }

    #[test]
    fn cancelling_an_interrupted_native_operation_cannot_replace_its_original_failure() {
        assert_eq!(
            terminal_outcome(
                DownloadState::Cancelled,
                Some(DownloadError::Permission),
                None
            ),
            (DownloadState::Failed, Some(DownloadError::Permission))
        );
        assert_eq!(
            terminal_outcome(
                DownloadState::Failed,
                Some(DownloadError::Certificate),
                Some(DownloadError::Cancelled)
            ),
            (DownloadState::Failed, Some(DownloadError::Certificate))
        );
        assert_eq!(
            terminal_outcome(DownloadState::Cancelled, None, None),
            (DownloadState::Cancelled, None)
        );
    }

    #[test]
    fn native_folder_consent_can_outlast_admission_and_restores_a_short_persistence_deadline() {
        let started = Instant::now();
        let after_consent = started + Duration::from_secs(45);
        assert!(DecisionDeadline::new(DecisionPhase::Admission, started).expired(after_consent));
        let preparation = DecisionDeadline::new(DecisionPhase::PreparingDestination, started);
        assert!(!preparation.expired(after_consent));
        let persisted = preparation
            .prepared(DownloadState::Pending, false, false, after_consent)
            .unwrap();
        assert_eq!(persisted.phase, DecisionPhase::PersistingDestination);
        assert!(!persisted.expired(after_consent + Duration::from_secs(29)));
        assert!(persisted.expired(after_consent + Duration::from_secs(31)));
    }

    #[test]
    fn late_destination_success_cannot_revive_cancelled_failed_or_expired_work() {
        let started = Instant::now();
        let preparation = DecisionDeadline::new(DecisionPhase::PreparingDestination, started);
        let after_consent = started + Duration::from_secs(45);
        assert!(preparation
            .prepared(DownloadState::Cancelling, true, false, after_consent)
            .is_none());
        assert!(preparation
            .prepared(DownloadState::Failed, false, true, after_consent)
            .is_none());
        assert!(preparation
            .prepared(DownloadState::Cancelled, false, true, after_consent)
            .is_none());
        assert!(preparation
            .prepared(
                DownloadState::Pending,
                false,
                false,
                started + Duration::from_secs(24 * 60 * 60 + 1)
            )
            .is_none());
    }

    #[test]
    fn reentrant_cancellation_latches_the_first_native_failure() {
        let mut error = None;
        latch_native_error(&mut error, DownloadError::Integrity);
        latch_native_error(&mut error, DownloadError::Cancelled);
        assert_eq!(
            terminal_outcome(DownloadState::Cancelled, error, None),
            (DownloadState::Failed, Some(DownloadError::Integrity))
        );
    }

    #[test]
    fn resumable_native_interruption_has_a_five_minute_bound() {
        let now = Instant::now();
        let deadline = DecisionDeadline::new(DecisionPhase::ResumableInterruption, now);
        assert!(!deadline.expired(now + Duration::from_secs(299)));
        assert!(deadline.expired(now + Duration::from_secs(301)));
    }

    #[test]
    fn closing_the_source_view_revokes_native_retry_even_when_receiving_can_finish() {
        let token = Arc::new(AtomicBool::new(true));
        let permit = EventPermit::bound(&token);
        assert!(retry_source_alive(&permit));
        assert!(permit.retire_for_download());
        assert!(!retry_source_alive(&permit));
        // The admitted receiving operation may still complete independently;
        // retirement prevents a new paused/retry lifetime from retaining it.
        assert_eq!(
            native_terminal_transition(DownloadState::Receiving, false, true),
            Some(DownloadState::Finalizing)
        );
    }
}
