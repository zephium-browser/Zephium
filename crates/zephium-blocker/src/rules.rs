use std::fmt;
use std::sync::Arc;

use adblock::request::Request;
#[cfg(feature = "runtime-exact")]
use adblock::url_parser;
use adblock::Engine;
use thiserror::Error;

use crate::{CompilationReport, CompileLimits, CompileTarget};

/// A page no official list names, standing in for any site.
#[cfg(feature = "runtime-exact")]
const NEUTRAL_PAGE: &str = "https://example.com/";
/// Requests and the official list whose rule blocks each; the last must stay
/// allowed.
const OFFICIAL_CANARY: [(&str, Option<&str>); 3] = [
    (
        "https://ad.doubleclick.com/activity;src=1",
        Some("easylist"),
    ),
    (
        "https://www.googletagmanager.com/zephium-blocker-smoke.js",
        Some("easyprivacy"),
    ),
    ("https://example.com/app.js", None),
];

/// Stable SHA-256 identity of a canonical effective policy.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PolicyDigest(pub(crate) [u8; 32]);

impl PolicyDigest {
    /// Returns the raw SHA-256 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// SHA-256 identity of the exact canonical native declarative artifact.
///
/// Unlike [`PolicyDigest`], this changes whenever any encoded JSON byte
/// changes and is therefore safe as a persistent native compiler cache key.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactDigest(pub(crate) [u8; 32]);

impl ArtifactDigest {
    /// Returns the raw SHA-256 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for PolicyDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for PolicyDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Canonical WebKit content-blocking JSON and its rule count.
#[derive(Clone, Debug)]
pub struct WebKitRules {
    pub(crate) json: Arc<str>,
    pub(crate) rule_count: usize,
    pub(crate) digest: ArtifactDigest,
}

impl WebKitRules {
    /// Returns canonical compact JSON suitable for native WebKit compilation.
    pub fn json(&self) -> &str {
        &self.json
    }

    /// Returns the number of emitted content-blocking rules.
    pub const fn rule_count(&self) -> usize {
        self.rule_count
    }

    /// Returns the identity of the exact encoded JSON artifact.
    pub const fn digest(&self) -> ArtifactDigest {
        self.digest
    }

    /// Clones the immutable encoded artifact without copying its bytes.
    pub fn encoded(&self) -> Arc<str> {
        self.json.clone()
    }
}

struct CompiledRulesInner {
    target: CompileTarget,
    engine: Option<Engine>,
    digest: PolicyDigest,
    webkit: Option<WebKitRules>,
    report: CompilationReport,
    limits: CompileLimits,
    cosmetics: Option<crate::cosmetics::PreparedCosmetics>,
}

/// Immutable, cheaply cloneable artifacts compiled from one effective policy.
#[derive(Clone)]
pub struct CompiledRules(Arc<CompiledRulesInner>);

impl CompiledRules {
    pub(crate) fn new(
        target: CompileTarget,
        engine: Option<Engine>,
        digest: PolicyDigest,
        webkit: Option<WebKitRules>,
        report: CompilationReport,
        limits: CompileLimits,
        cosmetics: Option<crate::cosmetics::PreparedCosmetics>,
    ) -> Self {
        Self(Arc::new(CompiledRulesInner {
            target,
            engine,
            digest,
            webkit,
            report,
            limits,
            cosmetics,
        }))
    }

    /// Returns the sole native artifact target retained by this value.
    pub fn target(&self) -> CompileTarget {
        self.0.target
    }

    /// Returns the canonical effective-policy identity.
    pub fn digest(&self) -> PolicyDigest {
        self.0.digest
    }

    /// Returns the WebKit artifact.
    pub fn webkit(&self) -> Option<&WebKitRules> {
        self.0.webkit.as_ref()
    }

    /// Returns the immutable compilation report.
    pub fn report(&self) -> &CompilationReport {
        &self.0.report
    }

    pub(crate) fn cosmetics(&self) -> Option<&crate::cosmetics::PreparedCosmetics> {
        self.0.cosmetics.as_ref()
    }

