use std::collections::BTreeMap;
#[cfg(feature = "webkit")]
use std::collections::HashSet;
use std::fmt;
#[cfg(feature = "webkit")]
use std::sync::Arc;

use crate::Sha256;
#[cfg(feature = "webkit")]
use adblock::content_blocking::{
    CbLoadType, CbRule, CbRuleCreationFailure, CbRuleEquivalent, CbType,
};
use adblock::filters::network::NetworkFilterMask;
use adblock::lists::{
    parse_filter, FilterFormat, FilterParseError, ParseOptions, ParsedLine, RuleTypes,
};
#[cfg(feature = "runtime")]
use adblock::{
    blocker::{NetworkMatcherPreparationError, NetworkMatcherPreparationLimits},
    Engine, FilterSet,
};
#[cfg(feature = "webkit")]
use serde::Serialize;
use thiserror::Error;

use crate::limits::CompileLimits;
#[cfg(feature = "runtime")]
use crate::report::RuntimeCoverage;
use crate::report::{
    input_counts, CompilationReport, InputDropReason, SourceReport, WebKitDropReason,
};
#[cfg(feature = "webkit")]
use crate::report::{webkit_counts, WebKitCoverage};
#[cfg(feature = "webkit")]
use crate::rules::{ArtifactDigest, WebKitRules};
use crate::rules::{CompiledRules, PolicyDigest};
#[cfg(feature = "webkit")]
use crate::webkit::ResourceType as WebKitResourceType;
#[cfg(feature = "webkit")]
use crate::WEBKIT_ARTIFACT_FORMAT_VERSION;
use crate::{ADBLOCK_ENGINE_VERSION, POLICY_FORMAT_VERSION};

const MAX_SOURCE_ID_BYTES: usize = 128;
const DIGEST_DOMAIN: &[u8] = b"zephium-blocker-policy";
#[cfg(feature = "webkit")]
const WEBKIT_ARTIFACT_DIGEST_DOMAIN: &[u8] = b"zephium-webkit-content-rules";
#[cfg(feature = "webkit")]
const MAX_WEBKIT_URL_FILTER_BYTES: usize = 8 * 1024;
/// Its wildcard rules never reach WebKit or its pruning.
#[cfg(feature = "webkit")]
const NAVIGATION_CHECKED_SOURCE: &str = "ublock-badware";
#[cfg(feature = "runtime")]
const MAX_RUNTIME_REGEXES: usize = 4_096;
#[cfg(feature = "runtime")]
const MAX_RUNTIME_PATTERN_BYTES: usize = 2 * 1024 * 1024;
#[cfg(feature = "runtime")]
const MAX_RUNTIME_PATTERNS_PER_REGEX: usize = 256;
#[cfg(feature = "runtime")]
const MAX_RUNTIME_PATTERN_BYTES_PER_REGEX: usize = 128 * 1024;
#[cfg(feature = "runtime")]
const MAX_RUNTIME_REGEX_SIZE_BYTES: usize = 96 * 1024;
#[cfg(feature = "runtime")]
const MAX_RUNTIME_REGEX_DFA_SIZE_BYTES: usize = 16 * 1024;
#[cfg(feature = "runtime")]
const MAX_RUNTIME_FILTER_CHECKS_PER_REQUEST: usize = 256;
#[cfg(feature = "runtime")]
const WEBVIEW2_AUDITED_REQUEST_TYPES: NetworkFilterMask = NetworkFilterMask::FROM_STYLESHEET
    .union(NetworkFilterMask::FROM_IMAGE)
    .union(NetworkFilterMask::FROM_MEDIA)
    .union(NetworkFilterMask::FROM_FONT)
    .union(NetworkFilterMask::FROM_SCRIPT)
    .union(NetworkFilterMask::FROM_XMLHTTPREQUEST)
    .union(NetworkFilterMask::FROM_PING);

/// Stable, filesystem-independent identifier for one filter source.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SourceId(Box<str>);

impl SourceId {
    /// Validates and owns a source identifier.
    ///
    /// Identifiers are deliberately limited to lowercase ASCII letters,
    /// digits, `.`, `_`, and `-` so sorting and digesting are unambiguous.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, SourceIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(SourceIdError::Empty);
        }
        if value.len() > MAX_SOURCE_ID_BYTES {
            return Err(SourceIdError::TooLong {
                actual: value.len(),
                limit: MAX_SOURCE_ID_BYTES,
            });
        }
        if !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        }) {
            return Err(SourceIdError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    /// Returns the canonical identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An invalid filter-source identifier.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum SourceIdError {
    /// The identifier is empty.
    #[error("source identifier must not be empty")]
    Empty,
    /// The identifier exceeds the fixed byte limit.
    #[error("source identifier is {actual} bytes; limit is {limit}")]
    TooLong {
        /// Observed byte length.
        actual: usize,
        /// Fixed byte limit.
        limit: usize,
    },
    /// The identifier contains a non-canonical character.
    #[error("source identifier contains a non-canonical character")]
    InvalidCharacter,
}

/// Syntax used by one filter source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFormat {
    /// Adblock Plus/uBlock-style rules.
    Standard,
    /// Hosts-file entries.
    Hosts,
}

/// Native artifact to produce for the current operating-system backend.
///
/// A compile publishes exactly one target so Windows does not retain a large
/// unused WebKit JSON string and WebKit platforms do not retain an unused
/// runtime matcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileTarget {
    /// Immutable synchronous matcher for WebView2 request callbacks.
    Runtime,
    /// Canonical declarative rules for WKWebView or WebKitGTK.
    WebKit,
}

impl SourceFormat {
    const fn adblock(self) -> FilterFormat {
        match self {
            Self::Standard => FilterFormat::Standard,
            Self::Hosts => FilterFormat::Hosts,
        }
    }

    const fn digest_tag(self) -> u8 {
        match self {
            Self::Standard => 0,
            Self::Hosts => 1,
        }
    }
}

/// Whether an official list may run `trusted-*` scriptlets: only uBlock
/// Origin's own lists, which are written for them. User filters and every
/// other list never may.
pub fn official_source_trusts_scriptlets(id: &SourceId) -> bool {
    matches!(
        id.as_str(),
        "ublock-filters"
            | "ublock-quick-fixes"
            | "ublock-link-shorteners"
            | "ublock-privacy"
            | "ublock-resource-abuse"
            | "ublock-unbreak"
            | "ublock-badware"
    )
}

/// Owned input list consumed by [`Compiler::compile`].
#[derive(Debug)]
pub struct FilterSource {
    id: SourceId,
    format: SourceFormat,
    contents: String,
    trusted: bool,
}

impl FilterSource {
    /// Creates an owned source. Resource budgets are applied during compilation.
    pub fn new(id: SourceId, format: SourceFormat, contents: String) -> Self {
        Self {
            id,
            format,
            contents,
            trusted: false,
        }
    }

