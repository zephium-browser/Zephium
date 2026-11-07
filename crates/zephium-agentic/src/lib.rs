//! Bounded domain contracts for Zephium's browser-execution proof.
//!
//! The default contract deliberately contains no browser engine, native
//! object, page script, selector, navigable page URL, profile path, provider
//! secret, or model adapter. It defines the closed vocabulary shared by the
//! non-shipping probe and the eventual product ports. Canonical origins are
//! retained only as native-attested provenance with redacted diagnostics.
//! Diagnostic fixtures and their fixed recipe enum are behind `probe-harness`
//! and cannot be compiled in an optimized build.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::dbg_macro, clippy::print_stderr, clippy::print_stdout)]
#![cfg_attr(
    not(test),
    deny(clippy::panic, clippy::unreachable, clippy::unwrap_used)
)]

#[cfg(all(feature = "probe-harness", not(debug_assertions)))]
compile_error!("the agentic probe harness is forbidden in optimized builds");

mod agent_action_metrics;
mod agent_audit;
mod agent_input_metrics;
mod agent_lifecycle;
mod agent_manifest;
mod agent_metric_closure;
mod agent_metrics;
mod agent_native_shutdown;
mod agent_native_shutdown_driver;
mod agent_policy;
mod agent_progress_metrics;
mod agent_provider;
mod agent_supervisor;
mod agent_work_artifact;
mod agent_work_evidence;
pub use agent_work_evidence::{
    WorkEvidenceBudget, WorkEvidenceBuilder, WorkEvidenceDescriptor, WorkEvidenceEntry,
    WorkEvidenceEntryId, WorkEvidenceError, WorkEvidenceReview, WorkEvidenceSet,
    MAX_WORK_EVIDENCE_CONTENT_BYTES, MAX_WORK_EVIDENCE_ENTRIES,
};
/// Existing product profile identity used by durable Work result contracts.
pub use zephium_core::ids::ProfileId as AgentWorkProfileId;
mod agent_work_journal;
pub use agent_work_artifact::{
    AgentWorkArchivedExtraction, AgentWorkArtifactCompletion, AgentWorkArtifactDescriptor,
    AgentWorkArtifactPublication, AgentWorkArtifactReply, AgentWorkArtifactRequest, ArchivedField,
    ArchivedSource, ArchivedSourceContent, ArchivedText, ArchivedValue,
    MAX_AGENT_WORK_ARTIFACT_BYTES, MAX_AGENT_WORK_ARTIFACT_TOTAL_BYTES,
};
mod context;
mod context_port;
mod context_registry;
mod work_browser_document;
mod work_browser_resource;
mod work_browser_session;
pub use work_browser_document::{
    registrable_site, same_site as same_work_site, site_scheme as work_site_scheme,
    WorkBrowserDocumentPolicy, WorkBrowserRedirect,
};
pub use work_browser_resource::WorkBrowserConstructionAttempt;
pub use work_browser_resource::{
    brand_family, same_work_human_site, WorkBrowserActionCompletion,
    WorkBrowserActionCompletionCallback, WorkBrowserActionCompletionOwner,
    WorkBrowserActionDeliveryCompletion, WorkBrowserActionDeliveryTicket,
    WorkBrowserActionDispatch, WorkBrowserActionEvent, WorkBrowserActionRefusal,
    WorkBrowserActionRequest, WorkBrowserAnonymousReadAuthority, WorkBrowserAnonymousReadLoss,
    WorkBrowserAnonymousReadStatus, WorkBrowserCropGeometry, WorkBrowserExecutionLease,
    WorkBrowserHistoryBackCompletionCallback, WorkBrowserHistoryBackDispatch,
    WorkBrowserHistoryBackRequest, WorkBrowserHumanCheck, WorkBrowserHumanCheckState,
    WorkBrowserHumanProgress, WorkBrowserHumanRegion, WorkBrowserHumanTime,
    WorkBrowserLeaseDeliveryCompletion, WorkBrowserLeaseDeliveryNotification,
    WorkBrowserLeaseDeliveryPollError, WorkBrowserLeaseDeliveryProof,
    WorkBrowserLeaseDeliveryReceipt, WorkBrowserLeaseDeliveryRefusal,
    WorkBrowserLeaseDeliveryTicket, WorkBrowserLeaseEnded, WorkBrowserLeaseNativeDebt,
    WorkBrowserMediaFetchMode, WorkBrowserMediaProgress, WorkBrowserNavigationCompletion,
    WorkBrowserNavigationCompletionCallback, WorkBrowserNavigationDispatch,
    WorkBrowserNavigationEvent, WorkBrowserNavigationPreparation, WorkBrowserNavigationRequest,
    WorkBrowserObservationCapability, WorkBrowserObservationCompletion,
    WorkBrowserObservationCompletionCallback, WorkBrowserObservationDispatch,
    WorkBrowserObservationEvent, WorkBrowserObservationRequest, WorkBrowserPublicAssetReply,
    WorkBrowserPublicAssetRequest, WorkBrowserReadBinding, WorkBrowserRecordImage,
    WorkBrowserRecordImageOrigin, WorkBrowserResourceCompletion,
    WorkBrowserResourceCompletionCallback, WorkBrowserResourceDispatch, WorkBrowserResourceError,
    WorkBrowserResourceEvent, WorkBrowserResourceFailure, WorkBrowserResourceHealth,
    WorkBrowserResourceHealthReporter, WorkBrowserResourceHealthState, WorkBrowserResourceId,
    WorkBrowserResourceIdentity, WorkBrowserResourceJoin, WorkBrowserResourceNativeOutcome,
    WorkBrowserResourceOperation, WorkBrowserResourcePhase, WorkBrowserResourceRequest,
    WorkBrowserResources, WorkId, MAX_PUBLIC_RECORD_IMAGE_BYTES, MAX_WORK_HUMAN_WAIT_MILLIS,
};
pub use work_browser_session::{
    WeakWorkBrowserSession, WorkBrowserSession, WorkBrowserSessionHumanHold,
};
#[cfg(feature = "probe-harness")]
mod contract;
#[cfg(feature = "probe-harness")]
mod control;
mod cookie_transfer;
#[cfg(feature = "probe-harness")]
mod evidence;
#[cfg(feature = "probe-harness")]
mod fixture_server;
#[cfg(feature = "probe-harness")]
mod foreground_rendering_probe;
#[cfg(feature = "probe-harness")]
pub use foreground_rendering_probe::{
    ForegroundRenderingProbeCompletion, ForegroundRenderingProbeOperation,
    ForegroundRenderingProbeRequest, ForegroundRenderingState,
};
#[cfg(feature = "probe-harness")]
mod probe_evidence_path;
#[cfg(feature = "probe-harness")]
mod probe_qualification;
#[cfg(feature = "probe-harness")]
mod probe_recipes;
mod profile_lease;
#[cfg(feature = "probe-harness")]
mod protocol;
#[cfg(feature = "provider-transport")]
mod provider_transport;
#[cfg(feature = "provider-transport")]
pub use provider_transport::lead;
#[cfg(feature = "provider-transport")]
pub mod public_asset;
mod semantic;
mod semantic_media;
#[cfg(feature = "probe-harness")]
mod semantic_media_probe;
#[cfg(feature = "provider-transport")]
pub use provider_transport::youtube_oembed;
pub use semantic_media::{
    SemanticMediaEvidence, SemanticMediaKind, SemanticRecordMediaDiagnostics,
    SemanticRecordMediaOutcome, SemanticRecordMediaPlan, MAX_SEMANTIC_MEDIA,
    MAX_SEMANTIC_MEDIA_RELATIONS,
};
#[cfg(feature = "probe-harness")]
pub use semantic_media_probe::{
    set_semantic_media_name_probe, SemanticMediaNameObserver, SemanticMediaNameProbe,
};
mod secret_shapes;
mod semantic_approval;
pub use semantic_approval::SemanticApprovalBinding;
mod semantic_action;
mod semantic_action_batch_result;
mod semantic_action_result;
mod semantic_diff;
mod semantic_diff_model;
mod semantic_execute;
mod semantic_execute_coordinator;
mod semantic_extract;
mod semantic_extract_model;
mod semantic_extract_repair;
mod semantic_locate;
mod semantic_locate_model;
mod semantic_model;
mod semantic_money;
mod semantic_observation;
mod semantic_person;
#[cfg(feature = "probe-harness")]
mod semantic_probe_evidence;
mod semantic_read;
mod semantic_read_model;
mod semantic_record;
mod semantic_runtime;
mod semantic_screenshot;
mod semantic_settle;
mod semantic_settle_coordinator;
mod semantic_suggestion;
mod semantic_verify;
mod semantic_wait;
mod semantic_wire;
pub use secret_shapes::contains_recognized_secret;
mod sign_in_handoff;

