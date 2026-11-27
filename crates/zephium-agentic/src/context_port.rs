//! Closed shell/native port for agent-browser context lifecycle work.
//!
//! This vocabulary is shared only by trusted shell and native adapters. It
//! cannot carry JavaScript, selectors, DOM data, native handles, profile
//! paths, cookies, headers, provider data, or platform error text. Every
//! context result retains the exact lifecycle join that admitted it.

use std::fmt;
use std::num::NonZeroU64;

use thiserror::Error;
use url::Url;

use crate::{
    ContextCapabilities, ContextCookieTransferRequest, ContextCookieTransferSettlement,
    ContextJoin, ContextKind, ContextOperationJoin, ContextOperationKind, ContextProfileLease,
    SemanticActionNativeRequest, SemanticActionNativeSettlement, SemanticOrigin,
    SemanticRuntimeInvocation, SemanticRuntimeSettlement, SemanticScreenshotNativeCapture,
    SemanticScreenshotNativeFailure, SemanticScreenshotNativeRequest, MAX_LIVE_CONTEXTS,
    MAX_PENDING_SEMANTIC_SCREENSHOTS,
};

/// Maximum number of lifecycle tasks one native adapter may retain.
///
/// The domain registry permits at most one mutating operation per live
/// context. The second cohort leaves room for cancellation and resource-audit
/// work without creating an unbounded native queue.
pub const MAX_PENDING_NATIVE_CONTEXT_TASKS: usize = MAX_LIVE_CONTEXTS * 2;

/// Maximum canonical destination origins one navigation may authorize for redirects.
///
/// This is intentionally narrower than a complete run manifest. A shell must
/// project only the origins needed by this exact navigation operation.
pub const MAX_CONTEXT_NAVIGATION_REDIRECT_ORIGINS: usize = 8;

/// Maximum server redirects one exact native navigation identity may follow.
pub const MAX_CONTEXT_NAVIGATION_REDIRECTS: usize = 8;

// Trusted agent operands retain their smaller contract independently of the
// ordinary browser's larger page-navigation allowance.
const MAX_CONTEXT_NAVIGATION_TARGET_BYTES: usize = 8 * 1024;

/// Fixed logical viewport for a run-owned browser context.
///
/// V1 deliberately exposes no arbitrary constructor: page layout, semantic
/// filtering, and viewport screenshots must not depend on a model-selected or
/// platform-default size. Borrowed and human-handoff contexts retain the
/// viewport of their ordinary presentation owner instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextOwnedViewport {
    width: u16,
    height: u16,
}

impl ContextOwnedViewport {
    /// Product-standard owned-context viewport in logical/CSS pixels.
    pub const STANDARD: Self = Self {
        width: 1_280,
        height: 800,
    };

    /// Logical/CSS width admitted by the native adapter.
    pub const fn width(self) -> u16 {
        self.width
    }

    /// Logical/CSS height admitted by the native adapter.
    pub const fn height(self) -> u16 {
        self.height
    }

    /// Fixed logical pixel area used by resource qualification.
    pub const fn logical_area(self) -> u32 {
        self.width as u32 * self.height as u32
    }
}

/// Refusal while constructing a closed native-port value.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ContextPortContractError {
    /// The operation join has the wrong closed operation class.
    #[error("context operation kind does not match native request")]
    OperationKind,
    /// The request source, proof, and immutable context kind disagree.
    #[error("context kind does not match native request")]
    ContextKind,
    /// The capability inventory belongs to another context kind.
    #[error("context capability kind does not match native request")]
    CapabilityKind,
    /// The retained profile lease belongs to another context identity.
    #[error("context profile lease does not match native request")]
    ProfileLease,
    /// The target is malformed or forbidden by the browser navigation gate.
    #[error("context navigation target is forbidden")]
    NavigationTarget,
    /// A redirect policy did not name any destination origin.
    #[error("context navigation redirect policy is empty")]
    EmptyRedirectPolicy,
    /// A redirect policy exceeded its fixed destination-origin ceiling.
    #[error("context navigation redirect origin ceiling exceeded")]
    RedirectOriginLimit,
    /// A redirect policy repeated one canonical destination origin.
    #[error("context navigation redirect policy contains a duplicate origin")]
    DuplicateRedirectOrigin,
    /// One resource count exceeds its fixed process ceiling.
    #[error("native context resource ceiling exceeded")]
    ResourceLimit,
    /// Native resource counts contradict each other.
    #[error("native context resource accounting is contradictory")]
    ResourceInvariant,
}

/// Browser navigation target validated at the trusted shell boundary.
///
/// This is not policy authorization. Later policy and approval layers still
/// decide whether a valid web target may be visited for a particular run.
#[derive(Clone, Eq, PartialEq)]
pub struct ContextNavigationTarget(Url);

impl ContextNavigationTarget {
    /// Parses and validates one exact absolute browser target.
    pub fn parse(value: &str) -> Result<Self, ContextPortContractError> {
        if value.len() > MAX_CONTEXT_NAVIGATION_TARGET_BYTES {
            return Err(ContextPortContractError::NavigationTarget);
        }
        let target = Url::parse(value).map_err(|_| ContextPortContractError::NavigationTarget)?;
        Self::try_new(target)
    }

    /// Validates an already-parsed absolute browser target.
    pub fn try_new(target: Url) -> Result<Self, ContextPortContractError> {
        if target.as_str().len() > MAX_CONTEXT_NAVIGATION_TARGET_BYTES
            || !zephium_core::navigation::is_allowed(&target)
        {
            return Err(ContextPortContractError::NavigationTarget);
        }
        Ok(Self(target))
    }

    /// Returns the validated target to the trusted native adapter.
    pub const fn as_url(&self) -> &Url {
        &self.0
    }

    /// Consumes the wrapper at the trusted native adapter boundary.
    pub fn into_url(self) -> Url {
        self.0
    }
}

impl fmt::Debug for ContextNavigationTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ContextNavigationTarget([redacted])")
    }
}

/// Immutable bounded redirect authority for one exact navigation operation.
///
/// The trusted shell derives this allowlist from already-approved run policy;
/// it is never supplied by page content or a model. The initial exact target
/// remains separately bound by [`ContextNavigationRequest`]. This value grants
/// only a possible redirect destination origin and never proves that a native
/// redirect or commit belongs to the requested navigation identity.
#[derive(Clone, Eq, PartialEq)]
pub struct ContextNavigationRedirectPolicy {
    allowed_origins: Vec<SemanticOrigin>,
}

impl ContextNavigationRedirectPolicy {
    /// Constructs a canonical, nonempty redirect-origin allowlist.
    pub fn try_new(
        mut allowed_origins: Vec<SemanticOrigin>,
    ) -> Result<Self, ContextPortContractError> {
        if allowed_origins.is_empty() {
            return Err(ContextPortContractError::EmptyRedirectPolicy);
        }
        if allowed_origins.len() > MAX_CONTEXT_NAVIGATION_REDIRECT_ORIGINS {
            return Err(ContextPortContractError::RedirectOriginLimit);
        }
        allowed_origins.sort();
        if allowed_origins.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(ContextPortContractError::DuplicateRedirectOrigin);
        }
        Ok(Self { allowed_origins })
    }

    /// Constructs a policy that permits redirects only within the target's origin.
    pub fn same_origin(target: &ContextNavigationTarget) -> Result<Self, ContextPortContractError> {
        let origin = SemanticOrigin::parse(target.as_url().as_str())
            .map_err(|_| ContextPortContractError::NavigationTarget)?;
        Self::try_new(vec![origin])
    }

    /// Checks one validated native destination against this immutable scope.
    pub fn allows(&self, target: &ContextNavigationTarget) -> bool {
        SemanticOrigin::parse(target.as_url().as_str())
            .ok()
            .is_some_and(|origin| self.allowed_origins.binary_search(&origin).is_ok())
    }

    /// Number of canonical origins retained by this operation-local scope.
    pub fn origin_count(&self) -> usize {
        self.allowed_origins.len()
    }

    /// Canonical destination origins retained by the trusted policy layer.
    pub fn allowed_origins(&self) -> &[SemanticOrigin] {
        &self.allowed_origins
    }
}