    /// Returns the independently validated static selector policy.
    pub fn cosmetic_policy(&self) -> Option<&crate::CosmeticPolicy> {
        self.0
            .cosmetics
            .as_ref()
            .map(|prepared| prepared.policy.as_ref())
    }

    #[cfg(feature = "runtime")]
    pub(crate) fn serialize_runtime_engine(&self) -> Option<Vec<u8>> {
        self.0.engine.as_ref().map(Engine::serialize)
    }

    /// Evaluates a bounded network request against the runtime matcher.
    ///
    /// This call performs no I/O and does not wait on another actor. Request
    /// parsing and matching are entirely in memory.
    pub fn evaluate(&self, request: NetworkRequest<'_>) -> Result<NetworkDecision, MatchError> {
        self.evaluate_inner(request, false)
    }

    /// Evaluates only rules which are independent of the initiating document.
    ///
    /// Native backends must use this mode when they can identify only the
    /// top-level page rather than the exact initiating frame. Domain and
    /// first/third-party constrained rules are intentionally skipped.
    pub fn evaluate_source_independent(
        &self,
        request: NetworkRequest<'_>,
    ) -> Result<NetworkDecision, MatchError> {
        self.evaluate_inner(request, true)
    }

    /// Whether a runtime policy still decides the known requests as its
    /// official lists must, so a list update that silently stops blocking is
    /// refused instead of used. Each blocked request is checked only when the
    /// list that blocks it is part of the policy.
    ///
    /// Every request is judged with no page known, as a native callback without
    /// the initiating document judges it, and where the matcher knows the page,
    /// on a neutral page too. Site-scoped exceptions such as uBlock Origin's
    /// `@@*$script,1p,domain=…` must lift neither.
    pub fn verify_official_canary(&self) -> Result<(), String> {
        if self.target() != CompileTarget::Runtime {
            return Ok(());
        }
        let present = |list: &str| {
            self.report()
                .sources()
                .iter()
                .any(|source| source.id().as_str() == list)
        };
        for (url, list) in OFFICIAL_CANARY {
            if list.is_some_and(|list| !present(list)) {
                continue;
            }
            let expected = if list.is_some() {
                NetworkAction::Block
            } else {
                NetworkAction::Allow
            };
            let decisions = [
                // Exact attribution exists only where the matcher has it.
                #[cfg(feature = "runtime-exact")]
                (
                    "on a neutral page",
                    self.evaluate(NetworkRequest::new(
                        url,
                        NEUTRAL_PAGE,
                        ResourceType::Script,
                        RequestMethod::Get,
                    )),
                ),
                (
                    "with no page known",
                    self.evaluate_source_independent(NetworkRequest::source_independent(
                        url,
                        ResourceType::Script,
                        RequestMethod::Get,
                    )),
                ),
            ];
            for (mode, decision) in decisions {
                let action = decision
                    .map_err(|error| format!("official canary {url} {mode} failed: {error:?}"))?
                    .action();
                if action != expected {
                    return Err(format!(
                        "official canary {url} {mode} returned {action:?}, expected {expected:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn evaluate_inner(
        &self,
        request: NetworkRequest<'_>,
        source_independent_only: bool,
    ) -> Result<NetworkDecision, MatchError> {
        let engine = self
            .0
            .engine
            .as_ref()
            .ok_or(MatchError::WrongArtifactTarget)?;
        evaluate_runtime_engine(engine, self.0.limits, request, source_independent_only)
    }

    #[cfg(feature = "runtime-exact")]
    pub(crate) fn source_document(&self, url: &str) -> Result<SourceDocument, MatchError> {
        SourceDocument::parse(url, self.0.limits)
    }

    pub(crate) fn runtime_engine(&self) -> Result<RuntimeEngine<'_>, MatchError> {
        let engine = self
            .0
            .engine
            .as_ref()
            .ok_or(MatchError::WrongArtifactTarget)?;
        Ok(RuntimeEngine {
            engine,
            limits: self.0.limits,
        })
    }

    /// Evaluates a request from a document parsed by [`Self::source_document`],
    /// with the same result as [`Self::evaluate`] given that document's URL.
    #[cfg(feature = "runtime-exact")]
    pub(crate) fn evaluate_from(
        &self,
        url: &str,
        source: &SourceDocument,
        resource_type: ResourceType,
        method: RequestMethod,
    ) -> Result<NetworkDecision, MatchError> {
        let engine = self
            .0
            .engine
            .as_ref()
            .ok_or(MatchError::WrongArtifactTarget)?;
        evaluate_runtime_engine_from(engine, self.0.limits, url, source, resource_type, method)
    }
}

/// What matching reads from an initiating document: its hostname and
/// registrable domain. Parsed once per document rather than once per request,
/// which keeps a URL parse and a public-suffix lookup off every callback.
#[cfg(feature = "runtime-exact")]
pub(crate) struct SourceDocument {
    hostname: Box<str>,
    domain: Box<str>,
}

#[cfg(feature = "runtime-exact")]
impl SourceDocument {
    fn parse(url: &str, limits: CompileLimits) -> Result<Self, MatchError> {
        if url.len() > limits.max_source_url_bytes() {
            return Err(MatchError::SourceUrlTooLong {
                actual: url.len(),
                limit: limits.max_source_url_bytes(),
            });
        }
        let parsed = url_parser::parse_url(url).ok_or(MatchError::InvalidRequest)?;
        Ok(Self {
            hostname: parsed.hostname().into(),
            domain: parsed.domain().into(),
        })
    }
}

#[cfg(feature = "runtime")]
pub(crate) struct CachedRuntimeRules {
    engine: Engine,
    limits: CompileLimits,
}

#[cfg(feature = "runtime")]
impl CachedRuntimeRules {
    pub(crate) const fn new(engine: Engine, limits: CompileLimits) -> Self {
        Self { engine, limits }
    }

    pub(crate) const fn runtime_engine(&self) -> RuntimeEngine<'_> {
        RuntimeEngine {
            engine: &self.engine,
            limits: self.limits,
        }
    }

    #[cfg(feature = "runtime-exact")]
    pub(crate) fn source_document(&self, url: &str) -> Result<SourceDocument, MatchError> {
        SourceDocument::parse(url, self.limits)
    }

    #[cfg(feature = "runtime-exact")]
    pub(crate) fn evaluate_from(
        &self,
        url: &str,
        source: &SourceDocument,
        resource_type: ResourceType,
        method: RequestMethod,
    ) -> Result<NetworkDecision, MatchError> {
        evaluate_runtime_engine_from(
            &self.engine,
            self.limits,
            url,
            source,
            resource_type,
            method,
        )
    }

    pub(crate) fn evaluate_source_independent(
        &self,
        request: NetworkRequest<'_>,
    ) -> Result<NetworkDecision, MatchError> {
        evaluate_runtime_engine(&self.engine, self.limits, request, true)
    }
}

fn evaluate_runtime_engine(
    engine: &Engine,
    limits: CompileLimits,
    request: NetworkRequest<'_>,
    source_independent_only: bool,
) -> Result<NetworkDecision, MatchError> {
    let native = native_request(limits, request, source_independent_only)?;
    match_native(engine, &native, source_independent_only)
}

/// A compiled or cached runtime matcher, as a policy split across two reads it.
#[derive(Clone, Copy)]
pub(crate) struct RuntimeEngine<'a> {
    engine: &'a Engine,
    limits: CompileLimits,
}

/// A policy split across two matchers, the request parsed once under the
/// first one's limits: the first decides unless it lets the request through,
/// or cannot judge it, and the second blocks it. Otherwise the first one's
/// result stands, an error included.
pub(crate) fn evaluate_split(
    first: RuntimeEngine<'_>,
    second: RuntimeEngine<'_>,
    request: NetworkRequest<'_>,
    source_independent_only: bool,
) -> Result<NetworkDecision, MatchError> {
    let native = native_request(first.limits, request, source_independent_only)?;
    split_decision(first, second, &native, source_independent_only)
}

/// As [`evaluate_split`], for a request from a parsed document.
#[cfg(feature = "runtime-exact")]
pub(crate) fn evaluate_split_from(
    first: RuntimeEngine<'_>,
    second: RuntimeEngine<'_>,
    url: &str,
    source: &SourceDocument,
    resource_type: ResourceType,
    method: RequestMethod,
) -> Result<NetworkDecision, MatchError> {
    with_native_request_from(first.limits, url, source, resource_type, method, |native| {
        split_decision(first, second, native, false)
    })
}

fn split_decision(
    first: RuntimeEngine<'_>,
    second: RuntimeEngine<'_>,
    native: &Request,
    source_independent_only: bool,
) -> Result<NetworkDecision, MatchError> {
    let decided = match_native(first.engine, native, source_independent_only);
    if matches!(decided, Ok(decision) if decision.action == NetworkAction::Block) {
        return decided;
    }
    match match_native(second.engine, native, source_independent_only) {
        Ok(decision) if decision.action == NetworkAction::Block => Ok(decision),
        _ => decided,
    }
}

fn native_request(
    limits: CompileLimits,
    request: NetworkRequest<'_>,
    source_independent_only: bool,
) -> Result<Request, MatchError> {
    if request.url.len() > limits.max_request_url_bytes() {
        return Err(MatchError::RequestUrlTooLong {
            actual: request.url.len(),
            limit: limits.max_request_url_bytes(),
        });
    }
    if source_independent_only {
        if request.source_url.is_some() {
            return Err(MatchError::InvalidRequest);
        }
        Request::new_source_independent(
            request.url,
            request.resource_type.as_adblock_str(),
            request.method.as_adblock_str(),
        )
    } else {
        #[cfg(not(feature = "runtime-exact"))]
        {
            return Err(MatchError::ExactAttributionUnsupported);
        }
        #[cfg(feature = "runtime-exact")]
        {
            let source_url = request.source_url.ok_or(MatchError::InvalidRequest)?;
            if source_url.len() > limits.max_source_url_bytes() {
                return Err(MatchError::SourceUrlTooLong {
                    actual: source_url.len(),
                    limit: limits.max_source_url_bytes(),
                });
            }
            Request::new(
                request.url,
                source_url,
                request.resource_type.as_adblock_str(),
                request.method.as_adblock_str(),
            )
        }
    }
    .map_err(|_| MatchError::InvalidRequest)
}

#[cfg(feature = "runtime-exact")]
fn evaluate_runtime_engine_from(
    engine: &Engine,
    limits: CompileLimits,
    url: &str,
    source: &SourceDocument,
    resource_type: ResourceType,
    method: RequestMethod,
) -> Result<NetworkDecision, MatchError> {
    with_native_request_from(limits, url, source, resource_type, method, |native| {
        match_native(engine, native, false)
    })
}

#[cfg(feature = "runtime-exact")]
fn with_native_request_from<T>(
    limits: CompileLimits,
    url: &str,
    source: &SourceDocument,
    resource_type: ResourceType,
    method: RequestMethod,
    decide: impl FnOnce(&Request) -> Result<T, MatchError>,
) -> Result<T, MatchError> {
    if url.len() > limits.max_request_url_bytes() {
        return Err(MatchError::RequestUrlTooLong {
            actual: url.len(),
            limit: limits.max_request_url_bytes(),
        });
    }
    let parsed = url_parser::parse_url(url).ok_or(MatchError::InvalidRequest)?;
    // Request::new with the source URL would derive the same hostname and
    // party from a fresh parse of that URL.
    let native = Request::preparsed(
        &parsed.url,
        parsed.hostname(),
        &source.hostname,
        resource_type.as_adblock_str(),
        parsed.domain() != &*source.domain,
        method.as_adblock_str(),
    );
    decide(&native)
}

fn match_native(
    engine: &Engine,
    native: &Request,
    source_independent_only: bool,
) -> Result<NetworkDecision, MatchError> {
    let result = if source_independent_only {
        engine.try_check_prepared_source_independent_network_request(native)
    } else {
        engine.try_check_prepared_network_request(native)
    }
    .map_err(map_prepared_match_error)?;

    // Compiler admission rejects all mutation rules. Treat any future
    // upstream behavior change as an explicit invariant failure rather
    // than silently applying a capability we never audited.
    if result.redirect.is_some() || result.rewritten_url.is_some() {
        return Err(MatchError::UnexpectedMutation);
    }

    Ok(NetworkDecision {
        action: if result.should_block() {
            NetworkAction::Block
        } else {
            NetworkAction::Allow
        },
        important: result.important,
        matched_rule: result.filter.is_some(),
        matched_exception: result.exception.is_some(),
    })
}

pub(crate) const fn map_prepared_match_error(
    error: adblock::blocker::PreparedNetworkMatcherError,
) -> MatchError {
    match error {
        adblock::blocker::PreparedNetworkMatcherError::Unprepared => MatchError::MatcherUnprepared,
        adblock::blocker::PreparedNetworkMatcherError::Unavailable => {
            MatchError::MatcherUnavailable
        }
        adblock::blocker::PreparedNetworkMatcherError::CandidateBudgetExhausted => {
            MatchError::CandidateBudgetExhausted
        }
    }
}

/// Browser resource category used for network matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceType {
    /// A beacon or ping.
    Beacon,
    /// A CSP report.
    Csp,
    /// A top-level document.
    Document,
    /// A document type definition.
    Dtd,
    /// A Fetch API request.
    Fetch,
    /// A font.
    Font,
    /// An image.
    Image,
    /// Audio or video media.
    Media,
    /// An object/embed resource.
    Object,
    /// A script.
    Script,
    /// A stylesheet.
    Stylesheet,
    /// A child frame document.
    Subdocument,
    /// A WebSocket.
    WebSocket,
    /// An XSLT resource.
    Xslt,
    /// An XMLHttpRequest.
    XmlHttpRequest,
    /// Any resource without a more precise native classification.
    Other,
}

impl ResourceType {
    const fn as_adblock_str(self) -> &'static str {
        match self {
            Self::Beacon => "beacon",
            Self::Csp => "csp_report",
            Self::Document => "document",
            Self::Dtd => "xml_dtd",
            // adblock-rust 0.13 models Fetch and XHR under the ABP
            // `xmlhttprequest` category. Passing "fetch" falls through to
            // `Other` and silently defeats `$xmlhttprequest` rules.
            Self::Fetch => "xmlhttprequest",
            Self::Font => "font",
            Self::Image => "image",
            Self::Media => "media",
            Self::Object => "object",
            Self::Script => "script",
            Self::Stylesheet => "stylesheet",
            Self::Subdocument => "subdocument",
            Self::WebSocket => "websocket",
            Self::Xslt => "xslt",
            Self::XmlHttpRequest => "xmlhttprequest",
            Self::Other => "other",
        }
    }
}

/// HTTP method category used for network matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestMethod {
    /// CONNECT.
    Connect,
    /// DELETE.
    Delete,
    /// GET.
    Get,
    /// HEAD.
    Head,
    /// OPTIONS.
    Options,
    /// PATCH.
    Patch,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// A method not represented by the audited matcher API.
    Other,
}

impl RequestMethod {
    const fn as_adblock_str(self) -> &'static str {
        match self {
            Self::Connect => "CONNECT",
            Self::Delete => "DELETE",
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Patch => "PATCH",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Other => "OTHER",
        }
    }
}

