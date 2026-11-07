//! The notes thread. It owns every open library, so file work never waits
//! behind the store and the store never waits behind a disk flush.
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{RecursiveMode, Watcher};
use zephium_core::ids::ProfileId;
use zephium_core::notes::{ChangedNote, NoteCall, NoteChanges, NoteDone, NoteError, NoteResponse};

use crate::legacy::{to_markdown, LegacyNote};
use crate::library::{Changes, Library};

const QUEUE: usize = 64;
/// Editors save in bursts (temporary file, rename, attribute update); one
/// scan after the burst settles is enough.
const SETTLE: Duration = Duration::from_millis(250);
const MAX_PENDING_PATHS: usize = 256;
const LEGACY_IMPORTED: &str = "legacy-imported";

/// What the notes thread needs from the application around it.
pub trait Host: Send + Sync + 'static {
    fn changed(&self, event: NoteChanges);
    /// Shows a file or folder in the system file manager.
    fn reveal(&self, path: &Path);
    /// Notes still kept in the profile database, or `None` when they cannot
    /// be read now and the import should be tried again later.
    fn legacy_notes(&self, profile: ProfileId) -> Option<Vec<LegacyNote>>;
    /// Removes imported notes from the profile database.
    fn retire_legacy_notes(&self, profile: ProfileId, ids: Vec<String>) -> bool;
}

enum Command {
    Call {
        profile: ProfileId,
        call: NoteCall,
        done: NoteDone,
    },
    Touched(ProfileId),
    Wake,
    Stop,
}

struct Release {
    profile: ProfileId,
    done: Box<dyn FnOnce() + Send>,
}

#[derive(Default)]
struct Releases {
    pending: VecDeque<Release>,
    closed: bool,
}

/// File-system events collected between two turns of the notes thread.
#[derive(Default)]
struct Pending {
    paths: Vec<PathBuf>,
    rescan: bool,
}

struct Open {
    library: Library,
    _watcher: Option<notify::RecommendedWatcher>,
    pending: Arc<Mutex<Pending>>,
    signalled: Arc<AtomicBool>,
    due: Option<Instant>,
}

pub struct NoteService {
    sender: SyncSender<Command>,
    releases: Arc<Mutex<Releases>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn complete_release(done: Box<dyn FnOnce() + Send>) {
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(done)).is_err() {
        tracing::warn!("notes release completion panicked");
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl NoteService {
    /// `base` is the application data directory.
    pub fn start(base: PathBuf, host: Arc<dyn Host>) -> std::io::Result<Self> {
        let (sender, receiver) = sync_channel(QUEUE);
        // Lifecycle requests have reserved bounded admission even while a
        // slow file operation has filled the ordinary call queue.
        let releases = Arc::new(Mutex::new(Releases::default()));
        let worker_releases = releases.clone();
        let own = sender.clone();
        let thread = std::thread::Builder::new()
            .name("zephium-notes".into())
            .spawn(move || {
                // Receiver disconnection can precede destruction of the
                // libraries and watchers. Catch outside the owning frame so
                // pending releases survive a panic and acknowledge only after
                // every worker resource has finished dropping.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Worker::new(base, host, own).run(receiver, &worker_releases);
                }));
                let pending = {
                    let mut releases = worker_releases.lock().unwrap_or_else(|e| e.into_inner());
                    releases.closed = true;
                    std::mem::take(&mut releases.pending)
                };
                for Release { done, .. } in pending {
                    // Completion failures are independent of worker cleanup.
                    complete_release(done);
                }
                if let Err(error) = result {
                    std::panic::resume_unwind(error);
                }
            })?;
        Ok(Self {
            sender,
            releases,
            thread: Some(thread),
        })
    }

    pub fn call(&self, profile: ProfileId, call: NoteCall, done: NoteDone) {
        if !call.validate() {
            return done(NoteResponse::Error {
                error: NoteError::Invalid,
            });
        }
        match self.sender.try_send(Command::Call {
            profile,
            call,
            done,
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(Command::Call { done, .. })) => done(NoteResponse::Error {
                error: NoteError::Capacity,
            }),
            Err(TrySendError::Disconnected(Command::Call { done, .. })) => {
                done(NoteResponse::Error {
                    error: NoteError::Unavailable,
                })
            }
            Err(_) => {}
        }
    }

    /// Closes a profile's notes for good in this process, before its data is
    /// erased. Later calls for it fail as unavailable.
    pub fn release(&self, profile: ProfileId, done: Box<dyn FnOnce() + Send>) -> bool {
        let mut releases = self.releases.lock().unwrap_or_else(|e| e.into_inner());
        if releases.closed {
            drop(releases);
            complete_release(done);
            return true;
        }
        if releases.pending.len() >= zephium_core::session::MAX_SESSION_PROFILES {
            return false;
        }
        releases.pending.push_back(Release { profile, done });
        drop(releases);
        // A full call queue already guarantees a wake. The worker drains
        // lifecycle requests before taking any more ordinary file work.
        let _ = self.sender.try_send(Command::Wake);
        true
    }
}

