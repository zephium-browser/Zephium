//! Crash-resumable profile deletion authorization, reconciliation, and purge.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use zephium_core::ports::store::{PendingProfileDeletion, ProfileDeletionAuthorizeOutcome};

use super::filesystem::profile_artifacts_absent;
use super::*;

const MAX_PROFILE_DELETION_JOURNAL: usize = core_session::MAX_SESSION_PROFILES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ProfileDeletionJournalEntry {
    pub(super) profile: ProfileId,
    native_erasure_verified: bool,
    local_unlink_process: Option<ProfileId>,
}

impl ProfileDeletionJournalEntry {
    fn pending(self) -> PendingProfileDeletion {
        PendingProfileDeletion {
            profile: self.profile,
            native_erasure_verified: self.native_erasure_verified,
        }
    }
}

impl Hub {
    #[cfg(test)]
    pub(crate) fn open_for_new_process(dir: PathBuf) -> rusqlite::Result<Self> {
        Self::open_with_deletion_process_generation(dir, new_deletion_process_generation())
    }

    /// Atomically publishes the exact canonical post-removal snapshot and its
    /// deletion journal row. No native erasure is authorized before this
    /// transaction commits.
    pub(crate) fn authorize_profile_deletion(
        &mut self,
        profile: ProfileId,
        filtered: &SessionState,
    ) -> rusqlite::Result<ProfileDeletionAuthorizeOutcome> {
        let prepared = self.prepare_session(filtered)?;
        if prepared.registry.contains(&profile) {
            return Ok(ProfileDeletionAuthorizeOutcome::InvalidSession);
        }

        let journal = self.profile_deletion_journal_entries()?;
        let already_authorized = journal.iter().any(|deletion| deletion.profile == profile);
        if self.registry.contains(&profile) {
            let mut expected = self.registry.clone();
            expected.remove(&profile);
            if prepared.registry != expected {
                return Ok(ProfileDeletionAuthorizeOutcome::SessionConflict);
            }
            if journal.len() >= MAX_PROFILE_DELETION_JOURNAL {
                return Err(invalid_data(
                    "profile deletion journal exceeds persistence limit",
                ));
            }
            self.validate_session_transition(&prepared.registry)?;
            // Only the live, validated deletion request may establish this
            // protected cleanup intent. Imported AppData tombstones alone
            // cannot authorize deleting bodies from the private Work journal.
            #[cfg(all(target_os = "windows", feature = "work-execution"))]
            self.authorize_windows_work_artifact_deletion(profile)?;
            self.commit_prepared_session(prepared, Some(profile))?;
            Ok(ProfileDeletionAuthorizeOutcome::Authorized)
        } else if already_authorized {
            // A crash/retry may reach this path after the atomic transaction
            // but before native erasure was started or acknowledged. Allow a
            // newer exact snapshot of the same registry to commit while
            // preserving the original authorization row.
            if prepared.registry != self.registry {
                return Ok(ProfileDeletionAuthorizeOutcome::SessionConflict);
            }
            self.validate_session_transition(&prepared.registry)?;
            self.commit_prepared_session(prepared, None)?;
            Ok(ProfileDeletionAuthorizeOutcome::AlreadyAuthorized)
        } else {
            Ok(ProfileDeletionAuthorizeOutcome::NotRegistered)
        }
    }