impl fmt::Debug for ContextNavigationRedirectPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextNavigationRedirectPolicy")
            .field("origin_count", &self.origin_count())
            .finish()
    }
}

/// Opaque process-local lease over an existing normal Browse tab.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BorrowedTabLeaseId(NonZeroU64);

impl BorrowedTabLeaseId {
    /// Constructs a nonzero shell-minted lease identity.
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the process-local value to the private native adapter.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Debug for BorrowedTabLeaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BorrowedTabLeaseId([redacted])")
    }
}

/// Shell-authorized source for one context construction.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ContextConstructionSource {
    /// Create a new run-owned context absent from ordinary Browse identity.
    Owned,
    /// Bind an existing normal tab through an already-admitted lease.
    BorrowedTab(BorrowedTabLeaseId),
    /// Create a temporary normal human sign-in handoff context.
    HumanSignInHandoff,
}

impl ContextConstructionSource {
    /// Immutable context kind represented by this source.
    pub const fn kind(self) -> ContextKind {
        match self {
            Self::Owned => ContextKind::Owned,
            Self::BorrowedTab(_) => ContextKind::BorrowedTab,
            Self::HumanSignInHandoff => ContextKind::HumanSignInHandoff,
        }
    }

    /// Returns the closed viewport contract only for a run-owned context.
    pub const fn owned_viewport(self) -> Option<ContextOwnedViewport> {
        match self {
            Self::Owned => Some(ContextOwnedViewport::STANDARD),
            Self::BorrowedTab(_) | Self::HumanSignInHandoff => None,
        }
    }
}

impl fmt::Debug for ContextConstructionSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owned => formatter.write_str("Owned"),
            Self::BorrowedTab(lease) => formatter.debug_tuple("BorrowedTab").field(lease).finish(),
            Self::HumanSignInHandoff => formatter.write_str("HumanSignInHandoff"),
        }
    }
}

/// Exact request to construct one native context binding.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ContextConstructionRequest {
    operation: ContextOperationJoin,
    capabilities: ContextCapabilities,
    profile_lease: ContextProfileLease,
    source: ContextConstructionSource,
}

impl ContextConstructionRequest {
    /// Joins construction, complete capabilities, and the shell-owned source.
    pub fn try_new(
        operation: ContextOperationJoin,
        capabilities: ContextCapabilities,
        profile_lease: ContextProfileLease,
        source: ContextConstructionSource,
    ) -> Result<Self, ContextPortContractError> {
        require_operation(operation, ContextOperationKind::Construct)?;
        let kind = operation.context().identity().kind();
        if source.kind() != kind {
            return Err(ContextPortContractError::ContextKind);
        }
        if capabilities.kind() != kind {
            return Err(ContextPortContractError::CapabilityKind);
        }
        if profile_lease.identity() != operation.context().identity() {
            return Err(ContextPortContractError::ProfileLease);
        }
        Ok(Self {
            operation,
            capabilities,
            profile_lease,
            source,
        })
    }

    /// Exact construction operation and complete lifecycle join.
    pub const fn operation(self) -> ContextOperationJoin {
        self.operation
    }

    /// Complete, kind-checked native capability inventory.
    pub const fn capabilities(self) -> ContextCapabilities {
        self.capabilities
    }

    /// Exact retained lease over the explicitly selected profile.
    pub const fn profile_lease(self) -> ContextProfileLease {
        self.profile_lease
    }

    /// Shell-authorized construction source.
    pub const fn source(self) -> ContextConstructionSource {
        self.source
    }

    /// Fixed native viewport derived from the already-validated source.
    pub const fn owned_viewport(self) -> Option<ContextOwnedViewport> {
        self.source.owned_viewport()
    }
}

impl fmt::Debug for ContextConstructionRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextConstructionRequest")
            .field("operation", &self.operation)
            .field("capabilities", &self.capabilities)
            .field("profile_lease", &self.profile_lease)
            .field("source", &self.source)
            .finish()
    }
}

/// Exact request for one shell-authorized browser navigation.
#[derive(Clone, Eq, PartialEq)]
pub struct ContextNavigationRequest {
    operation: ContextOperationJoin,
    target: ContextNavigationTarget,
    redirect_policy: Option<ContextNavigationRedirectPolicy>,
    document_policy: crate::WorkBrowserDocumentPolicy,
}

impl ContextNavigationRequest {
    /// Binds a validated target to the exact navigation operation.
    pub fn try_new(
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
    ) -> Result<Self, ContextPortContractError> {
        require_operation(operation, ContextOperationKind::Navigate)?;
        Ok(Self {
            operation,
            target,
            redirect_policy: None,
            document_policy: crate::WorkBrowserDocumentPolicy::Exact,
        })
    }

    /// Binds an exact target and immutable redirect scope to one navigation.
    pub fn try_new_with_redirect_policy(
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
        redirect_policy: ContextNavigationRedirectPolicy,
    ) -> Result<Self, ContextPortContractError> {
        require_operation(operation, ContextOperationKind::Navigate)?;
        Ok(Self {
            operation,
            target,
            redirect_policy: Some(redirect_policy),
            document_policy: crate::WorkBrowserDocumentPolicy::Exact,
        })
    }

    /// Freezes trusted finalization authority into this exact operation. This
    /// never authorizes a different load or any redirect.
    pub fn try_new_with_document_policy(
        operation: ContextOperationJoin,
        target: ContextNavigationTarget,
        document_policy: crate::WorkBrowserDocumentPolicy,
    ) -> Result<Self, ContextPortContractError> {
        let mut request = Self::try_new(operation, target)?;
        if document_policy == crate::WorkBrowserDocumentPolicy::InitialQueryFinalization
            || !document_policy.admits_request(request.target())
        {
            return Err(ContextPortContractError::NavigationTarget);
        }
        request.document_policy = document_policy;
        Ok(request)
    }

    /// Operation-specific finalization authority, separate from redirect scope.
    pub const fn document_policy(&self) -> crate::WorkBrowserDocumentPolicy {
        self.document_policy
    }

    /// Exact navigation operation and complete lifecycle join.
    pub const fn operation(&self) -> ContextOperationJoin {
        self.operation
    }

    /// Validated browser target.
    pub const fn target(&self) -> &ContextNavigationTarget {
        &self.target
    }

    /// Immutable redirect authority, or `None` when every redirect is denied.
    pub const fn redirect_policy(&self) -> Option<&ContextNavigationRedirectPolicy> {
        self.redirect_policy.as_ref()
    }

    /// Checks whether a validated target is a permitted redirect destination.
    ///
    /// This is only a policy fact. Native navigation identity and redirect
    /// count still have to be proven by the platform adapter.
    pub fn allows_redirect_target(&self, target: &ContextNavigationTarget) -> bool {
        self.redirect_policy
            .as_ref()
            .is_some_and(|policy| policy.allows(target))
    }
}

impl fmt::Debug for ContextNavigationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextNavigationRequest")
            .field("operation", &self.operation)
            .field("target", &self.target)
            .field("redirect_policy", &self.redirect_policy)
            .field("document_policy", &self.document_policy)
            .finish()
    }
}

