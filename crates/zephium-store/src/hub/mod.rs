//! Per-profile databases isolate history and favicon data. The shared meta
//! database deliberately owns the profile registry, application settings, and
//! the complete restorable non-private session, so a profile is not a
//! file-level isolation boundary. The hub owns every connection; a single
//! actor thread (`actor.rs`) serializes all access.

mod agent_audit;
#[cfg(feature = "work-execution")]
mod agent_work;
mod blocker;
mod bookmarks;
mod compatibility;
mod deletion;
mod downloads;
mod favicons;
mod filesystem;
mod history;
pub mod media;
mod page_permissions;
mod resources;
mod session;
mod settings;
mod time;
pub(crate) use time::{TimeBatchId, MAX_TIME_BATCH_RECEIPTS};
mod userscripts;
#[cfg(all(windows, feature = "work-execution"))]
mod windows_work_storage;
mod work_document;
#[cfg(all(windows, feature = "work-execution"))]
pub use windows_work_storage::WindowsWorkStorage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use zephium_core::blocker::ProfileBlockerConfig;
use zephium_core::ids::{ItemId, ProfileId, SpaceId};
use zephium_core::item::{Placement, SpaceSection};
use zephium_core::navigation;
use zephium_core::profiles::ProfileKind;
use zephium_core::session::{
    self as core_session, PersistedClosedTab, PersistedItem, PersistedKind, PersistedProfile,
    PersistedSpace, SessionState, MAX_RECENTLY_CLOSED_TABS, MAX_SESSION_ITEMS,
    MAX_SESSION_NAME_CHARS, MAX_SESSION_PROFILES, MAX_SESSION_SPACES,
};

use crate::{bounded_json, migrations};

pub(crate) use agent_audit::{AgentAuditAppendOutcome, MAX_DURABLE_AGENT_AUDIT_EVENTS};
#[cfg(all(
    test,
    feature = "work-execution",
    any(target_os = "macos", target_os = "linux", target_os = "windows")
))]
pub(crate) use agent_work::work_test_guard;
#[cfg(all(test, windows, feature = "work-execution"))]
pub(crate) use agent_work::work_test_storage;
use compatibility::remove_legacy_source;
pub(crate) use compatibility::LEGACY_IMPORT_STATE_KEY;
#[cfg(test)]
pub(crate) use compatibility::{LEGACY_HISTORY_MARKER, MAX_SPLIT_JSON_BYTES};
use deletion::deletion_process_generation;
pub(crate) use favicons::{valid_favicon_origin, validated_favicon};
pub(crate) use history::{
    MAX_HISTORY_BYTES, MAX_HISTORY_FORGET_URLS, MAX_HISTORY_PAGE, MAX_HISTORY_QUERY_BYTES,
    MAX_HISTORY_RESULTS,
};
pub(crate) use settings::{MAX_APP_SETTINGS, MAX_SETTING_KEY_BYTES, MAX_SETTING_VALUE_BYTES};

use filesystem::{
    configure, harden_registered_profile_files, open_database, open_meta_database,
    profile_artifacts_exist, regular_file_exists,
};

const SESSION_SCHEMA_VERSION: i64 = 1;
pub(crate) const MAX_SESSION_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_URL_BYTES: usize = 8 * 1024;
pub(crate) const MAX_TITLE_BYTES: usize = zephium_core::item::MAX_PAGE_TITLE_CHARS * 4;
pub(crate) const MAX_NAME_BYTES: usize = MAX_SESSION_NAME_CHARS * 4;