    /// Lets this source's `trusted-*` scriptlet rules run.
    #[must_use]
    pub const fn trusting_scriptlets(mut self) -> Self {
        self.trusted = true;
        self
    }

    /// Whether this source's `trusted-*` scriptlet rules may run.
    pub const fn trusts_scriptlets(&self) -> bool {
        self.trusted
    }

    /// Returns the source identifier.
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    /// Returns the declared syntax.
    pub const fn format(&self) -> SourceFormat {
        self.format
    }

    /// Returns the source byte length without exposing or copying its contents.
    pub const fn byte_len(&self) -> usize {
        self.contents.len()
    }
}

/// Stateless, reusable policy compiler with validated resource budgets.
#[derive(Clone, Copy, Debug)]
pub struct Compiler {
    limits: CompileLimits,
    #[cfg(feature = "webkit")]
    webkit_pruned: Option<&'static crate::webkit_prune::PrunedRules>,
    #[cfg(feature = "webkit")]
    webkit_pruning: bool,
}

impl Compiler {
    /// Creates a compiler using validated limits.
    pub const fn new(limits: CompileLimits) -> Self {
        Self {
            limits,
            #[cfg(feature = "webkit")]
            webkit_pruned: None,
            #[cfg(feature = "webkit")]
            webkit_pruning: true,
        }
    }

    /// Keeps every WebKit rule, for release tooling that measures which
    /// rules the page corpus matches.
    #[cfg(feature = "webkit")]
    pub const fn without_webkit_pruning(mut self) -> Self {
        self.webkit_pruning = false;
        self
    }

    #[cfg(all(test, feature = "webkit"))]
    pub(crate) fn with_webkit_pruned(
        mut self,
        pruned: &'static crate::webkit_prune::PrunedRules,
    ) -> Self {
        self.webkit_pruned = Some(pruned);
        self
    }

    /// Returns the active resource limits.
    pub const fn limits(self) -> CompileLimits {
        self.limits
    }

    /// Consumes, validates, canonicalizes, and compiles filter sources.
    pub fn compile(
        &self,
        target: CompileTarget,
        mut sources: Vec<FilterSource>,
    ) -> Result<CompiledRules, CompileError> {
        if (matches!(target, CompileTarget::Runtime) && !cfg!(feature = "runtime"))
            || (matches!(target, CompileTarget::WebKit) && !cfg!(feature = "webkit"))
        {
            return Err(CompileError::TargetUnavailable);
        }
        if sources.is_empty() {
            return Err(CompileError::NoSources);
        }
        if sources.len() > self.limits.max_sources() {
            return Err(CompileError::TooManySources {
                actual: sources.len(),
                limit: self.limits.max_sources(),
            });
        }

        sources.sort_unstable_by(|left, right| left.id.cmp(&right.id));
        for adjacent in sources.windows(2) {
            if adjacent[0].id == adjacent[1].id {
                return Err(CompileError::DuplicateSource {
                    id: adjacent[0].id.clone(),
                });
            }
        }

        let total_source_bytes = sources.iter().try_fold(0usize, |total, source| {
            if source.contents.len() > self.limits.max_source_bytes() {
                return Err(CompileError::SourceTooLarge {
                    id: source.id.clone(),
                    actual: source.contents.len(),
                    limit: self.limits.max_source_bytes(),
                });
            }
            total
                .checked_add(source.contents.len())
                .ok_or(CompileError::TotalSourceBytesOverflow)
        })?;
        if total_source_bytes > self.limits.max_total_source_bytes() {
            return Err(CompileError::TotalSourceBytesExceeded {
                actual: total_source_bytes,
                limit: self.limits.max_total_source_bytes(),
            });
        }

        let mut accepted_rules = 0usize;
        let mut candidate_rules = 0usize;
        let mut physical_lines = 0usize;
        let mut prepared = Vec::with_capacity(sources.len());
        for source in sources {
            prepared.push(prepare_source(
                target,
                source,
                self.limits,
                &mut accepted_rules,
                &mut candidate_rules,
                &mut physical_lines,
            )?);
        }
        if accepted_rules == 0 {
            return Err(CompileError::NoUsableRules);
        }

        let cosmetic_policy = crate::CosmeticPolicy::compile_with_trust(
            prepared
                .iter()
                .map(|source| (source.cosmetics.as_str(), source.trusted)),
        )
        .map_err(CompileError::Cosmetics)?;
        // A policy with only scriptlets still has a program to run.
        let cosmetics = if cosmetic_policy.report().accepted == 0
            && cosmetic_policy.scriptlet_program().is_none()
        {
            None
        } else {
            Some(
                crate::cosmetics::PreparedCosmetics::new(cosmetic_policy, target)
                    .map_err(CompileError::Cosmetics)?,
            )
        };
        let mut digest = digest_policy(&prepared);
        if let Some(cosmetics) = &cosmetics {
            use zephium_core::blocker::DocumentStyleProvider;
            let mut combined = Sha256::new();
            combined.update(b"zephium-network-and-cosmetic-policy-v1");
            combined.update(digest.as_bytes());
            combined.update(cosmetics.policy.fingerprint().as_bytes());
            digest = PolicyDigest(combined.finalize());
        }
        let (runtime, runtime_coverage, webkit, webkit_coverage, native_blocking_rule_entries) =
            match target {
                CompileTarget::Runtime => {
                    #[cfg(feature = "runtime")]
                    {
                        let (runtime, blocking_entries, coverage) =
                            compile_runtime(&mut prepared, self.limits)?;
                        (Some(runtime), Some(coverage), None, None, blocking_entries)
                    }
                    #[cfg(not(feature = "runtime"))]
                    {
                        return Err(CompileError::TargetUnavailable);
                    }
                }
                CompileTarget::WebKit => {
                    #[cfg(feature = "webkit")]
                    {
                        let pruned = if self.webkit_pruning {
                            self.webkit_pruned
                                .unwrap_or_else(crate::webkit_prune::PrunedRules::shipped)
                        } else {
                            crate::webkit_prune::PrunedRules::none()
                        };
                        let (rules, coverage) =
                            compile_webkit(&mut prepared, accepted_rules, self.limits, pruned)?;
                        let blocking_entries = coverage.blocking_rule_entries;
                        (None, None, Some(rules), Some(coverage), blocking_entries)
                    }
                    #[cfg(not(feature = "webkit"))]
                    {
                        return Err(CompileError::TargetUnavailable);
                    }
                }
            };
        let source_reports = prepared
            .iter()
            .map(|source| source.report.clone())
            .collect();
        let report = CompilationReport {
            sources: source_reports,
            total_source_bytes,
            accepted_rules,
            native_blocking_rule_entries,
            runtime: runtime_coverage,
            webkit: webkit_coverage,
        };

        Ok(CompiledRules::new(
            target,
            runtime,
            digest,
            webkit,
            report,
            self.limits,
            cosmetics,
        ))
    }
}

impl Default for Compiler {
    fn default() -> Self {
        Self::new(CompileLimits::default())
    }
}

