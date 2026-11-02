//! Versioned semantic Work boundary. Layout, live handles, credentials and
//! model transcripts do not participate in these contracts.
use serde::{Deserialize, Serialize};
use specta::Type;
use zephium_core::work::{runtime::*, *};

#[path = "work_human.rs"]
mod human;
pub use human::*;

#[path = "work_sites.rs"]
mod sites;
pub use sites::*;
#[path = "work_decision.rs"]
mod decision;
pub use decision::*;
#[path = "work_personal.rs"]
mod personal;
pub use personal::*;
#[path = "work_connections.rs"]
mod connections;
pub use connections::*;

pub type WorkProjectionV1 = WorkRuntimeProjection;

/// Invalidation only. Consumers read current facts; delivery grants no authority
/// and is neither an ordered event log nor proof that a command succeeded.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkChangedV1 {
    pub profile: String,
    pub work: WorkId,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkEnvironmentChangedV1 {
    pub profile: String,
    pub environment: WorkEnvironmentId,
}

/// Durable document operations share a bounded, profile-checked transport.
/// Provider generation and worker admission have separate lifetimes.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkCallV1 {
    Environment {
        version: u16,
        request: environment::WorkEnvironmentCall,
    },
    Query {
        request: WorkQueryV1,
    },
    Author {
        command: WorkAuthoringCommandV1,
    },
    Execute {
        command: WorkCommandV1,
    },
}
impl WorkCallV1 {
    pub fn into_request(self) -> Result<port::WorkRequest, WorkError> {
        let request = match self {
            Self::Environment {
                version: 1,
                request,
            } => port::WorkRequest::Environment {
                call: request,
                space_available: false,
                browser_available: false,
                note_available: false,
            },
            Self::Environment { .. } => return Err(WorkError::Invalid),
            Self::Query { request } => request.into_request()?,
            Self::Author { command } => command.into_request()?,
            Self::Execute { command } => command.into_request()?,
        };
        request.validate()?;
        Ok(request)
    }
}
/// One non-replayable provider operation. After a lost result, refresh Work;
/// never automatically repeat generation or interpret missing usage as zero.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkPlanRequestV1 {
    pub version: u16,
    pub work: WorkId,
    pub expected_revision: WorkRevision,
    /// Selected canvas objects to admit as planning context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<context::WorkContextSelectionV1>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkStartRequestV1 {
    pub version: u16,
    pub work: WorkId,
    pub expected_revision: WorkRevision,
    pub execution: WorkExecutionId,
}

/// On-demand operations have an application lifetime independent of a view.
/// Their correlation IDs do not constitute durable commands or worker handles.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkOperationV1 {
    ReadPublic {
        command: WorkCommandV1,
        /// Selected public canvas objects to accompany the query.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<context::WorkContextSelectionV1>,
    },
    /// The routine agent loop on the objective; the command carries the grant.
    Run {
        command: WorkCommandV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context: Option<context::WorkContextSelectionV1>,
        /// How a request without accounts treats the person's sessions.
        #[serde(default, skip_serializing_if = "WorkSignedInV1::is_offer")]
        signed_in: WorkSignedInV1,
    },
    Plan {
        request: WorkPlanRequestV1,
    },
    PreparePlan {
        request: WorkPlanRequestV1,
    },
    Prepare {
        request: WorkApprovalRequestV1,
    },
    /// Approval draft for one signed-in page chosen from an attached tab.
    PrepareAccount {
        request: WorkAccountApprovalRequestV1,
    },
    Start {
        request: WorkStartRequestV1,
    },
}
impl WorkOperationV1 {
    pub fn work(&self) -> WorkId {
        match self {
            Self::ReadPublic { command, .. } | Self::Run { command, .. } => command.work,
            Self::Plan { request } | Self::PreparePlan { request } => request.work,
            Self::Prepare { request } => request.work,
            Self::PrepareAccount { request } => request.work,
            Self::Start { request } => request.work,
        }
    }
}

