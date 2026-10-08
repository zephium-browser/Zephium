//! Storage actor over the per-profile SQLite hub. `rusqlite` is blocking, so
//! one dedicated thread owns every connection and serializes access. Session
//! saves are coalesced (latest wins) so navigation bursts cost one write, not
//! one per event; visits and loads are immediate. Loads and shutdown flush
//! pending state first.

mod activity;
mod agent_audit;
#[cfg(feature = "work-execution")]
mod agent_work;
mod work_document;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use zephium_agentic::{AgentAuditCompletion, AgentAuditDelivery};
use zephium_core::blocker::{BlockerConfig, BlockerConfigRevision, BlockerSitePreferences};
use zephium_core::downloads::{DownloadStoreCall, DownloadStoreReply};
use zephium_core::ids::ProfileId;
use zephium_core::item::sanitize_page_title;
use zephium_core::navigation;
use zephium_core::permissions::{PagePermissionCatalogRevision, PagePermissionPatch};
use zephium_core::ports::store::{
    BlockerConfigLoadOutcome, BlockerConfigUpdateOutcome, BlockerSiteLoadOutcome,
    BlockerSiteUpdateOutcome, HistoryHit, HistoryVisit, PagePermissionCatalogLoadOutcome,
    PagePermissionCatalogMutationOutcome, ProfileDeletionAuthorizeOutcome,
    ProfileDeletionFinalizeOutcome, ProfileDeletionLoad, SessionLoad, Store, StoreShutdownOutcome,
    UserscriptCatalogLoadOutcome, UserscriptCatalogMutationOutcome, MAX_FAVICON_BATCH_ORIGINS,
};
use zephium_core::profiles::ProfileKind;
use zephium_core::session::{
    PersistedKind, SessionState, MAX_RECENTLY_CLOSED_TABS, MAX_SESSION_ITEMS,
    MAX_SESSION_NAME_CHARS, MAX_SESSION_PROFILES, MAX_SESSION_SPACES, MAX_SPLIT_DEPTH,
};
use zephium_core::split::Pane;
use zephium_core::userscripts::{UserscriptCatalogMutation, UserscriptCatalogRevision};

use crate::hub::{
    self, Hub, MAX_HISTORY_QUERY_BYTES, MAX_HISTORY_RESULTS, MAX_SETTING_KEY_BYTES,
    MAX_SETTING_VALUE_BYTES,
};
#[cfg(test)]
use crate::migrations;

use activity::{ActivityWrite, ActivityWritePermit, PendingActivityWrites};
use agent_audit::AgentAuditDeliveryPermit;
#[cfg(test)]
use agent_audit::MAX_PENDING_AGENT_AUDIT_DELIVERIES;

const DEBOUNCE: Duration = Duration::from_millis(400);
const MAX_PENDING_AGE: Duration = Duration::from_secs(2);
const SAVE_RETRY_INITIAL: Duration = Duration::from_secs(1);
const SAVE_RETRY_MAX: Duration = Duration::from_secs(30);
const MAX_PENDING_VISITS: usize = 2048;
const MAX_PENDING_SETTINGS: usize = hub::MAX_APP_SETTINGS as usize;
const DEFAULT_FLUSH_TIMEOUT: Duration = Duration::from_secs(8);
const STORE_RPC_TIMEOUT: Duration = Duration::from_secs(2);
const IMPORT_RPC_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PENDING_USERSCRIPT_MUTATIONS: usize = 4;
const MAX_PENDING_USERSCRIPT_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PENDING_PAGE_PERMISSION_MUTATIONS: usize = 16;

type PendingVisits = HashMap<(ProfileId, String), String>;
type BlockerConfigUpdateDone = Box<dyn FnOnce(BlockerConfigUpdateOutcome) + Send>;
type BlockerConfigLoadDone = Box<dyn FnOnce(BlockerConfigLoadOutcome) + Send>;
type BlockerSiteLoadDone = Box<dyn FnOnce(BlockerSiteLoadOutcome) + Send>;
type BlockerSiteUpdateDone = Box<dyn FnOnce(BlockerSiteUpdateOutcome) + Send>;
type UserscriptCatalogLoadDone = Box<dyn FnOnce(UserscriptCatalogLoadOutcome) + Send>;
type UserscriptCatalogMutationDone = Box<dyn FnOnce(UserscriptCatalogMutationOutcome) + Send>;
type PagePermissionCatalogLoadDone = Box<dyn FnOnce(PagePermissionCatalogLoadOutcome) + Send>;
type PagePermissionCatalogMutationDone =
    Box<dyn FnOnce(PagePermissionCatalogMutationOutcome) + Send>;

#[derive(Default)]
struct UserscriptMutationAdmission {
    count: usize,
    source_bytes: usize,
}

struct UserscriptMutationPermit {
    admission: Arc<Mutex<UserscriptMutationAdmission>>,
    source_bytes: usize,
}

impl UserscriptMutationPermit {
    fn acquire(
        admission: &Arc<Mutex<UserscriptMutationAdmission>>,
        source_bytes: usize,
    ) -> Option<Self> {
        let mut state = admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next_count = state.count.checked_add(1)?;
        let next_bytes = state.source_bytes.checked_add(source_bytes)?;
        if next_count > MAX_PENDING_USERSCRIPT_MUTATIONS
            || next_bytes > MAX_PENDING_USERSCRIPT_SOURCE_BYTES
        {
            return None;
        }
        state.count = next_count;
        state.source_bytes = next_bytes;
        Some(Self {
            admission: admission.clone(),
            source_bytes,
        })
    }
}

impl Drop for UserscriptMutationPermit {
    fn drop(&mut self) {
        let mut state = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(count) = state.count.checked_sub(1) else {
            // An impossible accounting violation must close admission rather
            // than reset it and accidentally admit an unbounded queue.
            state.count = usize::MAX;
            state.source_bytes = usize::MAX;
            return;
        };
        let Some(source_bytes) = state.source_bytes.checked_sub(self.source_bytes) else {
            state.count = usize::MAX;
            state.source_bytes = usize::MAX;
            return;
        };
        state.count = count;
        state.source_bytes = source_bytes;
    }
}

#[derive(Default)]
struct PagePermissionMutationAdmission {
    count: usize,
}

struct PagePermissionMutationPermit {
    admission: Arc<Mutex<PagePermissionMutationAdmission>>,
}

impl PagePermissionMutationPermit {
    fn acquire(admission: &Arc<Mutex<PagePermissionMutationAdmission>>) -> Option<Self> {
        let mut state = admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let next = state.count.checked_add(1)?;
        if next > MAX_PENDING_PAGE_PERMISSION_MUTATIONS {
            return None;
        }
        state.count = next;
        Some(Self {
            admission: admission.clone(),
        })
    }
}

impl Drop for PagePermissionMutationPermit {
    fn drop(&mut self) {
        let mut state = self
            .admission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(count) = state.count.checked_sub(1) else {
            // Close admission permanently on impossible accounting drift.
            state.count = usize::MAX;
            return;
        };
        state.count = count;
    }
}

#[derive(Default)]
struct PendingSettings {
    pending: HashMap<String, String>,
    in_flight: HashSet<String>,
    /// Exact union of durable keys and newly accepted keys. Keeping this next
    /// to the mailboxes makes cardinality admission atomic with enqueueing;
    /// actor queue pressure can never turn a definite acceptance into a later
    /// quota rejection.
    known_keys: HashSet<String>,
}

impl PendingSettings {
    fn with_known_keys(known_keys: HashSet<String>) -> Self {
        Self {
            known_keys,
            ..Self::default()
        }
    }

    fn contains_key(&self, key: &str) -> bool {
        self.known_keys.contains(key)
    }

    fn unique_keys(&self) -> usize {
        self.known_keys.len()
    }
}

struct PendingSession {
    state: SessionState,
    first: Instant,
    latest: Instant,
    failures: u32,
    retry_at: Option<Instant>,
}

impl PendingSession {
    fn new(state: SessionState, now: Instant) -> Self {
        Self {
            state,
            first: now,
            latest: now,
            failures: 0,
            retry_at: None,
        }
    }

    fn deadline(&self) -> Instant {
        self.retry_at
            .unwrap_or_else(|| (self.latest + DEBOUNCE).min(self.first + MAX_PENDING_AGE))
    }

    fn due(&self, now: Instant) -> bool {
        now >= self.deadline()
    }

    fn failed(&mut self, now: Instant) {
        let delay = retry_delay(self.failures);
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now + delay);
    }
}

#[derive(Default)]
struct WriteRetry {
    failures: u32,
    retry_at: Option<Instant>,
}

impl WriteRetry {
    fn failed(&mut self, now: Instant) {
        self.retry_at = Some(now + retry_delay(self.failures));
        self.failures = self.failures.saturating_add(1);
    }

    fn clear(&mut self) {
        self.failures = 0;
        self.retry_at = None;
    }
}

fn retry_delay(failures: u32) -> Duration {
    SAVE_RETRY_INITIAL
        .saturating_mul(1_u32 << failures.min(5))
        .min(SAVE_RETRY_MAX)
}

