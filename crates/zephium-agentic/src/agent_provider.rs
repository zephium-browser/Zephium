//! Provider-neutral, content-redacted model transport contracts.
//!
//! This module contains no HTTP client, credential, task, timer, retry loop,
//! browser authority, or model-selected native operation. It defines the
//! bounded identity, usage, failure, and stream-budget vocabulary shared by
//! the first provider codecs. Vendor wire data remains private to those
//! codecs and can cross the public boundary only as closed typed values.

#[cfg(any(test, feature = "provider-transport"))]
mod anthropic;
mod call_schema;
mod continuation;
mod extraction;
mod form;
#[cfg(feature = "provider-transport")]
mod model_client;
#[cfg(any(test, feature = "provider-transport"))]
mod openai;
mod pricing;
mod progress;
mod request;
mod settlement;
#[cfg(any(test, feature = "provider-transport"))]
mod sse;
mod tool;

use std::fmt;
use std::sync::Arc;

#[cfg(any(test, feature = "provider-transport"))]
use sha2::{Digest, Sha256};
use thiserror::Error;

#[cfg(feature = "provider-transport")]
use anthropic::AnthropicMessagesStreamDecoder;
#[cfg(feature = "provider-transport")]
use openai::OpenAiResponsesStreamDecoder;

pub(crate) use continuation::AgentProviderContinuationSeed;
pub use continuation::{
    AgentProviderActionAuthority, AgentProviderActionRefusal, AgentProviderActionRefusalContext,
    AgentProviderActionRefusalKey, AgentProviderActionResolution,
    AgentProviderActionResolutionError, AgentProviderBoundDiffContinuation,
    AgentProviderBoundExtractionContinuation, AgentProviderBoundLocateContinuation,
    AgentProviderBoundReadContinuation, AgentProviderBoundScreenshotContinuation,
    AgentProviderContinuation, AgentProviderContinuationError, AgentProviderNavigationCheckpoint,
    AgentProviderNavigationRefusal, AgentProviderNavigationRefusalReason,
    AgentProviderObservationCheckpoint, AgentProviderObservationRefusal,
    AgentProviderObservationResolution, MAX_AGENT_PROVIDER_CONTINUATION_INITIAL_OBSERVATION_BYTES,
    MAX_AGENT_PROVIDER_CONTINUATION_TRANSCRIPT_BYTES, MAX_AGENT_PROVIDER_CONTINUATION_TURNS,
};
pub use extraction::{
    AgentProviderExtractionOutputBinding, AgentProviderExtractionOutputCollector,
    AgentProviderExtractionOutputError,
};
pub use form::{
    FormBinding as AgentBrowserFormBinding, FormError as AgentBrowserFormError,
    FormFault as AgentBrowserFormFault, FormField as AgentBrowserFormField,
    FormProposal as AgentBrowserFormProposal, MAX_FORM_FIELDS as MAX_AGENT_BROWSER_FORM_FIELDS,
};
use pricing::AgentProviderCatalogBinding;
pub(crate) use pricing::AgentProviderPricedUsage;
pub use pricing::{
    AgentProviderPricingAttribution, AgentProviderPricingContractError, AgentProviderPricingError,
    AgentProviderPricingProfile, AgentProviderPricingRevision, AgentProviderPricingSchedule,
    AgentProviderTokenRates, MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS,
    MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS, MAX_AGENT_PROVIDER_REPORTED_INPUT_TOKENS,
};
pub use settlement::{
    AgentProviderPricingSettlement, AgentProviderPricingSettlementError,
    AgentProviderRejectedAction, AgentProviderSettledTerminal, AgentProviderSettledToolTurn,
};

use crate::{
    AgentActiveModelCall, AgentModelCallAdmission, AgentModelCallId, AgentModelCallRequest,
    AgentPlanLeaseId, AgentPlanNodeId, AgentRunManifestId, SemanticTokenMeasurement,
    SemanticTokenizerRevision, MAX_AGENT_RUN_MODEL_TOKENS,
};

#[cfg(feature = "provider-transport")]
pub(crate) use model_client::translate as translate_model_client_call;
#[cfg(feature = "provider-transport")]
pub use model_client::AgentProviderModelReplyFacts;
#[cfg(feature = "provider-transport")]
pub(crate) use request::AgentCommittedProviderRequest;
pub use request::{
    AgentCommittedProviderInput, AgentPreparedDiffRequest, AgentPreparedExtractionRequest,
    AgentPreparedLocateRequest, AgentPreparedObservationRequest,
    AgentPreparedReadContinuationRequest, AgentPreparedReadRequest, AgentPreparedScreenshotRequest,
    AgentProviderDecisionInputStats, AgentProviderDiffRequestDraft, AgentProviderEndpoint,
    AgentProviderExactInputCount, AgentProviderExtractionRequestDraft, AgentProviderInputEvidence,
    AgentProviderInputMetricReceipt, AgentProviderInputMetrics, AgentProviderInputOutcome,
    AgentProviderInputTokenBinding, AgentProviderInputTokenCount, AgentProviderInputTokenRequest,
    AgentProviderLocalInputTokenCounter, AgentProviderLocateRequestDraft, AgentProviderObjective,
    AgentProviderObjectiveError, AgentProviderReadContinuationRequestDraft, AgentProviderRequest,
    AgentProviderRequestDigest, AgentProviderRequestError, AgentProviderRequestSettlement,
    AgentProviderScreenshotRequestDraft, AgentProviderSemanticInputStats,
    AgentProviderTransportInput, MAX_AGENT_BROWSER_NAVIGATION_URL_BYTES,
    MAX_AGENT_PROVIDER_INPUT_METRIC_RECEIPT_BYTES, MAX_AGENT_PROVIDER_OBJECTIVE_BYTES,
    MAX_AGENT_PROVIDER_OBJECTIVE_TOKENS, MAX_AGENT_PROVIDER_REQUEST_BYTES,
    MAX_AGENT_PROVIDER_SCREENSHOT_PNG_BYTES, MAX_AGENT_PROVIDER_SCREENSHOT_TRANSCRIPT_BYTES,
    MAX_AGENT_PROVIDER_VIEWPORT_IMAGE_PNG_BYTES,
};
pub use tool::{
    AgentBrowserActProposal, AgentBrowserHumanReason, AgentBrowserScopeProposal,
    AgentBrowserSemanticQuery, AgentBrowserToolCallId, AgentBrowserToolContractError,
    AgentBrowserToolKind, AgentBrowserToolProposal, AgentBrowserWaitCondition,
    MAX_AGENT_BROWSER_SEMANTIC_QUERY_BYTES, MAX_AGENT_PROVIDER_TOOL_CALL_ID_BYTES,
};
pub(crate) use tool::{AgentBrowserToolCall, AgentProviderToolCallCorrelation};

/// Maximum bytes in one pinned provider model revision.
pub const MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES: usize = 96;
/// Minimum total native dispatch, settlement and adjacent-observation allowance
/// admitted by the current snapshot-action driver. This is not a required wait.
/// Public native qualification observed >1 s captures on throttled owned pages.
pub const MIN_AGENT_BROWSER_SNAPSHOT_SETTLE_MILLIS: u32 = 2_000;

/// Fixed isolated-runtime actions supported by the retained snapshot-verifying
/// controller and desktop adapters.
pub const AGENT_BROWSER_SNAPSHOT_ACTION_KINDS: [crate::SemanticActionKind; 5] = [
    crate::SemanticActionKind::Click,
    crate::SemanticActionKind::Fill,
    crate::SemanticActionKind::Select,
    crate::SemanticActionKind::Press,
    crate::SemanticActionKind::Scroll,
];
/// Maximum bytes in one provider-attested service-tier identity.
pub const MAX_AGENT_PROVIDER_SERVICE_TIER_BYTES: usize = 32;
/// Maximum trusted effective model revisions accepted for one requested alias.
pub const MAX_AGENT_PROVIDER_ALLOWED_EFFECTIVE_MODELS: usize = 4;
/// Maximum decoded UTF-8 bytes in one SSE line.
pub const MAX_AGENT_PROVIDER_SSE_LINE_BYTES: usize = 512 * 1024;
/// Maximum decoded UTF-8 bytes in one complete SSE event payload.
pub const MAX_AGENT_PROVIDER_SSE_EVENT_BYTES: usize = 512 * 1024;
/// Maximum aggregate provider response-body bytes accepted for one model call.
pub const MAX_AGENT_PROVIDER_STREAM_WIRE_BYTES: u32 = 2 * 1024 * 1024;
/// Maximum provider SSE events accepted for one model call.
pub const MAX_AGENT_PROVIDER_STREAM_EVENTS: u32 = 4_096;
/// Maximum model-authored plain-text bytes accepted for one model call.
pub const MAX_AGENT_PROVIDER_OUTPUT_TEXT_BYTES: u32 = 64 * 1024;
/// Maximum client tool calls accepted from one model response.
pub const MAX_AGENT_PROVIDER_TOOL_CALLS: u8 = 1;
/// Maximum aggregate tool-argument bytes accepted for one model response.
pub const MAX_AGENT_PROVIDER_TOOL_ARGUMENT_BYTES: u32 = 32 * 1024;
/// Maximum provider-requested retry delay surfaced to policy.
pub const MAX_AGENT_PROVIDER_RETRY_AFTER_MILLIS: u64 = 24 * 60 * 60 * 1_000;

pub(crate) const OPENAI_STANDARD_SERVICE_TIER: &str = "default";
pub(crate) const ANTHROPIC_STANDARD_SERVICE_TIER_REQUEST: &str = "standard_only";
pub(crate) const ANTHROPIC_STANDARD_SERVICE_TIER_RESPONSE: &str = "standard";
pub(crate) const ANTHROPIC_GLOBAL_INFERENCE_GEO: &str = "global";
pub(crate) const MODEL_REPORTED_SERVICE_TIER: &str = "model-reported";

/// Explicit immutable reasoning effort selected for one model call.
///
/// Provider defaults are deliberately forbidden: changing a provider-side
/// default must not silently change Zephium's execution or cost identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderReasoningEffort {
    /// Disable model reasoning where the selected model supports it.
    None,
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort.
    XHigh,
    /// Maximum reasoning effort where the selected model supports it.
    Max,
}

impl AgentProviderReasoningEffort {
    pub(crate) const fn as_openai_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// First provider protocols qualified through the shared adapter contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderKind {
    /// OpenAI Responses API.
    OpenAiResponses,
    /// Anthropic Messages API.
    AnthropicMessages,
}

impl AgentProviderKind {
    /// Fixed billing class encoded for this provider protocol.
    pub const fn billing_class(self) -> AgentProviderBillingClass {
        match self {
            Self::OpenAiResponses => AgentProviderBillingClass::OpenAiDefault,
            Self::AnthropicMessages => AgentProviderBillingClass::AnthropicStandardGlobal,
        }
    }
}

/// Fixed provider billing mode selected by the immutable request contract.
///
/// This is pricing identity, not account authority. Provider terminal events
/// must attest the matching actual mode before reported usage may be priced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderBillingClass {
    /// OpenAI standard performance and pricing via `service_tier: default`.
    OpenAiDefault,
    /// Anthropic standard capacity and global inference.
    AnthropicStandardGlobal,
    /// Usage the person's chosen model reported, priced from their catalog.
    ModelReported,
}

