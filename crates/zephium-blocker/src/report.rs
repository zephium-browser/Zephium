use std::collections::BTreeMap;

use crate::SourceId;

/// Exact bounded resources retained by a prepared runtime matcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeCoverage {
    pub(crate) regexes: usize,
    pub(crate) omitted_regexes: usize,
    pub(crate) regex_limit: usize,
    pub(crate) pattern_bytes: usize,
    pub(crate) pattern_bytes_limit: usize,
    pub(crate) largest_regex_patterns: usize,
    pub(crate) patterns_per_regex_limit: usize,
    pub(crate) largest_regex_pattern_bytes: usize,
    pub(crate) pattern_bytes_per_regex_limit: usize,
    pub(crate) regex_size_limit_bytes: usize,
    pub(crate) regex_dfa_size_limit_bytes: usize,
    pub(crate) max_filter_checks_per_request: usize,
}

impl RuntimeCoverage {
    /// Returns eagerly prepared regular-expression entries, the omitted ones
    /// that never match included.
    pub const fn regexes(self) -> usize {
        self.regexes
    }

    /// Returns regexes past the size limits, prepared as never matching; one
    /// can stand for several rules the optimizer merged.
    pub const fn omitted_regexes(self) -> usize {
        self.omitted_regexes
    }

    /// Returns the hard regular-expression count ceiling.
    pub const fn regex_limit(self) -> usize {
        self.regex_limit
    }

    /// Returns aggregate source-pattern bytes retained by regex objects.
    pub const fn pattern_bytes(self) -> usize {
        self.pattern_bytes
    }

    /// Returns the aggregate pattern-byte ceiling.
    pub const fn pattern_bytes_limit(self) -> usize {
        self.pattern_bytes_limit
    }

    /// Returns patterns in the largest compiled regex set.
    pub const fn largest_regex_patterns(self) -> usize {
        self.largest_regex_patterns
    }

    /// Returns the per-regex pattern-count ceiling.
    pub const fn patterns_per_regex_limit(self) -> usize {
        self.patterns_per_regex_limit
    }

    /// Returns source-pattern bytes in the largest compiled regex set.
    pub const fn largest_regex_pattern_bytes(self) -> usize {
        self.largest_regex_pattern_bytes
    }

    /// Returns the per-regex pattern-byte ceiling.
    pub const fn pattern_bytes_per_regex_limit(self) -> usize {
        self.pattern_bytes_per_regex_limit
    }

    /// Returns the regex program-size ceiling used for every object.
    pub const fn regex_size_limit_bytes(self) -> usize {
        self.regex_size_limit_bytes
    }

    /// Returns the regex DFA cache-size ceiling used for every object.
    pub const fn regex_dfa_size_limit_bytes(self) -> usize {
        self.regex_dfa_size_limit_bytes
    }

    /// Returns the exact shared candidate-evaluation ceiling per request.
    pub const fn max_filter_checks_per_request(self) -> usize {
        self.max_filter_checks_per_request
    }
}

/// Why a candidate input rule was excluded from every compiled artifact.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum InputDropReason {
    /// The adblock network-rule parser rejected the line.
    InvalidNetworkRule,
    /// The cosmetic-rule parser rejected the line.
    InvalidCosmeticRule,
    /// The line used syntax not recognized as a supported rule.
    UnsupportedRule,
    /// A cosmetic rule was excluded from the network-only v1 policy.
    ///
    /// Cosmetics require a separate cross-platform document-world pipeline;
    /// applying them only on WebKit would create silent platform divergence.
    UnsupportedCosmeticRule,
    /// A `$tag` rule cannot be enabled safely without an explicit tag policy.
    UnsupportedTag,
    /// The rule requested a redirect or redirect-rule resource.
    ForbiddenRedirect,
    /// The rule requested response CSP modification.
    ForbiddenCsp,
    /// The rule requested URL parameter rewriting.
    ForbiddenRemoveParam,
    /// The rule requested generic cosmetic-hiding control.
    ForbiddenGenericHide,
    /// The runtime matcher cannot exactly represent this negated or
    /// non-GET/HEAD/POST HTTP-method predicate.
    UnsupportedMethodPredicate,
    /// A `$domain` or `$from` condition contains a value that is not a
    /// canonicalizable DNS hostname.
    InvalidDomainPredicate,
    /// A regular-expression rule uses syntax the runtime matcher's regex
    /// engine cannot compile within its bounds, such as backreferences.
    UnsupportedRegularExpression,
}