/// Exact request for a non-construction, non-navigation lifecycle transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextTransitionRequest {
    operation: ContextOperationJoin,
}

impl ContextTransitionRequest {
    /// Validates that the exact join represents a closed transition class.
    pub fn try_new(operation: ContextOperationJoin) -> Result<Self, ContextPortContractError> {
        if !is_transition(operation.kind()) {
            return Err(ContextPortContractError::OperationKind);
        }
        Ok(Self { operation })
    }

    /// Exact lifecycle transition and complete context join.
    pub const fn operation(self) -> ContextOperationJoin {
        self.operation
    }
}

/// Best-effort revocation request after the functional core advances authority.
///
/// Old callbacks remain unsafe even if native cancellation is refused because
/// their cancellation generation no longer rejoins the current context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextCancellationRequest {
    current: ContextJoin,
}

impl ContextCancellationRequest {
    /// Captures the post-revocation context join.
    pub const fn new(current: ContextJoin) -> Self {
        Self { current }
    }

    /// Current post-revocation authority.
    pub const fn current(self) -> ContextJoin {
        self.current
    }
}

/// Closed request vocabulary accepted by a native context adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextNativeRequest {
    /// Construct a new native binding or bind an admitted borrowed lease.
    Construct(ContextConstructionRequest),
    /// Navigate one exact context to one validated web target.
    Navigate(ContextNavigationRequest),
    /// Apply one exact closed lifecycle transition.
    Transition(ContextTransitionRequest),
    /// Revoke pending native work after run cancellation or human takeover.
    Cancel(ContextCancellationRequest),
}

impl ContextNativeRequest {
    /// Complete context join carried by every request class.
    pub const fn context(&self) -> ContextJoin {
        match self {
            Self::Construct(request) => request.operation().context(),
            Self::Navigate(request) => request.operation().context(),
            Self::Transition(request) => request.operation().context(),
            Self::Cancel(request) => request.current(),
        }
    }
}

/// Platform family that established a construction attestation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextNativePlatform {
    /// Pinned WebKit/WKWebView adapter.
    MacOs,
    /// Pinned WebView2 adapter.
    Windows,
}

/// Closed native attestation for profile binding and extension behavior.
///
/// A trusted adapter emits a variant only after checking the actual native
/// configuration and extension inventory. The paired facts are one enum so a
/// caller cannot combine an owned storage binding with a normal extension
/// principal accidentally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextConstructionProof {
    /// Selected macOS profile data store, no extension controller, extension,
    /// or ordinary Zephium script principal attached; the sole user script is
    /// the exact private semantic-runtime program in its dedicated world.
    MacOsOwnedSelectedProfileExtensionFree,
    /// Selected Windows profile used only after its extension inventory was
    /// proven empty for this construction.
    WindowsOwnedSelectedProfileEmptyInventory,
    /// Stable selected-profile Windows automation subprofile whose extension
    /// inventory was proven empty for this construction.
    WindowsOwnedAutomationSubprofileEmptyInventory,
    /// Existing macOS Browse tab retaining its normal extension behavior.
    MacOsBorrowedTabNormalExtensions,
    /// Existing Windows Browse tab retaining its normal extension behavior.
    WindowsBorrowedTabNormalExtensions,
    /// Temporary macOS human handoff retaining normal extension behavior.
    MacOsHumanSignInHandoffNormalExtensions,
    /// Temporary Windows human handoff retaining normal extension behavior.
    WindowsHumanSignInHandoffNormalExtensions,
}

impl ContextConstructionProof {
    /// Platform that produced the attestation.
    pub const fn platform(self) -> ContextNativePlatform {
        match self {
            Self::MacOsOwnedSelectedProfileExtensionFree
            | Self::MacOsBorrowedTabNormalExtensions
            | Self::MacOsHumanSignInHandoffNormalExtensions => ContextNativePlatform::MacOs,
            Self::WindowsOwnedSelectedProfileEmptyInventory
            | Self::WindowsOwnedAutomationSubprofileEmptyInventory
            | Self::WindowsBorrowedTabNormalExtensions
            | Self::WindowsHumanSignInHandoffNormalExtensions => ContextNativePlatform::Windows,
        }
    }

    /// Context kind for which this paired attestation is truthful.
    pub const fn kind(self) -> ContextKind {
        match self {
            Self::MacOsOwnedSelectedProfileExtensionFree
            | Self::WindowsOwnedSelectedProfileEmptyInventory
            | Self::WindowsOwnedAutomationSubprofileEmptyInventory => ContextKind::Owned,
            Self::MacOsBorrowedTabNormalExtensions | Self::WindowsBorrowedTabNormalExtensions => {
                ContextKind::BorrowedTab
            }
            Self::MacOsHumanSignInHandoffNormalExtensions
            | Self::WindowsHumanSignInHandoffNormalExtensions => ContextKind::HumanSignInHandoff,
        }
    }
}

/// Closed, redaction-safe reason a native request did not apply.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ContextPortFailure {
    /// The pinned native runtime cannot support the requested invariant.
    #[error("native context operation is unsupported")]
    Unsupported,
    /// A bounded native queue or physical resource ceiling is full.
    #[error("native context resources are exhausted")]
    ResourceExhausted,
    /// The exact selected profile is unavailable.
    #[error("selected context profile is unavailable")]
    ProfileUnavailable,
    /// The exact selected profile is already exclusively leased.
    #[error("selected context profile is busy")]
    ProfileBusy,
    /// Extension-free construction could not be proven.
    #[error("context extension isolation is unproven")]
    ExtensionIsolationUnproven,
    /// A bounded platform cookie transfer failed.
    #[error("context cookie transfer failed")]
    CookieTransferFailed,
    /// The native API synchronously or asynchronously refused the operation.
    #[error("native context operation was refused")]
    NativeRefused,
    /// Cancellation terminated the operation before it applied.
    #[error("native context operation was cancelled")]
    Cancelled,
    /// The bounded native deadline elapsed.
    #[error("native context operation timed out")]
    TimedOut,
    /// Native authority no longer matches the request join.
    #[error("native context operation is stale")]
    Stale,
    /// Process teardown permanently sealed the native adapter.
    #[error("native context adapter is shutting down")]
    Shutdown,
}

/// Synchronous admission result from the bounded native task queue.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextDispatch {
    /// The exact request entered the owning native executor.
    Scheduled,
    /// The adapter synchronously refused the exact request.
    Rejected(ContextPortFailure),
    /// The pinned platform does not implement the request class.
    Unsupported,
}

/// Synchronous result of the atomic native shutdown seal and audit admission.
///
/// Unlike [`ContextDispatch`], every variant means that non-audit admission is
/// permanently sealed. A scheduled audit owns one exact terminal
/// [`ContextNativeEvent::ShutdownAuditSettled`] obligation. If audit admission
/// or outer dispatch fails, callers may issue a bounded read-only
/// [`AgentBrowserPort::audit_resources`] request after retained work drains.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContextShutdownDispatch {
    /// The adapter was sealed and the exact shutdown audit entered its executor.
    AuditScheduled,
    /// The adapter was sealed but no terminal shutdown-audit event is owed.
    SealedWithoutAudit(ContextPortFailure),
}

/// Exact asynchronous settlement of initial native construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextConstructionSettlement {
    operation: ContextOperationJoin,
    outcome: Result<ContextConstructionProof, ContextPortFailure>,
}

