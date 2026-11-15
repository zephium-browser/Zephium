//! Transactional DNR API state; adapters compile, persist and publish before settlement.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use zephium_webext_dnr as dnr;

use crate::browser::{self, BrowserPort};
use crate::dispatch::{ApiTable, Handled, Invocation, RegistrationError};
use crate::{ApiError, InstallPermissions, Sender};

pub(crate) const METHODS: &[&str] = &[
    "declarativeNetRequest.updateDynamicRules",
    "declarativeNetRequest.updateSessionRules",
    "declarativeNetRequest.getDynamicRules",
    "declarativeNetRequest.getSessionRules",
    "declarativeNetRequest.updateEnabledRulesets",
    "declarativeNetRequest.getEnabledRulesets",
    "declarativeNetRequest.updateStaticRules",
    "declarativeNetRequest.getDisabledRuleIds",
    "declarativeNetRequest.getAvailableStaticRuleCount",
    "declarativeNetRequest.isRegexSupported",
    "declarativeNetRequest.testMatchOutcome",
    "declarativeNetRequest.getMatchedRules",
    "declarativeNetRequest.setExtensionActionOptions",
];

/// Maximum retained feedback records per install; older records are discarded first.
pub const MAX_MATCHED_RULES: usize = 10_000;
/// Feedback without a live tab is retained for five minutes.
pub const MATCHED_RULE_RETENTION_MS: u64 = 5 * 60 * 1_000;
/// Chrome's getMatchedRules quota, excluding trusted user gestures.
pub const MAX_GET_MATCHED_RULES_CALLS: usize = 20;
/// Quota window in milliseconds.
pub const GET_MATCHED_RULES_INTERVAL_MS: u64 = 10 * 60 * 1_000;

const FEEDBACK_PERMISSION_ERROR: &str = "The extension must have the declarativeNetRequestFeedback permission or have activeTab granted for the specified tab ID in order to call this function.";

/// An admitted static resource, with invalid package rules already reported by the host.
#[derive(Clone, Debug)]
pub struct StaticRuleset {
    /// Manifest-local identifier, excluding reserved leading underscores.
    pub id: String,
    /// The current enabled choice; initialized from the manifest.
    pub enabled: bool,
    /// Accepted rules, including separately reported unsupported Linux rule kinds.
    pub rules: Vec<dnr::Rule>,
}

/// One reported effective rule, retaining no browsing URL.
#[derive(Clone, Debug)]
pub struct MatchedRule {
    /// Manifest or reserved dynamic/session ruleset identity.
    pub ruleset_id: String,
    /// Positive rule identity within the ruleset.
    pub rule_id: u32,
    /// Ordinary tab identity, or -1 for requests with no live tab.
    pub tab_id: i32,
    /// Unix milliseconds from the authenticated host observation.
    pub time_stamp: f64,
    active_document: bool,
}

/// Profile/install-scoped state. Clone to prepare an update without changing live rules.
#[derive(Clone, Debug)]
pub struct DnrState {
    /// Raw schema-normalized rules preserve the getDynamicRules round trip.
    pub dynamic_rules: Vec<Value>,
    /// Ephemeral rules; never included in a durable snapshot.
    pub session_rules: Vec<Value>,
    /// Static package declarations in manifest order.
    pub static_rulesets: Vec<StaticRuleset>,
    /// Disabled rule identities, including disabled rulesets.
    pub disabled_static_rules: BTreeMap<String, BTreeSet<u32>>,
    /// Host allocation from the profile's shared static-rule pool.
    pub static_rule_limit: usize,
    /// Whether native action counts should be presented as badge text.
    pub display_action_count_as_badge_text: bool,
    /// Native per-tab effective action counts, reset on a new document.
    pub badge_counts: BTreeMap<i32, i32>,
    matched: VecDeque<MatchedRule>,
    feedback_queries: VecDeque<u64>,
}

impl Default for DnrState {
    fn default() -> Self {
        Self {
            dynamic_rules: Vec::new(),
            session_rules: Vec::new(),
            static_rulesets: Vec::new(),
            disabled_static_rules: BTreeMap::new(),
            static_rule_limit: dnr::GUARANTEED_MINIMUM_STATIC_RULES,
            display_action_count_as_badge_text: false,
            badge_counts: BTreeMap::new(),
            matched: VecDeque::new(),
            feedback_queries: VecDeque::new(),
        }
    }
}

