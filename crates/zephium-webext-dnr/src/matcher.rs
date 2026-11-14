//! Request matching and Chrome's per-extension and cross-extension arbitration.
//! See <https://developer.chrome.com/docs/extensions/reference/api/declarativeNetRequest#rule-evaluation>.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;

use regex::bytes::{Regex, RegexBuilder};
use url::Url;

use crate::model::{
    Action, DomainType, HeaderCondition, HeaderModification, HeaderOperation, QueryTransform,
    RequestMethod, ResourceType, Rule, UrlTransform, MAX_HEADER_PATTERN_BYTES,
    MAX_HEADER_PATTERN_MEMORY_BYTES,
};
use crate::parse::compile_regex;

/// Chrome compares the initiator's registrable domain, ignoring URL scheme.
pub fn domain_type(host: &str, initiator_host: Option<&str>) -> DomainType {
    let Some(initiator) = initiator_host else {
        return DomainType::ThirdParty;
    };
    let host = host.trim_end_matches('.');
    let initiator = initiator.trim_end_matches('.');
    let request_ip = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>();
    let initiator_ip = initiator
        .trim_matches(['[', ']'])
        .parse::<std::net::IpAddr>();
    if request_ip.is_ok() || initiator_ip.is_ok() {
        return if request_ip.is_ok() && request_ip == initiator_ip {
            DomainType::FirstParty
        } else {
            DomainType::ThirdParty
        };
    }
    if psl::domain_str(host)
        .unwrap_or(host)
        .eq_ignore_ascii_case(psl::domain_str(initiator).unwrap_or(initiator))
    {
        DomainType::FirstParty
    } else {
        DomainType::ThirdParty
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    BeforeRequest,
    /// Blocking here discards a received response; the request was already sent.
    ResponseHeaders,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

/// Highest inherited allowAllRequests priority for this extension and frame.
/// The host owns frame/document identity, ancestry, and navigation invalidation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameAllow<'a> {
    pub extension_id: &'a str,
    pub priority: u32,
}

/// Trusted network metadata. URLs must be canonical ASCII (punycode host and
/// percent-encoded non-ASCII bytes); hosts exclude credentials, port and dots
/// terminating a DNS name. The caller supplies the actual eTLD+1 party relation,
/// opaque initiator handling, and permission-admitted extension candidates.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub url: &'a str,
    pub host: &'a str,
    pub initiator_host: Option<&'a str>,
    pub top_host: Option<&'a str>,
    pub resource_type: ResourceType,
    pub method: RequestMethod,
    pub domain_type: DomainType,
    pub tab_id: i32,
    pub stage: Stage,
    pub response_headers: &'a [Header<'a>],
    pub frame_allow: &'a [FrameAllow<'a>],
}

impl<'a> Request<'a> {
    pub fn new(url: &'a str, host: &'a str, resource_type: ResourceType) -> Self {
        Self {
            url,
            host,
            initiator_host: None,
            top_host: None,
            resource_type,
            method: RequestMethod::Get,
            domain_type: DomainType::ThirdParty,
            tab_id: -1,
            stage: Stage::BeforeRequest,
            response_headers: &[],
            frame_allow: &[],
        }
    }
}

#[derive(Clone, Debug)]
pub struct PreparedRule {
    pub(crate) rule: Rule,
    url_regex: Option<Regex>,
    url_literal: Option<std::ops::Range<usize>>,
    request_domains: PreparedDomains,
    initiator_domains: PreparedDomains,
    top_domains: PreparedDomains,
    response: Vec<PreparedHeaderCondition>,
    excluded_response: Vec<PreparedHeaderCondition>,
}

impl PreparedRule {
    pub fn new(rule: Rule) -> Result<Self, String> {
        let condition = &rule.condition;
        let url_regex = if let Some(pattern) = &condition.regex_filter {
            Some(compile_regex(
                pattern,
                condition.is_url_filter_case_sensitive,
            )?)
        } else {
            None
        };
        let url_literal = condition.url_filter.as_deref().and_then(|filter| {
            let mut offset = 0;
            filter
                .split(['*', '^', '|'])
                .filter_map(|part| {
                    let start = offset;
                    offset += part.len() + 1;
                    (part.len() >= 3).then_some(start..start + part.len())
                })
                .max_by_key(|range| range.len())
        });
        let response = prepare_headers(condition.response_headers.as_deref())?;
        let excluded_response = prepare_headers(condition.excluded_response_headers.as_deref())?;
        let request_domains = PreparedDomains::new(
            condition.request_domains.as_deref(),
            condition.excluded_request_domains.as_deref(),
        );
        let initiator_domains = PreparedDomains::new(
            condition
                .initiator_domains
                .as_deref()
                .or(condition.domains.as_deref()),
            condition
                .excluded_initiator_domains
                .as_deref()
                .or(condition.excluded_domains.as_deref()),
        );
        let top_domains = PreparedDomains::new(
            condition.top_domains.as_deref(),
            condition.excluded_top_domains.as_deref(),
        );
        Ok(Self {
            rule,
            url_regex,
            url_literal,
            request_domains,
            initiator_domains,
            top_domains,
            response,
            excluded_response,
        })
    }

    pub fn rule(&self) -> &Rule {
        &self.rule
    }

    /// Only regexFilter rules are returned; urlFilter has its own index prefilter.
    pub fn compiled_regex(&self) -> Option<&Regex> {
        self.rule
            .condition
            .regex_filter
            .as_ref()
            .and(self.url_regex.as_ref())
    }

    pub fn matches(&self, request: &Request<'_>) -> bool {
        let condition = &self.rule.condition;
        if let Some(types) = &condition.resource_types {
            if !types.contains(&request.resource_type) {
                return false;
            }
        } else if let Some(excluded) = &condition.excluded_resource_types {
            if excluded.contains(&request.resource_type)
                || (excluded.is_empty() && request.resource_type == ResourceType::MainFrame)
            {
                return false;
            }
        } else if request.resource_type == ResourceType::MainFrame {
            // Chrome RuleCondition.excludedResourceTypes: default omits main_frame.
            return false;
        }
        if let Some(methods) = &condition.request_methods {
            if !(request.url.starts_with("http://") || request.url.starts_with("https://"))
                || !methods.contains(&request.method)
            {
                return false;
            }
        }
        if condition
            .excluded_request_methods
            .as_ref()
            .is_some_and(|methods| {
                (request.url.starts_with("http://") || request.url.starts_with("https://"))
                    && methods.contains(&request.method)
            })
            || condition
                .domain_type
                .is_some_and(|party| party != request.domain_type)
            || !included(&condition.tab_ids, request.tab_id)
            || excluded(&condition.excluded_tab_ids, request.tab_id)
        {
            return false;
        }
        if let (Some(filter), Some(range)) = (&condition.url_filter, &self.url_literal) {
            if !contains_literal(
                request.url.as_bytes(),
                &filter.as_bytes()[range.clone()],
                condition.is_url_filter_case_sensitive,
            ) {
                return false;
            }
        }
        if !self.request_domains.matches(
            Some(request.host),
            condition.request_domains.as_deref(),
            condition.excluded_request_domains.as_deref(),
        ) || !self.initiator_domains.matches(
            request.initiator_host,
            condition
                .initiator_domains
                .as_deref()
                .or(condition.domains.as_deref()),
            condition
                .excluded_initiator_domains
                .as_deref()
                .or(condition.excluded_domains.as_deref()),
        ) || !self.top_domains.matches(
            if request.resource_type == ResourceType::MainFrame {
                Some(request.host)
            } else {
                request.top_host.or(request.initiator_host)
            },
            condition.top_domains.as_deref(),
            condition.excluded_top_domains.as_deref(),
        ) {
            return false;
        }
        if condition.has_response_header_condition()
            && (request.stage != Stage::ResponseHeaders
                || self
                    .excluded_response
                    .iter()
                    .any(|header| header.matches(request.response_headers))
                || (condition.response_headers.is_some()
                    && !self
                        .response
                        .iter()
                        .any(|header| header.matches(request.response_headers))))
        {
            return false;
        }
        condition.url_filter.as_deref().is_none_or(|filter| {
            url_filter_matches(filter, request.url, condition.is_url_filter_case_sensitive)
        }) && self
            .url_regex
            .as_ref()
            .is_none_or(|regex| regex.is_match(request.url.as_bytes()))
    }
}