pub use agent_action_metrics::{
    AgentActionBackendMetrics, AgentActionDurationMetrics, AgentActionMetricError,
    AgentActionSettleEventMetrics, AgentRunActionPerformanceMetrics,
    AgentRunActionPerformanceSnapshot, AGENT_ACTION_DURATION_BUCKET_COUNT,
    AGENT_ACTION_DURATION_BUCKET_UPPER_BOUNDS_MILLIS, MAX_AGENT_ACTION_PERFORMANCE_SNAPSHOT_BYTES,
};
pub use agent_audit::{
    AgentAuditCompletion, AgentAuditDelivery, AgentAuditDeliveryId, AgentAuditDeliveryOutcome,
    AgentAuditDeliveryProof, AgentAuditDeliverySettlement, AgentAuditDispatch, AgentAuditError,
    AgentAuditEvent, AgentAuditEventId, AgentAuditLedger, AgentAuditLedgerStatus, AgentAuditPort,
    AgentAuditRecordV1, AgentAuditSinkFailure, AGENT_AUDIT_RECORD_V1_BYTES,
    MAX_AGENT_AUDIT_DELIVERY_EVENTS, MAX_PENDING_AGENT_AUDIT_EVENTS,
};
pub use agent_input_metrics::{
    AgentProviderInputKind, AgentProviderInputKindMetrics, AgentProviderInputMetricError,
    AgentProviderInputNodeMetrics, AgentProviderInputShapeMetrics, AgentRunProviderInputMetrics,
    AgentRunProviderInputSnapshot, MAX_AGENT_PROVIDER_INPUT_SNAPSHOT_BYTES,
};
pub use agent_lifecycle::{
    AgentBrowserLifecycle, AgentBrowserShutdownOutcome, AgentProviderShutdownProof,
};
pub use agent_manifest::{
    AgentAccountAttestationId, AgentAccountId, AgentAccountScope, AgentContextAccountBinding,
    AgentDataFlowRule, AgentEffectScope, AgentManifestContractError, AgentNavigationDiscovery,
    AgentNavigationOriginRule, AgentNavigationRoute, AgentPlanLeaseId, AgentPlanNodeAuthority,
    AgentPlanNodeId, AgentPlanNodeScope, AgentPolicyInstant, AgentRunBudget, AgentRunManifest,
    AgentRunManifestId, AgentRunScope, MAX_AGENT_DATA_FLOW_RULES,
    MAX_AGENT_NAVIGATION_DESTINATION_VISITS, MAX_AGENT_NAVIGATION_DISCOVERY_HOPS,
    MAX_AGENT_NAVIGATION_DISCOVERY_RULES, MAX_AGENT_NAVIGATION_ROUTE_HOPS, MAX_AGENT_PLAN_NODES,
    MAX_AGENT_RUN_ACCOUNTS, MAX_AGENT_RUN_CONTEXTS, MAX_AGENT_RUN_COST_MICRO_USD,
    MAX_AGENT_RUN_LIFETIME_MILLIS, MAX_AGENT_RUN_MODEL_TOKENS, MAX_AGENT_RUN_OPERATIONS,
    MAX_AGENT_RUN_ORIGINS, MAX_AGENT_RUN_PROFILES,
};
pub use agent_metric_closure::{
    AgentRunMetricClosure, AgentRunMetricClosureError, MAX_AGENT_RUN_METRIC_CLOSURE_BYTES,
};
pub use agent_metrics::{
    AgentEffectAccountingMetrics, AgentEffectClassMetrics, AgentMetricError,
    AgentModelAccountingMetrics, AgentNodeAccountingMetrics, AgentPricingScheduleMetrics,
    AgentRunAccountingMetrics, AgentRunAccountingSnapshot, MAX_AGENT_METRIC_PRICING_SCHEDULES,
};
pub use agent_native_shutdown::{
    AgentNativeShutdownAdmissionError, AgentNativeShutdownAdmissionRefusal,
    AgentNativeShutdownCoordinator, AgentNativeShutdownError, AgentNativeShutdownFinishRefusal,
    AgentNativeShutdownProof, AgentNativeShutdownResources, AgentNativeShutdownStage,
    AgentNativeShutdownStatus, MAX_AGENT_NATIVE_SHUTDOWN_AUDITS,
    MAX_AGENT_NATIVE_SHUTDOWN_PROOF_BYTES,
};
pub use agent_native_shutdown_driver::{
    agent_native_shutdown_retry_delay, drive_agent_native_shutdown_until,
    AgentNativeShutdownDriveError, AgentNativeShutdownEventSource, AgentNativeShutdownWait,
    AGENT_NATIVE_SHUTDOWN_RETRY_BASE_MILLIS, AGENT_NATIVE_SHUTDOWN_RETRY_MAX_MILLIS,
};
pub use agent_policy::AgentNavigationLedgerRefusal;
pub use agent_policy::{
    AgentActiveEffect, AgentActiveModelCall, AgentActiveNavigation, AgentEffectAssessment,
    AgentEffectAuthorization, AgentEffectCancellation, AgentEffectDispatchRequest, AgentEffectId,
    AgentEffectPermit, AgentEffectReceipt, AgentEffectRequest, AgentEffectSettlement,
    AgentFailedSemanticEffect, AgentModelCallAdmission, AgentModelCallBudget, AgentModelCallId,
    AgentModelCallReceipt, AgentModelCallRequest, AgentModelCallSettlement,
    AgentModelCallUnaccountedSettlement, AgentModelInputCancellation, AgentModelUsageAccounting,
    AgentNavigationAuthorizationRequest, AgentNavigationKind, AgentNavigationPermit,
    AgentNavigationProgressId, AgentNavigationReceipt, AgentNavigationSettlement,
    AgentNeedsHumanReason, AgentNeedsHumanTransition, AgentPlanLeaseBinding, AgentPolicyAccounting,
    AgentPolicyError, AgentRunPolicy, AgentRunPolicySettlement, AgentRunPolicySettlementBinding,
    AgentRunPolicySettlementError, AgentRunPolicySettlementRefusal, AgentTaintCohort,
    AgentVerifiedSemanticEffect, MAX_AGENT_ACCOUNT_ATTESTATION_AGE_MILLIS,
    MAX_AGENT_PENDING_EFFECTS, MAX_AGENT_PENDING_MODEL_CALLS,
    MAX_AGENT_RUN_POLICY_SETTLEMENT_BYTES, MAX_AGENT_TAINT_COHORTS, MAX_AGENT_TAINT_REFERENCES,
};
pub use agent_progress_metrics::{
    AgentDurationMetrics, AgentNeedsHumanMetrics, AgentProgressMetricError,
    AgentRunProgressMetrics, AgentRunProgressOutcome, AgentRunProgressSnapshot,
};
#[cfg(test)]
pub(crate) use agent_provider::AgentBrowserToolCall;
#[cfg(feature = "provider-transport")]
pub use agent_provider::AgentProviderModelReplyFacts;
pub use agent_provider::AgentProviderNavigationRefusalReason;
pub(crate) use agent_provider::AgentProviderPricedUsage;
#[cfg(feature = "provider-transport")]
pub(crate) use agent_provider::{
    translate_model_client_call, AgentCommittedProviderRequest, AgentProviderContinuationSeed,
    AgentProviderFinishedStream, AgentProviderStreamDecoder,
};
pub use agent_provider::{
    AgentBrowserActProposal, AgentBrowserFormBinding, AgentBrowserFormError, AgentBrowserFormFault,
    AgentBrowserFormField, AgentBrowserFormProposal, AgentBrowserHumanReason,
    AgentBrowserScopeProposal, AgentBrowserSemanticQuery, AgentBrowserToolCallId,
    AgentBrowserToolContractError, AgentBrowserToolKind, AgentBrowserToolProposal,
    AgentBrowserWaitCondition, AgentCommittedProviderInput, AgentPreparedDiffRequest,
    AgentPreparedExtractionRequest, AgentPreparedLocateRequest, AgentPreparedObservationRequest,
    AgentPreparedReadContinuationRequest, AgentPreparedReadRequest, AgentPreparedScreenshotRequest,
    AgentProviderActionAuthority, AgentProviderActionRefusal, AgentProviderActionRefusalContext,
    AgentProviderActionRefusalKey, AgentProviderActionResolution,
    AgentProviderActionResolutionError, AgentProviderBillingClass,
    AgentProviderBoundDiffContinuation, AgentProviderBoundExtractionContinuation,
    AgentProviderBoundLocateContinuation, AgentProviderBoundReadContinuation,
    AgentProviderBoundScreenshotContinuation, AgentProviderCallConfig, AgentProviderCallIdentity,
    AgentProviderCompletion, AgentProviderContinuation, AgentProviderContinuationError,
    AgentProviderContractError, AgentProviderDecisionInputStats, AgentProviderDiffRequestDraft,
    AgentProviderEndpoint, AgentProviderExactInputCount, AgentProviderExtractionOutputBinding,
    AgentProviderExtractionOutputCollector, AgentProviderExtractionOutputError,
    AgentProviderExtractionRequestDraft, AgentProviderFailure, AgentProviderFailureClass,
    AgentProviderInputAccountingMode, AgentProviderInputEvidence, AgentProviderInputMetricReceipt,
    AgentProviderInputMetrics, AgentProviderInputOutcome, AgentProviderInputTokenBinding,
    AgentProviderInputTokenCount, AgentProviderInputTokenRequest, AgentProviderKind,
    AgentProviderLocalInputTokenCounter, AgentProviderLocateRequestDraft,
    AgentProviderModelRevision, AgentProviderNavigationCheckpoint, AgentProviderNavigationRefusal,
    AgentProviderObjective, AgentProviderObjectiveError, AgentProviderObservationCheckpoint,
    AgentProviderObservationRefusal, AgentProviderObservationResolution,
    AgentProviderPricingAttribution, AgentProviderPricingContractError, AgentProviderPricingError,
    AgentProviderPricingProfile, AgentProviderPricingRevision, AgentProviderPricingSchedule,
    AgentProviderPricingSettlement, AgentProviderPricingSettlementError,
    AgentProviderProtocolError, AgentProviderProtocolEvent,
    AgentProviderReadContinuationRequestDraft, AgentProviderReasoningEffort,
    AgentProviderRejectedAction, AgentProviderRequest, AgentProviderRequestDigest,
    AgentProviderRequestError, AgentProviderRequestSettlement, AgentProviderResponseIdentity,
    AgentProviderResponseRoute, AgentProviderRetryAfter, AgentProviderRetryDisposition,
    AgentProviderScreenshotRequestDraft, AgentProviderSemanticInputStats,
    AgentProviderSettledTerminal, AgentProviderSettledToolTurn, AgentProviderStopReason,
    AgentProviderStreamBatch, AgentProviderStreamBudget, AgentProviderStreamConclusion,
    AgentProviderStreamStats, AgentProviderTerminalFailure, AgentProviderTextDelta,
    AgentProviderTokenRates, AgentProviderTransportInput, AgentProviderUsage,
    AGENT_BROWSER_SNAPSHOT_ACTION_KINDS, MAX_AGENT_BROWSER_FORM_FIELDS,
    MAX_AGENT_BROWSER_NAVIGATION_URL_BYTES, MAX_AGENT_BROWSER_SEMANTIC_QUERY_BYTES,
    MAX_AGENT_PROVIDER_ALLOWED_EFFECTIVE_MODELS,
    MAX_AGENT_PROVIDER_CONTINUATION_INITIAL_OBSERVATION_BYTES,
    MAX_AGENT_PROVIDER_CONTINUATION_TRANSCRIPT_BYTES, MAX_AGENT_PROVIDER_CONTINUATION_TURNS,
    MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS, MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES,
    MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES, MAX_AGENT_PROVIDER_OBJECTIVE_BYTES,
    MAX_AGENT_PROVIDER_OBJECTIVE_TOKENS, MAX_AGENT_PROVIDER_OUTPUT_TEXT_BYTES,
    MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS, MAX_AGENT_PROVIDER_REPORTED_INPUT_TOKENS,
    MAX_AGENT_PROVIDER_REQUEST_BYTES, MAX_AGENT_PROVIDER_RETRY_AFTER_MILLIS,
    MAX_AGENT_PROVIDER_SCREENSHOT_PNG_BYTES, MAX_AGENT_PROVIDER_SCREENSHOT_TRANSCRIPT_BYTES,
    MAX_AGENT_PROVIDER_SERVICE_TIER_BYTES, MAX_AGENT_PROVIDER_SSE_EVENT_BYTES,
    MAX_AGENT_PROVIDER_SSE_LINE_BYTES, MAX_AGENT_PROVIDER_STREAM_EVENTS,
    MAX_AGENT_PROVIDER_STREAM_WIRE_BYTES, MAX_AGENT_PROVIDER_TOOL_ARGUMENT_BYTES,
    MAX_AGENT_PROVIDER_TOOL_CALLS, MAX_AGENT_PROVIDER_TOOL_CALL_ID_BYTES,
    MAX_AGENT_PROVIDER_VIEWPORT_IMAGE_PNG_BYTES, MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS,
};
pub use agent_supervisor::{
    AgentDelegationNode, AgentDelegationSpec, AgentDelegationTopology, AgentNodeExecution,
    AgentProgressActivity, AgentProgressBlocker, AgentProgressOperation, AgentProgressResource,
    AgentProgressResult, AgentProgressState, AgentRunSupervisor, AgentSemanticProgress,
    AgentSupervisorAttemptId, AgentSupervisorCancellation, AgentSupervisorCancellationBatch,
    AgentSupervisorCancellationId, AgentSupervisorCancellationReason,
    AgentSupervisorCancellationTarget, AgentSupervisorCompletion, AgentSupervisorContextAssignment,
    AgentSupervisorContextCancellationTarget, AgentSupervisorContextRelease,
    AgentSupervisorContextReleaseOutcome, AgentSupervisorContractError,
    AgentSupervisorExecutionOutcome, AgentSupervisorExecutionReceipt, AgentSupervisorFailure,
    AgentSupervisorId, AgentSupervisorNodeCancellation, AgentSupervisorNodeSnapshot,
    AgentSupervisorNodeStatus, AgentSupervisorRuntimeError, AgentSupervisorRuntimeStatus,
    AgentSupervisorWait, AgentWorkChildOutput, AgentWorkExecution, AgentWorkExecutionRefusal,
    AgentWorkNodeCheckpoint, AgentWorkOrchestration, AgentWorkOrchestrationCheckpoint,
    AgentWorkOrchestrationError, AgentWorkOutputReference, MAX_AGENT_DELEGATION_DEPTH,
    MAX_AGENT_EXECUTING_SUPERVISOR_NODES, MAX_AGENT_LIVE_SUPERVISOR_NODES,
    MAX_AGENT_WORK_NODE_OUTPUTS, MAX_AGENT_WORK_ORCHESTRATION_OUTPUTS,
    MAX_AGENT_WORK_ORCHESTRATION_TURNS,
};
pub use agent_work_journal::{
    outstanding_agent_work_runs, remember_agent_work_record, AgentWorkDebt, AgentWorkDisposition,
    AgentWorkHumanHandoff, AgentWorkIncarnation, AgentWorkJournalCompletion, AgentWorkJournalError,
    AgentWorkJournalMutation, AgentWorkJournalPort, AgentWorkJournalReply, AgentWorkJournalRequest,
    AgentWorkRecord, AGENT_WORK_RECORD_BYTES, MAX_AGENT_WORK_INVENTORY,
    MAX_DURABLE_AGENT_WORK_RUNS, RETAINED_CLEAN_AGENT_WORK_RUNS,
};
#[cfg(all(
    feature = "provider-transport",
    feature = "probe-harness",
    target_os = "macos"
))]
pub use provider_transport::load_macos_probe_openai_credential;
#[cfg(all(feature = "provider-transport", feature = "probe-harness"))]
pub use provider_transport::load_probe_openai_credential;
#[cfg(all(feature = "provider-transport", feature = "probe-harness"))]
pub use provider_transport::{exact_loopback_url, ProviderEndpoints};
#[cfg(feature = "provider-transport")]
pub use provider_transport::{
    load_development_openai_credential, load_development_typesafe_credential,
    AgentProviderVaultError,
};
#[cfg(all(feature = "provider-transport", target_os = "macos"))]
pub use provider_transport::{
    load_macos_development_openai_credential, load_macos_development_typesafe_credential,
    MacosAgentProviderCredentialError, MACOS_OPENAI_KEYCHAIN_ACCOUNT,
    MACOS_OPENAI_KEYCHAIN_SERVICE, MACOS_TYPESAFE_KEYCHAIN_ACCOUNT,
    MACOS_TYPESAFE_KEYCHAIN_SERVICE,
};
#[cfg(feature = "provider-transport")]
pub use provider_transport::{
    AgentCredentialBinding, AgentProviderAbortReason, AgentProviderAdmissionError,
    AgentProviderAttempt, AgentProviderAttemptStateError, AgentProviderBatchDisposition,
    AgentProviderCancellation, AgentProviderCountedAttempt, AgentProviderCredential,
    AgentProviderCredentialError, AgentProviderDisclosureStage, AgentProviderExactCountOutcome,
    AgentProviderImmediateSettlement, AgentProviderImmediateSettlementError,
    AgentProviderPolicySettlement, AgentProviderTransport, AgentProviderTransportConfig,
    AgentProviderTransportConfigError, AgentProviderTransportOutcome, AgentProviderTransportResult,
    AgentProviderTransportShutdownError, AgentProviderTransportShutdownProof,
    AgentProviderTransportSnapshot, AgentProviderTransportStateError, AgentProviderUsageKnowledge,
    DecisionCredentialProvider, AGENT_PROVIDER_HTTP2_INITIAL_RECEIVE_WINDOW_BYTES,
    MAX_AGENT_PROVIDER_CONNECT_TIMEOUT_MILLIS, MAX_AGENT_PROVIDER_CREDENTIAL_BYTES,
    MAX_AGENT_PROVIDER_HTTP2_FRAME_BYTES, MAX_AGENT_PROVIDER_READ_TIMEOUT_MILLIS,
    MAX_AGENT_PROVIDER_REQUEST_TIMEOUT_MILLIS, MAX_AGENT_PROVIDER_RESPONSE_HEADER_BYTES,
    MAX_AGENT_PROVIDER_TRANSPORT_CALLS, MAX_AGENT_PROVIDER_TRANSPORT_SHUTDOWN_PROOF_BYTES,
    MAX_OPENAI_INPUT_TOKEN_RESPONSE_BYTES,
};
pub use semantic_wait::{
    SemanticStandaloneWait, SemanticStandaloneWaitBackoff, SemanticStandaloneWaitError,
    SemanticStandaloneWaitOutcome, SemanticStandaloneWaitResult, SemanticStandaloneWaitStep,
    SEMANTIC_STANDALONE_WAIT_INITIAL_POLL_MILLIS, SEMANTIC_STANDALONE_WAIT_MAX_POLL_MILLIS,
};

