//! Download metadata is private to its durable profile database.

use super::*;
use zephium_core::downloads::*;

impl Hub {
    pub(crate) fn download_recovery_profiles(&self) -> DownloadStoreReply {
        if self.recovery_required.is_some()
            || self.registry.len() > zephium_core::session::MAX_SESSION_PROFILES
        {
            return DownloadStoreReply::Error(DownloadError::Storage);
        }
        let mut profiles: Vec<_> = self.registry.iter().copied().collect();
        let Ok(deleting) = self.download_cleanup_deletions() else {
            return DownloadStoreReply::Error(DownloadError::Storage);
        };
        profiles.extend(deleting);
        profiles.sort();
        profiles.dedup();
        DownloadStoreReply::Profiles(profiles)
    }

    pub(crate) fn download_call(
        &mut self,
        profile: ProfileId,
        call: DownloadStoreCall,
    ) -> DownloadStoreReply {
        let retiring_cleanup = matches!(
            &call,
            DownloadStoreCall::RecoveryPage { .. } | DownloadStoreCall::ClearStaging { .. }
        ) || matches!(&call, DownloadStoreCall::Save(record) if record.state.terminal());
        let authorized_retirement = retiring_cleanup
            && self
                .download_cleanup_deletions()
                .is_ok_and(|profiles| profiles.contains(&profile));
        if (!self.registry.contains(&profile) && !authorized_retirement)
            || self.degraded_profiles.contains(&profile)
            || self.recovery_required.is_some()
        {
            return DownloadStoreReply::Error(DownloadError::Storage);
        }
        let result = self.download_call_inner(profile, call);
        result.unwrap_or(DownloadStoreReply::Error(DownloadError::Storage))
    }