pub(crate) fn contains_literal(haystack: &[u8], needle: &[u8], case_sensitive: bool) -> bool {
    if case_sensitive {
        return memchr::memmem::find(haystack, needle).is_some();
    }
    let first = needle[0];
    memchr::memchr2_iter(
        first.to_ascii_lowercase(),
        first.to_ascii_uppercase(),
        haystack,
    )
    .any(|start| {
        haystack
            .get(start..start + needle.len())
            .is_some_and(|part| part.eq_ignore_ascii_case(needle))
    })
}

fn included<T: PartialEq>(items: &Option<Vec<T>>, value: T) -> bool {
    items.as_ref().is_none_or(|items| items.contains(&value))
}

fn excluded<T: PartialEq>(items: &Option<Vec<T>>, value: T) -> bool {
    items.as_ref().is_some_and(|items| items.contains(&value))
}

pub(crate) fn domain_matches(host: &str, domain: &str) -> bool {
    host.eq_ignore_ascii_case(domain)
        || host
            .len()
            .checked_sub(domain.len() + 1)
            .is_some_and(|prefix| {
                host.as_bytes().get(prefix) == Some(&b'.')
                    && host
                        .get(prefix + 1..)
                        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(domain))
            })
}

#[derive(Clone, Debug)]
struct PreparedDomains {
    included: Option<HashSet<String>>,
    excluded: Option<HashSet<String>>,
}

impl PreparedDomains {
    fn new(included: Option<&[String]>, excluded: Option<&[String]>) -> Self {
        let large = |domains: &[String]| {
            (domains.len() > 8).then(|| {
                domains
                    .iter()
                    .map(|domain| domain.to_ascii_lowercase())
                    .collect()
            })
        };
        Self {
            included: included.and_then(large),
            excluded: excluded.and_then(large),
        }
    }

    fn matches(
        &self,
        host: Option<&str>,
        included: Option<&[String]>,
        excluded: Option<&[String]>,
    ) -> bool {
        let contains = |domains: &[String], prepared: &Option<HashSet<String>>| {
            host.is_some_and(|host| {
                if let Some(prepared) = prepared {
                    let normalized = if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
                        Cow::Owned(host.to_ascii_lowercase())
                    } else {
                        Cow::Borrowed(host)
                    };
                    std::iter::successors(Some(normalized.as_ref()), |domain| {
                        domain.find('.').map(|dot| &domain[dot + 1..])
                    })
                    .any(|suffix| prepared.contains(suffix))
                } else {
                    domains.iter().any(|domain| domain_matches(host, domain))
                }
            })
        };
        !excluded.is_some_and(|domains| contains(domains, &self.excluded))
            && included.is_none_or(|domains| contains(domains, &self.included))
    }
}

/// Translate only Chrome's URL-filter grammar; regex metacharacters are literals.
#[cfg(test)]
pub(crate) fn url_filter_regex(filter: &str) -> String {
    let mut regex = String::new();
    let mut pattern = filter;
    if let Some(rest) = pattern.strip_prefix("||") {
        regex.push_str(r"^(?:[^/?#@:.]+\.)*");
        pattern = rest;
    } else if let Some(rest) = pattern.strip_prefix('|') {
        regex.push('^');
        pattern = rest;
    }
    let right_anchor = pattern.ends_with('|');
    if right_anchor {
        pattern = &pattern[..pattern.len() - 1];
    }
    for character in pattern.chars() {
        match character {
            '*' => regex.push_str(".*"),
            '^' => regex.push_str(r"(?:[^a-zA-Z0-9_\-.%]|$)"),
            character => regex.push_str(&regex::escape(&character.to_string())),
        }
    }
    if right_anchor {
        regex.push('$');
    }
    regex
}

fn host_suffix(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let start = rest[..authority_end].rfind('@').map_or(0, |at| at + 1);
    rest.get(start..)
}

pub(crate) fn url_filter_matches(filter: &str, url: &str, case_sensitive: bool) -> bool {
    let (pattern, domain_anchor, left_anchor) = if let Some(rest) = filter.strip_prefix("||") {
        (rest, true, true)
    } else if let Some(rest) = filter.strip_prefix('|') {
        (rest, false, true)
    } else {
        (filter, false, false)
    };
    let right_anchor = pattern.ends_with('|');
    let pattern = pattern.strip_suffix('|').unwrap_or(pattern).as_bytes();
    if domain_anchor {
        let Some(suffix) = host_suffix(url) else {
            return false;
        };
        let host_end = suffix.find([':', '/', '?', '#']).unwrap_or(suffix.len());
        return std::iter::once(0)
            .chain(
                suffix.as_bytes()[..host_end]
                    .iter()
                    .enumerate()
                    .filter_map(|(index, byte)| (*byte == b'.').then_some(index + 1)),
            )
            .any(|start| {
                wildcard_matches(
                    pattern,
                    &suffix.as_bytes()[start..],
                    right_anchor,
                    case_sensitive,
                    false,
                )
            });
    }
    if left_anchor {
        wildcard_matches(pattern, url.as_bytes(), right_anchor, case_sensitive, false)
    } else {
        wildcard_matches(pattern, url.as_bytes(), right_anchor, case_sensitive, true)
    }
}

fn wildcard_matches(
    pattern: &[u8],
    text: &[u8],
    right_anchor: bool,
    case_sensitive: bool,
    search: bool,
) -> bool {
    let (mut token, mut position) = (0, 0);
    let mut star = search.then_some((0, 0));
    loop {
        if token == pattern.len() {
            if !right_anchor || position == text.len() {
                return true;
            }
        } else if pattern[token] == b'*' {
            token += 1;
            star = Some((token, position));
            continue;
        } else if pattern[token] == b'^' && position == text.len() {
            token += 1;
            continue;
        } else if let Some(&byte) = text.get(position) {
            let matches = if pattern[token] == b'^' {
                !(byte.is_ascii_alphanumeric() || b"_-.%".contains(&byte))
            } else if case_sensitive {
                byte == pattern[token]
            } else {
                byte.eq_ignore_ascii_case(&pattern[token])
            };
            if matches {
                token += 1;
                position += 1;
                continue;
            }
        }
        match star {
            Some((after_star, expanded)) if expanded < text.len() => {
                position = expanded + 1;
                star = Some((after_star, position));
                token = after_star;
            }
            _ => return false,
        }
    }
}

#[derive(Clone, Debug)]
struct PreparedHeaderCondition {
    name: String,
    values: Option<Vec<Regex>>,
    excluded: Vec<Regex>,
}

