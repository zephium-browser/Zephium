//! Bounded, platform-neutral content-blocking policy contracts.
//!
//! Parsing filter-list syntax and owning native WebKit/WebView2 objects belong
//! outside core. This module carries only immutable policy artifacts and the
//! exact identities used to order compilation, native installation, profile
//! retirement, and first-navigation admission.

use std::fmt;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::ids::ProfileId;

mod activity;
pub use activity::{ActivePageLoad, PageActivity};
mod statistics;
pub use statistics::{BlockedLoadCounter, BlockerStatistics};

mod cosmetics;
pub use cosmetics::{DocumentStyleFailure, DocumentStylePlan, DocumentStyleProvider};
mod sites;
pub use sites::{
    BlockerSite, BlockerSitePreferences, PersonalHide, PreparedBlockerSite, PreparedBlockerSites,
    SitePreferenceChange, SitePreferenceError, MAX_PAUSED_BLOCKER_SITES, MAX_PERSONAL_HIDES,
    MAX_PERSONAL_RULE_BYTES,
};

/// Upper bound for one native declarative policy crossing the application to
/// engine boundary. WebKit compilation has substantially higher temporary
/// costs than the encoded JSON itself, so accepting an unbounded artifact
/// would turn a maintained filter-list update into an allocation attack.
pub const MAX_DECLARATIVE_RULE_BYTES: usize = 32 * 1024 * 1024;

/// Native request callbacks must reject oversized values before allocating or
/// invoking the matcher. This is intentionally independent of the smaller
/// omnibox/navigation limit because subresource URLs can legitimately carry
/// longer query strings.
pub const MAX_NETWORK_REQUEST_URL_BYTES: usize = 32 * 1024;
pub const MAX_NETWORK_REQUEST_METHOD_BYTES: usize = 32;

/// Durable per-profile preference revision.
///
/// This is not a compiled-list generation: refreshing the same enabled list
/// configuration produces a new process-local [`ContentPolicyGeneration`]
/// without fabricating a user preference mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockerConfigRevision(u64);

impl BlockerConfigRevision {
    pub const INITIAL: Self = Self(1);

    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }
}

/// Exact process-local identity for one compile/install attempt.
///
/// Native callbacks from an older attempt may arrive after a profile is
/// updated or deleted. Generations never wrap, so those callbacks can only be
/// discarded, never mistaken for a replacement policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentPolicyGeneration(u64);

impl ContentPolicyGeneration {
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 {
            None
        } else {
            Some(Self(value))
        }
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockerConfig {
    /// Desired protection. Readiness is reported separately until a concrete
    /// bundled or validated updated policy has been installed.
    pub enabled: bool,
}

impl Default for BlockerConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileBlockerConfig {
    pub profile: ProfileId,
    pub revision: BlockerConfigRevision,
    pub config: BlockerConfig,
}

/// Stable compiler failure classification.
///
/// These values deliberately contain no parser text, native error string, or
/// filter-list content. They can be exposed to privileged diagnostics without
/// turning untrusted source material into a logging or IPC surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockerCompileFailure {
    SourceUnavailable,
    InvalidSource,
    ResourceLimit,
    Internal,
}

/// Content digest of the canonical source/configuration used for an artifact.
///
/// Public subscription digests identify filter material. Digests derived from
/// personal selections or site preferences remain private and must not enter
/// persisted diagnostics.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentRuleDigest([u8; 32]);

impl ContentRuleDigest {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ContentRuleDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContentRuleDigest(")?;
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        formatter.write_str("…)")
    }
}

/// Identity of the exact native declarative artifact bytes and format.
///
/// This is deliberately distinct from [`ContentRuleDigest`]: policy source
/// identity and native compiler cache identity have different versioning
/// domains.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeclarativeArtifactDigest([u8; 32]);

/// Shared compiler/consumer identity version for native WebKit JSON.
pub const WEBKIT_ARTIFACT_FORMAT_VERSION: u32 = 4;