impl AgentProviderBillingClass {
    /// Request protocol this billing class can price.
    pub const fn provider(self) -> AgentProviderKind {
        match self {
            Self::OpenAiDefault | Self::ModelReported => AgentProviderKind::OpenAiResponses,
            Self::AnthropicStandardGlobal => AgentProviderKind::AnthropicMessages,
        }
    }
}

/// Closed provider execution route selected by the trusted catalog.
///
/// The route fixes both the request wire value and every response-side tier or
/// geography attestation. Arbitrary provider strings never become pricing
/// identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AgentProviderResponseRoute {
    /// OpenAI standard processing: request and response `service_tier=default`.
    OpenAiDefault,
    /// Anthropic standard capacity constrained to global inference.
    AnthropicStandardGlobal,
    /// The person's chosen model, called through its own client. Requests
    /// keep the OpenAI Responses shape internally and are translated per call;
    /// the response identity is the requested model, never a provider claim.
    ModelReported,
}

impl AgentProviderResponseRoute {
    /// Provider protocol compatible with this exact route.
    pub const fn provider(self) -> AgentProviderKind {
        match self {
            Self::OpenAiDefault | Self::ModelReported => AgentProviderKind::OpenAiResponses,
            Self::AnthropicStandardGlobal => AgentProviderKind::AnthropicMessages,
        }
    }

    /// Billing class fixed by this route.
    pub const fn billing_class(self) -> AgentProviderBillingClass {
        match self {
            Self::OpenAiDefault => AgentProviderBillingClass::OpenAiDefault,
            Self::AnthropicStandardGlobal => AgentProviderBillingClass::AnthropicStandardGlobal,
            Self::ModelReported => AgentProviderBillingClass::ModelReported,
        }
    }

    pub(crate) const fn request_service_tier(self) -> &'static str {
        match self {
            Self::OpenAiDefault | Self::ModelReported => OPENAI_STANDARD_SERVICE_TIER,
            Self::AnthropicStandardGlobal => ANTHROPIC_STANDARD_SERVICE_TIER_REQUEST,
        }
    }

    pub(crate) const fn response_service_tier(self) -> &'static str {
        match self {
            Self::OpenAiDefault => OPENAI_STANDARD_SERVICE_TIER,
            Self::AnthropicStandardGlobal => ANTHROPIC_STANDARD_SERVICE_TIER_RESPONSE,
            Self::ModelReported => MODEL_REPORTED_SERVICE_TIER,
        }
    }

    pub(crate) const fn response_inference_geo(self) -> Option<&'static str> {
        match self {
            Self::OpenAiDefault | Self::ModelReported => None,
            Self::AnthropicStandardGlobal => Some(ANTHROPIC_GLOBAL_INFERENCE_GEO),
        }
    }
}

/// Provider-neutral single-owner decoder for one exact streamed model call.
///
/// The selected provider is fixed by `AgentProviderCallConfig`; callers do not
/// branch on vendor wire events and receive only the shared bounded contract.
#[must_use]
#[cfg(feature = "provider-transport")]
pub(crate) struct AgentProviderStreamDecoder {
    inner: AgentProviderStreamDecoderInner,
}

#[cfg(feature = "provider-transport")]
enum AgentProviderStreamDecoderInner {
    OpenAi(Box<OpenAiResponsesStreamDecoder>),
    Anthropic(Box<AnthropicMessagesStreamDecoder>),
}

#[cfg(feature = "provider-transport")]
impl AgentProviderStreamDecoder {
    /// Constructs the exact decoder selected by one admitted call config.
    pub(crate) fn try_new(
        call: AgentProviderCallIdentity,
        config: &AgentProviderCallConfig,
    ) -> Result<Self, AgentProviderProtocolError> {
        let inner = match config.provider() {
            AgentProviderKind::OpenAiResponses => AgentProviderStreamDecoderInner::OpenAi(
                Box::new(OpenAiResponsesStreamDecoder::try_new(call, config)?),
            ),
            AgentProviderKind::AnthropicMessages => AgentProviderStreamDecoderInner::Anthropic(
                Box::new(AnthropicMessagesStreamDecoder::try_new(call, config)?),
            ),
        };
        Ok(Self { inner })
    }

    /// Decodes one arbitrary transport chunk through the selected adapter.
    pub(crate) fn push(
        &mut self,
        bytes: &[u8],
    ) -> Result<AgentProviderStreamBatch, AgentProviderProtocolError> {
        match &mut self.inner {
            AgentProviderStreamDecoderInner::OpenAi(decoder) => decoder.push(bytes),
            AgentProviderStreamDecoderInner::Anthropic(decoder) => decoder.push(bytes),
        }
    }

    /// Requires complete framing and one unambiguous terminal provider event.
    pub(crate) fn finish(self) -> Result<AgentProviderFinishedStream, AgentProviderProtocolError> {
        match self.inner {
            AgentProviderStreamDecoderInner::OpenAi(decoder) => (*decoder).finish(),
            AgentProviderStreamDecoderInner::Anthropic(decoder) => (*decoder).finish(),
        }
    }

    pub(crate) fn protocol_event(&self) -> Option<AgentProviderProtocolEvent> {
        match &self.inner {
            AgentProviderStreamDecoderInner::OpenAi(decoder) => decoder.protocol_event(),
            AgentProviderStreamDecoderInner::Anthropic(_) => None,
        }
    }
}

#[cfg(feature = "provider-transport")]
impl fmt::Debug for AgentProviderStreamDecoder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            AgentProviderStreamDecoderInner::OpenAi(decoder) => {
                formatter.debug_tuple("OpenAi").field(decoder).finish()
            }
            AgentProviderStreamDecoderInner::Anthropic(decoder) => {
                formatter.debug_tuple("Anthropic").field(decoder).finish()
            }
        }
    }
}

/// Bounded exact provider model revision selected before request admission.
#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct AgentProviderModelRevision(String);

impl AgentProviderModelRevision {
    /// Accepts an ASCII model revision rather than a URL, path, or free text.
    pub fn try_new(value: String) -> Result<Self, AgentProviderContractError> {
        if !valid_model_revision(&value) {
            return Err(AgentProviderContractError::ModelRevision);
        }
        Ok(Self(value))
    }

    /// Exact value for a fixed provider request body.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentProviderModelRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderModelRevision")
            .field("bytes", &self.0.len())
            .field("value", &"[redacted]")
            .finish()
    }
}

/// Provider-attested response execution identity for one exact call.
///
/// The requested model remains distinct from the effective model returned by
/// the provider. This value is attestation only: a different effective model
/// is not thereby catalog-approved, tokenizer-compatible, or priceable.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AgentProviderResponseIdentity {
    provider: AgentProviderKind,
    requested_model: [u8; MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES],
    requested_model_len: u8,
    effective_model: [u8; MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES],
    effective_model_len: u8,
    route: AgentProviderResponseRoute,
    reasoning_effort: AgentProviderReasoningEffort,
    guard: [u8; 32],
}

impl AgentProviderResponseIdentity {
    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) fn try_attested(
        config: &AgentProviderCallConfig,
        effective_model: &str,
        effective_service_tier: &str,
        effective_inference_geo: Option<&str>,
    ) -> Result<Self, AgentProviderProtocolError> {
        if !valid_model_revision(effective_model)
            || !config.allows_response_identity(
                effective_model,
                effective_service_tier,
                effective_inference_geo,
            )
        {
            return Err(AgentProviderProtocolError::Event);
        }
        let (requested_model, requested_model_len) =
            copy_identity::<MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES>(config.model().as_str())
                .ok_or(AgentProviderProtocolError::Event)?;
        let (effective_model, effective_model_len) =
            copy_identity::<MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES>(effective_model)
                .ok_or(AgentProviderProtocolError::Event)?;
        let provider = config.provider();
        let route = config.response_route();
        let reasoning_effort = config.reasoning_effort();
        let guard = response_identity_guard(
            provider,
            config.model().as_str(),
            bounded_identity_str(&effective_model, effective_model_len),
            route,
            reasoning_effort,
        );
        Ok(Self {
            provider,
            requested_model,
            requested_model_len,
            effective_model,
            effective_model_len,
            route,
            reasoning_effort,
            guard,
        })
    }

    /// Provider protocol that attested this response identity.
    pub const fn provider(self) -> AgentProviderKind {
        self.provider
    }

    /// Exact bounded model identity sent in the immutable request.
    pub fn requested_model(&self) -> &str {
        bounded_identity_str(&self.requested_model, self.requested_model_len)
    }

    /// Exact bounded model identity attested by the provider response.
    pub fn effective_model(&self) -> &str {
        bounded_identity_str(&self.effective_model, self.effective_model_len)
    }

    /// Exact bounded service tier attested by the provider response.
    pub fn effective_service_tier(&self) -> &str {
        self.route.response_service_tier()
    }

    /// Exact bounded inference geography, when the provider attests one.
    pub const fn effective_inference_geo(self) -> Option<&'static str> {
        self.route.response_inference_geo()
    }

    /// Closed catalog-bound provider response route.
    pub const fn response_route(self) -> AgentProviderResponseRoute {
        self.route
    }

    /// Explicit requested reasoning effort used by this response.
    pub const fn reasoning_effort(self) -> AgentProviderReasoningEffort {
        self.reasoning_effort
    }

    /// Content-free digest of the exact provider-attested execution identity.
    pub const fn guard(self) -> [u8; 32] {
        self.guard
    }

    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) fn matches_attestation(
        self,
        model: &str,
        service_tier: &str,
        inference_geo: Option<&str>,
    ) -> bool {
        self.effective_model() == model
            && self.effective_service_tier() == service_tier
            && self.effective_inference_geo() == inference_geo
    }
}

impl fmt::Debug for AgentProviderResponseIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderResponseIdentity")
            .field("provider", &self.provider)
            .field("requested_model_bytes", &self.requested_model_len)
            .field("effective_model_bytes", &self.effective_model_len)
            .field("response_route", &self.route)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("guard", &"[redacted]")
            .field("identity", &"[redacted]")
            .finish()
    }
}

#[cfg(any(test, feature = "provider-transport"))]
fn response_identity_guard(
    provider: AgentProviderKind,
    requested_model: &str,
    effective_model: &str,
    route: AgentProviderResponseRoute,
    reasoning_effort: AgentProviderReasoningEffort,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ZEPHIUM-AGENT-PROVIDER-RESPONSE-IDENTITY-1\0");
    hasher.update([match provider {
        AgentProviderKind::OpenAiResponses => 1,
        AgentProviderKind::AnthropicMessages => 2,
    }]);
    hasher.update([match route {
        AgentProviderResponseRoute::OpenAiDefault => 1,
        AgentProviderResponseRoute::AnthropicStandardGlobal => 2,
        AgentProviderResponseRoute::ModelReported => 3,
    }]);
    hasher.update([match reasoning_effort {
        AgentProviderReasoningEffort::None => 1,
        AgentProviderReasoningEffort::Low => 2,
        AgentProviderReasoningEffort::Medium => 3,
        AgentProviderReasoningEffort::High => 4,
        AgentProviderReasoningEffort::XHigh => 5,
        AgentProviderReasoningEffort::Max => 6,
    }]);
    hasher.update([requested_model.len() as u8]);
    hasher.update(requested_model.as_bytes());
    hasher.update([effective_model.len() as u8]);
    hasher.update(effective_model.as_bytes());
    hasher.finalize().into()
}

