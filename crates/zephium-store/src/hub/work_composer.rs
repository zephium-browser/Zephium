//! Revision-scoped unsent composer text, erased with authoritative scopes.
use rusqlite::{params, OptionalExtension, Transaction};
use zephium_core::{
    ids::{ProfileId, SpaceId},
    work::{environment::*, WorkError, WorkRevision},
};

fn unavailable(_: rusqlite::Error) -> WorkError {
    WorkError::Unavailable
}

fn read(
    tx: &Transaction<'_>,
    profile: ProfileId,
    space: SpaceId,
) -> Result<WorkComposerDraft, WorkError> {
    let row = tx.query_row(
        "SELECT revision, CASE WHEN length(CAST(text AS BLOB))<=?3 THEN text END FROM work_composer_drafts WHERE profile_id=?1 AND space_id=?2",
        params![profile.to_string(), space.to_string(), MAX_COMPOSER_DRAFT_BYTES as i64],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
    ).optional().map_err(unavailable)?;
    let (revision, text) = match row {
        Some((revision, Some(text))) => (
            WorkRevision::new(u64::try_from(revision).map_err(|_| WorkError::Unavailable)?)
                .ok_or(WorkError::Unavailable)?,
            text,
        ),
        Some(_) => return Err(WorkError::Unavailable),
        None => (WorkRevision::INITIAL, String::new()),
    };
    validate_composer_draft(&text).map_err(|_| WorkError::Unavailable)?;
    Ok(WorkComposerDraft {
        profile,
        space,
        revision,
        text,
    })
}