impl DeclarativeArtifactDigest {
    fn for_encoded(format: DeclarativeRuleFormat, encoded: &str) -> Self {
        let format_version = match format {
            DeclarativeRuleFormat::WebKitContentBlockerV1 => WEBKIT_ARTIFACT_FORMAT_VERSION,
        };
        let mut digest = Sha256::new();
        digest.update(b"zephium-webkit-content-rules");
        digest.update(format_version.to_be_bytes());
        digest.update(encoded.as_bytes());
        Self(digest.finalize().into())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// The rule text behind a declarative artifact. A freshly compiled artifact
/// holds it in memory; one read back from the compiled-artifact cache reads,
/// re-checks and validates it only when the native store has no compiled list
/// for its digest, so a warm launch never brings it into memory.
pub trait DeclarativeRuleText: Send + Sync {
    /// The text's length in bytes, known without reading it.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The validated text, or `None` when it no longer matches the artifact
    /// digest. This may read and parse tens of megabytes: never call it on a
    /// thread that serves the UI.
    fn load(&self) -> Option<Arc<str>>;
}

impl DeclarativeRuleText for Arc<str> {
    fn len(&self) -> usize {
        str::len(self)
    }

    fn load(&self) -> Option<Arc<str>> {
        Some(self.clone())
    }
}

impl fmt::Debug for DeclarativeArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeclarativeArtifactDigest(")?;
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        formatter.write_str("…)")
    }
}

/// Honest compiler coverage for the exact artifact.
///
/// WebKit's declarative language cannot express every adblock-rust feature.
/// Keeping accepted and omitted counts beside the artifact prevents the UI or
/// diagnostics from presenting lossy conversion as cross-engine parity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContentRuleCoverage {
    pub source_rules: u64,
    pub accepted_rules: u64,
    pub rejected_rules: u64,
    pub platform_omitted_rules: u64,
    /// Accepted rules whose native resource/source reachability or request
    /// attribution is intentionally conservative rather than semantically
    /// exact. Each rule is counted once across all detail dimensions.
    pub platform_approximated_rules: u64,
    /// Accepted rules with only partial native resource-type reachability.
    pub platform_resource_approximated_rules: u64,
    /// Accepted rules with only partial native request-source-kind
    /// reachability, such as worker-originated WebView2 requests.
    pub platform_source_kind_approximated_rules: u64,
    /// Accepted rules whose initiating-document attribution is approximated
    /// by the native policy language.
    pub platform_attribution_approximated_rules: u64,
    /// Post-`$badfilter` blocking entries represented by the native artifact.
    ///
    /// This is a structural count, not proof that exceptions leave a
    /// semantically reachable block for every possible request.
    pub blocking_rule_entries: u64,
}

impl ContentRuleCoverage {
    pub const fn is_consistent(self) -> bool {
        let Some(parsed) = self.accepted_rules.checked_add(self.rejected_rules) else {
            return false;
        };
        let Some(represented) = self.accepted_rules.checked_sub(self.platform_omitted_rules) else {
            return false;
        };
        parsed == self.source_rules
            && self.platform_approximated_rules <= represented
            && self.platform_resource_approximated_rules <= self.platform_approximated_rules
            && self.platform_source_kind_approximated_rules <= self.platform_approximated_rules
            && self.platform_attribution_approximated_rules <= self.platform_approximated_rules
            && self.blocking_rule_entries <= represented
    }