/// Owned work that can be prepared by an adapter's FIFO compilation executor.
#[derive(Clone, Debug)]
pub enum Request {
    /// Remove first, then add; validate the complete result atomically.
    UpdateRules {
        /// Dynamic or session scope.
        kind: dnr::RulesetKind,
        /// Unknown removals are ignored.
        remove_rule_ids: Vec<u32>,
        /// Schema-validated new rules.
        add_rules: Vec<Value>,
    },
    /// Disable before enabling, applying a single candidate choice.
    UpdateEnabledRulesets {
        /// Manifest identifiers to enable.
        enable: Vec<String>,
        /// Manifest identifiers to disable.
        disable: Vec<String>,
    },
    /// Change disabled static identities without changing their source file.
    UpdateStaticRules {
        /// Manifest ruleset identity.
        ruleset_id: String,
        /// Rules to re-enable.
        enable: Vec<u32>,
        /// Rules to disable.
        disable: Vec<u32>,
    },
    /// Native action-model integration; counts are ephemeral.
    SetExtensionActionOptions {
        /// Toggle automatic badge text.
        display_action_count_as_badge_text: Option<bool>,
        /// Ordinary tab and signed count increment.
        tab_update: Option<(i32, i32)>,
    },
    /// Simulate the same arbitration with current granted host permissions.
    TestMatchOutcome {
        /// Generated hypothetical request details.
        request: Value,
        /// Current install permissions, fenced again by the adapter at completion.
        permissions: Box<InstallPermissions>,
        /// Owning extension origin identity.
        extension_id: String,
    },
    /// Permission checks precede this shared quota and bounded history query.
    GetMatchedRules {
        /// Restrict to one tab, including -1.
        tab_id: Option<i32>,
        /// Strict lower timestamp bound.
        min_time_stamp: Option<f64>,
        /// Trusted Unix milliseconds.
        now_ms: u64,
        /// A trusted native user gesture bypasses Chrome's quota.
        bypass_quota: bool,
    },
}

impl Request {
    /// Whether the adapter must compile and publish a new policy before settlement.
    pub fn changes_policy(&self) -> bool {
        matches!(
            self,
            Self::UpdateRules { .. }
                | Self::UpdateEnabledRulesets { .. }
                | Self::UpdateStaticRules { .. }
        )
    }

    /// Whether successful work changes durable state, including the action-count preference.
    pub fn changes_persistent_state(&self) -> bool {
        matches!(
            self,
            Self::UpdateRules {
                kind: dnr::RulesetKind::Dynamic,
                ..
            } | Self::UpdateEnabledRulesets { .. }
                | Self::UpdateStaticRules { .. }
                | Self::SetExtensionActionOptions {
                    display_action_count_as_badge_text: Some(_),
                    ..
                }
        )
    }
}

/// Runtime host boundary. A successful queued mutation means durable and native publication.
pub trait DnrHost {
    /// Returns the current committed state for this authenticated install generation.
    fn dnr_state(&self, sender: &Sender) -> Result<&DnrState, ApiError>;
    /// Queues work. Recheck the sender, persist the candidate, publish to attached pages,
    /// then replace committed state and settle. A failure retains the previous state.
    fn dnr_request(&mut self, request: Request, invocation: Invocation<'_>) -> Handled;
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    dynamic_rules: Vec<Value>,
    enabled_rulesets: Vec<String>,
    disabled_static_rules: BTreeMap<String, BTreeSet<u32>>,
    #[serde(default)]
    display_action_count_as_badge_text: bool,
}

impl DnrState {
    /// Creates initial choices from admitted package resources. The host assigns any
    /// shared-pool allocation before compiling enabled rules.
    pub fn from_static(static_rulesets: Vec<StaticRuleset>) -> Result<Self, ApiError> {
        if static_rulesets.len() > dnr::MAX_NUMBER_OF_STATIC_RULESETS {
            return Err(ApiError::other(
                "The number of static rulesets exceeds the maximum allowed.",
            ));
        }
        let mut seen = BTreeSet::new();
        for set in &static_rulesets {
            if set.id.is_empty()
                || set.id.starts_with('_')
                || set.id.len() > 1024
                || !seen.insert(&set.id)
            {
                return Err(ApiError::other("Invalid or duplicate static ruleset ID."));
            }
        }
        if static_rulesets.iter().filter(|set| set.enabled).count()
            > dnr::MAX_NUMBER_OF_ENABLED_STATIC_RULESETS
        {
            return Err(ApiError::other(
                "The number of enabled static rulesets exceeds the enabled ruleset count limit.",
            ));
        }
        Ok(Self {
            static_rulesets,
            ..Self::default()
        })
    }

    /// Validates raw dynamic/session rules with the common compiler grammar and quotas.
    pub fn rules(&self, kind: dnr::RulesetKind) -> Result<Vec<dnr::Rule>, ApiError> {
        parse_rules(
            match kind {
                dnr::RulesetKind::Dynamic => &self.dynamic_rules,
                dnr::RulesetKind::Session => &self.session_rules,
                dnr::RulesetKind::Static => return Err(ApiError::malformed_arguments()),
            },
            kind,
        )
    }