fn valid_model_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_AGENT_PROVIDER_MODEL_REVISION_BYTES
        && !value.contains("://")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

#[cfg(any(test, feature = "provider-transport"))]
fn copy_identity<const N: usize>(value: &str) -> Option<([u8; N], u8)> {
    let len = u8::try_from(value.len()).ok()?;
    if value.len() > N {
        return None;
    }
    let mut bytes = [0_u8; N];
    bytes[..value.len()].copy_from_slice(value.as_bytes());
    Some((bytes, len))
}

fn bounded_identity_str(bytes: &[u8], len: u8) -> &str {
    std::str::from_utf8(&bytes[..usize::from(len)]).unwrap_or_default()
}

/// Content-free correlation copied from one exact policy authority.
///
/// This value is intentionally cloneable for callbacks and metrics. It omits
/// the policy admission guard and therefore cannot commit input, spend a
/// reservation, authorize a retry, or settle a call.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AgentProviderCallIdentity {
    manifest: AgentRunManifestId,
    manifest_guard: [u8; 32],
    call: AgentModelCallId,
    lease: AgentPlanLeaseId,
    node: AgentPlanNodeId,
}

impl AgentProviderCallIdentity {
    /// A correlation joined to no run, for probe replays outside any policy.
    #[cfg(feature = "probe-harness")]
    pub(crate) fn probe_fixture() -> Self {
        Self {
            manifest: crate::AgentRunManifestId::generate(),
            manifest_guard: [0; 32],
            call: AgentModelCallId::new(1).expect("nonzero call"),
            lease: crate::AgentPlanLeaseId::generate(),
            node: crate::AgentPlanNodeId::generate(),
        }
    }

    /// Projects non-authorizing correlation from a prepared policy admission.
    pub(crate) fn from_admission(admission: &AgentModelCallAdmission) -> Self {
        Self {
            manifest: admission.manifest(),
            manifest_guard: admission.manifest_guard_for_provider(),
            call: admission.id(),
            lease: admission.lease(),
            node: admission.node(),
        }
    }

    /// Projects non-authorizing correlation from a committed model call.
    pub(crate) fn from_active(active: &AgentActiveModelCall) -> Self {
        Self {
            manifest: active.manifest(),
            manifest_guard: active.manifest_guard_for_metrics(),
            call: active.id(),
            lease: active.lease(),
            node: active.node(),
        }
    }

    /// Exact immutable run-manifest revision.
    pub const fn manifest(self) -> AgentRunManifestId {
        self.manifest
    }

    /// Exact monotonic model-call identity.
    pub const fn call(self) -> AgentModelCallId {
        self.call
    }

    /// Exact mutable plan lease reserving the call.
    pub const fn lease(self) -> AgentPlanLeaseId {
        self.lease
    }

    /// Exact responsible plan node.
    pub const fn node(self) -> AgentPlanNodeId {
        self.node
    }

    /// Checks this non-authorizing correlation against one exact private
    /// manifest revision without exposing the revision guard to callers.
    pub(crate) fn matches_manifest_revision(
        self,
        manifest: AgentRunManifestId,
        manifest_guard: [u8; 32],
    ) -> bool {
        self.manifest == manifest && self.manifest_guard == manifest_guard
    }

    pub(crate) const fn manifest_guard_for_continuation(self) -> [u8; 32] {
        self.manifest_guard
    }

    /// Whether this correlation still names the supplied committed authority.
    #[cfg(any(feature = "provider-transport", test))]
    pub(crate) fn matches_active(self, active: &AgentActiveModelCall) -> bool {
        self == Self::from_active(active)
    }
}

impl fmt::Debug for AgentProviderCallIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderCallIdentity")
            .field("manifest", &self.manifest)
            .field("manifest_guard", &"[redacted]")
            .field("call", &self.call)
            .field("lease", &self.lease)
            .field("node", &self.node)
            .finish()
    }
}

/// Per-call stream ceilings, always bounded by process-wide hard limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderStreamBudget {
    max_wire_bytes: u32,
    max_events: u32,
    max_output_text_bytes: u32,
    max_tool_calls: u8,
    max_tool_argument_bytes: u32,
}

impl AgentProviderStreamBudget {
    /// Conservative default for one browser-agent model turn.
    pub const STANDARD: Self = Self {
        max_wire_bytes: 1024 * 1024,
        max_events: 2_048,
        max_output_text_bytes: 32 * 1024,
        max_tool_calls: MAX_AGENT_PROVIDER_TOOL_CALLS,
        max_tool_argument_bytes: 16 * 1024,
    };

    /// Validates nonzero call ceilings under every process-wide maximum.
    pub const fn try_new(
        max_wire_bytes: u32,
        max_events: u32,
        max_output_text_bytes: u32,
        max_tool_calls: u8,
        max_tool_argument_bytes: u32,
    ) -> Result<Self, AgentProviderContractError> {
        if max_wire_bytes == 0
            || max_wire_bytes > MAX_AGENT_PROVIDER_STREAM_WIRE_BYTES
            || max_events == 0
            || max_events > MAX_AGENT_PROVIDER_STREAM_EVENTS
            || max_output_text_bytes == 0
            || max_output_text_bytes > MAX_AGENT_PROVIDER_OUTPUT_TEXT_BYTES
            || max_tool_calls == 0
            || max_tool_calls > MAX_AGENT_PROVIDER_TOOL_CALLS
            || max_tool_argument_bytes == 0
            || max_tool_argument_bytes > MAX_AGENT_PROVIDER_TOOL_ARGUMENT_BYTES
        {
            return Err(AgentProviderContractError::StreamBudget);
        }
        Ok(Self {
            max_wire_bytes,
            max_events,
            max_output_text_bytes,
            max_tool_calls,
            max_tool_argument_bytes,
        })
    }

    /// Maximum aggregate provider response-body bytes.
    pub const fn max_wire_bytes(self) -> u32 {
        self.max_wire_bytes
    }

    /// Maximum accepted SSE events, including pings and ignored future events.
    pub const fn max_events(self) -> u32 {
        self.max_events
    }

    /// Maximum aggregate model-authored plain-text bytes.
    pub const fn max_output_text_bytes(self) -> u32 {
        self.max_output_text_bytes
    }

    /// Maximum complete client tool calls.
    pub const fn max_tool_calls(self) -> u8 {
        self.max_tool_calls
    }

    /// Maximum aggregate incomplete and complete tool-argument bytes.
    pub const fn max_tool_argument_bytes(self) -> u32 {
        self.max_tool_argument_bytes
    }
}

/// Fixed provider, pricing identity, and response bounds for one admitted call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderInputAccountingMode {
    /// A pinned local counter supplies exact semantic counts and this exact
    /// non-payload envelope count before policy admission.
    ExactLocal {
        /// Exact pinned envelope/tool-schema tokens beyond semantic content.
        fixed_input_tokens: u32,
    },
    /// The full serialized OpenAI request is first reserved using UTF-8 bytes,
    /// then replaced by an authenticated `/responses/input_tokens` count before
    /// model generation.
    ProviderExactAfterConservativeReservation,
    /// The person's chosen model. The complete request is reserved before
    /// dispatch as a strict upper bound: its UTF-8 bytes without image data,
    /// plus a fixed envelope allowance and a fixed allowance per image. The
    /// call then settles on the usage its provider reports. This is never
    /// exact admission: reported usage above the reservation is charged the
    /// reservation and fails the call.
    ProviderReportedAfterConservativeReservation,
}

impl AgentProviderInputAccountingMode {
    const fn fixed_input_tokens(self) -> Option<u32> {
        match self {
            Self::ExactLocal { fixed_input_tokens } => Some(fixed_input_tokens),
            Self::ProviderExactAfterConservativeReservation
            | Self::ProviderReportedAfterConservativeReservation => None,
        }
    }

    /// Whether admission reserves the whole serialized request rather than a
    /// locally counted payload and fixed envelope.
    pub const fn reserves_whole_request(self) -> bool {
        matches!(
            self,
            Self::ProviderExactAfterConservativeReservation
                | Self::ProviderReportedAfterConservativeReservation
        )
    }
}

/// Fixed provider, accounting mode, pricing identity, and response bounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentProviderCallConfig {
    catalog: Arc<AgentProviderCatalogBinding>,
    input_accounting: AgentProviderInputAccountingMode,
    max_output_tokens: u32,
    stream: AgentProviderStreamBudget,
    store_response: bool,
    tools: BrowserToolProfile,
    baseline_read: bool,
    compound_form: bool,
    progressive_observation: bool,
    viewport_screenshot: bool,
    standalone_wait: bool,
    human_request: bool,
    navigation_available: bool,
    history_back: bool,
    history_back_available: bool,
    snapshot_withheld: WithheldSnapshot,
    decision_budget: Option<(AgentModelCallId, u8)>,
}

/// Per-turn narrowing of the vocabulary. It only removes `snapshot`, so it
/// never changes whether two configs bind the same lineage.
#[derive(Clone, Copy, Debug, Default)]
struct WithheldSnapshot(bool);

impl PartialEq for WithheldSnapshot {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for WithheldSnapshot {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BrowserToolProfile {
    Full,
    LocateAct,
    Extraction,
    LocateActExtraction,
    ScopedExtraction,
    LocateActScopedExtraction,
    NavigationExtraction,
    NavigationActionsExtraction,
    SiteActionsExtraction,
}

impl AgentProviderCallConfig {
    pub(in crate::agent_provider) fn from_catalog(
        catalog: Arc<AgentProviderCatalogBinding>,
        input_accounting: AgentProviderInputAccountingMode,
        max_output_tokens: u32,
        stream: AgentProviderStreamBudget,
    ) -> Result<Self, AgentProviderContractError> {
        match input_accounting {
            AgentProviderInputAccountingMode::ExactLocal { fixed_input_tokens }
                if fixed_input_tokens == 0
                    || u64::from(fixed_input_tokens) > MAX_AGENT_RUN_MODEL_TOKENS =>
            {
                return Err(AgentProviderContractError::InputTokens);
            }
            AgentProviderInputAccountingMode::ProviderExactAfterConservativeReservation
                if catalog.provider() != AgentProviderKind::OpenAiResponses
                    || catalog.response_route() != AgentProviderResponseRoute::OpenAiDefault
                    || catalog.pricing_profile().max_input_tokens()
                        > pricing::MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS =>
            {
                return Err(AgentProviderContractError::InputAccountingMode);
            }
            AgentProviderInputAccountingMode::ProviderReportedAfterConservativeReservation
                if catalog.response_route() != AgentProviderResponseRoute::ModelReported
                    || catalog.pricing_profile().max_input_tokens()
                        > pricing::MAX_AGENT_PROVIDER_REPORTED_INPUT_TOKENS =>
            {
                return Err(AgentProviderContractError::InputAccountingMode);
            }
            AgentProviderInputAccountingMode::ExactLocal { .. }
                if catalog.response_route() == AgentProviderResponseRoute::ModelReported =>
            {
                return Err(AgentProviderContractError::InputAccountingMode);
            }
            AgentProviderInputAccountingMode::ExactLocal { .. }
            | AgentProviderInputAccountingMode::ProviderExactAfterConservativeReservation
            | AgentProviderInputAccountingMode::ProviderReportedAfterConservativeReservation => {}
        }
        if max_output_tokens == 0 || u64::from(max_output_tokens) > MAX_AGENT_RUN_MODEL_TOKENS {
            return Err(AgentProviderContractError::OutputTokens);
        }
        Ok(Self {
            catalog,
            input_accounting,
            max_output_tokens,
            stream,
            store_response: false,
            tools: BrowserToolProfile::Full,
            baseline_read: false,
            compound_form: false,
            progressive_observation: false,
            viewport_screenshot: false,
            standalone_wait: false,
            human_request: false,
            navigation_available: true,
            history_back: false,
            history_back_available: false,
            snapshot_withheld: WithheldSnapshot(false),
            decision_budget: None,
        })
    }