impl ContextConstructionSettlement {
    /// Validates construction operation class and any successful attestation.
    pub fn try_new(
        operation: ContextOperationJoin,
        outcome: Result<ContextConstructionProof, ContextPortFailure>,
    ) -> Result<Self, ContextPortContractError> {
        require_operation(operation, ContextOperationKind::Construct)?;
        if let Ok(proof) = outcome {
            if proof.kind() != operation.context().identity().kind() {
                return Err(ContextPortContractError::ContextKind);
            }
        }
        Ok(Self { operation, outcome })
    }

    /// Exact construction operation and complete context join.
    pub const fn operation(self) -> ContextOperationJoin {
        self.operation
    }

    /// Successful native attestation or closed refusal.
    pub const fn outcome(self) -> Result<ContextConstructionProof, ContextPortFailure> {
        self.outcome
    }
}

/// Exact asynchronous settlement of a requested navigation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextNavigationSettlement {
    operation: ContextOperationJoin,
    outcome: Result<ContextNavigationTarget, ContextPortFailure>,
}

impl ContextNavigationSettlement {
    /// Validates the navigation operation and binds its committed target.
    pub fn try_new(
        operation: ContextOperationJoin,
        outcome: Result<ContextNavigationTarget, ContextPortFailure>,
    ) -> Result<Self, ContextPortContractError> {
        require_operation(operation, ContextOperationKind::Navigate)?;
        Ok(Self { operation, outcome })
    }

    /// Exact navigation operation and complete context join.
    pub const fn operation(&self) -> ContextOperationJoin {
        self.operation
    }

    /// Exact committed target or closed refusal.
    pub const fn outcome(&self) -> &Result<ContextNavigationTarget, ContextPortFailure> {
        &self.outcome
    }
}

/// Exact asynchronous settlement of a closed lifecycle transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextTransitionSettlement {
    operation: ContextOperationJoin,
    outcome: Result<(), ContextPortFailure>,
}

impl ContextTransitionSettlement {
    /// Validates a non-construction, non-navigation transition result.
    pub fn try_new(
        operation: ContextOperationJoin,
        outcome: Result<(), ContextPortFailure>,
    ) -> Result<Self, ContextPortContractError> {
        ContextTransitionRequest::try_new(operation)?;
        Ok(Self { operation, outcome })
    }

    /// Exact lifecycle operation and complete context join.
    pub const fn operation(self) -> ContextOperationJoin {
        self.operation
    }

    /// Applied transition or closed refusal.
    pub const fn outcome(self) -> Result<(), ContextPortFailure> {
        self.outcome
    }
}

/// Exact committed navigation that was not initiated by a shell request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextNavigationReplacement {
    prior: ContextJoin,
    target: ContextNavigationTarget,
}

impl ContextNavigationReplacement {
    /// Joins the native commit to the exact document it replaced.
    pub const fn new(prior: ContextJoin, target: ContextNavigationTarget) -> Self {
        Self { prior, target }
    }

    /// Complete join for the document that was replaced.
    pub const fn prior(&self) -> ContextJoin {
        self.prior
    }

    /// Exact committed replacement target.
    pub const fn target(&self) -> &ContextNavigationTarget {
        &self.target
    }
}

/// Exact native renderer-loss notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextRendererLoss {
    prior: ContextJoin,
}

impl ContextRendererLoss {
    /// Captures the last native authority known before renderer loss.
    pub const fn new(prior: ContextJoin) -> Self {
        Self { prior }
    }

    /// Complete pre-loss context join.
    pub const fn prior(self) -> ContextJoin {
        self.prior
    }
}

/// Exact asynchronous result of best-effort native cancellation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextCancellationSettlement {
    current: ContextJoin,
    outcome: Result<(), ContextPortFailure>,
}

impl ContextCancellationSettlement {
    /// Binds cancellation outcome to the post-revocation join.
    pub const fn new(current: ContextJoin, outcome: Result<(), ContextPortFailure>) -> Self {
        Self { current, outcome }
    }

    /// Current post-revocation authority.
    pub const fn current(self) -> ContextJoin {
        self.current
    }

    /// Native cancellation outcome; stale old work remains unauthorized either way.
    pub const fn outcome(self) -> Result<(), ContextPortFailure> {
        self.outcome
    }
}

/// Nonzero process-local correlation for one native resource audit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ContextResourceAuditId(NonZeroU64);

impl ContextResourceAuditId {
    /// Constructs a nonzero shell-minted audit identity.
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Returns the process-local correlation value.
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// Requested native resource counts before invariant validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextNativeResourceCounts {
    /// Context-to-native bindings retained by the adapter.
    pub known_bindings: u8,
    /// Resident native page views.
    pub resident_views: u8,
    /// Owned or handoff native reservations.
    pub owned_reservations: u8,
    /// Leases over existing normal Browse tabs.
    pub borrowed_leases: u8,
    /// Interactive native surfaces currently presented.
    pub visible_surfaces: u8,
    /// Resident views in the platform's suspended state.
    pub suspended_views: u8,
    /// Context-bound logical operations awaiting settlement.
    pub pending_operations: u8,
    /// Viewport captures reserved, executing, or awaiting physical settlement.
    pub pending_captures: u8,
    /// Tasks retained by the native executor but not yet running.
    pub queued_tasks: u8,
}

/// Validated privacy-preserving native resource snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextNativeResourceSnapshot(ContextNativeResourceCounts);

impl ContextNativeResourceSnapshot {
    /// Validates every count against the process ceilings and relationships.
    pub fn try_new(counts: ContextNativeResourceCounts) -> Result<Self, ContextPortContractError> {
        let live_limit =
            u8::try_from(MAX_LIVE_CONTEXTS).map_err(|_| ContextPortContractError::ResourceLimit)?;
        let queue_limit = u8::try_from(MAX_PENDING_NATIVE_CONTEXT_TASKS)
            .map_err(|_| ContextPortContractError::ResourceLimit)?;
        let capture_limit = u8::try_from(MAX_PENDING_SEMANTIC_SCREENSHOTS)
            .map_err(|_| ContextPortContractError::ResourceLimit)?;
        if [
            counts.known_bindings,
            counts.resident_views,
            counts.owned_reservations,
            counts.borrowed_leases,
            counts.visible_surfaces,
            counts.suspended_views,
            counts.pending_operations,
        ]
        .into_iter()
        .any(|count| count > live_limit)
            || counts.queued_tasks > queue_limit
            || counts.pending_captures > capture_limit
        {
            return Err(ContextPortContractError::ResourceLimit);
        }
        let partitioned = counts
            .owned_reservations
            .checked_add(counts.borrowed_leases)
            .ok_or(ContextPortContractError::ResourceInvariant)?;
        if partitioned != counts.known_bindings
            || counts.resident_views > counts.known_bindings
            || counts.visible_surfaces > counts.resident_views
            || counts.suspended_views > counts.resident_views
            || counts.pending_operations > counts.known_bindings
        {
            return Err(ContextPortContractError::ResourceInvariant);
        }
        Ok(Self(counts))
    }

    /// Complete validated count cohort.
    pub const fn counts(self) -> ContextNativeResourceCounts {
        self.0
    }
}

/// Exact asynchronous response to one native resource audit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextResourceAuditSettlement {
    audit: ContextResourceAuditId,
    outcome: Result<ContextNativeResourceSnapshot, ContextPortFailure>,
}

/// Exact asynchronous response to the audit that atomically sealed admission.
///
/// This separate type prevents an earlier ordinary audit from being mistaken
/// for the shutdown linearization point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextShutdownAuditSettlement {
    audit: ContextResourceAuditId,
    outcome: Result<ContextNativeResourceSnapshot, ContextPortFailure>,
}