    /// Effective static rules in manifest order, excluding disabled identities.
    pub fn enabled_static_rules(&self) -> Vec<(&str, Vec<dnr::Rule>)> {
        self.static_rulesets
            .iter()
            .filter(|set| set.enabled)
            .map(|set| {
                let disabled = self.disabled_static_rules.get(&set.id);
                (
                    set.id.as_str(),
                    set.rules
                        .iter()
                        .filter(|rule| !disabled.is_some_and(|ids| ids.contains(&rule.id)))
                        .cloned()
                        .collect(),
                )
            })
            .collect()
    }

    /// Effective enabled static count used by the profile allocator.
    pub fn enabled_static_rule_count(&self) -> usize {
        self.static_rulesets
            .iter()
            .filter(|set| set.enabled)
            .map(|set| {
                let disabled = self.disabled_static_rules.get(&set.id);
                set.rules
                    .iter()
                    .filter(|rule| !disabled.is_some_and(|ids| ids.contains(&rule.id)))
                    .count()
            })
            .sum()
    }

    /// Encodes only durable rule state. The adapter writes this atomically per install.
    pub fn snapshot(&self) -> Result<Vec<u8>, ApiError> {
        let snapshot = Snapshot {
            version: 1,
            dynamic_rules: self.dynamic_rules.clone(),
            enabled_rulesets: self
                .static_rulesets
                .iter()
                .filter(|set| set.enabled)
                .map(|set| set.id.clone())
                .collect(),
            disabled_static_rules: self.disabled_static_rules.clone(),
            display_action_count_as_badge_text: self.display_action_count_as_badge_text,
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(|_| ApiError::malformed_arguments())?;
        if bytes.len() > dnr::MAX_RULESET_JSON_BYTES {
            return Err(ApiError::message_too_large());
        }
        Ok(bytes)
    }

    /// Restores against the admitted package definitions. Malformed snapshots leave
    /// existing state untouched; session rules, feedback and badge counts start empty.
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        if bytes.len() > dnr::MAX_RULESET_JSON_BYTES {
            return Err(ApiError::message_too_large());
        }
        let snapshot: Snapshot = serde_json::from_slice(bytes)
            .map_err(|_| ApiError::other("Invalid declarativeNetRequest snapshot."))?;
        if snapshot.version != 1 {
            return Err(ApiError::other(
                "Invalid declarativeNetRequest snapshot version.",
            ));
        }
        if snapshot.enabled_rulesets.len() > dnr::MAX_NUMBER_OF_STATIC_RULESETS
            || snapshot
                .enabled_rulesets
                .iter()
                .collect::<BTreeSet<_>>()
                .len()
                != snapshot.enabled_rulesets.len()
            || snapshot.disabled_static_rules.len() > dnr::MAX_NUMBER_OF_STATIC_RULESETS
            || snapshot
                .disabled_static_rules
                .values()
                .map(BTreeSet::len)
                .sum::<usize>()
                > dnr::MAX_NUMBER_OF_DISABLED_STATIC_RULES
        {
            return Err(ApiError::other(
                "Invalid declarativeNetRequest snapshot limits.",
            ));
        }
        parse_rules(&snapshot.dynamic_rules, dnr::RulesetKind::Dynamic)?;
        let mut candidate = Self::from_static(self.static_rulesets.clone())?;
        candidate.static_rule_limit = self.static_rule_limit;
        candidate.dynamic_rules = snapshot.dynamic_rules;
        candidate.display_action_count_as_badge_text = snapshot.display_action_count_as_badge_text;
        for id in &snapshot.enabled_rulesets {
            candidate.static_ruleset(id)?;
        }
        for set in &mut candidate.static_rulesets {
            set.enabled = snapshot.enabled_rulesets.contains(&set.id);
        }
        for (id, disabled) in &snapshot.disabled_static_rules {
            let set = candidate.static_ruleset(id)?;
            let known: BTreeSet<_> = set.rules.iter().map(|rule| rule.id).collect();
            if !disabled.is_subset(&known) {
                return Err(ApiError::other(
                    "Invalid disabled static rule ID in snapshot.",
                ));
            }
        }
        candidate.disabled_static_rules = snapshot.disabled_static_rules;
        candidate.check_static_limits()?;
        *self = candidate;
        Ok(())
    }

    /// Restores dynamic rules and the badge preference after an extension update.
    /// Static choices reset to the new manifest, and session rules are discarded.
    pub fn restore_after_update(&mut self, bytes: &[u8]) -> Result<(), ApiError> {
        if bytes.len() > dnr::MAX_RULESET_JSON_BYTES {
            return Err(ApiError::message_too_large());
        }
        let snapshot: Snapshot = serde_json::from_slice(bytes)
            .map_err(|_| ApiError::other("Invalid declarativeNetRequest snapshot."))?;
        if snapshot.version != 1 {
            return Err(ApiError::other(
                "Invalid declarativeNetRequest snapshot version.",
            ));
        }
        parse_rules(&snapshot.dynamic_rules, dnr::RulesetKind::Dynamic)?;
        let mut candidate = Self::from_static(self.static_rulesets.clone())?;
        candidate.static_rule_limit = self.static_rule_limit;
        candidate.dynamic_rules = snapshot.dynamic_rules;
        candidate.display_action_count_as_badge_text = snapshot.display_action_count_as_badge_text;
        *self = candidate;
        Ok(())
    }