    /// Restricts advertised capabilities to locate and one snapshot-verifiable
    /// action per turn, with immediate or mutation-quiet settlement only.
    ///
    /// This trusted host setting is bound into continuation configuration and
    /// applies equally to every provider protocol and request turn.
    pub fn restrict_to_locate_and_act(mut self) -> Self {
        self.tools = BrowserToolProfile::LocateAct;
        self
    }

    /// Enables the bounded host form executor, without granting any native effect.
    pub fn with_compound_form(mut self) -> Self {
        self.compound_form = true;
        self
    }

    /// Enables bounded, nonterminal reads of the already acknowledged initial
    /// observation. This frozen capability grants no recapture, new refs or
    /// action authority and adds no turns to the shared transcript ceiling.
    pub fn with_baseline_read(mut self) -> Self {
        self.baseline_read = true;
        self
    }

    /// Whether the host explicitly enabled nonterminal initial-baseline reads.
    pub const fn permits_baseline_read(&self) -> bool {
        self.baseline_read
    }

    pub(super) fn adds_baseline_read(&self) -> bool {
        self.baseline_read && self.tools != BrowserToolProfile::Full
    }

    /// Enables closed, reference-bound native inspection of the current page.
    /// This freezes tool availability only; each request still needs exact
    /// acknowledged predecessor, native scope, account and provider admission.
    pub fn with_progressive_observation(mut self) -> Self {
        self.progressive_observation = true;
        self
    }

    /// Whether this fixed provider lineage permits progressive inspection.
    pub const fn permits_progressive_observation(&self) -> bool {
        self.progressive_observation
    }

    pub(super) fn adds_progressive_observation(&self) -> bool {
        self.progressive_observation
            && !self.snapshot_withheld.0
            && self.tools != BrowserToolProfile::Full
    }

    /// Stops offering `snapshot` on the next turns, or offers it again. A
    /// planner that only looked turn after turn is left with tools that move
    /// the task on.
    pub fn with_snapshot_withheld(mut self, withheld: bool) -> Self {
        self.snapshot_withheld = WithheldSnapshot(withheld);
        self
    }

    /// Enables one bounded viewport capture of the exact acknowledged semantic
    /// observation. This only freezes model vocabulary; task policy, native
    /// platform support, secret screening and current-document admission remain
    /// independently mandatory for every capture.
    pub fn with_viewport_screenshot(mut self) -> Self {
        self.viewport_screenshot = true;
        self
    }

    /// Whether this immutable provider lineage can request a viewport capture.
    pub const fn permits_viewport_screenshot(&self) -> bool {
        self.viewport_screenshot
    }

    pub(super) fn adds_viewport_screenshot(&self) -> bool {
        self.viewport_screenshot && self.tools != BrowserToolProfile::Full
    }

    /// Enables only proof-carrying semantic-change and exact target-state waits.
    /// The host remains responsible for one absolute deadline and fresh native
    /// observations; this setting grants no timer or action authority.
    pub fn with_standalone_wait(mut self) -> Self {
        self.standalone_wait = true;
        self
    }

    /// Whether this immutable provider lineage permits a bounded semantic wait.
    pub const fn permits_standalone_wait(&self) -> bool {
        self.standalone_wait
    }

    pub(super) fn adds_standalone_wait(&self) -> bool {
        self.standalone_wait && self.tools != BrowserToolProfile::Full
    }

    /// Enables model-requested human handoff with one closed reason. This never
    /// enables `resume_after_human`; only a trusted host can start a successor.
    pub fn with_human_request(mut self) -> Self {
        self.human_request = true;
        self
    }

    /// Whether this immutable lineage lets the model stop and request a person.
    pub const fn permits_human_request(&self) -> bool {
        self.human_request
    }

    pub(super) fn adds_human_request(&self) -> bool {
        self.human_request && self.tools != BrowserToolProfile::Full
    }

    /// Restricts browser planning to the one trusted run-local extraction
    /// schema (identity 1), using only the current initial observation.
    /// This enables no native action, navigation or scope expansion.
    pub fn restrict_to_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::Extraction;
        self
    }

    /// Advertises only locate, one task-authorized exact navigation and terminal
    /// initial-scope extraction. This grants no native action or destination authority.
    pub fn restrict_to_navigation_and_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::NavigationExtraction;
        self
    }

