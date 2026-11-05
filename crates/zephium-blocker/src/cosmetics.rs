//! Bounded static CSS policy, plus the scriptlet rules the scriptlet program is
//! built from (see `scriptlets`). No replacement code or DOM scanning.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};

use crate::Sha256;
use adblock::filters::cosmetic::{CosmeticFilter, CosmeticFilterMask};
use adblock::{Engine, FilterSet};
use serde::{Deserialize, Serialize};

const MAX_RULES: usize = 50_000;
const MAX_SELECTOR_BYTES: usize = 4 * 1024;
const MAX_SCOPE_DOMAINS: usize = 1_024;
const MAX_SCOPE_PREDICATES: usize = 100_000;
const MAX_POLICY_BYTES: usize = 16 * 1024 * 1024;
const MAX_STYLESHEET_BYTES: usize = 1024 * 1024;
const MAX_CONTROLS: usize = 4_096;
/// The generic-hide matcher's per-regex compile limits.
const CONTROL_REGEX_SIZE_BYTES: usize = 96 * 1024;
const CONTROL_REGEX_DFA_SIZE_BYTES: usize = 16 * 1024;

/// Static cosmetic admission statistics, separate from network rule coverage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CosmeticReport {
    /// Accepted static hide and exception input lines.
    pub accepted: usize,
    /// Unsupported, invalid or overlong cosmetic input lines.
    pub rejected: usize,
    /// Generic-hide controls admitted into the document matcher.
    pub generic_controls: usize,
    /// `##+js` and `#@#+js` rules admitted for the scriptlet program.
    pub scriptlet_rules: usize,
    /// Admitted scriptlet rules naming a scriptlet the library lacks.
    pub unknown_scriptlet_rules: usize,
    /// Admitted scriptlet rules that run with a stand-in for what they name:
    /// `no-xhr-if` answers a `war:` resource the app does not ship with an
    /// empty body, still keeping the request from leaving.
    pub substituted_scriptlet_rules: usize,
    /// Scriptlet rules left out: generic, overlong, or a hostname form the
    /// program cannot key.
    pub unsupported_scriptlet_rules: usize,
}

/// A cosmetic artifact could not be admitted or queried within its limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CosmeticError {
    /// Aggregate input, rule count or emitted CSS exceeds its fixed budget.
    #[error("cosmetic policy exceeds its resource budget")]
    ResourceLimit,
    /// Encoded policy, selector, domain or control is not valid.
    #[error("cosmetic policy is invalid")]
    InvalidPolicy,
    /// The document is not a bounded HTTP(S) URL with a hostname.
    #[error("cosmetic document URL is invalid")]
    InvalidDocument,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    selector: String,
    include: Vec<String>,
    exclude: Vec<String>,
    exception: bool,
}

impl Rule {
    fn matches(&self, host: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|domain| within(host, domain)))
            && !self.exclude.iter().any(|domain| within(host, domain))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    version: u32,
    rules: Vec<Rule>,
    controls: Vec<String>,
    scriptlets: Vec<crate::scriptlets::ScriptletRule>,
    report: CosmeticReport,
}

const DATA_VERSION: u32 = 2;

/// Immutable selector index shared by profiles and documents. Queries only
/// inspect generic rules and rules indexed under the document's domain labels.
pub struct CosmeticPolicy {
    report: CosmeticReport,
    encoded: Arc<[u8]>,
    fingerprint: [u8; 32],
    selective: Engine,
    generic_index: Arc<str>,
    generic_index_digest: zephium_core::blocker::ContentRuleDigest,
    fallback_generic: Vec<Rule>,
    scriptlets: Option<crate::scriptlets::Program>,
}

// Retain no policy by ourselves. Native profiles own the live public policy;
// this single weak slot lets later profiles recover that same instance even
// after the temporary network JSON envelope has been released.
static LIVE_POLICY: Mutex<Weak<CosmeticPolicy>> = Mutex::new(Weak::new());

fn reuse_live_policy(bytes: &[u8]) -> Option<Arc<CosmeticPolicy>> {
    let policy = LIVE_POLICY
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .upgrade()?;
    // Exact validated bytes, not a caller-supplied digest or profile identity.
    (policy.encoded.as_ref() == bytes).then_some(policy)
}