struct ResourcePermit(Arc<AtomicUsize>);
impl Drop for ResourcePermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

enum Cmd {
    WorkDocument(
        ProfileId,
        Box<zephium_core::work::port::WorkRequest>,
        work_document::Permit,
        zephium_core::work::port::WorkCompletion,
    ),
    ResourceCall(
        ProfileId,
        zephium_core::resources::ResourceCall,
        zephium_core::resources::ResourceDone,
        ResourcePermit,
    ),
    ImportMedia(
        ProfileId,
        zephium_core::resources::MediaImport,
        zephium_core::resources::ResourceDone,
        ResourcePermit,
    ),
    LegacyNotes(ProfileId, zephium_core::ports::store::LegacyNotesDone),
    RetireLegacyNotes(ProfileId, Vec<String>, Box<dyn FnOnce(bool) + Send>),
    #[cfg(feature = "work-execution")]
    AgentWork(
        zephium_agentic::AgentWorkJournalRequest,
        agent_work::WorkPermit,
        zephium_agentic::AgentWorkJournalCompletion,
    ),
    #[cfg(feature = "work-execution")]
    AgentWorkArtifact(
        zephium_agentic::AgentWorkArtifactRequest,
        agent_work::WorkPermit,
        zephium_agentic::AgentWorkArtifactCompletion,
    ),
    SaveWake,
    VisitWake,
    SettingWake,
    Load(Sender<SessionLoad>),
    /// Sets an unrestorable session aside and loads the restarted one.
    SetAsideSession(Sender<Option<(SessionLoad, Option<std::path::PathBuf>)>>),
    UpdateProfileBlockerConfig(
        ProfileId,
        BlockerConfigRevision,
        BlockerConfig,
        BlockerConfigUpdateDone,
    ),
    LoadProfileBlockerConfig(ProfileId, BlockerConfigLoadDone),
    LoadProfileBlockerSites(ProfileId, BlockerSiteLoadDone),
    Activity(ActivityWrite, ActivityWritePermit),
    TimeReport(
        ProfileId,
        zephium_core::time::TimeQuery,
        Box<dyn FnOnce(Option<zephium_core::time::TimeReport>) + Send>,
    ),
    FocusDays(
        i64,
        u32,
        Box<dyn FnOnce(Option<Vec<zephium_core::time::FocusDay>>) + Send>,
    ),
    LoadBlockerStatistics(
        ProfileId,
        Box<dyn FnOnce(Option<zephium_core::blocker::BlockerStatistics>) + Send>,
    ),
    SaveBlockerStatistics(
        ProfileId,
        zephium_core::blocker::BlockerStatistics,
        Box<dyn FnOnce(bool) + Send>,
    ),
    UpdateProfileBlockerSites(
        ProfileId,
        u64,
        Arc<BlockerSitePreferences>,
        BlockerSiteUpdateDone,
    ),
    LoadUserscriptCatalog(ProfileId, UserscriptCatalogLoadDone),
    MutateUserscriptCatalog(
        ProfileId,
        UserscriptCatalogRevision,
        UserscriptCatalogMutation,
        UserscriptMutationPermit,
        UserscriptCatalogMutationDone,
    ),
    LoadPagePermissionCatalog(ProfileId, PagePermissionCatalogLoadDone),
    MutatePagePermissionCatalog(
        ProfileId,
        PagePermissionCatalogRevision,
        PagePermissionPatch,
        PagePermissionMutationPermit,
        PagePermissionCatalogMutationDone,
    ),
    DownloadRecoveryProfiles(Box<dyn FnOnce(DownloadStoreReply) + Send>),
    DownloadCall(
        ProfileId,
        DownloadStoreCall,
        Box<dyn FnOnce(DownloadStoreReply) + Send>,
    ),
    GetSetting(String, Sender<Option<String>>),
    SearchHistory(ProfileId, String, u32, Sender<Vec<HistoryHit>>),
    RecordSearch(ProfileId, String, String),
    // Two adjacent Option<i64> bounds mean different things; name them.
    HistoryPage {
        profile: ProfileId,
        query: String,
        since: Option<i64>,
        before: Option<i64>,
        limit: u32,
        reply: Sender<Vec<HistoryVisit>>,
    },
    ForgetHistoryUrls(ProfileId, Vec<String>, Sender<u32>),
    ImportHistory(
        ProfileId,
        Vec<zephium_core::ports::store::ImportedVisit>,
        Sender<Option<u32>>,
    ),
    ImportFavicons(ProfileId, Vec<(String, Vec<u8>)>, Sender<Option<u32>>),
    Bookmarks(
        ProfileId,
        zephium_core::bookmarks::BookmarkRequest,
        Sender<zephium_core::bookmarks::BookmarkReply>,
    ),
    ClearHistory(ProfileId, Option<i64>, Sender<Option<u32>>),
    AmendVisitTitle(ProfileId, String, String),
    FaviconAge(ProfileId, String, Sender<Option<i64>>),
    FaviconRasterWithAge(ProfileId, String, Sender<Option<(Vec<u8>, i64)>>),
    SaveFavicon(ProfileId, String, Option<String>, Vec<u8>),
    FaviconBytes(ProfileId, String, Sender<Option<(Option<String>, Vec<u8>)>>),
    FaviconRasters(ProfileId, Vec<String>, Sender<Vec<(String, Vec<u8>)>>),
    PendingProfileDeletions(Sender<ProfileDeletionLoad>),
    AuthorizeProfileDeletion(
        ProfileId,
        SessionState,
        Sender<ProfileDeletionAuthorizeOutcome>,
    ),
    FinalizeProfileDeletion(ProfileId, Sender<ProfileDeletionFinalizeOutcome>),
    AppendAgentAudit(
        AgentAuditDelivery,
        AgentAuditDeliveryPermit,
        AgentAuditCompletion,
    ),
    Flush(Sender<bool>),
    Shutdown(Sender<bool>),
}

struct ActorLifecycle {
    join: Option<JoinHandle<()>>,
    exited: Mutex<Receiver<()>>,
    terminal_admitted: bool,
}

pub struct SqliteStore {
    work_document_admission: OnceLock<Arc<AtomicUsize>>,
    resource_admission: Arc<AtomicUsize>,
    #[cfg(feature = "work-execution")]
    work_admission: OnceLock<Arc<AtomicUsize>>,
    tx: SyncSender<Cmd>,
    latest_session: Arc<Mutex<Option<SessionState>>>,
    pending_visits: Arc<Mutex<PendingVisits>>,
    pending_settings: Arc<Mutex<PendingSettings>>,
    userscript_mutation_admission: Arc<Mutex<UserscriptMutationAdmission>>,
    page_permission_mutation_admission: Arc<Mutex<PagePermissionMutationAdmission>>,
    activity_admission: Arc<Mutex<activity::ActivityAdmission>>,
    agent_audit_delivery_admission: OnceLock<Arc<AtomicUsize>>,
    lifecycle: RwLock<ActorLifecycle>,
    shutdown_clean: AtomicBool,
}

impl SqliteStore {
    fn admits_writes(&self) -> bool {
        let lifecycle = self.lifecycle.read().unwrap_or_else(|p| p.into_inner());
        !lifecycle.terminal_admitted && !self.shutdown_clean.load(Ordering::Acquire)
    }

