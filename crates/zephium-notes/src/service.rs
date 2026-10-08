//! The notes thread. It owns every open library, so file work never waits
//! behind the store and the store never waits behind a disk flush.
use std::collections::{HashMap, HashSet};
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
    Release {
        profile: ProfileId,
        done: Box<dyn FnOnce() + Send>,
    },
    Stop,
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
    thread: Option<std::thread::JoinHandle<()>>,
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
        let own = sender.clone();
        let thread = std::thread::Builder::new()
            .name("zephium-notes".into())
            .spawn(move || Worker::new(base, host, own).run(receiver))?;
        Ok(Self {
            sender,
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
    pub fn release(&self, profile: ProfileId, done: Box<dyn FnOnce() + Send>) {
        if let Err(error) = self.sender.send(Command::Release { profile, done }) {
            if let Command::Release { done, .. } = error.0 {
                done();
            }
        }
    }
}

impl zephium_core::ports::notes::Notes for NoteService {
    fn call(&self, profile: ProfileId, call: NoteCall, done: NoteDone) {
        NoteService::call(self, profile, call, done);
    }

    fn release(&self, profile: ProfileId, done: Box<dyn FnOnce() + Send>) {
        NoteService::release(self, profile, done);
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

    fn run(mut self, receiver: Receiver<Command>) {
        loop {
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
                Ok(Command::Release { profile, done }) => {
                    self.open.remove(&profile);
                    self.retired.insert(profile);
                    done();
                }
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