#[cfg(feature = "webkit")]
#[derive(Clone)]
struct AcceptedRule {
    format: SourceFormat,
    webkit_failure: Option<WebKitDropReason>,
    attribution_sensitive: bool,
}

struct PreparedSource {
    id: SourceId,
    format: SourceFormat,
    trusted: bool,
    rules: String,
    cosmetics: String,
    #[cfg(feature = "webkit")]
    accepted: Vec<AcceptedRule>,
    report: SourceReport,
}

fn prepare_source(
    target: CompileTarget,
    source: FilterSource,
    limits: CompileLimits,
    accepted_total: &mut usize,
    candidate_total: &mut usize,
    physical_line_total: &mut usize,
) -> Result<PreparedSource, CompileError> {
    let options = parse_options(source.format);
    let mut rules = String::new();
    let mut cosmetics = String::new();
    #[cfg(feature = "webkit")]
    let mut accepted = Vec::new();
    let mut input_drops = BTreeMap::new();
    let mut total_lines = 0usize;
    let mut ignored_lines = 0usize;
    let mut source_accepted_rules = 0usize;
    let mut attribution_sensitive_rules = 0usize;
    let mut runtime_omitted_rules = 0usize;
    let mut runtime_approximated_rules = 0usize;
    let mut runtime_resource_approximated_rules = 0usize;
    let mut runtime_source_kind_approximated_rules = 0usize;
    let mut conditions = crate::conditions::ListConditions::default();

    for (line_index, raw_line) in source.contents.split('\n').enumerate() {
        *physical_line_total = physical_line_total
            .checked_add(1)
            .ok_or(CompileError::RuleCountOverflow)?;
        if *physical_line_total > limits.max_physical_lines() {
            return Err(CompileError::TooManyPhysicalLines {
                actual: *physical_line_total,
                limit: limits.max_physical_lines(),
            });
        }
        total_lines = total_lines
            .checked_add(1)
            .ok_or(CompileError::RuleCountOverflow)?;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.len() > limits.max_line_bytes() {
            return Err(CompileError::LineTooLong {
                id: source.id.clone(),
                line: line_index + 1,
                actual: line.len(),
                limit: limits.max_line_bytes(),
            });
        }
        let line = line.trim();
        if matches!(source.format, SourceFormat::Standard) && !conditions.keep(line) {
            ignored_lines += 1;
            continue;
        }
        if is_ignored_line(source.format, line) {
            ignored_lines += 1;
            continue;
        }
        if matches!(source.format, SourceFormat::Standard) && looks_like_cosmetic_rule(line) {
            *candidate_total = candidate_total
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
            if *candidate_total > limits.max_rules() {
                return Err(CompileError::TooManyRules {
                    actual: *candidate_total,
                    limit: limits.max_rules(),
                });
            }
            cosmetics.push_str(line);
            cosmetics.push('\n');
            increment(&mut input_drops, InputDropReason::UnsupportedCosmeticRule);
            continue;
        }

        *candidate_total = candidate_total
            .checked_add(1)
            .ok_or(CompileError::RuleCountOverflow)?;
        if *candidate_total > limits.max_rules() {
            return Err(CompileError::TooManyRules {
                actual: *candidate_total,
                limit: limits.max_rules(),
            });
        }

        let parsed = match parse_filter(line, true, options) {
            Ok(parsed) => parsed,
            Err(error) => {
                increment(&mut input_drops, parse_drop_reason(&error));
                continue;
            }
        };

        if matches!(&parsed, ParsedLine::Network(filter) if filter.is_generic_hide()) {
            cosmetics.push_str(line);
            cosmetics.push('\n');
        }
        let admission = inspect_rule(target, line, parsed)?;
        let (
            webkit_failure,
            attribution_sensitive,
            runtime_omitted,
            runtime_resource_approximated,
            runtime_source_kind_approximated,
        ) = match admission {
            Admission::Drop(reason) => {
                increment(&mut input_drops, reason);
                continue;
            }
            Admission::Accept {
                webkit_failure,
                attribution_sensitive,
                runtime_omitted,
                runtime_resource_approximated,
                runtime_source_kind_approximated,
            } => (
                webkit_failure,
                attribution_sensitive,
                runtime_omitted,
                runtime_resource_approximated,
                runtime_source_kind_approximated,
            ),
        };
        #[cfg(not(feature = "webkit"))]
        let _ = webkit_failure;

        let start = rules.len();
        rules.push_str(line);
        rules.push('\n');
        #[cfg(feature = "webkit")]
        if matches!(target, CompileTarget::WebKit) {
            accepted.push(AcceptedRule {
                format: source.format,
                webkit_failure,
                attribution_sensitive,
            });
        }
        debug_assert_eq!(&rules[start..rules.len() - 1], line);
        source_accepted_rules = source_accepted_rules
            .checked_add(1)
            .ok_or(CompileError::RuleCountOverflow)?;
        *accepted_total = accepted_total
            .checked_add(1)
            .ok_or(CompileError::RuleCountOverflow)?;
        if attribution_sensitive {
            attribution_sensitive_rules = attribution_sensitive_rules
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
        }
        if runtime_omitted {
            runtime_omitted_rules = runtime_omitted_rules
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
        }
        if runtime_resource_approximated || runtime_source_kind_approximated {
            runtime_approximated_rules = runtime_approximated_rules
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
        }
        if runtime_resource_approximated {
            runtime_resource_approximated_rules = runtime_resource_approximated_rules
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
        }
        if runtime_source_kind_approximated {
            runtime_source_kind_approximated_rules = runtime_source_kind_approximated_rules
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
        }
    }

    let report = SourceReport {
        id: source.id.clone(),
        total_lines,
        ignored_lines,
        accepted_rules: source_accepted_rules,
        attribution_sensitive_rules,
        runtime_omitted_rules,
        runtime_approximated_rules,
        runtime_resource_approximated_rules,
        runtime_source_kind_approximated_rules,
        dropped: input_counts(input_drops),
    };
    Ok(PreparedSource {
        id: source.id,
        format: source.format,
        trusted: source.trusted,
        rules,
        cosmetics,
        #[cfg(feature = "webkit")]
        accepted,
        report,
    })
}

enum Admission {
    Accept {
        webkit_failure: Option<WebKitDropReason>,
        attribution_sensitive: bool,
        runtime_omitted: bool,
        runtime_resource_approximated: bool,
        runtime_source_kind_approximated: bool,
    },
    Drop(InputDropReason),
}