    fn download_call_inner(
        &mut self,
        profile: ProfileId,
        call: DownloadStoreCall,
    ) -> rusqlite::Result<DownloadStoreReply> {
        let conn = self.profile_conn(profile)?;
        match call {
            DownloadStoreCall::Save(record) => {
                if !record.validate() {
                    return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                }
                let payload = serde_json::to_string(&record)
                    .map_err(|_| invalid_data("invalid download record"))?;
                if payload.len() > MAX_DOWNLOAD_RECORD_BYTES {
                    return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                }
                let tx = conn.transaction()?;
                let previous: Option<(String, u32, bool, String)> = tx
                    .query_row(
                        "SELECT session,revision,terminal,payload FROM downloads WHERE id=?1",
                        [record.id.to_string()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()?;
                if let Some((session, revision, terminal, old)) = previous {
                    if old == payload {
                        return Ok(DownloadStoreReply::Saved);
                    }
                    if session != record.session.to_string()
                        || revision >= record.revision
                        || terminal
                    {
                        return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                    }
                } else {
                    tx.execute("DELETE FROM downloads WHERE id IN (SELECT id FROM downloads WHERE terminal=1 ORDER BY id ASC LIMIT max(0,(SELECT count(*) FROM downloads)-9999))", [])?;
                    let count: i64 =
                        tx.query_row("SELECT count(*) FROM downloads", [], |row| row.get(0))?;
                    if count >= MAX_DOWNLOAD_HISTORY as i64 {
                        return Ok(DownloadStoreReply::Error(DownloadError::Capacity));
                    }
                }
                if record.staging.is_some() && record.staging_identity.is_some() {
                    let count: i64 = tx.query_row(
                        "SELECT count(*) FROM download_cleanup WHERE id<>?1",
                        [record.id.to_string()],
                        |row| row.get(0),
                    )?;
                    if count >= MAX_DOWNLOAD_HISTORY as i64 {
                        return Ok(DownloadStoreReply::Error(DownloadError::Capacity));
                    }
                    tx.execute("INSERT INTO download_cleanup(id,session,terminal,payload) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET terminal=excluded.terminal,payload=excluded.payload WHERE download_cleanup.session=excluded.session", params![record.id.to_string(),record.session.to_string(),record.state.terminal(),payload])?;
                }
                if record.state.terminal() {
                    // Publication can clear the history's staging fields, but
                    // only a filesystem cleanup acknowledgement clears ownership.
                    tx.execute(
                        "UPDATE download_cleanup SET terminal=?3,payload=json_set(payload,'$.writer_released',json(?4)) WHERE id=?1 AND session=?2",
                        params![record.id.to_string(), record.session.to_string(),record.writer_released,if record.writer_released {"true"} else {"false"}],
                    )?;
                }
                tx.execute("INSERT INTO downloads(id,session,revision,terminal,payload) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,terminal=excluded.terminal,payload=excluded.payload",
                    params![record.id.to_string(), record.session.to_string(), record.revision, record.state.terminal(), payload])?;
                tx.commit()?;
                Ok(DownloadStoreReply::Saved)
            }
            DownloadStoreCall::List {
                before,
                limit,
                session,
                active,
            } => {
                if limit == 0 || limit > MAX_DOWNLOAD_PAGE || active.len() > MAX_ACTIVE_DOWNLOADS {
                    return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                }
                // A previous process cannot still own a native operation. Do
                // not pretend its transfer resumed, or leave it active forever.
                let active = serde_json::to_string(&active)
                    .map_err(|_| invalid_data("invalid active download cohort"))?;
                conn.execute("UPDATE downloads SET terminal=1,revision=min(revision+1,4294967295),payload=json_set(payload,'$.revision',min(revision+1,4294967295),'$.state','interrupted','$.error','unavailable') WHERE terminal=0 AND (session<>?1 OR id NOT IN (SELECT value FROM json_each(?2)))", params![session.to_string(),active])?;
                let mut query = conn.prepare_cached("SELECT payload FROM downloads WHERE (?1 IS NULL OR id<?1) ORDER BY id DESC LIMIT ?2")?;
                let rows = query
                    .query_map(params![before.map(|id| id.to_string()), limit], |row| {
                        row.get::<_, String>(0)
                    })?;
                let mut records = Vec::new();
                for row in rows {
                    records.push(decode_record(&row?)?);
                }
                Ok(DownloadStoreReply::Page(records))
            }
            DownloadStoreCall::RecoveryPage {
                after,
                limit,
                session,
                active,
            } => {
                if limit == 0 || limit > MAX_DOWNLOAD_PAGE || active.len() > MAX_ACTIVE_DOWNLOADS {
                    return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                }
                let active = serde_json::to_string(&active)
                    .map_err(|_| invalid_data("invalid active download cohort"))?;
                conn.execute("UPDATE downloads SET terminal=1,revision=min(revision+1,4294967295),payload=json_set(payload,'$.revision',min(revision+1,4294967295),'$.state','interrupted','$.error','unavailable') WHERE terminal=0 AND (session<>?1 OR id NOT IN (SELECT value FROM json_each(?2)))", params![session.to_string(),active])?;
                let mut query = conn.prepare_cached("SELECT payload FROM download_cleanup WHERE (?1 IS NULL OR id>?1) AND (session<>?2 OR terminal=1 OR id NOT IN (SELECT value FROM json_each(?3))) ORDER BY id LIMIT ?4")?;
                let rows = query.query_map(
                    params![
                        after.map(|id| id.to_string()),
                        session.to_string(),
                        active,
                        limit
                    ],
                    |row| row.get::<_, String>(0),
                )?;
                let mut records = Vec::new();
                for row in rows {
                    let mut record = decode_record(&row?)?;
                    record.state = DownloadState::Interrupted;
                    records.push(record);
                }
                Ok(DownloadStoreReply::Page(records))
            }
            DownloadStoreCall::Get(id) => {
                let payload: Option<String> = conn
                    .query_row(
                        "SELECT payload FROM downloads WHERE id=?1",
                        [id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?;
                Ok(DownloadStoreReply::Record(
                    payload
                        .map(|value| decode_record(&value).map(Box::new))
                        .transpose()?,
                ))
            }
            DownloadStoreCall::Forget(id) => {
                let changed = conn.execute(
                    "DELETE FROM downloads WHERE id=?1 AND terminal=1",
                    [id.to_string()],
                )?;
                if changed == 1 {
                    Ok(DownloadStoreReply::Saved)
                } else {
                    Ok(DownloadStoreReply::Error(DownloadError::Invalid))
                }
            }
            DownloadStoreCall::Clear => {
                conn.execute("DELETE FROM downloads WHERE terminal=1", [])?;
                Ok(DownloadStoreReply::Saved)
            }
            DownloadStoreCall::ClearSince(since) => {
                conn.execute(
                    "DELETE FROM downloads WHERE terminal=1 AND json_extract(payload,'$.created_at')>=?1",
                    [since],
                )?;
                Ok(DownloadStoreReply::Saved)
            }
            DownloadStoreCall::ClearStaging { id, expected } => {
                let tx = conn.transaction()?;
                let receipt: Option<String> = tx
                    .query_row(
                        "SELECT payload FROM download_cleanup WHERE id=?1",
                        [id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?;
                let Some(receipt) = receipt else {
                    return Ok(DownloadStoreReply::Saved);
                };
                if decode_record(&receipt)?.staging_identity.as_ref() != Some(&expected) {
                    return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                }
                let value: Option<String> = tx
                    .query_row(
                        "SELECT payload FROM downloads WHERE id=?1 AND terminal=1",
                        [id.to_string()],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(value) = value {
                    let mut record = decode_record(&value)?;
                    if record.staging_identity.as_ref() == Some(&expected) {
                        record.staging = None;
                        record.staging_identity = None;
                        record.revision = record
                            .revision
                            .checked_add(1)
                            .ok_or_else(|| invalid_data("download revision exhausted"))?;
                        let payload = serde_json::to_string(&record)
                            .map_err(|_| invalid_data("invalid download record"))?;
                        tx.execute(
                            "UPDATE downloads SET revision=?2,payload=?3 WHERE id=?1",
                            params![id.to_string(), record.revision, payload],
                        )?;
                    }
                }
                tx.execute("DELETE FROM download_cleanup WHERE id=?1", [id.to_string()])?;
                tx.commit()?;
                Ok(DownloadStoreReply::Saved)
            }
            DownloadStoreCall::Preferences => {
                let payload: Option<String> = conn
                    .query_row(
                        "SELECT payload FROM download_preferences WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?;
                let preferences: DownloadPreferences = match payload {
                    Some(value) => serde_json::from_str(&value)
                        .map_err(|_| invalid_data("invalid download preferences"))?,
                    None => DownloadPreferences::default(),
                };
                if !preferences.validate() {
                    return Err(invalid_data("invalid download directory"));
                }
                Ok(DownloadStoreReply::Preferences(preferences))
            }
            DownloadStoreCall::SetPreferences(change) => {
                let payload: Option<String> = conn
                    .query_row(
                        "SELECT payload FROM download_preferences WHERE id=1",
                        [],
                        |row| row.get(0),
                    )
                    .optional()?;
                let mut preferences: DownloadPreferences = match payload {
                    Some(value) => serde_json::from_str(&value)
                        .map_err(|_| invalid_data("invalid download preferences"))?,
                    None => DownloadPreferences::default(),
                };
                match change {
                    DownloadPreferenceChange::AskDestination(enabled) => {
                        preferences.ask_destination = enabled
                    }
                    DownloadPreferenceChange::Directory { path, identity } => {
                        preferences.directory = Some(path);
                        preferences.directory_identity = Some(identity);
                    }
                }
                if !preferences.validate() {
                    return Ok(DownloadStoreReply::Error(DownloadError::Invalid));
                }
                let payload = serde_json::to_string(&preferences)
                    .map_err(|_| invalid_data("invalid download preferences"))?;
                conn.execute("INSERT INTO download_preferences(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload", [payload])?;
                Ok(DownloadStoreReply::Preferences(preferences))
            }
        }
    }
}

fn decode_record(value: &str) -> rusqlite::Result<DownloadRecord> {
    if value.len() > MAX_DOWNLOAD_RECORD_BYTES {
        return Err(invalid_data("oversized download record"));
    }
    let record: DownloadRecord =
        serde_json::from_str(value).map_err(|_| invalid_data("invalid download record"))?;
    if !record.validate() {
        return Err(invalid_data("invalid download metadata"));
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::ids::DownloadId;

    fn record() -> DownloadRecord {
        DownloadRecord {
            id: DownloadId::generate(),
            session: DownloadId::generate(),
            revision: 1,
            created_at: 1,
            filename: "report.txt".into(),
            source: "https://example.com".into(),
            source_is_context: false,
            state: DownloadState::Pending,
            received: 0,
            total: None,
            error: None,
            destination: None,
            staging: None,
            staging_identity: None,
            identity: None,
            writer: None,
            writer_released: false,
        }
    }

    #[test]
    fn durable_payload_rejects_invalid_paths_and_completed_without_identity() {
        let mut value = record();
        assert!(decode_record(&serde_json::to_string(&value).unwrap()).is_ok());
        value.destination = Some(PathBuf::from("relative.txt"));
        assert!(decode_record(&serde_json::to_string(&value).unwrap()).is_err());
        value.destination = Some(PathBuf::from("/tmp/report.txt"));
        value.state = DownloadState::Completed;
        assert!(decode_record(&serde_json::to_string(&value).unwrap()).is_err());
    }
    fn registered_hub() -> (Hub, ProfileId) {
        let mut hub = Hub::in_memory().unwrap();
        let profile = ProfileId::from(1);
        hub.registry.insert(profile);
        (hub, profile)
    }

    fn staged_record() -> DownloadRecord {
        let mut record = record();
        record.id = DownloadId::from(1);
        record.staging =
            Some(std::env::temp_dir().join(format!(".zephium-download-{}", record.id)));
        record.staging_identity = Some(FileIdentity {
            file_high: 0,
            volume: 4,
            file: 9,
            bytes: 0,
            modified: None,
        });
        record
    }

    fn recovery(
        hub: &mut Hub,
        profile: ProfileId,
        session: DownloadId,
        active: Vec<DownloadId>,
    ) -> Vec<DownloadRecord> {
        match hub.download_call(
            profile,
            DownloadStoreCall::RecoveryPage {
                after: None,
                limit: 100,
                session,
                active,
            },
        ) {
            DownloadStoreReply::Page(records) => records,
            reply => panic!("expected recovery records, got {reply:?}"),
        }
    }

    #[test]
    fn deletion_authorization_preserves_cleanup_but_revokes_history_access() {
        let (mut hub, profile) = registered_hub();
        let mut record = staged_record();
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
            DownloadStoreReply::Saved
        ));
        hub.registry.remove(&profile);
        hub.meta
            .execute(
                "INSERT INTO profile_deletion_journal(profile_id,authorized_at) VALUES(?1,1)",
                [profile.to_string()],
            )
            .unwrap();
        assert!(
            matches!(hub.download_recovery_profiles(),DownloadStoreReply::Profiles(profiles) if profiles.contains(&profile))
        );
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Get(record.id)),
            DownloadStoreReply::Error(_)
        ));
        record.state = DownloadState::Cancelled;
        record.writer_released = true;
        record.revision += 1;
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
            DownloadStoreReply::Saved
        ));
        assert_eq!(recovery(&mut hub, profile, record.session, vec![]).len(), 1);
        assert!(matches!(
            hub.download_call(
                profile,
                DownloadStoreCall::ClearStaging {
                    id: record.id,
                    expected: record.staging_identity.unwrap()
                }
            ),
            DownloadStoreReply::Saved
        ));
        assert!(recovery(&mut hub, profile, record.session, vec![]).is_empty());
    }

    #[test]
    fn cleanup_ownership_survives_forget_and_requires_the_matching_receipt() {
        let (mut hub, profile) = registered_hub();
        let mut record = staged_record();
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
            DownloadStoreReply::Saved
        ));
        assert!(recovery(&mut hub, profile, record.session, vec![record.id]).is_empty());
        record.state = DownloadState::Cancelled;
        record.revision += 1;
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
            DownloadStoreReply::Saved
        ));
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Forget(record.id)),
            DownloadStoreReply::Saved
        ));
        assert_eq!(recovery(&mut hub, profile, record.session, vec![]).len(), 1);
        let identity = record.staging_identity.unwrap();
        let mut wrong = identity.clone();
        wrong.file += 1;
        assert!(matches!(
            hub.download_call(
                profile,
                DownloadStoreCall::ClearStaging {
                    id: record.id,
                    expected: wrong
                }
            ),
            DownloadStoreReply::Error(DownloadError::Invalid)
        ));
        assert_eq!(recovery(&mut hub, profile, record.session, vec![]).len(), 1);
        assert!(matches!(
            hub.download_call(
                profile,
                DownloadStoreCall::ClearStaging {
                    id: record.id,
                    expected: identity
                }
            ),
            DownloadStoreReply::Saved
        ));
        assert!(recovery(&mut hub, profile, record.session, vec![]).is_empty());
    }

    #[test]
    fn cleanup_ownership_survives_history_capacity_eviction() {
        let (mut hub, profile) = registered_hub();
        let mut record = staged_record();
        record.state = DownloadState::Cancelled;
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
            DownloadStoreReply::Saved
        ));
        let mut plain = record.clone();
        plain.staging = None;
        plain.staging_identity = None;
        let payload = serde_json::to_string(&plain).unwrap();
        let conn = hub.profile_conn(profile).unwrap();
        conn.execute("WITH RECURSIVE ids(n) AS (SELECT 2 UNION ALL SELECT n+1 FROM ids WHERE n<10000) INSERT INTO downloads(id,session,revision,terminal,payload) SELECT printf('%026d',n),?1,1,1,json_set(?2,'$.id',printf('%026d',n)) FROM ids",params![record.session.to_string(),payload]).unwrap();
        let mut fresh = plain;
        fresh.id = DownloadId::generate();
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(fresh))),
            DownloadStoreReply::Saved
        ));
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Get(record.id)),
            DownloadStoreReply::Record(None)
        ));
        assert_eq!(recovery(&mut hub, profile, record.session, vec![]).len(), 1);
    }

    #[test]
    fn startup_recovery_finds_orphans_without_a_history_read_and_preserves_active_owners() {
        let (mut hub, profile) = registered_hub();
        let record = staged_record();
        assert!(matches!(
            hub.download_call(profile, DownloadStoreCall::Save(Box::new(record.clone()))),
            DownloadStoreReply::Saved
        ));
        assert!(recovery(&mut hub, profile, record.session, vec![record.id]).is_empty());
        let recovered = recovery(&mut hub, profile, DownloadId::generate(), vec![]);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].state, DownloadState::Interrupted);
        let DownloadStoreReply::Record(Some(saved)) =
            hub.download_call(profile, DownloadStoreCall::Get(record.id))
        else {
            panic!("record missing")
        };
        assert_eq!(saved.state, DownloadState::Interrupted);
        assert_eq!(saved.revision, record.revision + 1);
        assert!(
            matches!(hub.download_recovery_profiles(),DownloadStoreReply::Profiles(profiles) if profiles==vec![profile])
        );
        let foreign = ProfileId::from(2);
        hub.registry.insert(foreign);
        assert!(recovery(&mut hub, foreign, DownloadId::generate(), vec![]).is_empty());
    }
}