impl PreparedHeaderCondition {
    fn new(condition: &HeaderCondition) -> Result<Self, String> {
        let patterns = |values: &[String]| {
            values
                .iter()
                .map(|value| compile_header_pattern(value))
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(Self {
            name: condition.header.clone(),
            values: condition.values.as_deref().map(patterns).transpose()?,
            excluded: patterns(condition.excluded_values.as_deref().unwrap_or_default())?,
        })
    }

    fn matches(&self, headers: &[Header<'_>]) -> bool {
        let mut present = false;
        let mut value_matches = self.values.is_none();
        for header in headers {
            if !header.name.eq_ignore_ascii_case(&self.name) {
                continue;
            }
            present = true;
            if self
                .excluded
                .iter()
                .any(|pattern| pattern.is_match(header.value.as_bytes()))
            {
                return false;
            }
            value_matches |= self.values.as_ref().is_some_and(|values| {
                values
                    .iter()
                    .any(|pattern| pattern.is_match(header.value.as_bytes()))
            });
        }
        present && value_matches
    }
}

fn prepare_headers(
    headers: Option<&[HeaderCondition]>,
) -> Result<Vec<PreparedHeaderCondition>, String> {
    headers
        .unwrap_or_default()
        .iter()
        .map(PreparedHeaderCondition::new)
        .collect()
}

fn header_pattern_regex(pattern: &str) -> String {
    let mut regex = String::from(r"\A(?:");
    let mut chars = pattern.chars();
    while let Some(character) = chars.next() {
        match character {
            '*' => regex.push_str(".*"),
            '?' => regex.push_str(".?"),
            '\\' => {
                let escaped = chars.next().unwrap_or('\\');
                regex.push_str(&regex::escape(&escaped.to_string()));
            }
            character => regex.push_str(&regex::escape(&character.to_string())),
        }
    }
    regex.push_str(r")\z");
    regex
}

pub(crate) fn compile_header_pattern(pattern: &str) -> Result<Regex, String> {
    if pattern.len() > MAX_HEADER_PATTERN_BYTES {
        return Err("response header pattern exceeds the byte limit".into());
    }
    RegexBuilder::new(&header_pattern_regex(pattern))
        .unicode(false)
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .size_limit(MAX_HEADER_PATTERN_MEMORY_BYTES)
        .dfa_size_limit(MAX_HEADER_PATTERN_MEMORY_BYTES)
        .build()
        .map_err(|error| format!("response header pattern: {error}"))
}

#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    pub extension_id: &'a str,
    /// Larger values mean more recently installed. All rules of one extension
    /// must use the same value; distinct extensions should have distinct values.
    pub install_order: u64,
    pub ruleset_id: &'a str,
    pub rule: &'a PreparedRule,
}

#[derive(Clone, Copy)]
pub(crate) struct MatchingCandidate<'a, 'rule> {
    pub extension_id: &'a str,
    pub install_order: u64,
    pub ruleset_id: &'a str,
    pub rule: &'rule Rule,
    pub regex: Option<&'rule Regex>,
}

impl<'a> MatchingCandidate<'a, '_> {
    fn feedback(self) -> MatchedRule<'a> {
        MatchedRule {
            extension_id: self.extension_id,
            ruleset_id: self.ruleset_id,
            rule_id: self.rule.id,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchedRule<'a> {
    pub extension_id: &'a str,
    pub ruleset_id: &'a str,
    pub rule_id: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionAction {
    Allow,
    Block,
    /// Proposed target only. The runtime must check redirect host permissions
    /// and web-accessible-resource policy before following it.
    Redirect {
        url: String,
    },
    Upgrade {
        url: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderEdit<'a> {
    pub matched_rule: MatchedRule<'a>,
    pub modification: HeaderModification,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision<'a> {
    pub action: DecisionAction,
    pub stage: Stage,
    /// Selected terminal rule and rules with accepted header operations.
    pub matched_rules: Vec<MatchedRule<'a>>,
    pub request_headers: Vec<HeaderEdit<'a>>,
    /// Kept separate because WebKitGTK cannot apply response-header edits.
    /// Other native adapters can apply these same ordered operations.
    pub unsupported_response_headers: Vec<HeaderEdit<'a>>,
    /// Pending state: commit only to the successfully loaded frame/document.
    pub frame_allow_updates: Vec<FrameAllow<'a>>,
}

/// All rulesets of an extension share one priority space. Candidate generation
/// may over-select rules but must never omit a possible match. No-match calls
/// allocate nothing in arbitration; regex caches can initialize on first use.
/// Permission decisions belong to the native caller.
pub fn evaluate_candidates<'a>(
    request: &Request<'_>,
    candidates: impl Iterator<Item = Candidate<'a>>,
) -> Decision<'a> {
    evaluate_matching_candidates(
        request,
        candidates.filter_map(|candidate| {
            candidate
                .rule
                .matches(request)
                .then_some(MatchingCandidate {
                    extension_id: candidate.extension_id,
                    install_order: candidate.install_order,
                    ruleset_id: candidate.ruleset_id,
                    rule: &candidate.rule.rule,
                    regex: candidate.rule.compiled_regex(),
                })
        }),
    )
}

pub(crate) fn evaluate_matching_candidates<'a, 'rule>(
    request: &Request<'_>,
    candidates: impl Iterator<Item = MatchingCandidate<'a, 'rule>>,
) -> Decision<'a> {
    let mut decision = Decision {
        action: DecisionAction::Allow,
        stage: request.stage,
        matched_rules: Vec::new(),
        request_headers: Vec::new(),
        unsupported_response_headers: Vec::new(),
        frame_allow_updates: Vec::new(),
    };
    let mut matching: Vec<_> = candidates.collect();
    matching.sort_unstable_by(candidate_order);
    matching.dedup_by(|left, right| {
        left.extension_id == right.extension_id
            && left.ruleset_id == right.ruleset_id
            && left.rule.id == right.rule.id
    });
    let mut winning: Option<(MatchingCandidate<'a, 'rule>, DecisionAction)> = None;
    let mut header_rules = Vec::new();
    let mut offset = 0;
    while offset < matching.len() {
        let first = matching[offset];
        let count = matching[offset..]
            .iter()
            .take_while(|candidate| candidate.extension_id == first.extension_id)
            .count();
        let group = &matching[offset..offset + count];
        offset += count;
        let inherited = request
            .frame_allow
            .iter()
            .filter(|state| state.extension_id == first.extension_id)
            .map(|state| state.priority)
            .max()
            .unwrap_or_default();
        let allow_priority = group
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.rule.action,
                    Action::Allow | Action::AllowAllRequests
                )
            })
            .map(|candidate| candidate.rule.priority)
            .max()
            .unwrap_or_default()
            .max(inherited);
        // Chrome DNR #rule-evaluation: allow exempts this extension only;
        // modifyHeaders must have strictly higher priority than matching allow.
        header_rules.extend(group.iter().copied().filter(|candidate| {
            matches!(candidate.rule.action, Action::ModifyHeaders { .. })
                && candidate.rule.priority > allow_priority
        }));
        if let Some(priority) = group
            .iter()
            .filter(|candidate| {
                matches!(candidate.rule.action, Action::AllowAllRequests)
                    && (request.stage == Stage::BeforeRequest
                        || candidate.rule.condition.has_response_header_condition())
            })
            .map(|candidate| candidate.rule.priority)
            .filter(|priority| *priority > inherited)
            .max()
        {
            decision.frame_allow_updates.push(FrameAllow {
                extension_id: first.extension_id,
                priority,
            });
        }
        for candidate in group {
            let rule = &candidate.rule;
            if matches!(rule.action, Action::ModifyHeaders { .. })
                || (request.stage == Stage::ResponseHeaders
                    && !rule.condition.has_response_header_condition())
                || rule.priority <= inherited
                || (!matches!(rule.action, Action::Allow | Action::AllowAllRequests)
                    && rule.priority <= allow_priority)
            {
                continue;
            }
            let Some(action) = terminal_action(*candidate, request) else {
                // The selected candidate can be a no-op; it does not promote
                // a lower-priority terminal rule from the same extension.
                break;
            };
            if winning.as_ref().is_none_or(|(previous, _)| {
                across_extensions(*candidate, *previous) == Ordering::Greater
            }) {
                winning = Some((*candidate, action));
            }
            break;
        }
    }
    if let Some((candidate, action)) = winning {
        decision.matched_rules.push(candidate.feedback());
        decision.action = action;
    }
    if !matches!(decision.action, DecisionAction::Allow) {
        decision.frame_allow_updates.clear();
        return decision;
    }
    // Chrome DNR #before-request-headers-are-sent: newer extensions precede
    // older extensions even if the older rule has greater numeric priority.
    for candidate in header_rules {
        let Action::ModifyHeaders {
            request_headers,
            response_headers,
        } = &candidate.rule.action
        else {
            continue;
        };
        let (modifications, edits) = match request.stage {
            Stage::BeforeRequest => (request_headers, &mut decision.request_headers),
            Stage::ResponseHeaders => {
                (response_headers, &mut decision.unsupported_response_headers)
            }
        };
        let mut applied = false;
        for modification in modifications {
            if header_operation_allowed(edits, candidate.extension_id, modification) {
                edits.push(HeaderEdit {
                    matched_rule: candidate.feedback(),
                    modification: modification.clone(),
                });
                applied = true;
            }
        }
        if applied && !decision.matched_rules.contains(&candidate.feedback()) {
            decision.matched_rules.push(candidate.feedback());
        }
    }
    decision
}

