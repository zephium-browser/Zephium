//! Context admission: Rust resolves the user's selected canvas objects to
//! bodies at operation begin, budgets them, and binds a manifest to the
//! operation. The model receives exactly the admitted snapshot.
use std::time::Duration;
use zephium_core::{
    ids::ProfileId,
    resources::{ResourceCall, ResourceContent, ResourceResponse},
    work::{context::*, environment::*, port::*, runtime::*, *},
};

const READ_TIMEOUT: Duration = Duration::from_secs(8);
/// A session check never holds a request up for longer than this.
const PRESENCE_TIMEOUT: Duration = Duration::from_millis(5500);
const SITE_LOADS_TIMEOUT: Duration = Duration::from_secs(2);
#[path = "work_context_media.rs"]
mod media;

/// Answers, per host, whether the profile's own website data holds cookies
/// for that host's site. Closed facts only; the composition root installs the
/// engine's check. Unavailable checks remain unknown.
pub type WorkSessionPresence =
    dyn Fn(ProfileId, Vec<String>) -> std::sync::mpsc::Receiver<Vec<bool>> + Send + Sync;
static PRESENCE: std::sync::RwLock<Option<std::sync::Arc<WorkSessionPresence>>> =
    std::sync::RwLock::new(None);
pub fn install_session_presence(presence: std::sync::Arc<WorkSessionPresence>) {
    if let Ok(mut slot) = PRESENCE.write() {
        *slot = Some(presence);
    }
}
/// One answer per host, in order; unavailable or incomplete checks stay unknown.
pub(crate) async fn sessions_present(profile: ProfileId, hosts: Vec<String>) -> Vec<Option<bool>> {
    let count = hosts.len();
    let presence = PRESENCE.read().ok().and_then(|slot| slot.clone());
    let (Some(presence), false) = (presence, hosts.is_empty()) else {
        return vec![None; count];
    };
    let receiver = presence(profile, hosts);
    presence_answers(
        count,
        tokio::task::spawn_blocking(move || receiver.recv_timeout(PRESENCE_TIMEOUT))
            .await
            .ok()
            .and_then(Result::ok),
    )
}

fn presence_answers(count: usize, answers: Option<Vec<bool>>) -> Vec<Option<bool>> {
    match answers.filter(|answers| answers.len() == count) {
        Some(answers) => answers.into_iter().map(Some).collect(),
        None => vec![None; count],
    }
}

fn confirmed_session(present: Option<bool>) -> bool {
    present == Some(true)
}

#[cfg(all(test, feature = "work-runtime"))]
mod presence_tests {
    use super::{confirmed_session, presence_answers};
    use crate::work_sites::{Entry, PrivateBecause, RunSites};
    use zephium_core::work::sites::{WorkSiteAccessV1, WorkSiteEntryV1};
    #[test]
    fn unavailable_and_partial_presence_never_enter_yours_or_claim_a_badge() {
        for answers in [None, Some(vec![false])] {
            let facts = presence_answers(2, answers);
            assert_eq!(facts, vec![None, None]);
            let mut sites = RunSites::new(false, vec![]);
            for fact in facts {
                assert_eq!(sites.entry("example.com", fact.unwrap_or(true)), Entry::Ask);
                assert!(!confirmed_session(fact));
            }
        }
        assert_eq!(
            presence_answers(2, Some(vec![false, true])),
            vec![Some(false), Some(true)]
        );
        let mut always = RunSites::new(
            false,
            vec![WorkSiteEntryV1 {
                site: "example.com".into(),
                access: WorkSiteAccessV1::Always,
            }],
        );
        assert!(matches!(
            always.entry("example.com", true),
            Entry::Session(_)
        ));
        let mut never = RunSites::new(
            false,
            vec![WorkSiteEntryV1 {
                site: "example.com".into(),
                access: WorkSiteAccessV1::Never,
            }],
        );
        assert_eq!(
            never.entry("example.com", true),
            Entry::Private(PrivateBecause::Never)
        );
        assert!(!confirmed_session(Some(false)));
        assert!(confirmed_session(Some(true)));
    }
}

/// Finished page loads in the profile's ordinary tabs on one site: a count
/// only, so a sign-in the person finishes in a tab can wake a held page.
pub type WorkSiteLoads = dyn Fn(ProfileId, String) -> std::sync::mpsc::Receiver<u64> + Send + Sync;
static SITE_LOADS: std::sync::RwLock<Option<std::sync::Arc<WorkSiteLoads>>> =
    std::sync::RwLock::new(None);