    /// Stages a mutation without modifying the committed state.
    pub fn prepare(&self, request: &Request) -> Result<Self, ApiError> {
        let mut candidate = self.clone();
        match request {
            Request::UpdateRules {
                kind,
                remove_rule_ids,
                add_rules,
            } => {
                let current = match kind {
                    dnr::RulesetKind::Dynamic => &mut candidate.dynamic_rules,
                    dnr::RulesetKind::Session => &mut candidate.session_rules,
                    dnr::RulesetKind::Static => return Err(ApiError::malformed_arguments()),
                };
                let removals: BTreeSet<_> = remove_rule_ids.iter().copied().collect();
                current.retain(|rule| !removals.contains(&rule_id(rule).unwrap_or(0)));
                current.extend(add_rules.iter().cloned().map(|mut rule| {
                    if let Some(object) = rule.as_object_mut() {
                        object.entry("priority").or_insert(json!(1));
                    }
                    rule
                }));
                parse_rules(current, *kind)?;
            }
            Request::UpdateEnabledRulesets { enable, disable } => {
                for id in enable.iter().chain(disable) {
                    candidate.static_ruleset(id)?;
                }
                for set in &mut candidate.static_rulesets {
                    if disable.contains(&set.id) {
                        set.enabled = false;
                    }
                    if enable.contains(&set.id) {
                        set.enabled = true;
                    }
                }
                candidate.check_static_limits()?;
            }
            Request::UpdateStaticRules {
                ruleset_id,
                enable,
                disable,
            } => {
                let known: BTreeSet<_> = candidate
                    .static_ruleset(ruleset_id)?
                    .rules
                    .iter()
                    .map(|rule| rule.id)
                    .collect();
                let ids = candidate
                    .disabled_static_rules
                    .entry(ruleset_id.clone())
                    .or_default();
                for id in disable {
                    if known.contains(id) {
                        ids.insert(*id);
                    }
                }
                for id in enable {
                    ids.remove(id);
                }
                candidate.check_static_limits()?;
            }
            Request::SetExtensionActionOptions {
                display_action_count_as_badge_text,
                tab_update,
            } => {
                if let Some(display) = display_action_count_as_badge_text {
                    candidate.display_action_count_as_badge_text = *display;
                }
                if let Some((tab, increment)) = tab_update {
                    if !candidate.display_action_count_as_badge_text {
                        return Err(ApiError::other("Cannot increment action count unless displaying action count as badge text."));
                    }
                    if *tab < 0 {
                        return Err(ApiError::other(format!("No tab with id: {tab}")));
                    }
                    let count = candidate.badge_counts.entry(*tab).or_default();
                    *count = count.saturating_add(*increment).max(0);
                }
            }
            _ => return Err(ApiError::malformed_arguments()),
        }
        Ok(candidate)
    }

    /// Prepares owned executor work and its API result. Adapters publish only for
    /// changes_policy; feedback queries still replace the shared quota state.
    pub fn execute(&self, request: &Request) -> Result<(Self, Value), ApiError> {
        match request {
            Request::GetMatchedRules {
                tab_id,
                min_time_stamp,
                now_ms,
                bypass_quota,
            } => {
                let mut candidate = self.clone();
                let reply =
                    candidate.matched_rules(*tab_id, *min_time_stamp, *now_ms, *bypass_quota)?;
                Ok((candidate, reply))
            }
            Request::TestMatchOutcome {
                request,
                permissions,
                extension_id,
            } => Ok((
                self.clone(),
                self.test_match_outcome(request, permissions, extension_id)?,
            )),
            _ => Ok((self.prepare(request)?, Value::Null)),
        }
    }

    fn static_ruleset(&self, id: &str) -> Result<&StaticRuleset, ApiError> {
        self.static_rulesets
            .iter()
            .find(|set| set.id == id)
            .ok_or_else(|| ApiError::other(format!("Invalid ruleset id: {id}.")))
    }