    fn queue_activity(&self, write: ActivityWrite) -> bool {
        // Keep acceptance ordered with terminal shutdown without ever waiting
        // for SQLite or a shutdown writer on the caller/UI thread.
        let lifecycle = match self.lifecycle.try_read() {
            Ok(lifecycle) => lifecycle,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        let Some(permit) = ActivityWritePermit::acquire(&self.activity_admission, &write) else {
            return false;
        };
        self.tx.try_send(Cmd::Activity(write, permit)).is_ok()
    }

    /// `dir` is the app data directory; the hub lays out `meta.sqlite` plus
    /// one `profile-<ulid>.sqlite` per profile inside it.
    pub fn open(dir: impl AsRef<Path>) -> rusqlite::Result<Self> {
        let mut hub = Hub::open(dir.as_ref().to_path_buf())?;
        // Fail before the shell/UI starts if even a compatibility snapshot
        // cannot be read. Startup must never turn storage failure into a new,
        // empty authoritative session.
        if let Err(error) = hub.load_authoritative() {
            if hub.recovery_reason().is_none() {
                return Err(error);
            }
        }
        Self::spawn(hub)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        Self::spawn(Hub::in_memory()?)
    }

    /// Installs protected Work routing before startup tombstone reconciliation.
    /// Ordinary application/profile databases retain their existing location.
    #[cfg(all(windows, feature = "work-execution"))]
    pub fn open_with_windows_work_storage(
        dir: impl AsRef<Path>,
        storage: crate::WindowsWorkStorage,
    ) -> rusqlite::Result<Self> {
        let mut hub = Hub::open_with_windows_work_storage(dir.as_ref().to_path_buf(), storage)?;
        if let Err(error) = hub.load_authoritative() {
            if hub.recovery_reason().is_none() {
                return Err(error);
            }
        }
        Self::spawn(hub)
    }

    fn spawn(hub: Hub) -> rusqlite::Result<Self> {
        let setting_keys = hub.app_setting_keys()?;
        // Session snapshots use a latest-value mailbox below. Bound every
        // remaining request too, so a compromised privileged UI cannot retain
        // unlimited settings/search/favicon commands in this actor.
        let (tx, rx) = mpsc::sync_channel::<Cmd>(256);
        let latest_session = Arc::new(Mutex::new(None));
        let pending_visits = Arc::new(Mutex::new(PendingVisits::new()));
        let pending_settings = Arc::new(Mutex::new(PendingSettings::with_known_keys(setting_keys)));
        let actor_latest_session = latest_session.clone();
        let actor_pending_visits = pending_visits.clone();
        let actor_pending_settings = pending_settings.clone();
        let userscript_mutation_admission =
            Arc::new(Mutex::new(UserscriptMutationAdmission::default()));
        let page_permission_mutation_admission =
            Arc::new(Mutex::new(PagePermissionMutationAdmission::default()));
        let (actor_exited, actor_exit) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("zephium-store".into())
            .spawn(move || {
                // Send the exit proof only after `actor` has returned and its
                // Hub/SQLite connections have been dropped. Preserve panic
                // visibility for JoinHandle while still unblocking the
                // bounded shutdown waiter.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    actor(
                        hub,
                        rx,
                        actor_latest_session,
                        actor_pending_visits,
                        actor_pending_settings,
                    )
                }));
                let _ = actor_exited.send(());
                if let Err(payload) = result {
                    std::panic::resume_unwind(payload);
                }
            })
            .map_err(|error| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!("cannot start the storage actor: {error}")),
                )
            })?;
        Ok(Self {
            resource_admission: Arc::new(AtomicUsize::new(0)),
            tx,
            work_document_admission: OnceLock::new(),
            #[cfg(feature = "work-execution")]
            work_admission: OnceLock::new(),
            latest_session,
            pending_visits,
            pending_settings,
            userscript_mutation_admission,
            page_permission_mutation_admission,
            activity_admission: Arc::new(Mutex::new(activity::ActivityAdmission::default())),
            agent_audit_delivery_admission: OnceLock::new(),
            lifecycle: RwLock::new(ActorLifecycle {
                join: Some(join),
                exited: Mutex::new(actor_exit),
                terminal_admitted: false,
            }),
            shutdown_clean: AtomicBool::new(false),
        })
    }

    /// Waits for the latest queued session snapshot to commit, but never past
    /// the store's bounded default shutdown budget.
    pub fn flush(&self) -> bool {
        self.flush_until(Instant::now() + DEFAULT_FLUSH_TIMEOUT)
    }

    /// Deadline-aware durability barrier. Admission to the bounded actor queue
    /// and waiting for SQLite completion share the same caller-owned budget.
    pub fn flush_until(&self, deadline: Instant) -> bool {
        let (tx, rx) = mpsc::channel();
        let mut command = Cmd::Flush(tx);
        loop {
            match self.tx.try_send(command) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
                Err(mpsc::TrySendError::Full(returned)) => {
                    command = returned;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return false;
                    }
                    thread::sleep(remaining.min(Duration::from_millis(1)));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        !remaining.is_zero() && rx.recv_timeout(remaining).unwrap_or(false)
    }

    /// Executes the terminal actor protocol under the caller's original
    /// deadline. A negative actor reply means durability failed before the
    /// actor transferred terminal ownership and is therefore retryable. Once
    /// the command is admitted without such a reply, any uncertainty is
    /// terminal: the actor may already have released its database handles.
    pub fn shutdown_until(&self, deadline: Instant) -> StoreShutdownOutcome {
        if self.shutdown_clean.load(Ordering::Acquire) {
            return StoreShutdownOutcome::Clean;
        }

        let mut lifecycle = self
            .lifecycle
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.shutdown_clean.load(Ordering::Acquire) {
            return StoreShutdownOutcome::Clean;
        }

        if !lifecycle.terminal_admitted {
            let (reply, result) = mpsc::channel();
            let mut command = Cmd::Shutdown(reply);
            loop {
                match self.tx.try_send(command) {
                    Ok(()) => {
                        lifecycle.terminal_admitted = true;
                        break;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        return StoreShutdownOutcome::Unclean;
                    }
                    Err(mpsc::TrySendError::Full(returned)) => {
                        command = returned;
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            return StoreShutdownOutcome::RetryableFailure;
                        }
                        thread::sleep(remaining.min(Duration::from_millis(1)));
                    }
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return StoreShutdownOutcome::Unclean;
            }
            match result.recv_timeout(remaining) {
                Ok(true) => {}
                Ok(false) => {
                    // The actor stays live after a definite failed flush.
                    lifecycle.terminal_admitted = false;
                    return StoreShutdownOutcome::RetryableFailure;
                }
                Err(_) => return StoreShutdownOutcome::Unclean,
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero()
            || lifecycle
                .exited
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv_timeout(remaining)
                .is_err()
        {
            return StoreShutdownOutcome::Unclean;
        }
        let Some(join) = lifecycle.join.take() else {
            return StoreShutdownOutcome::Unclean;
        };
        // The exit proof is sent after actor resources are dropped. Wait for
        // the OS thread itself to reach its terminal state before calling the
        // otherwise-unbounded JoinHandle::join.
        while !join.is_finished() && Instant::now() < deadline {
            thread::yield_now();
        }
        if !join.is_finished() || join.join().is_err() {
            return StoreShutdownOutcome::Unclean;
        }
        self.shutdown_clean.store(true, Ordering::Release);
        StoreShutdownOutcome::Clean
    }
}

impl Store for SqliteStore {
    fn work_document(
        &self,
        profile: ProfileId,
        request: zephium_core::work::port::WorkRequest,
        completion: zephium_core::work::port::WorkCompletion,
    ) -> Result<(), zephium_core::work::WorkError> {
        self.dispatch_work_document(profile, request, completion)
    }

    fn resource_call(
        &self,
        profile: ProfileId,
        call: zephium_core::resources::ResourceCall,
        done: zephium_core::resources::ResourceDone,
    ) {
        use zephium_core::resources::{ResourceError, ResourceResponse};
        if !call.validate() {
            done(ResourceResponse::Error {
                error: ResourceError::Invalid,
            });
            return;
        }
        if self
            .resource_admission
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 4).then_some(n + 1)
            })
            .is_err()
        {
            done(ResourceResponse::Error {
                error: ResourceError::Capacity,
            });
            return;
        }
        if let Err(error) = self.tx.try_send(Cmd::ResourceCall(
            profile,
            call,
            done,
            ResourcePermit(self.resource_admission.clone()),
        )) {
            let command = match error {
                mpsc::TrySendError::Full(c) | mpsc::TrySendError::Disconnected(c) => c,
            };
            if let Cmd::ResourceCall(_, _, done, _) = command {
                done(ResourceResponse::Error {
                    error: ResourceError::Unavailable,
                });
            }
        }
    }

    fn import_media(
        &self,
        profile: ProfileId,
        import: zephium_core::resources::MediaImport,
        done: zephium_core::resources::ResourceDone,
    ) {
        use zephium_core::resources::{ResourceError, ResourceResponse};
        if self
            .resource_admission
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 4).then_some(n + 1)
            })
            .is_err()
        {
            done(ResourceResponse::Error {
                error: ResourceError::Capacity,
            });
            return;
        }
        if let Err(error) = self.tx.try_send(Cmd::ImportMedia(
            profile,
            import,
            done,
            ResourcePermit(self.resource_admission.clone()),
        )) {
            let command = match error {
                mpsc::TrySendError::Full(c) | mpsc::TrySendError::Disconnected(c) => c,
            };
            if let Cmd::ImportMedia(_, _, done, _) = command {
                done(ResourceResponse::Error {
                    error: ResourceError::Unavailable,
                });
            }
        }
    }

    fn legacy_notes(&self, profile: ProfileId, done: zephium_core::ports::store::LegacyNotesDone) {
        if let Err(
            mpsc::TrySendError::Full(Cmd::LegacyNotes(_, done))
            | mpsc::TrySendError::Disconnected(Cmd::LegacyNotes(_, done)),
        ) = self.tx.try_send(Cmd::LegacyNotes(profile, done))
        {
            done(None);
        }
    }

    fn retire_legacy_notes(
        &self,
        profile: ProfileId,
        ids: Vec<String>,
        done: Box<dyn FnOnce(bool) + Send>,
    ) {
        if let Err(
            mpsc::TrySendError::Full(Cmd::RetireLegacyNotes(_, _, done))
            | mpsc::TrySendError::Disconnected(Cmd::RetireLegacyNotes(_, _, done)),
        ) = self.tx.try_send(Cmd::RetireLegacyNotes(profile, ids, done))
        {
            done(false);
        }
    }

    fn save_session(&self, session: SessionState) {
        if !admissible_session(&session) {
            return;
        }
        let mut latest = self
            .latest_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let needs_wake = latest.is_none();
        *latest = Some(session);
        drop(latest);
        // A full queue already guarantees the actor is awake. It checks this
        // mailbox before every queued command, so no blocking or lost wakeup.
        if needs_wake {
            let _ = self.tx.try_send(Cmd::SaveWake);
        }
    }

    fn flush(&self) -> bool {
        SqliteStore::flush(self)
    }

    fn flush_until(&self, deadline: Instant) -> bool {
        SqliteStore::flush_until(self, deadline)
    }

    fn shutdown_until(&self, deadline: Instant) -> StoreShutdownOutcome {
        SqliteStore::shutdown_until(self, deadline)
    }

    fn load_session(&self) -> SessionLoad {
        let (tx, rx) = mpsc::channel();
        if self.tx.try_send(Cmd::Load(tx)).is_err() {
            return SessionLoad::Failed;
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT)
            .unwrap_or(SessionLoad::Failed)
    }

    fn set_aside_session(&self) -> Option<(SessionLoad, Option<std::path::PathBuf>)> {
        let (tx, rx) = mpsc::channel();
        self.tx.try_send(Cmd::SetAsideSession(tx)).ok()?;
        rx.recv_timeout(STORE_RPC_TIMEOUT).ok().flatten()
    }

    fn update_profile_blocker_config(
        &self,
        profile: ProfileId,
        expected: BlockerConfigRevision,
        next: BlockerConfig,
        done: BlockerConfigUpdateDone,
    ) -> bool {
        // Terminal admission transfers ownership of the actor and can make a
        // later queued callback unreachable. Serialize this command with that
        // transition so `true` always guarantees exactly one completion.
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::UpdateProfileBlockerConfig(
                profile, expected, next, done,
            ))
            .is_ok()
    }

    fn load_profile_blocker_config(&self, profile: ProfileId, done: BlockerConfigLoadDone) -> bool {
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::LoadProfileBlockerConfig(profile, done))
            .is_ok()
    }

    fn load_blocker_statistics(
        &self,
        profile: ProfileId,
        done: Box<dyn FnOnce(Option<zephium_core::blocker::BlockerStatistics>) + Send>,
    ) -> bool {
        let lifecycle = self.lifecycle.read().unwrap_or_else(|p| p.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::LoadBlockerStatistics(profile, done))
            .is_ok()
    }
    fn save_blocker_statistics(
        &self,
        profile: ProfileId,
        statistics: zephium_core::blocker::BlockerStatistics,
        done: Box<dyn FnOnce(bool) + Send>,
    ) -> bool {
        let lifecycle = self.lifecycle.read().unwrap_or_else(|p| p.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::SaveBlockerStatistics(profile, statistics, done))
            .is_ok()
    }
    fn record_time(
        &self,
        profile: ProfileId,
        tallies: Vec<zephium_core::time::HourTally>,
        keep_from_hour: i64,
    ) -> bool {
        self.queue_activity(ActivityWrite::RecordTime(
            profile,
            hub::TimeBatchId::generate(),
            tallies,
            keep_from_hour,
        ))
    }
    fn time_report(
        &self,
        profile: ProfileId,
        query: zephium_core::time::TimeQuery,
        done: Box<dyn FnOnce(Option<zephium_core::time::TimeReport>) + Send>,
    ) -> bool {
        self.admits_writes()
            && self
                .tx
                .try_send(Cmd::TimeReport(profile, query, done))
                .is_ok()
    }
    fn clear_time(&self, profile: ProfileId, since_hour: Option<i64>) -> bool {
        self.queue_activity(ActivityWrite::ClearTime(profile, since_hour))
    }
    fn record_focus(&self, record: zephium_core::time::FocusRecord, day: i64) -> bool {
        self.queue_activity(ActivityWrite::RecordFocus(record, day))
    }
    fn focus_days(
        &self,
        from_day: i64,
        days: u32,
        done: Box<dyn FnOnce(Option<Vec<zephium_core::time::FocusDay>>) + Send>,
    ) -> bool {
        self.admits_writes()
            && self
                .tx
                .try_send(Cmd::FocusDays(from_day, days, done))
                .is_ok()
    }
    fn load_profile_blocker_sites(&self, profile: ProfileId, done: BlockerSiteLoadDone) -> bool {
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::LoadProfileBlockerSites(profile, done))
            .is_ok()
    }

    fn update_profile_blocker_sites(
        &self,
        profile: ProfileId,
        expected: u64,
        next: Arc<BlockerSitePreferences>,
        done: BlockerSiteUpdateDone,
    ) -> bool {
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::UpdateProfileBlockerSites(
                profile, expected, next, done,
            ))
            .is_ok()
    }

    fn load_userscript_catalog(&self, profile: ProfileId, done: UserscriptCatalogLoadDone) -> bool {
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::LoadUserscriptCatalog(profile, done))
            .is_ok()
    }

    fn mutate_userscript_catalog(
        &self,
        profile: ProfileId,
        expected: UserscriptCatalogRevision,
        mutation: UserscriptCatalogMutation,
        done: UserscriptCatalogMutationDone,
    ) -> bool {
        if !mutation.source_envelope_is_valid() {
            return false;
        }
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        let Some(permit) = UserscriptMutationPermit::acquire(
            &self.userscript_mutation_admission,
            mutation.source_bytes(),
        ) else {
            return false;
        };
        self.tx
            .try_send(Cmd::MutateUserscriptCatalog(
                profile, expected, mutation, permit, done,
            ))
            .is_ok()
    }

    fn load_page_permission_catalog(
        &self,
        profile: ProfileId,
        done: PagePermissionCatalogLoadDone,
    ) -> bool {
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        self.tx
            .try_send(Cmd::LoadPagePermissionCatalog(profile, done))
            .is_ok()
    }

    fn mutate_page_permission_catalog(
        &self,
        profile: ProfileId,
        expected: PagePermissionCatalogRevision,
        patch: PagePermissionPatch,
        done: PagePermissionCatalogMutationDone,
    ) -> bool {
        let lifecycle = self
            .lifecycle
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.terminal_admitted || self.shutdown_clean.load(Ordering::Acquire) {
            return false;
        }
        let Some(permit) =
            PagePermissionMutationPermit::acquire(&self.page_permission_mutation_admission)
        else {
            return false;
        };
        self.tx
            .try_send(Cmd::MutatePagePermissionCatalog(
                profile, expected, patch, permit, done,
            ))
            .is_ok()
    }

    fn record_visit(&self, profile: ProfileId, url: String, title: String) {
        if !navigation::is_allowed_str(&url) {
            return;
        }
        let title = sanitize_page_title(&title);
        let mut visits = self
            .pending_visits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (profile, url);
        let needs_wake = visits.is_empty();
        if visits.contains_key(&key) || visits.len() < MAX_PENDING_VISITS {
            visits.insert(key, title);
        }
        drop(visits);
        // A failed nonblocking wake means the bounded command queue already
        // contains work. The actor drains this mailbox before every command,
        // so visits remain bounded without ever stalling the shell thread.
        if needs_wake {
            let _ = self.tx.try_send(Cmd::VisitWake);
        }
    }

    fn app_setting(&self, key: &str) -> Option<String> {
        if key.is_empty() || key.len() > MAX_SETTING_KEY_BYTES {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        self.tx.try_send(Cmd::GetSetting(key.into(), tx)).ok()?;
        rx.recv_timeout(STORE_RPC_TIMEOUT).ok().flatten()
    }

    fn set_app_setting(&self, key: String, value: String) -> bool {
        if key.is_empty()
            || key.len() > MAX_SETTING_KEY_BYTES
            || value.len() > MAX_SETTING_VALUE_BYTES
        {
            return false;
        }
        let mut settings = self
            .pending_settings
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let was_known = settings.contains_key(&key);
        if !was_known && settings.unique_keys() >= MAX_PENDING_SETTINGS {
            return false;
        }
        settings.known_keys.insert(key.clone());
        let previous = settings.pending.insert(key.clone(), value);
        match self.tx.try_send(Cmd::SettingWake) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                if !was_known {
                    settings.known_keys.remove(&key);
                }
                match previous {
                    Some(previous) => {
                        settings.pending.insert(key, previous);
                    }
                    None => {
                        settings.pending.remove(&key);
                    }
                }
                false
            }
        }
    }

    fn favicon_age(&self, profile: ProfileId, origin: &str) -> Option<i64> {
        if !hub::valid_favicon_origin(origin) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Cmd::FaviconAge(profile, origin.into(), tx))
            .ok()?;
        rx.recv_timeout(STORE_RPC_TIMEOUT).ok().flatten()
    }

    fn save_favicon(
        &self,
        profile: ProfileId,
        origin: String,
        _content_type: Option<String>,
        bytes: Vec<u8>,
    ) {
        let Some(content_type) = hub::validated_favicon(&origin, &bytes).map(str::to_owned) else {
            return;
        };
        let _ = self
            .tx
            .try_send(Cmd::SaveFavicon(profile, origin, Some(content_type), bytes));
    }

    fn favicon_bytes(&self, profile: ProfileId, origin: &str) -> Option<(Option<String>, Vec<u8>)> {
        if !hub::valid_favicon_origin(origin) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Cmd::FaviconBytes(profile, origin.into(), tx))
            .ok()?;
        rx.recv_timeout(STORE_RPC_TIMEOUT).ok().flatten()
    }

    fn favicon_raster_with_age(&self, profile: ProfileId, origin: &str) -> Option<(Vec<u8>, i64)> {
        if !hub::valid_favicon_origin(origin) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Cmd::FaviconRasterWithAge(profile, origin.into(), tx))
            .ok()?;
        rx.recv_timeout(STORE_RPC_TIMEOUT).ok().flatten()
    }

    fn favicon_rasters(&self, profile: ProfileId, origins: &[String]) -> Vec<(String, Vec<u8>)> {
        if origins.len() > MAX_FAVICON_BATCH_ORIGINS
            || origins
                .iter()
                .any(|origin| !hub::valid_favicon_origin(origin))
        {
            return Vec::new();
        }
        let mut unique = HashSet::with_capacity(origins.len());
        if origins.iter().any(|origin| !unique.insert(origin.as_str())) {
            return Vec::new();
        }
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .try_send(Cmd::FaviconRasters(profile, origins.to_vec(), tx))
            .is_err()
        {
            return Vec::new();
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT).unwrap_or_default()
    }

    fn record_search(&self, profile: ProfileId, query: String, url: String) -> bool {
        if query.trim().is_empty()
            || query.len() > 512
            || !zephium_core::navigation::is_allowed_str(&url)
        {
            return false;
        }
        self.tx
            .try_send(Cmd::RecordSearch(profile, query, url))
            .is_ok()
    }

    fn search_history(&self, profile: ProfileId, query: &str, limit: u32) -> Vec<HistoryHit> {
        if query.len() > MAX_HISTORY_QUERY_BYTES || limit == 0 {
            return Vec::new();
        }
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .try_send(Cmd::SearchHistory(
                profile,
                query.into(),
                limit.min(MAX_HISTORY_RESULTS),
                tx,
            ))
            .is_err()
        {
            return Vec::new();
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT).unwrap_or_default()
    }

    fn download_recovery_profiles(&self, done: Box<dyn FnOnce(DownloadStoreReply) + Send>) -> bool {
        self.tx
            .try_send(Cmd::DownloadRecoveryProfiles(done))
            .is_ok()
    }

    fn download_call(
        &self,
        profile: ProfileId,
        call: DownloadStoreCall,
        done: Box<dyn FnOnce(DownloadStoreReply) + Send>,
    ) -> bool {
        if let DownloadStoreCall::Save(record) = &call {
            if !record.validate() {
                return false;
            }
        }
        self.tx
            .try_send(Cmd::DownloadCall(profile, call, done))
            .is_ok()
    }

    fn history_page(
        &self,
        profile: ProfileId,
        query: &str,
        since: Option<i64>,
        before: Option<i64>,
        limit: u32,
    ) -> Vec<HistoryVisit> {
        if limit == 0 || query.len() > MAX_HISTORY_QUERY_BYTES {
            return Vec::new();
        }
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .try_send(Cmd::HistoryPage {
                profile,
                query: query.to_owned(),
                since,
                before,
                limit: limit.min(hub::MAX_HISTORY_PAGE),
                reply: tx,
            })
            .is_err()
        {
            return Vec::new();
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT).unwrap_or_default()
    }

    fn forget_history_urls(&self, profile: ProfileId, urls: &[String]) -> u32 {
        if urls.is_empty() || urls.len() > hub::MAX_HISTORY_FORGET_URLS {
            return 0;
        }
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .try_send(Cmd::ForgetHistoryUrls(profile, urls.to_vec(), tx))
            .is_err()
        {
            return 0;
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT).unwrap_or_default()
    }

    fn import_history(
        &self,
        profile: ProfileId,
        visits: Vec<zephium_core::ports::store::ImportedVisit>,
    ) -> Option<u32> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Cmd::ImportHistory(profile, visits, tx))
            .ok()?;
        // An import writes many rows in one transaction; give it longer than
        // an interactive read.
        rx.recv_timeout(IMPORT_RPC_TIMEOUT).ok().flatten()
    }

    fn import_favicons(&self, profile: ProfileId, icons: Vec<(String, Vec<u8>)>) -> Option<u32> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .try_send(Cmd::ImportFavicons(profile, icons, tx))
            .ok()?;
        rx.recv_timeout(IMPORT_RPC_TIMEOUT).ok().flatten()
    }

    fn bookmarks(
        &self,
        profile: ProfileId,
        request: zephium_core::bookmarks::BookmarkRequest,
    ) -> zephium_core::bookmarks::BookmarkReply {
        use zephium_core::bookmarks::{BookmarkFailure, BookmarkReply, BookmarkRequest};
        let timeout = if matches!(request, BookmarkRequest::Import { .. }) {
            IMPORT_RPC_TIMEOUT
        } else {
            STORE_RPC_TIMEOUT
        };
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .try_send(Cmd::Bookmarks(profile, request, tx))
            .is_err()
        {
            return BookmarkReply::Failed(BookmarkFailure::Unavailable);
        }
        rx.recv_timeout(timeout)
            .unwrap_or(BookmarkReply::Failed(BookmarkFailure::Unavailable))
    }

    fn clear_history(&self, profile: ProfileId, since: Option<i64>) -> u32 {
        self.clear_history_checked(profile, since)
            .unwrap_or_default()
    }

    fn clear_history_checked(&self, profile: ProfileId, since: Option<i64>) -> Option<u32> {
        let (tx, rx) = mpsc::channel();
        if self
            .tx
            .try_send(Cmd::ClearHistory(profile, since, tx))
            .is_err()
        {
            return None;
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT).ok().flatten()
    }

    fn amend_visit_title(&self, profile: ProfileId, url: String, title: String) -> bool {
        if !zephium_core::navigation::is_allowed_str(&url) || title.len() > hub::MAX_TITLE_BYTES {
            return false;
        }
        self.tx
            .try_send(Cmd::AmendVisitTitle(profile, url, title))
            .is_ok()
    }

    fn pending_profile_deletions(&self) -> ProfileDeletionLoad {
        let (tx, rx) = mpsc::channel();
        if self.tx.try_send(Cmd::PendingProfileDeletions(tx)).is_err() {
            return ProfileDeletionLoad::Failed;
        }
        rx.recv_timeout(STORE_RPC_TIMEOUT)
            .unwrap_or(ProfileDeletionLoad::Failed)
    }

    fn authorize_profile_deletion(
        &self,
        profile: ProfileId,
        filtered_session: SessionState,
        deadline: Instant,
    ) -> ProfileDeletionAuthorizeOutcome {
        if !admissible_session(&filtered_session)
            || filtered_session
                .profiles
                .iter()
                .any(|candidate| candidate.id == profile)
            || zephium_core::session::canonicalize(filtered_session.clone()) != filtered_session
        {
            return ProfileDeletionAuthorizeOutcome::InvalidSession;
        }
        if Instant::now() >= deadline {
            return ProfileDeletionAuthorizeOutcome::NotAdmitted;
        }

        let (tx, rx) = mpsc::channel();
        let mut command = Cmd::AuthorizeProfileDeletion(profile, filtered_session, tx);
        loop {
            match self.tx.try_send(command) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return ProfileDeletionAuthorizeOutcome::NotAdmitted;
                }
                Err(mpsc::TrySendError::Full(returned)) => {
                    command = returned;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return ProfileDeletionAuthorizeOutcome::NotAdmitted;
                    }
                    thread::sleep(remaining.min(Duration::from_millis(1)));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ProfileDeletionAuthorizeOutcome::OutcomeUnknown;
        }
        rx.recv_timeout(remaining)
            .unwrap_or(ProfileDeletionAuthorizeOutcome::OutcomeUnknown)
    }

    fn finalize_profile_deletion(
        &self,
        profile: ProfileId,
        deadline: Instant,
    ) -> ProfileDeletionFinalizeOutcome {
        if Instant::now() >= deadline {
            return ProfileDeletionFinalizeOutcome::NotAdmitted;
        }
        let (tx, rx) = mpsc::channel();
        let mut command = Cmd::FinalizeProfileDeletion(profile, tx);
        loop {
            match self.tx.try_send(command) {
                Ok(()) => break,
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    return ProfileDeletionFinalizeOutcome::NotAdmitted;
                }
                Err(mpsc::TrySendError::Full(returned)) => {
                    command = returned;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return ProfileDeletionFinalizeOutcome::NotAdmitted;
                    }
                    thread::sleep(remaining.min(Duration::from_millis(1)));
                }
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return ProfileDeletionFinalizeOutcome::OutcomeUnknown;
        }
        rx.recv_timeout(remaining)
            .unwrap_or(ProfileDeletionFinalizeOutcome::OutcomeUnknown)
    }
}