/// A counted input-drop reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputDropCount {
    reason: InputDropReason,
    count: usize,
}

impl InputDropCount {
    /// Returns the exclusion reason.
    pub const fn reason(self) -> InputDropReason {
        self.reason
    }

    /// Returns the number of excluded rules.
    pub const fn count(self) -> usize {
        self.count
    }
}

/// Why a runtime-supported rule was not represented by WebKit content rules.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum WebKitDropReason {
    /// The rule combines positive and negative domain constraints.
    MixedDomainConditions,
    /// None of the rule's requested resource types exists in WebKit syntax.
    UnsupportedResourceTypes,
    /// The rule is a `$badfilter` control rule or was controlled by one.
    BadFilterControl,
    /// The rule used a full regular expression unsupported by WebKit syntax.
    FullRegularExpression,
    /// The rule was transformed to an internal optimized representation.
    OptimizedRule,
    /// The cosmetic rule targets domain entities rather than hostnames.
    CosmeticEntity,
    /// The converted rule contains non-ASCII data rejected by WebKit.
    NonAscii,
    /// A network-domain condition could not be represented as valid IDNA.
    InvalidDomain,
    /// The rule uses `from` as a domain alias.
    FromAlias,
    /// WebKit content rules cannot express an HTTP-method predicate.
    RequestMethod,
    /// WebKit's ordered exception model cannot preserve `$important`.
    ImportantPriority,
    /// Aggregate rule semantics, such as `$badfilter`, suppressed the rule.
    SuppressedByRuleSemantics,
    /// Wildcard/separator expansion produced a native URL regex above the
    /// per-rule compiler budget.
    UrlFilterTooLarge,
}

/// A counted WebKit conversion-loss reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebKitDropCount {
    reason: WebKitDropReason,
    count: usize,
}

impl WebKitDropCount {
    /// Returns the conversion-loss reason.
    pub const fn reason(self) -> WebKitDropReason {
        self.reason
    }

    /// Returns the number of affected input rules.
    pub const fn count(self) -> usize {
        self.count
    }
}

/// Compilation statistics for one named source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceReport {
    pub(crate) id: SourceId,
    pub(crate) total_lines: usize,
    pub(crate) ignored_lines: usize,
    pub(crate) accepted_rules: usize,
    pub(crate) attribution_sensitive_rules: usize,
    pub(crate) runtime_omitted_rules: usize,
    pub(crate) runtime_approximated_rules: usize,
    pub(crate) runtime_resource_approximated_rules: usize,
    pub(crate) runtime_source_kind_approximated_rules: usize,
    pub(crate) dropped: Box<[InputDropCount]>,
}

impl SourceReport {
    /// Returns the source identifier.
    pub fn id(&self) -> &SourceId {
        &self.id
    }

    /// Returns the number of physical lines.
    pub const fn total_lines(&self) -> usize {
        self.total_lines
    }

    /// Returns the number of blank, comment, and metadata lines.
    pub const fn ignored_lines(&self) -> usize {
        self.ignored_lines
    }

    /// Returns the number of rules admitted to the runtime matcher.
    pub const fn accepted_rules(&self) -> usize {
        self.accepted_rules
    }

    /// Returns rules whose first/third-party or domain behavior depends on
    /// an initiating document. Native backends may only approximate that
    /// source for subframes and workers.
    pub const fn attribution_sensitive_rules(&self) -> usize {
        self.attribution_sensitive_rules
    }

    /// Returns rules whose source predicates or declared request types cannot
    /// be evaluated exactly by the audited WebView2 interception API. Source
    /// predicates are judged against the view's main document, not the
    /// initiating frame, and only once the view has committed one.
    pub const fn runtime_omitted_rules(&self) -> usize {
        self.runtime_omitted_rules
    }

    /// Returns rules which are only partially enforceable by WebView2 across
    /// all native resource-type and request-source-kind dimensions.
    pub const fn runtime_approximated_rules(&self) -> usize {
        self.runtime_approximated_rules
    }