pub use semantic_locate::{
    locate_semantic_observation, SemanticLocateBudget, SemanticLocateError, SemanticLocateId,
    SemanticLocateMatch, SemanticLocateMatchQuality, SemanticLocateQuery, SemanticLocateRequest,
    SemanticLocateResult, SemanticLocateScope, SemanticLocateStats, MAX_SEMANTIC_LOCATE_MATCHES,
    MAX_SEMANTIC_LOCATE_QUERY_BYTES, MAX_SEMANTIC_LOCATE_QUERY_TERMS,
};
pub use semantic_locate_model::{
    encode_semantic_locate_result, SemanticEncodedLocateResult, SemanticLocateDeliveryReceipt,
    SemanticLocateEncodingStats, SemanticLocateModelPayload, SEMANTIC_LOCATE_MODEL_SCHEMA_VERSION,
};

pub use context::{
    ContextAutomationState, ContextCapabilities, ContextCapability, ContextCapabilityError,
    ContextControl, ContextFreshness, ContextGeneration, ContextId, ContextIdentity, ContextJoin,
    ContextKind, ContextLifecycle, ContextOperationId, ContextOperationJoin, ContextOperationKind,
    ContextRunId, ContextSettlement, ContextStatus, ContextTerminal, ContextTransitionError,
    ContextVisibility, FrameGeneration, FrameId, NavigationEpoch, RunCancellationGeneration,
};
pub use context_port::{
    AgentBrowserPort, BorrowedTabLeaseId, ContextCancellationRequest,
    ContextCancellationSettlement, ContextConstructionProof, ContextConstructionRequest,
    ContextConstructionSettlement, ContextConstructionSource, ContextDispatch, ContextNativeEvent,
    ContextNativePlatform, ContextNativeRequest, ContextNativeResourceCounts,
    ContextNativeResourceSnapshot, ContextNavigationRedirectPolicy, ContextNavigationReplacement,
    ContextNavigationRequest, ContextNavigationSettlement, ContextNavigationTarget,
    ContextOwnedViewport, ContextPortContractError, ContextPortFailure, ContextRendererLoss,
    ContextResourceAuditId, ContextResourceAuditSettlement, ContextShutdownAuditSettlement,
    ContextShutdownDispatch, ContextTransitionRequest, ContextTransitionSettlement,
    SemanticActionNativeCompletion, SemanticScreenshotNativeCompletion, WorkBrowserFrame,
    MAX_CONTEXT_NAVIGATION_REDIRECTS, MAX_CONTEXT_NAVIGATION_REDIRECT_ORIGINS,
    MAX_PENDING_NATIVE_CONTEXT_TASKS,
};
pub use context_registry::{
    ContextRegistry, ContextRegistryEntry, ContextRegistryEntryState, ContextRegistryError,
    ContextRegistryStatus, ContextResourceDisposition, RetiredContext, MAX_EXECUTING_CONTEXTS,
    MAX_LIVE_CONTEXTS,
};
#[cfg(feature = "probe-harness")]
pub use contract::{
    FixtureCase, FixtureTarget, FocusOwner, GateOutcome, InputBackend, InputEventKind,
    PresentationState,
};
#[cfg(feature = "probe-harness")]
pub use control::{ProbeAdmissionError, ProbeGate, ProbeRunPermit};
pub use cookie_transfer::{
    ContextCookieOrigin, ContextCookieScope, ContextCookieTransferCounts,
    ContextCookieTransferDirection, ContextCookieTransferError, ContextCookieTransferFailure,
    ContextCookieTransferId, ContextCookieTransferInstant, ContextCookieTransferOutcome,
    ContextCookieTransferRegistry, ContextCookieTransferRegistryStatus,
    ContextCookieTransferRequest, ContextCookieTransferSettlement, ContextCookieTransferStats,
    ContextCookieTransferWindow, MAX_COOKIES_PER_TRANSFER, MAX_COOKIE_BYTES,
    MAX_COOKIE_TRANSFER_BYTES, MAX_COOKIE_TRANSFER_MILLIS, MAX_COOKIE_TRANSFER_ORIGINS,
    MAX_PENDING_COOKIE_TRANSFERS,
};
#[cfg(feature = "probe-harness")]
pub use evidence::{
    ActivationEvidence, BackendAvailability, BackendCapability, CaseEvidence, CaseOutcome,
    EvidenceLabel, EvidenceValidationError, FocusEvidence, InputEventEvidence, Platform,
    ProbeFailure, ProbeFailureCode, ProbeStage, ResourceEvidence, RunEvidence, RuntimeFingerprint,
    TargetEvidence, TeardownEvidence, MAX_CASE_EVIDENCE, MAX_EVENT_EVIDENCE,
    MAX_RESOURCE_HELPER_PROCESSES, MAX_RESOURCE_RESIDENT_BYTES, MAX_RESOURCE_WEBVIEW2_PROCESSES,
};
#[cfg(feature = "probe-harness")]
pub use fixture_server::{FixtureRoute, FixtureServer, FixtureServerError};
#[cfg(feature = "probe-harness")]
pub use probe_evidence_path::{
    evidence_metadata_is_direct_directory, evidence_metadata_is_direct_file,
};
#[cfg(feature = "probe-harness")]
pub use probe_qualification::{
    qualify_windows_probe_evidence, WindowsProbeAggregate, WindowsProbeMode,
    WindowsProbeQualificationError, WINDOWS_PHYSICAL_REVIEW_MODES, WINDOWS_PROBE_CAPABILITIES,
    WINDOWS_PROBE_CASES,
};
#[cfg(feature = "probe-harness")]
pub use probe_recipes::{
    windows_input_plan, windows_key_message_lparam, FixedProbeScript, ProbeScriptWorld,
    WindowsInputStep, WindowsProbeGeometry, WindowsProbeKey, WindowsProbePoint,
    MACOS_NATIVE_INPUT_RUNTIME_V1, MACOS_PROBE_CONTENT_WORLD_V1, MACOS_PROBE_HANDLER_V1,
    MAX_NATIVE_INPUT_RUNTIME_ROW, NATIVE_INPUT_RUNTIME_PROTOCOL_V1,
};
pub use profile_lease::{
    ContextProfileLease, ContextProfileLeaseError, ContextProfileLeaseId,
    ContextProfileLeasePurpose, ContextProfileLeaseRegistry, ContextProfileLeaseStatus,
    ContextProfileStorageClass, MAX_CONTEXT_PROFILE_TOMBSTONES,
};
#[cfg(feature = "probe-harness")]
pub use protocol::{
    decode_request_line, decode_response_line, encode_response_line, CancelRequest, CancelledReply,
    HelloReply, HelloRequest, ProbeCommand, ProbeProtocolError, ProbeReply, ProbeRequest,
    ProbeResponse, RunMatrixRequest, ShutdownReply, ShutdownRequest, MAX_BACKENDS_PER_REQUEST,
    MAX_CASES_PER_REQUEST, MAX_PROTOCOL_INPUT_BYTES, MAX_PROTOCOL_OUTPUT_BYTES,
    PROBE_PROTOCOL_VERSION,
};
pub use semantic::{
    SemanticActivation, SemanticCompleteness, SemanticContractError, SemanticEditableStructure,
    SemanticFillSupport, SemanticFormFacts, SemanticFormMethod, SemanticFrameJoin,
    SemanticFrameTrust, SemanticHeadingLevel, SemanticInvocationId, SemanticLandmarkKind,
    SemanticNode, SemanticOperationClass, SemanticOperations, SemanticOrigin, SemanticRect,
    SemanticReference, SemanticReferenceError, SemanticReferenceId, SemanticRole,
    SemanticSensitivity, SemanticSnapshot, SemanticSnapshotGeneration, SemanticState,
    SemanticStates, SemanticText, SemanticTruncation, SemanticTrust, SemanticValuePreview,
    SemanticValueSummary, SemanticValueText, MAX_SEMANTIC_DEPTH, MAX_SEMANTIC_FRAMES,
    MAX_SEMANTIC_NAME_BYTES, MAX_SEMANTIC_NODES, MAX_SEMANTIC_TEXT_BYTES,
    MAX_SEMANTIC_TOTAL_TEXT_BYTES, MAX_SEMANTIC_VALUE_BYTES, MAX_SEMANTIC_VALUE_PREVIEW_BYTES,
};
pub(crate) use semantic_action::SemanticActionRuntimeDescriptor;
pub use semantic_action::{
    SemanticActionBatch, SemanticActionBatchId, SemanticActionBindingError,
    SemanticActionContractError, SemanticActionIntent, SemanticActionKind,
    SemanticActionPreparationError, SemanticActionProposal, SemanticActionRevalidationError,
    SemanticActionText, SemanticActionTextError, SemanticBoundAction, SemanticDialogState,
    SemanticEffectClass, SemanticMutationQuietPeriod, SemanticPreparedAction, SemanticPressKey,
    SemanticScrollAmount, SemanticScrollDirection, SemanticSettleBudget, SemanticVerification,
    SemanticWaitCondition, MAX_SEMANTIC_ACTIONS_PER_BATCH, MAX_SEMANTIC_ACTION_BATCH_SETTLE_MILLIS,
    MAX_SEMANTIC_ACTION_BATCH_TEXT_BYTES, MAX_SEMANTIC_ACTION_SETTLE_MILLIS,
    MAX_SEMANTIC_ACTION_TEXT_BYTES, MAX_SEMANTIC_MUTATION_QUIET_MILLIS,
};
pub use semantic_action_batch_result::{
    SemanticActionBatchAdmissionRefusal, SemanticActionBatchCompletion,
    SemanticActionBatchContinuation, SemanticActionBatchExecution,
    SemanticActionBatchExecutionError, SemanticActionBatchFailure,
    SemanticActionBatchFailureAdmissionRefusal, SemanticActionBatchFailureStage,
    SemanticActionBatchOutcome, SemanticActionBatchResult, SemanticActionBatchStopReason,
    MAX_SEMANTIC_ACTION_BATCH_COMPLETION_BYTES, MAX_SEMANTIC_ACTION_BATCH_FAILURE_BYTES,
};
pub use semantic_action_result::{
    finalize_accounted_semantic_action_result, AgentAccountedSemanticActionResult,
    AgentAccountedSemanticActionResultRefusal, SemanticActionNextState, SemanticActionResult,
    SemanticActionResultError, SemanticPostActionObservation,
};
pub use semantic_diff::{
    compute_semantic_diff, SemanticDiff, SemanticDiffBudget, SemanticDiffBudgetError,
    SemanticDiffEntry, SemanticDiffEntryKind, SemanticDiffFrame, SemanticDiffOutcome,
    SemanticDiffStats, SemanticFreshSnapshotReason, SemanticNodeChange, SemanticNodeChanges,
    SemanticNodeMove, SemanticObservationAcknowledgement, SemanticReferenceRebase,
    SemanticRetiredReferenceId, MAX_SEMANTIC_DIFF_ENTRIES,
};
pub use semantic_diff_model::{
    encode_semantic_diff, SemanticDiffDeliveryReceipt, SemanticDiffEncodingStats,
    SemanticDiffModelPayload, SemanticEncodedDiff, SEMANTIC_DIFF_MODEL_SCHEMA_VERSION,
};
pub use semantic_execute::{
    begin_semantic_action_settlement, SemanticActionExecutionApplied,
    SemanticActionExecutionBackend, SemanticActionExecutionContractError,
    SemanticActionExecutionDisposition, SemanticActionExecutionInstant,
    SemanticActionExecutionOutcome, SemanticActionExecutionPreparationError, SemanticActionFollow,
    SemanticActionNativeFailure, SemanticActionNativeReadiness, SemanticActionNativeRequest,
    SemanticActionNativeSettlement, SemanticActionNativeTargetId, SemanticActionNativeViewport,
    SemanticActionNativeViewportError, SemanticActionSettlementRefusal,
    SemanticActionSettlementStart, SemanticActionSettlementStartError,
    MAX_SEMANTIC_ACTION_NATIVE_EXECUTION_MILLIS, MAX_SEMANTIC_ACTION_VIEWPORT_DIMENSION,
};
pub(crate) use semantic_execute::{
    prepare_semantic_action_execution, SemanticActionExecutionPending,
};
pub use semantic_execute_coordinator::{
    SemanticActionExecutionCoordinator, SemanticActionExecutionCoordinatorError,
    SemanticActionExecutionCoordinatorRefusal, SemanticActionExecutionCoordinatorStatus,
    SemanticActionExecutionDispatch, SemanticActionExecutionReservation,
    MAX_PENDING_SEMANTIC_ACTION_EXECUTIONS,
};
#[cfg(feature = "probe-harness")]
pub use semantic_execute_coordinator::{
    SemanticActionQualificationError, SemanticClickQualificationError,
    SemanticClickQualificationExecution, SemanticFillQualificationError,
    SemanticFillQualificationExecution, SemanticModelActionQualificationExecution,
    SemanticModelClickQualificationExecution,
};
pub use semantic_extract::{
    extract_delivered_semantic_read, extract_semantic_read,
    semantic_extraction_refusal_is_retryable, semantic_extraction_refusal_note,
    SemanticExtractedBoolean, SemanticExtractedField, SemanticExtractedMoney, SemanticExtractedRow,
    SemanticExtractedRows, SemanticExtractedText, SemanticExtractedTextList,
    SemanticExtractedUnsigned, SemanticExtractedValue, SemanticExtractionError,
    SemanticExtractionFieldSchema, SemanticExtractionResult, SemanticExtractionSchema,
    SemanticExtractionSchemaError, SemanticExtractionSchemaId, SemanticExtractionSource,
    SemanticExtractionSourceSpan, SemanticExtractionStats, SemanticExtractionTrust,
    SemanticExtractionValueKind, SemanticOwnedExtractionResult, SemanticOwnedExtractionSource,
    SemanticOwnedReadContent, MAX_SEMANTIC_EXTRACTION_FIELDS,
    MAX_SEMANTIC_EXTRACTION_FIELD_NAME_BYTES, MAX_SEMANTIC_EXTRACTION_INPUT_BYTES,
    MAX_SEMANTIC_EXTRACTION_LIST_ITEMS, MAX_SEMANTIC_EXTRACTION_LIST_ITEM_BYTES,
    MAX_SEMANTIC_EXTRACTION_SCHEMA_NAME_BYTES, MAX_SEMANTIC_EXTRACTION_SOURCES_PER_VALUE,
    MAX_SEMANTIC_EXTRACTION_SOURCE_EDGES, MAX_SEMANTIC_EXTRACTION_TEXT_BYTES,
    MAX_SEMANTIC_EXTRACTION_TOTAL_TEXT_BYTES, MAX_SEMANTIC_EXTRACTION_VALUES,
    SEMANTIC_EXTRACTION_EMPTY_NOTE, SEMANTIC_EXTRACTION_SCHEMA_VERSION,
};
pub use semantic_extract_model::{
    encode_semantic_extraction_request, SemanticEncodedExtractionRequest,
    SemanticExtractionDeliveryReceipt, SemanticExtractionEncodingStats,
    SemanticExtractionModelPayload, SEMANTIC_EXTRACTION_MODEL_SCHEMA_VERSION,
};
pub use semantic_model::{
    encode_semantic_observation, fit_semantic_observation_for_model, SemanticEncodedObservation,
    SemanticEncodingStats, SemanticModelDeliveryError, SemanticModelDeliverySettlement,
    SemanticModelEncodingBudget, SemanticModelEncodingError, SemanticModelPayload,
    SemanticTokenCountQuality, SemanticTokenCountRequirement, SemanticTokenCounter,
    SemanticTokenCounterError, SemanticTokenMeasurement, SemanticTokenMeasurementError,
    SemanticTokenizerRevision, SemanticTokenizerRevisionError,
    ACTION_DIFF_PROVIDER_EXACT_CONSERVATIVE_TOKEN_CEILING, ACTION_SEMANTIC_DIFF_TOKEN_TARGET,
    INITIAL_SEMANTIC_MODEL_TOKEN_TARGET, MAX_SEMANTIC_MODEL_BYTES, MAX_SEMANTIC_MODEL_TOKENS,
    MAX_SEMANTIC_TOKENIZER_REVISION_BYTES, SEMANTIC_LOCATE_RESULT_TOKEN_CEILING,
    SEMANTIC_MODEL_SCHEMA_VERSION,
};
pub use semantic_observation::{
    SemanticExpansionKind, SemanticFrameBoundary, SemanticFrameBoundaryStatus,
    SemanticFrameDeferral, SemanticFrameUnsupported, SemanticObservation,
    SemanticObservationAssembler, SemanticObservationBudget, SemanticObservationError,
    SemanticObservationGeneration, SemanticObservationId, SemanticObservationParent,
    SemanticObservationRequest, SemanticScope, SemanticScopeAnchor, SemanticTextSearch,
    SemanticTextWindow, MAX_SEMANTIC_OBSERVATION_EXPANSIONS, MAX_SEMANTIC_OBSERVATION_NODES,
    MAX_SEMANTIC_OBSERVATION_TEXT_BYTES, MAX_SEMANTIC_SURROUNDING_TEXT_BYTES,
    MAX_SEMANTIC_TEXT_SEARCH_BYTES, MAX_SEMANTIC_TEXT_SEARCH_QUERY_BYTES,
    MAX_SEMANTIC_TEXT_SEARCH_RESULTS,
};
pub use semantic_person::PersonDirectedAction;
#[cfg(feature = "probe-harness")]
pub use semantic_probe_evidence::{
    decode_windows_semantic_probe_response, encode_windows_semantic_probe_response,
    qualify_windows_semantic_probe_evidence, WindowsSemanticProbeAggregate,
    WindowsSemanticProbeEvidence, WindowsSemanticProbeFailure, WindowsSemanticProbeFailureCode,
    WindowsSemanticProbeMode, WindowsSemanticProbeProtocolError,
    WindowsSemanticProbeQualificationError, WindowsSemanticProbeReply,
    WindowsSemanticProbeResponse, WindowsSemanticProbeStage, WindowsSemanticProbeValidationError,
    WindowsSemanticResourceEvidence, WindowsSemanticTeardownEvidence,
    MAX_WINDOWS_SEMANTIC_PROBE_OUTPUT_BYTES, WINDOWS_SEMANTIC_PHYSICAL_REVIEW_MODES,
    WINDOWS_SEMANTIC_PROBE_PROTOCOL_VERSION,
};
pub use semantic_read::{
    read_selected_semantic_observation, read_semantic_observation,
    read_semantic_observation_for_schema, read_semantic_observation_for_schema_with_viewport_image,
    read_semantic_target, semantic_observation_needs_visual_mapping,
    semantic_observation_reads_sparse, SemanticCaptureInstant, SemanticReadAuthority,
    SemanticReadBudget, SemanticReadBudgetError, SemanticReadContent, SemanticReadError,
    SemanticReadField, SemanticReadFragment, SemanticReadFragmentId, SemanticReadOmission,
    SemanticReadOmissions, SemanticReadProvenance, SemanticReadResult, SemanticReadRoleSelection,
    SemanticReadRoleSelectionError, SemanticReadSensitivityLimit, SemanticReadStats,
    SemanticRetainedReadEvidence, MAX_SEMANTIC_READ_BYTES, MAX_SEMANTIC_READ_ITEMS,
    SEMANTIC_VIEWPORT_IMAGE_TEXT,
};
pub use semantic_read_model::{
    encode_semantic_read, SemanticEncodedRead, SemanticReadDeliveryReceipt,
    SemanticReadEncodingStats, SemanticReadModelPayload, SEMANTIC_READ_MODEL_SCHEMA_VERSION,
};
pub use semantic_runtime::SEMANTIC_RUNTIME_HUMAN_SURVEY;
pub use semantic_runtime::{
    encode_semantic_action_runtime_invocation, encode_semantic_runtime_invocation,
    SemanticActionRuntimeEvidence, SemanticActionRuntimeFault, SemanticActionRuntimeInvocation,
    SemanticActionRuntimeInvocationError, SemanticActionRuntimeResultError, SemanticRuntimeBudget,
    SemanticRuntimeBudgetError, SemanticRuntimeCorrelation, SemanticRuntimeFault,
    SemanticRuntimeInvocation, SemanticRuntimeInvocationError, SemanticRuntimePortFailure,
    SemanticRuntimeProgram, SemanticRuntimeResultError, SemanticRuntimeScopeClass,
    SemanticRuntimeSettlement, SemanticRuntimeSettlementError,
    MAX_SEMANTIC_ACTION_RUNTIME_REQUEST_BYTES, MAX_SEMANTIC_ACTION_RUNTIME_RESULT_BYTES,
    MAX_SEMANTIC_RUNTIME_CHANNEL_RESULT_BYTES, MAX_SEMANTIC_RUNTIME_DOCUMENT_INVOCATIONS,
    MAX_SEMANTIC_RUNTIME_REQUEST_BYTES, MAX_SEMANTIC_RUNTIME_SAFE_INTEGER,
    MAX_SEMANTIC_RUNTIME_SOURCE_BYTES, MAX_SEMANTIC_RUNTIME_VISITED_NODES,
    MIN_SEMANTIC_RUNTIME_WIRE_BYTES, SEMANTIC_RUNTIME_CHANNEL_ACK,
    SEMANTIC_RUNTIME_CHANNEL_EXHAUSTED, SEMANTIC_RUNTIME_CHANNEL_NAME,
    SEMANTIC_RUNTIME_CHANNEL_PARK, SEMANTIC_RUNTIME_CHANNEL_PARKED, SEMANTIC_RUNTIME_CHANNEL_PULL,
    SEMANTIC_RUNTIME_CHANNEL_RESULT_PREFIX, SEMANTIC_RUNTIME_CHANNEL_STOP,
    SEMANTIC_RUNTIME_GLOBAL_NAME, SEMANTIC_RUNTIME_PROGRAM, SEMANTIC_RUNTIME_PROTOCOL_VERSION,
};
pub use semantic_screenshot::{
    prepare_semantic_screenshot, SemanticModelImageHost, SemanticModelImagePlan,
    SemanticScreenshot, SemanticScreenshotBudget, SemanticScreenshotBudgetError,
    SemanticScreenshotCoordinator, SemanticScreenshotCoordinatorError,
    SemanticScreenshotCoordinatorStatus, SemanticScreenshotDeliveryReceipt,
    SemanticScreenshotError, SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure,
    SemanticScreenshotNativeRequest, SemanticScreenshotPaintEvidence, SemanticScreenshotPending,
    SemanticScreenshotPixelLayout, SemanticScreenshotRequest, SemanticScreenshotRequestError,
    SemanticScreenshotRequestId, SemanticScreenshotScope, SemanticScreenshotSource,
    SemanticScreenshotSourcePort, SemanticScreenshotStats, SemanticScreenshotTrust,
    MAX_PENDING_SEMANTIC_SCREENSHOTS, MAX_SEMANTIC_SCREENSHOT_CAPTURE_MILLIS,
    MAX_SEMANTIC_SCREENSHOT_HEIGHT, MAX_SEMANTIC_SCREENSHOT_PIXELS,
    MAX_SEMANTIC_SCREENSHOT_PNG_BYTES, MAX_SEMANTIC_SCREENSHOT_PNG_CHUNKS,
    MAX_SEMANTIC_SCREENSHOT_WIDTH,
};
pub use semantic_settle::{
    SemanticActionAttemptId, SemanticActionFailure, SemanticActionRecoveryHint,
    SemanticSettleError, SemanticSettleEvent, SemanticSettleFact, SemanticSettleInstant,
    SemanticSettleStatus, SemanticSettleTracker, MAX_SEMANTIC_SETTLE_EVENTS,
};
pub use semantic_settle_coordinator::{
    SemanticActionSettlementAdmissionRefusal, SemanticActionSettlementAdvanceRefusal,
    SemanticActionSettlementCoordinator, SemanticActionSettlementCoordinatorError,
    SemanticActionSettlementCoordinatorStatus, SemanticActionSettlementReservation,
    SemanticActionSettlementTerminal, SemanticActionSettlementUpdate,
    MAX_PENDING_SEMANTIC_ACTION_SETTLEMENTS,
};
pub use semantic_suggestion::{
    semantic_focused_text_field, semantic_picker_values, semantic_shows_value,
    semantic_sole_choice, semantic_typed_choices,
};
#[cfg(test)]
pub(crate) use semantic_verify::verify_semantic_action;
pub use semantic_verify::{
    prepare_semantic_action_snapshot_evidence, verify_semantic_action_terminal,
    SemanticActionVerificationRefusal, SemanticActionVerifiedTerminal, SemanticEffectEvidence,
    SemanticEffectProofKind, SemanticScrollPosition, SemanticScrollPositionError,
    SemanticSnapshotEvidenceError, SemanticVerificationError, SemanticVerifiedAction,
    MAX_SEMANTIC_SCROLL_COORDINATE,
};
pub use semantic_wire::{
    decode_semantic_snapshot, plain_url_fragment, SemanticDecodeContext, SemanticDecodeError,
    MAX_SEMANTIC_WIRE_BYTES, SEMANTIC_WIRE_VERSION,
};
pub use sign_in_handoff::{
    ContextSignInHandoff, ContextSignInHandoffBlocker, ContextSignInHandoffCleanup,
    ContextSignInHandoffError, ContextSignInHandoffId, ContextSignInHandoffPlatform,
    ContextSignInHandoffState,
};