fn inspect_rule(
    target: CompileTarget,
    raw_line: &str,
    parsed: ParsedLine<'_>,
) -> Result<Admission, CompileError> {
    match parsed {
        ParsedLine::Network(filter) => {
            if filter.is_redirect() {
                return Ok(Admission::Drop(InputDropReason::ForbiddenRedirect));
            }
            if filter.is_csp() {
                return Ok(Admission::Drop(InputDropReason::ForbiddenCsp));
            }
            if filter.is_removeparam() {
                return Ok(Admission::Drop(InputDropReason::ForbiddenRemoveParam));
            }
            if filter.is_generic_hide() {
                return Ok(Admission::Drop(InputDropReason::ForbiddenGenericHide));
            }
            if has_named_option(raw_line, "tag") {
                return Ok(Admission::Drop(InputDropReason::UnsupportedTag));
            }
            if has_invalid_domain_predicate(raw_line) {
                return Ok(Admission::Drop(InputDropReason::InvalidDomainPredicate));
            }
            if matches!(target, CompileTarget::Runtime)
                && has_unsupported_method_predicate(raw_line)
            {
                return Ok(Admission::Drop(InputDropReason::UnsupportedMethodPredicate));
            }
            #[cfg(feature = "runtime")]
            if matches!(target, CompileTarget::Runtime) && !runtime_regex_compiles(&filter) {
                return Ok(Admission::Drop(
                    InputDropReason::UnsupportedRegularExpression,
                ));
            }
            let attribution_sensitive = filter.opt_domains.is_some()
                || filter.opt_not_domains.is_some()
                || !filter
                    .mask
                    .contains(NetworkFilterMask::FIRST_PARTY | NetworkFilterMask::THIRD_PARTY);
            let runtime_coverage = runtime_native_coverage(filter.mask);
            let runtime_omitted = matches!(target, CompileTarget::Runtime)
                && (attribution_sensitive || runtime_coverage.resource_omitted);
            let runtime_resource_approximated = matches!(target, CompileTarget::Runtime)
                && !runtime_omitted
                && runtime_coverage.resource_approximated;
            let runtime_source_kind_approximated = matches!(target, CompileTarget::Runtime)
                && !runtime_omitted
                && runtime_coverage.source_kind_approximated;
            if filter.is_badfilter() {
                return Ok(Admission::Accept {
                    webkit_failure: Some(WebKitDropReason::BadFilterControl),
                    attribution_sensitive,
                    runtime_omitted,
                    runtime_resource_approximated,
                    runtime_source_kind_approximated,
                });
            }
            if matches!(target, CompileTarget::Runtime) {
                return Ok(Admission::Accept {
                    webkit_failure: None,
                    attribution_sensitive,
                    runtime_omitted,
                    runtime_resource_approximated,
                    runtime_source_kind_approximated,
                });
            }
            #[cfg(not(feature = "webkit"))]
            {
                Err(CompileError::TargetUnavailable)
            }
            #[cfg(feature = "webkit")]
            {
                if filter.mask.contains(NetworkFilterMask::IS_IMPORTANT) {
                    return Ok(Admission::Accept {
                        webkit_failure: Some(WebKitDropReason::ImportantPriority),
                        attribution_sensitive,
                        runtime_omitted,
                        runtime_resource_approximated,
                        runtime_source_kind_approximated,
                    });
                }
                if filter.mask.intersects(NetworkFilterMask::FROM_ANY_METHODS) {
                    return Ok(Admission::Accept {
                        webkit_failure: Some(WebKitDropReason::RequestMethod),
                        attribution_sensitive,
                        runtime_omitted,
                        runtime_resource_approximated,
                        runtime_source_kind_approximated,
                    });
                }
                Ok(Admission::Accept {
                    webkit_failure: webkit_equivalent_without_resource_semantics(filter)
                        .err()
                        .map(map_webkit_failure)
                        .transpose()?,
                    attribution_sensitive,
                    runtime_omitted,
                    runtime_resource_approximated,
                    runtime_source_kind_approximated,
                })
            }
        }
        // Cosmetic filtering needs a separate, cross-platform document-world
        // pipeline. Admitting it only into WebKit would make Windows policy
        // materially weaker while reporting one common enabled state.
        ParsedLine::Cosmetic(_) => Ok(Admission::Drop(InputDropReason::UnsupportedCosmeticRule)),
    }
}

#[cfg(feature = "webkit")]
fn webkit_equivalent_without_resource_semantics(
    mut filter: adblock::filters::network::NetworkFilter<'_>,
) -> Result<CbRuleEquivalent, CbRuleCreationFailure> {
    // The upstream converter targets an older Safari resource vocabulary.
    // Zephium serializes the current WebKit resource tokens itself, so use one
    // harmless legacy type only to obtain the audited URL/domain/load/action
    // conversion and replace its resource set before serialization.
    filter.mask.remove(NetworkFilterMask::FROM_ALL_TYPES);
    filter.mask.insert(NetworkFilterMask::FROM_IMAGE);
    CbRuleEquivalent::try_from(filter)
}

#[derive(Clone, Copy)]
struct RuntimeNativeCoverage {
    resource_omitted: bool,
    resource_approximated: bool,
    source_kind_approximated: bool,
}

fn runtime_native_coverage(mask: NetworkFilterMask) -> RuntimeNativeCoverage {
    let supported = NetworkFilterMask::FROM_STYLESHEET
        | NetworkFilterMask::FROM_IMAGE
        | NetworkFilterMask::FROM_MEDIA
        | NetworkFilterMask::FROM_FONT
        | NetworkFilterMask::FROM_SCRIPT
        | NetworkFilterMask::FROM_XMLHTTPREQUEST
        | NetworkFilterMask::FROM_PING;
    let requested = mask & NetworkFilterMask::FROM_ALL_TYPES;
    let unsupported = requested - supported;
    let resource_approximated = !unsupported.is_empty();
    let resource_omitted = resource_approximated && (requested & supported).is_empty();
    // V1 registers every supported context only for WebView2's DOCUMENT
    // request-source kind. Do not infer that a context can never be emitted
    // by a shared/service worker: the native contract exposes those source
    // kinds independently, and future runtimes may expand their context
    // routing. Every represented rule therefore has conservatively partial
    // source-kind reachability.
    RuntimeNativeCoverage {
        resource_omitted,
        resource_approximated,
        source_kind_approximated: !(requested & supported).is_empty(),
    }
}

#[cfg(feature = "webkit")]
fn map_webkit_failure(failure: CbRuleCreationFailure) -> Result<WebKitDropReason, CompileError> {
    match failure {
        CbRuleCreationFailure::NeedsDebugMode => Err(CompileError::UpstreamInvariant),
        CbRuleCreationFailure::UnlessAndIfDomainTogetherUnsupported => {
            Ok(WebKitDropReason::MixedDomainConditions)
        }
        CbRuleCreationFailure::NoSupportedNetworkOptions(_) => {
            Ok(WebKitDropReason::UnsupportedResourceTypes)
        }
        CbRuleCreationFailure::NetworkBadFilterUnsupported => {
            Ok(WebKitDropReason::BadFilterControl)
        }
        CbRuleCreationFailure::FullRegexUnsupported => Ok(WebKitDropReason::FullRegularExpression),
        CbRuleCreationFailure::OptimizedRulesUnsupported => Ok(WebKitDropReason::OptimizedRule),
        CbRuleCreationFailure::CosmeticEntitiesUnsupported => Ok(WebKitDropReason::CosmeticEntity),
        CbRuleCreationFailure::RuleContainsNonASCII => Ok(WebKitDropReason::NonAscii),
        CbRuleCreationFailure::InvalidDomain => Ok(WebKitDropReason::InvalidDomain),
        CbRuleCreationFailure::FromNotSupported => Ok(WebKitDropReason::FromAlias),
        CbRuleCreationFailure::NetworkRedirectUnsupported
        | CbRuleCreationFailure::NetworkGenerichideUnsupported
        | CbRuleCreationFailure::NetworkCspUnsupported
        | CbRuleCreationFailure::NetworkRemoveparamUnsupported
        | CbRuleCreationFailure::CosmeticActionRulesNotSupported
        | CbRuleCreationFailure::ScriptletInjectionsNotSupported
        | CbRuleCreationFailure::ProceduralCosmeticFiltersUnsupported => {
            Err(CompileError::AdmissionInvariant)
        }
    }
}