    /// Returns rules with partial WebView2 resource-context coverage.
    pub const fn runtime_resource_approximated_rules(&self) -> usize {
        self.runtime_resource_approximated_rules
    }

    /// Returns rules with partial WebView2 request-source-kind coverage.
    ///
    /// V1 intentionally excludes service- and shared-worker initiated
    /// requests from its per-view registration cohort.
    pub const fn runtime_source_kind_approximated_rules(&self) -> usize {
        self.runtime_source_kind_approximated_rules
    }

    /// Returns the number of candidate rule lines before admission.
    pub fn candidate_rules(&self) -> usize {
        self.accepted_rules + self.dropped.iter().map(|entry| entry.count).sum::<usize>()
    }

    /// Returns the number of candidate rules rejected by admission.
    pub fn rejected_rules(&self) -> usize {
        self.dropped.iter().map(|entry| entry.count).sum()
    }

    /// Returns sorted counts for input rules rejected by policy or parsing.
    pub fn dropped(&self) -> &[InputDropCount] {
        &self.dropped
    }

    /// Returns the count for one input exclusion reason.
    pub fn dropped_for(&self, reason: InputDropReason) -> usize {
        self.dropped
            .iter()
            .find(|entry| entry.reason == reason)
            .map_or(0, |entry| entry.count)
    }
}

/// Coverage of the runtime rule set by the generated WebKit artifact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebKitCoverage {
    pub(crate) accepted_input_rules: usize,
    pub(crate) converted_input_rules: usize,
    pub(crate) pruned_input_rules: usize,
    pub(crate) approximated_input_rules: usize,
    pub(crate) attribution_approximated_input_rules: usize,
    pub(crate) resource_approximated_input_rules: usize,
    pub(crate) blocking_rule_entries: usize,
    pub(crate) emitted_rules: usize,
    pub(crate) json_bytes: usize,
    pub(crate) dropped: Box<[WebKitDropCount]>,
}

impl WebKitCoverage {
    /// Returns the number of runtime-admitted input rules.
    pub const fn accepted_input_rules(&self) -> usize {
        self.accepted_input_rules
    }

    /// Returns the number of input rules represented in WebKit syntax.
    pub const fn converted_input_rules(&self) -> usize {
        self.converted_input_rules
    }

    /// Returns represented rules with at least one approximate WebKit
    /// dimension. Each input rule is counted once across all dimensions.
    pub const fn approximated_input_rules(&self) -> usize {
        self.approximated_input_rules
    }

    /// Returns represented rules whose initiator semantics are necessarily
    /// approximated by WebKit's main-document domain predicates.
    pub const fn attribution_approximated_input_rules(&self) -> usize {
        self.attribution_approximated_input_rules
    }

    /// Returns represented rules whose adblock resource category has no exact
    /// WebKit equivalent.
    pub const fn resource_approximated_input_rules(&self) -> usize {
        self.resource_approximated_input_rules
    }

    /// Returns blocking rules emitted before ordered exception rules.
    pub const fn blocking_rule_entries(&self) -> usize {
        self.blocking_rule_entries
    }

    /// Returns wildcard block rules left out because the recorded page
    /// corpus never matched them. They are supported, not omitted.
    pub const fn pruned_input_rules(&self) -> usize {
        self.pruned_input_rules
    }

    /// Returns runtime-admitted rules WebKit syntax could not represent.
    pub const fn omitted_input_rules(&self) -> usize {
        self.accepted_input_rules - self.converted_input_rules - self.pruned_input_rules
    }

    /// Returns the number of emitted WebKit rules, including synthetic rules.
    pub const fn emitted_rules(&self) -> usize {
        self.emitted_rules
    }

    /// Returns the canonical JSON byte length.
    pub const fn json_bytes(&self) -> usize {
        self.json_bytes
    }

    /// Returns sorted counts for runtime rules lost in WebKit conversion.
    pub fn dropped(&self) -> &[WebKitDropCount] {
        &self.dropped
    }

    /// Returns the count for one WebKit conversion-loss reason.
    pub fn dropped_for(&self, reason: WebKitDropReason) -> usize {
        self.dropped
            .iter()
            .find(|entry| entry.reason == reason)
            .map_or(0, |entry| entry.count)
    }
}