    pub(super) fn profile_deletion_journal_entries(
        &self,
    ) -> rusqlite::Result<Vec<ProfileDeletionJournalEntry>> {
        let count =
            self.meta
                .query_row("SELECT count(*) FROM profile_deletion_journal", [], |row| {
                    row.get::<_, i64>(0)
                })?;
        if !(0..=MAX_PROFILE_DELETION_JOURNAL as i64).contains(&count) {
            return Err(invalid_data(
                "profile deletion journal exceeds persistence limit",
            ));
        }
        let mut statement = self.meta.prepare(
            "SELECT CASE
                        WHEN length(CAST(profile_id AS BLOB)) <= 26 THEN profile_id
                    END,
                    native_erasure_verified,
                    local_unlink_process IS NULL,
                    CASE
                        WHEN length(CAST(local_unlink_process AS BLOB)) <= 26
                        THEN local_unlink_process
                    END
             FROM profile_deletion_journal
             ORDER BY authorized_at, profile_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut profiles = Vec::with_capacity(count as usize);
        for row in rows {
            let (raw, native_erasure_verified, local_unlink_is_null, local_unlink_process) = row?;
            let raw = raw.ok_or_else(|| invalid_data("profile deletion id exceeds limit"))?;
            let profile = ProfileId::parse(&raw)
                .filter(|profile| profile.to_string() == raw)
                .ok_or_else(|| invalid_data("profile deletion journal has invalid id"))?;
            let native_erasure_verified = match native_erasure_verified {
                0 => false,
                1 => true,
                _ => {
                    return Err(invalid_data(
                        "profile deletion journal has invalid native-erasure state",
                    ))
                }
            };
            let local_unlink_process = match (local_unlink_is_null, local_unlink_process) {
                (1, None) => None,
                (0, Some(raw)) => {
                    let generation = ProfileId::parse(&raw)
                        .filter(|generation| generation.to_string() == raw)
                        .ok_or_else(|| {
                            invalid_data("profile deletion journal has invalid process generation")
                        })?;
                    Some(generation)
                }
                _ => {
                    return Err(invalid_data(
                        "profile deletion journal has invalid local-unlink state",
                    ));
                }
            };
            if local_unlink_process.is_some() && !native_erasure_verified {
                return Err(invalid_data(
                    "profile deletion journal completed local unlink without native proof",
                ));
            }
            if self.registry.contains(&profile) {
                return Err(invalid_data(
                    "profile deletion journal overlaps active registry",
                ));
            }
            profiles.push(ProfileDeletionJournalEntry {
                profile,
                native_erasure_verified,
                local_unlink_process,
            });
        }
        if profiles.len() != count as usize {
            return Err(invalid_data(
                "profile deletion journal changed while loading",
            ));
        }
        Ok(profiles)
    }

    fn pending_profile_deletion_entries(
        entries: &[ProfileDeletionJournalEntry],
    ) -> Vec<PendingProfileDeletion> {
        entries
            .iter()
            .copied()
            // A Windows unlink is reported complete to the current process,
            // while its authorization remains internally durable until a
            // later process start verifies absence. Do not make the shell
            // repeat a completed local operation during the same run.
            .filter(|entry| entry.local_unlink_process.is_none())
            .map(ProfileDeletionJournalEntry::pending)
            .collect()
    }

    pub(super) fn reconcile_completed_profile_deletion_tombstones(
        &mut self,
    ) -> rusqlite::Result<()> {
        let Some(dir) = self.dir.clone() else {
            return Ok(());
        };
        let completed: Vec<_> = self
            .profile_deletion_journal_entries()?
            .into_iter()
            .filter(|entry| entry.local_unlink_process != Some(self.deletion_process_generation))
            .filter(|entry| entry.local_unlink_process.is_some())
            .collect();
        if completed.is_empty() {
            return Ok(());
        }

        // Resolve filesystem truth before taking SQLite's write transaction.
        // A path that exists in any form (regular file, directory, symlink or
        // reparse-point-like entry) is not considered absent.
        let mut resolutions = Vec::with_capacity(completed.len());
        for entry in completed {
            let Some(prior_process) = entry.local_unlink_process else {
                return Err(invalid_data(
                    "completed profile deletion lost its process generation",
                ));
            };
            // The protected Windows journal is a separate database. Its
            // idempotent purge must settle while this exact durable deletion
            // obligation still exists, before removing the final tombstone.
            #[cfg(all(target_os = "windows", feature = "work-execution"))]
            self.purge_windows_work_artifacts(entry.profile)?;
            resolutions.push((
                entry.profile,
                prior_process,
                profile_artifacts_absent(&dir, entry.profile)?,
            ));
        }

        let tx = self.meta.transaction()?;
        for (profile, prior_process, absent) in resolutions {
            let changed = if absent {
                tx.execute(
                    "DELETE FROM profile_deletion_journal
                     WHERE profile_id = ?1
                       AND native_erasure_verified = 1
                       AND local_unlink_process = ?2",
                    params![profile.to_string(), prior_process.to_string()],
                )?
            } else {
                // The filesystem did not preserve the prior unlink across
                // restart. Keep native proof, reopen only the idempotent local
                // phase, and retain the original deletion authorization.
                tx.execute(
                    "UPDATE profile_deletion_journal
                     SET local_unlink_process = NULL
                     WHERE profile_id = ?1
                       AND native_erasure_verified = 1
                       AND local_unlink_process = ?2",
                    params![profile.to_string(), prior_process.to_string()],
                )?
            };
            if changed != 1 {
                return Err(invalid_data(
                    "profile deletion tombstone changed during restart reconciliation",
                ));
            }
        }
        tx.commit()
    }

    #[cfg(test)]
    pub(crate) fn pending_profile_deletions(
        &self,
    ) -> rusqlite::Result<Vec<PendingProfileDeletion>> {
        Ok(Self::pending_profile_deletion_entries(
            &self.profile_deletion_journal_entries()?,
        ))
    }

    #[cfg(test)]
    pub(crate) fn completed_profile_deletion_tombstones(&self) -> rusqlite::Result<Vec<ProfileId>> {
        Ok(self
            .profile_deletion_journal_entries()?
            .into_iter()
            .filter_map(|entry| {
                entry
                    .local_unlink_process
                    .is_some()
                    .then_some(entry.profile)
            })
            .collect())
    }

    /// Tombstoned profiles still own download staging until native erasure is
    /// proven. Only the internal cleanup path can access their download records.
    pub(super) fn download_cleanup_deletions(&self) -> rusqlite::Result<Vec<ProfileId>> {
        Ok(self
            .profile_deletion_journal_entries()?
            .into_iter()
            .filter(|entry| !entry.native_erasure_verified && entry.local_unlink_process.is_none())
            .map(|entry| entry.profile)
            .collect())
    }

    /// Refreshes process-local registry truth from the durable transaction
    /// before interpreting the deletion journal. This is required after a
    /// commit error: SQLite/OS failures can leave the caller unable to infer
    /// whether COMMIT reached stable storage.
    pub(crate) fn reconcile_profile_deletion_journal(
        &mut self,
    ) -> rusqlite::Result<Vec<PendingProfileDeletion>> {
        self.load_registry()?;
        self.profiles
            .retain(|profile, _| self.registry.contains(profile));
        let journal = self.profile_deletion_journal_entries()?;
        let pending = Self::pending_profile_deletion_entries(&journal);
        if !journal.is_empty() {
            let authoritative = self.meta.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_snapshot WHERE id = 1)",
                [],
                |row| row.get::<_, bool>(0),
            )?;
            if !authoritative || self.load_authoritative()?.is_none() {
                return Err(invalid_data(
                    "profile deletion journal has no valid authoritative session",
                ));
            }
        }
        Ok(pending)
    }

    #[cfg(test)]
    pub(crate) fn fail_next_profile_deletion_commit_as_ambiguous(&mut self) {
        self.ambiguous_profile_deletion_commit_once = true;
    }

    #[cfg(test)]
    pub(crate) fn fail_next_profile_deletion_after_local_purge(&mut self) {
        self.fail_profile_deletion_after_local_purge_once = true;
    }

    pub(crate) fn finalize_profile_deletion(
        &mut self,
        profile: ProfileId,
    ) -> rusqlite::Result<bool> {
        self.finalize_profile_deletion_with_restart_confirmation(
            profile,
            cfg!(windows) && self.dir.is_some(),
        )
    }

    #[cfg(test)]
    pub(crate) fn finalize_profile_deletion_requiring_restart_confirmation(
        &mut self,
        profile: ProfileId,
    ) -> rusqlite::Result<bool> {
        self.finalize_profile_deletion_with_restart_confirmation(profile, self.dir.is_some())
    }

    fn finalize_profile_deletion_with_restart_confirmation(
        &mut self,
        profile: ProfileId,
        require_restart_confirmation: bool,
    ) -> rusqlite::Result<bool> {
        // Validate every row first. A malformed sibling must not be hidden by
        // a targeted query and later crowd a valid authorization out of the
        // bounded cohort.
        let journal = self.profile_deletion_journal_entries()?;
        let Some(deletion) = journal
            .into_iter()
            .find(|deletion| deletion.profile == profile)
        else {
            return Ok(false);
        };
        if self.registry.contains(&profile) {
            return Err(invalid_data("active profile cannot complete deletion"));
        }
        // Registry removal already prevents new artifact publication. Keep
        // the original durable deletion row until both stores have settled;
        // a crash or unavailable journal repeats this purge on the next try.
        #[cfg(all(target_os = "windows", feature = "work-execution"))]
        self.purge_windows_work_artifacts(profile)?;
        if deletion.local_unlink_process.is_some() && require_restart_confirmation {
            // The first process has already completed its local phase. Only a
            // Hub carrying a different process generation may clear this
            // tombstone after re-observing the recovered filesystem namespace.
            return Ok(true);
        }

        // Persist native proof before deleting the SQLite file. A crash after
        // this commit resumes only the local file phase; a crash before it
        // safely repeats the idempotent native verification.
        if !deletion.native_erasure_verified {
            let changed = self.meta.execute(
                "UPDATE profile_deletion_journal
                 SET native_erasure_verified = 1
                 WHERE profile_id = ?1 AND native_erasure_verified = 0",
                [profile.to_string()],
            )?;
            if changed != 1 {
                return Err(invalid_data(
                    "profile deletion native proof changed no exact durable obligation",
                ));
            }
        }

        self.profiles.remove(&profile);
        if let Some(dir) = &self.dir {
            // Only this exact durable row authorizes unlinking. Arbitrary
            // profile-shaped orphans are never discovered or removed here.
            purge_profile_file(dir, profile)?;
        }

        #[cfg(test)]
        if std::mem::take(&mut self.fail_profile_deletion_after_local_purge_once) {
            return Err(invalid_data("injected failure after local profile purge"));
        }

        let changed = if require_restart_confirmation {
            self.meta.execute(
                "UPDATE profile_deletion_journal
                 SET local_unlink_process = ?2
                 WHERE profile_id = ?1
                   AND native_erasure_verified = 1
                   AND local_unlink_process IS NULL",
                params![
                    profile.to_string(),
                    self.deletion_process_generation.to_string()
                ],
            )?
        } else {
            self.meta.execute(
                "DELETE FROM profile_deletion_journal
                 WHERE profile_id = ?1 AND native_erasure_verified = 1",
                [profile.to_string()],
            )?
        };
        if changed != 1 {
            return Err(invalid_data(
                "profile deletion journal changed during completion",
            ));
        }
        self.degraded_profiles.remove(&profile);
        Ok(true)
    }
}