fn admissible_session(session: &SessionState) -> bool {
    const MAX_NAME_BYTES: usize = MAX_SESSION_NAME_CHARS * 4;
    const MAX_URL_BYTES: usize = 8 * 1024;
    const MAX_TITLE_BYTES: usize = zephium_core::item::MAX_PAGE_TITLE_CHARS * 4;

    session.profiles.len() <= MAX_SESSION_PROFILES
        && session.spaces.len() <= MAX_SESSION_SPACES
        && session.items.len() <= MAX_SESSION_ITEMS
        && session.profiles.iter().all(|profile| {
            profile.kind != ProfileKind::Incognito && profile.name.len() <= MAX_NAME_BYTES
        })
        && session
            .spaces
            .iter()
            .all(|space| space.name.len() <= MAX_NAME_BYTES)
        && session.items.iter().all(|item| match &item.kind {
            PersistedKind::Folder { name } => name.len() <= MAX_NAME_BYTES,
            PersistedKind::Tab { url, title, zoom } => {
                url.len() <= MAX_URL_BYTES
                    && title.len() <= MAX_TITLE_BYTES
                    && zoom.is_finite()
                    && (0.3..=3.0).contains(zoom)
            }
            PersistedKind::BrowserTab { .. } => true,
        })
        && session.recently_closed.len() <= MAX_RECENTLY_CLOSED_TABS
        && session.recently_closed.iter().all(|entry| {
            entry.url.len() <= MAX_URL_BYTES
                && entry.title.len() <= MAX_TITLE_BYTES
                && entry.zoom.is_finite()
                && (0.3..=3.0).contains(&entry.zoom)
        })
        && session.splits.as_ref().is_none_or(admissible_split)
}