    pub const fn has_blocking_entries(self) -> bool {
        self.blocking_rule_entries != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkResourceType {
    Document,
    Subdocument,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    XmlHttpRequest,
    Fetch,
    WebSocket,
    Ping,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkRequestSourceKind {
    Document,
    SharedWorker,
    ServiceWorker,
    Unknown,
}

/// Quality of the source URL supplied to network-rule matching.
///
/// WebView2 currently exposes the requested resource and broad source kind,
/// but not the exact initiating frame URL. Callers must never represent a
/// top-level approximation as exact attribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkAttribution {
    Exact,
    TopLevelApproximation,
    /// The native engine exposed no trustworthy initiating document. Only
    /// rules proven independent of source-domain and party classification may
    /// be evaluated.
    SourceIndependent,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkRequest<'a> {
    url: &'a str,
    source_url: Option<&'a str>,
    method: &'a str,
    resource_type: NetworkResourceType,
    source_kind: NetworkRequestSourceKind,
    attribution: NetworkAttribution,
}

impl<'a> NetworkRequest<'a> {
    /// Constructs metadata with an exact initiating-document URL.
    pub fn exact(
        url: &'a str,
        source_url: &'a str,
        method: &'a str,
        resource_type: NetworkResourceType,
        source_kind: NetworkRequestSourceKind,
    ) -> Option<Self> {
        Self::bounded(
            url,
            Some(source_url),
            method,
            resource_type,
            source_kind,
            NetworkAttribution::Exact,
        )
    }

    /// Constructs the conservative WebView2 form: a known top-level URL that
    /// is not represented as the exact initiating frame.
    pub fn top_level_approximation(
        url: &'a str,
        top_level_url: &'a str,
        method: &'a str,
        resource_type: NetworkResourceType,
        source_kind: NetworkRequestSourceKind,
    ) -> Option<Self> {
        Self::bounded(
            url,
            Some(top_level_url),
            method,
            resource_type,
            source_kind,
            NetworkAttribution::TopLevelApproximation,
        )
    }

    /// Constructs metadata when the native engine cannot establish any
    /// initiating URL. Matchers must fail open rather than substitute an
    /// empty or stale source and accidentally classify the request as third
    /// party.
    pub fn unavailable(
        url: &'a str,
        method: &'a str,
        resource_type: NetworkResourceType,
        source_kind: NetworkRequestSourceKind,
    ) -> Option<Self> {
        Self::bounded(
            url,
            None,
            method,
            resource_type,
            source_kind,
            NetworkAttribution::Unavailable,
        )
    }

    /// Constructs metadata for a source-independent native matcher.
    ///
    /// This is stronger than [`Self::unavailable`]: the caller has an exact
    /// resource type and may evaluate only rules which cannot inspect an
    /// initiating document.
    pub fn source_independent(
        url: &'a str,
        method: &'a str,
        resource_type: NetworkResourceType,
        source_kind: NetworkRequestSourceKind,
    ) -> Option<Self> {
        Self::bounded(
            url,
            None,
            method,
            resource_type,
            source_kind,
            NetworkAttribution::SourceIndependent,
        )
    }

    fn bounded(
        url: &'a str,
        source_url: Option<&'a str>,
        method: &'a str,
        resource_type: NetworkResourceType,
        source_kind: NetworkRequestSourceKind,
        attribution: NetworkAttribution,
    ) -> Option<Self> {
        if url.is_empty()
            || url.len() > MAX_NETWORK_REQUEST_URL_BYTES
            || source_url.is_some_and(|source| {
                source.is_empty() || source.len() > MAX_NETWORK_REQUEST_URL_BYTES
            })
            || method.is_empty()
            || method.len() > MAX_NETWORK_REQUEST_METHOD_BYTES
            || !method.bytes().all(|byte| {
                byte.is_ascii_uppercase()
                    || byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(
                        byte,
                        b'!' | b'#'
                            | b'$'
                            | b'%'
                            | b'&'
                            | b'\''
                            | b'*'
                            | b'+'
                            | b'-'
                            | b'.'
                            | b'^'
                            | b'_'
                            | b'`'
                            | b'|'
                            | b'~'
                    )
            })
        {
            return None;
        }
        let requires_source = matches!(
            attribution,
            NetworkAttribution::Exact | NetworkAttribution::TopLevelApproximation
        );
        if requires_source != source_url.is_some() {
            return None;
        }
        Some(Self {
            url,
            source_url,
            method,
            resource_type,
            source_kind,
            attribution,
        })
    }

    pub const fn url(&self) -> &'a str {
        self.url
    }

    pub const fn source_url(&self) -> Option<&'a str> {
        self.source_url
    }

    pub const fn method(&self) -> &'a str {
        self.method
    }

    pub const fn resource_type(&self) -> NetworkResourceType {
        self.resource_type
    }

    pub const fn source_kind(&self) -> NetworkRequestSourceKind {
        self.source_kind
    }

    pub const fn attribution(&self) -> NetworkAttribution {
        self.attribution
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkDecision {
    Allow,
    Block,
}

/// Volatile, URL-free aggregate health of one immutable runtime matcher.
///
/// Counters saturate at `u64::MAX`, are never persisted, and are intentionally
/// limited to fail-open classes needed to assess matcher capacity. They do not
/// contain profile IDs, origins, URLs, rule identities, or request metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkPolicyDiagnostics {
    pub total_decisions: u64,
    pub candidate_budget_exhausted: u64,
    pub matcher_unavailable: u64,
    pub matcher_unprepared: u64,
    pub attribution_unavailable: u64,
    pub evaluation_errors: u64,
}

