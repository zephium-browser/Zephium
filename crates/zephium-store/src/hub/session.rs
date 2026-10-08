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
        profile: ProfileId,
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
        space.name.len() <= MAX_NAME_BYTES
    })
}

fn deserialize_bounded_items<'de, D>(deserializer: D) -> Result<Vec<BoundedItem>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    bounded_json::deserialize_bounded_vec(deserializer, MAX_SESSION_ITEMS, |item| {
        match &item.kind {
            BoundedKind::Folder { name } => name.len() <= MAX_NAME_BYTES,
            BoundedKind::Tab { url, title, zoom } => {
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

impl From<BoundedSessionState> for SessionState {
    fn from(value: BoundedSessionState) -> Self {
        Self {
            profiles: value
                .profiles
                .into_iter()
                .map(|profile| PersistedProfile {
                    id: profile.id,
                    name: profile.name,
                    kind: profile.kind,
                })
                .collect(),
            spaces: value
                .spaces
                .into_iter()
                .map(|space| PersistedSpace {
                    id: space.id,
                    profile: space.profile,
                    name: space.name,
                })
                .collect(),
            items: value
                .items
                .into_iter()
                .map(|item| PersistedItem {
                    id: item.id,
                    parent: item.parent,
                    placement: match item.placement {
                        BoundedPlacement::Favorites { profile } => Placement::Favorites { profile },
                        BoundedPlacement::Space { space, section } => {
                            Placement::Space { space, section }
                        }
                    },
                    kind: match item.kind {
                        BoundedKind::Folder { name } => PersistedKind::Folder { name },
                        BoundedKind::Tab { url, title, zoom } => {
                            PersistedKind::Tab { url, title, zoom }
                        }
                        BoundedKind::BrowserTab { page } => PersistedKind::BrowserTab { page },
                    },
                })
                .collect(),
            active_space: value.active_space,
            active_item: value.active_item,
            splits: value.splits.map(|pane| pane.0),
            recently_closed: value
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
                })
                .collect(),
        }
    }
}

fn decode_authoritative_snapshot(data: &str) -> Option<SessionState> {
    bounded_json::from_str::<BoundedSessionState>(data)
        .ok()
        .map(SessionState::from)
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
    fn a_session_saved_under_older_canonical_rules_recovers_in_canonical_form() {
        let (dir, mut state) = marked_directory(REASON);
        let meta = Connection::open(dir.path().join("meta.sqlite")).unwrap();
        state.active_item = Some(ItemId::from(999_999));
        let data = serde_json::to_string(&state).unwrap();
        meta.execute(
            "UPDATE session_snapshot SET data = ?1 WHERE id = 1",
            [&data],
        )
        .unwrap();
        meta.execute(
            "UPDATE session_recovery SET data = ?1 WHERE id = 1",
            [data.as_bytes()],
        )
        .unwrap();
        drop(meta);

        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert!(hub.recovery_reason().is_none());
        let loaded = hub.load().unwrap().expect("the session is restored");
        assert_eq!(loaded, core_session::canonicalize(state));
        assert_ne!(loaded.active_item, Some(ItemId::from(999_999)));
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
            if version != SESSION_SCHEMA_VERSION {
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
            let state = match decode_authoritative_snapshot(&data) {
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
            // Canonical form is the sanitizer, and its rules tighten between
            // releases. A session saved by an earlier build that decodes and
            // matches the registry is restored in today's canonical form;
            // locking the store over it would cost the person every tab.
            let canonical = core_session::canonicalize(state.clone());
            if canonical != state {
                eprintln!("store: restoring a session saved under older canonical rules");
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
        // Legacy profile files store positions per container, while the
        // authoritative snapshot has one canonical cross-profile container
        // order: every profile's favorites, then every space's pinned/today
        // sections. Reorder only whole already-validated containers; do not use
        // canonicalization itself to drop or repair source rows.
        let mut by_placement: HashMap<Placement, Vec<PersistedItem>> = HashMap::new();
        for item in std::mem::take(&mut out.items) {
            by_placement.entry(item.placement).or_default().push(item);
        }
        for profile in &out.profiles {
            if let Some(mut items) = by_placement.remove(&Placement::Favorites {
                profile: profile.id,
            }) {
                out.items.append(&mut items);
            }
        }
        for space in &out.spaces {
            for section in [SpaceSection::Pinned, SpaceSection::Today] {
                if let Some(mut items) = by_placement.remove(&Placement::Space {
                    space: space.id,
                    section,
                }) {
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

    /// Earlier builds put a valid session that was not in the then-exact
    /// canonical form (an older QA build's Settings tab, or rules that
    /// tightened later) into read-only recovery. Clear that specific marker
    /// when its preserved bytes still equal the current bounded snapshot;
    /// loading restores the canonical form and the next save publishes it.
    pub(super) fn recover_canonical_form_quarantine(&mut self) -> rusqlite::Result<bool> {
        const REASON: &str = "authoritative session is not in exact canonical form";
        if self.recovery_required.as_deref() != Some(REASON) {
            return Ok(false);
        }
        let marker = self
            .meta
            .query_row(
                "SELECT schema_version,
                        CASE WHEN length(CAST(data AS BLOB)) <= ?1 THEN data END
                 FROM session_recovery WHERE id = 1 AND reason = ?2",
                params![MAX_SESSION_SNAPSHOT_BYTES as i64, REASON],
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
        if version != SESSION_SCHEMA_VERSION {
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
        let Some(state) = decode_authoritative_snapshot(&data) else {
            return Ok(false);
        };
        // Loading now restores such a session in today's canonical form, so
        // any quarantine for this reason alone is lifted once the preserved
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
            params![REASON, version, marked_bytes],
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