pub fn install_site_loads(loads: std::sync::Arc<WorkSiteLoads>) {
    if let Ok(mut slot) = SITE_LOADS.write() {
        *slot = Some(loads);
    }
}
/// None when the count is unavailable.
pub async fn site_loads(profile: ProfileId, site: String) -> Option<u64> {
    let loads = SITE_LOADS.read().ok().and_then(|slot| slot.clone())?;
    let receiver = loads(profile, site);
    tokio::task::spawn_blocking(move || receiver.recv_timeout(SITE_LOADS_TIMEOUT))
        .await
        .ok()
        .and_then(Result::ok)
}

pub struct WorkContextAdmission {
    handle: crate::Handle,
}
impl WorkContextAdmission {
    pub fn new(handle: crate::Handle) -> Self {
        Self { handle }
    }

    /// Manifest only, for chrome to render before dispatch. Bodies never
    /// leave this process here.
    pub async fn preview(
        &self,
        profile: ProfileId,
        purpose: WorkContextPurpose,
        selection: &WorkContextSelectionV1,
    ) -> Result<WorkContextDisclosureV1, WorkError> {
        let admitted = self.admit(profile, purpose, selection).await;
        match admitted {
            Ok(admitted) => Ok(admitted.disclosure),
            // A private selection still has a useful manifest: chrome shows why
            // the request moves to the reviewed path.
            Err(WorkError::ReviewRequired) => self
                .admit(profile, WorkContextPurpose::Planning, selection)
                .await
                .map(|admitted| WorkContextDisclosureV1 {
                    purpose,
                    ..admitted.disclosure
                }),
            Err(error) => Err(error),
        }
    }

    pub async fn admit(
        &self,
        profile: ProfileId,
        purpose: WorkContextPurpose,
        selection: &WorkContextSelectionV1,
    ) -> Result<WorkAdmittedContext, WorkError> {
        selection.validate()?;
        let environment = self.environment(profile, selection.environment).await?;
        let mut sources = Vec::with_capacity(selection.items.len());
        for item in &selection.items {
            let element = environment
                .elements
                .iter()
                .find(|element| element.id == item.element)
                .ok_or(WorkError::NotFound)?;
            sources.push(self.resolve(profile, element).await?);
        }
        // Decisions are durable user context for reviewed work. They never ride
        // a public search query.
        let mut implicit = Vec::new();
        if !selection.blocks
            && matches!(
                purpose,
                WorkContextPurpose::Planning | WorkContextPurpose::Agent
            )
        {
            let mut decisions: Vec<_> = environment.decisions.iter().collect();
            decisions.sort_by_key(|decision| decision.element);
            for decision in decisions {
                let Some(element) = environment
                    .elements
                    .iter()
                    .find(|element| element.id == decision.element)
                else {
                    continue;
                };
                let title = match sources.iter().find(|source| source.element == element.id) {
                    Some(source) => source.title.clone(),
                    None => self.resolve(profile, element).await?.title,
                };
                implicit.push(WorkContextSource {
                    element: element.id,
                    kind: WorkContextItemKind::Decision,
                    title,
                    revision: String::new(),
                    visibility: WorkContextVisibility::Private,
                    text: decision.choice.clone(),
                });
            }
        }
        let tabs = if selection.tabs {
            self.window_tabs(profile).await?
        } else {
            Vec::new()
        };
        for source in sources.iter().chain(&implicit) {
            for text in [&source.title, &source.revision, &source.text] {
                require_model_safe_text(text)?;
            }
        }
        for tab in &tabs {
            for text in [&tab.title, &tab.host, &tab.path] {
                require_model_safe_text(text)?;
            }
        }
        WorkAdmittedContext::admit_with_tabs(
            environment.id,
            environment.revision,
            purpose,
            selection,
            sources,
            implicit,
            tabs,
        )
    }

    /// The focused window's open HTTPS tabs as title, host and path, capped.
    async fn window_tabs(&self, profile: ProfileId) -> Result<Vec<WorkContextTabV1>, WorkError> {
        let receiver = self.handle.window_tabs(profile);
        let tabs = tokio::task::spawn_blocking(move || receiver.recv_timeout(READ_TIMEOUT))
            .await
            .map_err(|_| WorkError::Unavailable)?
            .map_err(|_| WorkError::Unavailable)?;
        let mut tabs: Vec<WorkContextTabV1> = tabs
            .iter()
            .filter_map(|tab| WorkContextTabV1::from_page(&tab.title, tab.url.as_deref()?))
            .take(MAX_CONTEXT_TABS)
            .collect();
        let present =
            sessions_present(profile, tabs.iter().map(|tab| tab.host.clone()).collect()).await;
        for (tab, present) in tabs.iter_mut().zip(present) {
            tab.signed_in = confirmed_session(present);
        }
        Ok(tabs)
    }