impl NetworkPolicyDiagnostics {
    /// Every exceptional class is mutually exclusive and must be a subset of
    /// the decisions observed by the same matcher. Saturating addition keeps
    /// the invariant meaningful after the counters themselves saturate.
    pub const fn is_consistent(self) -> bool {
        let exceptional = self
            .candidate_budget_exhausted
            .saturating_add(self.matcher_unavailable)
            .saturating_add(self.matcher_unprepared)
            .saturating_add(self.attribution_unavailable)
            .saturating_add(self.evaluation_errors);
        exceptional <= self.total_decisions
    }
}

/// Synchronous request policy used only by native adapters with a trustworthy
/// interception callback.
///
/// Implementations must be immutable, bounded, nonblocking and free of
/// filesystem, actor, UI, and network dependencies.
pub trait NetworkRequestPolicy: Send + Sync {
    fn decide(&self, request: &NetworkRequest<'_>) -> NetworkDecision;

    /// Returns a lock-free snapshot of volatile aggregate matcher health.
    ///
    /// Policies which do not instrument runtime matching retain the explicit
    /// all-zero default. Native callbacks must not dispatch work merely to
    /// publish these diagnostics.
    fn diagnostics(&self) -> NetworkPolicyDiagnostics {
        NetworkPolicyDiagnostics::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarativeRuleFormat {
    WebKitContentBlockerV1,
}

#[derive(Clone)]
pub enum ContentRulesPayload {
    /// An explicit generation with no blocking. This is required when the
    /// profile preference is disabled; absence is never interpreted as
    /// allow-all.
    AllowAll,
    Runtime(Arc<dyn NetworkRequestPolicy>),
    Declarative {
        format: DeclarativeRuleFormat,
        artifact_digest: DeclarativeArtifactDigest,
        encoded: Arc<dyn DeclarativeRuleText>,
    },
}

impl fmt::Debug for ContentRulesPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AllowAll => formatter.write_str("AllowAll"),
            Self::Runtime(_) => formatter.write_str("Runtime(<bounded matcher>)"),
            Self::Declarative {
                format,
                artifact_digest,
                encoded,
            } => formatter
                .debug_struct("Declarative")
                .field("format", format)
                .field("artifact_digest", artifact_digest)
                .field("encoded_bytes", &encoded.len())
                .finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ContentRules {
    digest: ContentRuleDigest,
    coverage: ContentRuleCoverage,
    payload: ContentRulesPayload,
    cosmetics: Option<Arc<dyn DocumentStyleProvider>>,
}

impl ContentRules {
    pub fn allow_all(digest: ContentRuleDigest) -> Arc<Self> {
        Arc::new(Self {
            digest,
            coverage: ContentRuleCoverage::default(),
            payload: ContentRulesPayload::AllowAll,
            cosmetics: None,
        })
    }

    pub fn runtime(
        digest: ContentRuleDigest,
        coverage: ContentRuleCoverage,
        policy: Arc<dyn NetworkRequestPolicy>,
    ) -> Option<Arc<Self>> {
        (coverage.is_consistent() && coverage.has_blocking_entries()).then(|| {
            Arc::new(Self {
                digest,
                coverage,
                payload: ContentRulesPayload::Runtime(policy),
                cosmetics: None,
            })
        })
    }

    pub fn declarative(
        digest: ContentRuleDigest,
        coverage: ContentRuleCoverage,
        format: DeclarativeRuleFormat,
        encoded: Arc<str>,
    ) -> Option<Arc<Self>> {
        if !coverage.is_consistent()
            || !coverage.has_blocking_entries()
            || encoded.is_empty()
            || encoded.len() > MAX_DECLARATIVE_RULE_BYTES
        {
            return None;
        }
        let artifact_digest = DeclarativeArtifactDigest::for_encoded(format, &encoded);
        Some(Arc::new(Self {
            digest,
            coverage,
            payload: ContentRulesPayload::Declarative {
                format,
                artifact_digest,
                encoded: Arc::new(encoded),
            },
            cosmetics: None,
        }))
    }

    /// A declarative artifact whose producer already bound `artifact_digest`
    /// to the text, as the integrity-checked compiled-artifact cache does.
    /// `encoded.load()` must refuse text that does not match it.
    pub fn declarative_with_digest(
        digest: ContentRuleDigest,
        coverage: ContentRuleCoverage,
        format: DeclarativeRuleFormat,
        artifact_digest: [u8; 32],
        encoded: Arc<dyn DeclarativeRuleText>,
    ) -> Option<Arc<Self>> {
        if !coverage.is_consistent()
            || !coverage.has_blocking_entries()
            || encoded.is_empty()
            || encoded.len() > MAX_DECLARATIVE_RULE_BYTES
        {
            return None;
        }
        Some(Arc::new(Self {
            digest,
            coverage,
            payload: ContentRulesPayload::Declarative {
                format,
                artifact_digest: DeclarativeArtifactDigest(artifact_digest),
                encoded,
            },
            cosmetics: None,
        }))
    }

    /// Attaches separately compiled subscription styles without changing the
    /// network artifact. The producer's policy digest must already bind both.
    pub fn with_cosmetics(
        mut self: Arc<Self>,
        provider: Arc<dyn DocumentStyleProvider>,
    ) -> Arc<Self> {
        if self.enabled() {
            let rules = Arc::make_mut(&mut self);
            rules.cosmetics = Some(provider);
        }
        self
    }

    pub fn cosmetics(&self) -> Option<&Arc<dyn DocumentStyleProvider>> {
        self.cosmetics.as_ref()
    }

    pub const fn digest(&self) -> ContentRuleDigest {
        self.digest
    }

    pub const fn coverage(&self) -> ContentRuleCoverage {
        self.coverage
    }

    pub const fn payload(&self) -> &ContentRulesPayload {
        &self.payload
    }

    /// Returns the immutable runtime policy for a native adapter or a
    /// non-owning diagnostics observer.
    pub fn runtime_policy(&self) -> Option<&Arc<dyn NetworkRequestPolicy>> {
        match &self.payload {
            ContentRulesPayload::Runtime(policy) => Some(policy),
            ContentRulesPayload::AllowAll | ContentRulesPayload::Declarative { .. } => None,
        }
    }

    pub const fn enabled(&self) -> bool {
        !matches!(self.payload, ContentRulesPayload::AllowAll)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentRuleApplyFailure {
    UnsupportedArtifact,
    InvalidArtifact,
    NativeCompilation,
    NativeInstallation,
    /// A partially installed or superseded native registration could not be
    /// removed exactly. Continued browsing is unsafe because two policies may
    /// now coexist without an authoritative owner.
    NativeCleanup,
    Superseded,
}

/// Exact terminal reason why one desired profile policy is unavailable.
///
/// The application keeps this distinct from the current native generation: a
/// failed replacement may still retain a prior known-good policy, while an
/// initial failure may leave browsing under the explicit provisional policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentPolicyFailure {
    SitePreferencesUnavailable,
    GenerationExhausted,
    CompilerDispatchRejected,
    CompilerUnavailable,
    Compile(BlockerCompileFailure),
    CompiledArtifactMismatch,
    NativeDispatchRejected,
    NativeUnsupported,
    Native(ContentRuleApplyFailure),
    ContradictoryNativeSettlement,
}

impl ContentPolicyFailure {
    /// Whether retrying the same durable configuration can plausibly recover
    /// from a transient process-local condition.
    ///
    /// This is a policy hint, not an automatic action. Callers must still use
    /// an exact failed generation and the application enforces a bounded
    /// explicit retry budget.
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::CompilerDispatchRejected
                | Self::SitePreferencesUnavailable
                | Self::Compile(BlockerCompileFailure::SourceUnavailable)
                | Self::Compile(BlockerCompileFailure::Internal)
                | Self::NativeDispatchRejected
                | Self::Native(
                    ContentRuleApplyFailure::NativeCompilation
                        | ContentRuleApplyFailure::NativeInstallation
                        | ContentRuleApplyFailure::Superseded
                )
        )
    }
}

/// Process-local state of one profile's exact content-policy generation.
///
/// This type is port-neutral so a future privileged UI projection can carry
/// it without duplicating or weakening the actor's state model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileContentPolicyState {
    Uninitialized,
    Compiling {
        desired: ContentPolicyGeneration,
        retained: Option<ContentPolicyGeneration>,
        retries_remaining: u8,
    },
    Installing {
        desired: ContentPolicyGeneration,
        retained: Option<ContentPolicyGeneration>,
        retries_remaining: u8,
    },
    Ready {
        applied: ContentPolicyGeneration,
    },
    Failed {
        desired: ContentPolicyGeneration,
        retained: Option<ContentPolicyGeneration>,
        failure: ContentPolicyFailure,
        retries_remaining: u8,
    },
    Retired,
}

/// Typed, bounded status returned by the application actor for one exact
/// durable profile configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProfileContentPolicyStatus {
    pub profile: ProfileId,
    pub config_revision: BlockerConfigRevision,
    /// Configuration the current desired generation is trying to realize.
    pub desired_config: BlockerConfig,
    /// Configuration of the exact known-good native generation, if any.
    ///
    /// This is intentionally separate from `desired_config`: a replacement
    /// can fail while the prior policy remains active, and diagnostics must
    /// never present a retained allow-all generation as enabled protection.
    pub applied_config: Option<BlockerConfig>,
    /// Coverage of the exact native generation represented by
    /// `applied_config`. A failed replacement retains both values from the
    /// prior known-good generation; loss of native authority clears both.
    pub applied_coverage: Option<ContentRuleCoverage>,
    pub state: ProfileContentPolicyState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_and_generations_never_admit_zero_or_wrap() {
        assert!(BlockerConfigRevision::new(0).is_none());
        assert_eq!(
            BlockerConfigRevision::INITIAL
                .next()
                .map(|revision| revision.get()),
            Some(2)
        );
        assert!(BlockerConfigRevision::new(u64::MAX)
            .and_then(BlockerConfigRevision::next)
            .is_none());
        assert!(ContentPolicyGeneration::new(0).is_none());
        assert_eq!(
            ContentPolicyGeneration::new(7).map(|value| value.get()),
            Some(7)
        );
    }

