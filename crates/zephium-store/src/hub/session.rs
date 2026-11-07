//! Authoritative session snapshots, bounded decoding, and recovery quarantine.

use super::*;

pub(super) struct PreparedSession {
    state: SessionState,
    snapshot: String,
    pub(super) registry: HashSet<ProfileId>,
}

/// Allocation-bounded wire representation for the authoritative snapshot.
/// These local types intentionally deny unknown fields; deserializing the
/// public core types directly would allow a damaged/newer snapshot to be
/// silently projected onto an older schema and then overwritten.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedSessionState {
    #[serde(deserialize_with = "deserialize_bounded_profiles")]
    profiles: Vec<BoundedProfile>,
    #[serde(deserialize_with = "deserialize_bounded_spaces")]
    spaces: Vec<BoundedSpace>,
    #[serde(deserialize_with = "deserialize_bounded_items")]
    items: Vec<BoundedItem>,
    active_space: Option<SpaceId>,
    active_item: Option<ItemId>,
    splits: Option<bounded_json::BoundedPane>,
    #[serde(default, deserialize_with = "deserialize_bounded_recently_closed")]
    recently_closed: Vec<BoundedClosedTab>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedProfile {
    id: ProfileId,
    name: String,
    kind: ProfileKind,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedSpace {
    id: SpaceId,
    profile: ProfileId,
    name: String,
    #[serde(default)]
    look: zephium_core::spaces::SpaceLook,
}

fn space_look_bounded(look: &zephium_core::spaces::SpaceLook) -> bool {
    use zephium_core::spaces::{SpaceIcon, SpaceTheme, MAX_SPACE_EMOJI_BYTES};
    let icon = match &look.icon {
        Some(SpaceIcon::Emoji(text)) => text.len() <= MAX_SPACE_EMOJI_BYTES,
        _ => true,
    };
    let theme = match &look.theme {
        Some(SpaceTheme::Preset(id)) => id.len() <= 64,
        Some(SpaceTheme::Custom(seeds)) => seeds.len() <= zephium_core::theme::MAX_CUSTOM_BYTES,
        None => true,
    };
    icon && theme
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedItem {
    id: ItemId,
    parent: Option<ItemId>,
    placement: BoundedPlacement,
    kind: BoundedKind,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundedClosedTab {
    profile: ProfileId,
    space: SpaceId,
    url: String,
    title: String,
    zoom: f64,
    #[serde(default)]
    session_id: Option<zephium_core::ids::ClosedSessionId>,
    #[serde(default)]
    closed_at_ms: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum BoundedPlacement {
    Favorites {
        #[serde(default)]
        space: Option<SpaceId>,
        /// Only in sessions saved as version 1, which kept one set per profile.
        #[serde(default)]
        profile: Option<ProfileId>,
    },
    Space {
        space: SpaceId,
        section: SpaceSection,
    },
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
enum BoundedKind {
    Folder {
        name: String,
    },
    Tab {
        url: String,
        title: String,
        zoom: f64,
        #[serde(default)]
        last_active_ms: Option<u64>,
    },
    BrowserTab {
        page: zephium_core::item::BrowserOwnedTab,
    },
}

fn deserialize_bounded_profiles<'de, D>(deserializer: D) -> Result<Vec<BoundedProfile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_PROFILES, |profile| {
        profile.kind != ProfileKind::Incognito && profile.name.len() <= MAX_NAME_BYTES
    })
}

fn deserialize_bounded_spaces<'de, D>(deserializer: D) -> Result<Vec<BoundedSpace>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_SPACES, |space| {
        space.name.len() <= MAX_NAME_BYTES && space_look_bounded(&space.look)
    })
}

fn deserialize_bounded_items<'de, D>(deserializer: D) -> Result<Vec<BoundedItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_ITEMS, |item| {
        match &item.kind {
            BoundedKind::Folder { name } => name.len() <= MAX_NAME_BYTES,
            BoundedKind::Tab {
                url, title, zoom, ..
            } => {
                url.len() <= MAX_URL_BYTES
                    && title.len() <= MAX_TITLE_BYTES
                    && zoom.is_finite()
                    && (0.3..=3.0).contains(zoom)
            }
            BoundedKind::BrowserTab { .. } => true,
        }
    })
}

fn deserialize_bounded_recently_closed<'de, D>(
    deserializer: D,
) -> Result<Vec<BoundedClosedTab>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_RECENTLY_CLOSED_TABS, |entry| {
        entry.url.len() <= MAX_URL_BYTES
            && entry.title.len() <= MAX_TITLE_BYTES
            && entry.zoom.is_finite()
            && (0.3..=3.0).contains(&entry.zoom)
            && (entry.session_id.is_some() == entry.closed_at_ms.is_some())
            && entry
                .closed_at_ms
                .is_none_or(|ms| (1_000..=9_007_199_254_740_991).contains(&ms))
    })
}

impl BoundedSessionState {
    /// `version` is the schema the snapshot was saved under, which decides
    /// whose Essentials its items name: a space's (2), or a profile's (1).
    fn into_state(self, version: i64) -> Option<SessionState> {
        let spaces: Vec<PersistedSpace> = self
            .spaces
            .into_iter()
            .map(|space| PersistedSpace {
                id: space.id,
                profile: space.profile,
                name: space.name,
                look: space.look,
            })
            .collect();
        let mut homes = HashMap::new();
        let mut items = Vec::with_capacity(self.items.len());
        for item in self.items {
            let placement = match (item.placement, version) {
                (
                    BoundedPlacement::Favorites {
                        space: Some(space),
                        profile: None,
                    },
                    2..,
                ) => Placement::Favorites { space },
                (
                    BoundedPlacement::Favorites {
                        space: None,
                        profile: Some(profile),
                    },
                    ..=1,
                ) => {
                    // A profile without a space has nowhere to keep them.
                    let Some(home) = essentials::home_space(&spaces, profile, self.active_space)
                    else {
                        continue;
                    };
                    homes.insert(profile, home);
                    Placement::Favorites { space: home }
                }
                (BoundedPlacement::Favorites { .. }, _) => return None,
                (BoundedPlacement::Space { space, section }, _) => {
                    Placement::Space { space, section }
                }
            };
            items.push(PersistedItem {
                id: item.id,
                parent: item.parent,
                placement,
                kind: match item.kind {
                    BoundedKind::Folder { name } => PersistedKind::Folder { name },
                    BoundedKind::Tab {
                        url,
                        title,
                        zoom,
                        last_active_ms,
                    } => PersistedKind::Tab {
                        url,
                        title,
                        zoom,
                        last_active_ms,
                    },
                    BoundedKind::BrowserTab { page } => PersistedKind::BrowserTab { page },
                },
            });
        }
        if !homes.is_empty() {
            items = essentials::spread(items, &spaces, &homes);
        }
        Some(SessionState {
            profiles: self
                .profiles
                .into_iter()
                .map(|profile| PersistedProfile {
                    id: profile.id,
                    name: profile.name,
                    kind: profile.kind,
                })
                .collect(),
            spaces,
            items,
            active_space: self.active_space,
            active_item: self.active_item,
            splits: self.splits.map(|pane| pane.0),
            recently_closed: self
                .recently_closed
                .into_iter()
                .map(|entry| PersistedClosedTab {
                    profile: entry.profile,
                    space: entry.space,
                    url: entry.url,
                    title: entry.title,
                    zoom: entry.zoom,
                    session_id: entry.session_id,
                    closed_at_ms: entry.closed_at_ms,
                    position: None,
                })
                .collect(),
        })
    }
}

/// Whether `version` is a schema this build reads, now or by upgrading.
fn readable_session_version(version: i64) -> bool {
    (1..=SESSION_SCHEMA_VERSION).contains(&version)
}

fn decode_authoritative_snapshot(data: &str) -> Option<SessionState> {
    decode_snapshot(data, SESSION_SCHEMA_VERSION)
}

fn decode_snapshot(data: &str, version: i64) -> Option<SessionState> {
    bounded_json::from_str::<BoundedSessionState>(data)
        .ok()?
        .into_state(version)
}

pub(super) struct SessionBackupSource {
    version: i64,
    data: Vec<u8>,
    canonical: String,
}

const SESSION_BACKUP_LIFETIME_SECS: i64 = 30 * 24 * 3600;

pub(super) struct SessionBackupExpiry {
    saved_at: i64,
    expires_at: std::time::Instant,
    retry_at: Option<std::time::Instant>,
}

fn snapshot_names_profile(state: &SessionState, profile: ProfileId) -> bool {
    state.profiles.iter().any(|entry| entry.id == profile)
        || state.spaces.iter().any(|space| space.profile == profile)
        || state
            .recently_closed
            .iter()
            .any(|entry| entry.profile == profile)
}

/// Unreadable recovery bytes cannot justify retaining a possible copy of
/// erased data. SQL failures still leave the deletion obligation pending.
pub(super) fn discard_profile_backup(
    meta: &Connection,
    profile: ProfileId,
) -> rusqlite::Result<bool> {
    let backup = meta
        .query_row(
            "SELECT CASE WHEN typeof(schema_version) = 'integer' THEN schema_version ELSE -1 END,
                    CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN CAST(data AS BLOB) END
             FROM session_snapshot_backup WHERE id = 1",
            [MAX_SESSION_SNAPSHOT_BYTES as i64],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .optional();
    let (version, data) = match backup {
        Ok(Some(backup)) => backup,
        Ok(None) => return Ok(false),
        Err(_) => {
            return meta
                .execute("DELETE FROM session_snapshot_backup WHERE id = 1", [])
                .map(|removed| removed > 0);
        }
    };
    let state = (version == SESSION_SCHEMA_VERSION)
        .then_some(data)
        .flatten()
        .and_then(|data| String::from_utf8(data).ok())
        .and_then(|data| decode_authoritative_snapshot(&data));
    if state
        .as_ref()
        .is_some_and(|state| !snapshot_names_profile(state, profile))
    {
        return Ok(false);
    }
    if meta.execute("DELETE FROM session_snapshot_backup WHERE id = 1", [])? != 1 {
        return Err(invalid_data(
            "session backup disposal did not remove its row",
        ));
    }
    Ok(true)
}

/// Runs after the disposal transaction commits. SQLite reports a busy
/// checkpoint as a successful query with a nonzero first result column.
/// Call even when the row is already gone: an earlier attempt may have
/// deleted it while an older WAL reader prevented retiring its frames.
pub(super) fn checkpoint_session_backup(meta: &Connection) -> rusqlite::Result<()> {
    let previous_timeout: u64 = meta.query_row("PRAGMA busy_timeout", [], |row| row.get(0))?;
    // A pinned WAL reader must defer cleanup immediately, without spending
    // the Store actor's ordinary write budget or delaying startup.
    meta.busy_timeout(std::time::Duration::ZERO)?;
    let result = meta.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    });
    // Restore before propagating either a SQL error or SQLite's busy row.
    meta.busy_timeout(std::time::Duration::from_millis(previous_timeout))?;
    let result = result?;
    if result != (0, 0, 0) && result != (0, -1, -1) {
        return Err(invalid_data("session backup WAL checkpoint is pending"));
    }
    Ok(())
}