fn candidate_order(
    left: &MatchingCandidate<'_, '_>,
    right: &MatchingCandidate<'_, '_>,
) -> Ordering {
    right
        .install_order
        .cmp(&left.install_order)
        .then_with(|| left.extension_id.cmp(right.extension_id))
        .then_with(|| right.rule.priority.cmp(&left.rule.priority))
        .then_with(|| action_rank(&right.rule.action).cmp(&action_rank(&left.rule.action)))
        // Chrome explicitly leaves same-action, same-priority order unspecified,
        // including across static/dynamic/session rulesets. Use stable local IDs.
        .then_with(|| left.ruleset_id.cmp(right.ruleset_id))
        .then_with(|| left.rule.id.cmp(&right.rule.id))
}

fn action_rank(action: &Action) -> u8 {
    match action {
        Action::Allow | Action::AllowAllRequests => 4,
        Action::Block => 3,
        Action::UpgradeScheme => 2,
        Action::Redirect { .. } => 1,
        Action::ModifyHeaders { .. } => 0,
    }
}

fn across_extensions(
    left: MatchingCandidate<'_, '_>,
    right: MatchingCandidate<'_, '_>,
) -> Ordering {
    let rank = |action: &Action| match action {
        Action::Block => 3,
        Action::UpgradeScheme | Action::Redirect { .. } => 2,
        _ => 1,
    };
    rank(&left.rule.action)
        .cmp(&rank(&right.rule.action))
        .then_with(|| left.install_order.cmp(&right.install_order))
        .then_with(|| left.extension_id.cmp(right.extension_id))
}

fn header_operation_allowed(
    edits: &[HeaderEdit<'_>],
    extension_id: &str,
    modification: &HeaderModification,
) -> bool {
    let Some(previous) = edits.iter().find(|edit| {
        edit.modification
            .header
            .eq_ignore_ascii_case(&modification.header)
    }) else {
        return true;
    };
    match previous.modification.operation {
        HeaderOperation::Append => modification.operation == HeaderOperation::Append,
        HeaderOperation::Set => {
            modification.operation == HeaderOperation::Append
                && previous.matched_rule.extension_id == extension_id
        }
        HeaderOperation::Remove => false,
    }
}

fn terminal_action(
    candidate: MatchingCandidate<'_, '_>,
    request: &Request<'_>,
) -> Option<DecisionAction> {
    let action = match &candidate.rule.action {
        Action::Allow | Action::AllowAllRequests => DecisionAction::Allow,
        Action::Block => DecisionAction::Block,
        Action::UpgradeScheme => {
            let rest = request
                .url
                .strip_prefix("http:")
                .or_else(|| request.url.strip_prefix("ftp:"))?;
            DecisionAction::Upgrade {
                url: format!("https:{rest}"),
            }
        }
        Action::Redirect { redirect } => {
            if matches!(
                request.resource_type,
                ResourceType::Websocket | ResourceType::Webtransport
            ) {
                return None;
            }
            let target = if let Some(url) = &redirect.url {
                url.clone()
            } else if let Some(path) = &redirect.extension_path {
                format!("chrome-extension://{}{path}", candidate.extension_id)
            } else if let Some(transform) = &redirect.transform {
                transform_url(request.url, transform)?
            } else if let Some(substitution) = &redirect.regex_substitution {
                let compiled;
                let regex = if let Some(regex) = candidate.regex {
                    regex
                } else {
                    let condition = &candidate.rule.condition;
                    compiled = compile_regex(
                        condition.regex_filter.as_deref()?,
                        condition.is_url_filter_case_sensitive,
                    )
                    .ok()?;
                    &compiled
                };
                regex_substitute(regex, request.url, substitution)?
            } else {
                return None;
            };
            let parsed = Url::parse(&target).ok()?;
            if parsed.scheme() == "javascript" || parsed.as_str() == request.url {
                return None;
            }
            if redirect.url.is_none()
                && redirect.extension_path.is_some()
                && (parsed.scheme() != "chrome-extension"
                    || parsed.host_str() != Some(candidate.extension_id)
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.port().is_some())
            {
                return None;
            }
            DecisionAction::Redirect { url: parsed.into() }
        }
        Action::ModifyHeaders { .. } => return None,
    };
    Some(action)
}

fn regex_substitute(regex: &Regex, url: &str, substitution: &str) -> Option<String> {
    let captures = regex.captures(url.as_bytes())?;
    let matched = captures.get(0)?;
    let mut target = String::with_capacity(url.len() + substitution.len());
    target.push_str(url.get(..matched.start())?);
    let mut chars = substitution.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            target.push(character);
            continue;
        }
        let next = chars.next()?;
        if let Some(index) = next.to_digit(10) {
            if let Some(group) = captures.get(index as usize) {
                target.push_str(std::str::from_utf8(group.as_bytes()).ok()?);
            }
        } else if next == '\\' {
            target.push('\\');
        } else {
            return None;
        }
    }
    target.push_str(url.get(matched.end()..)?);
    Some(target)
}

fn transform_url(original: &str, transform: &UrlTransform) -> Option<String> {
    let mut url = Url::parse(original).ok()?;
    if let Some(scheme) = &transform.scheme {
        if url.set_scheme(scheme).is_err() {
            let (_, rest) = url.as_str().split_once(':')?;
            url = Url::parse(&format!("{scheme}:{rest}")).ok()?;
        }
    }
    if let Some(host) = &transform.host {
        url.set_host(Some(host)).ok()?;
    }
    if let Some(port) = &transform.port {
        let port = if port.is_empty() {
            None
        } else {
            Some(port.parse().ok()?)
        };
        url.set_port(port).ok()?;
    }
    if let Some(path) = &transform.path {
        url.set_path(path);
    }
    if let Some(query) = &transform.query {
        url.set_query(if query.is_empty() {
            None
        } else {
            query.strip_prefix('?')
        });
    } else if let Some(query) = &transform.query_transform {
        if let Some(transformed) = transform_query(url.query().unwrap_or_default(), query) {
            url.set_query(if transformed.is_empty() {
                None
            } else {
                Some(&transformed)
            });
        }
    }
    if let Some(fragment) = &transform.fragment {
        url.set_fragment(if fragment.is_empty() {
            None
        } else {
            fragment.strip_prefix('#')
        });
    }
    if let Some(username) = &transform.username {
        url.set_username(username).ok()?;
    }
    if let Some(password) = &transform.password {
        url.set_password(Some(password)).ok()?;
    }
    Some(url.into())
}