    pub(crate) async fn environment(
        &self,
        profile: ProfileId,
        id: WorkEnvironmentId,
    ) -> Result<WorkEnvironmentSnapshot, WorkError> {
        let request = self.handle.submit_work_document(
            WorkRequest::Environment {
                call: WorkEnvironmentCall::Read { id },
                space_available: false,
                browser_available: false,
                note_available: false,
            },
            Some(profile),
        )?;
        let projection = tokio::time::timeout(READ_TIMEOUT, request)
            .await
            .map_err(|_| WorkError::Unavailable)??;
        match projection.reply {
            WorkReply::Environment(WorkEnvironmentReply::Snapshot { snapshot })
                if projection.profile == profile && snapshot.profile == profile =>
            {
                Ok(*snapshot)
            }
            _ => Err(WorkError::NotFound),
        }
    }

    async fn resolve(
        &self,
        profile: ProfileId,
        element: &WorkEnvironmentElement,
    ) -> Result<WorkContextSource, WorkError> {
        let id = element.id;
        match &element.reference {
            WorkEnvironmentReference::Resource { resource } => {
                self.resource(profile, id, resource.to_string()).await
            }
            // A folder reaches the agent as a grant, never as context text.
            WorkEnvironmentReference::Folder { .. } => Err(WorkError::Invalid),
            // A placed link is public by construction; the agent may read it.
            WorkEnvironmentReference::Link { url, title } => Ok(WorkContextSource {
                element: id,
                kind: WorkContextItemKind::Tab,
                title: title.clone(),
                revision: url_revision(url),
                visibility: WorkContextVisibility::Public,
                text: format!("{title}\n{url}"),
            }),
            WorkEnvironmentReference::Browser { tab } => {
                let tab = *tab;
                let handle = self.handle.clone();
                let receiver = handle.tab_metadata(profile, vec![tab]);
                let mut tabs =
                    tokio::task::spawn_blocking(move || receiver.recv_timeout(READ_TIMEOUT))
                        .await
                        .map_err(|_| WorkError::Unavailable)?
                        .map_err(|_| WorkError::Unavailable)?;
                let tab = tabs.pop().ok_or(WorkError::NotFound)?;
                let url = tab.url.unwrap_or_default();
                Ok(WorkContextSource {
                    element: id,
                    kind: WorkContextItemKind::Tab,
                    title: tab.title.clone(),
                    revision: url_revision(&url),
                    visibility: WorkContextVisibility::Private,
                    text: format!("{}\n{url}", tab.title),
                })
            }
            WorkEnvironmentReference::Objective { objective } => {
                let projection = self.projection(profile, *objective).await?;
                Ok(WorkContextSource {
                    element: id,
                    kind: WorkContextItemKind::Objective,
                    title: projection.work.objective.clone(),
                    revision: projection.work.revision.get().to_string(),
                    visibility: WorkContextVisibility::Private,
                    text: projection.work.objective,
                })
            }
            WorkEnvironmentReference::Artifact {
                objective,
                execution,
                artifact,
            } => {
                let (artifact, visibility) = self
                    .artifact(profile, *objective, *execution, *artifact)
                    .await?;
                Ok(WorkContextSource {
                    element: id,
                    kind: WorkContextItemKind::Artifact,
                    title: artifact.title.clone(),
                    revision: artifact.id.to_string(),
                    visibility,
                    text: artifact.data.plain_text(),
                })
            }
            WorkEnvironmentReference::Subject {
                objective,
                execution,
                artifact,
                index,
            } => {
                let (artifact, visibility) = self
                    .artifact(profile, *objective, *execution, *artifact)
                    .await?;
                let (title, text) =
                    subject_body(&artifact.data, *index).ok_or(WorkError::NotFound)?;
                Ok(WorkContextSource {
                    element: id,
                    kind: WorkContextItemKind::Subject,
                    title,
                    revision: artifact.id.to_string(),
                    visibility,
                    text,
                })
            }
            WorkEnvironmentReference::Finding {
                objective,
                execution,
                artifact,
                index,
            } => {
                let (artifact, visibility) = self
                    .artifact(profile, *objective, *execution, *artifact)
                    .await?;
                let (title, text) =
                    finding_body(&artifact.data, *index).ok_or(WorkError::NotFound)?;
                Ok(WorkContextSource {
                    element: id,
                    kind: WorkContextItemKind::Finding,
                    title,
                    revision: artifact.id.to_string(),
                    visibility,
                    text,
                })
            }
            WorkEnvironmentReference::Source {
                objective,
                execution,
                artifact,
                index,
            } => {
                let (artifact, visibility) = self
                    .artifact(profile, *objective, *execution, *artifact)
                    .await?;
                let (title, text) =
                    source_body(&artifact.data, *index).ok_or(WorkError::NotFound)?;
                Ok(WorkContextSource {
                    element: id,
                    kind: WorkContextItemKind::Source,
                    title,
                    revision: artifact.id.to_string(),
                    visibility,
                    text,
                })
            }
        }
    }