fn parse_drop_reason(error: &FilterParseError) -> InputDropReason {
    match error {
        FilterParseError::Network(_) => InputDropReason::InvalidNetworkRule,
        FilterParseError::Cosmetic(_) => InputDropReason::InvalidCosmeticRule,
        FilterParseError::Unsupported
        | FilterParseError::Empty
        | FilterParseError::InvalidExpiresInterval => InputDropReason::UnsupportedRule,
    }
}

/// A source's physical lines, and an upper bound on its candidate rules (lines
/// the compiler does not skip as comments or blanks), as the compiler counts
/// them against the line and rule ceilings.
pub(crate) fn line_counts(format: SourceFormat, text: &str) -> (usize, usize) {
    let mut lines = 0;
    let mut rules = 0;
    for line in text.split('\n') {
        lines += 1;
        let line = line.strip_suffix('\r').unwrap_or(line).trim();
        if !is_ignored_line(format, line) {
            rules += 1;
        }
    }
    (lines, rules)
}

fn is_ignored_line(format: SourceFormat, line: &str) -> bool {
    if line.is_empty() {
        return true;
    }
    match format {
        SourceFormat::Hosts => matches!(line.as_bytes().first(), Some(b'!' | b'#')),
        SourceFormat::Standard => {
            line.starts_with('!')
                || line.starts_with("[Adblock")
                || line
                    .strip_prefix('#')
                    .and_then(|tail| tail.chars().next())
                    .is_some_and(char::is_whitespace)
        }
    }
}

fn parse_options(format: SourceFormat) -> ParseOptions {
    ParseOptions {
        format: format.adblock(),
        rule_types: RuleTypes::NetworkOnly,
        // PermissionMask::default() is zero. This crate never exposes resource
        // storage, and scriptlet/redirect rules are rejected separately.
        ..ParseOptions::default()
    }
}

pub(crate) fn looks_like_cosmetic_rule(line: &str) -> bool {
    if line.starts_with('|') || line.starts_with("@@|") {
        return false;
    }
    let bytes = line.as_bytes();
    let Some(sharp) = bytes.iter().position(|byte| *byte == b'#') else {
        return false;
    };
    let tail = &bytes[sharp.saturating_add(1)..(sharp.saturating_add(5)).min(bytes.len())];
    tail.contains(&b'#')
}

#[cfg(feature = "runtime")]
/// Whether a regex-backed rule compiles the way the runtime matcher compiles
/// it, so one rule the regex engine refuses cannot fail the whole policy.
fn runtime_regex_compiles(filter: &adblock::filters::network::NetworkFilter) -> bool {
    use adblock::filters::network::FilterPart;
    let patterns: Vec<&str> = match &filter.filter {
        FilterPart::Empty => Vec::new(),
        FilterPart::Simple(pattern) => vec![pattern.as_ref()],
        FilterPart::AnyOf(patterns) => patterns.iter().map(AsRef::as_ref).collect(),
    };
    adblock::regex_manager::regex_compiles_within(
        filter.mask,
        &patterns,
        MAX_RUNTIME_REGEX_SIZE_BYTES,
        MAX_RUNTIME_REGEX_DFA_SIZE_BYTES,
    )
}

fn has_named_option(line: &str, expected: &str) -> bool {
    let Some((_, options)) = line.rsplit_once('$') else {
        return false;
    };
    options.split(',').any(|option| {
        option
            .strip_prefix('~')
            .unwrap_or(option)
            .split_once('=')
            .map_or(option, |(name, _)| name)
            .eq_ignore_ascii_case(expected)
    })
}

fn has_unsupported_method_predicate(line: &str) -> bool {
    let Some((_, options)) = line.rsplit_once('$') else {
        return false;
    };
    options.split(',').any(|option| {
        option.split_once('=').is_some_and(|(name, values)| {
            name.eq_ignore_ascii_case("method")
                && values.split('|').any(|value| {
                    value.starts_with('~')
                        || !matches!(value.to_ascii_lowercase().as_str(), "get" | "head" | "post")
                })
        })
    })
}

fn has_invalid_domain_predicate(line: &str) -> bool {
    let Some((_, options)) = line.rsplit_once('$') else {
        return false;
    };
    options.split(',').any(|option| {
        let Some((name, values)) = option.split_once('=') else {
            return false;
        };
        if !name.eq_ignore_ascii_case("domain") && !name.eq_ignore_ascii_case("from") {
            return false;
        }
        values.split('|').any(|value| {
            let value = value.strip_prefix('~').unwrap_or(value);
            normalize_domain_predicate(value).is_none()
        })
    })
}

fn normalize_domain_predicate(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    let normalized = idna::domain_to_ascii(&value.to_lowercase()).ok()?;
    if normalized.is_empty()
        || normalized.len() > 253
        || normalized.starts_with('.')
        || normalized.ends_with('.')
        || normalized.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return None;
    }
    Some(normalized)
}

#[cfg(feature = "runtime")]
fn compile_runtime(
    prepared: &mut [PreparedSource],
    limits: CompileLimits,
) -> Result<(Engine, usize, RuntimeCoverage), CompileError> {
    let mut filters = FilterSet::new(false);
    for source in prepared {
        filters.add_filter_list(
            std::mem::take(&mut source.rules),
            parse_options(source.format),
        );
    }
    let engine = Engine::new_with_filter_set(filters);
    let (blocking_entries, coverage) = prepare_and_validate_runtime_engine(&engine, limits)?;
    Ok((engine, blocking_entries, coverage))
}