impl ContextShutdownAuditSettlement {
    /// Binds one validated snapshot or refusal to the exact shutdown audit.
    pub const fn new(
        audit: ContextResourceAuditId,
        outcome: Result<ContextNativeResourceSnapshot, ContextPortFailure>,
    ) -> Self {
        Self { audit, outcome }
    }

    /// Exact shutdown-audit correlation identity.
    pub const fn audit(self) -> ContextResourceAuditId {
        self.audit
    }

    /// Validated count cohort or closed refusal.
    pub const fn outcome(self) -> Result<ContextNativeResourceSnapshot, ContextPortFailure> {
        self.outcome
    }
}

impl ContextResourceAuditSettlement {
    /// Binds one validated snapshot or refusal to the exact audit request.
    pub const fn new(
        audit: ContextResourceAuditId,
        outcome: Result<ContextNativeResourceSnapshot, ContextPortFailure>,
    ) -> Self {
        Self { audit, outcome }
    }

    /// Exact resource-audit correlation identity.
    pub const fn audit(self) -> ContextResourceAuditId {
        self.audit
    }

    /// Validated count cohort or closed refusal.
    pub const fn outcome(self) -> Result<ContextNativeResourceSnapshot, ContextPortFailure> {
        self.outcome
    }
}

/// Closed event vocabulary emitted by the native context adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextNativeEvent {
    /// Initial native construction reached a terminal result.
    ConstructionSettled(ContextConstructionSettlement),
    /// Shell-requested navigation committed or reached a terminal refusal.
    NavigationSettled(ContextNavigationSettlement),
    /// A closed non-navigation lifecycle transition settled.
    TransitionSettled(ContextTransitionSettlement),
    /// Page-initiated, redirect, history, or human navigation replaced a document.
    NavigationReplaced(ContextNavigationReplacement),
    /// The exact native renderer disappeared.
    RendererLost(ContextRendererLoss),
    /// Best-effort native cancellation settled.
    CancellationSettled(ContextCancellationSettlement),
    /// One bounded native-only cookie transfer reached a terminal outcome.
    CookieTransferSettled(Box<ContextCookieTransferSettlement>),
    /// One bounded native resource audit settled.
    ResourceAuditSettled(ContextResourceAuditSettlement),
    /// The audit admitted at the atomic native shutdown seal settled.
    ShutdownAuditSettled(ContextShutdownAuditSettlement),
    /// One fixed isolated-world semantic invocation reached a terminal result.
    SemanticRuntimeSettled(Box<SemanticRuntimeSettlement>),
}

/// One bounded PNG of a hosted Work page at a fixed reduced width. Frames are
/// transient, per resource, and replaced in place; they carry no authority.
#[derive(Clone, Debug)]
pub struct WorkBrowserFrame {
    /// Monotonic per resource; a newer frame replaces an older one.
    pub generation: u64,
    /// Encoded width in pixels.
    pub width: u32,
    /// Encoded height in pixels.
    pub height: u32,
    /// Metadata-free PNG bytes under the frame budget.
    pub png: std::sync::Arc<Vec<u8>>,
}

/// Move-only terminal callback for one admitted native viewport capture.
///
/// A port returning [`ContextDispatch::Scheduled`] must invoke this exactly
/// once. A synchronous `Rejected` or `Unsupported` result transfers no
/// settlement obligation; callers terminally cancel their retained pending
/// half from the dispatch result itself.
pub type SemanticScreenshotNativeCompletion = Box<
    dyn FnOnce(Result<SemanticScreenshotNativeCapture, SemanticScreenshotNativeFailure>)
        + Send
        + 'static,
>;

/// Move-only terminal callback for one admitted native semantic action.
///
/// A port returning [`ContextDispatch::Scheduled`] must invoke this exactly
/// once with a settlement created by consuming the exact request. Synchronous
/// `Rejected` or `Unsupported` results transfer no callback obligation.
pub type SemanticActionNativeCompletion =
    Box<dyn FnOnce(SemanticActionNativeSettlement) + Send + 'static>;

/// Trusted imperative boundary implemented by one bounded platform adapter.
///
/// Implementations own native handles and private identity mappings. They must
/// never project owned contexts into tab/session/extension inventories and
/// must retain no queue, worker, timer, or page when no contexts exist.
pub trait AgentBrowserPort: Send + Sync {
    /// Negative-only routing check: whether the original owner can attempt a
    /// public survey in fresh native storage. True never authorizes pixels.
    fn work_screenshot_public_candidate(
        &self,
        _lease: &crate::WorkBrowserExecutionLease,
        _context: crate::ContextJoin,
    ) -> bool {
        false
    }

    /// Claims the original native capture once, under this exact resource lease.
    /// Public mode additionally requires actual fresh-store lineage. This does
    /// not grant consent or disclose pixels, and never uses another native port.
    fn claim_work_screenshot_source(
        &self,
        _lease: &crate::WorkBrowserExecutionLease,
        _image: &crate::SemanticScreenshot,
    ) -> Option<std::sync::Arc<dyn crate::SemanticScreenshotSource>> {
        None
    }

    /// Whether this bound native adapter implements the production history
    /// retention/reactivation contract on the running platform version.
    fn supports_work_resource_history_back(&self) -> bool {
        false
    }

    /// Already policy-authorized semantic effect on one retained execution
    /// lease. An adapter must own the exact recipe, native callback and physical
    /// callback-return debt; it must not route this through legacy tab contexts.
    /// Unsupported adapters preserve the original request without a callback.
    fn work_resource_act(
        &self,
        request: crate::WorkBrowserActionRequest,
        _completion: crate::WorkBrowserActionCompletionCallback,
    ) -> crate::WorkBrowserActionDispatch {
        crate::WorkBrowserActionDispatch::Rejected {
            request: Box::new(request),
            failure: ContextPortFailure::Unsupported,
        }
    }

    /// Policy-bound exact document transition on an existing execution lease.
    /// Unsupported adapters return the original request without a callback.
    fn work_resource_navigate(
        &self,
        request: crate::WorkBrowserNavigationRequest,
        _completion: crate::WorkBrowserNavigationCompletionCallback,
    ) -> crate::WorkBrowserNavigationDispatch {
        crate::WorkBrowserNavigationDispatch::Rejected {
            request: Box::new(request),
            failure: ContextPortFailure::Unsupported,
        }
    }

    /// Policy-bound exact predecessor traversal on a retained execution lease.
    /// Unsupported adapters return the original request without a callback.
    fn work_resource_back(
        &self,
        request: crate::WorkBrowserHistoryBackRequest,
        _completion: crate::WorkBrowserHistoryBackCompletionCallback,
    ) -> crate::WorkBrowserHistoryBackDispatch {
        crate::WorkBrowserHistoryBackDispatch::Rejected {
            request: Box::new(request),
            failure: ContextPortFailure::Unsupported,
        }
    }
    /// Product resource-lifetime seam, unsupported until an adapter proves
    /// persistent resource ownership and exact scoped lease drain. This does
    /// not reuse legacy cancellation, unseal a port or grant a model tool.
    fn work_resource_lifecycle(
        &self,
        request: crate::WorkBrowserResourceRequest,
        _completion: crate::WorkBrowserResourceCompletionCallback,
    ) -> crate::WorkBrowserResourceDispatch {
        crate::WorkBrowserResourceDispatch::Rejected {
            request: Box::new(request),
            failure: ContextPortFailure::Unsupported,
        }
    }