    async fn note(
        &self,
        profile: ProfileId,
        element: WorkElementId,
        id: &str,
    ) -> Result<Option<WorkContextSource>, WorkError> {
        use zephium_core::notes::{NoteCall, NoteResponse};
        let receiver = self
            .handle
            .note_call(profile, NoteCall::Get { id: id.to_owned() });
        let reply = tokio::task::spawn_blocking(move || receiver.recv_timeout(READ_TIMEOUT))
            .await
            .map_err(|_| WorkError::Unavailable)?
            .map_err(|_| WorkError::Unavailable)?;
        let NoteResponse::Record { record } = reply.response else {
            return Ok(None);
        };
        if reply.profile.as_deref() != Some(&profile.to_string()) {
            return Err(WorkError::ProfileUnavailable);
        }
        if record.summary.trashed {
            return Err(WorkError::NotFound);
        }
        Ok(Some(WorkContextSource {
            element,
            kind: WorkContextItemKind::Note,
            title: record.summary.title,
            revision: record.summary.revision,
            visibility: WorkContextVisibility::Private,
            text: record.markdown,
        }))
    }

    async fn resource(
        &self,
        profile: ProfileId,
        element: WorkElementId,
        resource: String,
    ) -> Result<WorkContextSource, WorkError> {
        // Notes are Markdown files; any other resource lives in the store.
        if let Some(note) = self.note(profile, element, &resource).await? {
            return Ok(note);
        }
        let receiver = self
            .handle
            .resource_call(profile, ResourceCall::Get { id: resource });
        let reply = tokio::task::spawn_blocking(move || receiver.recv_timeout(READ_TIMEOUT))
            .await
            .map_err(|_| WorkError::Unavailable)?
            .map_err(|_| WorkError::Unavailable)?;
        if reply.profile.as_deref() != Some(&profile.to_string()) {
            return Err(WorkError::ProfileUnavailable);
        }
        let ResourceResponse::Record { record } = reply.response else {
            return Err(WorkError::NotFound);
        };
        if record.trashed {
            return Err(WorkError::NotFound);
        }
        let (kind, text) = match &record.draft.content {
            ResourceContent::Note { document } => {
                (WorkContextItemKind::Note, document.plain_text())
            }
            ResourceContent::Task {
                description,
                completed,
                ..
            } => (
                WorkContextItemKind::Task,
                format!(
                    "{description}\n{}",
                    if *completed { "completed" } else { "open" }
                ),
            ),
            ResourceContent::Object { .. } => {
                (WorkContextItemKind::Object, block_context_body(&record))
            }
            ResourceContent::Media { asset } => {
                let text = if asset.kind == zephium_core::resources::MediaKind::Image {
                    media::metadata(asset)
                } else {
                    let request = self.handle.submit_work_document(
                        WorkRequest::ReadMediaContext {
                            resource: record.id.clone(),
                            revision: record.revision.clone(),
                        },
                        Some(profile),
                    )?;
                    let response = tokio::time::timeout(READ_TIMEOUT, request)
                        .await
                        .map_err(|_| WorkError::Unavailable)??;
                    if response.profile != profile {
                        return Err(WorkError::ProfileUnavailable);
                    }
                    let WorkReply::MediaContext(bytes) = response.reply else {
                        return Err(WorkError::Invalid);
                    };
                    let asset = asset.clone();
                    media::disclose(asset, bytes.0).await
                };
                (WorkContextItemKind::Object, text)
            }
        };
        Ok(WorkContextSource {
            element,
            kind,
            title: record.draft.title.clone(),
            revision: record.revision,
            visibility: WorkContextVisibility::Private,
            text,
        })
    }