impl zephium_core::ports::notes::Notes for NoteService {
    fn call(&self, profile: ProfileId, call: NoteCall, done: NoteDone) {
        NoteService::call(self, profile, call, done);
    }

    fn release(&self, profile: ProfileId, done: Box<dyn FnOnce() + Send>) -> bool {
        NoteService::release(self, profile, done)
    }
}

impl Drop for NoteService {
    fn drop(&mut self) {
        let _ = self.sender.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Reports what changed, with each note's revision now on disk.
fn emit(host: &dyn Host, profile: ProfileId, library: &Library, changes: Changes) {
    if changes.is_empty() {
        return;
    }
    let notes = changes
        .ids
        .into_iter()
        .map(|id| ChangedNote {
            revision: library
                .index()
                .get(&id)
                .ok()
                .flatten()
                .map(|row| row.revision),
            id,
        })
        .collect();
    host.changed(NoteChanges {
        profile: profile.to_string(),
        notes,
        reset: changes.reset,
        links: changes.links,
    });
}

struct Worker {
    base: PathBuf,
    host: Arc<dyn Host>,
    sender: SyncSender<Command>,
    open: HashMap<ProfileId, Open>,
    retired: HashSet<ProfileId>,
}

impl Worker {
    fn new(base: PathBuf, host: Arc<dyn Host>, sender: SyncSender<Command>) -> Self {
        Self {
            base,
            host,
            sender,
            open: HashMap::new(),
            retired: HashSet::new(),
        }
    }

    fn run(mut self, receiver: Receiver<Command>, releases: &Mutex<Releases>) {
        loop {
            loop {
                let profile = releases
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pending
                    .front()
                    .map(|release| release.profile);
                let Some(profile) = profile else { break };
                // Keep ownership of the completion in the shared queue until
                // closing these resources succeeds, including their Drop work.
                self.open.remove(&profile);
                self.retired.insert(profile);
                let release = releases
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pending
                    .pop_front();
                if let Some(Release { done, .. }) = release {
                    complete_release(done);
                }
            }
            let due = self.open.values().filter_map(|open| open.due).min();
            let command = match due {
                Some(due) => receiver.recv_timeout(due.saturating_duration_since(Instant::now())),
                None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match command {
                Ok(Command::Call {
                    profile,
                    call,
                    done,
                }) => done(self.call(profile, call)),
                Ok(Command::Touched(profile)) => {
                    if let Some(open) = self.open.get_mut(&profile) {
                        open.signalled.store(false, Ordering::Release);
                        open.due.get_or_insert_with(|| Instant::now() + SETTLE);
                    }
                }
                Ok(Command::Wake) => {}
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => self.settle(),
            }
        }
    }

    /// Scans the libraries whose folders changed and whose events have settled.
    fn settle(&mut self) {
        let instant = Instant::now();
        let due: Vec<ProfileId> = self
            .open
            .iter()
            .filter(|(_, open)| open.due.is_some_and(|due| due <= instant))
            .map(|(profile, _)| *profile)
            .collect();
        for profile in due {
            let Some(open) = self.open.get_mut(&profile) else {
                continue;
            };
            open.due = None;
            let pending =
                std::mem::take(&mut *open.pending.lock().unwrap_or_else(|e| e.into_inner()));
            if !pending.rescan && !open.library.stale(&pending.paths) {
                continue;
            }
            match open.library.reconcile(now()) {
                Ok(changes) => emit(self.host.as_ref(), profile, &open.library, changes),
                Err(error) => tracing::warn!(?error, "notes folder scan failed"),
            }
        }
    }

    fn call(&mut self, profile: ProfileId, call: NoteCall) -> NoteResponse {
        if self.retired.contains(&profile) {
            return NoteResponse::Error {
                error: NoteError::Unavailable,
            };
        }
        if !self.open.contains_key(&profile) {
            match self.open_library(profile) {
                Some(open) => {
                    self.open.insert(profile, open);
                }
                None => {
                    return NoteResponse::Error {
                        error: NoteError::Unavailable,
                    }
                }
            }
        }
        let Some(open) = self.open.get_mut(&profile) else {
            return NoteResponse::Error {
                error: NoteError::Unavailable,
            };
        };
        if let NoteCall::Reveal { id } = &call {
            let path = match id {
                Some(id) => open.library.absolute(id),
                None => Some(open.library.root().to_path_buf()),
            };
            return match path {
                Some(path) => {
                    self.host.reveal(&path);
                    NoteResponse::Done
                }
                None => NoteResponse::Error {
                    error: NoteError::NotFound,
                },
            };
        }
        let (response, changes) = open.library.call(call, now());
        emit(self.host.as_ref(), profile, &open.library, changes);
        response
    }

    fn open_library(&mut self, profile: ProfileId) -> Option<Open> {
        let mut library = match Library::open(&self.base, &profile.to_string()) {
            Ok(library) => library,
            Err(error) => {
                tracing::warn!(?error, "notes folder unavailable");
                return None;
            }
        };
        let instant = now();
        self.import_legacy(profile, &mut library, instant);
        let mut changes = library.reconcile(instant).ok()?;
        if let Ok(expired) = library.expire_trash(instant) {
            changes.ids.extend(expired.ids);
            changes.reset |= expired.reset;
        }
        let pending = Arc::new(Mutex::new(Pending::default()));
        let signalled = Arc::new(AtomicBool::new(false));
        let watcher = self.watch(profile, library.root(), &pending, &signalled);
        emit(self.host.as_ref(), profile, &library, changes);
        Some(Open {
            library,
            _watcher: watcher,
            pending,
            signalled,
            due: None,
        })
    }

    /// Follows edits made by other apps. Without a watcher the folder is
    /// still reconciled whenever it is opened, only later.
    fn watch(
        &self,
        profile: ProfileId,
        root: &Path,
        pending: &Arc<Mutex<Pending>>,
        signalled: &Arc<AtomicBool>,
    ) -> Option<notify::RecommendedWatcher> {
        let sender = self.sender.clone();
        let pending = Arc::clone(pending);
        let signalled = Arc::clone(signalled);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                {
                    let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
                    match event {
                        Ok(event) if !event.need_rescan() => {
                            if pending.paths.len() + event.paths.len() > MAX_PENDING_PATHS {
                                pending.rescan = true;
                                pending.paths.clear();
                            } else if !pending.rescan {
                                pending.paths.extend(event.paths);
                            }
                        }
                        _ => {
                            pending.rescan = true;
                            pending.paths.clear();
                        }
                    }
                }
                // One wake-up per burst; the thread drains everything collected.
                if !signalled.swap(true, Ordering::AcqRel)
                    && sender.try_send(Command::Touched(profile)).is_err()
                {
                    signalled.store(false, Ordering::Release);
                }
            })
            .map_err(|error| tracing::warn!(?error, "notes watcher unavailable"))
            .ok()?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| tracing::warn!(?error, "notes folder not watched"))
            .ok()?;
        Some(watcher)
    }