pub struct Hub {
    work_runtime_session: zephium_core::work::WorkRuntimeSessionId,
    work_runtime_epoch: std::time::Instant,
    #[cfg(feature = "work-execution")]
    work: Option<std::sync::Arc<agent_work::WorkOwnership>>,
    #[cfg(all(windows, feature = "work-execution"))]
    windows_work_storage: Option<WindowsWorkStorage>,
    #[cfg(all(windows, feature = "work-execution"))]
    windows_work_database: Option<windows_work_storage::WindowsWorkDatabase>,
    dir: Option<PathBuf>,
    media: Option<media::MediaStore>,
    meta: Connection,
    profiles: HashMap<ProfileId, Connection>,
    registry: HashSet<ProfileId>,
    /// Registered profiles whose exact ancillary SQLite file was securely
    /// identified but failed read-only schema/configuration validation. These
    /// files are preserved and never retried, opened read-write, or recreated
    /// during this process.
    degraded_profiles: HashSet<ProfileId>,
    /// Visits recorded per profile since history was last held to its row
    /// cap; the cap's ordered walk runs once per batch of visits, not each.
    visits_since_prune: HashMap<ProfileId, u32>,
    legacy_state_purged: bool,
    recovery_required: Option<String>,
    /// One unpredictable token shared by every Hub constructed in this
    /// process. A completed Windows unlink may be reconciled only by a Hub
    /// carrying a different token, making "after restart" an enforceable
    /// state-machine transition rather than a caller convention.
    deletion_process_generation: ProfileId,
    #[cfg(test)]
    ambiguous_profile_deletion_commit_once: bool,
    #[cfg(test)]
    fail_profile_deletion_after_local_purge_once: bool,
    #[cfg(test)]
    ambiguous_page_permission_commit_once: bool,
    #[cfg(test)]
    ambiguous_time_commit_once: bool,
}

pub(crate) struct AuthoritativeLoad {
    pub(crate) state: SessionState,
    pub(crate) blocker_configs: Vec<ProfileBlockerConfig>,
}

impl Hub {
    pub fn open(dir: PathBuf) -> rusqlite::Result<Self> {
        Self::open_with_deletion_process_generation(dir, deletion_process_generation())
    }

    fn open_with_deletion_process_generation(
        dir: PathBuf,
        deletion_process_generation: ProfileId,
    ) -> rusqlite::Result<Self> {
        Self::open_prepared(
            dir,
            deletion_process_generation,
            #[cfg(all(windows, feature = "work-execution"))]
            None,
        )
    }

    #[cfg(all(windows, feature = "work-execution"))]
    pub(crate) fn open_with_windows_work_storage(
        dir: PathBuf,
        storage: WindowsWorkStorage,
    ) -> rusqlite::Result<Self> {
        Self::open_prepared(dir, deletion_process_generation(), Some(storage))
    }