pub(super) fn deletion_process_generation() -> ProfileId {
    static GENERATION: OnceLock<ProfileId> = OnceLock::new();
    *GENERATION.get_or_init(new_deletion_process_generation)
}

fn new_deletion_process_generation() -> ProfileId {
    // Migration 9 reserves the zero ULID as the generation marker for a
    // completed version-8 unlink. Never mint it for a live process, making a
    // migrated tombstone provably eligible only for restart reconciliation.
    loop {
        let generation = ProfileId::generate();
        if generation != ProfileId::from(0) {
            return generation;
        }
    }
}

fn purge_profile_file(dir: &Path, profile: ProfileId) -> rusqlite::Result<()> {
    // This provides fail-closed logical deletion and overwrites SQLite cells
    // where the filesystem honors those writes. It is not a promise of
    // physical secure erasure on copy-on-write filesystems or SSD media.
    // Notes go first: if erasing them fails, the database and its journal
    // authorization remain and the whole cleanup is retried.
    remove_notes_directory(dir, profile)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    let path = dir.join(format!("profile-{profile}.sqlite"));
    if regular_file_exists(&path)? {
        let scrub = scrub_profile_database(&path);
        if let Err(error) = scrub {
            // A corrupt/unsupported database cannot be logically scrubbed
            // with SQLite, but it is still an exact journal-authorized file.
            // Continue to unlink it; retaining known private data forever is
            // not a safer fallback. NOFOLLOW/canonical-child validation keeps
            // authorization scoped to the owned data directory.
            eprintln!("store: unlinking unsrubbable deleted profile {profile}: {error}");
        }
    }

    // Remove sidecars first so a sidecar failure leaves the canonical file in
    // place and the next save/open can retry the whole cleanup.
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        remove_file_if_present(&PathBuf::from(sidecar))?;
    }
    remove_file_if_present(&path)?;
    // On Unix, order the file unlink before the journal authorization is
    // cleared. Otherwise a sudden power loss could retain a directory entry
    // while SQLite durably forgets that cleanup is pending. Win32 does not
    // document FlushFileBuffers for directory handles; its power-cut behavior
    // stays a packaged release gate rather than using an unsupported call
    // that would make every deletion fail.
    #[cfg(unix)]
    sync_directory(dir)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    Ok(())
}

