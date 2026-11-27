//! Atomic environment edits and retained replay receipts on the profile actor.
use super::{db, read as read_objective};
use rusqlite::{params, OptionalExtension, Transaction};
use sha2::{Digest, Sha256};
use zephium_core::{
    ids::ProfileId,
    work::{artifact::WorkArtifactDataV1, environment::*, *},
};

fn read(
    tx: &Transaction<'_>,
    profile: ProfileId,
    id: WorkEnvironmentId,
) -> Result<WorkEnvironmentSnapshot, WorkError> {
    let (body, view, space, revision, view_revision): (String, String, String, i64, i64) = tx
        .query_row(
            "SELECT body,view,space_id,revision,view_revision FROM work_environments WHERE id=?1",
            [id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(db)?
        .ok_or(WorkError::NotFound)?;
    if body.len() > MAX_ENVIRONMENT_BODY_BYTES || view.len() > 65536 {
        return Err(WorkError::Capacity);
    }
    let mut snapshot: WorkEnvironmentSnapshot =
        serde_json::from_str(&body).map_err(|_| WorkError::Unavailable)?;
    snapshot.view = serde_json::from_str(&view).map_err(|_| WorkError::Unavailable)?;
    snapshot.validate()?;
    if snapshot.id != id
        || snapshot.profile != profile
        || snapshot.space.to_string() != space
        || snapshot.revision.get() as i64 != revision
        || snapshot.view.revision.get() as i64 != view_revision
    {
        return Err(WorkError::Unavailable);
    }
    Ok(snapshot)
}

fn write(
    tx: &Transaction<'_>,
    snapshot: &WorkEnvironmentSnapshot,
    creating: bool,
) -> Result<(), WorkError> {
    snapshot.validate()?;
    let view = serde_json::to_string(&snapshot.view).map_err(|_| WorkError::Invalid)?;
    let mut semantic = snapshot.clone();
    semantic.view = WorkEnvironmentView::default();
    let body = serde_json::to_string(&semantic).map_err(|_| WorkError::Invalid)?;
    if body.len() > MAX_ENVIRONMENT_BODY_BYTES || view.len() > 65536 {
        return Err(WorkError::Capacity);
    }
    if creating {
        let count: usize = tx
            .query_row("SELECT count(*) FROM work_environments", [], |r| r.get(0))
            .map_err(db)?;
        if count >= MAX_WORKS_PER_PROFILE {
            return Err(WorkError::Capacity);
        }
    }
    let sql = if creating {
        "INSERT INTO work_environments(id,space_id,revision,view_revision,body,view) VALUES (?1,?2,?3,?4,?5,?6)"
    } else {
        "UPDATE work_environments SET space_id=?2,revision=?3,view_revision=?4,body=?5,view=?6 WHERE id=?1"
    };
    if tx
        .execute(
            sql,
            params![
                snapshot.id.to_string(),
                snapshot.space.to_string(),
                snapshot.revision.get() as i64,
                snapshot.view.revision.get() as i64,
                body,
                view
            ],
        )
        .map_err(db)?
        != 1
    {
        return Err(WorkError::Conflict);
    }
    Ok(())
}

fn validate_reference(
    tx: &Transaction<'_>,
    profile: ProfileId,
    reference: &WorkEnvironmentReference,
    note_available: bool,
) -> Result<(), WorkError> {
    match reference {
        WorkEnvironmentReference::Resource { resource } => {
            // A note is a file outside this store; the application actor
            // attests it. Any other resource must be live here.
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM user_resources WHERE id=?1 AND trashed=0)",
                    [resource.to_string()],
                    |r| r.get(0),
                )
                .map_err(db)?;
            if !exists && !note_available {
                return Err(WorkError::NotFound);
            }
        }
        WorkEnvironmentReference::Objective { objective } => {
            read_objective(tx, profile, *objective)?;
        }
        WorkEnvironmentReference::Artifact {
            objective,
            execution,
            artifact,
        } => {
            super::runtime_store::validate_artifact_reference(
                tx, profile, *objective, *execution, *artifact,
            )?;
        }
        WorkEnvironmentReference::Subject {
            objective,
            execution,
            artifact,
            index,
        } => {
            let value = super::runtime_store::read_artifact(
                tx, profile, *objective, *execution, *artifact,
            )?;
            let subjects = match &value.data {
                WorkArtifactDataV1::ComparisonMatrix { subjects, .. }
                | WorkArtifactDataV1::Findings { subjects, .. }
                | WorkArtifactDataV1::EvidenceCollection { subjects, .. } => subjects.len(),
                _ => 0,
            };
            if usize::from(*index) >= subjects {
                return Err(WorkError::NotFound);
            }
        }
        WorkEnvironmentReference::Finding {
            objective,
            execution,
            artifact,
            index,
        } => {
            let value = super::runtime_store::read_artifact(
                tx, profile, *objective, *execution, *artifact,
            )?;
            let items = match &value.data {
                WorkArtifactDataV1::Findings { items, .. } => items.len(),
                _ => 0,
            };
            if usize::from(*index) >= items {
                return Err(WorkError::NotFound);
            }
        }
        WorkEnvironmentReference::Source {
            objective,
            execution,
            artifact,
            index,
        } => {
            let value = super::runtime_store::read_artifact(
                tx, profile, *objective, *execution, *artifact,
            )?;
            let entries = match &value.data {
                WorkArtifactDataV1::EvidenceCollection { entries, .. } => entries.len(),
                _ => 0,
            };
            if usize::from(*index) >= entries {
                return Err(WorkError::NotFound);
            }
        }
        // Native tab ownership is validated by the application actor. The Store
        // retains only the ID; it neither opens a URL nor constructs a context.
        WorkEnvironmentReference::Browser { .. } => {}
        // Admitted by the application before placement; the run re-checks it.
        WorkEnvironmentReference::Folder { .. } => {}
        // Validated as a public URL on the edit; nothing to look up.
        WorkEnvironmentReference::Link { .. } => {}
    }
    Ok(())
}