    /// Enables model-selected navigation, one snapshot-verifiable Click, Fill
    /// or Select action per turn, and terminal mapping. Only Read/LocalWrite
    /// effects and immediate/mutation-quiet settlement are advertised inside
    /// one frozen lineage. Trusted host assessment, exact current refs,
    /// policy admission and independent verification remain mandatory per effect.
    pub fn restrict_to_navigation_actions_and_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::NavigationActionsExtraction;
        self
    }

    /// The same tools for work on one site in the person's session, where a
    /// click may also declare a commitment. Declaring one grants nothing: the
    /// trusted host classifies every step and holds commitments for the person.
    pub fn restrict_to_site_actions_and_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::SiteActionsExtraction;
        self
    }

    /// Freezes the host's total model-call allowance for one bounded workflow.
    /// Call identities advance once per attempt; the last call is reserved for
    /// terminal mapping. This only narrows decisions, never policy budgets or
    /// the task's independent extraction, action, or navigation preconditions.
    pub fn with_decision_budget(
        mut self,
        first_call: AgentModelCallId,
        max_calls: u8,
    ) -> Result<Self, AgentProviderContractError> {
        if self.provider() != AgentProviderKind::OpenAiResponses
            || !self.input_accounting.reserves_whole_request()
        {
            return Err(AgentProviderContractError::InputAccountingMode);
        }
        if max_calls < 2
            || first_call
                .get()
                .checked_add(u64::from(max_calls) - 1)
                .is_none()
        {
            return Err(AgentProviderContractError::AdmissionBudget);
        }
        self.decision_budget = Some((first_call, max_calls));
        Ok(self)
    }

    pub(super) fn remaining_decision_calls(
        &self,
        call: AgentModelCallId,
    ) -> Result<Option<u8>, AgentProviderContractError> {
        let Some((first, max_calls)) = self.decision_budget else {
            return Ok(None);
        };
        let remaining = call
            .get()
            .checked_sub(first.get())
            .and_then(|used| u64::from(max_calls).checked_sub(used))
            .and_then(|remaining| remaining.checked_sub(1))
            .filter(|remaining| *remaining > 0)
            .and_then(|remaining| u8::try_from(remaining).ok())
            .ok_or(AgentProviderContractError::AdmissionBudget)?;
        Ok(Some(remaining))
    }

    /// Restricts a trusted combined task to snapshot actions, bounded locate
    /// and schema-1 extraction from the current initial observation. The host
    /// must independently gate extraction on its fresh task postcondition.
    /// This profile never changes within a continuation lineage.
    pub fn restrict_to_actions_and_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::LocateActExtraction;
        self
    }

    /// Enables bounded locate and schema-1 extraction from either the exact
    /// current baseline or one freshly captured, acknowledged-ref subtree.
    /// No native action, navigation or standalone snapshot is enabled.
    pub fn restrict_to_scoped_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::ScopedExtraction;
        self
    }

    /// Enables the same verified snapshot actions and terminal scoped read.
    /// The trusted host must gate both task readiness and scope admission.
    pub fn restrict_to_actions_and_scoped_extraction(mut self) -> Self {
        self.tools = BrowserToolProfile::LocateActScopedExtraction;
        self
    }

    pub(super) fn permits_subtree_extraction(&self) -> bool {
        matches!(
            self.tools,
            BrowserToolProfile::Full
                | BrowserToolProfile::ScopedExtraction
                | BrowserToolProfile::LocateActScopedExtraction
        )
    }

    /// Reports whether this immutable request profile advertises a tool kind.
    pub fn permits_tool(&self, kind: AgentBrowserToolKind) -> bool {
        if kind == AgentBrowserToolKind::FillForm {
            return self.compound_form && self.permits_tool(AgentBrowserToolKind::Act);
        }
        if matches!(
            kind,
            AgentBrowserToolKind::Navigate | AgentBrowserToolKind::Back
        ) && !self.navigation_available
        {
            return false;
        }
        if self.baseline_read && kind == AgentBrowserToolKind::Read {
            return true;
        }
        if self.progressive_observation && kind == AgentBrowserToolKind::Snapshot {
            return !self.snapshot_withheld.0;
        }
        if self.viewport_screenshot && kind == AgentBrowserToolKind::Screenshot {
            return true;
        }
        if self.standalone_wait && kind == AgentBrowserToolKind::Wait {
            return true;
        }
        if self.human_request && kind == AgentBrowserToolKind::ShowForHuman {
            return true;
        }
        if kind == AgentBrowserToolKind::Back {
            return self.history_back && self.history_back_available;
        }
        match self.tools {
            BrowserToolProfile::Full => true,
            BrowserToolProfile::Extraction => kind == AgentBrowserToolKind::Extract,
            BrowserToolProfile::NavigationExtraction => matches!(
                kind,
                AgentBrowserToolKind::Locate
                    | AgentBrowserToolKind::Navigate
                    | AgentBrowserToolKind::Extract
            ),
            BrowserToolProfile::NavigationActionsExtraction
            | BrowserToolProfile::SiteActionsExtraction => matches!(
                kind,
                AgentBrowserToolKind::Locate
                    | AgentBrowserToolKind::Navigate
                    | AgentBrowserToolKind::Act
                    | AgentBrowserToolKind::Extract
            ),
            BrowserToolProfile::ScopedExtraction => matches!(
                kind,
                AgentBrowserToolKind::Locate | AgentBrowserToolKind::Extract
            ),
            BrowserToolProfile::LocateAct => matches!(
                kind,
                AgentBrowserToolKind::Locate | AgentBrowserToolKind::Act
            ),
            BrowserToolProfile::LocateActExtraction
            | BrowserToolProfile::LocateActScopedExtraction => matches!(
                kind,
                AgentBrowserToolKind::Locate
                    | AgentBrowserToolKind::Act
                    | AgentBrowserToolKind::Extract
            ),
        }
    }

    /// Enables one-step native history proposals. Policy and the native adapter
    /// still require an exact run-enrolled predecessor for every invocation.
    pub fn with_history_back(mut self) -> Self {
        self.history_back = true;
        self
    }

    /// Projects current-phase Back availability without changing whether the
    /// host supports the primitive. This may only be enabled after a committed
    /// run-local predecessor exists and must be cleared again at history root.
    pub fn with_history_back_available(mut self, available: bool) -> Self {
        self.history_back_available = self.history_back && available;
        self
    }

    /// Projects whether another navigation effect can still be accepted by
    /// the trusted route/discovery state. This removes both forward and Back
    /// vocabulary; it grants no destination or navigation authority.
    pub fn with_navigation_available(mut self, available: bool) -> Self {
        self.navigation_available = available;
        self
    }

    /// Enables provider-side response retention for an inspectable public-data probe.
    ///
    /// This switch does not exist in release builds. Production and ordinary
    /// BYOK calls remain unconditionally stateless; the diagnostic harness must
    /// opt in explicitly before it serializes any request.
    #[cfg(feature = "probe-harness")]
    pub fn retain_response_for_inspectable_probe(mut self) -> Self {
        self.store_response = true;
        self
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_for_test(
        provider: AgentProviderKind,
        model: AgentProviderModelRevision,
        reasoning_effort: AgentProviderReasoningEffort,
        tokenizer: SemanticTokenizerRevision,
        pricing_profile: AgentProviderPricingProfile,
        fixed_input_tokens: u32,
        max_output_tokens: u32,
        stream: AgentProviderStreamBudget,
    ) -> Result<Self, AgentProviderContractError> {
        if provider == AgentProviderKind::AnthropicMessages
            && reasoning_effort != AgentProviderReasoningEffort::None
        {
            return Err(AgentProviderContractError::ReasoningEffort);
        }
        let rates = AgentProviderTokenRates::try_new(1, 1, 1, 1)
            .expect("fixed test rates must remain valid");
        let schedule = AgentProviderPricingSchedule::try_for_test(
            provider,
            model,
            reasoning_effort,
            tokenizer,
            pricing_profile,
            rates,
        )
        .expect("validated test catalog must remain constructible");
        schedule.try_call_config(fixed_input_tokens, max_output_tokens, stream)
    }

    /// Selected provider protocol.
    pub fn provider(&self) -> AgentProviderKind {
        self.catalog.provider()
    }

    #[cfg(feature = "provider-transport")]
    pub(crate) fn planning_cost_ceiling(&self, input: u32, output: u32) -> Option<u64> {
        self.cost_ceiling(u64::from(input), output)
    }

    /// Strict ceiling for `input` and `output` tokens: every input token at
    /// the highest input rate, rounded up once.
    pub(crate) fn cost_ceiling(&self, input: u64, output: u32) -> Option<u64> {
        let rates = self.catalog.rates();
        let input_rate = rates
            .uncached_input()
            .max(rates.cached_input())
            .max(rates.cache_write_input());
        let numerator = u128::from(input)
            .checked_mul(u128::from(input_rate))?
            .checked_add(u128::from(output) * u128::from(rates.output()))?;
        u64::try_from(numerator.div_ceil(1_000_000)).ok()
    }
    #[cfg(feature = "provider-transport")]
    pub(crate) fn planning_identity_matches(&self, model: &str, tier: &str) -> bool {
        self.catalog.allows_response_identity(model, tier, None)
    }

    /// Exact billing mode encoded into and required from the provider call.
    pub fn billing_class(&self) -> AgentProviderBillingClass {
        self.catalog.response_route().billing_class()
    }

    /// Exact selected provider model revision.
    pub fn model(&self) -> &AgentProviderModelRevision {
        self.catalog.requested_model()
    }

    /// Explicit reasoning effort encoded for this exact call.
    pub fn reasoning_effort(&self) -> AgentProviderReasoningEffort {
        self.catalog.reasoning_effort()
    }

    /// Exact tokenizer/counting revision used for input admission.
    pub fn tokenizer(&self) -> &SemanticTokenizerRevision {
        self.catalog.tokenizer()
    }

    /// Exact trusted catalog revision and inclusive input pricing range.
    pub fn pricing_profile(&self) -> AgentProviderPricingProfile {
        self.catalog.pricing_profile()
    }

    /// Exact provider response route bound by the trusted catalog.
    pub fn response_route(&self) -> AgentProviderResponseRoute {
        self.catalog.response_route()
    }

    /// Digest of the complete trusted execution and pricing catalog entry.
    pub fn accounting_guard(&self) -> [u8; 32] {
        self.catalog.accounting_guard()
    }

    /// Pre-dispatch input-accounting contract selected by the trusted schedule.
    pub const fn input_accounting_mode(&self) -> AgentProviderInputAccountingMode {
        self.input_accounting
    }

    /// Hard requested provider output-token ceiling.
    pub const fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }

    /// Hard per-call stream ceilings.
    pub const fn stream_budget(&self) -> AgentProviderStreamBudget {
        self.stream
    }

    pub(crate) const fn stores_response(&self) -> bool {
        self.store_response
    }

    pub(crate) fn validate_request(
        &self,
        request: AgentModelCallRequest,
        payload: &SemanticTokenMeasurement,
        objective: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if payload.revision() != self.tokenizer() || objective.revision() != self.tokenizer() {
            return Err(AgentProviderContractError::TokenizerRevision);
        }
        let Some(fixed_input_tokens) = self.input_accounting.fixed_input_tokens() else {
            return Err(AgentProviderContractError::InputAccountingMode);
        };
        if !matches!(
            payload.quality(),
            crate::SemanticTokenCountQuality::ExactLocal
                | crate::SemanticTokenCountQuality::ProviderExact
        ) || !matches!(
            objective.quality(),
            crate::SemanticTokenCountQuality::ExactLocal
                | crate::SemanticTokenCountQuality::ProviderExact
        ) {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        let additional_input_tokens = u64::from(objective.tokens())
            .checked_add(u64::from(fixed_input_tokens))
            .ok_or(AgentProviderContractError::AdmissionBudget)?;
        let total_input_tokens = u64::from(payload.tokens())
            .checked_add(additional_input_tokens)
            .ok_or(AgentProviderContractError::AdmissionBudget)?;
        if additional_input_tokens > u64::from(request.budget().additional_input_tokens())
            || u64::from(self.max_output_tokens) > u64::from(request.budget().output_tokens())
            || !self.pricing_profile().contains(total_input_tokens)
        {
            return Err(AgentProviderContractError::AdmissionBudget);
        }
        Ok(())
    }

    pub(crate) fn validate_diff_request(
        &self,
        request: AgentModelCallRequest,
        diff: &SemanticTokenMeasurement,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if diff.revision() != self.tokenizer() || structured_input.revision() != self.tokenizer() {
            return Err(AgentProviderContractError::TokenizerRevision);
        }
        let Some(fixed_input_tokens) = self.input_accounting.fixed_input_tokens() else {
            return Err(AgentProviderContractError::InputAccountingMode);
        };
        if !matches!(
            diff.quality(),
            crate::SemanticTokenCountQuality::ExactLocal
                | crate::SemanticTokenCountQuality::ProviderExact
        ) {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        if structured_input.quality() != crate::SemanticTokenCountQuality::ExactLocal {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        let allowed_input_tokens = u64::from(diff.tokens())
            .checked_add(u64::from(request.budget().additional_input_tokens()))
            .ok_or(AgentProviderContractError::AdmissionBudget)?;
        if u64::from(fixed_input_tokens) > u64::from(request.budget().additional_input_tokens())
            || u64::from(structured_input.tokens()) > allowed_input_tokens
            || u64::from(self.max_output_tokens) > u64::from(request.budget().output_tokens())
            || !self
                .pricing_profile()
                .contains(u64::from(structured_input.tokens()))
        {
            return Err(AgentProviderContractError::AdmissionBudget);
        }
        Ok(())
    }

    pub(crate) fn validate_provider_exact_initial_request(
        &self,
        request: AgentModelCallRequest,
        payload: &SemanticTokenMeasurement,
        objective: &SemanticTokenMeasurement,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if payload.quality() != crate::SemanticTokenCountQuality::Conservative
            || objective.quality() != crate::SemanticTokenCountQuality::Conservative
        {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        self.validate_provider_exact_structured_request(
            request,
            [Some(payload), Some(objective)],
            structured_input,
        )
    }

    pub(crate) fn validate_provider_exact_continuation_request(
        &self,
        request: AgentModelCallRequest,
        newest_semantic: Option<&SemanticTokenMeasurement>,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        self.validate_provider_exact_structured_request(
            request,
            [newest_semantic, None],
            structured_input,
        )
    }

    fn validate_provider_exact_structured_request(
        &self,
        request: AgentModelCallRequest,
        semantic_inputs: [Option<&SemanticTokenMeasurement>; 2],
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if !self.input_accounting.reserves_whole_request()
            || self.provider() != AgentProviderKind::OpenAiResponses
        {
            return Err(AgentProviderContractError::InputAccountingMode);
        }
        if structured_input.revision() != self.tokenizer() {
            return Err(AgentProviderContractError::TokenizerRevision);
        }
        if structured_input.quality() != crate::SemanticTokenCountQuality::Conservative {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        for measurement in semantic_inputs.into_iter().flatten() {
            if measurement.revision() != self.tokenizer() {
                return Err(AgentProviderContractError::TokenizerRevision);
            }
            if measurement.quality() == crate::SemanticTokenCountQuality::ProviderEstimate {
                return Err(AgentProviderContractError::InputTokenQuality);
            }
        }
        let structured_tokens = u64::from(structured_input.tokens());
        if structured_tokens > u64::from(request.budget().additional_input_tokens())
            || u64::from(self.max_output_tokens) > u64::from(request.budget().output_tokens())
            || !self.pricing_profile().contains(structured_tokens)
        {
            return Err(AgentProviderContractError::AdmissionBudget);
        }
        Ok(())
    }

    pub(crate) fn validate_locate_request(
        &self,
        request: AgentModelCallRequest,
        locate: &SemanticTokenMeasurement,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if locate.quality() != crate::SemanticTokenCountQuality::ExactLocal {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        self.validate_diff_request(request, locate, structured_input)
    }

    pub(crate) fn validate_read_continuation_request(
        &self,
        request: AgentModelCallRequest,
        read: &SemanticTokenMeasurement,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if read.quality() != crate::SemanticTokenCountQuality::ExactLocal {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        self.validate_diff_request(request, read, structured_input)
    }

    pub(crate) fn validate_extraction_request(
        &self,
        request: AgentModelCallRequest,
        extraction: &SemanticTokenMeasurement,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if extraction.quality() != crate::SemanticTokenCountQuality::ExactLocal {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        self.validate_diff_request(request, extraction, structured_input)
    }

    pub(crate) fn validate_screenshot_request(
        &self,
        request: AgentModelCallRequest,
        structured_input: &SemanticTokenMeasurement,
    ) -> Result<(), AgentProviderContractError> {
        if structured_input.revision() != self.tokenizer() {
            return Err(AgentProviderContractError::TokenizerRevision);
        }
        if structured_input.quality() != crate::SemanticTokenCountQuality::ExactLocal {
            return Err(AgentProviderContractError::InputTokenQuality);
        }
        let authorized_input_tokens = u64::from(request.budget().additional_input_tokens());
        let Some(fixed_input_tokens) = self.input_accounting.fixed_input_tokens() else {
            return Err(AgentProviderContractError::InputAccountingMode);
        };
        if u64::from(fixed_input_tokens) > authorized_input_tokens
            || u64::from(structured_input.tokens()) > authorized_input_tokens
            || u64::from(self.max_output_tokens) > u64::from(request.budget().output_tokens())
            || !self
                .pricing_profile()
                .contains(u64::from(structured_input.tokens()))
        {
            return Err(AgentProviderContractError::AdmissionBudget);
        }
        Ok(())
    }

    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) fn allows_response_identity(
        &self,
        effective_model: &str,
        service_tier: &str,
        inference_geo: Option<&str>,
    ) -> bool {
        self.catalog
            .allows_response_identity(effective_model, service_tier, inference_geo)
    }
}

/// Normalized provider token accounting from one terminal response.
///
/// `input_tokens` and `output_tokens` are the totals supplied to policy.
/// Cache and reasoning fields are informational subsets after vendor-specific
/// normalization and are never added to those totals a second time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
    reasoning_output_tokens: u64,
}

/// One bounded untrusted pre-EOF plain-text delta from a provider stream.
///
/// This value is never HTML, policy, a browser operation, or trusted UI. A
/// presentation consumer must render it as escaped text and must not persist
/// or log it indiscriminately. It is model-authored progress only: it is not a
/// terminal result, extraction authority, or evidence that a response reached
/// an authenticated transport EOF.
#[derive(Eq, PartialEq)]
pub struct AgentProviderTextDelta(String);

impl AgentProviderTextDelta {
    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    /// Exact untrusted plain text for an explicitly authorized consumer.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Exact UTF-8 byte count for stream accounting.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the provider emitted an empty delta.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for AgentProviderTextDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderTextDelta")
            .field("bytes", &self.0.len())
            .field("content", &"[redacted]")
            .finish()
    }
}

/// One bounded, non-authorizing text batch emitted before transport EOF.
///
/// Browser proposals, provider correlation, continuation state, terminal
/// usage, and extraction authority can never enter this type. Those remain in
/// the move-only finished stream until exact pricing and policy settlement.
#[derive(Eq, PartialEq)]
pub struct AgentProviderStreamBatch {
    call: AgentProviderCallIdentity,
    deltas: Vec<AgentProviderTextDelta>,
}

impl AgentProviderStreamBatch {
    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) fn new(
        call: AgentProviderCallIdentity,
        deltas: Vec<AgentProviderTextDelta>,
    ) -> Self {
        Self { call, deltas }
    }

    /// Exact non-authorizing call correlation.
    pub const fn call(&self) -> AgentProviderCallIdentity {
        self.call
    }

    /// Ordered untrusted, nonterminal text deltas decoded from this chunk.
    pub fn deltas(&self) -> &[AgentProviderTextDelta] {
        &self.deltas
    }

    /// Consumes the batch without copying model-authored text.
    pub fn into_deltas(self) -> Vec<AgentProviderTextDelta> {
        self.deltas
    }
}

impl fmt::Debug for AgentProviderStreamBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderStreamBatch")
            .field("call", &self.call)
            .field("deltas", &self.deltas.len())
            .finish()
    }
}

/// Content-free counters for one terminal provider stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderStreamStats {
    wire_bytes: u32,
    events: u32,
    output_text_bytes: u32,
    tool_calls: u8,
    tool_argument_bytes: u32,
}