#[cfg(feature = "runtime")]
pub(crate) fn prepare_and_validate_runtime_engine(
    engine: &Engine,
    limits: CompileLimits,
) -> Result<(usize, RuntimeCoverage), CompileError> {
    let preparation_limits = NetworkMatcherPreparationLimits {
        max_regexes: MAX_RUNTIME_REGEXES.min(limits.max_rules()),
        max_pattern_bytes: MAX_RUNTIME_PATTERN_BYTES.min(limits.max_total_source_bytes()),
        max_patterns_per_regex: MAX_RUNTIME_PATTERNS_PER_REGEX,
        max_pattern_bytes_per_regex: MAX_RUNTIME_PATTERN_BYTES_PER_REGEX,
        max_regex_size_bytes: MAX_RUNTIME_REGEX_SIZE_BYTES,
        max_regex_dfa_size_bytes: MAX_RUNTIME_REGEX_DFA_SIZE_BYTES,
        max_filter_checks_per_request: MAX_RUNTIME_FILTER_CHECKS_PER_REQUEST,
    };
    let preparation = engine
        .prepare_and_freeze_network_matcher(preparation_limits)
        .map_err(map_runtime_preparation_error)?;
    let blocking_entries = engine
        .source_independent_blocking_filter_entry_count_for_types(WEBVIEW2_AUDITED_REQUEST_TYPES);
    if blocking_entries == 0 {
        return Err(CompileError::NoNativeBlockingRules);
    }
    Ok((
        blocking_entries,
        RuntimeCoverage {
            regexes: preparation.regexes,
            omitted_regexes: preparation.omitted_regexes,
            regex_limit: preparation_limits.max_regexes,
            pattern_bytes: preparation.pattern_bytes,
            pattern_bytes_limit: preparation_limits.max_pattern_bytes,
            largest_regex_patterns: preparation.largest_regex_patterns,
            patterns_per_regex_limit: preparation_limits.max_patterns_per_regex,
            largest_regex_pattern_bytes: preparation.largest_regex_pattern_bytes,
            pattern_bytes_per_regex_limit: preparation_limits.max_pattern_bytes_per_regex,
            regex_size_limit_bytes: preparation.regex_size_limit_bytes,
            regex_dfa_size_limit_bytes: preparation.regex_dfa_size_limit_bytes,
            max_filter_checks_per_request: preparation.max_filter_checks_per_request,
        },
    ))
}

#[cfg(feature = "runtime")]
const fn map_runtime_preparation_error(error: NetworkMatcherPreparationError) -> CompileError {
    match error {
        NetworkMatcherPreparationError::InvalidLimits => CompileError::RuntimePreparationInvariant,
        NetworkMatcherPreparationError::RegexLimitExceeded => {
            CompileError::RuntimeRegexBudgetExceeded
        }
        NetworkMatcherPreparationError::PatternBytesOverflow => {
            CompileError::RuntimePatternBytesOverflow
        }
        NetworkMatcherPreparationError::PatternBytesLimitExceeded => {
            CompileError::RuntimePatternBytesBudgetExceeded
        }
        NetworkMatcherPreparationError::PatternsPerRegexLimitExceeded => {
            CompileError::RuntimeRegexBucketBudgetExceeded
        }
        NetworkMatcherPreparationError::PatternBytesPerRegexLimitExceeded => {
            CompileError::RuntimeRegexBucketBytesBudgetExceeded
        }
        NetworkMatcherPreparationError::RegexCompilationFailed => {
            CompileError::RuntimeRegexCompilationFailed
        }
        NetworkMatcherPreparationError::PreparedLimitMismatch => {
            CompileError::RuntimePreparationInvariant
        }
        NetworkMatcherPreparationError::LockUnavailable => {
            CompileError::RuntimePreparationInvariant
        }
    }
}

#[cfg(feature = "webkit")]
fn compile_webkit(
    prepared: &mut [PreparedSource],
    accepted_rules: usize,
    limits: CompileLimits,
    pruned: &crate::webkit_prune::PrunedRules,
) -> Result<(WebKitRules, WebKitCoverage), CompileError> {
    let mut bad_filter_ids = HashSet::new();
    for source in &*prepared {
        for line in source.rules.lines() {
            let ParsedLine::Network(filter) =
                parse_filter(line, true, parse_options(source.format))
                    .map_err(|_| CompileError::UpstreamInvariant)?
            else {
                return Err(CompileError::AdmissionInvariant);
            };
            if filter.is_badfilter() {
                bad_filter_ids.insert(filter.get_id());
            }
        }
    }

    let mut drops = BTreeMap::new();
    let mut pruned_rules = 0usize;
    let mut converted = 0usize;
    let mut converted_approximated = 0usize;
    let mut converted_attribution_approximated = 0usize;
    let mut converted_resource_approximated = 0usize;
    let mut blocking_rules = Vec::new();
    let mut exception_rules = Vec::new();
    for source in &*prepared {
        let mut lines = source.rules.lines();
        for descriptor in &source.accepted {
            let line = lines.next().ok_or(CompileError::AdmissionInvariant)?;
            if let Some(failure) = descriptor.webkit_failure {
                increment(&mut drops, failure);
                continue;
            }
            let ParsedLine::Network(filter) =
                parse_filter(line, true, parse_options(descriptor.format))
                    .map_err(|_| CompileError::UpstreamInvariant)?
            else {
                return Err(CompileError::AdmissionInvariant);
            };
            if filter.is_badfilter() {
                return Err(CompileError::AdmissionInvariant);
            }
            if bad_filter_ids.contains(&filter.get_id()) {
                increment(&mut drops, WebKitDropReason::SuppressedByRuleSemantics);
                continue;
            }
            let (resource_types, resource_approximated) = webkit_resource_types(filter.mask);
            if resource_types.is_empty() {
                return Err(CompileError::AdmissionInvariant);
            }
            let equivalent = webkit_equivalent_without_resource_semantics(filter)
                .map_err(|_| CompileError::UpstreamInvariant)?;
            let mut native_rules: Vec<_> = equivalent.into_iter().collect();
            for native in &mut native_rules {
                if let Some(pattern) =
                    crate::webkit::simplify_canonical_host_boundary(&native.trigger.url_filter)
                {
                    native.trigger.url_filter = pattern;
                }
            }
            if native_rules.is_empty() {
                return Err(CompileError::UpstreamInvariant);
            }
            if native_rules
                .iter()
                .any(|native| native.trigger.url_filter.len() > MAX_WEBKIT_URL_FILTER_BYTES)
            {
                increment(&mut drops, WebKitDropReason::UrlFilterTooLarge);
                continue;
            }
            if source.id.as_str() == NAVIGATION_CHECKED_SOURCE
                && native_rules.iter().any(|native| {
                    matches!(native.action.typ, CbType::Block)
                        && native.trigger.url_filter.contains(".*")
                })
            {
                increment(&mut drops, WebKitDropReason::NavigationChecked);
                continue;
            }
            let mut kept = Vec::with_capacity(native_rules.len());
            for native in native_rules {
                let converted_rule = ConvertedRule {
                    native,
                    resource_types: resource_types.clone(),
                };
                if matches!(converted_rule.native.action.typ, CbType::Block)
                    && converted_rule.native.trigger.url_filter.contains(".*")
                    && pruned.contains(
                        &serde_json::to_vec(&canonical(&converted_rule))
                            .map_err(CompileError::SerializeWebKit)?,
                    )
                {
                    continue;
                }
                kept.push(converted_rule);
            }
            if kept.is_empty() {
                pruned_rules = pruned_rules
                    .checked_add(1)
                    .ok_or(CompileError::RuleCountOverflow)?;
                continue;
            }
            let emitted = blocking_rules
                .len()
                .checked_add(exception_rules.len())
                .and_then(|emitted| emitted.checked_add(kept.len()))
                .ok_or(CompileError::RuleCountOverflow)?;
            if emitted > limits.max_webkit_rules() {
                return Err(CompileError::TooManyWebKitRules {
                    actual: emitted,
                    limit: limits.max_webkit_rules(),
                });
            }
            for converted_rule in kept {
                let destination = match converted_rule.native.action.typ {
                    CbType::Block => &mut blocking_rules,
                    CbType::IgnorePreviousRules => &mut exception_rules,
                    // The product policy is network cancellation only. Keep this
                    // terminal gate after conversion as well as parser admission:
                    // a future upstream converter must not silently introduce
                    // cookie mutation, cosmetic injection, or URL upgrading.
                    CbType::BlockCookies | CbType::CssDisplayNone | CbType::MakeHttps => {
                        return Err(CompileError::AdmissionInvariant);
                    }
                };
                destination.push(converted_rule);
            }
            converted = converted
                .checked_add(1)
                .ok_or(CompileError::RuleCountOverflow)?;
            if descriptor.attribution_sensitive || resource_approximated {
                converted_approximated = converted_approximated
                    .checked_add(1)
                    .ok_or(CompileError::RuleCountOverflow)?;
            }
            if descriptor.attribution_sensitive {
                converted_attribution_approximated = converted_attribution_approximated
                    .checked_add(1)
                    .ok_or(CompileError::RuleCountOverflow)?;
            }
            if resource_approximated {
                converted_resource_approximated = converted_resource_approximated
                    .checked_add(1)
                    .ok_or(CompileError::RuleCountOverflow)?;
            }
        }
        if lines.next().is_some() {
            return Err(CompileError::AdmissionInvariant);
        }
    }
    if blocking_rules.is_empty() {
        return Err(CompileError::NoNativeBlockingRules);
    }
    let blocking_rule_entries = blocking_rules.len();
    blocking_rules.extend(exception_rules);
    let rules = blocking_rules;

    let json = canonical_webkit_json(&rules, limits.max_webkit_json_bytes())?;

    let coverage = WebKitCoverage {
        accepted_input_rules: accepted_rules,
        converted_input_rules: converted,
        pruned_input_rules: pruned_rules,
        approximated_input_rules: converted_approximated,
        attribution_approximated_input_rules: converted_attribution_approximated,
        resource_approximated_input_rules: converted_resource_approximated,
        blocking_rule_entries,
        emitted_rules: rules.len(),
        json_bytes: json.len(),
        dropped: webkit_counts(drops),
    };
    let mut artifact_digest = Sha256::new();
    artifact_digest.update(WEBKIT_ARTIFACT_DIGEST_DOMAIN);
    artifact_digest.update(WEBKIT_ARTIFACT_FORMAT_VERSION.to_be_bytes());
    artifact_digest.update(json.as_bytes());
    Ok((
        WebKitRules {
            json: Arc::from(json),
            rule_count: rules.len(),
            digest: ArtifactDigest(artifact_digest.finalize()),
        },
        coverage,
    ))
}