impl Hub {
    pub(super) fn arm_session_backup_expiry(&mut self) -> rusqlite::Result<()> {
        let saved_at = self
            .meta
            .query_row(
                "SELECT CASE WHEN typeof(saved_at) = 'integer' THEN saved_at ELSE 0 END
             FROM session_snapshot_backup WHERE id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .unwrap_or(Some(0));
        self.session_backup_expiry = saved_at.map(|saved_at| {
            let remaining = now_secs()
                .checked_sub(saved_at)
                .filter(|age| saved_at > 0 && (0..SESSION_BACKUP_LIFETIME_SECS).contains(age))
                .map_or(0, |age| SESSION_BACKUP_LIFETIME_SECS - age);
            SessionBackupExpiry {
                saved_at,
                expires_at: std::time::Instant::now()
                    + std::time::Duration::from_secs(remaining as u64),
                retry_at: None,
            }
        });
        self.prune_expired_session_backup()
    }

    pub(crate) fn session_backup_deadline(&self) -> Option<std::time::Instant> {
        self.session_backup_expiry
            .as_ref()
            .map(|expiry| expiry.retry_at.unwrap_or(expiry.expires_at))
    }

    pub(crate) fn prune_expired_session_backup(&mut self) -> rusqlite::Result<()> {
        let Some(expiry) = &self.session_backup_expiry else {
            return Ok(());
        };
        let instant = std::time::Instant::now();
        if expiry.retry_at.is_some_and(|retry| instant < retry) {
            return Ok(());
        }
        let valid_age = now_secs().checked_sub(expiry.saved_at).is_some_and(|age| {
            expiry.saved_at > 0 && (0..SESSION_BACKUP_LIFETIME_SECS).contains(&age)
        });
        if valid_age && instant < expiry.expires_at {
            return Ok(());
        }
        let result = (|| {
            self.meta
                .execute("DELETE FROM session_snapshot_backup WHERE id = 1", [])?;
            let remains: bool = self.meta.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_snapshot_backup WHERE id = 1)",
                [],
                |row| row.get(0),
            )?;
            if remains {
                return Err(invalid_data("expired session backup was not removed"));
            }
            checkpoint_session_backup(&self.meta)
        })();
        if let Err(error) = result {
            if let Some(expiry) = &mut self.session_backup_expiry {
                expiry.retry_at = Some(instant + std::time::Duration::from_secs(30));
            }
            return Err(error);
        }
        self.session_backup_expiry = None;
        Ok(())
    }

    pub(crate) fn discard_session_backup(&mut self, profile: ProfileId) -> rusqlite::Result<()> {
        if !self.registry.contains(&profile) || self.recovery_required.is_some() {
            return Err(invalid_data("session backup profile is unavailable"));
        }
        let replacement = self.pending_session_backup.as_ref().and_then(|source| {
            let state = decode_authoritative_snapshot(&source.canonical)?;
            snapshot_names_profile(&state, profile).then(|| source.canonical.clone())
        });
        let tx = self.meta.transaction()?;
        let removed = discard_profile_backup(&tx, profile)?;
        if let (Some(source), Some(replacement)) = (&self.pending_session_backup, &replacement) {
            // A later load must not turn the just-erased raw source into a
            // pending backup again. Keep the current canonical session.
            if tx.execute(
                "UPDATE session_snapshot SET data = ?1, schema_version = ?4
                 WHERE id = 1 AND schema_version = ?2 AND CAST(data AS BLOB) = ?3",
                params![
                    replacement,
                    source.version,
                    source.data,
                    SESSION_SCHEMA_VERSION
                ],
            )? != 1
            {
                return Err(invalid_data(
                    "session backup source changed during disposal",
                ));
            }
        }
        tx.commit()?;
        self.session_backup_checkpoint_pending |= removed || replacement.is_some();
        if replacement.is_some() {
            self.pending_session_backup = None;
            self.committed_session = None;
        }
        if self.session_backup_checkpoint_pending {
            checkpoint_session_backup(&self.meta)?;
            self.session_backup_checkpoint_pending = false;
        }
        Ok(())
    }
}

#[cfg(test)]
mod qa_settings_recovery_tests {
    use super::*;

    const REASON: &str = "authoritative session is not in exact canonical form";