impl AgentProviderStreamStats {
    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) const fn new(
        wire_bytes: u32,
        events: u32,
        output_text_bytes: u32,
        tool_calls: u8,
        tool_argument_bytes: u32,
    ) -> Self {
        Self {
            wire_bytes,
            events,
            output_text_bytes,
            tool_calls,
            tool_argument_bytes,
        }
    }

    /// Accepted aggregate provider response-body bytes.
    pub const fn wire_bytes(self) -> u32 {
        self.wire_bytes
    }

    /// Accepted SSE event count, including ignored typed lifecycle events.
    pub const fn events(self) -> u32 {
        self.events
    }

    /// Accepted model-authored plain-text bytes.
    pub const fn output_text_bytes(self) -> u32 {
        self.output_text_bytes
    }

    /// Accepted complete typed client tool calls.
    pub const fn tool_calls(self) -> u8 {
        self.tool_calls
    }

    /// Accepted aggregate incomplete and complete tool-argument bytes.
    pub const fn tool_argument_bytes(self) -> u32 {
        self.tool_argument_bytes
    }
}

/// One normalized successful terminal provider response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderCompletion {
    call: AgentProviderCallIdentity,
    stop: AgentProviderStopReason,
    usage: AgentProviderUsage,
    stats: AgentProviderStreamStats,
    tool_only_output: bool,
    response_identity: Option<AgentProviderResponseIdentity>,
}

impl AgentProviderCompletion {
    #[cfg(test)]
    pub(crate) const fn new(
        call: AgentProviderCallIdentity,
        stop: AgentProviderStopReason,
        usage: AgentProviderUsage,
        stats: AgentProviderStreamStats,
        tool_only_output: bool,
    ) -> Self {
        Self {
            call,
            stop,
            usage,
            stats,
            tool_only_output,
            response_identity: None,
        }
    }

    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) const fn new_with_response_identity(
        call: AgentProviderCallIdentity,
        stop: AgentProviderStopReason,
        usage: AgentProviderUsage,
        stats: AgentProviderStreamStats,
        tool_only_output: bool,
        response_identity: AgentProviderResponseIdentity,
    ) -> Self {
        Self {
            call,
            stop,
            usage,
            stats,
            tool_only_output,
            response_identity: Some(response_identity),
        }
    }

    /// Exact non-authorizing call correlation.
    pub const fn call(self) -> AgentProviderCallIdentity {
        self.call
    }

    /// Normalized successful stop class.
    pub const fn stop(self) -> AgentProviderStopReason {
        self.stop
    }

    /// Exact normalized provider token counters.
    pub const fn usage(self) -> AgentProviderUsage {
        self.usage
    }

    /// Content-free bounded stream counters.
    pub const fn stats(self) -> AgentProviderStreamStats {
        self.stats
    }

    /// Whether the assistant output contained only decoded client tool calls.
    ///
    /// Text, refusal, and provider-owned tool blocks make this false. Bounded
    /// encrypted OpenAI reasoning replay remains opaque inside the correlation
    /// and therefore does not make an otherwise tool-only response mixed.
    pub const fn tool_only_output(self) -> bool {
        self.tool_only_output
    }

    /// Provider-attested response identity when supplied by the adapter.
    ///
    /// A differing effective model remains unpriced until a trusted catalog
    /// explicitly joins it to the request's tokenizer and pricing identity.
    pub const fn response_identity(self) -> Option<AgentProviderResponseIdentity> {
        self.response_identity
    }
}

/// One normalized terminal provider failure after a stream began.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderTerminalFailure {
    call: AgentProviderCallIdentity,
    failure: AgentProviderFailure,
    usage: Option<AgentProviderUsage>,
    stats: AgentProviderStreamStats,
    response_identity: Option<AgentProviderResponseIdentity>,
}

impl AgentProviderTerminalFailure {
    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) const fn new(
        call: AgentProviderCallIdentity,
        failure: AgentProviderFailure,
        usage: Option<AgentProviderUsage>,
        stats: AgentProviderStreamStats,
    ) -> Self {
        Self {
            call,
            failure,
            usage,
            stats,
            response_identity: None,
        }
    }

    #[cfg(any(test, feature = "provider-transport"))]
    pub(crate) const fn new_with_response_identity(
        call: AgentProviderCallIdentity,
        failure: AgentProviderFailure,
        usage: Option<AgentProviderUsage>,
        stats: AgentProviderStreamStats,
        response_identity: AgentProviderResponseIdentity,
    ) -> Self {
        Self {
            call,
            failure,
            usage,
            stats,
            response_identity: Some(response_identity),
        }
    }

    /// Exact non-authorizing call correlation.
    pub const fn call(self) -> AgentProviderCallIdentity {
        self.call
    }

    /// Closed content-free provider failure.
    pub const fn failure(self) -> AgentProviderFailure {
        self.failure
    }

    /// Provider usage when the terminal event supplied valid accounting.
    pub const fn usage(self) -> Option<AgentProviderUsage> {
        self.usage
    }

    /// Content-free bounded stream counters.
    pub const fn stats(self) -> AgentProviderStreamStats {
        self.stats
    }

    /// Provider-attested response identity established before this failure.
    pub const fn response_identity(self) -> Option<AgentProviderResponseIdentity> {
        self.response_identity
    }
}

/// Sole normalized terminal outcome for one provider stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderStreamConclusion {
    /// Provider returned one valid completed or incomplete response.
    Completed(AgentProviderCompletion),
    /// Provider returned one closed failure or cancellation.
    Failed(AgentProviderTerminalFailure),
}

/// Move-only provider terminal constructed only after complete SSE framing and
/// transport-observed response-body EOF.
///
/// A client tool call, when present, remains private inside this value until
/// trusted catalog pricing and exact policy settlement both succeed.
#[must_use]
pub(crate) struct AgentProviderFinishedStream {
    conclusion: AgentProviderStreamConclusion,
    tool: Option<AgentBrowserToolCall>,
}

impl AgentProviderFinishedStream {
    #[cfg(any(test, feature = "provider-transport"))]
    pub(super) fn new(
        conclusion: AgentProviderStreamConclusion,
        tool: Option<AgentBrowserToolCall>,
    ) -> Result<Self, AgentProviderProtocolError> {
        let (stop, tool_only, tool_calls) = match conclusion {
            AgentProviderStreamConclusion::Completed(completion) => (
                Some(completion.stop()),
                completion.tool_only_output(),
                completion.stats().tool_calls(),
            ),
            AgentProviderStreamConclusion::Failed(failure) => {
                (None, false, failure.stats().tool_calls())
            }
        };
        let valid_tool = matches!(stop, Some(AgentProviderStopReason::ToolCalls))
            && tool_only
            && tool_calls == 1
            && tool.is_some();
        let valid_without_tool = !matches!(stop, Some(AgentProviderStopReason::ToolCalls))
            && !tool_only
            && tool.is_none();
        if !valid_tool && !valid_without_tool {
            return Err(AgentProviderProtocolError::Terminal);
        }
        Ok(Self { conclusion, tool })
    }

    /// Content-free normalized terminal facts. This is not tool authority.
    pub const fn conclusion(&self) -> AgentProviderStreamConclusion {
        self.conclusion
    }

    /// Borrowed content-free terminal facts without copying response identity.
    #[cfg(feature = "provider-transport")]
    pub const fn conclusion_ref(&self) -> &AgentProviderStreamConclusion {
        &self.conclusion
    }