fn digest_policy(prepared: &[PreparedSource]) -> PolicyDigest {
    let mut digest = Sha256::new();
    digest.update(DIGEST_DOMAIN);
    digest.update(POLICY_FORMAT_VERSION.to_be_bytes());
    digest.update(ADBLOCK_ENGINE_VERSION.as_bytes());
    digest.update((prepared.len() as u64).to_be_bytes());
    for source in prepared {
        digest.update((source.id.as_str().len() as u64).to_be_bytes());
        digest.update(source.id.as_str().as_bytes());
        digest.update([source.format.digest_tag()]);
        digest.update((source.rules.len() as u64).to_be_bytes());
        digest.update(source.rules.as_bytes());
    }
    PolicyDigest(digest.finalize())
}

#[cfg(feature = "webkit")]
#[derive(Serialize)]
struct CanonicalRule<'a> {
    action: CanonicalAction<'a>,
    trigger: CanonicalTrigger<'a>,
}

#[cfg(feature = "webkit")]
struct ConvertedRule {
    native: CbRule,
    resource_types: Vec<WebKitResourceType>,
}

#[cfg(feature = "webkit")]
#[derive(Serialize)]
struct CanonicalAction<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<&'a str>,
}

#[cfg(feature = "webkit")]
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct CanonicalTrigger<'a> {
    url_filter: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url_filter_is_case_sensitive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    if_domain: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unless_domain: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource_type: Option<&'a [WebKitResourceType]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    load_type: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    if_top_url: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unless_top_url: Option<&'a [String]>,
}

/// One rule as the WebKit artifact holds it; pruned-rule digests are taken
/// over exactly these bytes.
#[cfg(feature = "webkit")]
fn canonical(rule: &ConvertedRule) -> CanonicalRule<'_> {
    CanonicalRule {
        action: CanonicalAction {
            kind: webkit_action_name(&rule.native.action.typ),
            selector: rule.native.action.selector.as_deref(),
        },
        trigger: CanonicalTrigger {
            url_filter: &rule.native.trigger.url_filter,
            url_filter_is_case_sensitive: rule.native.trigger.url_filter_is_case_sensitive,
            if_domain: rule.native.trigger.if_domain.as_deref(),
            unless_domain: rule.native.trigger.unless_domain.as_deref(),
            resource_type: Some(&rule.resource_types),
            load_type: rule
                .native
                .trigger
                .load_type
                .iter()
                .map(webkit_load_name)
                .collect(),
            if_top_url: rule.native.trigger.if_top_url.as_deref(),
            unless_top_url: rule.native.trigger.unless_top_url.as_deref(),
        },
    }
}

#[cfg(feature = "webkit")]
fn canonical_webkit_json(
    rules: &[ConvertedRule],
    byte_limit: usize,
) -> Result<String, CompileError> {
    use serde::ser::{SerializeSeq, Serializer};
    let mut output = BoundedJsonWriter::new(byte_limit);
    let result = (|| -> Result<(), serde_json::Error> {
        let mut serializer = serde_json::Serializer::new(&mut output);
        let mut sequence = serializer.serialize_seq(Some(rules.len()))?;
        for rule in rules {
            sequence.serialize_element(&canonical(rule))?;
        }
        sequence.end()
    })();
    if let Err(error) = result {
        if let Some(actual) = output.exceeded_at {
            return Err(CompileError::WebKitJsonTooLarge {
                actual,
                limit: byte_limit,
            });
        }
        return Err(CompileError::SerializeWebKit(error));
    }
    String::from_utf8(output.bytes).map_err(|_| CompileError::UpstreamInvariant)
}