fn admissible_split(root: &Pane) -> bool {
    // This boundary accepts an in-process DTO, not only bounded JSON. Walk it
    // iteratively so a future privileged caller cannot feed an oversized
    // recursive tree into clone/canonicalization/serialization first.
    let mut stack = vec![(root, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((pane, depth)) = stack.pop() {
        nodes = nodes.saturating_add(1);
        if depth > MAX_SPLIT_DEPTH || nodes > MAX_SESSION_ITEMS.saturating_mul(2).saturating_sub(1)
        {
            return false;
        }
        match pane {
            Pane::Leaf(_) => {}
            Pane::Branch { ratio, a, b, .. } => {
                if !ratio.is_finite() || !(0.05..=0.95).contains(ratio) {
                    return false;
                }
                stack.push((b, depth + 1));
                stack.push((a, depth + 1));
            }
        }
    }
    true
}

fn actor(
    mut hub: Hub,
    rx: Receiver<Cmd>,
    latest_session: Arc<Mutex<Option<SessionState>>>,
    pending_visits: Arc<Mutex<PendingVisits>>,
    pending_settings: Arc<Mutex<PendingSettings>>,
) {
    let mut pending: Option<PendingSession> = None;
    let mut visit_retry = WriteRetry::default();
    let mut setting_retry = WriteRetry::default();
    let mut activity = PendingActivityWrites::default();
    loop {
        let deadline = pending
            .as_ref()
            .map(PendingSession::deadline)
            .into_iter()
            .chain(visit_retry.retry_at)
            .chain(setting_retry.retry_at)
            .chain(activity.deadline())
            .min();
        let cmd = if let Some(deadline) = deadline {
            let wait = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(cmd) => Some(cmd),
                Err(_) => break,
            }
        };
        if cmd.is_some() {
            absorb_latest_session(&latest_session, &mut pending);
            if !matches!(
                &cmd,
                Some(Cmd::Flush(_) | Cmd::Shutdown(_) | Cmd::PendingProfileDeletions(_))
            ) {
                let _ = flush_settings(&mut hub, &pending_settings, &mut setting_retry, false);
                let _ = flush_visits(
                    &mut hub,
                    &pending_visits,
                    &mut pending,
                    &mut visit_retry,
                    false,
                );
                let _ = activity.flush(&mut hub, &mut pending, false);
            }
        }
        match cmd {
            None => {
                if pending
                    .as_ref()
                    .is_some_and(|pending| pending.due(Instant::now()))
                {
                    let _ = flush(&mut hub, &mut pending);
                }
                let _ = flush_settings(&mut hub, &pending_settings, &mut setting_retry, false);
                let _ = flush_visits(
                    &mut hub,
                    &pending_visits,
                    &mut pending,
                    &mut visit_retry,
                    false,
                );
                let _ = activity.flush(&mut hub, &mut pending, false);
            }
            Some(Cmd::SaveWake) => {}
            Some(Cmd::VisitWake) => {}
            Some(Cmd::SettingWake) => {}
            Some(Cmd::Load(reply)) => {
                if flush(&mut hub, &mut pending) {
                    let _ = flush_visits(
                        &mut hub,
                        &pending_visits,
                        &mut pending,
                        &mut visit_retry,
                        false,
                    );
                }
                let loaded = session_load(&mut hub);
                let _ = reply.send(loaded);
            }
            Some(Cmd::SetAsideSession(reply)) => {
                let restarted = match hub.set_aside_recovery() {
                    Ok(file) => Some((session_load(&mut hub), file)),
                    Err(error) => {
                        eprintln!("store: setting the unrestorable session aside failed: {error}");
                        None
                    }
                };
                let _ = reply.send(restarted);
            }
            Some(Cmd::UpdateProfileBlockerConfig(profile, expected, next, done)) => {
                let outcome = match hub.update_profile_blocker_config(profile, expected, next) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!(
                            "store: profile {profile} blocker preference update failed: {error}"
                        );
                        BlockerConfigUpdateOutcome::Failed
                    }
                };
                done(outcome);
            }
            Some(Cmd::Activity(write, permit)) => {
                activity.push(write, permit);
                let _ = activity.flush(&mut hub, &mut pending, false);
            }
            Some(Cmd::TimeReport(profile, query, done)) => {
                if !activity.flush_profile(profile, &mut hub, &mut pending, false) {
                    done(None);
                    continue;
                }
                if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    done(None);
                    continue;
                }
                done(hub.time_report(profile, &query).ok());
            }
            Some(Cmd::FocusDays(from_day, days, done)) => {
                if !activity.flush_focus(&mut hub, &mut pending) {
                    done(None);
                    continue;
                }
                done(hub.focus_days(from_day, days).ok());
            }
            Some(Cmd::LoadBlockerStatistics(profile, done)) => {
                if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    done(None);
                    continue;
                }
                done(hub.load_blocker_statistics(profile).ok());
            }
            Some(Cmd::SaveBlockerStatistics(profile, statistics, done)) => {
                if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    done(false);
                    continue;
                }
                done(hub.save_blocker_statistics(profile, &statistics).is_ok());
            }
            Some(Cmd::LoadProfileBlockerSites(profile, done)) => {
                if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    done(BlockerSiteLoadOutcome::Failed);
                    continue;
                }
                done(
                    hub.profile_blocker_sites(profile)
                        .unwrap_or(BlockerSiteLoadOutcome::Failed),
                );
            }
            Some(Cmd::UpdateProfileBlockerSites(profile, expected, next, done)) => {
                if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    done(BlockerSiteUpdateOutcome::Failed);
                    continue;
                }
                done(
                    hub.update_profile_blocker_sites(profile, expected, next)
                        .unwrap_or(BlockerSiteUpdateOutcome::Failed),
                );
            }
            Some(Cmd::LoadProfileBlockerConfig(profile, done)) => {
                let outcome = match hub.profile_blocker_config(profile) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!(
                            "store: profile {profile} blocker preference reconciliation failed: {error}"
                        );
                        BlockerConfigLoadOutcome::Failed
                    }
                };
                done(outcome);
            }
            Some(Cmd::LoadUserscriptCatalog(profile, done)) => {
                let outcome = match hub.load_userscript_catalog(profile) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!(
                            "store: profile {profile} userscript catalog load failed: {error}"
                        );
                        UserscriptCatalogLoadOutcome::Failed
                    }
                };
                done(outcome);
            }
            Some(Cmd::MutateUserscriptCatalog(profile, expected, mutation, _permit, done)) => {
                let outcome = match hub.mutate_userscript_catalog(profile, expected, mutation) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!(
                            "store: profile {profile} userscript catalog mutation failed: {error}"
                        );
                        UserscriptCatalogMutationOutcome::Failed
                    }
                };
                done(outcome);
            }
            Some(Cmd::LoadPagePermissionCatalog(profile, done)) => {
                let outcome = match hub.load_page_permission_catalog(profile) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!(
                            "store: profile {profile} page-permission catalog load failed: {error}"
                        );
                        PagePermissionCatalogLoadOutcome::Failed
                    }
                };
                done(outcome);
            }
            Some(Cmd::MutatePagePermissionCatalog(profile, expected, patch, _permit, done)) => {
                let outcome = match hub.mutate_page_permission_catalog(profile, expected, patch) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        eprintln!(
                            "store: profile {profile} page-permission catalog mutation failed: {error}"
                        );
                        PagePermissionCatalogMutationOutcome::Failed
                    }
                };
                done(outcome);
            }
            Some(Cmd::GetSetting(key, reply)) => {
                let _ = reply.send(hub.app_setting(&key));
            }
            Some(Cmd::ResourceCall(profile, call, done, admission)) => {
                let response = if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    zephium_core::resources::ResourceResponse::Error {
                        error: zephium_core::resources::ResourceError::Unavailable,
                    }
                } else {
                    hub.resource_call(profile, call)
                };
                drop(admission);
                done(response);
            }
            Some(Cmd::ImportMedia(profile, import, done, admission)) => {
                let response = if !hub.knows(profile) && !flush(&mut hub, &mut pending) {
                    zephium_core::resources::ResourceResponse::Error {
                        error: zephium_core::resources::ResourceError::Unavailable,
                    }
                } else {
                    hub.import_media(profile, import)
                };
                drop(admission);
                done(response);
            }
            Some(Cmd::LegacyNotes(profile, done)) => {
                let known = hub.knows(profile) || flush(&mut hub, &mut pending);
                done(known.then(|| hub.legacy_notes(profile)).flatten());
            }
            Some(Cmd::RetireLegacyNotes(profile, ids, done)) => {
                done(hub.knows(profile) && hub.retire_legacy_notes(profile, &ids));
            }
            Some(Cmd::DownloadRecoveryProfiles(done)) => {
                done(hub.download_recovery_profiles());
            }
            Some(Cmd::DownloadCall(profile, call, done)) => {
                done(hub.download_call(profile, call));
            }
            Some(Cmd::RecordSearch(profile, query, url)) => {
                hub.record_search(profile, &query, &url);
            }
            Some(Cmd::SearchHistory(profile, query, limit, reply)) => {
                let mut hits = hub.search_queries(profile, &query);
                hits.extend(hub.search_history(profile, &query, limit));
                let mut seen = std::collections::HashSet::new();
                hits.retain(|hit| seen.insert(hit.url.clone()));
                hits.truncate(limit as usize);
                let _ = reply.send(hits);
            }
            Some(Cmd::HistoryPage {
                profile,
                query,
                since,
                before,
                limit,
                reply,
            }) => {
                let _ = reply.send(hub.history_page(profile, &query, since, before, limit));
            }
            Some(Cmd::ForgetHistoryUrls(profile, urls, reply)) => {
                let _ = reply.send(hub.forget_history_urls(profile, &urls));
            }
            Some(Cmd::ImportHistory(profile, visits, reply)) => {
                let _ = reply.send(hub.import_history(profile, &visits));
            }
            Some(Cmd::ImportFavicons(profile, icons, reply)) => {
                let _ = reply.send(hub.import_favicons(profile, &icons));
            }
            Some(Cmd::Bookmarks(profile, request, reply)) => {
                let _ = reply.send(hub.bookmarks(profile, request));
            }
            Some(Cmd::ClearHistory(profile, since, reply)) => {
                let session_durable = hub.knows(profile) || flush(&mut hub, &mut pending);
                let visits_durable = session_durable
                    && flush_profile_visits(&mut hub, &pending_visits, &mut visit_retry, profile);
                let activity_durable =
                    activity.flush_profile(profile, &mut hub, &mut pending, true);
                let result = if session_durable && visits_durable && activity_durable {
                    hub.clear_history_checked(profile, since)
                } else {
                    None
                };
                let _ = reply.send(result);
            }
            Some(Cmd::AmendVisitTitle(profile, url, title)) => {
                hub.amend_visit_title(profile, &url, &title);
            }
            Some(Cmd::FaviconAge(profile, origin, reply)) => {
                let _ = reply.send(hub.favicon_age(profile, &origin));
            }
            Some(Cmd::FaviconRasterWithAge(profile, origin, reply)) => {
                let _ = reply.send(hub.favicon_raster_with_age(profile, &origin));
            }
            Some(Cmd::SaveFavicon(profile, origin, content_type, bytes)) => {
                hub.save_favicon(profile, &origin, content_type.as_deref(), &bytes);
            }
            Some(Cmd::FaviconBytes(profile, origin, reply)) => {
                let _ = reply.send(hub.favicon_bytes(profile, &origin));
            }
            Some(Cmd::FaviconRasters(profile, origins, reply)) => {
                let rasters = origins
                    .into_iter()
                    .filter_map(|origin| {
                        hub.favicon_bytes(profile, &origin)
                            .map(|(_, bytes)| (origin, bytes))
                    })
                    .collect();
                let _ = reply.send(rasters);
            }
            Some(Cmd::PendingProfileDeletions(reply)) => {
                let result = match hub.reconcile_profile_deletion_journal() {
                    Ok(deletions) => {
                        activity.reconciled_authorizations();
                        for deletion in &deletions {
                            activity.forget_profile(deletion.profile);
                        }
                        if deletions.is_empty() {
                            if flush(&mut hub, &mut pending) {
                                ProfileDeletionLoad::Loaded(deletions)
                            } else {
                                ProfileDeletionLoad::Failed
                            }
                        } else {
                            // A durable authorization supersedes a retained
                            // pre-barrier snapshot that still contains any
                            // journaled profile. Never retry that stale snapshot
                            // before reporting the authoritative journal. Newer
                            // survivor-only state remains pending and is
                            // rescheduled by the application after tombstoning.
                            let conflicts = pending.as_ref().is_some_and(|save| {
                                deletions.iter().any(|deletion| {
                                    save.state
                                        .profiles
                                        .iter()
                                        .any(|profile| profile.id == deletion.profile)
                                })
                            });
                            if conflicts {
                                pending = None;
                            }
                            ProfileDeletionLoad::Loaded(deletions)
                        }
                    }
                    Err(error) => {
                        eprintln!("store: cannot reconcile profile deletion journal: {error}");
                        ProfileDeletionLoad::Failed
                    }
                };
                let _ = reply.send(result);
            }
            Some(Cmd::AuthorizeProfileDeletion(profile, session, reply)) => {
                let result = match hub.authorize_profile_deletion(profile, &session) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        // SQLite commit errors can be durability-ambiguous.
                        // Never tell the coordinator authorization failed and
                        // invite it to infer the opposite; reconciliation via
                        // the durable journal is required.
                        eprintln!("store: profile {profile} deletion authorization returned an ambiguous error: {error}");
                        match hub.reconcile_profile_deletion_journal() {
                            Ok(deletions)
                                if deletions.iter().any(|deletion| deletion.profile == profile) =>
                            {
                                ProfileDeletionAuthorizeOutcome::Authorized
                            }
                            Ok(_) => ProfileDeletionAuthorizeOutcome::Failed,
                            Err(reconcile_error) => {
                                eprintln!("store: profile {profile} deletion authorization could not be reconciled: {reconcile_error}");
                                ProfileDeletionAuthorizeOutcome::OutcomeUnknown
                            }
                        }
                    }
                };
                if matches!(
                    result,
                    ProfileDeletionAuthorizeOutcome::Authorized
                        | ProfileDeletionAuthorizeOutcome::AlreadyAuthorized
                ) {
                    // This synchronous snapshot supersedes every coalesced
                    // save observed before the authorization command.
                    pending = None;
                    activity.forget_profile(profile);
                } else if result == ProfileDeletionAuthorizeOutcome::OutcomeUnknown {
                    activity.quarantine_profile(profile);
                }
                let _ = reply.send(result);
            }
            Some(Cmd::FinalizeProfileDeletion(profile, reply)) => {
                let result = if flush(&mut hub, &mut pending) {
                    match hub.finalize_profile_deletion(profile) {
                        Ok(true) => ProfileDeletionFinalizeOutcome::Completed,
                        Ok(false) => ProfileDeletionFinalizeOutcome::NotAuthorized,
                        Err(error) => {
                            eprintln!("store: cannot finalize profile {profile} deletion: {error}");
                            ProfileDeletionFinalizeOutcome::Failed
                        }
                    }
                } else {
                    ProfileDeletionFinalizeOutcome::Failed
                };
                let _ = reply.send(result);
            }
            Some(Cmd::WorkDocument(profile, request, _permit, completion)) => {
                let result = hub.work_document(profile, *request);
                let _ =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| completion(result)));
            }
            #[cfg(feature = "work-execution")]
            Some(Cmd::AgentWork(request, _permit, completion)) => {
                agent_work::settle(&mut hub, request, completion);
            }
            #[cfg(feature = "work-execution")]
            Some(Cmd::AgentWorkArtifact(request, _permit, completion)) => {
                agent_work::settle_artifact(&mut hub, request, completion);
            }
            Some(Cmd::AppendAgentAudit(delivery, _permit, completion)) => {
                if let Some(message) =
                    agent_audit::append_and_settle(&mut hub, delivery, completion).diagnostic()
                {
                    // `message` can only be one of the two content-free static
                    // literals owned by the isolated agent-audit module.
                    eprintln!("{message}");
                }
            }
            Some(Cmd::Flush(ack)) => {
                let settings_durable =
                    flush_settings(&mut hub, &pending_settings, &mut setting_retry, true);
                let session_durable = flush(&mut hub, &mut pending);
                let activity_durable = activity.flush(&mut hub, &mut pending, true);
                let visits_durable = session_durable
                    && flush_visits(
                        &mut hub,
                        &pending_visits,
                        &mut pending,
                        &mut visit_retry,
                        true,
                    );
                let _ = ack.send(
                    settings_durable && session_durable && visits_durable && activity_durable,
                );
            }
            Some(Cmd::Shutdown(ack)) => {
                let settings_durable =
                    flush_settings(&mut hub, &pending_settings, &mut setting_retry, true);
                let session_durable = flush(&mut hub, &mut pending);
                let activity_durable = activity.flush(&mut hub, &mut pending, true) || {
                    let lost = activity.only_unrecoverable_records();
                    if lost {
                        eprintln!(
                            "store: shutting down with time or focus records that keep failing"
                        );
                    }
                    lost
                };
                let visits_durable = session_durable
                    && flush_visits(
                        &mut hub,
                        &pending_visits,
                        &mut pending,
                        &mut visit_retry,
                        true,
                    );
                let durable =
                    settings_durable && session_durable && visits_durable && activity_durable;
                let _ = ack.send(durable);
                if durable {
                    // Returning drops Hub and every SQLite connection before
                    // the wrapper thread publishes its exit proof.
                    return;
                }
            }
        }
        // Non-save traffic must not reset either deadline. Long-running reads
        // may overshoot it, so check again after every command as well.
        if pending
            .as_ref()
            .is_some_and(|pending| pending.due(Instant::now()))
            && flush(&mut hub, &mut pending)
        {
            let _ = flush_visits(
                &mut hub,
                &pending_visits,
                &mut pending,
                &mut visit_retry,
                false,
            );
        }
    }
    absorb_latest_session(&latest_session, &mut pending);
    // Never make the dropping/UI thread wait here. Normal shutdown already
    // used its caller-owned deadline barrier. On an unexpected sender drop,
    // this detached actor gets one best-effort terminal durability attempt.
    let _ = flush_settings(&mut hub, &pending_settings, &mut setting_retry, true);
    let _ = activity.flush(&mut hub, &mut pending, true);
    if flush(&mut hub, &mut pending) {
        let _ = flush_visits(
            &mut hub,
            &pending_visits,
            &mut pending,
            &mut visit_retry,
            true,
        );
    }
}