    /// Exact non-authorizing call correlation.
    #[cfg(feature = "provider-transport")]
    pub const fn call(&self) -> AgentProviderCallIdentity {
        match self.conclusion {
            AgentProviderStreamConclusion::Completed(completion) => completion.call(),
            AgentProviderStreamConclusion::Failed(failure) => failure.call(),
        }
    }

    /// Provider-reported usage when an authenticated terminal supplied it.
    #[cfg(feature = "provider-transport")]
    pub const fn usage(&self) -> Option<AgentProviderUsage> {
        match self.conclusion {
            AgentProviderStreamConclusion::Completed(completion) => Some(completion.usage()),
            AgentProviderStreamConclusion::Failed(failure) => failure.usage(),
        }
    }

    /// Provider-attested response identity when established by the adapter.
    #[cfg(feature = "provider-transport")]
    pub const fn response_identity(&self) -> Option<AgentProviderResponseIdentity> {
        match self.conclusion {
            AgentProviderStreamConclusion::Completed(completion) => completion.response_identity(),
            AgentProviderStreamConclusion::Failed(failure) => failure.response_identity(),
        }
    }

    pub(super) fn into_parts(
        self,
    ) -> (AgentProviderStreamConclusion, Option<AgentBrowserToolCall>) {
        (self.conclusion, self.tool)
    }
}

impl fmt::Debug for AgentProviderFinishedStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderFinishedStream")
            .field("conclusion", &self.conclusion)
            .field("has_tool", &self.tool.is_some())
            .field("content", &"[redacted]")
            .finish()
    }
}

impl AgentProviderUsage {
    /// Validates checked totals and subset details without capping real overage.
    pub const fn try_new(
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        cache_write_input_tokens: u64,
        reasoning_output_tokens: u64,
    ) -> Result<Self, AgentProviderContractError> {
        let priced_input_subsets = match cached_input_tokens.checked_add(cache_write_input_tokens) {
            Some(tokens) => tokens,
            None => return Err(AgentProviderContractError::Usage),
        };
        if input_tokens.checked_add(output_tokens).is_none()
            || cached_input_tokens > input_tokens
            || cache_write_input_tokens > input_tokens
            || priced_input_subsets > input_tokens
            || reasoning_output_tokens > output_tokens
        {
            return Err(AgentProviderContractError::Usage);
        }
        Ok(Self {
            input_tokens,
            output_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            reasoning_output_tokens,
        })
    }

    /// Provider-accounted total input tokens for policy settlement.
    pub const fn input_tokens(self) -> u64 {
        self.input_tokens
    }

    /// Provider-accounted total output tokens for policy settlement.
    pub const fn output_tokens(self) -> u64 {
        self.output_tokens
    }

    /// Cached input subset after vendor normalization.
    pub const fn cached_input_tokens(self) -> u64 {
        self.cached_input_tokens
    }

    /// Cache-write input subset after vendor normalization.
    pub const fn cache_write_input_tokens(self) -> u64 {
        self.cache_write_input_tokens
    }

    /// Reasoning-token subset of output, never reasoning content.
    pub const fn reasoning_output_tokens(self) -> u64 {
        self.reasoning_output_tokens
    }

    /// Checked provider-accounted input plus output tokens.
    pub const fn total_tokens(self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Normalized successful provider stop class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderStopReason {
    /// The model completed its turn normally.
    Completed,
    /// The model emitted one or more complete client tool calls.
    ToolCalls,
    /// The provider stopped at the configured output limit.
    OutputLimit,
    /// The model explicitly refused the request.
    Refused,
    /// Provider safety filtering prevented a complete result.
    ContentFiltered,
    /// A caller-selected stop sequence ended generation.
    StopSequence,
    /// The provider paused a resumable turn without completing it.
    Paused,
}

/// Closed terminal provider failure class without provider-authored text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderFailureClass {
    /// Locally invalid fixed request or provider HTTP 400-class refusal.
    InvalidRequest,
    /// Provider authentication was missing or rejected.
    Authentication,
    /// Credential lacks authority for the selected provider resource.
    Permission,
    /// Selected model or provider resource was not found.
    NotFound,
    /// Provider state conflicted with this exact request.
    Conflict,
    /// Provider rate limit rejected this attempt.
    RateLimited,
    /// Provider was temporarily overloaded or unavailable.
    Overloaded,
    /// Exact transport deadline expired.
    Timeout,
    /// Network/TLS/body transport failed after dispatch.
    Transport,
    /// Provider wire data violated the typed protocol contract.
    Protocol,
    /// A trusted transport consumer violated its integration contract.
    Integration,
    /// Provider returned another terminal server-side failure.
    Provider,
    /// Exact run cancellation won the terminal race.
    Cancelled,
}

/// Closed provider event phase retained for content-free protocol diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderProtocolEvent {
    /// OpenAI response creation event.
    OpenAiCreated,
    /// OpenAI response in-progress event.
    OpenAiInProgress,
    /// OpenAI output-item added event.
    OpenAiOutputItemAdded,
    /// OpenAI output-item completed event.
    OpenAiOutputItemDone,
    /// OpenAI content-part lifecycle event.
    OpenAiContentPart,
    /// OpenAI text or refusal delta/done event.
    OpenAiText,
    /// OpenAI function-call argument delta event.
    OpenAiToolArgumentsDelta,
    /// OpenAI function-call argument completion event.
    OpenAiToolArgumentsDone,
    /// OpenAI response terminal event.
    OpenAiTerminal,
    /// OpenAI stream-level error event.
    OpenAiError,
    /// OpenAI `[DONE]` sentinel.
    OpenAiDone,
    /// Unrecognized OpenAI event name.
    OpenAiUnknown,
}

impl AgentProviderFailureClass {
    /// Whether policy may consider a newly admitted retry.
    ///
    /// This never authorizes or schedules a retry. A retry requires a new
    /// supervisor decision, model-call identity, budget reservation, and
    /// cancellation check.
    pub const fn retry_disposition(self) -> AgentProviderRetryDisposition {
        match self {
            Self::RateLimited | Self::Overloaded | Self::Timeout | Self::Transport => {
                AgentProviderRetryDisposition::PolicyMayRetry
            }
            Self::InvalidRequest
            | Self::Authentication
            | Self::Permission
            | Self::NotFound
            | Self::Conflict
            | Self::Protocol
            | Self::Integration
            | Self::Provider
            | Self::Cancelled => AgentProviderRetryDisposition::Never,
        }
    }
}

/// Non-authorizing retry classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentProviderRetryDisposition {
    /// The same failure must not be automatically repeated.
    Never,
    /// Policy may decide to create one newly admitted attempt.
    PolicyMayRetry,
}

/// Bounded provider-requested delay offered only to a future retry policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderRetryAfter(u64);

impl AgentProviderRetryAfter {
    /// Validates a nonzero delay no longer than one day.
    pub const fn try_from_millis(millis: u64) -> Result<Self, AgentProviderContractError> {
        if millis == 0 || millis > MAX_AGENT_PROVIDER_RETRY_AFTER_MILLIS {
            Err(AgentProviderContractError::RetryAfter)
        } else {
            Ok(Self(millis))
        }
    }

    /// Exact bounded provider delay in milliseconds.
    pub const fn millis(self) -> u64 {
        self.0
    }
}

/// Content-free terminal provider failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderFailure {
    class: AgentProviderFailureClass,
    retry_after: Option<AgentProviderRetryAfter>,
    protocol_error: Option<AgentProviderProtocolError>,
    protocol_event: Option<AgentProviderProtocolEvent>,
}

impl AgentProviderFailure {
    /// Constructs one failure without a provider-requested retry delay.
    pub const fn new(class: AgentProviderFailureClass) -> Self {
        Self {
            class,
            retry_after: None,
            protocol_error: None,
            protocol_event: None,
        }
    }

    /// Constructs a content-free provider-wire failure at a closed event phase.
    pub const fn protocol_at(
        error: AgentProviderProtocolError,
        event: Option<AgentProviderProtocolEvent>,
    ) -> Self {
        Self {
            class: AgentProviderFailureClass::Protocol,
            retry_after: None,
            protocol_error: Some(error),
            protocol_event: event,
        }
    }

    /// Constructs one closed failure and validates retry-after compatibility.
    pub const fn try_new(
        class: AgentProviderFailureClass,
        retry_after: Option<AgentProviderRetryAfter>,
    ) -> Result<Self, AgentProviderContractError> {
        if retry_after.is_some()
            && !matches!(
                class.retry_disposition(),
                AgentProviderRetryDisposition::PolicyMayRetry
            )
        {
            return Err(AgentProviderContractError::RetryAfter);
        }
        Ok(Self {
            class,
            retry_after,
            protocol_error: None,
            protocol_event: None,
        })
    }

    /// Closed terminal failure class.
    pub const fn class(self) -> AgentProviderFailureClass {
        self.class
    }

    /// Bounded provider hint, not retry authority.
    pub const fn retry_after(self) -> Option<AgentProviderRetryAfter> {
        self.retry_after
    }

    /// Closed decoder cause when provider wire data violated its contract.
    pub const fn protocol_error(self) -> Option<AgentProviderProtocolError> {
        self.protocol_error
    }

    /// Closed provider event phase where typed decoding failed.
    pub const fn protocol_event(self) -> Option<AgentProviderProtocolEvent> {
        self.protocol_event
    }

    /// Non-authorizing retry classification.
    pub const fn retry_disposition(self) -> AgentProviderRetryDisposition {
        self.class.retry_disposition()
    }
}

/// Refusal to construct or decode a provider-neutral contract.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentProviderContractError {
    /// Configuration selected a different provider protocol than the adapter.
    #[error("agent provider kind does not match the selected adapter")]
    ProviderKind,
    /// Model revision was empty, oversized, URL/path-like, or unsafe ASCII.
    #[error("agent provider model revision is invalid")]
    ModelRevision,
    /// Reasoning effort was incompatible with the selected provider protocol.
    #[error("agent provider reasoning effort is invalid")]
    ReasoningEffort,
    /// Fixed request-envelope input tokens were zero or exceeded the hard limit.
    #[error("agent provider fixed input-token count is invalid")]
    InputTokens,
    /// Requested input accounting is incompatible with the provider or catalog range.
    #[error("agent provider input-accounting mode is invalid")]
    InputAccountingMode,
    /// Requested output-token ceiling was zero or exceeded the hard limit.
    #[error("agent provider output-token ceiling is invalid")]
    OutputTokens,
    /// Provider/tokenizer selection or request ceilings exceed exact admission.
    #[error("agent provider call does not fit its policy admission")]
    AdmissionBudget,
    /// Semantic payload was measured with a different tokenizer revision.
    #[error("agent provider tokenizer revision does not match semantic payload")]
    TokenizerRevision,
    /// Whole structured input did not receive an exact pinned local count.
    #[error("agent provider structured input token quality is not exact local")]
    InputTokenQuality,
    /// One or more stream ceilings were zero or exceeded hard limits.
    #[error("agent provider stream budget is invalid")]
    StreamBudget,
    /// Provider usage overflowed or subset counters contradicted totals.
    #[error("agent provider usage is invalid")]
    Usage,
    /// Retry-after was zero, oversized, or attached to a terminal class.
    #[error("agent provider retry-after is invalid")]
    RetryAfter,
}