pub(super) fn apply(
    tx: &Transaction<'_>,
    profile: ProfileId,
    call: WorkEnvironmentCall,
    available: bool,
) -> Result<WorkEnvironmentReply, WorkError> {
    call.validate()?;
    if !available {
        return Err(WorkError::NotFound);
    }
    let (space, incoming) = match call {
        WorkEnvironmentCall::ComposerRead { space } => (space, None),
        WorkEnvironmentCall::ComposerSave {
            space,
            expected,
            text,
        } => (space, Some((expected, text))),
        _ => return Err(WorkError::Invalid),
    };
    let mut draft = read(tx, profile, space)?;
    if let Some((expected, text)) = incoming {
        if draft.revision != expected {
            return Err(WorkError::Conflict);
        }
        {
            let (rows, nonempty, bytes): (usize, usize, usize) = tx.query_row(
                "SELECT (SELECT count(*) FROM work_composer_drafts),count(*) FILTER(WHERE text!=''),coalesce(sum(length(CAST(text AS BLOB))),0) FROM work_composer_drafts WHERE profile_id=?1",
                [profile.to_string()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
            ).map_err(unavailable)?;
            if bytes
                .saturating_sub(draft.text.len())
                .saturating_add(text.len())
                > MAX_COMPOSER_DRAFT_PROFILE_BYTES
                || (draft.text.is_empty()
                    && !text.is_empty()
                    && nonempty >= MAX_COMPOSER_DRAFTS_PER_PROFILE)
                || (draft.revision == WorkRevision::INITIAL
                    && rows >= zephium_core::session::MAX_SESSION_SPACES)
            {
                return Err(WorkError::Capacity);
            }
            let revision = draft.revision.next()?;
            if draft.revision == WorkRevision::INITIAL {
                tx.execute("INSERT INTO work_composer_drafts(profile_id,space_id,revision,text) VALUES(?1,?2,?3,?4)", params![profile.to_string(), space.to_string(), revision.get() as i64, text]).map_err(unavailable)?;
            } else {
                let changed = tx.execute("UPDATE work_composer_drafts SET revision=?3,text=?4 WHERE profile_id=?1 AND space_id=?2 AND revision=?5", params![profile.to_string(), space.to_string(), revision.get() as i64, text, expected.get() as i64]).map_err(unavailable)?;
                if changed != 1 {
                    return Err(WorkError::Conflict);
                }
            }
            draft.revision = revision;
            draft.text = text;
        }
    }
    Ok(WorkEnvironmentReply::Composer { draft })
}

/// Scope deletion and its draft erasure share the authoritative session commit.
pub(super) fn retain_scopes(
    tx: &Transaction<'_>,
    state: &zephium_core::session::SessionState,
) -> rusqlite::Result<bool> {
    let scopes: std::collections::HashSet<_> = state
        .spaces
        .iter()
        .map(|space| (space.profile, space.id))
        .collect();
    let mut statement = tx.prepare("SELECT profile_id,space_id FROM work_composer_drafts")?;
    let stored = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .take(zephium_core::session::MAX_SESSION_SPACES + 1)
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if stored.len() > zephium_core::session::MAX_SESSION_SPACES {
        return Err(rusqlite::Error::InvalidQuery);
    }
    drop(statement);
    let mut erased = false;
    for (profile, space) in stored {
        let profile_id = ProfileId::parse(&profile)
            .filter(|id| id.to_string() == profile)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let space_id = SpaceId::parse(&space)
            .filter(|id| id.to_string() == space)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        if !scopes.contains(&(profile_id, space_id)) {
            tx.execute(
                "DELETE FROM work_composer_drafts WHERE profile_id=?1 AND space_id=?2",
                params![profile, space],
            )?;
            erased = true;
        }
    }
    Ok(erased)
}

/// Session recovery may discard tabs, but must keep every still-owned unsent
/// draft reachable. A damaged Space name/layout is replaced by a clean scope.
pub(super) fn recover_spaces(
    conn: &rusqlite::Connection,
    state: &mut zephium_core::session::SessionState,
) -> rusqlite::Result<()> {
    use zephium_core::session::{self, PersistedSpace, MAX_SESSION_SPACES};
    let mut statement = conn.prepare("SELECT profile_id,space_id,CASE WHEN length(CAST(text AS BLOB))<=?1 THEN text END FROM work_composer_drafts WHERE text!=''")?;
    let rows = statement.query_map([MAX_COMPOSER_DRAFT_BYTES as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut retained = std::collections::HashSet::new();
    for (index, row) in rows.enumerate() {
        if index >= MAX_SESSION_SPACES {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let (profile, space, text) = row?;
        let profile = ProfileId::parse(&profile)
            .filter(|id| id.to_string() == profile)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let space = SpaceId::parse(&space)
            .filter(|id| id.to_string() == space)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        let text = text.ok_or(rusqlite::Error::InvalidQuery)?;
        validate_composer_draft(&text).map_err(|_| rusqlite::Error::InvalidQuery)?;
        if !state.profiles.iter().any(|owner| owner.id == profile) {
            return Err(rusqlite::Error::InvalidQuery);
        }
        retained.insert((profile, space));
        if !state
            .spaces
            .iter()
            .any(|scope| scope.profile == profile && scope.id == space)
        {
            if state.spaces.len() >= MAX_SESSION_SPACES {
                return Err(rusqlite::Error::InvalidQuery);
            }
            state.spaces.push(PersistedSpace {
                id: space,
                profile,
                name: "Recovered drafts".into(),
                look: Default::default(),
            });
        }
    }
    let recovered = session::canonicalize(state.clone());
    if retained.iter().any(|(profile, space)| {
        !recovered
            .spaces
            .iter()
            .any(|scope| scope.profile == *profile && scope.id == *space)
    }) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    *state = recovered;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::Hub;
    use zephium_core::{
        profiles::ProfileKind,
        session::{PersistedProfile, PersistedSpace, SessionState},
        work::port::{WorkReply, WorkRequest},
    };

    fn state(profiles: usize, spaces: usize) -> SessionState {
        let mut state = SessionState::default();
        for index in 0..profiles {
            let profile = ProfileId::from(index as u128 + 1);
            state.profiles.push(PersistedProfile {
                id: profile,
                name: format!("Profile {index}"),
                kind: if index == 0 {
                    ProfileKind::Default
                } else {
                    ProfileKind::Named
                },
            });
            for offset in 0..spaces {
                state.spaces.push(PersistedSpace {
                    id: SpaceId::from(100 + index as u128 * 100 + offset as u128),
                    profile,
                    name: format!("Space {offset}"),
                    look: Default::default(),
                });
            }
        }
        state.active_space = state.spaces.first().map(|space| space.id);
        zephium_core::session::canonicalize(state)
    }
    fn call(
        hub: &mut Hub,
        profile: ProfileId,
        call: WorkEnvironmentCall,
        available: bool,
    ) -> Result<WorkComposerDraft, WorkError> {
        match hub.work_document(
            profile,
            WorkRequest::Environment {
                call,
                space_available: available,
                browser_available: false,
                note_available: false,
            },
        )? {
            WorkReply::Environment(WorkEnvironmentReply::Composer { draft }) => Ok(draft),
            _ => panic!("expected composer reply"),
        }
    }
    fn get(hub: &mut Hub, profile: ProfileId, space: SpaceId) -> WorkComposerDraft {
        call(
            hub,
            profile,
            WorkEnvironmentCall::ComposerRead { space },
            true,
        )
        .unwrap()
    }
    fn save(
        hub: &mut Hub,
        draft: &WorkComposerDraft,
        text: &str,
    ) -> Result<WorkComposerDraft, WorkError> {
        call(
            hub,
            draft.profile,
            WorkEnvironmentCall::ComposerSave {
                space: draft.space,
                expected: draft.revision,
                text: text.into(),
            },
            true,
        )
    }

    #[test]
    fn composers_survive_reopen_and_cas_prevents_competing_writes_and_late_clears() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(2, 1);
        let (profile, space) = (state.profiles[0].id, state.spaces[0].id);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        let first = get(&mut hub, profile, space);
        let other = get(&mut hub, profile, space);
        let kept = save(&mut hub, &first, "unsent private request").unwrap();
        assert_eq!(
            save(&mut hub, &other, "stale other window"),
            Err(WorkError::Conflict)
        );
        drop(hub);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        assert_eq!(get(&mut hub, profile, space), kept);
        let cleared = save(&mut hub, &kept, "").unwrap();
        assert!(cleared.revision > kept.revision);
        let newer = save(&mut hub, &cleared, "newer unsent request").unwrap();
        assert_eq!(save(&mut hub, &kept, ""), Err(WorkError::Conflict));
        assert_eq!(get(&mut hub, profile, space), newer);
        assert_eq!(
            call(
                &mut hub,
                state.profiles[1].id,
                WorkEnvironmentCall::ComposerRead { space },
                true
            ),
            Err(WorkError::NotFound)
        );
        assert_eq!(
            call(
                &mut hub,
                profile,
                WorkEnvironmentCall::ComposerRead { space },
                false
            ),
            Err(WorkError::NotFound)
        );
        assert_eq!(
            call(
                &mut hub,
                ProfileId::from(999),
                WorkEnvironmentCall::ComposerRead { space },
                true
            ),
            Err(WorkError::ProfileUnavailable)
        );
        assert!(serde_json::from_str::<WorkEnvironmentCall>(
            r#"{"kind":"composer_read","space":"../other"}"#
        )
        .is_err());
    }

    #[test]
    fn an_explicit_clear_of_an_absent_draft_rejects_a_delayed_first_save() {
        let state = state(1, 1);
        let profile = state.profiles[0].id;
        let space = state.spaces[0].id;
        let mut hub = Hub::in_memory().unwrap();
        hub.save(&state).unwrap();
        let delayed = get(&mut hub, profile, space);
        let cleared = save(&mut hub, &delayed, "").unwrap();
        assert!(cleared.revision > delayed.revision);
        assert_eq!(
            save(&mut hub, &delayed, "delayed first words"),
            Err(WorkError::Conflict)
        );
        assert_eq!(get(&mut hub, profile, space), cleared);
    }

    #[test]
    fn clear_and_space_erasure_checkpoint_private_wal_and_busy_barriers_remain_uncertain() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(1, 2);
        let profile = state.profiles[0].id;
        let space = state.spaces[0].id;
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        let initial = get(&mut hub, profile, space);
        let kept = save(&mut hub, &initial, "composer-private-wal-marker-92817").unwrap();
        let reader = rusqlite::Connection::open(dir.path().join("meta.sqlite")).unwrap();
        reader.execute_batch("BEGIN;").unwrap();
        reader
            .query_row(
                "SELECT revision FROM work_composer_drafts LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(save(&mut hub, &kept, ""), Err(WorkError::OutcomeUnknown));
        assert!(hub.session_backup_checkpoint_pending);
        reader.execute_batch("ROLLBACK;").unwrap();
        let empty = get(&mut hub, profile, space);
        assert!(empty.text.is_empty());
        assert!(!hub.session_backup_checkpoint_pending);
        let bytes = std::fs::read(dir.path().join("meta.sqlite-wal")).unwrap_or_default();
        assert!(!bytes
            .windows(b"composer-private-wal-marker-92817".len())
            .any(|bytes| bytes == b"composer-private-wal-marker-92817"));
        save(&mut hub, &empty, "composer-space-private-wal-marker-41962").unwrap();
        state.spaces.remove(0);
        state.active_space = state.spaces.first().map(|scope| scope.id);
        hub.save(&state).unwrap();
        assert_eq!(
            call(
                &mut hub,
                profile,
                WorkEnvironmentCall::ComposerSave {
                    space,
                    expected: empty.revision,
                    text: "late resurrected words".into()
                },
                true
            ),
            Err(WorkError::NotFound)
        );
        let bytes = std::fs::read(dir.path().join("meta.sqlite-wal")).unwrap_or_default();
        assert!(!bytes
            .windows(b"composer-space-private-wal-marker-41962".len())
            .any(|bytes| bytes == b"composer-space-private-wal-marker-41962"));
    }

    #[test]
    fn recovery_rebuilds_a_missing_draft_space_without_changing_preserved_session_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(1, 1);
        let profile = state.profiles[0].id;
        let space = state.spaces[0].id;
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        let initial = get(&mut hub, profile, space);
        let kept = save(&mut hub, &initial, "retain when metadata is damaged").unwrap();
        hub.quarantine_current_authoritative("fixture damaged space metadata")
            .unwrap();
        let preserved =
            serde_json::json!({"profiles":state.profiles,"spaces":[],"items":"damaged"})
                .to_string();
        hub.meta
            .execute(
                "UPDATE session_recovery SET data=?1 WHERE id=1",
                [preserved.as_bytes()],
            )
            .unwrap();
        let artifact = hub.set_aside_recovery().unwrap().unwrap();
        assert_eq!(std::fs::read(artifact).unwrap(), preserved.as_bytes());
        assert_eq!(get(&mut hub, profile, space), kept);
        assert!(hub
            .load()
            .unwrap()
            .unwrap()
            .spaces
            .iter()
            .any(|scope| scope.id == space
                && scope.profile == profile
                && scope.name == "Recovered drafts"));
    }

    #[test]
    fn excess_text_rows_and_profile_bytes_refuse_without_erasing_prior_words() {
        let state = state(1, 66);
        let profile = state.profiles[0].id;
        let mut hub = Hub::in_memory().unwrap();
        hub.save(&state).unwrap();
        for scope in state.spaces.iter().take(MAX_COMPOSER_DRAFTS_PER_PROFILE) {
            let draft = get(&mut hub, profile, scope.id);
            save(&mut hub, &draft, "retained").unwrap();
        }
        let next = get(&mut hub, profile, state.spaces[64].id);
        assert_eq!(save(&mut hub, &next, "too many"), Err(WorkError::Capacity));
        let first = get(&mut hub, profile, state.spaces[0].id);
        assert_eq!(
            save(
                &mut hub,
                &first,
                &"🦊".repeat(MAX_COMPOSER_DRAFT_UTF16 / 2 + 1)
            ),
            Err(WorkError::Capacity)
        );
        assert_eq!(get(&mut hub, profile, first.space).text, "retained");
        let empty = save(&mut hub, &first, "").unwrap();
        save(&mut hub, &next, "room reclaimed").unwrap();
        assert_eq!(
            save(&mut hub, &first, "late revision"),
            Err(WorkError::Conflict)
        );
        assert!(get(&mut hub, profile, empty.space).text.is_empty());

        let mut hub = Hub::in_memory().unwrap();
        hub.save(&state).unwrap();
        for scope in state.spaces.iter().take(21) {
            let draft = get(&mut hub, profile, scope.id);
            save(&mut hub, &draft, &"界".repeat(MAX_COMPOSER_DRAFT_UTF16)).unwrap();
        }
        let final_room = get(&mut hub, profile, state.spaces[21].id);
        save(&mut hub, &final_room, &"x".repeat(MAX_COMPOSER_DRAFT_UTF16)).unwrap();
        let full = get(&mut hub, profile, state.spaces[22].id);
        assert_eq!(save(&mut hub, &full, "x"), Err(WorkError::Capacity));
        assert_eq!(
            get(&mut hub, profile, state.spaces[0].id).text,
            "界".repeat(MAX_COMPOSER_DRAFT_UTF16)
        );
    }

    #[test]
    fn session_scope_erasure_and_recovery_preserve_only_still_owned_drafts() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(2, 2);
        let mut hub = Hub::open(dir.path().to_path_buf()).unwrap();
        hub.save(&state).unwrap();
        let removed = get(&mut hub, state.profiles[0].id, state.spaces[0].id);
        save(&mut hub, &removed, "deleted space words").unwrap();
        let retained = get(&mut hub, state.profiles[0].id, state.spaces[1].id);
        let retained = save(&mut hub, &retained, "recover these unsent words").unwrap();
        let other = get(&mut hub, state.profiles[1].id, state.spaces[2].id);
        save(&mut hub, &other, "other profile").unwrap();
        state.spaces.remove(0);
        state.active_space = Some(state.spaces[0].id);
        hub.save(&state).unwrap();
        assert_eq!(
            call(
                &mut hub,
                removed.profile,
                WorkEnvironmentCall::ComposerRead {
                    space: removed.space
                },
                true
            ),
            Err(WorkError::NotFound)
        );
        let before: Vec<u8> = hub
            .meta
            .query_row(
                "SELECT CAST(data AS BLOB) FROM session_snapshot WHERE id=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        hub.quarantine_current_authoritative("fixture failed startup")
            .unwrap();
        let preserved = hub.set_aside_recovery().unwrap().unwrap();
        assert_eq!(std::fs::read(preserved).unwrap(), before);
        assert_eq!(get(&mut hub, retained.profile, retained.space), retained);
        assert!(hub
            .load()
            .unwrap()
            .unwrap()
            .spaces
            .iter()
            .any(|scope| scope.id == retained.space));
        let removed_profile = state.profiles[0].id;
        state.profiles.remove(0);
        state.profiles[0].kind = ProfileKind::Default;
        state
            .spaces
            .retain(|scope| scope.profile != removed_profile);
        state.active_space = state.spaces.first().map(|scope| scope.id);
        hub.authorize_profile_deletion(removed_profile, &state)
            .unwrap();
        hub.quarantine_current_authoritative("fixture recovery after deletion")
            .unwrap();
        hub.set_aside_recovery().unwrap();
        assert!(hub
            .load()
            .unwrap()
            .unwrap()
            .spaces
            .iter()
            .all(|scope| scope.profile != removed_profile));
        assert_eq!(
            hub.meta
                .query_row(
                    "SELECT count(*) FROM work_composer_drafts WHERE profile_id=?1",
                    [removed_profile.to_string()],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}