fn flush_settings(
    hub: &mut Hub,
    mailbox: &Mutex<PendingSettings>,
    retry: &mut WriteRetry,
    force: bool,
) -> bool {
    if !force
        && retry
            .retry_at
            .is_some_and(|retry_at| Instant::now() < retry_at)
    {
        return false;
    }
    let settings = {
        let mut mailbox = mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if mailbox.pending.is_empty() {
            retry.clear();
            return true;
        }
        let settings = std::mem::take(&mut mailbox.pending);
        mailbox.in_flight.extend(settings.keys().cloned());
        settings
    };

    let mut failed = PendingSettings::default();
    for (key, value) in settings {
        match hub.set_app_setting(&key, &value) {
            Ok(true) => {}
            Ok(false) => {
                // Admission and the actor's authoritative key registry should
                // make this unreachable. Treat external database divergence as
                // a failed durability barrier; never drop an accepted value or
                // acknowledge a clean shutdown.
                eprintln!(
                    "store: application-setting write was rejected after admission for key {key}"
                );
                failed.pending.insert(key, value);
            }
            Err(error) => {
                eprintln!("store: application-setting write failed: {error}");
                failed.pending.insert(key, value);
            }
        }
    }
    let had_failures = !failed.pending.is_empty();
    {
        let mut mailbox = mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Every key in this completed batch leaves the in-flight reservation.
        // If a concurrent caller admitted a newer value, it already occupies
        // `pending` and wins over a failed older write for the same key.
        mailbox.in_flight.clear();
        for (key, value) in failed.pending {
            mailbox.pending.entry(key).or_insert(value);
        }
    }
    if had_failures {
        retry.failed(Instant::now());
        false
    } else {
        retry.clear();
        true
    }
}