/// Closed provider stream protocol failure without wire or model text.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentProviderProtocolError {
    /// SSE framing, UTF-8, or end-of-stream state was invalid.
    #[error("agent provider stream framing is invalid")]
    Framing,
    /// A line, event, text, tool, argument, or event-count ceiling was exceeded.
    #[error("agent provider stream ceiling exceeded")]
    Limit,
    /// A recognized provider event had an invalid typed payload.
    #[error("agent provider stream event is invalid")]
    Event,
    /// Recognized events appeared in an impossible or ambiguous order.
    #[error("agent provider stream sequence is invalid")]
    Sequence,
    /// Provider emitted an output class this browser adapter never requested.
    #[error("agent provider output class is unsupported")]
    UnsupportedOutput,
    /// A complete client tool call failed the closed browser-tool contract.
    #[error("agent provider tool call is invalid")]
    ToolCall,
    /// A known tool failed a closed argument contract; carries no model text.
    #[error("agent provider tool contract is invalid ({0:?}, {1:?})")]
    ToolContract(AgentBrowserToolKind, AgentBrowserToolContractError),
    /// Terminal provider usage was absent or internally inconsistent.
    #[error("agent provider terminal usage is invalid")]
    Usage,
    /// Stream ended without one unambiguous terminal outcome.
    #[error("agent provider terminal event is invalid")]
    Terminal,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenizer(value: &str) -> SemanticTokenizerRevision {
        SemanticTokenizerRevision::try_new(value.to_owned()).expect("valid tokenizer")
    }

    #[test]
    fn provider_configuration_and_diagnostics_are_bounded() {
        let model = AgentProviderModelRevision::try_new("gpt-5.6-terra".to_owned())
            .expect("valid revision");
        assert_eq!(model.as_str(), "gpt-5.6-terra");
        assert!(!format!("{model:?}").contains("gpt-5.6-terra"));
        let config = AgentProviderCallConfig::try_for_test(
            AgentProviderKind::OpenAiResponses,
            model,
            AgentProviderReasoningEffort::High,
            tokenizer("openai:gpt-5.6-terra:v1"),
            AgentProviderPricingProfile::try_new(
                AgentProviderPricingRevision::new(1).expect("pricing revision"),
                16_384,
            )
            .expect("pricing profile"),
            512,
            4_096,
            AgentProviderStreamBudget::STANDARD,
        )
        .expect("valid config");
        assert_eq!(
            config.billing_class(),
            AgentProviderBillingClass::OpenAiDefault
        );
        assert_eq!(
            config.input_accounting_mode(),
            AgentProviderInputAccountingMode::ExactLocal {
                fixed_input_tokens: 512
            }
        );
        assert!(!config.permits_tool(AgentBrowserToolKind::Back));
        let supported = config.clone().with_history_back();
        assert!(!supported.permits_tool(AgentBrowserToolKind::Back));
        assert!(supported
            .clone()
            .with_history_back_available(true)
            .permits_tool(AgentBrowserToolKind::Back));
        assert!(!supported
            .with_history_back_available(false)
            .permits_tool(AgentBrowserToolKind::Back));
        assert_eq!(
            config.reasoning_effort(),
            AgentProviderReasoningEffort::High
        );
        assert_eq!(config.max_output_tokens(), 4_096);
        assert_eq!(config.stream_budget().max_tool_calls(), 1);
        assert_eq!(config.pricing_profile().revision().value(), 1);
        assert_eq!(config.pricing_profile().max_input_tokens(), 16_384);
        assert_eq!(AgentProviderReasoningEffort::None.as_openai_str(), "none");
        assert_eq!(AgentProviderReasoningEffort::Low.as_openai_str(), "low");
        assert_eq!(
            AgentProviderReasoningEffort::Medium.as_openai_str(),
            "medium"
        );
        assert_eq!(AgentProviderReasoningEffort::High.as_openai_str(), "high");
        assert_eq!(AgentProviderReasoningEffort::XHigh.as_openai_str(), "xhigh");
        assert_eq!(AgentProviderReasoningEffort::Max.as_openai_str(), "max");

        for invalid in ["", "../model", "https://model", "model name", "model/alias"] {
            assert_eq!(
                AgentProviderModelRevision::try_new(invalid.to_owned()),
                Err(AgentProviderContractError::ModelRevision)
            );
        }
        assert_eq!(
            AgentProviderModelRevision::try_new("x".repeat(97)),
            Err(AgentProviderContractError::ModelRevision)
        );
        assert_eq!(
            AgentProviderCallConfig::try_for_test(
                AgentProviderKind::AnthropicMessages,
                AgentProviderModelRevision::try_new("claude-opus-5".to_owned())
                    .expect("valid revision"),
                AgentProviderReasoningEffort::None,
                tokenizer("anthropic:claude-opus-5:v1"),
                AgentProviderPricingProfile::try_new(
                    AgentProviderPricingRevision::new(1).expect("pricing revision"),
                    16_384,
                )
                .expect("pricing profile"),
                512,
                0,
                AgentProviderStreamBudget::STANDARD,
            ),
            Err(AgentProviderContractError::OutputTokens)
        );
        assert_eq!(
            AgentProviderCallConfig::try_for_test(
                AgentProviderKind::AnthropicMessages,
                AgentProviderModelRevision::try_new("claude-opus-5".to_owned())
                    .expect("valid revision"),
                AgentProviderReasoningEffort::Low,
                tokenizer("anthropic:claude-opus-5:v1"),
                AgentProviderPricingProfile::try_new(
                    AgentProviderPricingRevision::new(1).expect("pricing revision"),
                    16_384,
                )
                .expect("pricing profile"),
                512,
                1_024,
                AgentProviderStreamBudget::STANDARD,
            ),
            Err(AgentProviderContractError::ReasoningEffort)
        );
    }

    #[test]
    fn provider_call_identity_keeps_its_manifest_guard_private_and_redacted() {
        let identity = AgentProviderCallIdentity {
            manifest: AgentRunManifestId::from_raw(7),
            manifest_guard: [0xA5; 32],
            call: AgentModelCallId::new(8).expect("call"),
            lease: AgentPlanLeaseId::from_raw(9),
            node: AgentPlanNodeId::from_raw(10),
        };
        let debug = format!("{identity:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("165"));
        assert!(identity.matches_manifest_revision(identity.manifest(), [0xA5; 32]));
        assert!(!identity.matches_manifest_revision(identity.manifest(), [0x5A; 32]));
    }

    #[test]
    fn stream_budget_refuses_every_zero_and_over_limit_dimension() {
        assert_eq!(
            AgentProviderStreamBudget::try_new(0, 1, 1, 1, 1),
            Err(AgentProviderContractError::StreamBudget)
        );
        assert_eq!(
            AgentProviderStreamBudget::try_new(1, 0, 1, 1, 1),
            Err(AgentProviderContractError::StreamBudget)
        );
        assert_eq!(
            AgentProviderStreamBudget::try_new(1, 1, 0, 1, 1),
            Err(AgentProviderContractError::StreamBudget)
        );
        assert_eq!(
            AgentProviderStreamBudget::try_new(1, 1, 1, 0, 1),
            Err(AgentProviderContractError::StreamBudget)
        );
        assert_eq!(
            AgentProviderStreamBudget::try_new(1, 1, 1, 1, 0),
            Err(AgentProviderContractError::StreamBudget)
        );
        assert_eq!(
            AgentProviderStreamBudget::try_new(1, MAX_AGENT_PROVIDER_STREAM_EVENTS + 1, 1, 1, 1,),
            Err(AgentProviderContractError::StreamBudget)
        );
        let maximum = AgentProviderStreamBudget::try_new(
            MAX_AGENT_PROVIDER_STREAM_WIRE_BYTES,
            MAX_AGENT_PROVIDER_STREAM_EVENTS,
            MAX_AGENT_PROVIDER_OUTPUT_TEXT_BYTES,
            MAX_AGENT_PROVIDER_TOOL_CALLS,
            MAX_AGENT_PROVIDER_TOOL_ARGUMENT_BYTES,
        )
        .expect("hard boundary");
        assert_eq!(
            maximum.max_wire_bytes(),
            MAX_AGENT_PROVIDER_STREAM_WIRE_BYTES
        );
        assert_eq!(maximum.max_events(), MAX_AGENT_PROVIDER_STREAM_EVENTS);
    }

    #[test]
    fn usage_keeps_vendor_details_as_subsets_without_hiding_overage() {
        let usage =
            AgentProviderUsage::try_new(5_000, 900, 2_000, 1_000, 700).expect("valid usage");
        assert_eq!(usage.total_tokens(), 5_900);
        assert_eq!(usage.cached_input_tokens(), 2_000);
        assert_eq!(usage.cache_write_input_tokens(), 1_000);
        assert_eq!(usage.reasoning_output_tokens(), 700);
        assert_eq!(
            AgentProviderUsage::try_new(10, 5, 11, 0, 0),
            Err(AgentProviderContractError::Usage)
        );
        assert_eq!(
            AgentProviderUsage::try_new(10, 5, 7, 4, 0),
            Err(AgentProviderContractError::Usage)
        );
        assert_eq!(
            AgentProviderUsage::try_new(u64::MAX, 1, 0, 0, 0),
            Err(AgentProviderContractError::Usage)
        );
    }

    #[test]
    fn retry_hints_never_become_retry_authority() {
        let retry_after = AgentProviderRetryAfter::try_from_millis(2_000).expect("delay");
        let failure = AgentProviderFailure::try_new(
            AgentProviderFailureClass::RateLimited,
            Some(retry_after),
        )
        .expect("retryable class");
        assert_eq!(
            failure.retry_disposition(),
            AgentProviderRetryDisposition::PolicyMayRetry
        );
        assert_eq!(failure.retry_after(), Some(retry_after));
        assert_eq!(
            AgentProviderFailure::try_new(
                AgentProviderFailureClass::Authentication,
                Some(retry_after),
            ),
            Err(AgentProviderContractError::RetryAfter)
        );
        assert_eq!(
            AgentProviderFailureClass::Cancelled.retry_disposition(),
            AgentProviderRetryDisposition::Never
        );
        assert_eq!(
            AgentProviderFailureClass::Integration.retry_disposition(),
            AgentProviderRetryDisposition::Never
        );
        assert_eq!(
            AgentProviderFailure::try_new(
                AgentProviderFailureClass::Integration,
                Some(retry_after),
            ),
            Err(AgentProviderContractError::RetryAfter)
        );
        let protocol = AgentProviderFailure::protocol_at(
            AgentProviderProtocolError::Sequence,
            Some(AgentProviderProtocolEvent::OpenAiTerminal),
        );
        assert_eq!(protocol.class(), AgentProviderFailureClass::Protocol);
        assert_eq!(
            protocol.protocol_error(),
            Some(AgentProviderProtocolError::Sequence)
        );
        assert_eq!(
            protocol.protocol_event(),
            Some(AgentProviderProtocolEvent::OpenAiTerminal)
        );
        assert_eq!(protocol.retry_after(), None);
    }
}