    /// Validates the host's allocation before publishing any enabled static rules.
    pub fn check_static_limits(&self) -> Result<(), ApiError> {
        if self
            .static_rulesets
            .iter()
            .filter(|set| set.enabled)
            .count()
            > dnr::MAX_NUMBER_OF_ENABLED_STATIC_RULESETS
        {
            return Err(ApiError::other(
                "The number of enabled static rulesets exceeds the enabled ruleset count limit.",
            ));
        }
        if self
            .disabled_static_rules
            .values()
            .map(BTreeSet::len)
            .sum::<usize>()
            > dnr::MAX_NUMBER_OF_DISABLED_STATIC_RULES
        {
            return Err(ApiError::other(
                "The number of disabled static rules exceeds the disabled rule count limit.",
            ));
        }
        if self.enabled_static_rule_count() > self.static_rule_limit {
            return Err(ApiError::other(
                "The set of enabled rulesets exceeds the rule count limit.",
            ));
        }
        let regex = self
            .static_rulesets
            .iter()
            .filter(|set| set.enabled)
            .flat_map(|set| {
                set.rules.iter().filter(|rule| {
                    !self
                        .disabled_static_rules
                        .get(&set.id)
                        .is_some_and(|ids| ids.contains(&rule.id))
                })
            })
            .filter(|rule| rule.condition.regex_filter.is_some())
            .count();
        if regex > dnr::MAX_NUMBER_OF_REGEX_RULES {
            return Err(ApiError::other(
                "The set of enabled rulesets exceeds the regular expression rule count limit.",
            ));
        }
        Ok(())
    }

    /// Accepts a rule report only if that identity is currently enabled. Native
    /// adapters additionally authenticate its policy generation and request facts.
    pub fn record_match(
        &mut self,
        ruleset_id: &str,
        rule_id: u32,
        tab_id: i32,
        time_stamp: f64,
    ) -> bool {
        if !time_stamp.is_finite() || time_stamp < 0.0 || tab_id < -1 {
            return false;
        }
        let known = match ruleset_id {
            dnr::DYNAMIC_RULESET_ID => self
                .dynamic_rules
                .iter()
                .any(|rule| self::rule_id(rule) == Some(rule_id)),
            dnr::SESSION_RULESET_ID => self
                .session_rules
                .iter()
                .any(|rule| self::rule_id(rule) == Some(rule_id)),
            _ => self.static_rulesets.iter().any(|set| {
                set.enabled
                    && set.id == ruleset_id
                    && set.rules.iter().any(|rule| rule.id == rule_id)
                    && !self
                        .disabled_static_rules
                        .get(ruleset_id)
                        .is_some_and(|ids| ids.contains(&rule_id))
            }),
        };
        if !known {
            return false;
        }
        if self.matched.len() == MAX_MATCHED_RULES {
            self.matched.pop_front();
        }
        self.matched.push_back(MatchedRule {
            ruleset_id: ruleset_id.to_owned(),
            rule_id,
            tab_id,
            time_stamp,
            active_document: tab_id >= 0,
        });
        if tab_id >= 0 {
            let count = self.badge_counts.entry(tab_id).or_default();
            *count = count.saturating_add(1);
        }
        true
    }

    /// Clears action counts on a committed main document; old feedback remains queryable.
    pub fn navigation_committed(&mut self, tab_id: i32) {
        self.badge_counts.remove(&tab_id);
        for record in &mut self.matched {
            if record.tab_id == tab_id {
                record.active_document = false;
            }
        }
    }

    /// Moves closed-tab feedback to Chrome's no-tab bucket without changing match timestamps.
    pub fn tab_closed(&mut self, tab_id: i32, _now_ms: u64) {
        self.badge_counts.remove(&tab_id);
        for record in &mut self.matched {
            if record.tab_id == tab_id {
                record.tab_id = -1;
                record.active_document = false;
            }
        }
    }

    /// Current DNR-owned badge value to submit through the native action model.
    pub fn badge_text(&self, tab_id: i32) -> Option<String> {
        self.display_action_count_as_badge_text.then(|| {
            self.badge_counts
                .get(&tab_id)
                .copied()
                .unwrap_or_default()
                .to_string()
        })
    }

    /// Queries retained feedback with the per-install quota. Future timestamps do not
    /// bypass quota, and discarded records never retain their request URLs.
    pub fn matched_rules(
        &mut self,
        tab_id: Option<i32>,
        min_time_stamp: Option<f64>,
        now_ms: u64,
        bypass_quota: bool,
    ) -> Result<Value, ApiError> {
        self.feedback_queries
            .retain(|time| now_ms.saturating_sub(*time) < GET_MATCHED_RULES_INTERVAL_MS);
        if !bypass_quota {
            if self.feedback_queries.len() >= MAX_GET_MATCHED_RULES_CALLS {
                return Err(ApiError::other(
                    "This request exceeds the MAX_GETMATCHEDRULES_CALLS_PER_INTERVAL quota.",
                ));
            }
            self.feedback_queries.push_back(now_ms);
        }
        self.matched.retain(|record| {
            record.active_document
                || (now_ms as f64 - record.time_stamp) < MATCHED_RULE_RETENTION_MS as f64
        });
        Ok(
            json!({"rulesMatchedInfo": self.matched.iter().filter(|record| tab_id.is_none_or(|tab| record.tab_id == tab)
            && min_time_stamp.is_none_or(|time| record.time_stamp > time)).map(|record| json!({
                "rule":{"ruleId":record.rule_id,"rulesetId":record.ruleset_id},"tabId":record.tab_id,"timeStamp":record.time_stamp
            })).collect::<Vec<_>>() }),
        )
    }