fn transform_query(original: &str, transform: &QueryTransform) -> Option<String> {
    let mut remaining: Vec<_> = transform.add_or_replace_params.iter().collect();
    let mut output = String::new();
    let mut changed = false;
    let mut append = |key: &str, value: &str| {
        if !output.is_empty() {
            output.push('&');
        }
        output.push_str(key);
        output.push('=');
        output.push_str(value);
    };
    // Chromium ruleset_matcher_base.cc GetModifiedQuery compares raw keys and
    // consumes each replacement once, preserving later duplicate parameters.
    for part in original.split('&').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        if transform.remove_params.iter().any(|remove| remove == key) {
            changed = true;
            continue;
        }
        if let Some(index) = remaining.iter().position(|parameter| parameter.key == key) {
            changed = true;
            append(key, &remaining.remove(index).value);
        } else {
            append(key, value);
        }
    }
    for parameter in remaining {
        if !parameter.replace_only {
            changed = true;
            append(&parameter.key, &parameter.value);
        }
    }
    changed.then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Condition, QueryKeyValue, Redirect};

    fn rule(id: u32, priority: u32, action: Action) -> PreparedRule {
        PreparedRule::new(Rule {
            id,
            priority,
            action,
            condition: Condition::default(),
        })
        .unwrap()
    }

    fn candidate<'a>(rule: &'a PreparedRule, extension: &'a str, order: u64) -> Candidate<'a> {
        Candidate {
            extension_id: extension,
            install_order: order,
            ruleset_id: "static",
            rule,
        }
    }

    fn request() -> Request<'static> {
        Request::new(
            "https://ads.example.com/path",
            "ads.example.com",
            ResourceType::Script,
        )
    }

    fn modification(header: &str, operation: HeaderOperation, value: &str) -> HeaderModification {
        HeaderModification {
            header: header.into(),
            operation,
            value: (operation != HeaderOperation::Remove).then(|| value.into()),
        }
    }

    fn header_rule(id: u32, priority: u32, operation: HeaderOperation) -> PreparedRule {
        rule(
            id,
            priority,
            Action::ModifyHeaders {
                request_headers: vec![modification("cookie", operation, "value")],
                response_headers: vec![],
            },
        )
    }

    #[test]
    fn url_filters_match_anchors_wildcards_and_literal_regex_characters() {
        let cases = [
            ("ads", "https://example.com/ads.js", true),
            ("|http:", "https://example.com/", false),
            ("|https://example.com/|", "https://example.com/", true),
            ("|https://example.com/|", "https://example.com/path", false),
            ("a*b*c", "https://example.com/a/b/c", true),
            ("a+b?c", "https://example.com/a+b?c", true),
            ("a+b?c", "https://example.com/aaabc", false),
            ("||example.com^", "https://sub.example.com/path", true),
            ("||example.com^", "https://example.com:8443/path", true),
            ("||example.com^", "https://example.company/path", false),
            (
                "||example.com^",
                "https://other.test/example.com/path",
                false,
            ),
            ("||example.com^", "https://example.com@other.test/", false),
            ("||example.com^", "https://user:pass@sub.example.com/", true),
            ("||example.com^", "https://other.test/?q=example.com", false),
            ("||example.com^", "https://badexample.com/", false),
            ("||example.com^", "https://example.com", true),
            ("||example.com^", "data:example.com", false),
            ("path^", "https://example.com/path", true),
            ("path^", "https://example.com/path?q", true),
            ("path^", "https://example.com/path%20", false),
            ("path^", "https://example.com/path_", false),
            ("path^", "https://example.com/path-", false),
            ("path^", "https://example.com/path.", false),
        ];
        for (pattern, url, expected) in cases {
            assert_eq!(
                url_filter_matches(pattern, url, false),
                expected,
                "{pattern}: {url}"
            );
        }
        assert!(url_filter_matches("UPPER", "https://a/upper", false));
        assert!(!url_filter_matches("UPPER", "https://a/upper", true));
    }

    #[test]
    fn direct_url_filter_matches_regex_reference_for_generated_patterns() {
        let mut random = 0x3e63_a91d_u64;
        let mut next = || {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            (random >> 32) as usize
        };
        let alphabet = ["a", "b", "*", "^", ".", "?", "/", "%"];
        let mut urls = vec![
            "https://user:pass@a.b/a?b".to_owned(),
            "https://a.b".to_owned(),
            "data:a.b".to_owned(),
        ];
        for _ in 0..100 {
            let mut path = String::from("https://a.b/");
            for _ in 0..12 {
                path.push_str(alphabet[next() % alphabet.len()]);
            }
            urls.push(path);
        }
        for index in 0..512 {
            let mut pattern = String::from(["", "|", "||"][index % 3]);
            for _ in 0..1 + next() % 7 {
                pattern.push_str(alphabet[next() % alphabet.len()]);
            }
            if index % 2 == 0 {
                pattern.push('|');
            }
            let sensitive = index % 4 == 0;
            let reference = RegexBuilder::new(&url_filter_regex(&pattern))
                .unicode(false)
                .case_insensitive(!sensitive)
                .build()
                .unwrap();
            let prepared = PreparedRule::new(Rule {
                id: 1,
                priority: 1,
                action: Action::Block,
                condition: Condition {
                    url_filter: Some(pattern.clone()),
                    is_url_filter_case_sensitive: sensitive,
                    ..Condition::default()
                },
            })
            .unwrap();
            for url in &urls {
                let text = if pattern.starts_with("||") {
                    host_suffix(url)
                } else {
                    Some(url.as_str())
                };
                let expected = text.is_some_and(|text| reference.is_match(text.as_bytes()));
                assert_eq!(
                    prepared.matches(&Request::new(url, "a.b", ResourceType::Script)),
                    expected,
                    "literal prefilter {pattern}: {url}"
                );
                assert_eq!(
                    url_filter_matches(&pattern, url, sensitive),
                    expected,
                    "{pattern}: {url}"
                );
            }
        }
    }

    #[test]
    fn default_resource_scope_excludes_main_frame_but_explicit_exclusions_do_not() {
        let prepared = rule(1, 1, Action::Block);
        let mut input = request();
        input.resource_type = ResourceType::MainFrame;
        assert!(!prepared.matches(&input));
        let mut model = prepared.rule;
        model.condition.excluded_resource_types = Some(vec![ResourceType::Image]);
        assert!(PreparedRule::new(model).unwrap().matches(&input));
    }

    #[test]
    fn every_resource_and_method_variant_matches_its_inclusion() {
        for resource in [
            ResourceType::MainFrame,
            ResourceType::SubFrame,
            ResourceType::Stylesheet,
            ResourceType::Script,
            ResourceType::Image,
            ResourceType::Font,
            ResourceType::Object,
            ResourceType::Xmlhttprequest,
            ResourceType::Ping,
            ResourceType::CspReport,
            ResourceType::Media,
            ResourceType::Websocket,
            ResourceType::Webtransport,
            ResourceType::Webbundle,
            ResourceType::Other,
        ] {
            let mut model = rule(1, 1, Action::Block).rule;
            model.condition.resource_types = Some(vec![resource]);
            let prepared = PreparedRule::new(model).unwrap();
            let mut input = request();
            input.resource_type = resource;
            assert!(prepared.matches(&input));
            input.resource_type = if resource == ResourceType::Script {
                ResourceType::Image
            } else {
                ResourceType::Script
            };
            assert!(!prepared.matches(&input));
        }
        for method in [
            RequestMethod::Connect,
            RequestMethod::Delete,
            RequestMethod::Get,
            RequestMethod::Head,
            RequestMethod::Options,
            RequestMethod::Patch,
            RequestMethod::Post,
            RequestMethod::Put,
            RequestMethod::Other,
        ] {
            let mut model = rule(1, 1, Action::Block).rule;
            model.condition.request_methods = Some(vec![method]);
            let prepared = PreparedRule::new(model.clone()).unwrap();
            let mut input = request();
            input.method = method;
            assert!(prepared.matches(&input));
            input.url = "ftp://ads.example.com/path";
            assert!(!prepared.matches(&input));
            model.condition.request_methods = None;
            model.condition.excluded_request_methods = Some(vec![method]);
            let excluded = PreparedRule::new(model).unwrap();
            assert!(excluded.matches(&input));
            input.url = "https://ads.example.com/path";
            assert!(!excluded.matches(&input));
        }
    }

    #[test]
    fn domain_conditions_match_subdomains_with_exclusions_and_opaque_origins() {
        let mut model = rule(1, 1, Action::Block).rule;
        model.condition.request_domains = Some(vec!["example.com".into()]);
        model.condition.excluded_request_domains = Some(vec!["excluded.example.com".into()]);
        model.condition.initiator_domains = Some(vec!["origin.test".into()]);
        model.condition.excluded_initiator_domains = Some(vec!["private.origin.test".into()]);
        model.condition.top_domains = Some(vec!["top.test".into()]);
        model.condition.excluded_top_domains = Some(vec!["private.top.test".into()]);
        let prepared = PreparedRule::new(model).unwrap();
        let mut input = request();
        input.initiator_host = Some("sub.origin.test");
        input.top_host = Some("sub.top.test");
        assert!(prepared.matches(&input));
        input.host = "sub.excluded.example.com";
        assert!(!prepared.matches(&input));
        input.host = "notexample.com";
        assert!(!prepared.matches(&input));
        input.host = "ads.example.com";
        input.initiator_host = None;
        assert!(!prepared.matches(&input));
        input.initiator_host = Some("private.origin.test");
        assert!(!prepared.matches(&input));
        input.initiator_host = Some("origin.test");
        input.top_host = Some("private.top.test");
        assert!(!prepared.matches(&input));
        assert!(domain_matches("SUB.Example.com", "example.COM"));
        assert!(!domain_matches("éexample.com", "example.com"));
    }

    #[test]
    fn legacy_domains_and_top_domain_fallback_use_initiator() {
        let mut model = rule(1, 1, Action::Block).rule;
        model.condition.domains = Some(vec!["origin.test".into()]);
        model.condition.excluded_domains = Some(vec!["private.origin.test".into()]);
        model.condition.top_domains = Some(vec!["origin.test".into()]);
        let prepared = PreparedRule::new(model).unwrap();
        let mut input = request();
        input.initiator_host = Some("origin.test");
        assert!(prepared.matches(&input));
        input.initiator_host = Some("private.origin.test");
        assert!(!prepared.matches(&input));
    }

    #[test]
    fn main_frame_top_domain_is_the_new_navigation_host() {
        let mut model = rule(1, 1, Action::Block).rule;
        model.condition.resource_types = Some(vec![ResourceType::MainFrame]);
        model.condition.top_domains = Some(vec!["example.com".into()]);
        let prepared = PreparedRule::new(model).unwrap();
        let mut input = request();
        input.resource_type = ResourceType::MainFrame;
        input.initiator_host = Some("previous.test");
        input.top_host = Some("previous.test");
        assert!(prepared.matches(&input));
        input.host = "unrelated.test";
        assert!(!prepared.matches(&input));
    }

    #[test]
    fn party_tab_and_case_sensitive_regex_conditions() {
        let mut model = rule(1, 1, Action::Block).rule;
        model.condition.domain_type = Some(DomainType::FirstParty);
        model.condition.tab_ids = Some(vec![-1, 7]);
        model.condition.excluded_tab_ids = Some(vec![8]);
        model.condition.regex_filter = Some(r"/PATH$".into());
        let prepared = PreparedRule::new(model.clone()).unwrap();
        let mut input = request();
        assert!(!prepared.matches(&input));
        input.domain_type = DomainType::FirstParty;
        assert!(prepared.matches(&input));
        input.tab_id = 8;
        assert!(!prepared.matches(&input));
        input.tab_id = 7;
        assert!(prepared.matches(&input));
        model.condition.is_url_filter_case_sensitive = true;
        assert!(!PreparedRule::new(model).unwrap().matches(&input));
    }

    #[test]
    fn per_extension_priority_precedes_action_then_documented_action_order() {
        let block = rule(1, 1, Action::Block);
        let allow = rule(2, 1, Action::Allow);
        let high_block = rule(3, 2, Action::Block);
        let upgrade = rule(4, 1, Action::UpgradeScheme);
        let redirect = rule(
            5,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    url: Some("https://target.test/".into()),
                    ..Redirect::default()
                }),
            },
        );
        let mut input = request();
        input.url = "http://ads.example.com/path";
        let decide = |rules: &[&PreparedRule]| {
            evaluate_candidates(&input, rules.iter().map(|rule| candidate(rule, "a", 1))).action
        };
        assert_eq!(decide(&[&block, &allow]), DecisionAction::Allow);
        assert_eq!(decide(&[&allow, &high_block]), DecisionAction::Block);
        assert_eq!(
            decide(&[&block, &upgrade, &redirect]),
            DecisionAction::Block
        );
        assert!(matches!(
            decide(&[&upgrade, &redirect]),
            DecisionAction::Upgrade { .. }
        ));
    }

    #[test]
    fn ruleset_type_does_not_override_priority_and_feedback_carries_identity() {
        let block = rule(1, 1, Action::Block);
        let allow = rule(1, 2, Action::Allow);
        let mut session = candidate(&block, "a", 1);
        session.ruleset_id = "_session";
        let mut dynamic = candidate(&allow, "a", 1);
        dynamic.ruleset_id = "_dynamic";
        let result = evaluate_candidates(&request(), [session, dynamic].into_iter());
        assert_eq!(result.action, DecisionAction::Allow);
        assert_eq!(result.matched_rules[0].ruleset_id, "_dynamic");
    }

    #[test]
    fn cross_extension_block_wins_and_install_order_breaks_ties() {
        let block = rule(1, 1, Action::Block);
        let allow = rule(2, 100, Action::Allow);
        let redirect = rule(
            3,
            200,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    url: Some("https://target.test/".into()),
                    ..Redirect::default()
                }),
            },
        );
        let result = evaluate_candidates(
            &request(),
            [
                candidate(&allow, "new", 5),
                candidate(&redirect, "middle", 3),
                candidate(&block, "old", 1),
            ]
            .into_iter(),
        );
        assert_eq!(result.action, DecisionAction::Block);
        assert_eq!(result.matched_rules[0].extension_id, "old");
        let result = evaluate_candidates(
            &request(),
            [candidate(&block, "old", 1), candidate(&block, "new", 5)].into_iter(),
        );
        assert_eq!(result.matched_rules[0].extension_id, "new");
        let result = evaluate_candidates(
            &request(),
            [candidate(&allow, "new", 5), candidate(&redirect, "old", 1)].into_iter(),
        );
        assert!(matches!(result.action, DecisionAction::Redirect { .. }));
    }

    #[test]
    fn inherited_frame_allow_is_per_extension_and_priority_bounded() {
        let block = rule(1, 4, Action::Block);
        let high_block = rule(2, 6, Action::Block);
        let header = header_rule(3, 5, HeaderOperation::Remove);
        let inherited = [FrameAllow {
            extension_id: "a",
            priority: 5,
        }];
        let mut input = request();
        input.frame_allow = &inherited;
        let result = evaluate_candidates(
            &input,
            [candidate(&block, "a", 1), candidate(&header, "a", 1)].into_iter(),
        );
        assert_eq!(result.action, DecisionAction::Allow);
        assert!(result.request_headers.is_empty());
        assert_eq!(
            evaluate_candidates(&input, [candidate(&high_block, "a", 1)].into_iter()).action,
            DecisionAction::Block
        );
        assert_eq!(
            evaluate_candidates(&input, [candidate(&block, "b", 2)].into_iter()).action,
            DecisionAction::Block
        );
    }

    #[test]
    fn allow_all_updates_survive_a_higher_priority_allow_and_clear_on_block() {
        let mut all = rule(1, 3, Action::AllowAllRequests);
        all.rule.condition.resource_types = Some(vec![ResourceType::MainFrame]);
        let mut allow = rule(2, 5, Action::Allow);
        allow.rule.condition.resource_types = Some(vec![ResourceType::MainFrame]);
        let mut block = rule(3, 6, Action::Block);
        block.rule.condition.resource_types = Some(vec![ResourceType::MainFrame]);
        let mut input = request();
        input.resource_type = ResourceType::MainFrame;
        let result = evaluate_candidates(
            &input,
            [candidate(&all, "a", 1), candidate(&allow, "a", 1)].into_iter(),
        );
        assert_eq!(
            result.frame_allow_updates,
            vec![FrameAllow {
                extension_id: "a",
                priority: 3
            }]
        );
        let result = evaluate_candidates(
            &input,
            [candidate(&all, "a", 1), candidate(&block, "b", 2)].into_iter(),
        );
        assert!(result.frame_allow_updates.is_empty());
    }

    #[test]
    fn header_arbitration_obeys_append_set_remove_and_install_order() {
        let append = header_rule(1, 10, HeaderOperation::Append);
        let set = header_rule(2, 9, HeaderOperation::Set);
        let remove = header_rule(3, 8, HeaderOperation::Remove);
        let lower_append = header_rule(4, 1, HeaderOperation::Append);
        let result = evaluate_candidates(
            &request(),
            [&append, &set, &remove, &lower_append]
                .into_iter()
                .map(|rule| candidate(rule, "a", 1)),
        );
        assert_eq!(
            result
                .request_headers
                .iter()
                .map(|edit| edit.matched_rule.rule_id)
                .collect::<Vec<_>>(),
            [1, 4]
        );
        let result = evaluate_candidates(
            &request(),
            [
                candidate(&set, "new", 2),
                candidate(&append, "old", 1),
                candidate(&lower_append, "new", 2),
            ]
            .into_iter(),
        );
        assert_eq!(
            result
                .request_headers
                .iter()
                .map(|edit| edit.matched_rule.rule_id)
                .collect::<Vec<_>>(),
            [2, 4]
        );
        let result = evaluate_candidates(
            &request(),
            [candidate(&remove, "new", 2), candidate(&append, "old", 1)].into_iter(),
        );
        assert_eq!(result.request_headers.len(), 1);
        assert_eq!(
            result.request_headers[0].modification.operation,
            HeaderOperation::Remove
        );
    }

    #[test]
    fn allow_threshold_suppresses_equal_headers_only_for_its_extension() {
        let allow = rule(1, 5, Action::Allow);
        let same = header_rule(2, 5, HeaderOperation::Append);
        let higher = header_rule(3, 6, HeaderOperation::Append);
        let other = header_rule(4, 1, HeaderOperation::Append);
        let result = evaluate_candidates(
            &request(),
            [
                candidate(&allow, "a", 1),
                candidate(&same, "a", 1),
                candidate(&higher, "a", 1),
                candidate(&other, "b", 2),
            ]
            .into_iter(),
        );
        assert_eq!(
            result
                .request_headers
                .iter()
                .map(|edit| edit.matched_rule.rule_id)
                .collect::<Vec<_>>(),
            [4, 3]
        );
    }

    #[test]
    fn duplicate_candidates_do_not_duplicate_header_operations() {
        let append = header_rule(1, 1, HeaderOperation::Append);
        let input = candidate(&append, "a", 1);
        let result = evaluate_candidates(&request(), [input, input].into_iter());
        assert_eq!(result.request_headers.len(), 1);
    }

    #[test]
    fn response_conditions_are_deferred_and_exclusions_take_precedence() {
        let mut model = rule(1, 1, Action::Block).rule;
        model.condition.response_headers = Some(vec![HeaderCondition {
            header: "content-type".into(),
            values: Some(vec!["TEXT/HTML*".into()]),
            excluded_values: Some(vec!["*private*".into()]),
        }]);
        model.condition.excluded_response_headers = Some(vec![HeaderCondition {
            header: "x-exempt".into(),
            values: None,
            excluded_values: None,
        }]);
        let prepared = PreparedRule::new(model).unwrap();
        let headers = [Header {
            name: "Content-Type",
            value: "text/html; charset=utf-8",
        }];
        let mut input = request();
        input.response_headers = &headers;
        assert!(!prepared.matches(&input));
        input.stage = Stage::ResponseHeaders;
        assert!(prepared.matches(&input));
        assert_eq!(
            evaluate_candidates(&input, [candidate(&prepared, "a", 1)].into_iter()).stage,
            Stage::ResponseHeaders
        );
        let private = [Header {
            name: "content-type",
            value: "text/html; private",
        }];
        input.response_headers = &private;
        assert!(!prepared.matches(&input));
        let exempt = [
            headers[0],
            Header {
                name: "X-Exempt",
                value: "yes",
            },
        ];
        input.response_headers = &exempt;
        assert!(!prepared.matches(&input));
    }

    #[test]
    fn excluded_response_header_matches_absence_and_header_wildcards_escape() {
        let mut model = rule(1, 1, Action::Block).rule;
        model.condition.excluded_response_headers = Some(vec![HeaderCondition {
            header: "x-test".into(),
            values: Some(vec![r"a\*b?".into()]),
            excluded_values: None,
        }]);
        let prepared = PreparedRule::new(model).unwrap();
        let mut input = request();
        input.stage = Stage::ResponseHeaders;
        assert!(prepared.matches(&input));
        let header = [Header {
            name: "X-Test",
            value: "A*b",
        }];
        input.response_headers = &header;
        assert!(!prepared.matches(&input));
        let header = [Header {
            name: "x-test",
            value: "a*bZ",
        }];
        input.response_headers = &header;
        assert!(!prepared.matches(&input));
        let header = [Header {
            name: "x-test",
            value: "a*bZZ",
        }];
        input.response_headers = &header;
        assert!(prepared.matches(&input));
    }

    #[test]
    fn response_edits_are_distinct_and_prior_allow_suppresses_response_block() {
        let edits = rule(
            1,
            10,
            Action::ModifyHeaders {
                request_headers: vec![modification("cookie", HeaderOperation::Remove, "")],
                response_headers: vec![modification("set-cookie", HeaderOperation::Remove, "")],
            },
        );
        let mut blocked = rule(2, 2, Action::Block).rule;
        blocked.condition.excluded_response_headers = Some(vec![HeaderCondition {
            header: "x-allow".into(),
            values: None,
            excluded_values: None,
        }]);
        let blocked = PreparedRule::new(blocked).unwrap();
        let allow = rule(3, 3, Action::Allow);
        let mut input = request();
        let before = evaluate_candidates(&input, [candidate(&edits, "a", 1)].into_iter());
        assert_eq!(before.request_headers.len(), 1);
        assert!(before.unsupported_response_headers.is_empty());
        input.stage = Stage::ResponseHeaders;
        let after = evaluate_candidates(
            &input,
            [
                candidate(&edits, "a", 1),
                candidate(&blocked, "a", 1),
                candidate(&allow, "a", 1),
            ]
            .into_iter(),
        );
        assert_eq!(after.action, DecisionAction::Allow);
        assert!(after.request_headers.is_empty());
        assert_eq!(after.unsupported_response_headers.len(), 1);
    }

    #[test]
    fn redirects_support_absolute_extension_regex_and_transform_targets() {
        let absolute = rule(
            1,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    url: Some("https://target.test/".into()),
                    ..Redirect::default()
                }),
            },
        );
        let extension = rule(
            2,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    extension_path: Some("/asset.js".into()),
                    ..Redirect::default()
                }),
            },
        );
        let mut substitution = rule(
            3,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    regex_substitution: Some(r"https://\1.test/".into()),
                    ..Redirect::default()
                }),
            },
        )
        .rule;
        substitution.condition.regex_filter = Some(r"^https://(ads)\.example\.com/".into());
        let substitution = PreparedRule::new(substitution).unwrap();
        let decide = |rule| {
            evaluate_candidates(&request(), [candidate(rule, "extension", 1)].into_iter()).action
        };
        assert_eq!(
            decide(&absolute),
            DecisionAction::Redirect {
                url: "https://target.test/".into()
            }
        );
        assert_eq!(
            decide(&extension),
            DecisionAction::Redirect {
                url: "chrome-extension://extension/asset.js".into()
            }
        );
        assert_eq!(
            decide(&substitution),
            DecisionAction::Redirect {
                url: "https://ads.test/path".into()
            }
        );
        assert_eq!(
            regex_substitute(
                &compile_regex("(ads)", true).unwrap(),
                "https://ads.test/",
                r"\0-\1"
            ),
            Some("https://ads-ads.test/".into())
        );
    }

    #[test]
    fn transform_covers_all_components_and_raw_query_parameter_semantics() {
        let transform = UrlTransform {
            scheme: Some("https".into()),
            host: Some("new.test".into()),
            port: Some("8443".into()),
            path: Some("/new".into()),
            fragment: Some("#new".into()),
            username: Some("user".into()),
            password: Some("pass".into()),
            query_transform: Some(QueryTransform {
                remove_params: vec!["remove".into()],
                add_or_replace_params: vec![
                    QueryKeyValue {
                        key: "key".into(),
                        value: "new%20value".into(),
                        replace_only: false,
                    },
                    QueryKeyValue {
                        key: "missing".into(),
                        value: "ignored".into(),
                        replace_only: true,
                    },
                    QueryKeyValue {
                        key: "add".into(),
                        value: "value".into(),
                        replace_only: false,
                    },
                ],
            }),
            ..UrlTransform::default()
        };
        let original = "http://old.test:8080/path?remove=1&key=first&key=second&raw=%2f#old";
        assert_eq!(transform_url(original, &transform), Some("https://user:pass@new.test:8443/new?key=new%20value&key=second&raw=%2f&add=value#new".into()));
        let clear = UrlTransform {
            port: Some(String::new()),
            path: Some(String::new()),
            query: Some(String::new()),
            fragment: Some(String::new()),
            ..UrlTransform::default()
        };
        assert_eq!(
            transform_url(original, &clear),
            Some("http://old.test/".into())
        );
        let query = UrlTransform {
            query: Some("?a=b".into()),
            ..UrlTransform::default()
        };
        assert_eq!(
            transform_url("https://a.test/", &query),
            Some("https://a.test/?a=b".into())
        );
        let extension_scheme = UrlTransform {
            scheme: Some("chrome-extension".into()),
            host: Some("extension".into()),
            ..UrlTransform::default()
        };
        assert_eq!(
            transform_url("https://a.test/asset.js", &extension_scheme),
            Some("chrome-extension://extension/asset.js".into())
        );
        let redirect = rule(
            1,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    transform: Some(transform),
                    ..Redirect::default()
                }),
            },
        );
        assert!(matches!(
            evaluate_candidates(&request(), [candidate(&redirect, "a", 1)].into_iter()).action,
            DecisionAction::Redirect { .. }
        ));
    }

    #[test]
    fn invalid_and_noop_redirects_do_not_promote_lower_rules_or_escape_extension_origin() {
        let noop = rule(
            1,
            2,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    url: Some(request().url.into()),
                    ..Redirect::default()
                }),
            },
        );
        let fallback = rule(2, 1, Action::Block);
        assert_eq!(
            evaluate_candidates(
                &request(),
                [candidate(&noop, "a", 1), candidate(&fallback, "a", 1)].into_iter()
            )
            .action,
            DecisionAction::Allow
        );
        for path in ["//evil.test/", "/\\evil.test/", "/@evil.test/"] {
            let redirect = rule(
                1,
                1,
                Action::Redirect {
                    redirect: Box::new(Redirect {
                        extension_path: Some(path.into()),
                        ..Redirect::default()
                    }),
                },
            );
            let result =
                evaluate_candidates(&request(), [candidate(&redirect, "a", 1)].into_iter());
            if let DecisionAction::Redirect { url } = result.action {
                assert_eq!(Url::parse(&url).unwrap().host_str(), Some("a"));
            }
        }
        let javascript = rule(
            1,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    url: Some("javascript:alert(1)".into()),
                    ..Redirect::default()
                }),
            },
        );
        assert_eq!(
            evaluate_candidates(&request(), [candidate(&javascript, "a", 1)].into_iter()).action,
            DecisionAction::Allow
        );
        let redirect = rule(
            3,
            1,
            Action::Redirect {
                redirect: Box::new(Redirect {
                    url: Some("https://target.test/".into()),
                    ..Redirect::default()
                }),
            },
        );
        for resource_type in [ResourceType::Websocket, ResourceType::Webtransport] {
            let mut input = request();
            input.resource_type = resource_type;
            assert_eq!(
                evaluate_candidates(&input, [candidate(&redirect, "a", 1)].into_iter()).action,
                DecisionAction::Allow
            );
        }
    }

    #[test]
    fn upgrades_only_http_and_ftp() {
        let upgrade = rule(1, 1, Action::UpgradeScheme);
        for (url, expected) in [
            ("http://a.test/", Some("https://a.test/")),
            ("ftp://a.test/path", Some("https://a.test/path")),
            ("https://a.test/", None),
            ("ws://a.test/", None),
        ] {
            let input = Request::new(url, "a.test", ResourceType::Script);
            let result = evaluate_candidates(&input, [candidate(&upgrade, "a", 1)].into_iter());
            assert_eq!(
                result.action,
                expected.map_or(DecisionAction::Allow, |url| DecisionAction::Upgrade {
                    url: url.into()
                })
            );
        }
        let upgrade = rule(1, 2, Action::UpgradeScheme);
        let lower_block = rule(2, 1, Action::Block);
        assert_eq!(
            evaluate_candidates(
                &request(),
                [candidate(&upgrade, "a", 1), candidate(&lower_block, "a", 1)].into_iter()
            )
            .action,
            DecisionAction::Allow
        );
    }
}

#[cfg(test)]
mod party_tests {
    use super::*;

    #[test]
    fn party_uses_public_suffix_boundaries_and_treats_opaque_as_third_party() {
        for (request, initiator) in [
            ("ads.example.co.uk", "www.example.co.uk"),
            ("example.com.", "EXAMPLE.COM"),
            ("localhost", "localhost"),
            ("127.0.0.1", "127.0.0.1"),
            ("[::1]", "[::1]"),
        ] {
            assert_eq!(
                domain_type(request, Some(initiator)),
                DomainType::FirstParty
            );
        }
        for (request, initiator) in [
            ("ads.example.co.uk", Some("different.co.uk")),
            ("one.github.io", Some("two.github.io")),
            ("127.0.0.1", Some("127.0.0.2")),
            ("127.0.0.1", Some("10.0.0.1")),
            ("127.0.0.1", Some("subdomain.0.1")),
            ("example.test", None),
        ] {
            assert_eq!(domain_type(request, initiator), DomainType::ThirdParty);
        }
    }
}