/// An intentional profile clear settles only its own pending visits. A failed
/// foreign visit remains in the bounded mailbox without holding this clear.
fn flush_profile_visits(
    hub: &mut Hub,
    mailbox: &Mutex<PendingVisits>,
    retry: &mut WriteRetry,
    profile: ProfileId,
) -> bool {
    let visits: PendingVisits = {
        let mut mailbox = mailbox.lock().unwrap_or_else(|p| p.into_inner());
        let (visits, kept) = std::mem::take(&mut *mailbox)
            .into_iter()
            .partition(|((owner, _), _)| *owner == profile);
        *mailbox = kept;
        visits
    };
    if visits.is_empty() {
        return true;
    }
    match hub.record_visits(
        visits
            .into_iter()
            .map(|((profile, url), title)| (profile, url, title)),
    ) {
        Ok(()) => true,
        Err(failed) => {
            requeue_visits(
                mailbox,
                failed
                    .into_iter()
                    .map(|(profile, url, title)| ((profile, url), title))
                    .collect(),
            );
            retry.failed(Instant::now());
            false
        }
    }
}

fn flush_visits(
    hub: &mut Hub,
    mailbox: &Mutex<PendingVisits>,
    pending_session: &mut Option<PendingSession>,
    retry: &mut WriteRetry,
    force: bool,
) -> bool {
    if !force
        && retry
            .retry_at
            .is_some_and(|retry_at| Instant::now() < retry_at)
    {
        return false;
    }
    // A failed first session save means Hub does not know the new profile yet.
    // Honour its backoff instead of hammering SQLite before every actor
    // command; the timeout path calls us again immediately after a retry.
    let registry_blocked = {
        let mailbox = mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        mailbox.keys().any(|(profile, _)| !hub.knows(*profile))
            && pending_session
                .as_ref()
                .and_then(|pending| pending.retry_at)
                .is_some_and(|retry_at| Instant::now() < retry_at)
    };
    if registry_blocked {
        if let Some(retry_at) = pending_session
            .as_ref()
            .and_then(|pending| pending.retry_at)
        {
            retry.retry_at = Some(retry_at);
        }
        return false;
    }
    let visits = {
        let mut mailbox = mailbox
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if mailbox.is_empty() {
            retry.clear();
            return true;
        }
        std::mem::take(&mut *mailbox)
    };
    // A first-run profile registry may still be in the session debounce
    // window. Publish it before attributing any visit to that profile.
    if visits.keys().any(|(profile, _)| !hub.knows(*profile)) && !flush(hub, pending_session) {
        requeue_visits(mailbox, visits);
        return false;
    }
    match hub.record_visits(
        visits
            .into_iter()
            .map(|((profile, url), title)| (profile, url, title)),
    ) {
        Ok(()) => {
            retry.clear();
            true
        }
        Err(failed) => {
            requeue_visits(
                mailbox,
                failed
                    .into_iter()
                    .map(|(profile, url, title)| ((profile, url), title))
                    .collect(),
            );
            retry.failed(Instant::now());
            false
        }
    }
}