    /// Simulates effective terminal/header rules on the compilation executor. Response
    /// header conditions are supported here, independently of Linux enforcement limits.
    pub fn test_match_outcome(
        &self,
        details: &Value,
        permissions: &InstallPermissions,
        extension_id: &str,
    ) -> Result<Value, ApiError> {
        let url = details
            .get("url")
            .and_then(Value::as_str)
            .and_then(|url| Url::parse(url).ok())
            .ok_or_else(|| ApiError::other("Invalid request URL."))?;
        let initiator = details
            .get("initiator")
            .and_then(Value::as_str)
            .and_then(|url| Url::parse(url).ok());
        let top = details
            .get("topUrl")
            .and_then(Value::as_str)
            .and_then(|url| Url::parse(url).ok());
        let resource_type: dnr::ResourceType = serde_json::from_value(details["type"].clone())
            .map_err(|_| ApiError::malformed_arguments())?;
        let mut request = dnr::Request::new(
            url.as_str(),
            url.host_str().unwrap_or_default(),
            resource_type,
        );
        request.initiator_host = initiator.as_ref().and_then(Url::host_str);
        request.top_host = top.as_ref().and_then(Url::host_str);
        request.tab_id = details
            .get("tabId")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok())
            .unwrap_or(-1);
        request.method = details
            .get("method")
            .map(|method| serde_json::from_value(method.clone()))
            .transpose()
            .map_err(|_| ApiError::malformed_arguments())?
            .unwrap_or(dnr::RequestMethod::Get);
        request.domain_type = dnr::domain_type(request.host, request.initiator_host);
        let headers: Vec<dnr::Header<'_>> = details
            .get("responseHeaders")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .flat_map(|(name, values)| {
                values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(move |value| {
                        value.as_str().map(|value| dnr::Header { name, value })
                    })
            })
            .collect();
        if details.get("responseHeaders").is_some() {
            request.stage = dnr::Stage::ResponseHeaders;
            request.response_headers = &headers;
        }
        let mut sets: Vec<(String, Vec<dnr::Rule>)> = self
            .enabled_static_rules()
            .into_iter()
            .map(|(id, rules)| (id.to_owned(), rules))
            .collect();
        sets.push((
            dnr::DYNAMIC_RULESET_ID.to_owned(),
            self.rules(dnr::RulesetKind::Dynamic)?,
        ));
        sets.push((
            dnr::SESSION_RULESET_ID.to_owned(),
            self.rules(dnr::RulesetKind::Session)?,
        ));
        let prepared: Vec<_> = sets
            .into_iter()
            .map(|(id, rules)| {
                let rules = rules
                    .into_iter()
                    .filter(|rule| {
                        let needs_host =
                            !permissions.has_api("declarativeNetRequest") || !rule.action.is_safe();
                        let navigation = matches!(
                            resource_type,
                            dnr::ResourceType::MainFrame | dnr::ResourceType::SubFrame
                        );
                        !needs_host
                            || (permissions.has_host_access(&url)
                                && (navigation
                                    || initiator
                                        .as_ref()
                                        .is_none_or(|url| permissions.has_host_access(url))))
                    })
                    .map(dnr::PreparedRule::new)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(ApiError::other)?;
                Ok((id, rules))
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        let decision = dnr::evaluate_candidates(
            &request,
            prepared.iter().flat_map(|(id, rules)| {
                rules.iter().map(move |rule| dnr::Candidate {
                    extension_id,
                    install_order: 0,
                    ruleset_id: id,
                    rule,
                })
            }),
        );
        Ok(
            json!({"matchedRules":decision.matched_rules.iter().map(|rule| json!({"ruleId":rule.rule_id,"rulesetId":rule.ruleset_id})).collect::<Vec<_>>() }),
        )
    }
}

fn rule_id(value: &Value) -> Option<u32> {
    value
        .get("id")?
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
}

fn parse_rules(rules: &[Value], kind: dnr::RulesetKind) -> Result<Vec<dnr::Rule>, ApiError> {
    let mut seen = BTreeSet::new();
    for rule in rules {
        if let Some(id) = rule_id(rule) {
            if !seen.insert(id) {
                return Err(ApiError::other(format!(
                    "Rule with id {id} does not have a unique ID."
                )));
            }
        }
    }
    let bytes = serde_json::to_vec(rules).map_err(|_| ApiError::malformed_arguments())?;
    dnr::parse_rules_json(&bytes, kind)
        .map(|set| set.rules)
        .map_err(|error| match error {
            dnr::ParseError::Rejected(rejected) => rejected
                .first()
                .map(|rejection| rule_rejection(rejection, kind))
                .unwrap_or_else(ApiError::malformed_arguments),
            _ => ApiError::other(error.to_string()),
        })
}

fn rule_rejection(rejection: &dnr::RuleRejection, kind: dnr::RulesetKind) -> ApiError {
    let id = rejection.id.unwrap_or(0);
    let reason = rejection.reason.as_str();
    let message = match reason {
        "MAX_NUMBER_OF_DYNAMIC_RULES exceeded" => "Dynamic rule count exceeded.".to_owned(),
        "MAX_NUMBER_OF_UNSAFE_DYNAMIC_RULES exceeded" => "Dynamic unsafe rule count exceeded.".to_owned(),
        "MAX_NUMBER_OF_SESSION_RULES exceeded" => "Session rule count exceeded.".to_owned(),
        "MAX_NUMBER_OF_REGEX_RULES exceeded" => if kind == dnr::RulesetKind::Session { "Session rule count for regex rules exceeded." } else { "Dynamic rule count for regex rules exceeded." }.to_owned(),
        "id must be an integer from 1 to 2147483647" => format!("Rule with id {id} has an invalid value for id key. This should be greater than or equal to 1."),
        "priority must be an integer from 1 to 2147483647" => format!("Rule with id {id} has an invalid value for priority key. This should be greater than or equal to 1."),
        "tabIds and excludedTabIds are only valid for session rules" => format!("Rule with id {id} specifies a value for \"tabIds\" or \"excludedTabIds\" key. These are only supported for session-scoped rules."),
        "a tab ID cannot be both included and excluded" => format!("Rule with id {id} includes and excludes the same tab ID."),
        "urlFilter and regexFilter cannot both be specified" => format!("Rule with id {id} can only specify one of \"urlFilter\" or \"regexFilter\" keys."),
        "invalid redirect URL" => format!("Rule with id {id} does not provide a valid URL for redirect.url key."),
        "invalid header modification name" => format!("Rule with id {id} must specify a valid header name to be modified."),
        "invalid header modification value" => format!("Rule with id {id} must provide a valid header value to be appended/set."),
        "append/set header operation requires value" => format!("Rule with id {id} must provide a value for a header to be appended/set."),
        "remove header operation must not specify value" => format!("Rule with id {id} must not provide a header value for a header to be removed."),
        "response-header conditions cannot modify request headers" => format!("Rule with id {id} which matches on response headers cannot modify request headers."),
        _ => format!("Rule with id {id} is invalid: {reason}."),
    };
    ApiError::other(message)
}

/// Installs DNR contracts from the generated schema and permission metadata.
pub fn register<S: DnrHost + BrowserPort + 'static>(
    table: &mut ApiTable<S>,
) -> Result<(), RegistrationError> {
    for &name in METHODS {
        table.register_generated(name, |host: &mut S, call| match dispatch(host, &call) {
            Ok(Dispatch::Reply(value)) => call.reply(Ok(value)),
            Ok(Dispatch::Request(request)) => host.dnr_request(request, call),
            Err(error) => call.reply(Err(error)),
        })?;
    }
    Ok(())
}

enum Dispatch {
    Reply(Value),
    Request(Request),
}

fn dispatch(
    host: &(impl DnrHost + BrowserPort),
    call: &Invocation<'_>,
) -> Result<Dispatch, ApiError> {
    let options = call.args().first().unwrap_or(&Value::Null);
    let state = host.dnr_state(call.sender())?;
    let request = match call.name() {
        "declarativeNetRequest.getDynamicRules" | "declarativeNetRequest.getSessionRules" => {
            let rules = if call.name().ends_with("getDynamicRules") {
                &state.dynamic_rules
            } else {
                &state.session_rules
            };
            let ids = options.get("ruleIds").and_then(Value::as_array);
            return Ok(Dispatch::Reply(Value::Array(
                rules
                    .iter()
                    .filter(|rule| ids.is_none_or(|ids| ids.iter().any(|id| id == &rule["id"])))
                    .cloned()
                    .collect(),
            )));
        }
        "declarativeNetRequest.getEnabledRulesets" => {
            return Ok(Dispatch::Reply(json!(state
                .static_rulesets
                .iter()
                .filter(|set| set.enabled)
                .map(|set| &set.id)
                .collect::<Vec<_>>())))
        }
        "declarativeNetRequest.getDisabledRuleIds" => {
            let id = options["rulesetId"]
                .as_str()
                .ok_or_else(ApiError::malformed_arguments)?;
            state.static_ruleset(id)?;
            return Ok(Dispatch::Reply(json!(state
                .disabled_static_rules
                .get(id)
                .cloned()
                .unwrap_or_default())));
        }
        "declarativeNetRequest.getAvailableStaticRuleCount" => {
            return Ok(Dispatch::Reply(json!(state
                .static_rule_limit
                .saturating_sub(state.enabled_static_rule_count()))))
        }
        "declarativeNetRequest.isRegexSupported" => {
            return Ok(Dispatch::Reply(regex_support(options)))
        }
        "declarativeNetRequest.updateDynamicRules" | "declarativeNetRequest.updateSessionRules" => {
            Request::UpdateRules {
                kind: if call.name().ends_with("updateDynamicRules") {
                    dnr::RulesetKind::Dynamic
                } else {
                    dnr::RulesetKind::Session
                },
                remove_rule_ids: ids(options, "removeRuleIds")?,
                add_rules: options
                    .get("addRules")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            }
        }
        "declarativeNetRequest.updateEnabledRulesets" => Request::UpdateEnabledRulesets {
            enable: strings(options, "enableRulesetIds")?,
            disable: strings(options, "disableRulesetIds")?,
        },
        "declarativeNetRequest.updateStaticRules" => Request::UpdateStaticRules {
            ruleset_id: options["rulesetId"]
                .as_str()
                .ok_or_else(ApiError::malformed_arguments)?
                .to_owned(),
            enable: ids(options, "enableRuleIds")?,
            disable: ids(options, "disableRuleIds")?,
        },
        "declarativeNetRequest.setExtensionActionOptions" => {
            let tab_update = options
                .get("tabUpdate")
                .map(|update| {
                    let tab = integer(update, "tabId")?;
                    let increment = integer(update, "increment")?;
                    browser::snapshot(host, call.sender())?.tab(tab)?;
                    Ok::<_, ApiError>((tab, increment))
                })
                .transpose()?;
            Request::SetExtensionActionOptions {
                display_action_count_as_badge_text: options
                    .get("displayActionCountAsBadgeText")
                    .and_then(Value::as_bool),
                tab_update,
            }
        }
        "declarativeNetRequest.getMatchedRules" => {
            let tab_id = options
                .get("tabId")
                .map(|_| integer(options, "tabId"))
                .transpose()?;
            if !call.permissions().has_api("declarativeNetRequestFeedback") {
                let allowed = tab_id
                    .and_then(|tab_id| {
                        let snapshot = browser::snapshot(host, call.sender()).ok()?;
                        let tab = snapshot.tab(tab_id).ok()?;
                        let url = tab.url().ok()?;
                        Some(call.permissions().has_active_tab_grant(
                            tab.tab.id(),
                            tab.document,
                            &url,
                        ))
                    })
                    .unwrap_or(false);
                if !allowed {
                    return Err(ApiError::other(FEEDBACK_PERMISSION_ERROR));
                }
            }
            Request::GetMatchedRules {
                tab_id,
                min_time_stamp: options.get("minTimeStamp").and_then(Value::as_f64),
                now_ms: host.now_ms(),
                bypass_quota: host.user_gesture(call.sender()),
            }
        }
        "declarativeNetRequest.testMatchOutcome" => {
            if !call.sender().unpacked() {
                return Err(ApiError::not_available_in_context(call.name()));
            }
            Request::TestMatchOutcome {
                request: options.clone(),
                permissions: Box::new(call.permissions().clone()),
                extension_id: call.sender().extension_id().as_str().to_owned(),
            }
        }
        _ => return Err(ApiError::unknown_method()),
    };
    Ok(Dispatch::Request(request))
}

fn integer(value: &Value, key: &str) -> Result<i32, ApiError> {
    value[key]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .ok_or_else(ApiError::malformed_arguments)
}

fn ids(options: &Value, key: &str) -> Result<Vec<u32>, ApiError> {
    options
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|id| id.as_i64().ok_or_else(ApiError::malformed_arguments))
        .collect::<Result<Vec<_>, _>>()
        .map(|ids| {
            ids.into_iter()
                .filter_map(|id| u32::try_from(id).ok())
                .collect()
        })
}

fn strings(options: &Value, key: &str) -> Result<Vec<String>, ApiError> {
    options
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|id| {
            id.as_str()
                .map(str::to_owned)
                .ok_or_else(ApiError::malformed_arguments)
        })
        .collect()
}

/// Uses the same ASCII/RE2 grammar and implementation budget as installed rules.
pub fn regex_support(options: &Value) -> Value {
    let pattern = options
        .get("regex")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sensitive = options
        .get("isCaseSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    match dnr::compile_regex(pattern, sensitive) {
        Ok(_) => json!({"isSupported": true}),
        Err(error) => {
            json!({"isSupported": false, "reason": if error.contains("limit") { "memoryLimitExceeded" } else { "syntaxError" }})
        }
    }
}

#[cfg(test)]
mod tests;
