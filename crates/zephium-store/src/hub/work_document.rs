//! Product authoring on the normal per-profile Store connection. All edits and
//! their content-free semantic event commit atomically; old plans are retained.
use super::Hub;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use zephium_core::{
    ids::ProfileId,
    work::{port::*, *},
};

const MAX_BODY_BYTES: usize = 131072;
#[path = "work_authoring_commands.rs"]
mod authoring_store;
#[path = "work_environment.rs"]
mod environment_store;
#[path = "work_personal.rs"]
mod personal_store;
#[path = "work_runtime.rs"]
pub(super) mod runtime_store;
const _: [(); 512] = [(); MAX_WORKS_PER_PROFILE];
const _: [(); 256] = [(); MAX_ACTIVE_WORKS_PER_PROFILE];
const _: [(); 32] = [(); MAX_WORK_PLAN_REVISIONS];
const _: [(); 64] = [(); MAX_WORK_NODES];
const _: [(); 32] = [(); MAX_WORK_QUESTIONS];
const _: [(); 2048] = [(); MAX_WORK_EVENTS];
const _: [(); 41943040] = [(); MAX_WORK_PROFILE_BYTES];

const SITE_ACCESS_KEY: &str = "work.site_access";

/// The profile's standing site answers, kept sorted in its settings table.
fn site_access(
    conn: &mut Connection,
    set: Option<(String, Option<sites::WorkSiteAccessV1>)>,
) -> Result<Vec<sites::WorkSiteEntryV1>, WorkError> {
    let tx = conn.transaction().map_err(|_| WorkError::Unavailable)?;
    let stored: Option<String> = tx
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            [SITE_ACCESS_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| WorkError::Unavailable)?;
    let mut entries: Vec<sites::WorkSiteEntryV1> = match stored {
        Some(value) => serde_json::from_str(&value).map_err(|_| WorkError::Invalid)?,
        None => Vec::new(),
    };
    if entries.len() > sites::MAX_WORK_SITE_ACCESS
        || entries
            .iter()
            .any(|entry| sites::validate_site(&entry.site).is_err())
    {
        return Err(WorkError::Invalid);
    }
    let Some((site, access)) = set else {
        return Ok(entries);
    };
    sites::validate_site(&site)?;
    entries.retain(|entry| entry.site != site);
    if let Some(access) = access {
        if entries.len() >= sites::MAX_WORK_SITE_ACCESS {
            return Err(WorkError::Capacity);
        }
        entries.push(sites::WorkSiteEntryV1 { site, access });
        entries.sort_by(|a, b| a.site.cmp(&b.site));
    }
    let value = serde_json::to_string(&entries).map_err(|_| WorkError::Invalid)?;
    tx.execute(
        "INSERT INTO settings(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![SITE_ACCESS_KEY, value],
    )
    .map_err(|_| WorkError::Unavailable)?;
    tx.commit().map_err(|_| WorkError::Unavailable)?;
    Ok(entries)
}

const SCREENSHOT_ACCESS_KEY: &str = "work.screenshot_access";
fn screenshot_access(
    conn: &mut Connection,
    set: Option<(String, bool)>,
) -> Result<Vec<screenshots::WorkScreenshotSiteV1>, WorkError> {
    let tx = conn.transaction().map_err(|_| WorkError::Unavailable)?;
    let stored: Option<String> = tx.query_row(
        "SELECT CASE WHEN length(CAST(value AS BLOB)) <= 262144 THEN value END FROM settings WHERE key = ?1",
        [SCREENSHOT_ACCESS_KEY], |row| row.get(0),
    ).optional().map_err(|_| WorkError::Unavailable)?;
    let mut entries: Vec<screenshots::WorkScreenshotSiteV1> = match stored {
        Some(value) => serde_json::from_str(&value).map_err(|_| WorkError::Invalid)?,
        None => Vec::new(),
    };
    if entries.len() > screenshots::MAX_SCREENSHOT_SITES
        || entries
            .iter()
            .any(|entry| screenshots::validate_origin(&entry.origin).is_err())
        || entries
            .windows(2)
            .any(|pair| pair[0].origin >= pair[1].origin)
    {
        return Err(WorkError::Invalid);
    }
    let Some((origin, allow)) = set else {
        return Ok(entries);
    };
    screenshots::validate_origin(&origin)?;
    entries.retain(|entry| entry.origin != origin);
    if allow {
        if entries.len() >= screenshots::MAX_SCREENSHOT_SITES {
            return Err(WorkError::Capacity);
        }
        entries.push(screenshots::WorkScreenshotSiteV1 { origin });
        entries.sort_by(|a, b| a.origin.cmp(&b.origin));
    }
    let value = serde_json::to_string(&entries).map_err(|_| WorkError::Invalid)?;
    tx.execute(
        "INSERT INTO settings(key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
        params![SCREENSHOT_ACCESS_KEY, value],
    )
    .map_err(|_| WorkError::Unavailable)?;
    tx.commit().map_err(|_| WorkError::Unavailable)?;
    Ok(entries)
}