#[cfg(all(feature = "probe-harness", feature = "provider-transport"))]
pub use provider_transport::agent::agent_turn_wire_faults;
#[cfg(feature = "provider-transport")]
pub use provider_transport::agent::{OpenAiWorkAgent, WorkAgentWireFault};
#[cfg(all(feature = "probe-harness", feature = "provider-transport"))]
pub use provider_transport::decision::capture_app_views;
#[cfg(feature = "provider-transport")]
pub use provider_transport::decision::{
    app_view_control, app_view_wire, app_views, captured_app_view, native_listing_census,
    read_app_view, read_cited_rows, search_enough_projection, search_reuse_projection,
    untracked_document_address, AdmittedDecisionOutput, AppView, AppViewSink, DailyApp,
    DecisionActionSelection, DecisionBackendKind, DecisionCallAccounting, DecisionCallDiagnostic,
    DecisionCallFailure, DecisionCallOutput, DecisionEnvelopeFacts, DecisionEnvelopeFailure,
    DecisionLocatedRead, DecisionObservation, DecisionObservationAnswers,
    DecisionObservationFallback, DecisionOperation, DecisionProjectionError, DecisionReadSelection,
    DecisionRowDiscovery, JevDecisionClient, ModelDecisionCall, NativeListingCensus,
    NativeListingContinuation, OpenAiDecisionCall, OpenAiDecisionClient, SearchQueryTerms,
};
#[cfg(feature = "provider-transport")]
pub use provider_transport::decision::{read_search_page, SearchPageRead, SEARCH_PAGE_RESULTS};
#[cfg(feature = "provider-transport")]
pub use provider_transport::planning::{OpenAiWorkPlanner, WorkPlanningConfig};
#[cfg(feature = "provider-transport")]
pub use provider_transport::synthesis::OpenAiWorkSynthesizer;

#[cfg(feature = "provider-transport")]
pub use provider_transport::browser_search::{BrowserSearch, BROWSER_SEARCH_FALLBACKS};
#[cfg(feature = "provider-transport")]
pub use provider_transport::model_client::AgentProviderModelClient;
#[cfg(feature = "provider-transport")]
pub use provider_transport::model_search::ModelNativeSearch;
#[cfg(feature = "provider-transport")]
pub use provider_transport::search::{
    OpenAiPublicSearch, OpenAiPublicSearchCitation, OpenAiPublicSearchConfig,
    OpenAiPublicSearchResult,
};
/// The provider-neutral model vocabulary a chosen-model client speaks.
#[cfg(feature = "provider-transport")]
pub use zephium_core::work::model as work_model;