/// Borrowed, allocation-free request metadata supplied by a native backend.
#[derive(Clone, Copy, Debug)]
pub struct NetworkRequest<'a> {
    url: &'a str,
    source_url: Option<&'a str>,
    resource_type: ResourceType,
    method: RequestMethod,
}

impl<'a> NetworkRequest<'a> {
    /// Creates request metadata.
    ///
    /// `source_url` must identify the initiating document. For a top-level
    /// document load, callers should pass the document URL itself.
    pub const fn new(
        url: &'a str,
        source_url: &'a str,
        resource_type: ResourceType,
        method: RequestMethod,
    ) -> Self {
        Self {
            url,
            source_url: Some(source_url),
            resource_type,
            method,
        }
    }

    /// Creates request metadata without initiating-document attribution.
    ///
    /// Values built through this constructor may be passed only to
    /// [`CompiledRules::evaluate_source_independent`].
    pub const fn source_independent(
        url: &'a str,
        resource_type: ResourceType,
        method: RequestMethod,
    ) -> Self {
        Self {
            url,
            source_url: None,
            resource_type,
            method,
        }
    }
}

/// Final action for a network request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkAction {
    /// Continue the native request unchanged.
    Allow,
    /// Cancel the native request.
    Block,
}

/// Auditable result of one runtime match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkDecision {
    action: NetworkAction,
    important: bool,
    matched_rule: bool,
    matched_exception: bool,
}