impl Hub {
    pub(crate) fn work_document(
        &mut self,
        profile: ProfileId,
        request: WorkRequest,
    ) -> Result<WorkReply, WorkError> {
        request.validate()?;
        if !self.registry.contains(&profile) || self.degraded_profiles.contains(&profile) {
            return Err(WorkError::ProfileUnavailable);
        }
        if self.recovery_required.is_some() {
            return Err(WorkError::Unavailable);
        }
        if let WorkRequest::Personal(request) = request {
            use zephium_core::work::personal::{
                WorkHistoryHit, WorkPersonalReply, WorkPersonalRequest,
            };
            if let WorkPersonalRequest::SearchHistory { query, limit } = &request {
                let hits = self
                    .search_history(profile, query, u32::from(*limit))
                    .into_iter()
                    .map(|hit| WorkHistoryHit {
                        url: hit.url,
                        title: hit.title,
                        last_visit: hit.last_visit,
                    })
                    .collect();
                return Ok(WorkReply::Personal(WorkPersonalReply::History(hits)));
            }
            if let WorkPersonalRequest::RecentHistory {
                since,
                until,
                limit,
            } = &request
            {
                let mut seen = std::collections::BTreeSet::new();
                let hits = self
                    .history_page_checked(
                        profile,
                        "",
                        Some(*since),
                        None,
                        super::history::MAX_HISTORY_PAGE,
                    )
                    .map_err(|_| WorkError::Unavailable)?
                    .into_iter()
                    .filter(|visit| visit.visited_at < *until && seen.insert(visit.url.clone()))
                    .take(usize::from(*limit))
                    .map(|visit| WorkHistoryHit {
                        url: visit.url,
                        title: visit.title,
                        last_visit: visit.visited_at,
                    })
                    .collect();
                return Ok(WorkReply::Personal(WorkPersonalReply::History(hits)));
            }
            let conn = self
                .profile_conn(profile)
                .map_err(|_| WorkError::Unavailable)?;
            return personal_store::call(conn, request).map(WorkReply::Personal);
        }
        if let WorkRequest::SiteAccess { set } = request {
            let conn = self
                .profile_conn(profile)
                .map_err(|_| WorkError::Unavailable)?;
            return site_access(conn, set).map(WorkReply::SiteAccess);
        }
        if let WorkRequest::ScreenshotAccess { set } = request {
            let conn = self
                .profile_conn(profile)
                .map_err(|_| WorkError::Unavailable)?;
            return screenshot_access(conn, set).map(WorkReply::ScreenshotAccess);
        }
        if let WorkRequest::ReadMediaContext { resource, revision } = &request {
            use sha2::{Digest, Sha256};
            use zephium_core::resources::{
                ResourceCall, ResourceContent, ResourceResponse, MAX_MEDIA_FILE_BYTES,
            };
            let ResourceResponse::Record { record } = self.resource_call(
                profile,
                ResourceCall::Get {
                    id: resource.clone(),
                },
            ) else {
                return Err(WorkError::NotFound);
            };
            if record.trashed {
                return Err(WorkError::NotFound);
            }
            if &record.revision != revision {
                return Err(WorkError::Conflict);
            }
            let ResourceContent::Media { asset } = record.draft.content else {
                return Err(WorkError::Invalid);
            };
            let bytes = self
                .media
                .as_ref()
                .and_then(|media| media.read(profile, &asset.digest, MAX_MEDIA_FILE_BYTES as usize))
                .ok_or(WorkError::NotFound)?;
            if bytes.len() != asset.bytes as usize
                || format!("{:x}", Sha256::digest(&bytes)) != asset.digest
            {
                return Err(WorkError::Conflict);
            }
            return Ok(WorkReply::MediaContext(WorkMediaContext(bytes)));
        }
        let runtime_session = runtime_store::RuntimeClock {
            session: self.work_runtime_session,
            tick_ms: i64::try_from(self.work_runtime_epoch.elapsed().as_millis())
                .map_err(|_| WorkError::Unavailable)?,
        };
        if let WorkRequest::PutRecordMedia { media, .. } | WorkRequest::ReadRecordMedia { media } =
            &request
        {
            #[cfg(not(feature = "work-execution"))]
            return Err(WorkError::Unavailable);
            #[cfg(feature = "work-execution")]
            {
                let conn = self
                    .profile_conn(profile)
                    .map_err(|_| WorkError::Unavailable)?;
                let projection =
                    runtime_store::projection(conn, profile, media.work, runtime_session)?;
                let execution = projection
                    .executions
                    .iter()
                    .find(|e| e.id == media.execution)
                    .ok_or(WorkError::NotFound)?;
                let attempt = execution
                    .attempts
                    .iter()
                    .find(|a| a.id == media.attempt)
                    .ok_or(WorkError::NotFound)?;
                let step = execution
                    .steps
                    .iter()
                    .find(|s| s.id == media.step)
                    .ok_or(WorkError::NotFound)?;
                if !matches!(step.kind, runtime::WorkStepKindV1::Read { .. }) {
                    return Err(WorkError::Invalid);
                }
                if let WorkRequest::PutRecordMedia { png, .. } = &request {
                    if execution.status != runtime::WorkExecutionStatus::Running
                        || attempt.status != runtime::WorkAttemptStatus::Running
                        || step.status != runtime::WorkStepStatus::Running
                        || projection.interrupted.contains(&execution.id)
                    {
                        return Err(WorkError::Conflict);
                    }
                    self.record_media(profile, media, Some(png))?;
                    return Ok(WorkReply::RecordMediaStored(media.clone()));
                }
                return self
                    .record_media(profile, media, None)?
                    .map(WorkReply::RecordMedia)
                    .ok_or(WorkError::NotFound);
            }
        }
        if let WorkRequest::ReadEvidence { id, link } = &request {
            // Both databases are owned by this actor. Validate the selected
            // Work/profile first; no mutation can interleave the archive read.
            let conn = self
                .profile_conn(profile)
                .map_err(|_| WorkError::Unavailable)?;
            let state = runtime_store::projection(conn, profile, *id, runtime_session)?;
            if !state
                .executions
                .iter()
                .flat_map(|e| &e.artifacts)
                .flat_map(|a| &a.evidence)
                .any(|candidate| candidate == link)
            {
                return Err(WorkError::NotFound);
            }
            // Provider attribution is stored in this same Work execution,
            // never reconstructed as a native archive or browser reference.
            if let Some(source) = state.executions.iter().find_map(|execution| {
                execution
                    .artifacts
                    .iter()
                    .any(|a| a.evidence.contains(link))
                    .then(|| {
                        execution
                            .provider_evidence
                            .iter()
                            .find(|source| source.id == link.extraction_id)
                    })
                    .flatten()
            }) {
                use zephium_core::work::artifact::{WorkEvidencePreviewV1, WorkEvidenceSourceV1};
                let index = link.source_id.checked_sub(1).ok_or(WorkError::Invalid)?;
                let citation = source
                    .evidence
                    .citations
                    .get(usize::from(index))
                    .ok_or(WorkError::NotFound)?;
                let source_bytes = source.evidence.answer.len();
                let text = source.evidence.citation_excerpt(usize::from(index))?;
                return Ok(WorkReply::Evidence(WorkEvidencePreviewV1 {
                    link_destination: None,
                    version: 1,
                    link: link.clone(),
                    origin: url::Url::parse(&citation.url)
                        .map_err(|_| WorkError::Invalid)?
                        .origin()
                        .ascii_serialization(),
                    role: "provider_search".into(),
                    truncated: text.len() < source_bytes,
                    text,
                    source_bytes: source_bytes.to_string(),
                    source: WorkEvidenceSourceV1::ProviderSearch {
                        provider: source.evidence.provider,
                        model: source.evidence.model.clone(),
                        url: citation.url.clone(),
                        title: citation.title.clone(),
                        response_id: source.evidence.response_id.clone(),
                        search_call_id: source.evidence.search_call_id.clone(),
                    },
                }));
            }
            if let Some(record) = state.executions.iter().find_map(|execution| {
                execution
                    .file_evidence
                    .iter()
                    .find(|record| record.id == link.extraction_id)
            }) {
                use zephium_core::work::artifact::{WorkEvidencePreviewV1, WorkEvidenceSourceV1};
                if link.source_id != 1 {
                    return Err(WorkError::NotFound);
                }
                let file = &record.file;
                let folder = std::path::Path::new(&file.path)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                return Ok(WorkReply::Evidence(WorkEvidencePreviewV1 {
                    link_destination: None,
                    version: 1,
                    link: link.clone(),
                    origin: format!("file://{folder}"),
                    role: "file".into(),
                    truncated: file.truncated,
                    text: file.text.clone(),
                    source_bytes: file.bytes.to_string(),
                    source: WorkEvidenceSourceV1::File {
                        path: file.path.clone(),
                        name: file.name.clone(),
                        file_kind: file.kind,
                    },
                }));
            }
            if let Some(record) = state
                .executions
                .iter()
                .flat_map(|e| &e.command_evidence)
                .find(|r| r.id == link.extraction_id)
            {
                use zephium_core::work::artifact::{WorkEvidencePreviewV1, WorkEvidenceSourceV1};
                if link.source_id != 1 {
                    return Err(WorkError::NotFound);
                }
                let command = &record.command;
                return Ok(WorkReply::Evidence(WorkEvidencePreviewV1 {
                    version: 1,
                    link: link.clone(),
                    link_destination: None,
                    origin: format!("file://{}", command.cwd),
                    role: "command".into(),
                    truncated: command.truncated,
                    text: command.text.clone(),
                    source_bytes: command.bytes.to_string(),
                    source: WorkEvidenceSourceV1::Command {
                        cwd: command.cwd.clone(),
                        command: command.command.clone(),
                        outcome: command.outcome(),
                    },
                }));
            }
            #[cfg(feature = "work-execution")]
            return self
                .read_agent_work_evidence(profile, link.clone())
                .map(WorkReply::Evidence);
            #[cfg(not(feature = "work-execution"))]
            return Err(WorkError::Unavailable);
        }
        let conn = self
            .profile_conn(profile)
            .map_err(|_| WorkError::Unavailable)?;
        let tx = conn.transaction().map_err(|_| WorkError::Unavailable)?;
        let (reply, write) = apply(&tx, profile, runtime_session, request)?;
        tx.commit().map_err(|_| {
            if write {
                WorkError::OutcomeUnknown
            } else {
                WorkError::Unavailable
            }
        })?;
        #[cfg(feature = "work-execution")]
        if let WorkReply::Deleted { id } = &reply {
            self.remove_record_media(profile, *id);
        }
        #[cfg(test)]
        if write && tests::LOSE_COMMIT_ACK.with(|fault| fault.replace(false)) {
            return Err(WorkError::OutcomeUnknown);
        }
        Ok(reply)
    }
}
fn apply(
    tx: &Transaction<'_>,
    profile: ProfileId,
    runtime_session: runtime_store::RuntimeClock,
    request: WorkRequest,
) -> Result<(WorkReply, bool), WorkError> {
    let result = match request {
        WorkRequest::Environment {
            call,
            space_available,
            browser_available,
            note_available,
        } => {
            let (reply, write) = environment_store::apply(
                tx,
                profile,
                call,
                space_available,
                browser_available,
                note_available,
            )?;
            (WorkReply::Environment(reply), write)
        }
        WorkRequest::AuthoringCommand { command, intent } => {
            authoring_store::command(tx, profile, runtime_session, command, intent)?
        }
        WorkRequest::ReadEvidence { .. }
        | WorkRequest::PutRecordMedia { .. }
        | WorkRequest::ReadRecordMedia { .. }
        | WorkRequest::ReadMediaContext { .. }
        | WorkRequest::SiteAccess { .. }
        | WorkRequest::ScreenshotAccess { .. }
        | WorkRequest::Personal(_) => return Err(WorkError::Invalid),
        WorkRequest::AcknowledgeOrphan {
            id,
            expected,
            command,
            execution,
        } => runtime_store::acknowledge_orphan(
            tx,
            profile,
            runtime_session,
            id,
            expected,
            command,
            execution,
        )?,
        WorkRequest::RuntimeAbandon {
            id,
            execution,
            attempt,
        } => {
            let expected = read(tx, profile, id)?.revision;
            runtime_store::abandon(
                tx,
                profile,
                runtime_session,
                id,
                expected,
                execution,
                attempt,
            )?
        }
        WorkRequest::RuntimeRead { id } => (
            WorkReply::Runtime(Box::new(runtime_store::projection(
                tx,
                profile,
                id,
                runtime_session,
            )?)),
            false,
        ),
        WorkRequest::RuntimeCommand {
            id,
            expected,
            command,
            intent,
        } => runtime_store::command(
            tx,
            profile,
            runtime_session,
            id,
            expected,
            command,
            runtime_store::RuntimeIntentInput {
                intent,
                context: None,
            },
        )?,
        WorkRequest::RuntimeCommandDisclosed {
            id,
            expected,
            command,
            intent,
            context,
        } => runtime_store::command(
            tx,
            profile,
            runtime_session,
            id,
            expected,
            command,
            runtime_store::RuntimeIntentInput {
                intent,
                context: Some(context),
            },
        )?,
        WorkRequest::RuntimeUpdate {
            id,
            expected,
            update,
        } => runtime_store::update(tx, profile, runtime_session, id, expected, update)?,
        WorkRequest::Create {
            id,
            objective,
            author,
        } => {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM works WHERE id = ?1)",
                    [id.to_string()],
                    |r| r.get(0),
                )
                .map_err(db)?;
            if exists {
                return Err(WorkError::Conflict);
            }
            let count: usize = tx
                .query_row("SELECT count(*) FROM works", [], |r| r.get(0))
                .map_err(db)?;
            if count >= MAX_WORKS_PER_PROFILE {
                return Err(WorkError::Capacity);
            }
            let mut snapshot = WorkSnapshot::create(id, profile, objective)?;
            snapshot.objective_author = author;
            tx.execute("INSERT INTO works(id, schema_version, revision, status, objective, created_unix_ms, updated_unix_ms, lifecycle, objective_revision, context_revision, objective_author) VALUES (?1, 2, 1, 'draft', ?2, ?3, ?3, 'active', 1, 1, ?4)", params![id.to_string(), snapshot.objective, timestamp()?, author_name(author)]).map_err(db)?;
            append_event(tx, id, snapshot.revision, WorkEventKind::Created, author)?;
            (WorkReply::Snapshot(Box::new(snapshot)), true)
        }
        WorkRequest::Delete { id, expected } => {
            let current = read(tx, profile, id)?;
            runtime_store::require_idle(tx, id, current.revision)?;
            if current.revision != expected {
                return Err(WorkError::Conflict);
            }
            tx.execute(
                "DELETE FROM works WHERE id = ?1 AND revision = ?2",
                params![id.to_string(), expected.get() as i64],
            )
            .map_err(db)?;
            (WorkReply::Deleted { id }, true)
        }
        WorkRequest::Read { id } => (WorkReply::Snapshot(Box::new(read(tx, profile, id)?)), false),
        WorkRequest::ListPlans { id } => {
            let current = read(tx, profile, id)?;
            let mut query = tx
                .prepare(
                    "SELECT revision FROM work_plans WHERE work_id = ?1 ORDER BY revision LIMIT 33",
                )
                .map_err(db)?;
            let rows = query
                .query_map([id.to_string()], |r| r.get::<_, i64>(0))
                .map_err(db)?;
            let mut revisions = Vec::new();
            for row in rows {
                let revision = revision(row.map_err(db)?)?;
                if revisions.len() >= MAX_WORK_PLAN_REVISIONS
                    || revision.get() < 2
                    || revision > current.revision
                {
                    return Err(WorkError::Invalid);
                }
                revisions.push(revision);
            }
            (WorkReply::PlanHistory { revisions }, false)
        }
        WorkRequest::ReadPlan { id, revision } => {
            // Revalidate owner/current facts before exposing historical content.
            read(tx, profile, id)?;
            (WorkReply::Plan(read_plan(tx, id, revision)?), false)
        }
        WorkRequest::List { after, limit } => (list(tx, after, limit)?, false),
        WorkRequest::Edit {
            id,
            expected,
            edit,
            author,
        } => {
            let current = read(tx, profile, id)?;
            if !matches!(edit, WorkEdit::CompactHistory) {
                runtime_store::require_idle(tx, id, current.revision)?;
            }
            let (next, event) = current.apply(expected, edit, author)?;
            if event == WorkEventKind::HistoryCompacted {
                // Explicit user action. Current plan and active clarification
                // provenance survive; only obsolete authoring content is removed.
                tx.execute("DELETE FROM work_plans WHERE work_id = ?1 AND revision != coalesce((SELECT current_plan FROM works WHERE id = ?1), -1) AND NOT EXISTS (SELECT 1 FROM work_executions e WHERE e.work_id = ?1 AND e.plan_revision = work_plans.revision)", [id.to_string()]).map_err(db)?;
                tx.execute(
                    "DELETE FROM work_questions WHERE work_id = ?1",
                    [id.to_string()],
                )
                .map_err(db)?;
                let floor = current.revision.get().saturating_sub(63).max(1);
                tx.execute(
                    "DELETE FROM work_events WHERE work_id = ?1 AND revision < ?2",
                    params![id.to_string(), floor as i64],
                )
                .map_err(db)?;
                tx.execute(
                    "UPDATE works SET event_floor = max(event_floor, ?2) WHERE id = ?1",
                    params![id.to_string(), floor as i64],
                )
                .map_err(db)?;
            }
            if event == WorkEventKind::DraftReplaced {
                let count: usize = tx
                    .query_row(
                        "SELECT count(*) FROM work_plans WHERE work_id = ?1",
                        [id.to_string()],
                        |r| r.get(0),
                    )
                    .map_err(db)?;
                if count >= MAX_WORK_PLAN_REVISIONS {
                    return Err(WorkError::Capacity);
                }
                write_plan(tx, id, next.plan.as_ref().ok_or(WorkError::Invalid)?)?;
            }
            let changed = tx.execute("UPDATE works SET revision = ?3, status = ?4, objective = ?5, current_plan = ?6, updated_unix_ms = max(updated_unix_ms, ?7), lifecycle = ?8, objective_revision = ?9, context_revision = ?10, objective_author = ?11 WHERE id = ?1 AND revision = ?2",
                    params![id.to_string(), expected.get() as i64, next.revision.get() as i64, status(next.status), next.objective, next.plan.as_ref().map(|p| p.revision.get() as i64), timestamp()?, lifecycle_name(next.lifecycle), next.objective_revision.get() as i64, next.context_revision.get() as i64, author_name(next.objective_author)]).map_err(db)?;
            if changed != 1 {
                return Err(WorkError::Conflict);
            }
            for (position, question) in next.questions.iter().enumerate() {
                if event != WorkEventKind::HistoryCompacted
                    && current.questions.get(position) == Some(question)
                {
                    continue;
                }
                let body = encode(question)?;
                tx.execute("INSERT INTO work_questions(work_id, question_id, position, body) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(work_id, question_id) DO UPDATE SET body = excluded.body",
                        params![id.to_string(), question.id.to_string(), position as i64, body]).map_err(db)?;
            }
            append_event(tx, id, next.revision, event, author)?;
            (WorkReply::Snapshot(Box::new(next)), true)
        }
    };
    Ok(result)
}
fn timestamp() -> Result<i64, WorkError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .ok_or(WorkError::Unavailable)
}
fn db(error: rusqlite::Error) -> WorkError {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &error {
        if matches!(
            message.as_str(),
            "CHECK constraint failed: work_payload_budget"
                | "Active Work capacity exceeded"
                | "Work capacity exceeded"
                | "Work event capacity exceeded"
                | "Work plan capacity exceeded"
                | "Work execution capacity exceeded"
                | "Work command capacity exceeded"
                | "Work authoring command capacity exceeded"
        ) {
            return WorkError::Capacity;
        }
    }
    WorkError::Unavailable
}
fn author_name(value: WorkAuthor) -> &'static str {
    match value {
        WorkAuthor::User => "user",
        WorkAuthor::PrimaryAgent => "primary_agent",
        WorkAuthor::OtherAgent => "other_agent",
        WorkAuthor::LegacyUnknown => "legacy_unknown",
    }
}
fn parse_author(value: &str) -> Result<WorkAuthor, WorkError> {
    match value {
        "user" => Ok(WorkAuthor::User),
        "primary_agent" => Ok(WorkAuthor::PrimaryAgent),
        "other_agent" => Ok(WorkAuthor::OtherAgent),
        "legacy_unknown" => Ok(WorkAuthor::LegacyUnknown),
        _ => Err(WorkError::Invalid),
    }
}
fn lifecycle_name(value: WorkLifecycle) -> &'static str {
    match value {
        WorkLifecycle::Active => "active",
        WorkLifecycle::Archived => "archived",
    }
}
fn parse_lifecycle(value: &str) -> Result<WorkLifecycle, WorkError> {
    match value {
        "active" => Ok(WorkLifecycle::Active),
        "archived" => Ok(WorkLifecycle::Archived),
        _ => Err(WorkError::Invalid),
    }
}
fn status(value: WorkAuthoringStatus) -> &'static str {
    match value {
        WorkAuthoringStatus::Draft => "draft",
        WorkAuthoringStatus::NeedsInput => "needs_input",
        WorkAuthoringStatus::PlanReady => "plan_ready",
    }
}
fn parse_status(value: &str) -> Result<WorkAuthoringStatus, WorkError> {
    match value {
        "draft" => Ok(WorkAuthoringStatus::Draft),
        "needs_input" => Ok(WorkAuthoringStatus::NeedsInput),
        "plan_ready" => Ok(WorkAuthoringStatus::PlanReady),
        _ => Err(WorkError::Invalid),
    }
}
fn revision(value: i64) -> Result<WorkRevision, WorkError> {
    u64::try_from(value)
        .ok()
        .and_then(WorkRevision::new)
        .ok_or(WorkError::Invalid)
}
fn encode<T: serde::Serialize>(value: &T) -> Result<String, WorkError> {
    let body = serde_json::to_string(value).map_err(|_| WorkError::Invalid)?;
    if body.len() > MAX_BODY_BYTES {
        return Err(WorkError::Capacity);
    }
    Ok(body)
}
fn decode<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, WorkError> {
    if body.len() > MAX_BODY_BYTES {
        return Err(WorkError::Invalid);
    }
    crate::bounded_json::preflight(body).map_err(|_| WorkError::Invalid)?;
    serde_json::from_str(body).map_err(|_| WorkError::Invalid)
}
fn append_event(
    tx: &Transaction<'_>,
    id: WorkId,
    revision: WorkRevision,
    event: WorkEventKind,
    author: WorkAuthor,
) -> Result<(), WorkError> {
    let kind = match event {
        WorkEventKind::RuntimeChanged => "runtime_changed",
        WorkEventKind::Archived => "archived",
        WorkEventKind::Restored => "restored",
        WorkEventKind::HistoryCompacted => "history_compacted",
        WorkEventKind::QuestionDismissed => "question_dismissed",
        WorkEventKind::Created => "created",
        WorkEventKind::ObjectiveEdited => "objective_edited",
        WorkEventKind::QuestionOpened => "question_opened",
        WorkEventKind::QuestionAnswered => "question_answered",
        WorkEventKind::DraftReplaced => "draft_replaced",
    };
    tx.execute("INSERT INTO work_events(work_id, revision, kind, recorded_unix_ms, author) VALUES (?1, ?2, ?3, (SELECT updated_unix_ms FROM works WHERE id = ?1), ?4)", params![id.to_string(), revision.get() as i64, kind, author_name(author)]).map_err(db)?;
    Ok(())
}
fn read(conn: &Connection, profile: ProfileId, id: WorkId) -> Result<WorkSnapshot, WorkError> {
    let row = conn
        .query_row(
            "SELECT schema_version, revision,
        CASE WHEN length(CAST(status AS BLOB)) <= 16 THEN status END,
        CASE WHEN length(CAST(objective AS BLOB)) <= 8192 THEN objective END,
        current_plan, CASE WHEN length(CAST(lifecycle AS BLOB)) <= 16 THEN lifecycle END, objective_revision, context_revision, CASE WHEN length(CAST(objective_author AS BLOB)) <= 32 THEN objective_author END, event_floor FROM works WHERE id = ?1",
            [id.to_string()],
            |r| {
                Ok((
                    r.get::<_, u16>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, i64>(9)?,
                ))
            },
        )
        .optional()
        .map_err(db)?
        .ok_or(WorkError::NotFound)?;
    let rev = revision(row.1)?;
    let mut statement = conn.prepare("SELECT CASE WHEN length(CAST(question_id AS BLOB)) = 26 THEN question_id END, position, CASE WHEN length(CAST(body AS BLOB)) <= 131072 THEN body END FROM work_questions WHERE work_id = ?1 ORDER BY position LIMIT 33").map_err(db)?;
    let rows = statement
        .query_map([id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, usize>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(db)?;
    let mut questions = Vec::new();
    for row in rows {
        if questions.len() >= MAX_WORK_QUESTIONS {
            return Err(WorkError::Invalid);
        }
        let (key, position, body) = row.map_err(db)?;
        let question: WorkQuestion = decode(&body.ok_or(WorkError::Invalid)?)?;
        if position != questions.len() || key != question.id.to_string() {
            return Err(WorkError::Invalid);
        }
        question.validate()?;
        questions.push(question);
    }
    let snapshot = WorkSnapshot {
        lifecycle: parse_lifecycle(&row.5)?,
        objective_revision: revision(row.6)?,
        context_revision: revision(row.7)?,
        objective_author: parse_author(&row.8)?,
        schema_version: row.0,
        id,
        profile,
        revision: rev,
        status: parse_status(&row.2.ok_or(WorkError::Invalid)?)?,
        objective: row.3.ok_or(WorkError::Invalid)?,
        plan: row
            .4
            .map(|v| revision(v).and_then(|v| read_plan(conn, id, v)))
            .transpose()?,
        questions,
    };
    snapshot.validate()?;
    // Missing audit rows are corruption, not an opportunity to repair facts.
    let (count, min, max): (i64, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT count(*), min(revision), max(revision) FROM work_events WHERE work_id = ?1",
            [id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(db)?;
    if row.9 < 1
        || count > MAX_WORK_EVENTS as i64
        || min != Some(row.9)
        || count != rev.get() as i64 - row.9 + 1
        || max != Some(rev.get() as i64)
    {
        return Err(WorkError::Invalid);
    }
    Ok(snapshot)
}
fn read_plan(
    conn: &Connection,
    id: WorkId,
    rev: WorkRevision,
) -> Result<WorkPlanRevision, WorkError> {
    let (plan_id, basis, author): (String, i64, String) = conn
        .query_row(
            "SELECT CASE WHEN length(CAST(plan_id AS BLOB)) = 26 THEN plan_id END, basis_revision, CASE WHEN length(CAST(author AS BLOB)) <= 32 THEN author END FROM work_plans WHERE work_id = ?1 AND revision = ?2",
            params![id.to_string(), rev.get() as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(db)?
        .ok_or(WorkError::NotFound)?;
    let mut statement = conn.prepare("SELECT CASE WHEN length(CAST(node_id AS BLOB)) = 26 THEN node_id END, position, CASE WHEN length(CAST(body AS BLOB)) <= 131072 THEN body END FROM work_plan_nodes WHERE work_id = ?1 AND plan_revision = ?2 ORDER BY position LIMIT 65").map_err(db)?;
    let rows = statement
        .query_map(params![id.to_string(), rev.get() as i64], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, usize>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(db)?;
    let mut nodes = Vec::new();
    for row in rows {
        if nodes.len() >= MAX_WORK_NODES {
            return Err(WorkError::Invalid);
        }
        let (key, position, body) = row.map_err(db)?;
        let node: WorkPlanNode = decode(&body.ok_or(WorkError::Invalid)?)?;
        if position != nodes.len() || key != node.id.to_string() {
            return Err(WorkError::Invalid);
        }
        nodes.push(node);
    }
    let plan = WorkPlanRevision {
        context: None,
        author: parse_author(&author)?,
        revision: rev,
        basis_revision: revision(basis)?,
        draft: WorkPlanDraft {
            id: WorkPlanId::parse(&plan_id).ok_or(WorkError::Invalid)?,
            nodes,
        },
    };
    plan.draft.validate()?;
    if plan.basis_revision.next()? != rev {
        return Err(WorkError::Invalid);
    }
    Ok(plan)
}
fn write_plan(tx: &Transaction<'_>, id: WorkId, plan: &WorkPlanRevision) -> Result<(), WorkError> {
    tx.execute("INSERT INTO work_plans(work_id, revision, plan_id, basis_revision, author) VALUES (?1, ?2, ?3, ?4, ?5)", params![id.to_string(), plan.revision.get() as i64, plan.draft.id.to_string(), plan.basis_revision.get() as i64, author_name(plan.author)]).map_err(db)?;
    for (position, node) in plan.draft.nodes.iter().enumerate() {
        tx.execute("INSERT INTO work_plan_nodes(work_id, plan_revision, node_id, position, body) VALUES (?1, ?2, ?3, ?4, ?5)", params![id.to_string(), plan.revision.get() as i64, node.id.to_string(), position as i64, encode(node)?]).map_err(db)?;
    }
    Ok(())
}
fn list(conn: &Connection, after: Option<WorkId>, limit: usize) -> Result<WorkReply, WorkError> {
    let mut statement = conn.prepare("SELECT CASE WHEN length(CAST(id AS BLOB)) = 26 THEN id END, revision, CASE WHEN length(CAST(status AS BLOB)) <= 16 THEN status END, CASE WHEN length(CAST(objective AS BLOB)) <= 8192 THEN objective END, schema_version, CASE WHEN length(CAST(lifecycle AS BLOB)) <= 16 THEN lifecycle END FROM works WHERE (?1 IS NULL OR id > ?1) ORDER BY id LIMIT ?2").map_err(db)?;
    let rows = statement
        .query_map(
            params![after.map(|id| id.to_string()), (limit + 1) as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, u16>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .map_err(db)?;
    let mut works = Vec::new();
    for row in rows {
        let (id, rev, state, objective, schema_version, lifecycle) = row.map_err(db)?;
        let summary = WorkSummary {
            lifecycle: parse_lifecycle(&lifecycle)?,
            schema_version,
            id: WorkId::parse(&id).ok_or(WorkError::Invalid)?,
            revision: revision(rev)?,
            status: parse_status(&state)?,
            objective: objective.ok_or(WorkError::Invalid)?,
        };
        summary.validate()?;
        works.push(summary);
    }
    let next = if works.len() > limit {
        works.pop();
        works.last().map(|w| w.id)
    } else {
        None
    };
    Ok(WorkReply::Page { works, next })
}

#[cfg(test)]
#[path = "work_document_tests.rs"]
mod tests;