/// The user names the tab, the effect, and (for updates) exact values. Rust
/// resolves the page, mints the account identity, and returns the draft the
/// user approves; approval is the account attestation.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkAccountApprovalRequestV1 {
    pub version: u16,
    pub work: WorkId,
    pub expected_revision: WorkRevision,
    pub environment: WorkEnvironmentId,
    /// The attached browser element on the canvas.
    pub element: WorkElementId,
    pub effect: WorkAccountEffectV1,
    /// `origin` drafts a grant to read pages on the tab's origin for the
    /// next request, instead of one exact page.
    #[serde(default, skip_serializing_if = "WorkAccountModeV1::is_page")]
    pub mode: WorkAccountModeV1,
    /// Signed-in pages an origin grant may open (1–12; 12 when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u8>,
}
/// A run sent without accounts may stop at a drafted origin grant instead of
/// starting: the reply is then `account_grant_draft`, and the person's answer
/// sends the request again with the grant or declined. Nothing is granted
/// without that answer.
#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkSignedInV1 {
    /// Draft a grant when the request names the site of an attached tab the
    /// profile holds a session for.
    #[default]
    Offer,
    /// The person said not now: this request reads anonymously.
    Declined,
    /// The person signed in to this origin and asked to read it as themselves.
    Origin { origin: String },
}
impl WorkSignedInV1 {
    pub fn is_offer(&self) -> bool {
        *self == Self::Offer
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkAccountModeV1 {
    #[default]
    Page,
    Origin,
}
impl WorkAccountModeV1 {
    pub fn is_page(&self) -> bool {
        *self == Self::Page
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkAccountEffectV1 {
    Read,
    Update { update: WorkFieldUpdateV1 },
}

/// The manifest chrome renders before dispatch, computed by the same
/// admission that later binds it to the operation.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkContextPreviewV1 {
    Admitted {
        disclosure: context::WorkContextDisclosureV1,
    },
    Refused {
        error: WorkFailureV1,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkOperationResponseV1 {
    pub version: u16,
    pub profile: String,
    pub operation: WorkCommandId,
    pub state: WorkOperationStateV1,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkOperationStateV1 {
    /// No retained observation. Reconcile durable Work; do not replay a model
    /// call or reconstruct an execution from this state after a restart.
    Unknown,
    Pending {
        work: WorkId,
    },
    Planned {
        response: WorkPlanningResponseV1,
    },
    Settled {
        response: WorkResponseV1,
    },
    Refused {
        error: WorkFailureV1,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkApprovalRequestV1 {
    pub version: u16,
    pub work: WorkId,
    pub expected_revision: WorkRevision,
    pub limits: WorkExecutionLimits,
    pub scope: WorkBrowseScope,
    pub primary: Option<WorkPlanNodeId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkPlanningResponseV1 {
    pub version: u16,
    pub profile: String,
    pub work: WorkId,
    pub basis_revision: WorkRevision,
    pub usage: Option<WorkPlanningUsageV1>,
    pub outcome: WorkPlanningOutcomeV1,
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkPlanningUsageV1 {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_ceiling_micro_usd: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkPlanningOutcomeV1 {
    Settled { response: Box<WorkResponseV1> },
    Refused { reason: WorkPlanningFailureV1 },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkPlanningFailureV1 {
    Invalid,
    Capacity,
    Unavailable,
    Cancelled,
    Timeout,
    Stale,
    NeedsInput,
    Privacy,
    ProviderOutcomeUnknown,
    ProviderRefused,
    Store { error: WorkFailureV1 },
}
pub use zephium_core::work::artifact::{
    WorkArtifactDataV1, WorkArtifactV1, WorkEvidenceLink, WorkEvidencePreviewV1,
};
pub use zephium_core::work::authoring::{WorkAuthoringIntent, WorkAuthoringReceipt, WorkUserEdit};

/// The command ID is profile-scoped and survives lost replies and restart.
/// New Work, question and plan identities are minted by Rust exactly once.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkAuthoringCommandV1 {
    pub version: u16,
    pub command: WorkCommandId,
    pub intent: WorkAuthoringIntent,
}
impl WorkAuthoringCommandV1 {
    pub fn into_request(self) -> Result<port::WorkRequest, WorkError> {
        if self.version != 1 {
            return Err(WorkError::Invalid);
        }
        let request = port::WorkRequest::AuthoringCommand {
            command: self.command,
            intent: self.intent,
        };
        request.validate()?;
        Ok(request)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkQueryV1 {
    pub version: u16,
    pub query: WorkQueryKindV1,
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkQueryKindV1 {
    Projection {
        work: WorkId,
    },
    List {
        after: Option<WorkId>,
        limit: u16,
    },
    PlanHistory {
        work: WorkId,
    },
    Plan {
        work: WorkId,
        revision: WorkRevision,
    },
    Evidence {
        work: WorkId,
        link: WorkEvidenceLink,
    },
}
impl WorkQueryV1 {
    pub fn into_request(self) -> Result<port::WorkRequest, WorkError> {
        if self.version != 1 {
            return Err(WorkError::Invalid);
        }
        let request = match self.query {
            WorkQueryKindV1::Projection { work } => port::WorkRequest::RuntimeRead { id: work },
            WorkQueryKindV1::List { after, limit } => port::WorkRequest::List {
                after,
                limit: usize::from(limit),
            },
            WorkQueryKindV1::PlanHistory { work } => port::WorkRequest::ListPlans { id: work },
            WorkQueryKindV1::Plan { work, revision } => {
                port::WorkRequest::ReadPlan { id: work, revision }
            }
            WorkQueryKindV1::Evidence { work, link } => {
                port::WorkRequest::ReadEvidence { id: work, link }
            }
        };
        request.validate()?;
        Ok(request)
    }
}

/// Closed product reply grammar: the host-only RuntimeStarted reply cannot
/// cross this conversion, so a read or JSON round trip cannot mint an attempt.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkResponseV1 {
    pub version: u16,
    pub profile: String,
    pub reply: WorkReplyV1,
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkReplyV1 {
    PublicReadAdmitted {
        projection: Box<WorkProjectionV1>,
        receipt: WorkCommandReceipt,
        replayed: bool,
    },
    Environment {
        reply: environment::WorkEnvironmentReply,
    },
    ApprovalDraft {
        work: WorkId,
        expected_revision: WorkRevision,
        spec: WorkExecutionSpec,
    },
    /// An origin grant for the next request: the person approves it, and
    /// `Run` carries it in the agent grant's `accounts`.
    AccountGrantDraft {
        work: WorkId,
        grant: WorkAccountGrantV1,
    },
    Projection {
        projection: Box<WorkProjectionV1>,
    },
    AuthoringApplied {
        receipt: WorkAuthoringReceipt,
    },
    ExecutionApplied {
        projection: Box<WorkProjectionV1>,
        receipt: WorkCommandReceipt,
    },
    Evidence {
        evidence: Box<WorkEvidencePreviewV1>,
    },
    Snapshot {
        snapshot: Box<WorkSnapshot>,
    },
    Plan {
        plan: WorkPlanRevision,
    },
    PlanHistory {
        revisions: Vec<WorkRevision>,
    },
    Page {
        works: Vec<port::WorkSummary>,
        next: Option<WorkId>,
    },
    Error {
        error: WorkFailureV1,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkFailureV1 {
    Invalid,
    Capacity,
    Conflict,
    NotFound,
    ProfileUnavailable,
    Unavailable,
    Shutdown,
    OutcomeUnknown,
    ReviewRequired,
}
impl From<WorkError> for WorkFailureV1 {
    fn from(value: WorkError) -> Self {
        match value {
            WorkError::Invalid => Self::Invalid,
            WorkError::Capacity => Self::Capacity,
            WorkError::Conflict => Self::Conflict,
            WorkError::NotFound => Self::NotFound,
            WorkError::ProfileUnavailable => Self::ProfileUnavailable,
            WorkError::Unavailable => Self::Unavailable,
            WorkError::Shutdown => Self::Shutdown,
            WorkError::OutcomeUnknown => Self::OutcomeUnknown,
            WorkError::ReviewRequired => Self::ReviewRequired,
        }
    }
}
impl WorkResponseV1 {
    pub fn from_result(
        profile: zephium_core::ids::ProfileId,
        result: Result<port::WorkReply, WorkError>,
    ) -> Self {
        let reply = match result {
            Ok(port::WorkReply::Environment(reply)) => WorkReplyV1::Environment { reply },
            Ok(port::WorkReply::Runtime(projection)) => WorkReplyV1::Projection { projection },
            Ok(port::WorkReply::AuthoringCommand(receipt)) => {
                WorkReplyV1::AuthoringApplied { receipt }
            }
            Ok(port::WorkReply::PublicReadAdmitted {
                projection,
                receipt,
                replayed,
            }) => WorkReplyV1::PublicReadAdmitted {
                projection,
                receipt,
                replayed,
            },
            Ok(
                port::WorkReply::RuntimeCommand {
                    projection,
                    receipt,
                }
                | port::WorkReply::AgentAdmitted {
                    projection,
                    receipt,
                    ..
                },
            ) => WorkReplyV1::ExecutionApplied {
                projection,
                receipt,
            },
            Ok(port::WorkReply::Evidence(evidence)) => WorkReplyV1::Evidence {
                evidence: Box::new(evidence),
            },
            Ok(port::WorkReply::Snapshot(snapshot)) => WorkReplyV1::Snapshot { snapshot },
            Ok(port::WorkReply::Plan(plan)) => WorkReplyV1::Plan { plan },
            Ok(port::WorkReply::PlanHistory { revisions }) => {
                WorkReplyV1::PlanHistory { revisions }
            }
            Ok(port::WorkReply::Page { works, next }) => WorkReplyV1::Page { works, next },
            Ok(
                port::WorkReply::RuntimeStarted { .. }
                | port::WorkReply::Deleted { .. }
                | port::WorkReply::SiteAccess(_)
                | port::WorkReply::ScreenshotAccess(_)
                | port::WorkReply::Personal(_)
                | port::WorkReply::MediaContext(_)
                | port::WorkReply::RecordMedia(_)
                | port::WorkReply::RecordMediaStored(_),
            ) => WorkReplyV1::Error {
                error: WorkFailureV1::Invalid,
            },
            Err(error) => WorkReplyV1::Error {
                error: error.into(),
            },
        };
        Self {
            version: 1,
            profile: profile.to_string(),
            reply,
        }
    }
}

/// Retry the same command and expected revision after a lost reply. The Store
/// returns its original receipt plus current facts. Reusing an ID with changed
/// operands is a conflict. Receipt eviction is never automatic.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkCommandV1 {
    pub version: u16,
    pub work: WorkId,
    pub expected_revision: WorkRevision,
    pub command: WorkCommandId,
    pub intent: WorkRuntimeIntent,
}
impl WorkCommandV1 {
    pub fn into_request(self) -> Result<port::WorkRequest, WorkError> {
        if self.version != 1 || match &self.intent {
            WorkRuntimeIntent::BeginAgent { grant, .. } => grant.no_usage_charge,
            WorkRuntimeIntent::Approve { spec } => spec.nodes.iter().any(|node| matches!(&node.capability,
                zephium_core::work::runtime::WorkCapability::Agent { grant } if grant.no_usage_charge)),
            _ => false,
        } {
            return Err(WorkError::Invalid);
        }
        Ok(port::WorkRequest::RuntimeCommand {
            id: self.work,
            expected: self.expected_revision,
            command: self.command,
            intent: self.intent,
        })
    }
}

/// A stale, missed or reordered signal never changes durable execution state.
/// Render only for the matching owner, Work and durable revision.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkSignalV1 {
    pub version: u16,
    pub owner: WorkRuntimeSessionId,
    pub profile: String,
    pub work: WorkId,
    pub basis_revision: WorkRevision,
    pub execution: WorkExecutionId,
    pub node: WorkPlanNodeId,
    pub attempt: WorkAttemptId,
    pub activity: WorkActivityV1,
}
/// A page one browser step opened, with its newest bounded frame for the
/// canvas. Frames are transient and person-facing only.
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkPageV1 {
    pub execution: WorkExecutionId,
    pub attempt: WorkAttemptId,
    pub step: WorkStepId,
    pub url: String,
    pub live: bool,
    pub frame: Option<WorkPageFrameV1>,
    /// The part that reads it; none when the lead reads it itself.
    #[specta(optional)]
    pub part: Option<WorkPartId>,
    /// The page's public title once it was read; never a signed-in page's.
    #[specta(optional)]
    pub title: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkPageFrameV1 {
    pub generation: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(deny_unknown_fields)]
pub struct WorkActivityResponseV1 {
    pub version: u16,
    pub profile: String,
    pub work: WorkId,
    pub signals: Vec<WorkSignalV1>,
    #[serde(default)]
    pub pages: Vec<WorkPageV1>,
    pub error: Option<WorkFailureV1>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Type)]
#[serde(rename_all = "snake_case")]
pub enum WorkActivityV1 {
    Planning,
    Delegating,
    Searching,
    Reading,
    Interacting,
    Verifying,
    Recovering,
    /// The hosted page waits while its window is hidden or minimized.
    Paused,
    Comparing,
    ProducingArtifact,
    WaitingForApproval,
    WaitingForHuman,
    Cancelling,
    Finishing,
}

impl From<planning::WorkPlanningError> for WorkPlanningFailureV1 {
    fn from(error: planning::WorkPlanningError) -> Self {
        use planning::WorkPlanningError;
        match error {
            WorkPlanningError::Invalid => WorkPlanningFailureV1::Invalid,
            WorkPlanningError::Capacity => WorkPlanningFailureV1::Capacity,
            WorkPlanningError::Unavailable => WorkPlanningFailureV1::Unavailable,
            WorkPlanningError::Cancelled => WorkPlanningFailureV1::Cancelled,
            WorkPlanningError::Timeout => WorkPlanningFailureV1::Timeout,
            WorkPlanningError::Stale => WorkPlanningFailureV1::Stale,
            WorkPlanningError::NeedsInput => WorkPlanningFailureV1::NeedsInput,
            WorkPlanningError::Privacy => WorkPlanningFailureV1::Privacy,
            WorkPlanningError::ProviderOutcomeUnknown => {
                WorkPlanningFailureV1::ProviderOutcomeUnknown
            }
            WorkPlanningError::ProviderStalled(_) => WorkPlanningFailureV1::Timeout,
            WorkPlanningError::ProviderRefused(_) => WorkPlanningFailureV1::ProviderRefused,
            WorkPlanningError::Store(error) => WorkPlanningFailureV1::Store {
                error: error.into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_media_context_cannot_be_projected_over_ipc() {
        let response = WorkResponseV1::from_result(
            1.into(),
            Ok(port::WorkReply::MediaContext(port::WorkMediaContext(
                b"private media body".to_vec(),
            ))),
        );
        assert!(matches!(
            response.reply,
            WorkReplyV1::Error {
                error: WorkFailureV1::Invalid
            }
        ));
    }
    #[test]
    fn fixtures_match_validated_rust_contracts() {
        let state: WorkProjectionV1 =
            serde_json::from_str(include_str!("../fixtures/work-approved-v1.json")).unwrap();
        state.work.validate().unwrap();
        state.executions[0]
            .validate(state.work.plan.as_ref().unwrap(), state.work.revision)
            .unwrap();
        let command: WorkCommandV1 =
            serde_json::from_str(include_str!("../fixtures/work-approve-command-v1.json")).unwrap();
        command.into_request().unwrap().validate().unwrap();
    }

    #[test]
    fn subject_and_finding_kinds_round_trip_with_claim_level_evidence() {
        let state: WorkProjectionV1 =
            serde_json::from_str(include_str!("../fixtures/work-research-v2-kinds-v1.json"))
                .unwrap();
        state.work.validate().unwrap();
        let execution = &state.executions[0];
        execution
            .validate(state.work.plan.as_ref().unwrap(), state.work.revision)
            .unwrap();
        let kinds: Vec<&str> = execution
            .artifacts
            .iter()
            .map(|artifact| match &artifact.data {
                WorkArtifactDataV1::ComparisonMatrix { .. } => "comparison_matrix",
                WorkArtifactDataV1::Findings { .. } => "findings",
                _ => "other",
            })
            .collect();
        assert!(kinds.contains(&"comparison_matrix"));
        let encoded = serde_json::to_string(&state).unwrap();
        let decoded: WorkProjectionV1 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            decoded.executions[0].artifacts[0].data,
            execution.artifacts[0].data
        );
    }

    #[test]
    fn live_work_fixtures_preserve_terminal_facts_and_linked_evidence() {
        let evidence: Vec<WorkEvidencePreviewV1> =
            serde_json::from_str(include_str!("../fixtures/work-research-evidence-v1.json"))
                .unwrap();
        for (json, status, artifacts) in [
            (
                include_str!("../fixtures/work-research-v1.json"),
                WorkExecutionStatus::NeedsReview,
                2,
            ),
            (
                include_str!("../fixtures/work-cancelled-v1.json"),
                WorkExecutionStatus::Cancelled,
                0,
            ),
            (
                include_str!("../fixtures/work-failed-v1.json"),
                WorkExecutionStatus::Failed,
                0,
            ),
        ] {
            let state: WorkProjectionV1 = serde_json::from_str(json).unwrap();
            assert_eq!(state.version, 1);
            state.work.validate().unwrap();
            assert_eq!(state.executions.len(), 1);
            let execution = &state.executions[0];
            execution
                .validate(state.work.plan.as_ref().unwrap(), state.work.revision)
                .unwrap();
            assert_eq!(execution.status, status);
            assert_eq!(execution.attempts.len(), 2);
            assert!(execution.attempts.iter().all(|a| a.usage.is_some()));
            assert_eq!(execution.artifacts.len(), artifacts);
            for artifact in &execution.artifacts {
                assert_eq!(artifact.review, WorkOutputReview::SourceMappedNeedsReview);
                assert!(!artifact.evidence.is_empty());
                for link in &artifact.evidence {
                    assert_eq!(evidence.iter().filter(|e| e.link == *link).count(), 1);
                }
            }
        }
        for preview in evidence {
            assert_eq!(preview.version, 1);
            assert!(!preview.truncated);
            assert_eq!(preview.source_bytes, preview.text.len().to_string());
        }
    }

    #[test]
    fn wire_cannot_submit_worker_settlement_or_authority() {
        let id = WorkId::from(1).to_string();
        let input = serde_json::json!({ "version": 1, "work": id, "expected_revision": "2", "command": id,
            "intent": { "kind": "settle", "execution": id, "status": "succeeded" }});
        assert!(serde_json::from_value::<WorkCommandV1>(input).is_err());
        // The generated contract must accept the actual structured projection;
        // there is no opaque JSON escape hatch at the rendering boundary.
        let _types = specta::Types::default()
            .register::<WorkProjectionV1>()
            .register::<WorkCommandV1>()
            .register::<WorkEvidencePreviewV1>()
            .register::<WorkSignalV1>();
    }
    #[test]
    fn wire_cannot_mint_a_trusted_zero_charge_grant() {
        let id = WorkId::from(1).to_string();
        let limits = serde_json::json!({ "model_tokens": 1000, "cost_micro_usd": 1000,
            "operations": 16, "timeout_seconds": 60, "max_workers": 1 });
        let grant = serde_json::json!({ "provider": "browser", "model": "duckduckgo",
            "max_turns": 8, "max_steps": 24, "browse_hops": 4,
            "lead": { "provider": "compatible", "wire": "chat_completions", "model": "local-fixture" } });
        let mut input = serde_json::json!({ "version": 1, "work": id, "expected_revision": "1", "command": id,
            "intent": { "kind": "begin_agent", "grant": grant, "limits": limits } });
        let legacy: WorkCommandV1 = serde_json::from_value(input.clone()).unwrap();
        let WorkRuntimeIntent::BeginAgent { grant, .. } = &legacy.intent else {
            panic!()
        };
        assert!(!grant.no_usage_charge);
        assert!(legacy.into_request().is_ok());
        input["intent"]["grant"]["no_usage_charge"] = true.into();
        let forged: WorkCommandV1 = serde_json::from_value(input.clone()).unwrap();
        assert!(matches!(forged.into_request(), Err(WorkError::Invalid)));
        let grant = input["intent"]["grant"].clone();
        input["intent"] = serde_json::json!({ "kind": "approve", "spec": {
            "plan_revision": "1", "limits": limits, "nodes": [{ "node": id,
                "capability": { "kind": "agent", "grant": grant }, "limits": limits }] } });
        let forged: WorkCommandV1 = serde_json::from_value(input).unwrap();
        assert!(matches!(forged.into_request(), Err(WorkError::Invalid)));
    }
}