#[cfg(feature = "webkit")]
const fn webkit_action_name(action: &CbType) -> &'static str {
    match action {
        CbType::Block => "block",
        CbType::BlockCookies => "block-cookies",
        CbType::CssDisplayNone => "css-display-none",
        CbType::IgnorePreviousRules => "ignore-previous-rules",
        CbType::MakeHttps => "make-https",
    }
}

#[cfg(feature = "webkit")]
const fn webkit_load_name(load: &CbLoadType) -> &'static str {
    match load {
        CbLoadType::FirstParty => "first-party",
        CbLoadType::ThirdParty => "third-party",
    }
}

#[cfg(feature = "webkit")]
fn webkit_resource_types(mask: NetworkFilterMask) -> (Vec<WebKitResourceType>, bool) {
    (
        WebKitResourceType::for_mask(mask),
        mask.intersects(NetworkFilterMask::FROM_OBJECT | NetworkFilterMask::FROM_OTHER),
    )
}

#[cfg(feature = "webkit")]
struct BoundedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded_at: Option<usize>,
}

#[cfg(feature = "webkit")]
impl BoundedJsonWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(1024 * 1024)),
            limit,
            exceeded_at: None,
        }
    }
}

#[cfg(feature = "webkit")]
impl std::io::Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(next_len) = self.bytes.len().checked_add(bytes.len()) else {
            self.exceeded_at = Some(usize::MAX);
            return Err(std::io::Error::other("content-rule JSON length overflow"));
        };
        if next_len > self.limit {
            self.exceeded_at = Some(next_len);
            return Err(std::io::Error::other(
                "content-rule JSON exceeds audited limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn increment<K: Ord>(counts: &mut BTreeMap<K, usize>, key: K) {
    let count = counts.entry(key).or_default();
    *count = count.saturating_add(1);
}

/// Compilation failed before a complete generation could be published.
#[derive(Debug, Error)]
pub enum CompileError {
    /// Static cosmetics could not be admitted within their syntax/resource bounds.
    #[error("static cosmetic policy failed: {0}")]
    Cosmetics(crate::CosmeticError),
    /// The crate was built without the requested native artifact feature.
    #[error("requested blocker artifact target is not enabled")]
    TargetUnavailable,
    /// Enabled compilation requires at least one authenticated source.
    #[error("enabled blocker policy has no filter sources")]
    NoSources,
    /// Every candidate rule was invalid or outside the audited v1 capability.
    #[error("enabled blocker policy has no usable network rules")]
    NoUsableRules,
    /// Accepted input produced no blocking rule the selected native target can enforce.
    #[error("enabled blocker policy has no native blocking-rule entries")]
    NoNativeBlockingRules,
    /// Source count exceeded the configured budget.
    #[error("source count is {actual}; limit is {limit}")]
    TooManySources {
        /// Observed source count.
        actual: usize,
        /// Configured source count limit.
        limit: usize,
    },
    /// Two sources share a canonical identifier.
    #[error("duplicate filter source {id}")]
    DuplicateSource {
        /// Duplicated identifier.
        id: SourceId,
    },
    /// One source exceeded its configured byte budget.
    #[error("filter source {id} is {actual} bytes; limit is {limit}")]
    SourceTooLarge {
        /// Source identifier.
        id: SourceId,
        /// Observed bytes.
        actual: usize,
        /// Configured bytes.
        limit: usize,
    },
    /// Aggregate input length overflowed the target integer.
    #[error("total filter source length overflowed")]
    TotalSourceBytesOverflow,
    /// Aggregate input exceeded its configured byte budget.
    #[error("filter sources total {actual} bytes; limit is {limit}")]
    TotalSourceBytesExceeded {
        /// Observed bytes.
        actual: usize,
        /// Configured bytes.
        limit: usize,
    },
    /// The optimized runtime matcher exceeded its regex-count budget.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher exceeded its regex-count budget")]
    RuntimeRegexBudgetExceeded,
    /// Runtime regex pattern accounting overflowed.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher pattern bytes overflowed")]
    RuntimePatternBytesOverflow,
    /// The complete runtime matcher exceeded its aggregate pattern-byte budget.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher exceeded its aggregate pattern-byte budget")]
    RuntimePatternBytesBudgetExceeded,
    /// One optimized runtime regex bucket retained too many patterns.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher exceeded its per-regex pattern-count budget")]
    RuntimeRegexBucketBudgetExceeded,
    /// One optimized runtime regex bucket retained too many pattern bytes.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher exceeded its per-regex pattern-byte budget")]
    RuntimeRegexBucketBytesBudgetExceeded,
    /// A newly constructed runtime matcher was unexpectedly unavailable.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher preparation invariant failed")]
    RuntimePreparationInvariant,
    /// A parser-admitted runtime regex could not be compiled safely.
    #[cfg(feature = "runtime")]
    #[error("runtime matcher contains an un-compilable regex")]
    RuntimeRegexCompilationFailed,
    /// One physical line exceeded its configured byte budget.
    #[error("filter source {id} line {line} is {actual} bytes; limit is {limit}")]
    LineTooLong {
        /// Source identifier.
        id: SourceId,
        /// One-based physical line number.
        line: usize,
        /// Observed bytes.
        actual: usize,
        /// Configured bytes.
        limit: usize,
    },
    /// A rule counter overflowed the target integer.
    #[error("filter rule count overflowed")]
    RuleCountOverflow,
    /// Candidate input rules exceeded the configured budget.
    #[error("candidate rule count is at least {actual}; limit is {limit}")]
    TooManyRules {
        /// First count over budget.
        actual: usize,
        /// Configured rule limit.
        limit: usize,
    },
    /// Physical lines, including ignored comments/blanks, exceeded the
    /// configured worker-iteration budget.
    #[error("physical line count is at least {actual}; limit is {limit}")]
    TooManyPhysicalLines {
        /// First count over budget.
        actual: usize,
        /// Configured physical-line limit.
        limit: usize,
    },
    /// Converted WebKit rules exceeded the configured budget.
    #[error("WebKit rule count is {actual}; limit is {limit}")]
    TooManyWebKitRules {
        /// Emitted rule count.
        actual: usize,
        /// Configured rule limit.
        limit: usize,
    },
    /// Canonical WebKit JSON exceeded its configured budget.
    #[error("WebKit JSON is {actual} bytes; limit is {limit}")]
    WebKitJsonTooLarge {
        /// Emitted JSON bytes.
        actual: usize,
        /// Configured JSON byte limit.
        limit: usize,
    },
    /// Upstream JSON serialization failed.
    #[cfg(feature = "webkit")]
    #[error("failed to serialize canonical WebKit rules: {0}")]
    SerializeWebKit(#[source] serde_json::Error),
    /// Upstream violated a debug-mode invariant required for conversion.
    #[error("adblock-rust violated a content-conversion invariant")]
    UpstreamInvariant,
    /// A forbidden action passed Zephium's admission classifier.
    #[error("a forbidden rule action passed compiler admission")]
    AdmissionInvariant,
}