impl std::fmt::Debug for CosmeticPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CosmeticPolicy")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl CosmeticPolicy {
    /// Compiles static selectors and generic-hide controls from bounded sources.
    /// Unsupported cosmetic syntax is reported and omitted, never executed.
    pub fn compile<'a>(
        sources: impl IntoIterator<Item = &'a str>,
    ) -> Result<Arc<Self>, CosmeticError> {
        Self::compile_with_trust(sources.into_iter().map(|source| (source, false)))
    }

    /// As [`Self::compile`], with each source's scriptlet trust.
    pub fn compile_with_trust<'a>(
        sources: impl IntoIterator<Item = (&'a str, bool)>,
    ) -> Result<Arc<Self>, CosmeticError> {
        let mut data = Data {
            version: DATA_VERSION,
            rules: Vec::new(),
            controls: Vec::new(),
            scriptlets: Vec::new(),
            report: CosmeticReport::default(),
        };
        let mut bytes = 0usize;
        let mut lines = 0usize;
        let mut predicates = 0usize;
        for (source_index, (source, trusted)) in sources.into_iter().enumerate() {
            if source_index >= 32 {
                return Err(CosmeticError::ResourceLimit);
            }
            bytes = bytes
                .checked_add(source.len())
                .ok_or(CosmeticError::ResourceLimit)?;
            if bytes > 32 * 1024 * 1024 {
                return Err(CosmeticError::ResourceLimit);
            }
            for line in source.lines().map(str::trim) {
                lines += 1;
                if lines > 500_000 || line.len() > 64 * 1024 {
                    return Err(CosmeticError::ResourceLimit);
                }
                if line.starts_with('!') || line.starts_with('[') || line.is_empty() {
                    continue;
                }
                match crate::scriptlets::classify(line) {
                    crate::scriptlets::Kind::None => {}
                    crate::scriptlets::Kind::Unsupported => {
                        data.report.unsupported_scriptlet_rules += 1;
                        continue;
                    }
                    crate::scriptlets::Kind::Rule {
                        unknown,
                        substituted,
                    } => {
                        // A scriptlet the library lacks runs nowhere, so it
                        // is counted and never reaches the program.
                        if !unknown {
                            data.scriptlets.push(crate::scriptlets::ScriptletRule {
                                line: line.to_owned(),
                                trusted,
                            });
                        }
                        data.report.scriptlet_rules += 1;
                        data.report.unknown_scriptlet_rules += usize::from(unknown);
                        data.report.substituted_scriptlet_rules += usize::from(substituted);
                        continue;
                    }
                }
                match generic_control(line) {
                    Ok(Some(control)) if !control_compiles(&control) => {
                        data.report.rejected += 1;
                        continue;
                    }
                    Ok(Some(control)) => {
                        data.controls.push(control);
                        if data.controls.len() > MAX_CONTROLS {
                            return Err(CosmeticError::ResourceLimit);
                        }
                        data.report.generic_controls += 1;
                        continue;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        data.report.rejected += 1;
                        continue;
                    }
                }
                if !crate::compiler::looks_like_cosmetic_rule(line) {
                    continue;
                }
                match parse_rule(line) {
                    Ok(rule) => {
                        predicates += rule.include.len() + rule.exclude.len();
                        if predicates > MAX_SCOPE_PREDICATES {
                            return Err(CosmeticError::ResourceLimit);
                        }
                        data.rules.push(rule);
                        data.report.accepted += 1;
                        if data.rules.len() > MAX_RULES {
                            return Err(CosmeticError::ResourceLimit);
                        }
                    }
                    Err(_) => data.report.rejected += 1,
                }
            }
        }
        data.rules.sort();
        data.rules.dedup();
        data.controls.sort();
        data.controls.dedup();
        data.scriptlets.sort();
        data.scriptlets.dedup();
        Self::from_data(data)
    }

    /// Serializes portable policy data; no browsing decisions or document URLs
    /// are retained in this representation.
    pub fn encode(&self) -> Result<Vec<u8>, CosmeticError> {
        Ok(self.encoded.to_vec())
    }

    /// Revalidates persisted data, including selectors and domain boundaries.
    pub fn decode(bytes: &[u8]) -> Result<Arc<Self>, CosmeticError> {
        if bytes.len() > MAX_POLICY_BYTES {
            return Err(CosmeticError::ResourceLimit);
        }
        if let Some(policy) = reuse_live_policy(bytes) {
            return Ok(policy);
        }
        let data: Data = serde_json::from_slice(bytes).map_err(|_| CosmeticError::InvalidPolicy)?;
        if serde_json::to_vec(&data).map_err(|_| CosmeticError::InvalidPolicy)? != bytes {
            return Err(CosmeticError::InvalidPolicy);
        }
        Self::from_data(data)
    }

    /// Returns static filtering admission counts.
    pub fn report(&self) -> CosmeticReport {
        self.report
    }

    /// The program every frame runs at document start in its own world, when
    /// any host has scriptlets.
    pub fn scriptlet_program(&self) -> Option<&str> {
        self.scriptlets
            .as_ref()
            .map(|program| program.script.as_ref())
    }

    /// Hosts with an entry in the scriptlet program, and distinct call sets.
    pub fn scriptlet_coverage(&self) -> (usize, usize) {
        self.scriptlets
            .as_ref()
            .map_or((0, 0), |program| (program.hosts, program.calls))
    }

    fn from_data(data: Data) -> Result<Arc<Self>, CosmeticError> {
        if data.version != DATA_VERSION
            || data.scriptlets.len() > data.report.scriptlet_rules
            || data.report.unknown_scriptlet_rules > data.report.scriptlet_rules
            || data.scriptlets.windows(2).any(|pair| pair[0] >= pair[1])
            || data.rules.len() > MAX_RULES
            || data.controls.len() > MAX_CONTROLS
            || data.rules.len() > data.report.accepted
            || data.report.accepted > MAX_RULES
            || data.report.rejected > 500_000
            || data.controls.len() > data.report.generic_controls
            || data.report.generic_controls > MAX_CONTROLS
            || data.rules.windows(2).any(|pair| pair[0] >= pair[1])
            || data.controls.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(CosmeticError::InvalidPolicy);
        }
        let mut generic = Vec::new();
        let mut predicates = 0usize;
        for (index, rule) in data.rules.iter().enumerate() {
            validate_selector(&rule.selector)?;
            if rule.include.len() + rule.exclude.len() > MAX_SCOPE_DOMAINS {
                return Err(CosmeticError::InvalidPolicy);
            }
            predicates += rule.include.len() + rule.exclude.len();
            if predicates > MAX_SCOPE_PREDICATES {
                return Err(CosmeticError::ResourceLimit);
            }
            for list in [&rule.include, &rule.exclude] {
                if list.len() > MAX_SCOPE_DOMAINS
                    || list.windows(2).any(|p| p[0] >= p[1])
                    || list.iter().any(|d| normalize_domain(d).as_ref() != Some(d))
                {
                    return Err(CosmeticError::InvalidPolicy);
                }
            }
            if rule.exception && (rule.include.is_empty() || !rule.exclude.is_empty()) {
                return Err(CosmeticError::InvalidPolicy);
            }
            if rule.include.is_empty() {
                generic.push(index);
            }
        }
        for control in &data.controls {
            // Reconstruct the only admitted syntax; persisted controls cannot
            // introduce redirect, CSP, removeparam, or executable resources.
            let (pattern, modifiers) = control
                .rsplit_once('$')
                .ok_or(CosmeticError::InvalidPolicy)?;
            let modifiers = modifiers
                .strip_prefix("document")
                .ok_or(CosmeticError::InvalidPolicy)?;
            let original = format!("@@{pattern}$generichide{modifiers}");
            if generic_control(&original)?.as_ref() != Some(control) {
                return Err(CosmeticError::InvalidPolicy);
            }
            if !control_compiles(control) {
                return Err(CosmeticError::InvalidPolicy);
            }
        }
        // Let adblock-rust own generic classification and hostname exceptions.
        let mut selected_filters = FilterSet::new(false);
        let mut selected_lines = Vec::new();
        for rule in &data.rules {
            let scope = rule
                .include
                .iter()
                .cloned()
                .chain(rule.exclude.iter().map(|d| format!("~{d}")))
                .collect::<Vec<_>>()
                .join(",");
            selected_lines.push(format!(
                "{scope}{}{}",
                if rule.exception { "#@#" } else { "##" },
                rule.selector
            ));
        }
        for control in &data.controls {
            let (pattern, modifiers) = control
                .rsplit_once('$')
                .ok_or(CosmeticError::InvalidPolicy)?;
            selected_lines.push(format!(
                "@@{pattern}$generichide{}",
                modifiers
                    .strip_prefix("document")
                    .ok_or(CosmeticError::InvalidPolicy)?
            ));
        }
        selected_filters.add_filter_list(selected_lines.join("\n"), Default::default());
        let selective = Engine::new_with_filter_set(selected_filters);
        selective
            .prepare_and_freeze_network_matcher(adblock::blocker::NetworkMatcherPreparationLimits {
                max_regexes: 256,
                max_pattern_bytes: 256 * 1024,
                max_patterns_per_regex: 256,
                max_pattern_bytes_per_regex: 128 * 1024,
                max_regex_size_bytes: CONTROL_REGEX_SIZE_BYTES,
                max_regex_dfa_size_bytes: CONTROL_REGEX_DFA_SIZE_BYTES,
                max_filter_checks_per_request: 256,
            })
            .map_err(|_| CosmeticError::ResourceLimit)?;
        let mut keys = BTreeSet::new();
        let mut fallback_generic = Vec::new();
        for &i in &generic {
            let selector = &data.rules[i].selector;
            let prefix = selector.as_bytes().first().copied();
            if !matches!(prefix, Some(b'.' | b'#')) {
                continue;
            }
            let key: String = selector
                .chars()
                .skip(1)
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .collect();
            // Escaped/non-ASCII keys stay in the small unconditional group.
            let end = key.len() + 1;
            if key.is_empty()
                || selector[end..].starts_with('\\')
                || selector[end..]
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_ascii())
            {
                fallback_generic.push(selector.clone());
            } else {
                keys.insert(format!("{}{key}", prefix.unwrap() as char));
            }
        }
        let empty = std::collections::HashSet::new();
        let mut index = Vec::new();
        for key in keys {
            let mut selectors = if let Some(name) = key.strip_prefix('.') {
                selective.hidden_class_id_selectors(
                    std::iter::once(name),
                    std::iter::empty::<&str>(),
                    &empty,
                )
            } else {
                selective.hidden_class_id_selectors(
                    std::iter::empty::<&str>(),
                    std::iter::once(key.strip_prefix('#').ok_or(CosmeticError::InvalidPolicy)?),
                    &empty,
                )
            };
            selectors.sort();
            selectors.dedup();
            if !selectors.is_empty() {
                index.push((key, selectors));
            }
        }
        let generic_index: Arc<str> = serde_json::to_string(&index)
            .map_err(|_| CosmeticError::InvalidPolicy)?
            .into();
        if generic_index.len() > MAX_STYLESHEET_BYTES {
            return Err(CosmeticError::ResourceLimit);
        }
        let fallback_names: BTreeSet<_> = fallback_generic.into_iter().collect();
        let fallback_generic = data
            .rules
            .iter()
            .filter(|r| fallback_names.contains(&r.selector))
            .cloned()
            .collect();
        // Rebuilt from the rules on every decode: the program is executable, so
        // a persisted copy is never trusted.
        let scriptlets = crate::scriptlets::build(&data.scriptlets)?;
        let encoded = serde_json::to_vec(&data).map_err(|_| CosmeticError::InvalidPolicy)?;
        if encoded.len() > MAX_POLICY_BYTES {
            return Err(CosmeticError::ResourceLimit);
        }
        let policy = Arc::new(Self {
            report: data.report,
            fingerprint: Sha256::digest(&encoded),
            encoded: encoded.into(),
            selective,
            generic_index_digest: zephium_core::blocker::ContentRuleDigest::from_bytes(
                Sha256::digest(generic_index.as_bytes()),
            ),
            generic_index,
            fallback_generic,
            scriptlets,
        });
        *LIVE_POLICY.lock().unwrap_or_else(|p| p.into_inner()) = Arc::downgrade(&policy);
        Ok(policy)
    }
}