    fn open_prepared(
        dir: PathBuf,
        deletion_process_generation: ProfileId,
        #[cfg(all(windows, feature = "work-execution"))] windows_work_storage: Option<
            WindowsWorkStorage,
        >,
    ) -> rusqlite::Result<Self> {
        // Pin every derived database path to the canonical application-data
        // directory selected at startup. Final components are still opened
        // with NOFOLLOW and verified by file identity below.
        let dir =
            std::fs::canonicalize(&dir).map_err(|_| rusqlite::Error::InvalidPath(dir.clone()))?;
        if !std::fs::symlink_metadata(&dir)
            .map_err(|_| rusqlite::Error::InvalidPath(dir.clone()))?
            .file_type()
            .is_dir()
        {
            return Err(rusqlite::Error::InvalidPath(dir));
        }
        let mut meta = open_meta_database(&dir.join("meta.sqlite"))?;
        configure(&meta)?;
        migrations::apply(&mut meta, migrations::META)?;
        let recovery_required = recovery_reason(&meta)?;
        let authoritative = meta.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_snapshot WHERE id = 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        let mut hub = Self {
            work_runtime_session: zephium_core::work::WorkRuntimeSessionId::generate(),
            work_runtime_epoch: std::time::Instant::now(),
            #[cfg(feature = "work-execution")]
            work: None,
            #[cfg(all(windows, feature = "work-execution"))]
            windows_work_storage,
            #[cfg(all(windows, feature = "work-execution"))]
            windows_work_database: None,
            dir: Some(dir.clone()),
            media: Some(media::MediaStore::new(dir.join("media"))),
            meta,
            profiles: HashMap::new(),
            registry: HashSet::new(),
            degraded_profiles: HashSet::new(),
            visits_since_prune: HashMap::new(),
            legacy_state_purged: false,
            recovery_required,
            deletion_process_generation,
            #[cfg(test)]
            ambiguous_profile_deletion_commit_once: false,
            #[cfg(test)]
            fail_profile_deletion_after_local_purge_once: false,
            #[cfg(test)]
            ambiguous_page_permission_commit_once: false,
            #[cfg(test)]
            ambiguous_time_commit_once: false,
        };
        hub.load_registry()?;
        let _ = hub.recover_canonical_form_quarantine()?;
        // The snapshot and registry must agree before profile files are
        // migrated, purged, or reconciled. A corrupt authoritative row must
        // fail startup without destroying the only recoverable profile data.
        if authoritative && hub.recovery_required.is_none() {
            match hub.load_authoritative() {
                Ok(Some(_)) => {}
                Ok(None) => return Err(invalid_data("authoritative session snapshot is absent")),
                // `load` records semantic/corruption failures before
                // returning. Keep the hub available in explicit read-only
                // recovery mode instead of making the caller treat this as a
                // first run.
                Err(_) if hub.recovery_required.is_some() => {}
                Err(error) => return Err(error),
            }
        }
        if hub.recovery_required.is_none() {
            // Validate the entire durable deletion cohort before opening or
            // migrating any profile database. Actual deletion is coordinated
            // later with the native engine and never runs on startup.
            let journal = hub.profile_deletion_journal_entries()?;
            if !journal.is_empty() && !authoritative {
                return Err(invalid_data(
                    "profile deletion journal has no valid authoritative session",
                ));
            }
            // Windows cannot portably flush a directory handle after unlink.
            // A completed local tombstone therefore survives until a later
            // process start observes that the canonical database and both
            // SQLite sidecars remain absent. A resurrected artifact reopens
            // local cleanup without repeating native erasure.
            hub.reconcile_completed_profile_deletion_tombstones()?;
            hub.degraded_profiles =
                harden_registered_profile_files(&dir, &hub.registry, authoritative, authoritative)?;
        }
        hub.legacy_state_purged = authoritative;
        if hub.recovery_required.is_some() {
            return Ok(hub);
        }
        let primary = dir.join("default.sqlite");
        let backup = dir.join("default.sqlite.bak");
        let legacy_source = if regular_file_exists(&primary)? {
            Some(primary)
        } else if regular_file_exists(&backup)? {
            Some(backup)
        } else {
            None
        };
        if let Some(source) = legacy_source {
            match hub.app_setting(LEGACY_IMPORT_STATE_KEY).as_deref() {
                Some("started") => hub.import_legacy(&source)?,
                Some("complete") => remove_legacy_source(&source)?,
                None if hub.registry.is_empty() => hub.import_legacy(&source)?,
                // Older Zephium builds left a `.bak` after a fully committed
                // authoritative import. Retire that duplicate once the meta
                // snapshot proves the new copy exists.
                None if authoritative => {
                    hub.meta.execute(
                        "INSERT INTO settings(key, value) VALUES (?1, 'complete')
                         ON CONFLICT(key) DO UPDATE SET value = 'complete'",
                        [LEGACY_IMPORT_STATE_KEY],
                    )?;
                    remove_legacy_source(&source)?;
                }
                None => {}
                Some(_) => {
                    return Err(rusqlite::Error::InvalidParameterName(
                        "invalid legacy import state".into(),
                    ));
                }
            }
        }
        Ok(hub)
    }

    pub fn in_memory() -> rusqlite::Result<Self> {
        let mut meta = Connection::open_in_memory()?;
        configure(&meta)?;
        migrations::apply(&mut meta, migrations::META)?;
        Ok(Self {
            work_runtime_session: zephium_core::work::WorkRuntimeSessionId::generate(),
            work_runtime_epoch: std::time::Instant::now(),
            #[cfg(feature = "work-execution")]
            work: None,
            #[cfg(all(windows, feature = "work-execution"))]
            windows_work_storage: None,
            #[cfg(all(windows, feature = "work-execution"))]
            windows_work_database: None,
            dir: None,
            media: None,
            meta,
            profiles: HashMap::new(),
            registry: HashSet::new(),
            degraded_profiles: HashSet::new(),
            visits_since_prune: HashMap::new(),
            legacy_state_purged: false,
            recovery_required: None,
            deletion_process_generation: deletion_process_generation(),
            #[cfg(test)]
            ambiguous_profile_deletion_commit_once: false,
            #[cfg(test)]
            fail_profile_deletion_after_local_purge_once: false,
            #[cfg(test)]
            ambiguous_page_permission_commit_once: false,
            #[cfg(test)]
            ambiguous_time_commit_once: false,
        })
    }