impl NetworkDecision {
    /// Returns the native action.
    pub const fn action(self) -> NetworkAction {
        self.action
    }

    /// Reports whether an `$important` rule matched.
    pub const fn important(self) -> bool {
        self.important
    }

    /// Reports whether a blocking rule matched.
    pub const fn matched_rule(self) -> bool {
        self.matched_rule
    }

    /// Reports whether an exception rule matched.
    pub const fn matched_exception(self) -> bool {
        self.matched_exception
    }
}

/// A bounded request could not be evaluated.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MatchError {
    /// A runtime match was requested from a WebKit-only artifact.
    #[error("compiled policy does not contain a runtime matcher")]
    WrongArtifactTarget,
    /// The immutable matcher was not transactionally prepared.
    ///
    /// Native adapters must fail open rather than block their synchronous
    /// request callback.
    #[error("prepared network matcher is not initialized")]
    MatcherUnprepared,
    /// The immutable matcher could not be borrowed without waiting.
    ///
    /// Native adapters must fail open rather than block their synchronous
    /// request callback.
    #[error("prepared network matcher is temporarily unavailable")]
    MatcherUnavailable,
    /// The matcher reached its shared candidate-evaluation ceiling before
    /// every decision-relevant list was scanned.
    ///
    /// Native adapters must fail open. This distinct class supports aggregate,
    /// URL-free capacity diagnostics.
    #[error("prepared network matcher candidate budget was exhausted")]
    CandidateBudgetExhausted,
    /// The requested URL exceeded its configured byte budget.
    #[error("request URL is {actual} bytes; limit is {limit}")]
    RequestUrlTooLong {
        /// Observed byte length.
        actual: usize,
        /// Configured byte limit.
        limit: usize,
    },
    /// The source-document URL exceeded its configured byte budget.
    #[error("source URL is {actual} bytes; limit is {limit}")]
    SourceUrlTooLong {
        /// Observed byte length.
        actual: usize,
        /// Configured byte limit.
        limit: usize,
    },
    /// The matcher rejected malformed URL metadata.
    #[error("invalid network request metadata")]
    InvalidRequest,
    /// V1 native runtime policies intentionally expose only the bounded
    /// source-independent matcher.
    #[error("exact-attribution runtime matching is not enabled")]
    ExactAttributionUnsupported,
    /// A forbidden mutation unexpectedly escaped compiler admission.
    #[error("compiled policy unexpectedly produced a request mutation")]
    UnexpectedMutation,
}

impl fmt::Debug for CompiledRules {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CompiledRules")
            .field("target", &self.target())
            .field("digest", &self.digest())
            .field("webkit_rules", &self.webkit().map(WebKitRules::rule_count))
            .field("accepted_rules", &self.report().accepted_rules())
            .finish()
    }
}