fn requeue_visits(mailbox: &Mutex<PendingVisits>, visits: PendingVisits) {
    let mut mailbox = mailbox
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (key, title) in visits {
        // A concurrent newer visit for the same URL wins. Preserve as many of
        // the older unique visits as the fixed mailbox bound permits.
        if !mailbox.contains_key(&key) && mailbox.len() < MAX_PENDING_VISITS {
            mailbox.insert(key, title);
        }
    }
}

fn absorb_latest_session(
    latest: &Mutex<Option<SessionState>>,
    pending: &mut Option<PendingSession>,
) {
    let Some(state) = latest
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    else {
        return;
    };
    let now = Instant::now();
    match pending {
        Some(pending) => {
            pending.state = state;
            pending.latest = now;
        }
        None => *pending = Some(PendingSession::new(state, now)),
    }
}

fn flush(hub: &mut Hub, pending: &mut Option<PendingSession>) -> bool {
    let Some(mut save) = pending.take() else {
        return true;
    };
    match hub.save(&save.state) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("store: save failed: {e}");
            // Keep the latest snapshot retryable instead of acknowledging a
            // failed barrier and silently throwing the only in-memory copy
            // away. Exponential backoff bounds disk wakeups and log spam when
            // the failure is persistent; an explicit Flush still retries
            // immediately as a caller-owned durability barrier.
            save.failed(Instant::now());
            *pending = Some(save);
            false
        }
    }
}

/// The session the hub holds, as the shell receives it.
fn session_load(hub: &mut Hub) -> SessionLoad {
    match hub.load_authoritative() {
        Ok(Some(authoritative)) => {
            let profiles = hub.degraded_profile_ids();
            if profiles.is_empty() {
                SessionLoad::Loaded {
                    state: authoritative.state,
                    blocker_configs: authoritative.blocker_configs,
                }
            } else {
                SessionLoad::LoadedWithDegradedProfiles {
                    state: authoritative.state,
                    profiles,
                    blocker_configs: authoritative.blocker_configs,
                }
            }
        }
        Ok(None) => SessionLoad::Absent,
        Err(_) if hub.recovery_reason().is_some() => SessionLoad::RecoveryRequired {
            reason: hub
                .recovery_reason()
                .unwrap_or("authoritative session requires recovery")
                .to_owned(),
        },
        Err(error) => {
            eprintln!("store: session load failed: {error}");
            SessionLoad::Failed
        }
    }
}

#[cfg(test)]
mod tests;