    /// Bounded initial observation through a separately admitted Work resource
    /// lease. It never enters legacy context/action/navigation routing.
    fn work_resource_observe(
        &self,
        request: crate::WorkBrowserObservationRequest,
        _completion: crate::WorkBrowserObservationCompletionCallback,
    ) -> crate::WorkBrowserObservationDispatch {
        crate::WorkBrowserObservationDispatch::Rejected {
            request: Box::new(request),
            failure: ContextPortFailure::Unsupported,
        }
    }

    /// Release-excluded fixed native diagnostic, unsupported by default.
    /// This is not a production rendering API or model-visible tool.
    #[cfg(feature = "probe-harness")]
    fn probe_foreground_rendering(
        &self,
        _request: crate::ForegroundRenderingProbeRequest,
        _completion: crate::ForegroundRenderingProbeCompletion,
    ) -> ContextDispatch {
        ContextDispatch::Unsupported
    }

    /// Attempts to admit one exact lifecycle request to the owning native executor.
    fn dispatch(&self, request: ContextNativeRequest) -> ContextDispatch;

    /// Attempts to admit one bounded native-only one-way cookie transfer.
    fn transfer_cookies(&self, request: ContextCookieTransferRequest) -> ContextDispatch;

    /// Attempts to admit one privacy-preserving native resource audit.
    ///
    /// Read-only audits remain bounded and may be admitted after shutdown is
    /// sealed so an owner can verify asynchronous drain. No other request
    /// class may reopen behind that seal.
    fn audit_resources(&self, audit: ContextResourceAuditId) -> ContextDispatch;

    /// Atomically seals non-audit admission and attempts one resource audit.
    ///
    /// Every return value means the seal is permanent. `AuditScheduled`
    /// transfers exactly one `ShutdownAuditSettled` event obligation; a
    /// `SealedWithoutAudit` result transfers none. A repeated call is closed as
    /// `SealedWithoutAudit(Shutdown)` and cannot create another barrier event.
    fn seal_for_shutdown(&self, audit: ContextResourceAuditId) -> ContextShutdownDispatch;

    /// Attempts to admit one already-encoded fixed semantic-runtime invocation.
    fn invoke_semantic(&self, invocation: SemanticRuntimeInvocation) -> ContextDispatch;

    /// Attempts one already-authorized closed semantic action recipe.
    fn execute_semantic_action(
        &self,
        request: SemanticActionNativeRequest,
        completion: SemanticActionNativeCompletion,
    ) -> ContextDispatch;

    /// The newest bounded frame of one hosted Work page, for the person
    /// watching the canvas. Never model-visible; `None` when nothing is hosted.
    fn latest_work_frame(
        &self,
        _resource: &crate::WorkBrowserResourceJoin,
    ) -> Option<std::sync::Arc<WorkBrowserFrame>> {
        None
    }

    /// Attempts one bounded viewport capture outside the cloneable event bus.
    fn capture_semantic_screenshot(
        &self,
        request: SemanticScreenshotNativeRequest,
        completion: SemanticScreenshotNativeCompletion,
    ) -> ContextDispatch;
}

fn require_operation(
    operation: ContextOperationJoin,
    expected: ContextOperationKind,
) -> Result<(), ContextPortContractError> {
    if operation.kind() == expected {
        Ok(())
    } else {
        Err(ContextPortContractError::OperationKind)
    }
}