/// Complete, immutable compilation and capability-coverage report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompilationReport {
    pub(crate) sources: Box<[SourceReport]>,
    pub(crate) total_source_bytes: usize,
    pub(crate) accepted_rules: usize,
    pub(crate) native_blocking_rule_entries: usize,
    pub(crate) runtime: Option<RuntimeCoverage>,
    pub(crate) webkit: Option<WebKitCoverage>,
}

impl CompilationReport {
    /// Returns source reports in canonical source-ID order.
    pub fn sources(&self) -> &[SourceReport] {
        &self.sources
    }

    /// Returns total UTF-8 source bytes accepted for inspection.
    pub const fn total_source_bytes(&self) -> usize {
        self.total_source_bytes
    }

    /// Returns total rules admitted to the runtime matcher.
    pub const fn accepted_rules(&self) -> usize {
        self.accepted_rules
    }

    /// Returns the exact number of post-`$badfilter` blocking entries
    /// represented by this platform artifact. Ordered or broader exceptions
    /// may still make an entry semantically unreachable.
    pub const fn native_blocking_rule_entries(&self) -> usize {
        self.native_blocking_rule_entries
    }

    /// Returns exact preparation resources for a runtime artifact.
    pub const fn runtime(&self) -> Option<RuntimeCoverage> {
        self.runtime
    }

    /// Returns accepted rules whose semantics depend on the initiating
    /// document or first/third-party classification.
    pub fn attribution_sensitive_rules(&self) -> usize {
        self.sources
            .iter()
            .map(SourceReport::attribution_sensitive_rules)
            .sum()
    }

    /// Returns runtime rules which WebView2 cannot evaluate exactly: their
    /// source predicates see only the view's main document, or their request
    /// types reach no audited context.
    pub fn runtime_omitted_rules(&self) -> usize {
        self.sources
            .iter()
            .map(SourceReport::runtime_omitted_rules)
            .sum()
    }

    /// Returns runtime rules with partial WebView2 coverage. Each source rule
    /// is counted once even if it has multiple approximate dimensions.
    pub fn runtime_approximated_rules(&self) -> usize {
        self.sources
            .iter()
            .map(SourceReport::runtime_approximated_rules)
            .sum()
    }

    /// Returns runtime rules with partial WebView2 resource-context coverage.
    pub fn runtime_resource_approximated_rules(&self) -> usize {
        self.sources
            .iter()
            .map(SourceReport::runtime_resource_approximated_rules)
            .sum()
    }

    /// Returns runtime rules with partial WebView2 request-source-kind
    /// coverage.
    pub fn runtime_source_kind_approximated_rules(&self) -> usize {
        self.sources
            .iter()
            .map(SourceReport::runtime_source_kind_approximated_rules)
            .sum()
    }

    /// Returns total candidate rule lines before admission.
    pub fn candidate_rules(&self) -> usize {
        self.sources.iter().map(SourceReport::candidate_rules).sum()
    }

    /// Returns total candidate rules rejected by admission.
    pub fn rejected_rules(&self) -> usize {
        self.sources.iter().map(SourceReport::rejected_rules).sum()
    }

    /// Returns WebKit conversion coverage.
    pub const fn webkit(&self) -> Option<&WebKitCoverage> {
        self.webkit.as_ref()
    }

    /// Returns the aggregate count for one input exclusion reason.
    pub fn dropped_for(&self, reason: InputDropReason) -> usize {
        self.sources
            .iter()
            .map(|source| source.dropped_for(reason))
            .sum()
    }
}

pub(crate) fn input_counts(counts: BTreeMap<InputDropReason, usize>) -> Box<[InputDropCount]> {
    counts
        .into_iter()
        .map(|(reason, count)| InputDropCount { reason, count })
        .collect()
}

#[cfg(feature = "webkit")]
pub(crate) fn webkit_counts(counts: BTreeMap<WebKitDropReason, usize>) -> Box<[WebKitDropCount]> {
    counts
        .into_iter()
        .map(|(reason, count)| WebKitDropCount { reason, count })
        .collect()
}