pub(super) fn apply(
    tx: &Transaction<'_>,
    profile: ProfileId,
    call: WorkEnvironmentCall,
    space_available: bool,
    browser_available: bool,
    note_available: bool,
) -> Result<(WorkEnvironmentReply, bool), WorkError> {
    call.validate()?;
    match call {
        WorkEnvironmentCall::ComposerRead { .. } | WorkEnvironmentCall::ComposerSave { .. } => {
            Err(WorkError::Invalid)
        }
        WorkEnvironmentCall::Checkpoint { id, expected, view } => {
            checkpoint(tx, profile, id, expected, view)
        }
        WorkEnvironmentCall::Open { id } => {
            let snapshot = read(tx, profile, id)?;
            if snapshot.lifecycle != WorkLifecycle::Active {
                return Err(WorkError::Conflict);
            }
            select(tx, &snapshot)?;
            Ok((
                WorkEnvironmentReply::Snapshot {
                    snapshot: Box::new(snapshot),
                },
                true,
            ))
        }
        WorkEnvironmentCall::Read { id } => Ok((
            WorkEnvironmentReply::Snapshot {
                snapshot: Box::new(read(tx, profile, id)?),
            },
            false,
        )),
        WorkEnvironmentCall::List {
            space,
            after,
            limit,
        } => {
            if !space_available {
                return Err(WorkError::NotFound);
            }
            let mut query = tx.prepare("SELECT id FROM work_environments WHERE space_id=?1 AND (?2 IS NULL OR id>?2) ORDER BY id LIMIT ?3").map_err(db)?;
            let ids = query
                .query_map(
                    params![
                        space.to_string(),
                        after.map(|id| id.to_string()),
                        i64::from(limit) + 1
                    ],
                    |r| r.get::<_, String>(0),
                )
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            let more = ids.len() > usize::from(limit);
            let mut works = vec![];
            for id in ids.into_iter().take(usize::from(limit)) {
                let id = WorkEnvironmentId::parse(&id).ok_or(WorkError::Unavailable)?;
                let snapshot = read(tx, profile, id)?;
                works.push(summary(tx, snapshot)?);
            }
            let next = if more {
                works.last().map(|w| w.id)
            } else {
                None
            };
            let selected: Option<String> = tx
                .query_row(
                    "SELECT environment_id FROM work_environment_selection WHERE space_id=?1",
                    [space.to_string()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            let selected = selected
                .map(|value| WorkEnvironmentId::parse(&value).ok_or(WorkError::Unavailable))
                .transpose()?;
            if let Some(id) = selected {
                let snapshot = read(tx, profile, id).map_err(|_| WorkError::Unavailable)?;
                if snapshot.space != space || snapshot.lifecycle != WorkLifecycle::Active {
                    return Err(WorkError::Unavailable);
                }
            }
            Ok((
                WorkEnvironmentReply::Page {
                    works,
                    next,
                    selected,
                },
                false,
            ))
        }
        WorkEnvironmentCall::Command { command, intent } => command_apply(
            tx,
            profile,
            command,
            intent,
            space_available,
            browser_available,
            note_available,
        ),
    }
}

/// A work as its list shows it: what its runs asked and were called, and when it was last worked in.
fn summary(
    tx: &Transaction<'_>,
    snapshot: WorkEnvironmentSnapshot,
) -> Result<WorkEnvironmentSummary, WorkError> {
    let mut requests: Vec<String> = vec![];
    let mut titled: Option<String> = None;
    let mut replied: Option<String> = None;
    let mut touched = 0_i64;
    let mut runs = tx
        .prepare_cached(
            "SELECT json_extract(body,'$.spec.request'), json_extract(body,'$.title'), \
             (SELECT json_extract(a.value,'$.title') FROM json_each(body,'$.artifacts') a \
              WHERE json_extract(a.value,'$.data.kind') IN ('reply','answer') LIMIT 1), \
             approved_unix_ms FROM work_executions WHERE work_id=?1 ORDER BY approved_unix_ms",
        )
        .map_err(db)?;
    let mut objective = tx
        .prepare_cached("SELECT objective, updated_unix_ms FROM works WHERE id=?1")
        .map_err(db)?;
    let mut seen = std::collections::HashSet::new();
    for element in &snapshot.elements {
        let WorkEnvironmentReference::Objective { objective: work } = &element.reference else {
            continue;
        };
        if !seen.insert(*work) {
            continue;
        }
        let id = work.to_string();
        let Some((text, updated)) = objective
            .query_row([&id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .optional()
            .map_err(db)?
        else {
            continue;
        };
        touched = touched.max(updated);
        type Run = (Option<String>, Option<String>, Option<String>, i64);
        let rows = runs
            .query_map([&id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(db)?
            .collect::<Result<Vec<Run>, _>>()
            .map_err(db)?;
        if rows.is_empty() {
            push_request(&mut requests, &text);
        }
        for (index, (request, title, reply, approved)) in rows.into_iter().enumerate() {
            touched = touched.max(approved);
            let asked = request.filter(|request| !request.trim().is_empty());
            push_request(
                &mut requests,
                asked
                    .as_deref()
                    .unwrap_or(if index == 0 { &text } else { "" }),
            );
            // The work's first request names it: the first of its runs to give a
            // title, else what its first run's reply was called.
            if seen.len() == 1 {
                let given = |name: Option<String>| {
                    name.map(|name| name.trim().to_owned())
                        .filter(|name| !name.is_empty())
                };
                if titled.is_none() {
                    titled = given(title);
                }
                if index == 0 {
                    replied = given(reply);
                }
            }
        }
    }
    Ok(WorkEnvironmentSummary {
        id: snapshot.id,
        space: snapshot.space,
        empty: snapshot.elements.is_empty() && snapshot.areas.is_empty(),
        title: snapshot.title,
        lifecycle: snapshot.lifecycle,
        revision: snapshot.revision,
        name: titled
            .or(replied)
            .map(|name| cut(&name, MAX_SUMMARY_REQUEST_CHARS)),
        requests,
        touched_ms: touched.max(0).to_string(),
    })
}

fn push_request(requests: &mut Vec<String>, text: &str) {
    let line = cut(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        MAX_SUMMARY_REQUEST_CHARS,
    );
    if !line.is_empty() && requests.len() < MAX_SUMMARY_REQUESTS && !requests.contains(&line) {
        requests.push(line);
    }
}

fn cut(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

fn command_apply(
    tx: &Transaction<'_>,
    profile: ProfileId,
    command: WorkCommandId,
    intent: WorkEnvironmentIntent,
    space_available: bool,
    browser_available: bool,
    note_available: bool,
) -> Result<(WorkEnvironmentReply, bool), WorkError> {
    let bytes = serde_json::to_vec(&intent).map_err(|_| WorkError::Invalid)?;
    if bytes.len() > MAX_ENVIRONMENT_BODY_BYTES {
        return Err(WorkError::Capacity);
    }
    let digest = Sha256::digest(bytes).to_vec();
    let receipt: Option<(Vec<u8>, String, i64, i64)> = tx.query_row(
        "SELECT digest,environment_id,revision,view_revision FROM work_environment_commands WHERE command_id=?1",
        [command.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional().map_err(db)?;
    if let Some((previous, id, revision, view_revision)) = receipt {
        if previous != digest {
            return Err(WorkError::Conflict);
        }
        let id = WorkEnvironmentId::parse(&id).ok_or(WorkError::Unavailable)?;
        let applied_revision = u64::try_from(revision)
            .ok()
            .and_then(WorkRevision::new)
            .ok_or(WorkError::Unavailable)?;
        let applied_view_revision = u64::try_from(view_revision)
            .ok()
            .and_then(WorkRevision::new)
            .ok_or(WorkError::Unavailable)?;
        return Ok((
            WorkEnvironmentReply::Applied {
                command,
                applied_revision,
                applied_view_revision,
                replayed: true,
                snapshot: Box::new(read(tx, profile, id)?),
            },
            false,
        ));
    }
    let receipts: usize = tx
        .query_row("SELECT count(*) FROM work_environment_commands", [], |r| {
            r.get(0)
        })
        .map_err(db)?;
    if receipts >= 8192 {
        return Err(WorkError::Capacity);
    }
    let creating = matches!(intent, WorkEnvironmentIntent::Create { .. });
    let snapshot = match intent {
        WorkEnvironmentIntent::Create { space, title } => {
            if !space_available {
                return Err(WorkError::NotFound);
            }
            WorkEnvironmentSnapshot::create(WorkEnvironmentId::generate(), profile, space, title)?
        }
        WorkEnvironmentIntent::Edit { id, expected, edit } => {
            let current = read(tx, profile, id)?;
            if current.revision != expected {
                return Err(WorkError::Conflict);
            }
            if let WorkEnvironmentEdit::Add { reference, .. } = &edit {
                if matches!(reference, WorkEnvironmentReference::Browser { .. })
                    && !browser_available
                {
                    return Err(WorkError::NotFound);
                }
                validate_reference(tx, profile, reference, note_available)?;
            }
            current.edit(
                edit,
                WorkElementId::generate(),
                WorkAreaId::generate(),
                WorkRelationId::generate(),
            )?
        }
    };
    write(tx, &snapshot, creating)?;
    if creating {
        select(tx, &snapshot)?;
    }
    if snapshot.lifecycle == WorkLifecycle::Archived {
        tx.execute(
            "DELETE FROM work_environment_selection WHERE environment_id=?1",
            [snapshot.id.to_string()],
        )
        .map_err(db)?;
    }
    tx.execute("INSERT INTO work_environment_commands(command_id,digest,environment_id,revision,view_revision) VALUES (?1,?2,?3,?4,?5)",
        params![command.to_string(), digest, snapshot.id.to_string(), snapshot.revision.get() as i64, snapshot.view.revision.get() as i64]).map_err(db)?;
    Ok((
        WorkEnvironmentReply::Applied {
            command,
            applied_revision: snapshot.revision,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot: Box::new(snapshot),
        },
        true,
    ))
}

/// The identity is (environment, expected view revision), not a reusable random
/// command ID. Pruned identities remain stale forever because revisions never
/// decrease, including after removal, archive and restore.
fn checkpoint(
    tx: &Transaction<'_>,
    profile: ProfileId,
    id: WorkEnvironmentId,
    expected: WorkRevision,
    view: WorkEnvironmentView,
) -> Result<(WorkEnvironmentReply, bool), WorkError> {
    let mut current = read(tx, profile, id)?;
    let bytes = serde_json::to_vec(&view).map_err(|_| WorkError::Invalid)?;
    if bytes.len() > 65536 {
        return Err(WorkError::Capacity);
    }
    let digest = Sha256::digest(bytes).to_vec();
    let receipt: Option<(Vec<u8>, i64)> = tx.query_row(
        "SELECT digest,applied_revision FROM work_environment_checkpoints WHERE environment_id=?1 AND expected_revision=?2",
        params![id.to_string(), expected.get() as i64], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional().map_err(db)?;
    let applied = expected.next()?;
    if let Some((previous, revision)) = receipt {
        if previous != digest {
            return Err(WorkError::Conflict);
        }
        if revision != applied.get() as i64 || current.view.revision < applied {
            return Err(WorkError::Unavailable);
        }
        return Ok((
            WorkEnvironmentReply::Checkpointed {
                expected,
                applied_view_revision: applied,
                replayed: true,
                snapshot: Box::new(current),
            },
            false,
        ));
    }
    if current.lifecycle != WorkLifecycle::Active || current.view.revision != expected {
        return Err(WorkError::Conflict);
    }
    current.view = view;
    current.view.revision = applied;
    current.validate()?;
    write(tx, &current, false)?;
    // Retain at most 64 exact receipts for this environment. This is not silent
    // random-command eviction: older expected revisions are permanently stale.
    tx.execute("DELETE FROM work_environment_checkpoints WHERE environment_id=?1 AND expected_revision NOT IN (SELECT expected_revision FROM work_environment_checkpoints WHERE environment_id=?1 ORDER BY expected_revision DESC LIMIT 63)", [id.to_string()]).map_err(db)?;
    tx.execute("INSERT INTO work_environment_checkpoints(environment_id,expected_revision,digest,applied_revision) VALUES (?1,?2,?3,?4)", params![id.to_string(),expected.get() as i64,digest,applied.get() as i64]).map_err(db)?;
    Ok((
        WorkEnvironmentReply::Checkpointed {
            expected,
            applied_view_revision: applied,
            replayed: false,
            snapshot: Box::new(current),
        },
        true,
    ))
}

fn select(tx: &Transaction<'_>, snapshot: &WorkEnvironmentSnapshot) -> Result<(), WorkError> {
    tx.execute("INSERT INTO work_environment_selection(space_id,environment_id) VALUES (?1,?2) ON CONFLICT(space_id) DO UPDATE SET environment_id=excluded.environment_id",
        params![snapshot.space.to_string(), snapshot.id.to_string()]).map_err(db)?;
    Ok(())
}

#[cfg(test)]
#[path = "work_environment/tests.rs"]
mod tests;