/// The profile's notes folder and index, `notes/<profile>`. The directory is
/// first renamed aside so a partial removal can never be mistaken for a live
/// folder, then removed without following any link inside it.
fn remove_notes_directory(dir: &Path, profile: ProfileId) -> std::io::Result<()> {
    let notes = dir.join("notes");
    let live = notes.join(profile.to_string());
    let erasing = notes.join(format!(".erasing-{profile}"));
    match std::fs::symlink_metadata(&live) {
        Ok(meta) if meta.file_type().is_dir() => {
            if std::fs::symlink_metadata(&erasing).is_ok() {
                std::fs::remove_dir_all(&erasing)?;
            }
            std::fs::rename(&live, &erasing)?;
        }
        Ok(_) => std::fs::remove_file(&live)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    match std::fs::symlink_metadata(&erasing) {
        Ok(meta) if meta.file_type().is_dir() => std::fs::remove_dir_all(&erasing)?,
        Ok(_) => std::fs::remove_file(&erasing)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    sync_directory(&notes)?;
    Ok(())
}

/// Logically removes every current profile-owned data and authority domain
/// before the journal-authorized unlink. Keep this list in dependency order:
/// future PROFILE migrations that add durable user data must extend this
/// boundary and its direct inventory test in the same change.
fn scrub_profile_database(path: &Path) -> rusqlite::Result<()> {
    let mut conn = open_database(path)?;
    configure(&conn)?;
    migrations::apply(&mut conn, migrations::PROFILE)?;
    let tx = conn.transaction()?;
    tx.execute_batch(
        "INSERT INTO history_fts(history_fts, rank) VALUES('secure-delete', 1);
         INSERT INTO work_memories_fts(work_memories_fts, rank) VALUES('secure-delete', 1);
         DELETE FROM work_memories;
         DELETE FROM work_context_consent;
         DELETE FROM work_environment_checkpoints;
         DELETE FROM work_environment_commands;
         DELETE FROM work_environment_selection;
         DELETE FROM work_environments;
         DELETE FROM work_authoring_commands;
         DELETE FROM works;
         DELETE FROM work_payload_usage;
         DELETE FROM page_permission_grants;
         DELETE FROM page_permission_catalog;
         DELETE FROM task_list_receipts;
         DELETE FROM task_lists;
         DELETE FROM user_resource_receipts;
         DELETE FROM user_resources;
         DELETE FROM user_resource_usage;
         DELETE FROM userscripts;
         DELETE FROM userscript_catalog;
         DELETE FROM download_cleanup;
         DELETE FROM download_preferences;
         DELETE FROM downloads;
         DELETE FROM blocker_statistics;
         DELETE FROM time_spent;
         DELETE FROM time_batch_receipts;
         DELETE FROM bookmarks;
         DELETE FROM search_queries;
         DELETE FROM history;
         DELETE FROM history_usage;
         DELETE FROM favicons;
         DELETE FROM settings;
         DELETE FROM items;
         DELETE FROM spaces;
         DELETE FROM focus;
         DELETE FROM sqlite_sequence WHERE name IN ('history', 'time_batch_receipts');",
    )?;
    tx.commit()?;
    conn.execute_batch(
        "PRAGMA wal_checkpoint(TRUNCATE);
         VACUUM;
         PRAGMA journal_mode=DELETE;",
    )
}

fn remove_file_if_present(path: &Path) -> rusqlite::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(rusqlite::Error::ToSqlConversionFailure(Box::new(error))),
    }
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "store durability barrier target is not a direct directory",
        ));
    }
    std::fs::File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    const PROFILE_SCRUB_MARKER: &str = "zephiumscrubmarker97613";

    #[test]
    fn profile_scrub_covers_every_current_user_data_and_authority_table() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("profile-test.sqlite");
        let mut conn = open_database(&path).unwrap();
        configure(&conn).unwrap();
        migrations::apply(&mut conn, migrations::PROFILE).unwrap();
        conn.execute_batch(
            "INSERT INTO spaces(id, name, position) VALUES ('space', 'Personal', 0);
             INSERT INTO items(
                 id, parent_id, space_id, section, position, kind, name, url, title, zoom
             ) VALUES (
                 'item', NULL, 'space', 'today', 0, 'tab', NULL,
                 'https://private.example/path', 'Private title', 1
             );
             INSERT INTO focus(id, active_space, active_item, splits)
             VALUES (1, 'space', 'item', 'private-layout');
             INSERT INTO favicons(origin, content_type, icon, fetched_at)
             VALUES ('https://history.example', 'image/png', X'01020304', 1);
             INSERT INTO settings(key, value) VALUES ('private-setting', 'private-value');
             INSERT INTO downloads(id,session,revision,terminal,payload) VALUES ('00000000000000000000000001','00000000000000000000000002',1,1,'{\"private\":\"download-history\"}');
             INSERT INTO download_preferences(id,payload) VALUES (1,'{\"directory\":\"/private/downloads\"}');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history(url, title, visited_at) VALUES (?1, ?2, 1)",
            params![
                format!("https://history.example/{PROFILE_SCRUB_MARKER}"),
                PROFILE_SCRUB_MARKER
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO userscripts(
                 id, revision, enabled, metadata_format, source, source_sha256_v1
             ) VALUES (?1, 1, 1, 1, ?2, ?3)",
            params![
                "00000000000000000000000001",
                "// ==UserScript==\n// @name Secret\n// @match https://private.example/*\n// ==/UserScript==\n",
                vec![7_u8; 32]
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO page_permission_grants(
                 id, revision, origin, kind, decision
             ) VALUES (?1, 1, 'https://private.example', 'camera', 'allow')",
            ["00000000000000000000000002"],
        )
        .unwrap();
        conn.execute("INSERT INTO works(id, schema_version, revision, status, objective, created_unix_ms, updated_unix_ms, lifecycle, objective_revision, context_revision, objective_author) VALUES ('00000000000000000000000001', 2, 1, 'draft', ?1, 1, 1, 'active', 1, 1, 'user')", [PROFILE_SCRUB_MARKER]).unwrap();
        conn.execute("INSERT INTO work_plans(work_id, revision, plan_id, author, basis_revision) VALUES ('00000000000000000000000001', 2, '00000000000000000000000002', 'user', 1)", []).unwrap();
        conn.execute("INSERT INTO work_executions(work_id, execution_id, plan_revision, owner_session, approved_unix_ms, expires_unix_ms, body) VALUES ('00000000000000000000000001', '00000000000000000000000003', 2, '00000000000000000000000004', 1, 2, ?1)", [PROFILE_SCRUB_MARKER]).unwrap();
        conn.execute("INSERT INTO work_commands(work_id, command_id, request_digest, body) VALUES ('00000000000000000000000001', '00000000000000000000000005', zeroblob(32), ?1)", [PROFILE_SCRUB_MARKER]).unwrap();
        conn.execute("INSERT INTO work_authoring_commands(command_id, request_digest, body) VALUES ('00000000000000000000000006', zeroblob(32), ?1)", [PROFILE_SCRUB_MARKER]).unwrap();
        // Environment rows have no objective FK; scrub them explicitly before
        // their accounting singleton, including selected state and replay IDs.
        conn.execute("INSERT INTO work_environments(id,space_id,revision,view_revision,body,view) VALUES ('00000000000000000000000007','00000000000000000000000008',1,1,?1,?1)", [PROFILE_SCRUB_MARKER]).unwrap();
        conn.execute("INSERT INTO work_environment_selection(space_id,environment_id) VALUES ('00000000000000000000000008','00000000000000000000000007')", []).unwrap();
        conn.execute("INSERT INTO work_environment_checkpoints(environment_id,expected_revision,digest,applied_revision) VALUES ('00000000000000000000000007',1,zeroblob(32),2)", []).unwrap();
        conn.execute("INSERT INTO work_environment_commands(command_id,digest,environment_id,revision,view_revision) VALUES ('00000000000000000000000009',zeroblob(32),'00000000000000000000000007',1,1)", []).unwrap();
        conn.execute("INSERT INTO work_memories(id,text,kind,work,execution,created_ms) VALUES ('0000000000000000000000000A',?1,'fact','00000000000000000000000001',NULL,1)", [PROFILE_SCRUB_MARKER]).unwrap();
        conn.execute("INSERT INTO work_context_consent(work,source,allowed) VALUES ('00000000000000000000000001','history',1)", []).unwrap();
        let body = serde_json::to_string(&zephium_core::resources::ResourceDraft {
            title: PROFILE_SCRUB_MARKER.into(),
            pinned: false,
            related: vec![],
            content: zephium_core::resources::ResourceContent::Task {
                details: Default::default(),
                description: PROFILE_SCRUB_MARKER.into(),
                completed: false,
                due_date: None,
                due_time: None,
                status: zephium_core::resources::TaskStatus::Open,
                assignee: zephium_core::resources::TaskActor::User,
                origin: zephium_core::resources::TaskActor::User,
                context: None,
                sort_key: None,
                work: None,
            },
        })
        .unwrap();
        conn.execute("INSERT INTO user_resources(id,kind,revision,title,pinned,trashed,created_at,updated_at,body,search_text,completed) VALUES('00000000000000000000000001','task',1,?1,0,0,1,1,?2,?1,0)",params![PROFILE_SCRUB_MARKER,body]).unwrap();
        conn.execute("INSERT INTO user_resource_receipts(request_id,digest,resource_id,revision,retained) VALUES(?1,?2,'00000000000000000000000001',1,1)",params![PROFILE_SCRUB_MARKER,vec![1_u8;32]]).unwrap();
        conn.execute("INSERT INTO task_lists(id,title,revision,deleted) VALUES('00000000000000000000000002',?1,1,0)",[PROFILE_SCRUB_MARKER]).unwrap();
        conn.execute("INSERT INTO task_list_receipts(request_id,digest,list_id,retained) VALUES(?1,?2,'00000000000000000000000002',1)",params![PROFILE_SCRUB_MARKER,vec![2_u8;32]]).unwrap();
        conn.execute(
            "INSERT INTO bookmarks(parent_id,position,title,url,added_at) VALUES(NULL,0,?1,'https://scrub.example/',1)",
            [PROFILE_SCRUB_MARKER],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO blocker_statistics VALUES(1,?1)",
            [r#"{"day":700000,"days":[0,0,0,0,0,0,12]}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO time_spent(hour,place,spent_ms,opens) VALUES(1,'scrub.example',1000,1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO time_batch_receipts(batch_id,digest) VALUES(?1,?2)",
            params![vec![0xaa_u8; 16], vec![0xbb_u8; 32]],
        )
        .unwrap();
        drop(conn);

        scrub_profile_database(&path).unwrap();

        let conn = Connection::open(&path).unwrap();
        let actual_tables = conn
            .prepare(
                "SELECT name
                 FROM sqlite_schema
                 WHERE type = 'table'
                 ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let expected_tables = [
            "blocker_statistics",
            "bookmarks",
            "download_cleanup",
            "download_preferences",
            "downloads",
            "favicons",
            "focus",
            "history",
            "history_fts",
            "history_fts_config",
            "history_fts_data",
            "history_fts_docsize",
            "history_fts_idx",
            "history_usage",
            "items",
            "page_permission_catalog",
            "page_permission_grants",
            "resource_titles_fts",
            "resource_titles_fts_config",
            "resource_titles_fts_data",
            "resource_titles_fts_docsize",
            "resource_titles_fts_idx",
            "search_queries",
            "settings",
            "spaces",
            "sqlite_sequence",
            "task_list_receipts",
            "task_lists",
            "time_batch_receipts",
            "time_spent",
            "user_resource_receipts",
            "user_resource_usage",
            "user_resources",
            "userscript_catalog",
            "userscripts",
            "work_authoring_commands",
            "work_commands",
            "work_context_consent",
            "work_environment_checkpoints",
            "work_environment_commands",
            "work_environment_selection",
            "work_environments",
            "work_events",
            "work_executions",
            "work_memories",
            "work_memories_fts",
            "work_memories_fts_config",
            "work_memories_fts_data",
            "work_memories_fts_docsize",
            "work_memories_fts_idx",
            "work_payload_usage",
            "work_plan_nodes",
            "work_plans",
            "work_questions",
            "works",
        ];
        assert_eq!(
            actual_tables,
            expected_tables.map(str::to_owned),
            "a PROFILE migration changed the scrub-owned table inventory"
        );

        for table in [
            "blocker_statistics",
            "time_spent",
            "time_batch_receipts",
            "bookmarks",
            "download_cleanup",
            "download_preferences",
            "downloads",
            "search_queries",
            "task_list_receipts",
            "task_lists",
            "user_resource_receipts",
            "user_resources",
            "user_resource_usage",
            "spaces",
            "items",
            "focus",
            "history",
            "history_usage",
            "favicons",
            "settings",
            "userscript_catalog",
            "userscripts",
            "page_permission_catalog",
            "page_permission_grants",
            "works",
            "work_payload_usage",
            "work_plans",
            "work_plan_nodes",
            "work_questions",
            "work_events",
            "work_executions",
            "work_authoring_commands",
            "work_commands",
            "work_environment_checkpoints",
            "work_environment_commands",
            "work_environment_selection",
            "work_environments",
            "work_memories",
            "work_context_consent",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "profile scrub retained rows in {table}");
        }
        let retained_fts_terms: i64 = conn
            .query_row(
                "SELECT count(*) FROM history_fts WHERE history_fts MATCH ?1",
                [PROFILE_SCRUB_MARKER],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained_fts_terms, 0, "profile scrub retained FTS terms");
        let retained_memory_terms: i64 = conn
            .query_row(
                "SELECT count(*) FROM work_memories_fts WHERE work_memories_fts MATCH ?1",
                [PROFILE_SCRUB_MARKER],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            retained_memory_terms, 0,
            "profile scrub retained memory terms"
        );
        let history_sequence: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_sequence WHERE name = 'history'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(history_sequence, 0);
        drop(conn);

        for database_file in database_files(&path) {
            if database_file.try_exists().unwrap() {
                let bytes = std::fs::read(&database_file).unwrap();
                assert!(
                    !bytes
                        .windows(PROFILE_SCRUB_MARKER.len())
                        .any(|window| window == PROFILE_SCRUB_MARKER.as_bytes()),
                    "profile scrub retained marker bytes in {}",
                    database_file.display()
                );
            }
        }
    }

    fn database_files(path: &Path) -> [PathBuf; 3] {
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let mut shm = path.as_os_str().to_owned();
        shm.push("-shm");
        [path.to_path_buf(), PathBuf::from(wal), PathBuf::from(shm)]
    }
}