impl zephium_core::blocker::DocumentStyleProvider for CosmeticPolicy {
    fn scriptlet_program(&self) -> Option<Arc<str>> {
        self.scriptlets
            .as_ref()
            .map(|program| program.script.clone())
    }

    fn fingerprint(&self) -> zephium_core::blocker::ContentRuleDigest {
        zephium_core::blocker::ContentRuleDigest::from_bytes(self.fingerprint)
    }

    fn generic_selectors(
        &self,
        url: &str,
        tokens: &[String],
    ) -> Result<Vec<String>, zephium_core::blocker::DocumentStyleFailure> {
        use zephium_core::blocker::DocumentStyleFailure as Failure;
        if tokens.len() > 256
            || tokens.iter().map(String::len).sum::<usize>() > 64 * 1024
            || tokens.iter().any(|token| {
                token.len() < 2 || token.len() > 4097 || !matches!(token.as_bytes()[0], b'.' | b'#')
            })
        {
            return Err(Failure::ResourceLimit);
        }
        let parsed = url::Url::parse(url).map_err(|_| Failure::InvalidDocument)?;
        if url.len() > 32768 || !matches!(parsed.scheme(), "http" | "https") {
            return Err(Failure::InvalidDocument);
        }
        let resources = self.selective.url_cosmetic_resources(url);
        if resources.generichide {
            return Ok(Vec::new());
        }
        let mut matched = self.selective.hidden_class_id_selectors(
            tokens.iter().filter_map(|token| token.strip_prefix('.')),
            tokens.iter().filter_map(|token| token.strip_prefix('#')),
            &resources.exceptions,
        );
        matched.sort();
        matched.dedup();
        matched.truncate(2048);
        if matched.iter().map(String::len).sum::<usize>() > MAX_STYLESHEET_BYTES {
            return Err(Failure::ResourceLimit);
        }
        Ok(matched)
    }