    fn saved_qa_session() -> SessionState {
        let profile = ProfileId::from(701);
        let space = SpaceId::from(702);
        SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "QA".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![PersistedSpace {
                id: space,
                profile,
                name: "Browse".into(),
                look: Default::default(),
            }],
            items: vec![
                PersistedItem {
                    id: ItemId::from(703),
                    parent: None,
                    placement: Placement::Space {
                        space,
                        section: SpaceSection::Today,
                    },
                    kind: PersistedKind::Tab {
                        url: "https://example.test/".into(),
                        title: "Example".into(),
                        zoom: 1.0,
                        last_active_ms: None,
                    },
                },
                PersistedItem {
                    id: ItemId::from(704),
                    parent: None,
                    placement: Placement::Space {
                        space,
                        section: SpaceSection::Today,
                    },
                    kind: PersistedKind::BrowserTab {
                        page: zephium_core::item::BrowserOwnedTab::Settings,
                    },
                },
            ],
            active_space: Some(space),
            active_item: Some(ItemId::from(704)),
            splits: None,
            recently_closed: Vec::new(),
        }
    }

    fn marked_directory(reason: &str) -> (tempfile::TempDir, SessionState) {
        let dir = tempfile::tempdir().unwrap();
        let state = saved_qa_session();
        assert_eq!(core_session::canonicalize(state.clone()), state);
        {
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            hub.save(&state).unwrap();
            hub.record_visit(state.profiles[0].id, "https://example.test/", "Example");
        }
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        let data: String = meta
            .query_row(
                "SELECT data FROM session_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        meta.execute(
            "INSERT INTO session_recovery(id, detected_at, reason, schema_version, data)
             VALUES (1, 1, ?1, ?2, ?3)",
            params![reason, SESSION_SCHEMA_VERSION, data.as_bytes()],
        )
        .unwrap();
        (dir, state)
    }

    #[test]
    fn exact_qa_settings_marker_recovers_without_rewriting_session_or_profile_files() {
        let (dir, expected) = marked_directory(REASON);
        let profile_file = dir
            .path()
            .join(format!("profile-{}.sqlite", expected.profiles[0].id));
        let original: String = Connection::open(dir.path().join("meta.sqlite"))
            .unwrap()
            .query_row(
                "SELECT data FROM session_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.recovery_reason().is_none());
        assert_eq!(hub.load().unwrap(), Some(expected));
        assert!(profile_file.exists());
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        let after: String = meta
            .query_row(
                "SELECT data FROM session_snapshot WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let markers: i64 = meta
            .query_row("SELECT count(*) FROM session_recovery", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after, original);
        assert_eq!(markers, 0);
    }

    #[test]
    fn noncanonical_sessions_restore_and_keep_exact_source_before_first_save() {
        for damage in ["dangling-parent", "duplicate-item", "unknown-space-owner"] {
            let dir = tempfile::tempdir().unwrap();
            let mut state = saved_qa_session();
            Hub::open(dir.path().to_path_buf())
                .unwrap()
                .save(&state)
                .unwrap();
            match damage {
                "dangling-parent" => state.items[0].parent = Some(ItemId::from(999)),
                "duplicate-item" => state.items.push(state.items[0].clone()),
                "unknown-space-owner" => state.spaces[0].profile = ProfileId::from(999),
                _ => unreachable!(),
            }
            let canonical = core_session::canonicalize(state.clone());
            assert_ne!(canonical, state);
            let original = format!("  {}\n", serde_json::to_string_pretty(&state).unwrap());
            let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            meta.execute(
                "UPDATE session_snapshot SET data = ?1 WHERE id = 1",
                [&original],
            )
            .unwrap();
            for _ in 0..2 {
                let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
                assert!(hub.recovery_reason().is_none(), "{damage}");
                assert_eq!(hub.load().unwrap(), Some(canonical.clone()));
                let current: String = meta
                    .query_row("SELECT data FROM session_snapshot WHERE id = 1", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(current, original);
            }
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            hub.save(&canonical).unwrap();
            let kept: Vec<u8> = meta
                .query_row(
                    "SELECT data FROM session_snapshot_backup WHERE id = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(kept, original.as_bytes());
            hub.save(&saved_qa_session()).unwrap();
            let kept_again: Vec<u8> = meta
                .query_row(
                    "SELECT data FROM session_snapshot_backup WHERE id = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(kept_again, kept);
            drop(hub);
            assert!(Hub::open(dir.path().to_path_buf())
                .unwrap()
                .recovery_reason()
                .is_none());
        }
    }

    #[test]
    fn compatibility_backup_and_replacement_commit_or_roll_back_together() {
        let (dir, mut state) = marked_directory(REASON);
        state.active_item = Some(ItemId::from(999_999));
        let original = serde_json::to_string_pretty(&state).unwrap();
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        meta.execute("UPDATE session_snapshot SET data = ?1", [&original])
            .unwrap();
        meta.execute(
            "UPDATE session_recovery SET data = ?1",
            [original.as_bytes()],
        )
        .unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(
            hub.recovery_reason().is_none(),
            "obsolete semantic repair locks lift"
        );
        let canonical = hub.load().unwrap().unwrap();
        meta.execute_batch("CREATE TRIGGER fail_snapshot BEFORE UPDATE ON session_snapshot BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        assert!(hub.save(&canonical).is_err());
        let backups: i64 = meta
            .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| {
                r.get(0)
            })
            .unwrap();
        let current: String = meta
            .query_row("SELECT data FROM session_snapshot WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(backups, 0);
        assert_eq!(current, original);
        meta.execute_batch("DROP TRIGGER fail_snapshot").unwrap();
        hub.save(&canonical).unwrap();
        let kept: Vec<u8> = meta
            .query_row(
                "SELECT data FROM session_snapshot_backup WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, original.as_bytes());
    }

    #[test]
    fn explicit_deletion_disposes_only_a_backup_that_names_its_profile() {
        for contains_deleted in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut state = saved_qa_session();
            Hub::open(dir.path().to_path_buf())
                .unwrap()
                .save(&state)
                .unwrap();
            state.active_item = Some(ItemId::from(999_999));
            let original = serde_json::to_string_pretty(&state).unwrap();
            let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            meta.execute("UPDATE session_snapshot SET data = ?1", [&original])
                .unwrap();
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            let mut canonical = hub.load().unwrap().unwrap();
            hub.save(&canonical).unwrap();
            let deleted = if contains_deleted {
                state.profiles[0].id
            } else {
                let id = ProfileId::from(705);
                canonical.profiles.push(PersistedProfile {
                    id,
                    name: "Other".into(),
                    kind: ProfileKind::Named,
                });
                hub.save(&canonical).unwrap();
                id
            };
            let filtered = if contains_deleted {
                SessionState::default()
            } else {
                core_session::canonicalize(saved_qa_session())
            };
            assert_eq!(
                hub.authorize_profile_deletion(deleted, &filtered).unwrap(),
                zephium_core::ports::store::ProfileDeletionAuthorizeOutcome::Authorized
            );
            hub.fail_next_profile_deletion_after_local_purge();
            assert!(hub.finalize_profile_deletion(deleted).is_err());
            let kept: Vec<u8> = meta
                .query_row(
                    "SELECT data FROM session_snapshot_backup WHERE id = 1",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(kept, original.as_bytes());
            assert!(String::from_utf8(kept)
                .unwrap()
                .contains("https://example.test/"));
            assert!(!hub.pending_profile_deletions().unwrap().is_empty());
            if contains_deleted {
                // A final journal failure retains its obligation after backup disposal.
                meta.execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON profile_deletion_journal BEGIN SELECT RAISE(ABORT, 'fixture'); END;
                    CREATE TRIGGER fail_update BEFORE UPDATE ON profile_deletion_journal BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
                assert!(hub.finalize_profile_deletion(deleted).is_err());
                let kept: Option<Vec<u8>> = meta
                    .query_row(
                        "SELECT data FROM session_snapshot_backup WHERE id = 1",
                        [],
                        |r| r.get(0),
                    )
                    .optional()
                    .unwrap();
                assert_eq!(kept, None);
                assert!(!hub.pending_profile_deletions().unwrap().is_empty());
                meta.execute_batch("DROP TRIGGER fail_delete; DROP TRIGGER fail_update;")
                    .unwrap();
            }
            assert!(hub.finalize_profile_deletion(deleted).unwrap());
            let kept: Option<Vec<u8>> = meta
                .query_row(
                    "SELECT data FROM session_snapshot_backup WHERE id = 1",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .unwrap();
            assert_eq!(
                kept,
                (!contains_deleted).then(|| original.as_bytes().to_vec())
            );
            assert!(hub.pending_profile_deletions().unwrap().is_empty());
        }
    }

    #[test]
    fn unreadable_backup_is_discarded_to_complete_explicit_deletion() {
        for damage in ["corrupt", "utf8", "newer", "unknown-field", "oversized"] {
            let mut hub = Hub::in_memory().unwrap();
            let state = saved_qa_session();
            hub.save(&state).unwrap();
            hub.meta
                .execute(
                    "INSERT INTO session_snapshot_backup VALUES (1, 1, ?1, ?2)",
                    params![SESSION_SCHEMA_VERSION, serde_json::to_vec(&state).unwrap()],
                )
                .unwrap();
            match damage {
                "corrupt" => {
                    hub.meta
                        .execute(
                            "UPDATE session_snapshot_backup SET data = ?1",
                            [b"{broken".as_slice()],
                        )
                        .unwrap();
                }
                "utf8" => {
                    hub.meta
                        .execute(
                            "UPDATE session_snapshot_backup SET data = ?1",
                            [b"\xff".as_slice()],
                        )
                        .unwrap();
                }
                "newer" => {
                    hub.meta
                        .execute(
                            "UPDATE session_snapshot_backup SET schema_version = ?1",
                            [SESSION_SCHEMA_VERSION + 1],
                        )
                        .unwrap();
                }
                "unknown-field" => {
                    let mut data = serde_json::to_value(&state).unwrap();
                    data["future"] = serde_json::json!(true);
                    hub.meta
                        .execute(
                            "UPDATE session_snapshot_backup SET data = ?1",
                            [serde_json::to_vec(&data).unwrap()],
                        )
                        .unwrap();
                }
                "oversized" => {
                    hub.meta
                        .execute(
                            "UPDATE session_snapshot_backup SET data = zeroblob(?1)",
                            [MAX_SESSION_SNAPSHOT_BYTES as i64 + 1],
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let profile = state.profiles[0].id;
            hub.authorize_profile_deletion(profile, &SessionState::default())
                .unwrap();
            assert!(hub.finalize_profile_deletion(profile).unwrap(), "{damage}");
            assert_eq!(
                hub.pending_profile_deletions().unwrap().len(),
                0,
                "{damage}"
            );
            let kept: i64 = hub
                .meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(kept, 0, "{damage}");
        }
    }

    #[test]
    fn backup_checkpoint_never_waits_and_restores_the_write_timeout_on_every_result() {
        let dir = tempfile::tempdir().unwrap();
        let meta = Connection::open(dir.path().join("checkpoint.sqlite")).unwrap();
        configure(&meta).unwrap();
        meta.execute_batch("CREATE TABLE fixture(data); INSERT INTO fixture VALUES ('before');")
            .unwrap();
        meta.busy_timeout(std::time::Duration::from_millis(5432))
            .unwrap();
        checkpoint_session_backup(&meta).unwrap();
        let timeout = || {
            meta.query_row("PRAGMA busy_timeout", [], |row| row.get::<_, u64>(0))
                .unwrap()
        };
        assert_eq!(timeout(), 5432);
        let reader = Connection::open(dir.path().join("checkpoint.sqlite")).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM fixture;")
            .unwrap();
        meta.execute_batch("UPDATE fixture SET data = 'after';")
            .unwrap();
        let started = std::time::Instant::now();
        assert!(checkpoint_session_backup(&meta).is_err());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "a pinned reader must defer instead of spending the write timeout"
        );
        assert_eq!(timeout(), 5432);
        reader.execute_batch("ROLLBACK").unwrap();
        checkpoint_session_backup(&meta).unwrap();
        assert_eq!(timeout(), 5432);
        // SQLite returns an actual query error when this connection itself
        // holds a write transaction. Restore the timeout on that path too.
        meta.execute_batch("BEGIN IMMEDIATE; UPDATE fixture SET data = 'transaction';")
            .unwrap();
        assert!(matches!(
            checkpoint_session_backup(&meta),
            Err(rusqlite::Error::SqliteFailure(..))
        ));
        assert_eq!(timeout(), 5432);
        meta.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn busy_backup_checkpoint_at_restart_reopens_local_deletion_without_blocking_session_load() {
        let dir = tempfile::tempdir().unwrap();
        let state = saved_qa_session();
        let profile = state.profiles[0].id;
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap();
        assert!(hub
            .finalize_profile_deletion_requiring_restart_confirmation(profile)
            .unwrap());
        assert_eq!(
            hub.completed_profile_deletion_tombstones().unwrap(),
            vec![profile]
        );
        // A reader can pin frames written after local unlink was confirmed,
        // including a malformed recovery copy requiring conservative disposal.
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![
                    now_secs(),
                    SESSION_SCHEMA_VERSION,
                    b"private restart bytes".as_slice()
                ],
            )
            .unwrap();
        let reader = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM session_snapshot_backup;")
            .unwrap();
        let mut reopened = Hub::open_for_new_process(dir.path().to_path_buf()).unwrap();
        assert_eq!(reopened.load().unwrap(), Some(SessionState::default()));
        let pending = reopened.pending_profile_deletions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].profile, profile);
        assert!(pending[0].native_erasure_verified);
        assert!(reopened
            .completed_profile_deletion_tombstones()
            .unwrap()
            .is_empty());
        reopened
            .meta
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        assert!(reopened.finalize_profile_deletion(profile).is_err());
        assert_eq!(reopened.pending_profile_deletions().unwrap().len(), 1);
        reader.execute_batch("ROLLBACK").unwrap();
        assert!(reopened.finalize_profile_deletion(profile).unwrap());
        assert!(reopened.pending_profile_deletions().unwrap().is_empty());
    }

    #[test]
    fn busy_backup_checkpoint_keeps_deletion_pending_after_row_disposal_until_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        let state = saved_qa_session();
        let profile = state.profiles[0].id;
        hub.save(&state).unwrap();
        hub.meta.busy_timeout(std::time::Duration::ZERO).unwrap();
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![
                    now_secs(),
                    SESSION_SCHEMA_VERSION,
                    b"private erased bytes".as_slice()
                ],
            )
            .unwrap();
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap();
        let reader = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        reader
            .execute_batch("BEGIN; SELECT * FROM session_snapshot_backup;")
            .unwrap();
        assert!(hub.finalize_profile_deletion(profile).is_err());
        assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
        assert_eq!(
            hub.meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        // Already-deleted backup must still checkpoint on every retry.
        assert!(hub.finalize_profile_deletion(profile).is_err());
        assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
        reader.execute_batch("ROLLBACK").unwrap();
        assert!(hub.finalize_profile_deletion(profile).unwrap());
        assert!(hub.pending_profile_deletions().unwrap().is_empty());
        let wal = std::fs::read(dir.path().join("meta.sqlite-wal")).unwrap();
        assert!(!wal
            .windows(b"private erased bytes".len())
            .any(|bytes| bytes == b"private erased bytes"));
    }

    #[test]
    fn failed_backup_disposal_keeps_deletion_pending_until_retry() {
        let mut hub = Hub::in_memory().unwrap();
        let state = saved_qa_session();
        let profile = state.profiles[0].id;
        hub.save(&state).unwrap();
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![now_secs(), SESSION_SCHEMA_VERSION, b"{broken".as_slice()],
            )
            .unwrap();
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap();
        hub.meta.execute_batch("CREATE TRIGGER fail_backup_delete BEFORE DELETE ON session_snapshot_backup BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        assert!(hub.finalize_profile_deletion(profile).is_err());
        assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
        hub.meta
            .execute_batch("DROP TRIGGER fail_backup_delete")
            .unwrap();
        assert!(hub.finalize_profile_deletion(profile).unwrap());
        assert!(hub.pending_profile_deletions().unwrap().is_empty());
    }

    #[test]
    fn clearing_history_discards_its_backup_even_for_a_recent_range() {
        for since in [None, Some(now_secs())] {
            let mut hub = Hub::in_memory().unwrap();
            let state = saved_qa_session();
            hub.save(&state).unwrap();
            hub.meta
                .execute(
                    "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                    params![
                        now_secs(),
                        SESSION_SCHEMA_VERSION,
                        serde_json::to_vec(&state).unwrap()
                    ],
                )
                .unwrap();
            assert_eq!(
                hub.clear_history_checked(state.profiles[0].id, since),
                Some(0)
            );
            let backups: i64 = hub
                .meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(backups, 0);
        }
    }

    #[test]
    fn old_or_invalid_backup_times_expire_at_open_without_refreshing_fresh_rows() {
        let now = now_secs();
        for (saved_at, retained) in [
            (now - 30 * 24 * 3600, false),
            (1, false),
            (0, false),
            (-1, false),
            (i64::MAX, false),
            (now + 3600, false),
            (now - 24 * 3600, true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let state = saved_qa_session();
            Hub::open(dir.path().to_path_buf())
                .unwrap()
                .save(&state)
                .unwrap();
            let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            let bytes = serde_json::to_vec(&state).unwrap();
            meta.execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![saved_at, SESSION_SCHEMA_VERSION, &bytes],
            )
            .unwrap();
            for _ in 0..2 {
                let _hub = Hub::open(dir.path().to_path_buf()).unwrap();
                let backup: Option<(i64, Vec<u8>)> = meta
                    .query_row(
                        "SELECT saved_at, data FROM session_snapshot_backup",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .unwrap();
                assert_eq!(
                    backup,
                    retained.then(|| (saved_at, bytes.clone())),
                    "saved_at={saved_at}"
                );
            }
        }
    }

    #[test]
    fn privacy_clear_retires_pending_source_before_load_reopen_or_later_save() {
        for history in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut state = saved_qa_session();
            Hub::open(dir.path().to_path_buf())
                .unwrap()
                .save(&state)
                .unwrap();
            state.active_item = Some(ItemId::from(999_999));
            let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            meta.execute(
                "UPDATE session_snapshot SET data = ?1",
                [serde_json::to_string_pretty(&state).unwrap()],
            )
            .unwrap();
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            assert!(hub.pending_session_backup.is_some());
            let profile = state.profiles[0].id;
            if history {
                assert_eq!(hub.clear_history_checked(profile, None), Some(0));
            } else {
                hub.discard_session_backup(profile).unwrap();
            }
            assert!(hub.pending_session_backup.is_none());
            let canonical = hub.load().unwrap().unwrap();
            assert!(hub.pending_session_backup.is_none());
            hub.save(&canonical).unwrap();
            drop(hub);
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            let loaded = hub.load().unwrap().unwrap();
            assert_eq!(loaded, canonical);
            hub.save(&loaded).unwrap();
            assert!(hub.pending_session_backup.is_none());
            let backups: i64 = meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(backups, 0);
        }
    }

    #[test]
    fn privacy_clear_preserves_unrelated_backups_and_rejects_unknown_profiles() {
        let mut hub = Hub::in_memory().unwrap();
        let backup = saved_qa_session();
        let mut state = backup.clone();
        let other = ProfileId::from(705);
        state.profiles.push(PersistedProfile {
            id: other,
            name: "Other".into(),
            kind: ProfileKind::Named,
        });
        hub.save(&core_session::canonicalize(state)).unwrap();
        let bytes = serde_json::to_vec(&backup).unwrap();
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![now_secs(), SESSION_SCHEMA_VERSION, &bytes],
            )
            .unwrap();
        assert!(hub.discard_session_backup(ProfileId::from(999)).is_err());
        hub.discard_session_backup(other).unwrap();
        let kept: Vec<u8> = hub
            .meta
            .query_row("SELECT data FROM session_snapshot_backup", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, bytes);
        hub.discard_session_backup(backup.profiles[0].id).unwrap();
        assert_eq!(
            hub.meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn history_clear_checkpoints_only_disposed_session_bytes_and_retains_failed_cleanup() {
        for unrelated_backup in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            let backup = saved_qa_session();
            let profile = ProfileId::from(705);
            let mut state = backup.clone();
            state.profiles.push(PersistedProfile {
                id: profile,
                name: "Other".into(),
                kind: ProfileKind::Named,
            });
            hub.save(&core_session::canonicalize(state)).unwrap();
            if unrelated_backup {
                hub.meta
                    .execute(
                        "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                        params![
                            now_secs(),
                            SESSION_SCHEMA_VERSION,
                            serde_json::to_vec(&backup).unwrap()
                        ],
                    )
                    .unwrap();
            }
            let reader = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            reader
                .execute_batch("BEGIN; SELECT * FROM session_snapshot")
                .unwrap();
            hub.meta
                .execute_batch("CREATE TABLE checkpoint_fixture(value INTEGER)")
                .unwrap();
            assert_eq!(hub.clear_history_checked(profile, None), Some(0));
            assert_eq!(
                hub.meta
                    .query_row("SELECT count(*) FROM session_snapshot_backup", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                i64::from(unrelated_backup)
            );

            let saved = serde_json::to_vec(&hub.load().unwrap().unwrap()).unwrap();
            hub.meta
                .execute(
                    "INSERT OR REPLACE INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                    params![now_secs(), SESSION_SCHEMA_VERSION, saved],
                )
                .unwrap();
            assert_eq!(hub.clear_history_checked(profile, None), None);
            assert_eq!(
                hub.meta
                    .query_row("SELECT count(*) FROM session_snapshot_backup", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0
            );
            assert_eq!(hub.clear_history_checked(profile, None), None);
            reader.execute_batch("ROLLBACK").unwrap();
            assert_eq!(hub.clear_history_checked(profile, None), Some(0));
            reader
                .execute_batch("BEGIN; SELECT * FROM session_snapshot")
                .unwrap();
            hub.meta
                .execute_batch("INSERT INTO checkpoint_fixture VALUES (1)")
                .unwrap();
            assert_eq!(hub.clear_history_checked(profile, None), Some(0));
            reader.execute_batch("ROLLBACK").unwrap();
        }
    }

    #[test]
    fn canonical_source_repair_retains_checkpoint_debt_without_a_backup_row() {
        let (dir, mut state) = marked_directory(REASON);
        state.active_item = Some(ItemId::from(999_999));
        let source = serde_json::to_string_pretty(&state).unwrap();
        let reader = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        reader
            .execute("UPDATE session_snapshot SET data = ?1", [&source])
            .unwrap();
        reader
            .execute("UPDATE session_recovery SET data = ?1", [source.as_bytes()])
            .unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.pending_session_backup.is_some());
        reader
            .execute_batch("BEGIN; SELECT * FROM session_snapshot")
            .unwrap();
        assert!(hub.discard_session_backup(state.profiles[0].id).is_err());
        assert!(hub.pending_session_backup.is_none());
        assert!(hub.discard_session_backup(state.profiles[0].id).is_err());
        reader.execute_batch("ROLLBACK").unwrap();
        hub.discard_session_backup(state.profiles[0].id).unwrap();
    }

    #[test]
    fn privacy_clear_reports_sql_failure_and_rolls_back_pending_source_repair() {
        let (dir, mut state) = marked_directory(REASON);
        state.active_item = Some(ItemId::from(999_999));
        let original = serde_json::to_string_pretty(&state).unwrap();
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        meta.execute("UPDATE session_snapshot SET data = ?1", [&original])
            .unwrap();
        meta.execute(
            "UPDATE session_recovery SET data = ?1",
            [original.as_bytes()],
        )
        .unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        let profile = state.profiles[0].id;
        let bytes = serde_json::to_vec(&state).unwrap();
        meta.execute(
            "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
            params![now_secs(), SESSION_SCHEMA_VERSION, &bytes],
        )
        .unwrap();
        for failure in [
            "DELETE ON session_snapshot_backup",
            "UPDATE ON session_snapshot",
        ] {
            meta.execute_batch(&format!("CREATE TRIGGER fail_privacy BEFORE {failure} BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;")).unwrap();
            assert!(hub.discard_session_backup(profile).is_err());
            assert_eq!(hub.clear_history_checked(profile, None), None);
            assert!(hub.pending_session_backup.is_some());
            assert_eq!(
                meta.query_row("SELECT data FROM session_snapshot", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                original
            );
            assert_eq!(
                meta.query_row("SELECT data FROM session_snapshot_backup", [], |r| r
                    .get::<_, Vec<u8>>(0))
                    .unwrap(),
                bytes
            );
            meta.execute_batch("DROP TRIGGER fail_privacy").unwrap();
        }
        hub.discard_session_backup(profile).unwrap();
        assert!(hub.pending_session_backup.is_none());
    }

    #[test]
    fn backup_deadline_bounds_clock_rollback_and_failed_expiry_retries_without_spinning() {
        let mut hub = Hub::in_memory().unwrap();
        let bytes = serde_json::to_vec(&saved_qa_session()).unwrap();
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![now_secs(), SESSION_SCHEMA_VERSION, &bytes],
            )
            .unwrap();
        hub.arm_session_backup_expiry().unwrap();
        let initial = hub.session_backup_deadline().unwrap();
        assert!(initial > std::time::Instant::now());
        hub.prune_expired_session_backup().unwrap();
        assert_eq!(hub.session_backup_deadline(), Some(initial));
        hub.session_backup_expiry.as_mut().unwrap().expires_at = std::time::Instant::now();
        hub.meta.execute_batch("CREATE TRIGGER fail_expiry BEFORE DELETE ON session_snapshot_backup BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        assert!(hub.prune_expired_session_backup().is_err());
        assert!(hub.session_backup_deadline().unwrap() > std::time::Instant::now());
        assert!(hub.prune_expired_session_backup().is_ok());
        hub.meta.execute_batch("DROP TRIGGER fail_expiry").unwrap();
        hub.session_backup_expiry.as_mut().unwrap().retry_at = None;
        hub.prune_expired_session_backup().unwrap();
        assert!(hub.session_backup_deadline().is_none());
        assert_eq!(
            hub.meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn unreadable_backup_query_still_attempts_erasure_and_reports_write_failure() {
        unsafe extern "C" fn deny_backup_data_read(
            _context: *mut std::ffi::c_void,
            action: std::ffi::c_int,
            table: *const std::ffi::c_char,
            column: *const std::ffi::c_char,
            _database: *const std::ffi::c_char,
            _trigger: *const std::ffi::c_char,
        ) -> std::ffi::c_int {
            if action == rusqlite::ffi::SQLITE_READ
                && !table.is_null()
                && !column.is_null()
                && unsafe { std::ffi::CStr::from_ptr(table).to_bytes() }
                    == b"session_snapshot_backup"
                && unsafe { std::ffi::CStr::from_ptr(column).to_bytes() } == b"data"
            {
                rusqlite::ffi::SQLITE_DENY
            } else {
                rusqlite::ffi::SQLITE_OK
            }
        }
        let mut hub = Hub::in_memory().unwrap();
        let state = saved_qa_session();
        let profile = state.profiles[0].id;
        hub.save(&state).unwrap();
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![
                    now_secs(),
                    SESSION_SCHEMA_VERSION,
                    serde_json::to_vec(&state).unwrap()
                ],
            )
            .unwrap();
        hub.authorize_profile_deletion(profile, &SessionState::default())
            .unwrap();
        hub.meta.execute_batch("CREATE TRIGGER fail_backup_delete BEFORE DELETE ON session_snapshot_backup BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(
                hub.meta.handle(),
                Some(deny_backup_data_read),
                std::ptr::null_mut(),
            );
        }
        let failed = hub.finalize_profile_deletion(profile);
        unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(hub.meta.handle(), None, std::ptr::null_mut());
        }
        assert!(failed
            .unwrap_err()
            .to_string()
            .contains("fixture write failure"));
        assert_eq!(hub.pending_profile_deletions().unwrap().len(), 1);
        hub.meta
            .execute_batch("DROP TRIGGER fail_backup_delete")
            .unwrap();
        unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(
                hub.meta.handle(),
                Some(deny_backup_data_read),
                std::ptr::null_mut(),
            );
        }
        let completed = hub.finalize_profile_deletion(profile);
        unsafe {
            rusqlite::ffi::sqlite3_set_authorizer(hub.meta.handle(), None, std::ptr::null_mut());
        }
        assert!(completed.unwrap());
        assert!(hub.pending_profile_deletions().unwrap().is_empty());
        assert_eq!(
            hub.meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn expiry_write_failure_at_open_preserves_session_availability_and_retries() {
        let dir = tempfile::tempdir().unwrap();
        let state = saved_qa_session();
        Hub::open(dir.path().to_path_buf())
            .unwrap()
            .save(&state)
            .unwrap();
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        meta.execute(
            "INSERT INTO session_snapshot_backup VALUES (1, 1, ?1, ?2)",
            params![SESSION_SCHEMA_VERSION, serde_json::to_vec(&state).unwrap()],
        )
        .unwrap();
        meta.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(hub.load().unwrap(), Some(state));
        assert!(hub.recovery_reason().is_none());
        assert!(hub.session_backup_deadline().unwrap() > std::time::Instant::now());
        meta.execute_batch("ROLLBACK").unwrap();
        hub.session_backup_expiry.as_mut().unwrap().retry_at = None;
        hub.prune_expired_session_backup().unwrap();
        assert!(hub.session_backup_deadline().is_none());
        assert_eq!(
            meta.query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn forgetting_history_discards_recovery_backup() {
        let mut hub = Hub::in_memory().unwrap();
        let state = saved_qa_session();
        let profile = state.profiles[0].id;
        let url = "https://example.test/";
        hub.save(&state).unwrap();
        hub.record_visit(profile, url, "Example");
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![
                    now_secs(),
                    SESSION_SCHEMA_VERSION,
                    serde_json::to_vec(&state).unwrap()
                ],
            )
            .unwrap();
        assert_eq!(hub.forget_history_urls(profile, &[url.into()]), 1);
        assert_eq!(
            hub.meta
                .query_row("SELECT count(*) FROM session_snapshot_backup", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn checked_history_forget_reports_backup_and_history_sql_failures() {
        let mut hub = Hub::in_memory().unwrap();
        let state = saved_qa_session();
        let profile = state.profiles[0].id;
        let urls = vec!["https://example.test/".to_owned()];
        hub.save(&state).unwrap();
        hub.record_visit(profile, &urls[0], "Example");
        hub.meta
            .execute(
                "INSERT INTO session_snapshot_backup VALUES (1, ?1, ?2, ?3)",
                params![
                    now_secs(),
                    SESSION_SCHEMA_VERSION,
                    serde_json::to_vec(&state).unwrap()
                ],
            )
            .unwrap();
        assert_eq!(
            hub.forget_history_urls_checked(ProfileId::from(999), &urls),
            None
        );
        hub.meta.execute_batch("CREATE TRIGGER fail_backup_delete BEFORE DELETE ON session_snapshot_backup BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        assert_eq!(hub.forget_history_urls_checked(profile, &urls), None);
        hub.meta
            .execute_batch("DROP TRIGGER fail_backup_delete")
            .unwrap();
        hub.profile_conn(profile).unwrap().execute_batch("CREATE TRIGGER fail_history_delete BEFORE DELETE ON history BEGIN SELECT RAISE(ABORT, 'fixture write failure'); END;").unwrap();
        assert_eq!(hub.forget_history_urls_checked(profile, &urls), None);
        assert_eq!(hub.history_page(profile, "", None, None, 10).len(), 1);
        hub.profile_conn(profile)
            .unwrap()
            .execute_batch("DROP TRIGGER fail_history_delete")
            .unwrap();
        assert_eq!(hub.forget_history_urls_checked(profile, &urls), Some(1));
        assert_eq!(hub.forget_history_urls_checked(profile, &urls), Some(0));
    }

    #[test]
    fn a_session_kept_for_a_newer_zephium_opens_once_a_build_that_reads_it_is_back() {
        // An older build met this session, could not read it and kept it;
        // this build reads it, so it opens as it was.
        let (dir, expected) = marked_directory(zephium_core::ports::store::NEWER_SESSION_REASON);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.recovery_reason().is_none());
        assert_eq!(
            hub.load().unwrap().expect("the session is restored"),
            expected
        );
    }

    #[test]
    fn an_unrestorable_session_is_set_aside_and_its_profile_keeps_its_data() {
        let (dir, saved) = marked_directory("corrupt authoritative session snapshot");
        let profile = saved.profiles[0].id;
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.load().is_err());

        let file = hub
            .set_aside_recovery()
            .unwrap()
            .expect("the bytes are kept");
        let kept = std::fs::read_to_string(&file).unwrap();
        assert!(kept.contains(&profile.to_string()), "{kept}");
        assert_eq!(
            file.parent().unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );

        assert!(hub.recovery_reason().is_none());
        let restarted = hub.load().unwrap().unwrap();
        assert_eq!(restarted.profiles.len(), 1);
        assert_eq!(restarted.profiles[0].id, profile);
        assert_eq!(restarted.profiles[0].name, "QA");
        assert_eq!(restarted.profiles[0].kind, ProfileKind::Default);
        assert!(restarted.items.is_empty() && restarted.spaces.is_empty());
        // The profile's history is still its own.
        assert!(!hub.search_history(profile, "example", 10).is_empty());

        // Reopening finds an ordinary session, not recovery.
        drop(hub);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.recovery_reason().is_none());
        assert_eq!(hub.load().unwrap().unwrap().profiles[0].id, profile);
    }

    #[test]
    fn a_session_from_a_newer_zephium_is_kept_for_it_rather_than_set_aside() {
        let (dir, _saved) = marked_directory("unrelated");
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        meta.execute("DELETE FROM session_recovery", []).unwrap();
        meta.execute(
            "UPDATE session_snapshot SET schema_version = ?1 WHERE id = 1",
            [SESSION_SCHEMA_VERSION + 1],
        )
        .unwrap();
        drop(meta);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.load().is_err());
        assert_eq!(
            hub.recovery_reason(),
            Some(zephium_core::ports::store::NEWER_SESSION_REASON)
        );
        assert!(hub.set_aside_recovery().is_err());
        assert!(hub.recovery_reason().is_some());
    }

    #[test]
    fn restarting_without_readable_names_still_keeps_one_default_profile() {
        let registry: HashSet<ProfileId> = [ProfileId::from(9), ProfileId::from(4)].into();
        let state = restarted_session(&registry, Some(b"{not json"));
        assert_eq!(state.profiles.len(), 2);
        assert_eq!(
            state
                .profiles
                .iter()
                .filter(|profile| profile.kind == ProfileKind::Default)
                .count(),
            1
        );
        assert!(state
            .profiles
            .iter()
            .any(|profile| profile.name == "Personal"));
    }

    #[test]
    fn unrelated_or_changed_recovery_markers_remain_read_only() {
        for case in ["other-reason", "changed-snapshot"] {
            let reason = if case == "other-reason" {
                "unrelated recovery"
            } else {
                REASON
            };
            let (dir, _state) = marked_directory(reason);
            let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            if case == "changed-snapshot" {
                meta.execute(
                    "UPDATE session_snapshot SET data = data || ' ' WHERE id = 1",
                    [],
                )
                .unwrap();
            }
            drop(meta);
            let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
            assert!(hub.recovery_reason().is_some(), "{case}");
            assert!(hub.load().is_err(), "{case}");
            let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
            let markers: i64 = meta
                .query_row("SELECT count(*) FROM session_recovery", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(markers, 1, "{case}");
        }
    }
}

#[cfg(test)]
mod unchanged_session_tests {
    use super::*;
    use zephium_core::session::PersistedProfile;

    #[test]
    fn an_identical_session_is_not_written_again_and_a_changed_one_is() {
        let mut hub = Hub::in_memory().unwrap();
        let mut session = SessionState {
            profiles: vec![PersistedProfile {
                id: 1.into(),
                name: "Personal".into(),
                kind: ProfileKind::Default,
            }],
            ..SessionState::default()
        };
        hub.save(&session).unwrap();
        let written = hub.meta.total_changes();
        hub.save(&session).unwrap();
        assert_eq!(hub.meta.total_changes(), written);
        session.profiles[0].name = "Home".into();
        hub.save(&session).unwrap();
        assert!(hub.meta.total_changes() > written);
        assert_eq!(hub.load().unwrap().unwrap().profiles[0].name, "Home");
    }
}

#[cfg(test)]
mod closed_session_wire_tests {
    use super::*;

    #[test]
    fn bounded_snapshot_decodes_old_closed_tabs_without_inventing_identity_or_time() {
        let profile = ProfileId::from(1);
        let space = SpaceId::from(2);
        let mut value = serde_json::json!({
            "profiles": [], "spaces": [], "items": [],
            "active_space": null, "active_item": null, "splits": null,
            "recently_closed": [{
                "profile": profile, "space": space,
                "url": "https://example.test/", "title": "Example", "zoom": 1.0
            }]
        });
        let old = decode_authoritative_snapshot(&value.to_string()).unwrap();
        assert_eq!(old.recently_closed[0].session_id, None);
        assert_eq!(old.recently_closed[0].closed_at_ms, None);

        value["recently_closed"][0]["session_id"] =
            serde_json::json!(zephium_core::ids::ClosedSessionId::from(3));
        assert!(decode_authoritative_snapshot(&value.to_string()).is_none());
        value["recently_closed"][0]["closed_at_ms"] = serde_json::json!(1_700_000_000_123_u64);
        assert!(decode_authoritative_snapshot(&value.to_string()).is_some());
        value["recently_closed"][0]["unknown"] = serde_json::json!(true);
        assert!(decode_authoritative_snapshot(&value.to_string()).is_none());
    }
}

#[cfg(test)]
mod space_look_wire_tests {
    use super::*;
    use zephium_core::spaces::{SpaceIcon, SpaceLook, SpaceTheme};

    fn session(look: SpaceLook) -> SessionState {
        let profile = ProfileId::from(1);
        let space = SpaceId::from(2);
        SessionState {
            profiles: vec![PersistedProfile {
                id: profile,
                name: "Personal".into(),
                kind: ProfileKind::Default,
            }],
            spaces: vec![
                PersistedSpace {
                    id: space,
                    profile,
                    name: "Home".into(),
                    look: SpaceLook::default(),
                },
                PersistedSpace {
                    id: SpaceId::from(3),
                    profile,
                    name: "Studio".into(),
                    look,
                },
            ],
            active_space: Some(SpaceId::from(3)),
            ..SessionState::default()
        }
    }

    #[test]
    fn spaces_keep_their_look_and_order_across_a_restart() {
        let look = SpaceLook {
            icon: Some(SpaceIcon::Emoji("🎨".into())),
            color: 4,
            theme: Some(SpaceTheme::Preset("nord".into())),
        };
        let dir = tempfile::tempdir().unwrap();
        let state = session(look.clone());
        Hub::open(dir.path().to_path_buf())
            .unwrap()
            .save(&state)
            .unwrap();
        let loaded = Hub::open(dir.path().to_path_buf())
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(loaded, state);
        assert_eq!(loaded.spaces[1].look, look);
        assert_eq!(loaded.active_space, Some(SpaceId::from(3)));
    }

    #[test]
    fn a_snapshot_from_before_looks_decodes_and_a_damaged_look_does_not() {
        let mut value = serde_json::to_value(session(SpaceLook::default())).unwrap();
        assert!(value["spaces"][1].get("look").is_none());
        let old = decode_authoritative_snapshot(&value.to_string()).unwrap();
        assert!(old.spaces.iter().all(|space| space.look.is_default()));

        value["spaces"][1]["look"] = serde_json::json!({ "icon": { "symbol": 3 }, "color": 2 });
        let decoded = decode_authoritative_snapshot(&value.to_string()).unwrap();
        assert_eq!(decoded.spaces[1].look.icon, Some(SpaceIcon::Symbol(3)));
        value["spaces"][1]["look"]["glow"] = serde_json::json!(true);
        assert!(decode_authoritative_snapshot(&value.to_string()).is_none());
        value["spaces"][1]["look"] = serde_json::json!({ "icon": { "emoji": "🎨".repeat(20) } });
        assert!(decode_authoritative_snapshot(&value.to_string()).is_none());
    }
}

impl Hub {
    pub(crate) fn save(&mut self, s: &SessionState) -> rusqlite::Result<()> {
        let prepared = self.prepare_session(s)?;
        if self
            .registry
            .difference(&prepared.registry)
            .next()
            .is_some()
        {
            return Err(invalid_data(
                "profile removal requires explicit deletion authorization",
            ));
        }
        // Most persists follow an action that changed nothing durable (a tab
        // focused again, a sidebar toggled back); the session already
        // committed is not written and synced again.
        if self.legacy_state_purged
            && prepared.registry == self.registry
            && self.committed_session.as_deref() == Some(prepared.snapshot.as_str())
        {
            return Ok(());
        }
        self.validate_session_transition(&prepared.registry)?;
        self.commit_prepared_session(prepared, None)
    }

    pub(super) fn prepare_session(&self, s: &SessionState) -> rusqlite::Result<PreparedSession> {
        if self.recovery_required.is_some() {
            return Err(invalid_data("session recovery mode is read-only"));
        }
        // Privacy is enforced again at the adapter boundary. Core normally
        // filters private profiles while constructing a snapshot, but a new
        // caller or regression must fail closed instead of silently restoring
        // an incognito profile as a persistent named profile.
        if s.profiles
            .iter()
            .any(|profile| profile.kind == ProfileKind::Incognito)
        {
            return Err(rusqlite::Error::InvalidParameterName(
                "incognito profiles cannot be persisted".into(),
            ));
        }
        let canonical = core_session::canonicalize(s.clone());
        if canonical != *s {
            return Err(invalid_data("refusing to persist a noncanonical session"));
        }
        let s = canonical;
        let snapshot = serde_json::to_string(&s)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        if snapshot.len() > MAX_SESSION_SNAPSHOT_BYTES {
            return Err(rusqlite::Error::InvalidParameterName(
                "session snapshot exceeds persistence limit".into(),
            ));
        }
        let registry: HashSet<ProfileId> = s.profiles.iter().map(|profile| profile.id).collect();
        Ok(PreparedSession {
            state: s,
            snapshot,
            registry,
        })
    }

    pub(super) fn validate_session_transition(
        &self,
        next_registry: &HashSet<ProfileId>,
    ) -> rusqlite::Result<()> {
        let pending_deletions = self.profile_deletion_journal_entries()?;
        if pending_deletions
            .iter()
            .any(|deletion| next_registry.contains(&deletion.profile))
        {
            return Err(invalid_data(
                "active profile collides with pending deletion authorization",
            ));
        }
        if let Some(dir) = &self.dir {
            for added in next_registry.difference(&self.registry) {
                if profile_artifacts_exist(dir, *added)? {
                    return Err(invalid_data(
                        "new profile id collides with unclaimed on-disk data",
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn commit_prepared_session(
        &mut self,
        prepared: PreparedSession,
        authorize_deletion: Option<ProfileId>,
    ) -> rusqlite::Result<()> {
        let PreparedSession {
            state,
            snapshot,
            registry,
        } = prepared;
        let tx = self.meta.transaction()?;
        Self::validate_blocker_cohort_before_session_commit(&tx, &self.registry)?;
        let backup_saved_at = now_secs();
        let mut inserted_backup = false;
        if let Some(source) = &self.pending_session_backup {
            inserted_backup = tx.execute(
                "INSERT INTO session_snapshot_backup(id, saved_at, schema_version, data)
                 VALUES (1, ?1, ?2, ?3) ON CONFLICT(id) DO NOTHING",
                params![backup_saved_at, source.version, source.data],
            )? == 1;
        }
        if let Some(profile) = authorize_deletion {
            // Establish the deletion anchor before removing the active-profile
            // anchor. The surrounding transaction keeps the temporary overlap
            // invisible and rolls both changes back together.
            let inserted = tx.execute(
                "INSERT INTO profile_deletion_journal(profile_id, authorized_at)
                 VALUES (?1, ?2)",
                params![profile.to_string(), now_secs()],
            )?;
            if inserted != 1 {
                return Err(invalid_data(
                    "profile deletion authorization was not inserted exactly once",
                ));
            }
        }
        {
            let mut ins = tx.prepare_cached(
                "INSERT INTO profiles(id, name, kind, position) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET
                     name = excluded.name,
                     kind = excluded.kind,
                     position = excluded.position",
            )?;
            for (i, p) in state.profiles.iter().enumerate() {
                ins.execute(params![
                    p.id.to_string(),
                    p.name,
                    kind_to_str(p.kind).ok_or_else(|| {
                        invalid_data("incognito profile reached persistent transaction")
                    })?,
                    i as i64
                ])?;
            }
        }
        for removed in self.registry.difference(&registry) {
            let deleted =
                tx.execute("DELETE FROM profiles WHERE id = ?1", [removed.to_string()])?;
            if deleted != 1 {
                return Err(invalid_data(
                    "profile registry changed during authoritative session commit",
                ));
            }
        }
        Self::reconcile_blocker_cohort_for_session_commit(&tx, &registry)?;
        let last = state
            .active_space
            .and_then(|sp| state.spaces.iter().find(|x| x.id == sp))
            .map(|x| x.profile)
            .or_else(|| state.profiles.first().map(|p| p.id));
        tx.execute(
            "INSERT INTO state(id, last_profile) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET last_profile = ?1",
            params![last.map(|p| p.to_string())],
        )?;
        // The complete restorable session has one authoritative transaction.
        // Per-profile databases remain isolation roots for history/favicons,
        // but are no longer part of a multi-file snapshot commit.
        tx.execute(
            "INSERT INTO session_snapshot(id, schema_version, data) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET schema_version = ?1, data = ?2",
            params![SESSION_SCHEMA_VERSION, snapshot],
        )?;
        tx.commit()?;
        self.pending_session_backup = None;
        if inserted_backup {
            self.session_backup_expiry = Some(SessionBackupExpiry {
                saved_at: backup_saved_at,
                expires_at: std::time::Instant::now()
                    + std::time::Duration::from_secs(SESSION_BACKUP_LIFETIME_SECS as u64),
                retry_at: None,
            });
        }
        self.committed_session = Some(snapshot);
        #[cfg(test)]
        if authorize_deletion.is_some()
            && std::mem::take(&mut self.ambiguous_profile_deletion_commit_once)
        {
            // Model an OS/SQLite commit result whose durable outcome cannot be
            // inferred from the returned error. The transaction is committed,
            // but process-local registry state has deliberately not advanced.
            return Err(invalid_data(
                "injected ambiguous profile-deletion commit outcome",
            ));
        }
        self.registry = registry;
        if !self.legacy_state_purged {
            match self.purge_legacy_profile_state() {
                Ok(()) => self.legacy_state_purged = true,
                Err(error) => {
                    // The authoritative transaction is already durable. Do
                    // not report it as failed and tempt a caller to make an
                    // unsafe assumption; retry one-time legacy cleanup later.
                    eprintln!("store: deferred legacy profile cleanup failed: {error}");
                }
            }
        }
        // Release removed connections immediately. Their files remain until
        // exact journal authorization plus native-erasure proof permits purge.
        self.profiles
            .retain(|profile, _| self.registry.contains(profile));
        Ok(())
    }

    pub(crate) fn load(&mut self) -> rusqlite::Result<Option<SessionState>> {
        if self.recovery_required.is_some() {
            return Err(invalid_data("authoritative session requires recovery"));
        }
        let authoritative = self
            .meta
            .query_row(
                "SELECT schema_version,
                        length(CAST(data AS BLOB)),
                        CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN data END
                 FROM session_snapshot WHERE id = 1",
                [MAX_SESSION_SNAPSHOT_BYTES as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((version, bytes, data)) = authoritative {
            if !readable_session_version(version) {
                self.quarantine_authoritative(
                    if version > SESSION_SCHEMA_VERSION {
                        zephium_core::ports::store::NEWER_SESSION_REASON
                    } else {
                        "unsupported authoritative session schema"
                    },
                    version,
                    data.as_deref().map(str::as_bytes),
                )?;
                return Err(invalid_data("unsupported session snapshot schema"));
            }
            let Some(data) = data else {
                let _ = bytes;
                self.quarantine_authoritative(
                    "authoritative session exceeds persistence limit",
                    version,
                    None,
                )?;
                return Err(invalid_data("session snapshot exceeds persistence limit"));
            };
            let state = match decode_snapshot(&data, version) {
                Some(state) => state,
                None => {
                    self.quarantine_authoritative(
                        "corrupt authoritative session snapshot",
                        version,
                        Some(data.as_bytes()),
                    )?;
                    return Err(invalid_data("corrupt authoritative session snapshot"));
                }
            };
            if self.validate_authoritative_registry(&state).is_err() {
                self.quarantine_authoritative(
                    "authoritative snapshot does not match profile registry",
                    version,
                    Some(data.as_bytes()),
                )?;
                return Err(invalid_data(
                    "authoritative snapshot does not match profile registry",
                ));
            }
            let canonical = core_session::canonicalize(state.clone());
            if canonical != state || version != SESSION_SCHEMA_VERSION {
                // Restore compatibility repairs immediately, but retain their
                // exact source until the replacing transaction backs it up.
                self.pending_session_backup = Some(SessionBackupSource {
                    version,
                    data: data.into_bytes(),
                    canonical: serde_json::to_string(&canonical)
                        .map_err(|_| invalid_data("session backup repair could not be encoded"))?,
                });
            }
            return Ok(Some(canonical));
        }

        // One-time compatibility reader for databases created before the
        // atomic meta snapshot migration. The next successful save publishes
        // the complete session into session_snapshot.
        let profiles: Vec<PersistedProfile> = {
            let mut stmt = self.meta.prepare_cached(
                "SELECT CASE WHEN length(CAST(id AS BLOB)) <= 26 THEN id END,
                            CASE WHEN length(CAST(name AS BLOB)) <= ?1 THEN name END,
                            CASE WHEN length(CAST(kind AS BLOB)) <= 16 THEN kind END
                     FROM profiles ORDER BY position, id",
            )?;
            let rows = stmt.query_map([MAX_NAME_BYTES as i64], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?;
            let mut profiles = Vec::with_capacity(self.registry.len());
            let mut loaded = HashSet::with_capacity(self.registry.len());
            for row in rows {
                let (id, name, kind) = row?;
                let id =
                    id.ok_or_else(|| invalid_data("compatibility profile id exceeds limit"))?;
                let name =
                    name.ok_or_else(|| invalid_data("compatibility profile name exceeds limit"))?;
                let kind =
                    kind.ok_or_else(|| invalid_data("compatibility profile kind exceeds limit"))?;
                let id = ProfileId::parse(&id)
                    .filter(|profile| profile.to_string() == id)
                    .ok_or_else(|| invalid_data("compatibility profile has an invalid id"))?;
                let kind = kind_from_str(&kind)
                    .ok_or_else(|| invalid_data("compatibility profile has an invalid kind"))?;
                if !self.registry.contains(&id) || !loaded.insert(id) {
                    return Err(invalid_data("compatibility profile registry mismatch"));
                }
                profiles.push(PersistedProfile { id, name, kind });
            }
            if loaded != self.registry {
                return Err(invalid_data("compatibility profile registry is incomplete"));
            }
            profiles
        };
        if profiles.is_empty() {
            return Ok(None);
        }
        let last_row = self
            .meta
            .query_row(
                "SELECT last_profile IS NULL,
                        CASE
                            WHEN length(CAST(last_profile AS BLOB)) <= 26 THEN last_profile
                        END
                 FROM state WHERE id = 1",
                [],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let fallback = profiles.first().map(|profile| profile.id);
        let last = match last_row {
            None => return Err(invalid_data("compatibility state row is missing")),
            Some((true, None)) => fallback,
            Some((false, Some(raw))) => {
                let profile = ProfileId::parse(&raw)
                    .filter(|profile| profile.to_string() == raw)
                    .filter(|profile| self.registry.contains(profile))
                    .ok_or_else(|| invalid_data("compatibility state has an invalid profile"))?;
                Some(profile)
            }
            _ => {
                return Err(invalid_data(
                    "compatibility state profile exceeds persistence limit",
                ));
            }
        };

        let mut out = SessionState {
            profiles,
            ..Default::default()
        };
        let ids: Vec<ProfileId> = out.profiles.iter().map(|p| p.id).collect();
        for id in ids {
            let space_budget = MAX_SESSION_SPACES.saturating_sub(out.spaces.len());
            let item_budget = MAX_SESSION_ITEMS.saturating_sub(out.items.len());
            self.load_profile(id, &mut out, last == Some(id), space_budget, item_budget)?;
        }
        // Those files kept one set of Essentials per profile, which the
        // loader put in the profile's first space; every other space of the
        // profile gets a copy.
        let homes: HashMap<ProfileId, SpaceId> = out
            .profiles
            .iter()
            .filter_map(|profile| {
                Some((
                    profile.id,
                    essentials::home_space(&out.spaces, profile.id, None)?,
                ))
            })
            .collect();
        out.items = essentials::spread(std::mem::take(&mut out.items), &out.spaces, &homes);
        // Legacy profile files store positions per container, while the
        // authoritative snapshot has one canonical container order: every
        // space's Essentials, then its pinned/today sections. Reorder only
        // whole already-validated containers; do not use canonicalization
        // itself to drop or repair source rows.
        let mut by_placement: HashMap<Placement, Vec<PersistedItem>> = HashMap::new();
        for item in std::mem::take(&mut out.items) {
            by_placement.entry(item.placement).or_default().push(item);
        }
        for space in &out.spaces {
            for placement in core_session::space_placements(space.id) {
                if let Some(mut items) = by_placement.remove(&placement) {
                    out.items.append(&mut items);
                }
            }
        }
        if !by_placement.is_empty() {
            return Err(invalid_data(
                "compatibility session contains an unowned item placement",
            ));
        }
        let canonical = core_session::canonicalize(out.clone());
        if canonical != out {
            return Err(invalid_data(
                "compatibility session is not in exact canonical form",
            ));
        }
        Ok(Some(out))
    }

    pub(crate) fn recovery_reason(&self) -> Option<&str> {
        self.recovery_required.as_deref()
    }

    /// Lifts a recovery marker this build can see past, when its preserved
    /// bytes still equal the current bounded snapshot and decode against the
    /// registry. Two reasons qualify. Earlier builds put a valid session that
    /// was not in the then-exact canonical form (an older QA build's Settings
    /// tab, or rules that tightened later) into recovery; loading restores
    /// the canonical form and the next save publishes it. And an older build
    /// keeps a newer build's session for it: once a build that reads the
    /// session is back, it opens again.
    pub(super) fn recover_liftable_quarantine(&mut self) -> rusqlite::Result<bool> {
        const CANONICAL_FORM: &str = "authoritative session is not in exact canonical form";
        let Some(reason) = self.recovery_required.clone().filter(|reason| {
            reason == CANONICAL_FORM || reason == zephium_core::ports::store::NEWER_SESSION_REASON
        }) else {
            return Ok(false);
        };
        let reason = reason.as_str();
        let marker = self
            .meta
            .query_row(
                "SELECT schema_version,
                        CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN data END
                 FROM session_recovery WHERE id = 1 AND reason = ?2",
                params![MAX_SESSION_SNAPSHOT_BYTES as i64, reason],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                    ))
                },
            )
            .optional()?;
        let Some((Some(version), Some(marked_bytes))) = marker else {
            return Ok(false);
        };
        if !readable_session_version(version) {
            return Ok(false);
        }
        let snapshot = self
            .meta
            .query_row(
                "SELECT schema_version,
                        CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN data END
                 FROM session_snapshot WHERE id = 1",
                [MAX_SESSION_SNAPSHOT_BYTES as i64],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let Some((current_version, Some(data))) = snapshot else {
            return Ok(false);
        };
        if current_version != version || data.as_bytes() != marked_bytes {
            return Ok(false);
        }
        let Some(state) = decode_snapshot(&data, version) else {
            return Ok(false);
        };
        // Loading now restores such a session in today's form, so a
        // quarantine for this reason alone is lifted once the preserved
        // bytes still decode and match the registry.
        if self.validate_authoritative_registry(&state).is_err() {
            return Ok(false);
        }
        let removed = self.meta.execute(
            "DELETE FROM session_recovery
             WHERE id = 1 AND reason = ?1 AND schema_version = ?2 AND data = ?3
               AND EXISTS (
                   SELECT 1 FROM session_snapshot
                   WHERE id = 1 AND schema_version = ?2 AND CAST(data AS BLOB) = ?3
               )",
            params![reason, version, marked_bytes],
        )?;
        if removed != 1 {
            return Ok(false);
        }
        self.recovery_required = None;
        Ok(true)
    }

    pub(super) fn has_authoritative_snapshot(&self) -> rusqlite::Result<bool> {
        self.meta.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_snapshot WHERE id = 1)",
            [],
            |row| row.get(0),
        )
    }

    pub(super) fn quarantine_current_authoritative(
        &mut self,
        reason: &str,
    ) -> rusqlite::Result<()> {
        let (schema_version, data) = self
            .meta
            .query_row(
                "SELECT schema_version,
                        CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN data END
                 FROM session_snapshot WHERE id = 1",
                [MAX_SESSION_SNAPSHOT_BYTES as i64],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| invalid_data("authoritative session snapshot is absent"))?;
        let data = data.ok_or_else(|| invalid_data("authoritative session exceeds limit"))?;
        self.quarantine_authoritative(reason, schema_version, Some(data.as_bytes()))
    }

    fn quarantine_authoritative(
        &mut self,
        reason: &str,
        schema_version: i64,
        data: Option<&[u8]>,
    ) -> rusqlite::Result<()> {
        let detected_at = now_secs();
        self.meta.execute(
            "INSERT INTO session_recovery(id, detected_at, reason, schema_version, data)
             VALUES (1, ?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO NOTHING",
            params![detected_at, reason, schema_version, data],
        )?;
        // Preserve the first diagnosis and exact bounded bytes. A later open
        // must stay in recovery mode until a dedicated recovery operation
        // explicitly resolves the marker.
        self.recovery_required = recovery_reason(&self.meta)?.or_else(|| Some(reason.into()));
        Ok(())
    }

    /// Starts again from a session the store could not restore, without
    /// losing what it held. The preserved bytes go to a file beside the
    /// databases, and the session restarts from the profile registry with no
    /// tabs, each profile named as the old snapshot still names it, so every
    /// profile keeps its history, bookmarks and settings. A store left in
    /// recovery could otherwise never open again.
    pub(crate) fn set_aside_recovery(&mut self) -> rusqlite::Result<Option<PathBuf>> {
        let Some(reason) = self.recovery_required.clone() else {
            return Err(invalid_data("no session recovery is pending"));
        };
        // A newer build's session opens again once that build is back.
        if reason == zephium_core::ports::store::NEWER_SESSION_REASON {
            return Err(invalid_data(
                zephium_core::ports::store::NEWER_SESSION_REASON,
            ));
        }
        let preserved: Option<Vec<u8>> = self
            .meta
            .query_row(
                "SELECT data FROM session_recovery WHERE id = 1",
                [],
                |row| row.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten()
            .or(self
                .meta
                .query_row(
                    "SELECT CASE WHEN length(CAST(data AS BLOB)) <= ?1
                            THEN CAST(data AS BLOB) END
                     FROM session_snapshot WHERE id = 1",
                    [MAX_SET_ASIDE_BYTES as i64],
                    |row| row.get::<_, Option<Vec<u8>>>(0),
                )
                .optional()?
                .flatten());
        let file = match (&self.dir, &preserved) {
            (Some(dir), Some(bytes)) => Some(
                write_set_aside(dir, bytes)
                    .map_err(|error| invalid_data(&format!("session set-aside failed: {error}")))?,
            ),
            _ => None,
        };
        let restarted = restarted_session(&self.registry, preserved.as_deref());
        self.recovery_required = None;
        if let Err(error) = self.save(&restarted) {
            self.recovery_required = Some(reason);
            return Err(error);
        }
        self.meta
            .execute("DELETE FROM session_recovery WHERE id = 1", [])?;
        Ok(file)
    }

    fn validate_authoritative_registry(&self, state: &SessionState) -> rusqlite::Result<()> {
        if state.profiles.len() > MAX_SESSION_PROFILES
            || state
                .profiles
                .iter()
                .any(|profile| profile.kind == ProfileKind::Incognito)
        {
            return Err(invalid_data("invalid profiles in authoritative snapshot"));
        }
        let snapshot: HashSet<ProfileId> =
            state.profiles.iter().map(|profile| profile.id).collect();
        if snapshot.len() != state.profiles.len() || snapshot != self.registry {
            return Err(invalid_data(
                "authoritative snapshot does not match profile registry",
            ));
        }
        Ok(())
    }
}

/// The largest unreadable snapshot copied out when no recovery bytes exist.
const MAX_SET_ASIDE_BYTES: usize = 64 * 1024 * 1024;

/// Writes `bytes` to a new private file beside the databases, durably, and
/// returns its path.
fn write_set_aside(dir: &std::path::Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let stamp = now_secs();
    let mut path = dir.join(format!("recovered-session-{stamp}.json"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("recovered-session-{stamp}-{n}.json"));
        n += 1;
    }
    let temporary = path.with_extension("json.partial");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, &path)?;
    #[cfg(unix)]
    if let Ok(directory) = std::fs::File::open(dir) {
        let _ = directory.sync_all();
    }
    Ok(path)
}

/// A session with every registered profile and nothing else. Names and kinds
/// come from the unreadable snapshot where it still holds them; exactly one
/// profile is the default.
fn restarted_session(registry: &HashSet<ProfileId>, preserved: Option<&[u8]>) -> SessionState {
    let named: Vec<(ProfileId, String, ProfileKind)> = preserved
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .and_then(|value| value.get("profiles")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|profile| {
            let raw = profile.get("id")?.as_str()?;
            let id = ProfileId::parse(raw).filter(|id| id.to_string() == raw)?;
            let name = profile.get("name")?.as_str()?.trim();
            let kind = match profile.get("kind")?.as_str()? {
                "default" | "Default" => ProfileKind::Default,
                _ => ProfileKind::Named,
            };
            (registry.contains(&id) && !name.is_empty() && name.len() <= MAX_NAME_BYTES)
                .then(|| (id, name.to_owned(), kind))
        })
        .collect();
    let mut ids: Vec<ProfileId> = named.iter().map(|(id, ..)| *id).collect();
    let mut rest: Vec<ProfileId> = registry
        .iter()
        .filter(|id| !ids.contains(id))
        .copied()
        .collect();
    rest.sort_by_key(ToString::to_string);
    ids.dedup();
    ids.extend(rest);
    let mut defaulted = false;
    let profiles = ids
        .into_iter()
        .map(|id| {
            let (name, kind) = named
                .iter()
                .find(|(known, ..)| *known == id)
                .map(|(_, name, kind)| (name.clone(), *kind))
                .unwrap_or_else(|| ("Profile".to_owned(), ProfileKind::Named));
            let kind = if kind == ProfileKind::Default && !defaulted {
                defaulted = true;
                kind
            } else {
                ProfileKind::Named
            };
            PersistedProfile { id, name, kind }
        })
        .collect::<Vec<_>>();
    let mut profiles = profiles;
    if !defaulted {
        if let Some(first) = profiles.first_mut() {
            first.kind = ProfileKind::Default;
            if first.name == "Profile" {
                first.name = "Personal".into();
            }
        }
    }
    core_session::canonicalize(SessionState {
        profiles,
        ..Default::default()
    })
}

fn kind_to_str(kind: ProfileKind) -> Option<&'static str> {
    match kind {
        ProfileKind::Default => Some("default"),
        ProfileKind::Named => Some("named"),
        ProfileKind::Incognito => None,
    }
}

fn kind_from_str(s: &str) -> Option<ProfileKind> {
    match s {
        "default" => Some(ProfileKind::Default),
        "named" => Some(ProfileKind::Named),
        _ => None,
    }
}

#[cfg(test)]
mod bounded_snapshot_tests {
    use super::*;

    fn snapshot_with_profiles(profiles: &str) -> String {
        format!(
            r#"{{"profiles":[{profiles}],"spaces":[],"items":[],"active_space":null,"active_item":null,"splits":null}}"#
        )
    }

    #[test]
    fn authoritative_collection_limit_is_enforced_by_the_sequence_visitor() {
        let profile = format!(
            r#"{{"id":"{}","name":"P","kind":"Default"}}"#,
            ProfileId::from(1)
        );
        let profiles = vec![profile; MAX_SESSION_PROFILES + 1].join(",");
        assert!(decode_authoritative_snapshot(&snapshot_with_profiles(&profiles)).is_none());
    }

    #[test]
    fn authoritative_nested_unknown_fields_are_not_silently_projected_away() {
        let profile = format!(
            r#"{{"id":"{}","name":"P","kind":"Default","future":true}}"#,
            ProfileId::from(1)
        );
        assert!(decode_authoritative_snapshot(&snapshot_with_profiles(&profile)).is_none());
    }

    #[test]
    fn near_snapshot_cap_oversized_string_is_rejected_by_lexical_preflight() {
        let name = "x".repeat(MAX_SESSION_SNAPSHOT_BYTES - 1024);
        let profile = format!(
            r#"{{"id":"{}","name":"{name}","kind":"Default"}}"#,
            ProfileId::from(1)
        );
        let snapshot = snapshot_with_profiles(&profile);
        assert!(snapshot.len() < MAX_SESSION_SNAPSHOT_BYTES);
        assert!(snapshot.len() > MAX_SESSION_SNAPSHOT_BYTES - 2048);
        assert!(decode_authoritative_snapshot(&snapshot).is_none());
    }
}

#[cfg(test)]
mod essentials_per_space_tests {
    use super::*;

    const PERSONAL: u128 = 1;
    const WORK_PROFILE: u128 = 2;
    const HOME: u128 = 11;
    const SECOND: u128 = 12;
    const WORK: u128 = 13;

    fn tab(id: u128, placement: Placement, url: &str) -> PersistedItem {
        PersistedItem {
            id: ItemId::from(id),
            parent: None,
            placement,
            kind: PersistedKind::Tab {
                url: url.into(),
                title: url.into(),
                zoom: 1.0,
                last_active_ms: None,
            },
        }
    }

    fn essentials(space: u128) -> Placement {
        Placement::Favorites {
            space: SpaceId::from(space),
        }
    }

    /// Two spaces in one profile, one in another, with the kept sites of the
    /// first profile in its second space and the other's in its only one.
    fn session(active: u128) -> SessionState {
        let space = |id, profile| PersistedSpace {
            id: SpaceId::from(id),
            profile: ProfileId::from(profile),
            name: "Space".into(),
            look: Default::default(),
        };
        let profile = |id, name: &str, kind| PersistedProfile {
            id: ProfileId::from(id),
            name: name.into(),
            kind,
        };
        SessionState {
            profiles: vec![
                profile(PERSONAL, "Personal", ProfileKind::Default),
                profile(WORK_PROFILE, "Work", ProfileKind::Named),
            ],
            spaces: vec![
                space(HOME, PERSONAL),
                space(SECOND, PERSONAL),
                space(WORK, WORK_PROFILE),
            ],
            items: vec![
                tab(
                    21,
                    Placement::Space {
                        space: SpaceId::from(HOME),
                        section: SpaceSection::Today,
                    },
                    "https://open.example/",
                ),
                tab(22, essentials(SECOND), "https://mail.example/"),
                tab(23, essentials(SECOND), "https://chat.example/"),
                tab(24, essentials(WORK), "https://tickets.example/"),
            ],
            active_space: Some(SpaceId::from(active)),
            active_item: Some(ItemId::from(22)),
            splits: None,
            recently_closed: Vec::new(),
        }
    }

    /// The same session as version 1 wrote it: each profile's kept sites are
    /// one set that names the profile.
    fn as_saved_by_version_one(state: &SessionState) -> String {
        let mut value = serde_json::to_value(state).unwrap();
        for item in value["items"].as_array_mut().unwrap() {
            let Some(favorites) = item["placement"].get_mut("Favorites") else {
                continue;
            };
            let space: SpaceId = serde_json::from_value(favorites["space"].clone()).unwrap();
            let owner = state.spaces.iter().find(|s| s.id == space).unwrap().profile;
            *favorites = serde_json::json!({ "profile": owner });
        }
        value.to_string()
    }

    fn rewind(hub: &mut Hub, state: &SessionState) {
        hub.meta
            .execute(
                "UPDATE session_snapshot SET schema_version = 1, data = ?1",
                [as_saved_by_version_one(state)],
            )
            .unwrap();
    }

    fn urls(state: &SessionState, placement: Placement) -> Vec<String> {
        state
            .items
            .iter()
            .filter(|item| item.placement == placement)
            .filter_map(|item| match &item.kind {
                PersistedKind::Tab { url, .. } => Some(url.clone()),
                _ => None,
            })
            .collect()
    }

    fn ids(state: &SessionState, placement: Placement) -> Vec<ItemId> {
        state
            .items
            .iter()
            .filter(|item| item.placement == placement)
            .map(|item| item.id)
            .collect()
    }

    #[test]
    fn upgrading_keeps_the_original_bytes_and_schema_once() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);
        let source = as_saved_by_version_one(&state).into_bytes();
        let restored = hub.load().unwrap().unwrap();
        hub.save(&restored).unwrap();
        let backup: (i64, Vec<u8>) = hub
            .meta
            .query_row(
                "SELECT schema_version, data FROM session_snapshot_backup WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(backup, (1, source));
        assert_eq!(hub.load().unwrap().unwrap(), restored);
        hub.save(&restored).unwrap();
        let count: i64 = hub
            .meta
            .query_row("SELECT count(*) FROM session_snapshot_backup", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn clearing_before_upgrade_cannot_recreate_the_old_source_backup() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);
        let restored = hub.load().unwrap().unwrap();
        assert!(hub.pending_session_backup.is_some());
        hub.discard_session_backup(ProfileId::from(PERSONAL))
            .unwrap();
        assert!(hub.pending_session_backup.is_none());
        assert_eq!(hub.load().unwrap().unwrap(), restored);
        assert!(hub.pending_session_backup.is_none());
        hub.save(&restored).unwrap();
        let count: i64 = hub
            .meta
            .query_row("SELECT count(*) FROM session_snapshot_backup", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn each_space_keeps_the_essentials_it_was_given() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();

        let loaded = hub.load().unwrap().unwrap();

        assert_eq!(loaded, state);
        assert!(urls(&loaded, essentials(HOME)).is_empty());
        assert_eq!(urls(&loaded, essentials(SECOND)).len(), 2);
    }

    #[test]
    fn a_session_saved_with_one_set_per_profile_gets_a_copy_in_every_space() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);

        let loaded = hub.load().unwrap().unwrap();

        for space in [HOME, SECOND] {
            assert_eq!(
                urls(&loaded, essentials(space)),
                ["https://mail.example/", "https://chat.example/"],
                "space {space} lost or reordered a kept site"
            );
        }
        assert_eq!(
            urls(&loaded, essentials(WORK)),
            ["https://tickets.example/"],
            "a profile's kept sites reach only its own spaces"
        );
        // The space the person was in keeps the originals, so what was in
        // front stays in front; the copies are new items.
        assert_eq!(
            ids(&loaded, essentials(SECOND)),
            [ItemId::from(22), ItemId::from(23)]
        );
        assert_eq!(loaded.active_item, Some(ItemId::from(22)));
        let mut every: Vec<ItemId> = loaded.items.iter().map(|item| item.id).collect();
        let count = every.len();
        every.sort();
        every.dedup();
        assert_eq!(every.len(), count, "an item is owned by two spaces");
        assert_eq!(count, 6);
    }

    #[test]
    fn the_originals_go_to_the_profiles_first_space_when_the_person_was_in_another_profile() {
        let mut hub = Hub::in_memory().unwrap();
        let mut state = session(WORK);
        state.active_item = Some(ItemId::from(24));
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);

        let loaded = hub.load().unwrap().unwrap();

        assert_eq!(
            ids(&loaded, essentials(HOME)),
            [ItemId::from(22), ItemId::from(23)]
        );
        assert_eq!(loaded.active_item, Some(ItemId::from(24)));
    }

    #[test]
    fn an_upgraded_session_is_saved_in_the_new_schema_and_not_copied_again() {
        let mut hub = Hub::in_memory().unwrap();
        let state = session(SECOND);
        hub.save(&state).unwrap();
        rewind(&mut hub, &state);

        let upgraded = hub.load().unwrap().unwrap();
        hub.save(&upgraded).unwrap();

        let version: i64 = hub
            .meta
            .query_row("SELECT schema_version FROM session_snapshot", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(version, SESSION_SCHEMA_VERSION);
        assert_eq!(hub.load().unwrap().unwrap(), upgraded);
    }

    #[test]
    fn a_schema_is_read_only_in_the_form_it_wrote() {
        let state = session(SECOND);
        let version_one = as_saved_by_version_one(&state);
        let version_two = serde_json::to_string(&state).unwrap();

        assert!(decode_snapshot(&version_one, 1).is_some());
        assert!(decode_snapshot(&version_two, 2).is_some());
        assert!(decode_snapshot(&version_one, 2).is_none());
        assert!(decode_snapshot(&version_two, 1).is_none());
    }

    #[test]
    fn essentials_naming_both_a_profile_and_a_space_are_not_a_session() {
        let state = session(SECOND);
        let mut value = serde_json::to_value(&state).unwrap();
        value["items"][1]["placement"]["Favorites"]["profile"] =
            serde_json::json!(ProfileId::from(PERSONAL));
        assert!(decode_snapshot(&value.to_string(), 2).is_none());
        assert!(decode_snapshot(&value.to_string(), 1).is_none());
    }
}