const fn is_transition(kind: ContextOperationKind) -> bool {
    matches!(
        kind,
        ContextOperationKind::Suspend
            | ContextOperationKind::Resume
            | ContextOperationKind::Recover
            | ContextOperationKind::Show
            | ContextOperationKind::Hide
            | ContextOperationKind::BeginHumanControl
            | ContextOperationKind::EndHumanControl
            | ContextOperationKind::Close
            | ContextOperationKind::Adopt
            | ContextOperationKind::Release
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContextCapability, ContextId, ContextIdentity, ContextOperationId, ContextProfileLeaseId,
        ContextProfileLeasePurpose, ContextProfileLeaseRegistry, ContextProfileStorageClass,
        ContextRegistry, ContextRunId,
    };
    use zephium_core::ids::ProfileId;

    const OWNED_CAPABILITIES: [ContextCapability; 3] = [
        ContextCapability::Navigate,
        ContextCapability::Present,
        ContextCapability::HumanControl,
    ];

    fn identity(value: u128, kind: ContextKind) -> ContextIdentity {
        ContextIdentity::new(
            ContextId::from_raw(value),
            ContextRunId::from_raw(20),
            ProfileId::from(30),
            kind,
        )
    }

    fn capabilities(kind: ContextKind) -> ContextCapabilities {
        let capabilities: &[ContextCapability] = match kind {
            ContextKind::Owned => &OWNED_CAPABILITIES,
            ContextKind::BorrowedTab => &[ContextCapability::Navigate],
            ContextKind::HumanSignInHandoff => &[ContextCapability::HumanControl],
        };
        ContextCapabilities::try_new(kind, capabilities).expect("capabilities")
    }

    fn operation(value: u64) -> ContextOperationId {
        ContextOperationId::new(value).expect("operation")
    }

    fn profile_lease(identity: ContextIdentity, value: u64) -> ContextProfileLease {
        let purpose = match identity.kind() {
            ContextKind::Owned => ContextProfileLeasePurpose::Owned,
            ContextKind::BorrowedTab => ContextProfileLeasePurpose::BorrowedTab,
            ContextKind::HumanSignInHandoff => ContextProfileLeasePurpose::HumanSignInHandoff,
        };
        ContextProfileLeaseRegistry::new()
            .acquire(
                ContextProfileLeaseId::new(value).expect("profile lease id"),
                identity,
                ContextProfileStorageClass::Durable,
                purpose,
            )
            .expect("profile lease")
    }

    fn constructing(kind: ContextKind) -> (ContextRegistry, ContextOperationJoin) {
        let identity = identity(10, kind);
        let mut registry = ContextRegistry::new();
        registry
            .reserve(identity, capabilities(kind))
            .expect("reserve");
        let join = registry
            .begin_context(identity.id(), operation(1))
            .expect("begin");
        (registry, join)
    }

    fn ready_owned() -> ContextRegistry {
        let (mut registry, construction) = constructing(ContextKind::Owned);
        registry
            .settle_construction(
                construction.context().identity().id(),
                construction,
                crate::ContextSettlement::Applied,
            )
            .expect("constructed");
        registry
    }

    #[test]
    fn construction_source_capabilities_and_proof_are_kind_exact() {
        let (_, owned) = constructing(ContextKind::Owned);
        assert_eq!(
            ContextConstructionRequest::try_new(
                owned,
                capabilities(ContextKind::Owned),
                profile_lease(owned.context().identity(), 1),
                ContextConstructionSource::BorrowedTab(BorrowedTabLeaseId::new(1).expect("lease")),
            ),
            Err(ContextPortContractError::ContextKind)
        );
        assert_eq!(
            ContextConstructionRequest::try_new(
                owned,
                capabilities(ContextKind::BorrowedTab),
                profile_lease(owned.context().identity(), 1),
                ContextConstructionSource::Owned,
            ),
            Err(ContextPortContractError::CapabilityKind)
        );
        assert_eq!(
            ContextConstructionRequest::try_new(
                owned,
                capabilities(ContextKind::Owned),
                profile_lease(identity(11, ContextKind::Owned), 2),
                ContextConstructionSource::Owned,
            ),
            Err(ContextPortContractError::ProfileLease)
        );
        assert_eq!(
            ContextConstructionSettlement::try_new(
                owned,
                Ok(ContextConstructionProof::WindowsBorrowedTabNormalExtensions),
            ),
            Err(ContextPortContractError::ContextKind)
        );
        let settlement = ContextConstructionSettlement::try_new(
            owned,
            Ok(ContextConstructionProof::MacOsOwnedSelectedProfileExtensionFree),
        )
        .expect("proof");
        assert_eq!(
            settlement.outcome().expect("attestation").platform(),
            ContextNativePlatform::MacOs
        );
    }

    #[test]
    fn owned_viewport_is_fixed_and_source_exact() {
        let viewport = ContextOwnedViewport::STANDARD;
        assert_eq!(viewport.width(), 1_280);
        assert_eq!(viewport.height(), 800);
        assert_eq!(viewport.logical_area(), 1_024_000);
        assert_eq!(
            ContextConstructionSource::Owned.owned_viewport(),
            Some(viewport)
        );
        assert_eq!(
            ContextConstructionSource::BorrowedTab(BorrowedTabLeaseId::new(1).expect("lease"))
                .owned_viewport(),
            None
        );
        assert_eq!(
            ContextConstructionSource::HumanSignInHandoff.owned_viewport(),
            None
        );

        let (_, operation) = constructing(ContextKind::Owned);
        let request = ContextConstructionRequest::try_new(
            operation,
            capabilities(ContextKind::Owned),
            profile_lease(operation.context().identity(), 1),
            ContextConstructionSource::Owned,
        )
        .expect("request");
        assert_eq!(request.owned_viewport(), Some(viewport));
    }

    #[test]
    fn navigation_targets_reject_secrets_local_access_and_internal_principals() {
        for target in [
            "javascript:alert(1)",
            "file:///private/secret",
            "https://user:secret@example.test/",
            "http://tauri.localhost/index.html",
            "about:config",
        ] {
            assert_eq!(
                ContextNavigationTarget::parse(target),
                Err(ContextPortContractError::NavigationTarget),
                "accepted {target}"
            );
        }
        let target = ContextNavigationTarget::parse("https://example.test/path?token=secret")
            .expect("ordinary web target");
        assert_eq!(format!("{target:?}"), "ContextNavigationTarget([redacted])");
        assert!(!format!("{target:?}").contains("secret"));
    }

    #[test]
    fn trusted_navigation_target_budget_bounds_raw_and_serialized_urls() {
        let prefix = "https://example.test/";
        let at_bound = format!(
            "{prefix}{}",
            "x".repeat(MAX_CONTEXT_NAVIGATION_TARGET_BYTES - prefix.len())
        );
        assert!(ContextNavigationTarget::parse(&at_bound).is_ok());
        assert!(ContextNavigationTarget::try_new(Url::parse(&at_bound).unwrap()).is_ok());
        let oversized = format!("{at_bound}x");
        assert!(ContextNavigationTarget::parse(&oversized).is_err());
        assert!(ContextNavigationTarget::try_new(Url::parse(&oversized).unwrap()).is_err());

        // Parsing strips this newline, but raw native-port input is still bounded.
        let normalized = format!("{at_bound}\n");
        assert_eq!(Url::parse(&normalized).unwrap().as_str(), at_bound);
        assert!(ContextNavigationTarget::parse(&normalized).is_err());

        // UTF-8 input fits while its percent-encoded serialized target does not.
        let expanded = format!(
            "{prefix}{}",
            "é".repeat((MAX_CONTEXT_NAVIGATION_TARGET_BYTES - prefix.len()) / 2)
        );
        assert!(expanded.len() <= MAX_CONTEXT_NAVIGATION_TARGET_BYTES);
        let parsed = Url::parse(&expanded).unwrap();
        assert!(parsed.as_str().len() > MAX_CONTEXT_NAVIGATION_TARGET_BYTES);
        assert!(zephium_core::navigation::is_allowed(&parsed));
        assert!(ContextNavigationTarget::parse(&expanded).is_err());
        assert!(ContextNavigationTarget::try_new(parsed).is_err());
    }

    #[test]
    fn redirect_policy_is_canonical_bounded_and_redacted() {
        let target = ContextNavigationTarget::parse("https://example.test/start?secret=value")
            .expect("target");
        let policy = ContextNavigationRedirectPolicy::same_origin(&target).expect("same origin");
        assert!(policy
            .allows(&ContextNavigationTarget::parse("https://example.test/final").expect("final")));
        assert!(!policy.allows(
            &ContextNavigationTarget::parse("http://example.test/final").expect("downgrade")
        ));
        assert_eq!(policy.origin_count(), 1);
        assert_eq!(
            format!("{policy:?}"),
            "ContextNavigationRedirectPolicy { origin_count: 1 }"
        );
        assert!(!format!("{policy:?}").contains("secret"));

        let first = SemanticOrigin::parse("https://first.test/").expect("first origin");
        let second = SemanticOrigin::parse("https://second.test/").expect("second origin");
        let forward = ContextNavigationRedirectPolicy::try_new(vec![first.clone(), second.clone()])
            .expect("forward policy");
        let reversed =
            ContextNavigationRedirectPolicy::try_new(vec![second, first]).expect("reversed policy");
        assert_eq!(forward, reversed);

        assert_eq!(
            ContextNavigationRedirectPolicy::try_new(Vec::new()),
            Err(ContextPortContractError::EmptyRedirectPolicy)
        );
        let duplicate = SemanticOrigin::parse("https://duplicate.test/path").expect("origin");
        assert_eq!(
            ContextNavigationRedirectPolicy::try_new(vec![duplicate.clone(), duplicate]),
            Err(ContextPortContractError::DuplicateRedirectOrigin)
        );
        let too_many = (0..=MAX_CONTEXT_NAVIGATION_REDIRECT_ORIGINS)
            .map(|index| {
                SemanticOrigin::parse(&format!("https://origin-{index}.test/")).expect("origin")
            })
            .collect();
        assert_eq!(
            ContextNavigationRedirectPolicy::try_new(too_many),
            Err(ContextPortContractError::RedirectOriginLimit)
        );
    }

    #[test]
    fn navigation_request_keeps_redirect_authority_explicit() {
        let mut registry = ready_owned();
        let context = identity(10, ContextKind::Owned).id();
        let navigation = registry
            .begin_navigation(context, operation(2))
            .expect("navigation");
        let requested =
            ContextNavigationTarget::parse("https://start.test/path").expect("requested");
        let redirected =
            ContextNavigationTarget::parse("https://final.test/path").expect("redirected");
        let policy = ContextNavigationRedirectPolicy::try_new(vec![SemanticOrigin::parse(
            "https://final.test/",
        )
        .expect("origin")])
        .expect("policy");

        let exact = ContextNavigationRequest::try_new(navigation, requested.clone())
            .expect("exact request");
        assert!(exact.redirect_policy().is_none());
        assert_eq!(
            exact.document_policy(),
            crate::WorkBrowserDocumentPolicy::Exact
        );
        assert!(!exact.allows_redirect_target(&redirected));

        let finalized = ContextNavigationRequest::try_new_with_document_policy(
            navigation,
            requested.clone(),
            crate::WorkBrowserDocumentPolicy::DocumentQueryFinalization,
        )
        .unwrap();
        assert_eq!(finalized.target(), &requested);
        assert!(finalized.redirect_policy().is_none());
        assert!(!finalized.allows_redirect_target(&redirected));
        assert!(ContextNavigationRequest::try_new_with_document_policy(
            navigation,
            requested.clone(),
            crate::WorkBrowserDocumentPolicy::InitialQueryFinalization,
        )
        .is_err());
        for invalid in [
            "http://start.test/path",
            "https://start.test/path?x=1",
            "https://start.test/path#fragment",
        ] {
            assert!(ContextNavigationRequest::try_new_with_document_policy(
                navigation,
                ContextNavigationTarget::parse(invalid).unwrap(),
                crate::WorkBrowserDocumentPolicy::DocumentQueryFinalization,
            )
            .is_err());
        }

        let scoped =
            ContextNavigationRequest::try_new_with_redirect_policy(navigation, requested, policy)
                .expect("redirect request");
        assert_eq!(
            scoped.redirect_policy().map(|value| value.origin_count()),
            Some(1)
        );
        assert!(scoped.allows_redirect_target(&redirected));
        assert_eq!(
            scoped.document_policy(),
            crate::WorkBrowserDocumentPolicy::Exact
        );
        assert!(!format!("{scoped:?}").contains("final.test"));
    }

    #[test]
    fn lifecycle_request_classes_cannot_be_substituted() {
        let mut registry = ready_owned();
        let context = identity(10, ContextKind::Owned).id();
        let navigation = registry
            .begin_navigation(context, operation(2))
            .expect("navigation");
        assert_eq!(
            ContextTransitionRequest::try_new(navigation),
            Err(ContextPortContractError::OperationKind)
        );
        assert!(ContextNavigationRequest::try_new(
            navigation,
            ContextNavigationTarget::parse("https://example.test/").expect("target")
        )
        .is_ok());
        let (_, construction) = constructing(ContextKind::Owned);
        assert_eq!(
            ContextNavigationSettlement::try_new(
                construction,
                Err(ContextPortFailure::NativeRefused),
            ),
            Err(ContextPortContractError::OperationKind)
        );
    }

    #[test]
    fn native_resource_snapshots_are_bounded_and_consistent() {
        let valid = ContextNativeResourceCounts {
            known_bindings: 3,
            resident_views: 2,
            owned_reservations: 2,
            borrowed_leases: 1,
            visible_surfaces: 1,
            suspended_views: 1,
            pending_operations: 2,
            pending_captures: 2,
            queued_tasks: 4,
        };
        assert_eq!(
            ContextNativeResourceSnapshot::try_new(valid)
                .expect("snapshot")
                .counts(),
            valid
        );
        let detached_capture = ContextNativeResourceCounts {
            known_bindings: 0,
            resident_views: 0,
            owned_reservations: 0,
            borrowed_leases: 0,
            visible_surfaces: 0,
            suspended_views: 0,
            pending_operations: 0,
            pending_captures: 2,
            queued_tasks: 0,
        };
        assert_eq!(
            ContextNativeResourceSnapshot::try_new(detached_capture)
                .expect("physical capture debt outlives logical binding")
                .counts(),
            detached_capture
        );
        assert_eq!(
            ContextNativeResourceSnapshot::try_new(ContextNativeResourceCounts {
                known_bindings: 2,
                ..valid
            }),
            Err(ContextPortContractError::ResourceInvariant)
        );
        assert_eq!(
            ContextNativeResourceSnapshot::try_new(ContextNativeResourceCounts {
                queued_tasks: u8::try_from(MAX_PENDING_NATIVE_CONTEXT_TASKS + 1)
                    .expect("small ceiling"),
                ..valid
            }),
            Err(ContextPortContractError::ResourceLimit)
        );
        assert_eq!(
            ContextNativeResourceSnapshot::try_new(ContextNativeResourceCounts {
                pending_captures: u8::try_from(MAX_PENDING_SEMANTIC_SCREENSHOTS + 1)
                    .expect("small ceiling"),
                ..valid
            }),
            Err(ContextPortContractError::ResourceLimit)
        );
    }

    #[test]
    fn debug_output_redacts_identity_profile_lease_and_navigation() {
        let (_, operation) = constructing(ContextKind::BorrowedTab);
        let request = ContextConstructionRequest::try_new(
            operation,
            capabilities(ContextKind::BorrowedTab),
            profile_lease(operation.context().identity(), 1),
            ContextConstructionSource::BorrowedTab(BorrowedTabLeaseId::new(9001).expect("lease")),
        )
        .expect("request");
        let output = format!("{request:?}");
        assert!(output.contains("[redacted]"));
        assert!(!output.contains("9001"));
        assert!(!output.contains("ProfileId"));
    }

    #[test]
    fn fake_port_observes_only_the_closed_request_vocabulary() {
        struct FakePort;

        impl AgentBrowserPort for FakePort {
            fn dispatch(&self, request: ContextNativeRequest) -> ContextDispatch {
                match request {
                    ContextNativeRequest::Construct(_) => ContextDispatch::Scheduled,
                    ContextNativeRequest::Navigate(_)
                    | ContextNativeRequest::Transition(_)
                    | ContextNativeRequest::Cancel(_) => {
                        ContextDispatch::Rejected(ContextPortFailure::Stale)
                    }
                }
            }

            fn transfer_cookies(&self, _request: ContextCookieTransferRequest) -> ContextDispatch {
                ContextDispatch::Unsupported
            }

            fn audit_resources(&self, _audit: ContextResourceAuditId) -> ContextDispatch {
                ContextDispatch::Scheduled
            }

            fn seal_for_shutdown(&self, _audit: ContextResourceAuditId) -> ContextShutdownDispatch {
                ContextShutdownDispatch::AuditScheduled
            }

            fn invoke_semantic(
                &self,
                _invocation: crate::SemanticRuntimeInvocation,
            ) -> ContextDispatch {
                ContextDispatch::Unsupported
            }

            fn execute_semantic_action(
                &self,
                _request: crate::SemanticActionNativeRequest,
                _completion: SemanticActionNativeCompletion,
            ) -> ContextDispatch {
                ContextDispatch::Unsupported
            }

            fn capture_semantic_screenshot(
                &self,
                _request: crate::SemanticScreenshotNativeRequest,
                _completion: SemanticScreenshotNativeCompletion,
            ) -> ContextDispatch {
                ContextDispatch::Unsupported
            }
        }

        let (_, operation) = constructing(ContextKind::Owned);
        let request = ContextConstructionRequest::try_new(
            operation,
            capabilities(ContextKind::Owned),
            profile_lease(operation.context().identity(), 1),
            ContextConstructionSource::Owned,
        )
        .expect("request");
        let port = FakePort;
        assert_eq!(
            port.dispatch(ContextNativeRequest::Construct(request)),
            ContextDispatch::Scheduled
        );
        assert_eq!(
            port.audit_resources(ContextResourceAuditId::new(1).expect("audit")),
            ContextDispatch::Scheduled
        );
        assert_eq!(
            port.seal_for_shutdown(ContextResourceAuditId::new(2).expect("shutdown audit")),
            ContextShutdownDispatch::AuditScheduled
        );
        let mut resources =
            crate::WorkBrowserResources::new(crate::WorkId::from(99), ProfileId::from(9));
        let resource_request = resources
            .construct(
                crate::WorkBrowserResourceId::from_raw(100),
                ContextId::from_raw(101),
                ContextProfileStorageClass::Ephemeral,
                crate::AgentPolicyInstant::from_millis(0),
            )
            .unwrap();
        let expected = resource_request.resource().clone();
        let result = port.work_resource_lifecycle(
            resource_request,
            Box::new(|_| panic!("unsupported adapter cannot emit a native callback")),
        );
        let crate::WorkBrowserResourceDispatch::Rejected { request, failure } = result else {
            panic!("legacy adapter must remain unsupported");
        };
        assert_eq!(failure, ContextPortFailure::Unsupported);
        assert_eq!(request.resource(), &expected);
        let _ = resources.dispatch_refused(*request, failure).unwrap();
        resources.seal();
        assert!(resources.is_quiescent());
    }
}