    fn document_plan(
        &self,
        url: &str,
    ) -> Result<zephium_core::blocker::DocumentStylePlan, zephium_core::blocker::DocumentStyleFailure>
    {
        use zephium_core::blocker::{DocumentStyleFailure, DocumentStylePlan};
        let parsed = url::Url::parse(url).map_err(|_| DocumentStyleFailure::InvalidDocument)?;
        if url.len() > 32768 || !matches!(parsed.scheme(), "http" | "https") {
            return Err(DocumentStyleFailure::InvalidDocument);
        }
        let resources = self.selective.url_cosmetic_resources(url);
        let mut selectors: BTreeSet<_> = resources.hide_selectors.into_iter().collect();
        if !resources.generichide {
            let host = parsed
                .host_str()
                .ok_or(DocumentStyleFailure::InvalidDocument)?
                .trim_end_matches('.');
            let mut allowed = BTreeSet::new();
            let mut excepted = BTreeSet::new();
            for rule in &self.fallback_generic {
                if rule.matches(host) {
                    if rule.exception {
                        excepted.insert(rule.selector.as_str());
                    } else {
                        allowed.insert(rule.selector.as_str());
                    }
                }
            }
            selectors.extend(
                allowed
                    .difference(&excepted)
                    .map(|selector| (*selector).to_owned()),
            );
        }
        let mut css = String::new();
        for selector in selectors {
            css.push_str(&selector);
            css.push_str("{display:none!important}\n");
        }
        if css.len() > MAX_STYLESHEET_BYTES {
            return Err(DocumentStyleFailure::ResourceLimit);
        }
        let exceptions: BTreeSet<_> = resources.exceptions.into_iter().collect();
        Ok(DocumentStylePlan {
            css: css.into(),
            generic_index: if resources.generichide {
                Arc::from("[]")
            } else {
                self.generic_index.clone()
            },
            generic_index_digest: if resources.generichide {
                // This constant-sized empty index is the same identity as pause.
                zephium_core::blocker::DocumentStylePlan::empty().generic_index_digest
            } else {
                self.generic_index_digest
            },
            exceptions: serde_json::to_string(&exceptions)
                .map_err(|_| DocumentStyleFailure::Unavailable)?
                .into(),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedCosmetics {
    pub(crate) policy: Arc<CosmeticPolicy>,
}

impl PreparedCosmetics {
    pub(crate) fn new(
        policy: Arc<CosmeticPolicy>,
        target: crate::CompileTarget,
    ) -> Result<Self, CosmeticError> {
        let _ = target;
        Ok(Self { policy })
    }
}

fn within(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn normalize_domain(domain: &str) -> Option<String> {
    let host = idna::domain_to_ascii(domain).ok()?.to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return None;
    }
    Some(host)
}

fn validate_selector(selector: &str) -> Result<(), CosmeticError> {
    if selector.is_empty() || selector.len() > MAX_SELECTOR_BYTES {
        return Err(CosmeticError::InvalidPolicy);
    }
    selector_preflight(selector)?;
    let parsed = CosmeticFilter::parse(
        &format!("example.invalid##{selector}"),
        false,
        Default::default(),
    )
    .map_err(|_| CosmeticError::InvalidPolicy)?;
    if parsed.action.is_some()
        || parsed.mask.contains(CosmeticFilterMask::SCRIPT_INJECT)
        || parsed.plain_css_selector() != Some(selector)
    {
        return Err(CosmeticError::InvalidPolicy);
    }
    Ok(())
}

fn selector_preflight(selector: &str) -> Result<(), CosmeticError> {
    if selector.is_empty() || selector.len() > MAX_SELECTOR_BYTES {
        return Err(CosmeticError::InvalidPolicy);
    }
    // Bound parser recursion before entering the upstream CSS grammar. This
    // is an admission ceiling, not a replacement for its syntax validation.
    let mut depth = 0u8;
    let mut quote = None;
    let mut escaped = false;
    let mut comment = false;
    let mut bytes = selector.bytes().peekable();
    while let Some(byte) = bytes.next() {
        if comment {
            if byte == b'*' && bytes.peek() == Some(&b'/') {
                bytes.next();
                comment = false;
            }
            continue;
        }
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            if byte == delimiter {
                quote = None;
            }
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'/' if bytes.peek() == Some(&b'*') => {
                bytes.next();
                comment = true;
            }
            b'(' | b'[' => {
                depth += 1;
                if depth > 32 {
                    return Err(CosmeticError::ResourceLimit);
                }
            }
            b')' | b']' => depth = depth.saturating_sub(1),
            b'{' | b'}' | b';' => return Err(CosmeticError::InvalidPolicy),
            _ => {}
        }
    }
    Ok(())
}

/// Validates and canonicalizes a personal selector without admitting scriptlets,
/// procedural actions, declarations or additional filter lines.
pub fn validate_personal_selector(selector: &str) -> Option<String> {
    if selector.chars().any(char::is_control) {
        return None;
    }
    selector_preflight(selector).ok()?;
    let parsed = CosmeticFilter::parse(
        &format!("example.invalid##{selector}"),
        false,
        Default::default(),
    )
    .ok()?;
    if parsed.action.is_some() || parsed.mask.contains(CosmeticFilterMask::SCRIPT_INJECT) {
        return None;
    }
    let canonical = parsed.plain_css_selector()?;
    validate_selector(canonical).ok()?;
    Some(canonical.to_owned())
}

/// Validates all personal rules, including disabled entries, before publishing
/// a complete derived snapshot. An invalid persisted row never becomes a
/// silently filtered subset. This operation performs no I/O.
pub fn prepare_site_preferences(
    preferences: &zephium_core::blocker::BlockerSitePreferences,
) -> Option<Arc<zephium_core::blocker::PreparedBlockerSites>> {
    let mut entries: BTreeMap<zephium_core::blocker::BlockerSite, (bool, BTreeSet<String>)> =
        BTreeMap::new();
    for site in preferences.paused_sites() {
        entries.entry(site.clone()).or_default().0 = true;
    }
    for hide in preferences.hides() {
        let selector = validate_personal_selector(&hide.selector)?;
        if hide.enabled {
            entries
                .entry(hide.site.clone())
                .or_default()
                .1
                .insert(selector);
        }
    }
    let entries = entries.into_iter().map(|(site, (paused, selectors))| {
        let mut css = String::new();
        for selector in selectors {
            css.push_str(&selector);
            css.push_str("{display:none!important}\n");
        }
        (site, paused, Arc::from(css))
    });
    zephium_core::blocker::PreparedBlockerSites::new(preferences.revision(), entries)
}

fn parse_rule(line: &str) -> Result<Rule, CosmeticError> {
    let first = line.find('#').ok_or(CosmeticError::InvalidPolicy)?;
    let second = line[first + 1..]
        .find('#')
        .ok_or(CosmeticError::InvalidPolicy)?
        + first
        + 1;
    selector_preflight(&line[second + 1..])?;
    let parsed = CosmeticFilter::parse(line, false, Default::default())
        .map_err(|_| CosmeticError::InvalidPolicy)?;
    if parsed.action.is_some() || parsed.mask.contains(CosmeticFilterMask::SCRIPT_INJECT) {
        return Err(CosmeticError::InvalidPolicy);
    }
    let selector = parsed
        .plain_css_selector()
        .ok_or(CosmeticError::InvalidPolicy)?
        .to_owned();
    validate_selector(&selector)?;
    let (scope, _) = line.split_once('#').ok_or(CosmeticError::InvalidPolicy)?;
    let mut include = BTreeSet::new();
    let mut exclude = BTreeSet::new();
    if !scope.is_empty() {
        for domain in scope.split(',') {
            let (domain, negative) = domain
                .strip_prefix('~')
                .map_or((domain, false), |d| (d, true));
            let domain = normalize_domain(domain).ok_or(CosmeticError::InvalidPolicy)?;
            if negative {
                exclude.insert(domain);
            } else {
                include.insert(domain);
            }
            if include.len() + exclude.len() > MAX_SCOPE_DOMAINS {
                return Err(CosmeticError::ResourceLimit);
            }
        }
    }
    if include.len() + exclude.len() > MAX_SCOPE_DOMAINS {
        return Err(CosmeticError::InvalidPolicy);
    }
    Ok(Rule {
        selector,
        include: include.into_iter().collect(),
        exclude: exclude.into_iter().collect(),
        exception: parsed.mask.contains(CosmeticFilterMask::UNHIDE),
    })
}

/// Whether a control parses and its regex compiles within the generic-hide
/// matcher's limits, so one control cannot fail the whole matcher.
fn control_compiles(control: &str) -> bool {
    use adblock::filters::network::{FilterPart, NetworkFilter};
    let Ok(filter) = NetworkFilter::parse(control, false, Default::default()) else {
        return false;
    };
    let patterns: Vec<&str> = match &filter.filter {
        FilterPart::Empty => Vec::new(),
        FilterPart::Simple(pattern) => vec![pattern.as_ref()],
        FilterPart::AnyOf(patterns) => patterns.iter().map(AsRef::as_ref).collect(),
    };
    adblock::regex_manager::regex_compiles_within(
        filter.mask,
        &patterns,
        CONTROL_REGEX_SIZE_BYTES,
        CONTROL_REGEX_DFA_SIZE_BYTES,
    )
}

fn generic_control(line: &str) -> Result<Option<String>, CosmeticError> {
    let Some((pattern, options)) = line.rsplit_once('$') else {
        return Ok(None);
    };
    if !options
        .split(',')
        .any(|o| matches!(o, "generichide" | "ghide"))
    {
        return Ok(None);
    }
    let pattern = pattern
        .strip_prefix("@@")
        .ok_or(CosmeticError::InvalidPolicy)?;
    let mut output = vec!["document".to_owned()];
    for option in options.split(',') {
        if matches!(option, "generichide" | "ghide") {
            continue;
        }
        if option == "match-case" {
            output.push(option.into());
            continue;
        }
        let value = option
            .strip_prefix("domain=")
            .ok_or(CosmeticError::InvalidPolicy)?;
        let domains: Option<Vec<_>> = value
            .split('|')
            .map(|domain| {
                let (domain, negative) = domain
                    .strip_prefix('~')
                    .map_or((domain, false), |d| (d, true));
                normalize_domain(domain).map(|d| if negative { format!("~{d}") } else { d })
            })
            .collect();
        let domains = domains.ok_or(CosmeticError::InvalidPolicy)?;
        if domains.len() > MAX_SCOPE_DOMAINS {
            return Err(CosmeticError::ResourceLimit);
        }
        output.push(format!("domain={}", domains.join("|")));
    }
    Ok(Some(format!("{pattern}${}", output.join(","))))
}

#[cfg(test)]
mod tests {
    use super::*;

    use zephium_core::blocker::DocumentStyleProvider;
    impl CosmeticPolicy {
        fn stylesheet(
            &self,
            url: &str,
        ) -> Result<Arc<str>, zephium_core::blocker::DocumentStyleFailure> {
            let plan = self.document_plan(url)?;
            let entries: Vec<(String, Vec<String>)> =
                serde_json::from_str(&plan.generic_index).unwrap();
            let exceptions: BTreeSet<String> = serde_json::from_str(&plan.exceptions).unwrap();
            let mut css = plan.css.to_string();
            for selector in entries
                .into_iter()
                .flat_map(|(_, v)| v)
                .collect::<BTreeSet<_>>()
            {
                if !exceptions.contains(&selector) {
                    css.push_str(&format!("{selector}{{display:none!important}}\n"));
                }
            }
            Ok(css.into())
        }
    }
    #[test]
    fn scopes_exceptions_and_generic_controls_compose_across_sources() {
        let policy = CosmeticPolicy::compile([
            "##.ad\nexample.com##.specific\n~safe.example.com##.banner\n",
            "example.com#@#.ad\n@@||quiet.example.com^$generichide\n",
        ])
        .unwrap();
        let css = policy.stylesheet("https://www.example.com/").unwrap();
        assert!(!css.contains(".ad{"));
        assert!(css.contains(".specific{"));
        assert!(css.contains(".banner{"));
        assert!(!policy
            .stylesheet("https://safe.example.com/")
            .unwrap()
            .contains(".banner{"));
        let quiet = policy.stylesheet("https://quiet.example.com/").unwrap();
        assert_eq!(quiet.as_ref(), ".specific{display:none!important}\n");
        assert!(policy
            .stylesheet("https://notexample.com/")
            .unwrap()
            .contains(".ad{"));
    }

    #[test]
    fn unsupported_code_and_selector_injection_are_never_css() {
        let policy = CosmeticPolicy::compile([
            "example.com##+js(foo)\nexample.com##.ad:remove()\nexample.com##.ad:has-text(foo)\nexample.com##.ad{}body{color:red}\nexample.*##.ad\n##.good",
        ]).unwrap();
        assert_eq!(policy.report().accepted, 1);
        // The scriptlet rule goes to the scriptlet program, never into CSS.
        assert_eq!(policy.report().rejected, 4);
        assert_eq!(policy.report().scriptlet_rules, 1);
        assert_eq!(policy.report().unknown_scriptlet_rules, 1);
        assert_eq!(
            policy.stylesheet("https://example.com/").unwrap().as_ref(),
            ".good{display:none!important}\n"
        );
        // An unsupported generic-hide control is rejected alone, so hiding
        // stays on there instead of the whole policy failing.
        let unsupported =
            CosmeticPolicy::compile(["@@||example.com^$generichide,third-party\n##.good"]).unwrap();
        assert_eq!(unsupported.report().rejected, 1);
        assert_eq!(unsupported.report().generic_controls, 0);
        assert_eq!(
            unsupported
                .stylesheet("https://example.com/")
                .unwrap()
                .as_ref(),
            ".good{display:none!important}\n"
        );
    }

    #[test]
    fn cache_round_trip_preserves_policy_and_revalidates_selectors() {
        let policy = CosmeticPolicy::compile(["##.ad\nexample.com#@#.ad"]).unwrap();
        let bytes = policy.encode().unwrap();
        let restored = CosmeticPolicy::decode(&bytes).unwrap();
        assert_eq!(restored.encode().unwrap(), bytes);
        assert_eq!(
            restored
                .stylesheet("https://example.com/")
                .unwrap()
                .as_ref(),
            ""
        );
        let mut data: Data = serde_json::from_slice(&bytes).unwrap();
        data.rules[0].selector = ".ad{}body{color:red}".into();
        assert!(CosmeticPolicy::decode(&serde_json::to_vec(&data).unwrap()).is_err());
    }

    #[test]
    fn unrelated_documents_share_the_generic_lookup_allocation() {
        let policy = CosmeticPolicy::compile(["##.ad\nexample.com#@#.ad"]).unwrap();
        let first = policy.document_plan("https://first.invalid/").unwrap();
        let second = policy.document_plan("https://second.invalid/").unwrap();
        assert!(Arc::ptr_eq(&first.generic_index, &second.generic_index));
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_ne!(
            first.fingerprint(),
            policy
                .document_plan("https://example.com/")
                .unwrap()
                .fingerprint()
        );
        assert!(policy
            .stylesheet("https://example.com/")
            .unwrap()
            .is_empty());
        let deeply_nested = format!("{}a{}", ":is(".repeat(33), ")".repeat(33));
        assert!(validate_personal_selector(&deeply_nested).is_none());
        assert!(validate_personal_selector(".ad\n##body").is_none());
    }

    #[test]
    fn fallback_selectors_preserve_domain_exclusions_and_exceptions() {
        let policy = CosmeticPolicy::compile([
            "##.реклама\nexample.com#@#.реклама\n~safe.example.com##.баннер\nexample.com##.баннер\n@@||quiet.example^$generichide",
        ])
        .unwrap();
        let regular = policy.document_plan("https://other.invalid/").unwrap();
        assert!(regular.css.contains(".реклама{"));
        assert!(regular.css.contains(".баннер{"));
        let excepted = policy.document_plan("https://example.com/").unwrap();
        assert!(!excepted.css.contains(".реклама{"));
        assert!(excepted.css.contains(".баннер{"));
        let excluded = policy.document_plan("https://safe.example.com/").unwrap();
        // A matching site-specific positive rule still applies.
        assert!(excluded.css.contains(".баннер{"));
        let quiet = policy.document_plan("https://quiet.example/").unwrap();
        assert_eq!(quiet, zephium_core::blocker::DocumentStylePlan::empty());
        let restored = CosmeticPolicy::decode(&policy.encode().unwrap()).unwrap();
        assert_eq!(
            restored.document_plan("https://example.com/").unwrap(),
            excepted
        );
    }

    #[test]
    fn subscription_identity_changes_with_index_and_generic_hide_controls() {
        let plain = CosmeticPolicy::compile(["##.ad"]).unwrap();
        let expanded = CosmeticPolicy::compile(["##.ad\n##.banner"]).unwrap();
        let controlled =
            CosmeticPolicy::compile(["##.ad\n@@||example.com/quiet$generichide"]).unwrap();
        let regular = plain.document_plan("https://example.com/").unwrap();
        assert_ne!(
            regular.fingerprint(),
            expanded
                .document_plan("https://example.com/")
                .unwrap()
                .fingerprint()
        );
        assert_eq!(
            regular.fingerprint(),
            controlled
                .document_plan("https://example.com/")
                .unwrap()
                .fingerprint()
        );
        assert_eq!(
            controlled
                .document_plan("https://example.com/quiet")
                .unwrap(),
            zephium_core::blocker::DocumentStylePlan::empty()
        );
    }

    #[test]
    fn selective_plan_preserves_scope_exceptions_without_blanket_generic_css() {
        let policy = CosmeticPolicy::compile(["##.ad\nexample.com##.specific\nexample.com#@#.ad\n@@||quiet.example^$generichide\nquiet.example##.specific"]).unwrap();
        let plan = policy.document_plan("https://example.com/").unwrap();
        assert!(!plan.css.contains(".ad{"));
        assert!(plan.css.contains(".specific{"));
        assert!(plan.exceptions.contains(".ad"));
        let quiet = policy.document_plan("https://quiet.example/").unwrap();
        assert_eq!(quiet.generic_index.as_ref(), "[]");
        assert!(quiet.css.contains(".specific{"));
    }
    #[test]
    fn worker_generic_lookup_obeys_domain_exceptions_and_generichide() {
        use zephium_core::blocker::{DocumentStyleFailure, DocumentStyleProvider};
        let policy = CosmeticPolicy::compile([
            "##.ad-slot\n###ad-box\nexcept.example#@#.ad-slot\n@@||quiet.example^$generichide",
        ])
        .unwrap();
        let tokens = vec![
            ".ad-slot".to_owned(),
            "#ad-box".to_owned(),
            ".useful".to_owned(),
        ];
        let normal = policy
            .generic_selectors("https://normal.example/", &tokens)
            .unwrap();
        assert!(normal.contains(&".ad-slot".to_owned()));
        assert!(normal.contains(&"#ad-box".to_owned()));
        assert!(!normal.contains(&".useful".to_owned()));
        let excepted = policy
            .generic_selectors("https://except.example/", &tokens)
            .unwrap();
        assert!(!excepted.contains(&".ad-slot".to_owned()));
        assert!(excepted.contains(&"#ad-box".to_owned()));
        assert!(policy
            .generic_selectors("https://quiet.example/", &tokens)
            .unwrap()
            .is_empty());
        assert_eq!(
            policy.generic_selectors("https://normal.example/", &vec![".x".to_owned(); 257]),
            Err(DocumentStyleFailure::ResourceLimit)
        );
    }

    #[test]
    fn a_scriptlet_the_library_lacks_is_counted_and_leaves_no_trace() {
        let policy = CosmeticPolicy::compile([concat!(
            "known.example##+js(set, a.b, true)\n",
            "only-unknown.example##+js(no-such-scriptlet, x)\n",
        )])
        .unwrap();
        assert_eq!(policy.report().scriptlet_rules, 2);
        assert_eq!(policy.report().unknown_scriptlet_rules, 1);
        assert_eq!(policy.scriptlet_coverage(), (1, 1));
        let program = policy.scriptlet_program().unwrap();
        assert!(!program.contains("no-such-scriptlet"));
        assert!(!program.contains(&crate::scriptlets::key_hash("only-unknown.example").to_string()));
        let encoded = String::from_utf8_lossy(&policy.encode().unwrap()).into_owned();
        assert!(!encoded.contains("no-such-scriptlet"));
    }

    #[test]
    fn a_generic_hide_control_past_the_regex_limits_is_rejected() {
        let policy = CosmeticPolicy::compile([concat!(
            "@@||ok.example^$generichide\n",
            r"@@/^https?:\/\/big\.example\/[+\/0-9a-zA-Z]{400,1200}$/$generichide",
            "\n",
        )])
        .unwrap();
        assert_eq!(policy.report().generic_controls, 1);
        assert_eq!(policy.report().rejected, 1);
    }

    #[test]
    fn generic_hide_controls_are_admitted_up_to_their_limit() {
        let controls = |count: usize| -> String {
            (0..count)
                .map(|n| format!("@@||c{n}.example^$generichide\n"))
                .collect()
        };
        let policy = CosmeticPolicy::compile([controls(MAX_CONTROLS).as_str()]).unwrap();
        assert_eq!(policy.report().generic_controls, MAX_CONTROLS);
        assert!(CosmeticPolicy::decode(&policy.encode().unwrap()).is_ok());
        assert_eq!(
            CosmeticPolicy::compile([controls(MAX_CONTROLS + 1).as_str()]).err(),
            Some(CosmeticError::ResourceLimit)
        );
    }
}