    #[test]
    fn content_policy_retryability_is_explicit_and_fail_closed() {
        for failure in [
            ContentPolicyFailure::CompilerDispatchRejected,
            ContentPolicyFailure::Compile(BlockerCompileFailure::SourceUnavailable),
            ContentPolicyFailure::Compile(BlockerCompileFailure::Internal),
            ContentPolicyFailure::NativeDispatchRejected,
            ContentPolicyFailure::Native(ContentRuleApplyFailure::NativeCompilation),
            ContentPolicyFailure::Native(ContentRuleApplyFailure::NativeInstallation),
            ContentPolicyFailure::Native(ContentRuleApplyFailure::Superseded),
        ] {
            assert!(failure.retryable(), "{failure:?}");
        }
        for failure in [
            ContentPolicyFailure::GenerationExhausted,
            ContentPolicyFailure::CompilerUnavailable,
            ContentPolicyFailure::Compile(BlockerCompileFailure::InvalidSource),
            ContentPolicyFailure::Compile(BlockerCompileFailure::ResourceLimit),
            ContentPolicyFailure::CompiledArtifactMismatch,
            ContentPolicyFailure::NativeUnsupported,
            ContentPolicyFailure::Native(ContentRuleApplyFailure::UnsupportedArtifact),
            ContentPolicyFailure::Native(ContentRuleApplyFailure::InvalidArtifact),
            ContentPolicyFailure::Native(ContentRuleApplyFailure::NativeCleanup),
            ContentPolicyFailure::ContradictoryNativeSettlement,
        ] {
            assert!(!failure.retryable(), "{failure:?}");
        }
    }