    fn load_registry(&mut self) -> rusqlite::Result<()> {
        // Count the complete table before parsing any row. Applying LIMIT or
        // filtering malformed IDs in SQL can let hostile rows crowd a valid
        // profile out of the registry and turn reconciliation into deletion.
        let count = self
            .meta
            .query_row("SELECT count(*) FROM profiles", [], |row| {
                row.get::<_, i64>(0)
            })?;
        if !(0..=MAX_SESSION_PROFILES as i64).contains(&count) {
            return Err(invalid_data("profile registry exceeds persistence limit"));
        }

        let mut stmt = self.meta.prepare(
            "SELECT CASE WHEN length(CAST(id AS BLOB)) <= 26 THEN id END
                 FROM profiles ORDER BY position, id",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, Option<String>>(0))?;
        let mut registry = HashSet::with_capacity(count as usize);
        for row in rows {
            // Read the bounded CASE result, never an attacker-sized TEXT
            // value. Oversized rows fail the complete registry instead of
            // being filtered and authorizing reconciliation from a subset.
            let raw = row?.ok_or_else(|| invalid_data("profile registry id exceeds limit"))?;
            let profile = ProfileId::parse(&raw)
                .filter(|profile| profile.to_string() == raw)
                .ok_or_else(|| invalid_data("profile registry contains an invalid id"))?;
            if !registry.insert(profile) {
                return Err(invalid_data("profile registry contains duplicate ids"));
            }
        }
        if registry.len() != count as usize {
            return Err(invalid_data("profile registry changed while loading"));
        }
        self.registry = registry;
        Ok(())
    }

    fn profile_conn(&mut self, id: ProfileId) -> rusqlite::Result<&mut Connection> {
        if self.recovery_required.is_some() {
            return Err(invalid_data("session recovery mode is read-only"));
        }
        if self.degraded_profiles.contains(&id) {
            return Err(invalid_data(
                "profile ancillary storage is degraded and read-disabled",
            ));
        }
        use std::collections::hash_map::Entry;
        match self.profiles.entry(id) {
            Entry::Occupied(slot) => Ok(slot.into_mut()),
            Entry::Vacant(slot) => {
                let mut conn = match &self.dir {
                    Some(dir) => open_database(&dir.join(format!("profile-{id}.sqlite")))?,
                    None => Connection::open_in_memory()?,
                };
                filesystem::configure_profile(&conn)?;
                migrations::apply(&mut conn, migrations::PROFILE)?;
                history::enforce_history_budget(&conn)?;
                resources::prune_receipts(&conn)?;
                Ok(slot.insert(conn))
            }
        }
    }

    pub fn knows(&self, profile: ProfileId) -> bool {
        self.registry.contains(&profile)
    }

    pub fn degraded_profile_ids(&self) -> Vec<ProfileId> {
        let mut profiles: Vec<_> = self
            .degraded_profiles
            .iter()
            .copied()
            .filter(|profile| self.registry.contains(profile))
            .collect();
        profiles.sort_unstable_by_key(ToString::to_string);
        profiles
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn recovery_reason(conn: &Connection) -> rusqlite::Result<Option<String>> {
    const MAX_REASON_BYTES: i64 = 512;
    conn.query_row(
        "SELECT CASE WHEN length(CAST(reason AS BLOB)) <= ?1 THEN reason END
         FROM session_recovery WHERE id = 1",
        [MAX_REASON_BYTES],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()
    .map(|row| match row {
        None => None,
        Some(Some(reason)) => Some(reason),
        Some(None) => Some("invalid session recovery marker".into()),
    })
}

fn invalid_data(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.to_owned())
}