    /// Moves notes still kept in the profile database into the folder, once.
    fn import_legacy(&self, profile: ProfileId, library: &mut Library, instant: i64) {
        if library
            .index()
            .setting(LEGACY_IMPORTED)
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        let Some(notes) = self.host.legacy_notes(profile) else {
            return;
        };
        let titles: HashMap<String, String> = notes
            .iter()
            .map(|note| (note.id.clone(), note.title.clone()))
            .collect();
        let mut imported = Vec::new();
        for note in &notes {
            let modified = UNIX_EPOCH + Duration::from_secs(note.updated_at.max(0) as u64);
            match library.import(
                &note.id,
                &to_markdown(note, &titles),
                note.pinned,
                note.trashed,
                modified,
                instant,
            ) {
                Ok(()) => imported.push(note.id.clone()),
                Err(error) => {
                    tracing::warn!(?error, "legacy note not imported");
                    return;
                }
            }
        }
        if imported.is_empty() || self.host.retire_legacy_notes(profile, imported) {
            let _ = library.index().set_setting(LEGACY_IMPORTED, "1");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::notes::NoteQuery;

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<NoteChanges>>,
        revealed: Mutex<Vec<PathBuf>>,
    }

    impl Host for Recorder {
        fn changed(&self, event: NoteChanges) {
            self.events.lock().unwrap().push(event);
        }
        fn reveal(&self, path: &Path) {
            self.revealed.lock().unwrap().push(path.to_path_buf());
        }
        fn legacy_notes(&self, _: ProfileId) -> Option<Vec<LegacyNote>> {
            Some(Vec::new())
        }
        fn retire_legacy_notes(&self, _: ProfileId, _: Vec<String>) -> bool {
            true
        }
    }

    fn call(notes: &NoteService, profile: ProfileId, call: NoteCall) -> NoteResponse {
        let (sender, receiver) = std::sync::mpsc::channel();
        notes.call(
            profile,
            call,
            Box::new(move |response| sender.send(response).unwrap()),
        );
        receiver.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    fn titles(notes: &NoteService, profile: ProfileId) -> Vec<String> {
        match call(
            notes,
            profile,
            NoteCall::List {
                query: NoteQuery {
                    search: String::new(),
                    trashed: false,
                    after: None,
                    limit: 50,
                },
            },
        ) {
            NoteResponse::Page { items, .. } => items.into_iter().map(|note| note.title).collect(),
            other => panic!("expected a page, got {other:?}"),
        }
    }

    struct GatedWorkerDrop {
        revealing: std::sync::mpsc::Sender<()>,
        fail: Mutex<std::sync::mpsc::Receiver<()>>,
        begun: std::sync::mpsc::Sender<()>,
        resume: Mutex<std::sync::mpsc::Receiver<()>>,
        marker: PathBuf,
        armed: AtomicBool,
    }
    impl Host for GatedWorkerDrop {
        fn changed(&self, _: NoteChanges) {}
        fn reveal(&self, _: &Path) {
            self.armed.store(true, Ordering::Release);
            let _ = self.revealing.send(());
            let _ = self
                .fail
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .recv();
            panic!("fixture worker failure");
        }
        fn legacy_notes(&self, _: ProfileId) -> Option<Vec<LegacyNote>> {
            Some(Vec::new())
        }
        fn retire_legacy_notes(&self, _: ProfileId, _: Vec<String>) -> bool {
            true
        }
    }
    impl Drop for GatedWorkerDrop {
        fn drop(&mut self) {
            if self.armed.load(Ordering::Acquire) {
                let _ = self.begun.send(());
                let _ = self
                    .resume
                    .get_mut()
                    .unwrap_or_else(|error| error.into_inner())
                    .recv();
                let _ = std::fs::write(&self.marker, "worker cleanup completed");
            }
        }
    }

    #[test]
    fn disconnected_release_waits_for_worker_destructors_before_acknowledging() {
        let base = tempfile::tempdir().unwrap();
        let marker = base.path().join("worker-cleanup");
        let (begun, dropping) = std::sync::mpsc::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let (revealing, revealed) = std::sync::mpsc::channel();
        let (fail, failure) = std::sync::mpsc::channel();
        let mut notes = NoteService::start(
            base.path().to_path_buf(),
            Arc::new(GatedWorkerDrop {
                revealing,
                fail: Mutex::new(failure),
                begun,
                resume: Mutex::new(gate),
                marker: marker.clone(),
                armed: AtomicBool::new(false),
            }),
        )
        .unwrap();
        let profile = ProfileId::from(3);
        assert!(matches!(
            call(
                &notes,
                profile,
                NoteCall::Create {
                    request_id: "request-000000000001".into(),
                    markdown: "# Before worker failure".into(),
                }
            ),
            NoteResponse::Applied { .. }
        ));
        notes.call(profile, NoteCall::Reveal { id: None }, Box::new(|_| {}));
        let started = revealed.recv_timeout(Duration::from_secs(5));
        let _ = fail.send(());
        let entered = dropping.recv_timeout(Duration::from_secs(5));
        let disconnected = matches!(
            notes.sender.try_send(Command::Wake),
            Err(TrySendError::Disconnected(_))
        );
        let (done, proof) = std::sync::mpsc::channel();
        let during_cleanup = notes.release(
            profile,
            Box::new(move || {
                let _ = done.send(());
            }),
        );
        let early_proof = proof.try_recv().is_ok();
        let marker_before = marker.exists();
        // Always release the fixture before asserting, even on a failed proof.
        let _ = resume.send(());
        assert!(notes.thread.take().unwrap().join().is_err());
        assert!(started.is_ok());
        assert!(entered.is_ok());
        assert!(
            disconnected,
            "receiver drops before the worker's library owners"
        );
        assert!(!early_proof && !marker_before);
        assert!(marker.exists());
        if during_cleanup {
            proof.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        let (done, proof) = std::sync::mpsc::channel();
        assert!(notes.release(
            profile,
            Box::new(move || {
                let _ = done.send(());
            })
        ));
        proof.recv_timeout(Duration::from_secs(5)).unwrap();
        let folder = base.path().join("notes").join(profile.to_string());
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(matches!(
            call(
                &notes,
                profile,
                NoteCall::Create {
                    request_id: "request-000000000002".into(),
                    markdown: "# Must stay erased".into(),
                }
            ),
            NoteResponse::Error {
                error: NoteError::Unavailable
            }
        ));
        assert!(!folder.exists());
    }

    #[test]
    fn accepted_releases_survive_worker_and_completion_panics_with_bounded_admission() {
        let base = tempfile::tempdir().unwrap();
        let marker = base.path().join("worker-cleanup");
        let (begun, dropping) = std::sync::mpsc::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let (revealing, revealed) = std::sync::mpsc::channel();
        let (fail, failure) = std::sync::mpsc::channel();
        let mut notes = NoteService::start(
            base.path().to_path_buf(),
            Arc::new(GatedWorkerDrop {
                revealing,
                fail: Mutex::new(failure),
                begun,
                resume: Mutex::new(gate),
                marker: marker.clone(),
                armed: AtomicBool::new(false),
            }),
        )
        .unwrap();
        let profile = ProfileId::from(3);
        assert!(matches!(
            call(
                &notes,
                profile,
                NoteCall::Create {
                    request_id: "request-000000000001".into(),
                    markdown: "# Before worker failure".into(),
                }
            ),
            NoteResponse::Applied { .. }
        ));
        notes.call(profile, NoteCall::Reveal { id: None }, Box::new(|_| {}));
        let started = revealed.recv_timeout(Duration::from_secs(5));
        let (done, proof) = std::sync::mpsc::channel();
        let mut admitted = 0;
        for index in 0..zephium_core::session::MAX_SESSION_PROFILES {
            let done = done.clone();
            let marker = marker.clone();
            admitted += usize::from(notes.release(
                profile,
                Box::new(move || {
                    let _ = done.send(marker.exists());
                    if index == 0 {
                        panic!("fixture completion failure");
                    }
                }),
            ));
        }
        drop(done);
        let (overflow, overflow_proof) = std::sync::mpsc::channel();
        let overflow_admitted = notes.release(
            profile,
            Box::new(move || {
                let _ = overflow.send(());
            }),
        );
        let before_panic = proof.try_recv().is_ok();
        let _ = fail.send(());
        let entered = dropping.recv_timeout(Duration::from_secs(5));
        let during_cleanup = proof.try_recv().is_ok();
        let marker_before = marker.exists();
        // Release both fixture gates before assertions or service shutdown.
        let _ = resume.send(());
        let panicked = notes.thread.take().unwrap().join().is_err();
        let completed: Vec<_> = proof.try_iter().collect();
        assert!(started.is_ok() && entered.is_ok() && panicked);
        assert_eq!(admitted, zephium_core::session::MAX_SESSION_PROFILES);
        assert!(!overflow_admitted && overflow_proof.try_recv().is_err());
        assert!(!before_panic && !during_cleanup && !marker_before);
        assert_eq!(
            completed,
            vec![true; admitted],
            "every accepted release completes only after worker cleanup"
        );
    }

    struct GatedLegacy {
        entered: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        resume: Mutex<std::sync::mpsc::Receiver<()>>,
    }
    impl Host for GatedLegacy {
        fn changed(&self, _: NoteChanges) {}
        fn reveal(&self, _: &Path) {}
        fn legacy_notes(&self, _: ProfileId) -> Option<Vec<LegacyNote>> {
            if let Some(entered) = self.entered.lock().unwrap().take() {
                entered.send(()).unwrap();
                self.resume.lock().unwrap().recv().unwrap();
            }
            Some(Vec::new())
        }
        fn retire_legacy_notes(&self, _: ProfileId, _: Vec<String>) -> bool {
            true
        }
    }

    #[test]
    fn release_has_reserved_admission_and_queued_or_late_calls_cannot_recreate_erased_notes() {
        let base = tempfile::tempdir().unwrap();
        let (entered, blocked) = std::sync::mpsc::channel();
        let (resume, gate) = std::sync::mpsc::channel();
        let notes = NoteService::start(
            base.path().to_path_buf(),
            Arc::new(GatedLegacy {
                entered: Mutex::new(Some(entered)),
                resume: Mutex::new(gate),
            }),
        )
        .unwrap();
        let blocker = ProfileId::from(1);
        let victim = ProfileId::from(2);
        notes.call(blocker, NoteCall::Reveal { id: None }, Box::new(|_| {}));
        blocked.recv_timeout(Duration::from_secs(5)).unwrap();
        let (created, response) = std::sync::mpsc::channel();
        notes.call(
            victim,
            NoteCall::Create {
                request_id: "request-000000000001".into(),
                markdown: "# Erased secret".into(),
            },
            Box::new(move |reply| {
                let _ = created.send(reply);
            }),
        );
        for _ in 1..QUEUE {
            notes.call(blocker, NoteCall::Reveal { id: None }, Box::new(|_| {}));
        }
        let (released, proof) = std::sync::mpsc::channel();
        assert!(notes.release(
            victim,
            Box::new(move || {
                released.send(()).unwrap();
            })
        ));
        assert!(
            proof.try_recv().is_err(),
            "an active writer must drain first"
        );
        resume.send(()).unwrap();
        proof.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            response.recv_timeout(Duration::from_secs(5)).unwrap(),
            NoteResponse::Error {
                error: NoteError::Unavailable
            }
        ));
        let folder = base.path().join("notes").join(victim.to_string());
        if folder.exists() {
            std::fs::remove_dir_all(&folder).unwrap();
        }
        assert!(matches!(
            call(
                &notes,
                victim,
                NoteCall::Create {
                    request_id: "request-000000000002".into(),
                    markdown: "# Late secret".into(),
                }
            ),
            NoteResponse::Error {
                error: NoteError::Unavailable
            }
        ));
        assert!(!folder.exists());
    }

    #[test]
    fn other_apps_edits_arrive_as_change_events() {
        let base = tempfile::tempdir().unwrap();
        let host = Arc::new(Recorder::default());
        let notes = NoteService::start(base.path().to_path_buf(), host.clone()).unwrap();
        let profile = ProfileId::generate();
        assert!(titles(&notes, profile).is_empty());
        let folder = std::fs::canonicalize(base.path())
            .unwrap()
            .join("notes")
            .join(profile.to_string())
            .join("Notes");
        std::fs::write(folder.join("From Finder.md"), "# From Finder").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while host.events.lock().unwrap().iter().all(|event| !event.reset) {
            assert!(Instant::now() < deadline, "no change event");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(titles(&notes, profile), ["From Finder"]);
        assert!(matches!(
            call(&notes, profile, NoteCall::Reveal { id: None }),
            NoteResponse::Done
        ));
        assert_eq!(host.revealed.lock().unwrap()[0], folder);
    }

    #[test]
    fn released_profiles_stay_closed() {
        let base = tempfile::tempdir().unwrap();
        let notes =
            NoteService::start(base.path().to_path_buf(), Arc::new(Recorder::default())).unwrap();
        let profile = ProfileId::generate();
        titles(&notes, profile);
        let (sender, receiver) = std::sync::mpsc::channel();
        notes.release(profile, Box::new(move || sender.send(()).unwrap()));
        receiver.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            call(
                &notes,
                profile,
                NoteCall::Get {
                    id: "01J9ZQ3V6Q4M8Y2K7T5R1N0B3A".into()
                }
            ),
            NoteResponse::Error {
                error: NoteError::Unavailable
            }
        ));
    }
}