    async fn projection(
        &self,
        profile: ProfileId,
        work: WorkId,
    ) -> Result<WorkRuntimeProjection, WorkError> {
        let response =
            tokio::time::timeout(READ_TIMEOUT, self.handle.work_projection(profile, work)?)
                .await
                .map_err(|_| WorkError::Unavailable)??;
        match response.reply {
            WorkReply::Runtime(state)
                if response.profile == profile && state.work.profile == profile =>
            {
                Ok(*state)
            }
            _ => Err(WorkError::NotFound),
        }
    }

    /// Public research results are public data; every reviewed-plan output may
    /// have been shaped by private context and stays private.
    async fn artifact(
        &self,
        profile: ProfileId,
        work: WorkId,
        execution: WorkExecutionId,
        artifact: WorkArtifactId,
    ) -> Result<(artifact::WorkArtifactV1, WorkContextVisibility), WorkError> {
        let projection = self.projection(profile, work).await?;
        let fact = projection
            .executions
            .into_iter()
            .find(|fact| fact.id == execution)
            .ok_or(WorkError::NotFound)?;
        let visibility = if matches!(
            fact.authorization,
            WorkExecutionAuthorization::UserDirectedPublicRead
                | WorkExecutionAuthorization::UserDirectedAgent
        ) && fact
            .spec
            .context
            .as_ref()
            .is_none_or(|context| !context.requires_review())
        {
            WorkContextVisibility::Public
        } else {
            WorkContextVisibility::Private
        };
        let mut found = fact
            .artifacts
            .into_iter()
            .find(|candidate| candidate.id == artifact)
            .ok_or(WorkError::NotFound)?;
        if let Some(edited) = fact
            .user_artifacts
            .iter()
            .find(|state| state.artifact == artifact)
            .and_then(|state| state.edited_data.clone())
        {
            found.data = edited;
        }
        Ok((found, visibility))
    }
}

/// Structured identity is never inferred from lossy text. Large blocks are read
/// in bounded pages by the context-bound tools before any edit is accepted.
fn block_context_body(record: &zephium_core::resources::ResourceRecord) -> String {
    let ResourceContent::Object { object } = &record.draft.content else {
        return String::new();
    };
    let mut visible = object.data.clone();
    visible.clear_record_media();
    let data = serde_json::to_value(&visible).unwrap_or_default();
    if let Some(rows) = data
        .get("rows")
        .or_else(|| data.get("items"))
        .or_else(|| data.get("steps"))
        .and_then(serde_json::Value::as_array)
    {
        return serde_json::json!({"resource":record.id,"revision":record.revision,"kind":data["kind"],"count":rows.len(),"read_required":true}).to_string();
    }
    let body = serde_json::json!({"resource":record.id,"revision":record.revision,"data":data})
        .to_string();
    if body.len() <= MAX_CONTEXT_ITEM_BYTES {
        body
    } else {
        serde_json::json!({"resource":record.id,"revision":record.revision,"kind":data["kind"],"read_required":true}).to_string()
    }
}

/// Context consent is not authority to disclose recognizable credentials.
/// Check materialized context before its final byte limit; return no matched content.
pub(crate) fn require_model_safe_text(text: &str) -> Result<(), WorkError> {
    if zephium_agentic::contains_recognized_secret(text) {
        Err(WorkError::Unavailable)
    } else {
        Ok(())
    }
}
#[cfg(test)]
mod credential_disclosure_tests {
    use super::*;
    #[test]
    fn selected_context_withholds_credentials_before_clipping() {
        for secret in [
            "ghp_inertFixtureToken",
            "Bearer inertFixtureValue",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
        ] {
            let text = format!(
                "{}\n{secret}",
                "Ordinary note. ".repeat(MAX_CONTEXT_ITEM_BYTES)
            );
            assert_eq!(require_model_safe_text(&text), Err(WorkError::Unavailable));
        }
        assert!(require_model_safe_text("Confidential client budget:71329 EUR").is_ok());
    }
    #[test]
    fn structured_block_replies_and_metadata_use_the_same_boundary() {
        for field in ["title", "column", "value"] {
            let mut data = serde_json::json!({});
            data[field] = serde_json::json!("sk-inertFixtureToken");
            let body = data.to_string();
            assert_eq!(require_model_safe_text(&body), Err(WorkError::Unavailable));
        }
        let body =
            serde_json::json!({"title":"My calendar","rows":[{"title":"Lunch","done":false}]})
                .to_string();
        assert!(require_model_safe_text(&body).is_ok());
    }
}