    #[test]
    fn coverage_rejects_overflow_and_inconsistent_counts() {
        assert!(ContentRuleCoverage {
            source_rules: 3,
            accepted_rules: 2,
            rejected_rules: 1,
            platform_omitted_rules: 1,
            platform_approximated_rules: 1,
            platform_resource_approximated_rules: 1,
            platform_source_kind_approximated_rules: 1,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        }
        .is_consistent());
        assert!(ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        }
        .is_consistent());
        assert!(!ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 2,
        }
        .is_consistent());
        assert!(!ContentRuleCoverage {
            source_rules: 3,
            accepted_rules: 2,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        }
        .is_consistent());
        assert!(!ContentRuleCoverage {
            source_rules: u64::MAX,
            accepted_rules: u64::MAX,
            rejected_rules: 1,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        }
        .is_consistent());
        assert!(!ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 1,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        }
        .is_consistent());
    }

    #[test]
    fn runtime_diagnostics_require_exceptional_counts_to_fit_total() {
        assert!(NetworkPolicyDiagnostics {
            total_decisions: 5,
            candidate_budget_exhausted: 1,
            matcher_unavailable: 1,
            matcher_unprepared: 1,
            attribution_unavailable: 1,
            evaluation_errors: 1,
        }
        .is_consistent());
        assert!(!NetworkPolicyDiagnostics {
            total_decisions: 1,
            candidate_budget_exhausted: 1,
            matcher_unavailable: 1,
            ..NetworkPolicyDiagnostics::default()
        }
        .is_consistent());
        assert!(NetworkPolicyDiagnostics {
            total_decisions: u64::MAX,
            candidate_budget_exhausted: u64::MAX,
            matcher_unavailable: 1,
            ..NetworkPolicyDiagnostics::default()
        }
        .is_consistent());
    }

    #[test]
    fn network_request_bounds_values_before_matcher_entry() {
        let exact = NetworkRequest::exact(
            "https://cdn.example/script.js",
            "https://example/",
            "GET",
            NetworkResourceType::Script,
            NetworkRequestSourceKind::Document,
        );
        assert!(exact.is_some());
        assert!(NetworkRequest::unavailable(
            "https://example/",
            "GET",
            NetworkResourceType::Document,
            NetworkRequestSourceKind::Document,
        )
        .is_some());
        assert!(NetworkRequest::source_independent(
            "https://cdn.example/script.js",
            "GET",
            NetworkResourceType::Script,
            NetworkRequestSourceKind::Document,
        )
        .is_some());
        assert!(NetworkRequest::unavailable(
            "https://example/",
            "BAD METHOD",
            NetworkResourceType::Document,
            NetworkRequestSourceKind::Document,
        )
        .is_none());
        let oversized = "x".repeat(MAX_NETWORK_REQUEST_URL_BYTES + 1);
        assert!(NetworkRequest::unavailable(
            &oversized,
            "GET",
            NetworkResourceType::Other,
            NetworkRequestSourceKind::Unknown,
        )
        .is_none());
    }

    #[test]
    fn declarative_artifacts_are_bounded_and_coverage_checked() {
        let digest = ContentRuleDigest::from_bytes([1; 32]);
        let valid = ContentRuleCoverage {
            source_rules: 1,
            accepted_rules: 1,
            rejected_rules: 0,
            platform_omitted_rules: 0,
            platform_approximated_rules: 0,
            platform_resource_approximated_rules: 0,
            platform_source_kind_approximated_rules: 0,
            platform_attribution_approximated_rules: 0,
            blocking_rule_entries: 1,
        };
        assert!(ContentRules::declarative(
            digest,
            valid,
            DeclarativeRuleFormat::WebKitContentBlockerV1,
            Arc::<str>::from("[]"),
        )
        .is_some());
        assert!(ContentRules::declarative(
            digest,
            valid,
            DeclarativeRuleFormat::WebKitContentBlockerV1,
            Arc::<str>::from(""),
        )
        .is_none());
        assert!(ContentRules::declarative(
            digest,
            ContentRuleCoverage {
                source_rules: 2,
                ..valid
            },
            DeclarativeRuleFormat::WebKitContentBlockerV1,
            Arc::<str>::from("[]"),
        )
        .is_none());

        let first = ContentRules::declarative(
            digest,
            valid,
            DeclarativeRuleFormat::WebKitContentBlockerV1,
            Arc::<str>::from("[]"),
        )
        .unwrap();
        let second = ContentRules::declarative(
            digest,
            valid,
            DeclarativeRuleFormat::WebKitContentBlockerV1,
            Arc::<str>::from("[ ]"),
        )
        .unwrap();
        let artifact_digest = |rules: &ContentRules| match rules.payload() {
            ContentRulesPayload::Declarative {
                artifact_digest, ..
            } => *artifact_digest,
            _ => unreachable!(),
        };
        assert_ne!(artifact_digest(&first), artifact_digest(&second));
    }
}

mod picker;
pub use picker::{
    ElementPickerCompletion, ElementPickerRequest, ElementPickerResult, ElementSelection,
};
