//! Pure fixed-catalog provider pricing contracts.
//!
//! The types in this module perform no lookup, network access, persistence, or
//! account inspection. A trusted product catalog supplies one immutable
//! schedule whose provider, model, tokenizer, billing class, price revision,
//! and applicable input range must exactly match the committed call. Pricing
//! uses checked integer arithmetic and returns an opaque value for policy
//! settlement; callers cannot hand policy an unbound raw cost.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    AgentProviderBillingClass, AgentProviderCallConfig, AgentProviderInputAccountingMode,
    AgentProviderKind, AgentProviderModelRevision, AgentProviderReasoningEffort,
    AgentProviderResponseIdentity, AgentProviderResponseRoute, AgentProviderUsage,
    MAX_AGENT_PROVIDER_ALLOWED_EFFECTIVE_MODELS,
};
use crate::{SemanticTokenizerRevision, MAX_AGENT_RUN_COST_MICRO_USD, MAX_AGENT_RUN_MODEL_TOKENS};

const TOKENS_PER_RATE_UNIT: u128 = 1_000_000;

/// Highest standard-rate input range supported by provider-exact reservation.
///
/// Long-context pricing needs a separate representable schedule tier before a
/// catalog entry may raise this bound.
pub const MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS: u64 = 272_000;

/// Hard ceiling for one token-class rate in micro-USD per million tokens.
///
/// At this rate a single token consumes the complete hard per-run cost ceiling,
/// so a higher rate cannot produce an admissible Zephium call.
pub const MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS: u64 =
    MAX_AGENT_RUN_COST_MICRO_USD * 1_000_000;

/// Nonzero revision of one immutable trusted provider-price catalog entry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AgentProviderPricingRevision(NonZeroU64);

impl AgentProviderPricingRevision {
    /// Constructs a nonzero catalog revision.
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// Numeric catalog revision for content-free accounting records.
    pub const fn value(self) -> u64 {
        self.0.get()
    }
}

/// Pricing identity and applicable input-token range bound before disclosure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderPricingProfile {
    revision: AgentProviderPricingRevision,
    min_input_tokens: u64,
    max_input_tokens: u64,
}

impl AgentProviderPricingProfile {
    /// Binds a nonzero catalog revision to a bounded inclusive input range.
    pub const fn try_new(
        revision: AgentProviderPricingRevision,
        max_input_tokens: u64,
    ) -> Result<Self, AgentProviderPricingContractError> {
        Self::try_for_input_range(revision, 1, max_input_tokens)
    }

    /// Binds a nonzero catalog revision to an inclusive pricing input range.
    pub const fn try_for_input_range(
        revision: AgentProviderPricingRevision,
        min_input_tokens: u64,
        max_input_tokens: u64,
    ) -> Result<Self, AgentProviderPricingContractError> {
        if min_input_tokens == 0
            || min_input_tokens > max_input_tokens
            || max_input_tokens > MAX_AGENT_RUN_MODEL_TOKENS
        {
            return Err(AgentProviderPricingContractError::InputRange);
        }
        Ok(Self {
            revision,
            min_input_tokens,
            max_input_tokens,
        })
    }

    /// Exact immutable trusted catalog revision.
    pub const fn revision(self) -> AgentProviderPricingRevision {
        self.revision
    }

    /// Inclusive provider-accounted input-token floor for these rates.
    pub const fn min_input_tokens(self) -> u64 {
        self.min_input_tokens
    }

    /// Inclusive provider-accounted input-token ceiling for these rates.
    pub const fn max_input_tokens(self) -> u64 {
        self.max_input_tokens
    }

    pub(super) const fn contains(self, input_tokens: u64) -> bool {
        input_tokens >= self.min_input_tokens && input_tokens <= self.max_input_tokens
    }
}

/// Fixed token rates for the four disjoint billable categories.
///
/// Rates are micro-USD per one million tokens. Output is the inclusive provider
/// total; reasoning tokens are never added again. Cached reads and cache writes
/// are disjoint input subsets; uncached input is the checked remainder. A
/// catalog entry must use the conservative compatible cache-write rate when a
/// provider collapses multiple cache-write classes into one usage counter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AgentProviderTokenRates {
    uncached_input: u64,
    cached_input: u64,
    cache_write_input: u64,
    output: u64,
}

impl AgentProviderTokenRates {
    /// Explicit zero-cost schedule for a resolved self-hosted model.
    #[cfg(feature = "provider-transport")]
    pub(crate) const fn free() -> Self {
        Self {
            uncached_input: 0,
            cached_input: 0,
            cache_write_input: 0,
            output: 0,
        }
    }

    /// Validates nonzero rates under the hard product arithmetic ceiling.
    pub const fn try_new(
        uncached_input: u64,
        cached_input: u64,
        cache_write_input: u64,
        output: u64,
    ) -> Result<Self, AgentProviderPricingContractError> {
        if uncached_input == 0
            || cached_input == 0
            || cache_write_input == 0
            || output == 0
            || uncached_input > MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS
            || cached_input > MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS
            || cache_write_input > MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS
            || output > MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS
        {
            return Err(AgentProviderPricingContractError::Rate);
        }
        Ok(Self {
            uncached_input,
            cached_input,
            cache_write_input,
            output,
        })
    }

    /// Uncached-input rate in micro-USD per million tokens.
    pub const fn uncached_input(self) -> u64 {
        self.uncached_input
    }

    /// Cached-read rate in micro-USD per million tokens.
    pub const fn cached_input(self) -> u64 {
        self.cached_input
    }

    /// Cache-write rate in micro-USD per million tokens.
    pub const fn cache_write_input(self) -> u64 {
        self.cache_write_input
    }

    /// Inclusive-output rate in micro-USD per million tokens.
    pub const fn output(self) -> u64 {
        self.output
    }
}

#[derive(Eq, PartialEq)]
pub(super) struct AgentProviderCatalogBinding {
    provider: AgentProviderKind,
    requested_model: AgentProviderModelRevision,
    allowed_effective_models: Vec<AgentProviderModelRevision>,
    response_route: AgentProviderResponseRoute,
    reasoning_effort: AgentProviderReasoningEffort,
    tokenizer: SemanticTokenizerRevision,
    profile: AgentProviderPricingProfile,
    rates: AgentProviderTokenRates,
    guard: [u8; 32],
}

impl AgentProviderCatalogBinding {
    pub(super) const fn provider(&self) -> AgentProviderKind {
        self.provider
    }

    pub(super) const fn requested_model(&self) -> &AgentProviderModelRevision {
        &self.requested_model
    }

    pub(super) const fn response_route(&self) -> AgentProviderResponseRoute {
        self.response_route
    }

    pub(super) const fn reasoning_effort(&self) -> AgentProviderReasoningEffort {
        self.reasoning_effort
    }

    pub(super) const fn tokenizer(&self) -> &SemanticTokenizerRevision {
        &self.tokenizer
    }

    pub(super) const fn pricing_profile(&self) -> AgentProviderPricingProfile {
        self.profile
    }

    pub(super) const fn rates(&self) -> AgentProviderTokenRates {
        self.rates
    }

    pub(super) const fn accounting_guard(&self) -> [u8; 32] {
        self.guard
    }

    pub(super) fn allows_response_identity(
        &self,
        effective_model: &str,
        service_tier: &str,
        inference_geo: Option<&str>,
    ) -> bool {
        self.response_route.response_service_tier() == service_tier
            && self.response_route.response_inference_geo() == inference_geo
            && self
                .allowed_effective_models
                .binary_search_by(|model| model.as_str().cmp(effective_model))
                .is_ok()
    }

    fn matches_response_identity(&self, identity: AgentProviderResponseIdentity) -> bool {
        identity.provider() == self.provider
            && identity.requested_model() == self.requested_model.as_str()
            && identity.response_route() == self.response_route
            && identity.reasoning_effort() == self.reasoning_effort
            && self.allows_response_identity(
                identity.effective_model(),
                identity.effective_service_tier(),
                identity.effective_inference_geo(),
            )
    }
}

impl fmt::Debug for AgentProviderCatalogBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderCatalogBinding")
            .field("provider", &self.provider)
            .field("requested_model", &self.requested_model)
            .field(
                "allowed_effective_models",
                &self.allowed_effective_models.len(),
            )
            .field("response_route", &self.response_route)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("tokenizer", &self.tokenizer)
            .field("profile", &self.profile)
            .field("guard", &"[redacted]")
            .finish()
    }
}

/// One immutable trusted schedule entry; no production entries are implicit.
#[must_use]
pub struct AgentProviderPricingSchedule {
    binding: Arc<AgentProviderCatalogBinding>,
}

impl AgentProviderPricingSchedule {
    /// Constructs an immutable entry selected by the trusted product catalog.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        provider: AgentProviderKind,
        requested_model: AgentProviderModelRevision,
        mut allowed_effective_models: Vec<AgentProviderModelRevision>,
        response_route: AgentProviderResponseRoute,
        reasoning_effort: AgentProviderReasoningEffort,
        tokenizer: SemanticTokenizerRevision,
        profile: AgentProviderPricingProfile,
        rates: AgentProviderTokenRates,
    ) -> Result<Self, AgentProviderPricingContractError> {
        if allowed_effective_models.is_empty()
            || allowed_effective_models.len() > MAX_AGENT_PROVIDER_ALLOWED_EFFECTIVE_MODELS
            || response_route.provider() != provider
            || (provider == AgentProviderKind::AnthropicMessages
                && reasoning_effort != AgentProviderReasoningEffort::None)
        {
            return Err(AgentProviderPricingContractError::CatalogIdentity);
        }
        allowed_effective_models.sort();
        if allowed_effective_models
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(AgentProviderPricingContractError::CatalogIdentity);
        }
        let mut binding = AgentProviderCatalogBinding {
            provider,
            requested_model,
            allowed_effective_models,
            response_route,
            reasoning_effort,
            tokenizer,
            profile,
            rates,
            guard: [0; 32],
        };
        binding.guard = schedule_guard(&binding);
        Ok(Self {
            binding: Arc::new(binding),
        })
    }

    #[cfg(test)]
    pub(crate) fn try_for_test(
        provider: AgentProviderKind,
        model: AgentProviderModelRevision,
        reasoning_effort: AgentProviderReasoningEffort,
        tokenizer: SemanticTokenizerRevision,
        profile: AgentProviderPricingProfile,
        rates: AgentProviderTokenRates,
    ) -> Result<Self, AgentProviderPricingContractError> {
        let response_route = match provider {
            AgentProviderKind::OpenAiResponses => AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderKind::AnthropicMessages => {
                AgentProviderResponseRoute::AnthropicStandardGlobal
            }
        };
        Self::try_new(
            provider,
            model.clone(),
            vec![model],
            response_route,
            reasoning_effort,
            tokenizer,
            profile,
            rates,
        )
    }

    /// Derives one per-call response budget from this exact trusted entry.
    pub fn try_call_config(
        &self,
        fixed_input_tokens: u32,
        max_output_tokens: u32,
        stream: super::AgentProviderStreamBudget,
    ) -> Result<AgentProviderCallConfig, super::AgentProviderContractError> {
        AgentProviderCallConfig::from_catalog(
            Arc::clone(&self.binding),
            AgentProviderInputAccountingMode::ExactLocal { fixed_input_tokens },
            max_output_tokens,
            stream,
        )
    }

    /// Derives an OpenAI call that must be provider-counted before generation.
    ///
    /// The trusted schedule must remain entirely within the standard pricing
    /// range. No placeholder local token count is manufactured.
    pub fn try_provider_exact_call_config(
        &self,
        max_output_tokens: u32,
        stream: super::AgentProviderStreamBudget,
    ) -> Result<AgentProviderCallConfig, super::AgentProviderContractError> {
        if self.provider() != AgentProviderKind::OpenAiResponses
            || self.response_route() != AgentProviderResponseRoute::OpenAiDefault
            || self.profile().max_input_tokens() > MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS
        {
            return Err(super::AgentProviderContractError::InputAccountingMode);
        }
        AgentProviderCallConfig::from_catalog(
            Arc::clone(&self.binding),
            AgentProviderInputAccountingMode::ProviderExactAfterConservativeReservation,
            max_output_tokens,
            stream,
        )
    }

    /// Derives a call for the person's chosen model, settled on the usage its
    /// provider reports after a conservative whole-request reservation.
    pub fn try_provider_reported_call_config(
        &self,
        max_output_tokens: u32,
        stream: super::AgentProviderStreamBudget,
    ) -> Result<AgentProviderCallConfig, super::AgentProviderContractError> {
        AgentProviderCallConfig::from_catalog(
            Arc::clone(&self.binding),
            AgentProviderInputAccountingMode::ProviderReportedAfterConservativeReservation,
            max_output_tokens,
            stream,
        )
    }

    /// Requested provider model alias or snapshot bound by this entry.
    pub fn requested_model(&self) -> &AgentProviderModelRevision {
        self.binding.requested_model()
    }

    /// Closed response route bound by this entry.
    pub fn response_route(&self) -> AgentProviderResponseRoute {
        self.binding.response_route()
    }

    /// Explicit reasoning effort bound by this entry.
    pub fn reasoning_effort(&self) -> AgentProviderReasoningEffort {
        self.binding.reasoning_effort()
    }

    /// Exact tokenizer revision bound by this entry.
    pub fn tokenizer(&self) -> &SemanticTokenizerRevision {
        self.binding.tokenizer()
    }

    /// Exact provider protocol bound by this entry.
    pub fn provider(&self) -> AgentProviderKind {
        self.binding.provider()
    }

    /// Number of catalog-approved effective provider revisions.
    pub fn allowed_effective_models(&self) -> usize {
        self.binding.allowed_effective_models.len()
    }

    /// Whether this schedule contains the exact effective revision.
    pub fn allows_effective_model(&self, model: &str) -> bool {
        self.binding
            .allowed_effective_models
            .binary_search_by(|candidate| candidate.as_str().cmp(model))
            .is_ok()
    }

    pub(super) fn exact_binding_matches(&self, config: &AgentProviderCallConfig) -> bool {
        config.accounting_guard() == self.binding.guard
            && config.catalog.as_ref() == self.binding.as_ref()
    }

    pub(super) fn try_price(
        &self,
        config: &AgentProviderCallConfig,
        identity: AgentProviderResponseIdentity,
        usage: AgentProviderUsage,
    ) -> Result<AgentProviderPricedUsage, AgentProviderPricingError> {
        if !self.exact_binding_matches(config) || !self.binding.matches_response_identity(identity)
        {
            return Err(AgentProviderPricingError::Identity);
        }
        if !self.binding.profile.contains(usage.input_tokens()) {
            return Err(AgentProviderPricingError::InputRange);
        }

        let priced_input_subsets = usage
            .cached_input_tokens()
            .checked_add(usage.cache_write_input_tokens())
            .ok_or(AgentProviderPricingError::Arithmetic)?;
        let uncached_input = usage
            .input_tokens()
            .checked_sub(priced_input_subsets)
            .ok_or(AgentProviderPricingError::Arithmetic)?;
        let rates = self.binding.rates();
        let mut numerator = price_component(uncached_input, rates.uncached_input)?;
        numerator =
            add_price_component(numerator, usage.cached_input_tokens(), rates.cached_input)?;
        numerator = add_price_component(
            numerator,
            usage.cache_write_input_tokens(),
            rates.cache_write_input,
        )?;
        numerator = add_price_component(numerator, usage.output_tokens(), rates.output)?;
        let whole = numerator / TOKENS_PER_RATE_UNIT;
        let rounded = whole
            .checked_add(u128::from(numerator % TOKENS_PER_RATE_UNIT != 0))
            .ok_or(AgentProviderPricingError::Arithmetic)?;
        let cost_ceiling_micro_usd =
            u64::try_from(rounded).map_err(|_| AgentProviderPricingError::Arithmetic)?;

        Ok(AgentProviderPricedUsage {
            config: config.clone(),
            usage,
            pricing_revision: self.binding.profile.revision,
            schedule_guard: self.binding.guard,
            response_identity_guard: identity.guard(),
            cost_ceiling_micro_usd,
        })
    }

    #[cfg(test)]
    pub(crate) fn try_price_for_test(
        &self,
        config: &AgentProviderCallConfig,
        identity: AgentProviderResponseIdentity,
        usage: AgentProviderUsage,
    ) -> Result<AgentProviderPricedUsage, AgentProviderPricingError> {
        self.try_price(config, identity, usage)
    }

    /// Exact schedule pricing identity without exposing mutable catalog input.
    pub fn profile(&self) -> AgentProviderPricingProfile {
        self.binding.profile
    }

    /// Content-free digest of provider, billing, model, tokenizer, profile, and rates.
    pub fn accounting_guard(&self) -> [u8; 32] {
        self.binding.guard
    }
}

impl fmt::Debug for AgentProviderPricingSchedule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderPricingSchedule")
            .field("binding", &self.binding)
            .finish()
    }
}

fn schedule_guard(binding: &AgentProviderCatalogBinding) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"ZEPHIUM-AGENT-PROVIDER-PRICING-SCHEDULE-2\0");
    hasher.update([match binding.provider {
        AgentProviderKind::OpenAiResponses => 1,
        AgentProviderKind::AnthropicMessages => 2,
    }]);
    hasher.update([match binding.response_route.billing_class() {
        AgentProviderBillingClass::OpenAiDefault => 1,
        AgentProviderBillingClass::AnthropicStandardGlobal => 2,
        AgentProviderBillingClass::ModelReported => 3,
    }]);
    hasher.update([response_route_guard_byte(binding.response_route)]);
    hasher.update([reasoning_effort_guard_byte(binding.reasoning_effort)]);
    hasher.update([binding.requested_model.as_str().len() as u8]);
    hasher.update(binding.requested_model.as_str().as_bytes());
    hasher.update([binding.allowed_effective_models.len() as u8]);
    for model in &binding.allowed_effective_models {
        hasher.update([model.as_str().len() as u8]);
        hasher.update(model.as_str().as_bytes());
    }
    hasher.update([0]);
    hasher.update(binding.tokenizer.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(binding.profile.revision().value().to_be_bytes());
    hasher.update(binding.profile.min_input_tokens().to_be_bytes());
    hasher.update(binding.profile.max_input_tokens().to_be_bytes());
    hasher.update(binding.rates.uncached_input().to_be_bytes());
    hasher.update(binding.rates.cached_input().to_be_bytes());
    hasher.update(binding.rates.cache_write_input().to_be_bytes());
    hasher.update(binding.rates.output().to_be_bytes());
    hasher.finalize().into()
}

const fn response_route_guard_byte(route: AgentProviderResponseRoute) -> u8 {
    match route {
        AgentProviderResponseRoute::OpenAiDefault => 1,
        AgentProviderResponseRoute::AnthropicStandardGlobal => 2,
        AgentProviderResponseRoute::ModelReported => 3,
    }
}

const fn reasoning_effort_guard_byte(effort: AgentProviderReasoningEffort) -> u8 {
    match effort {
        AgentProviderReasoningEffort::None => 1,
        AgentProviderReasoningEffort::Low => 2,
        AgentProviderReasoningEffort::Medium => 3,
        AgentProviderReasoningEffort::High => 4,
        AgentProviderReasoningEffort::XHigh => 5,
        AgentProviderReasoningEffort::Max => 6,
    }
}

fn price_component(tokens: u64, rate: u64) -> Result<u128, AgentProviderPricingError> {
    u128::from(tokens)
        .checked_mul(u128::from(rate))
        .ok_or(AgentProviderPricingError::Arithmetic)
}

fn add_price_component(
    current: u128,
    tokens: u64,
    rate: u64,
) -> Result<u128, AgentProviderPricingError> {
    current
        .checked_add(price_component(tokens, rate)?)
        .ok_or(AgentProviderPricingError::Arithmetic)
}

/// Opaque checked price bound to exact provider configuration and usage.
#[must_use]
pub(crate) struct AgentProviderPricedUsage {
    config: AgentProviderCallConfig,
    usage: AgentProviderUsage,
    pricing_revision: AgentProviderPricingRevision,
    schedule_guard: [u8; 32],
    response_identity_guard: [u8; 32],
    cost_ceiling_micro_usd: u64,
}

impl AgentProviderPricedUsage {
    /// Exact normalized provider usage priced by the matching schedule.
    #[cfg(test)]
    pub const fn usage(&self) -> AgentProviderUsage {
        self.usage
    }

    /// Exact immutable schedule revision used for this ceiling.
    #[cfg(test)]
    pub const fn pricing_revision(&self) -> AgentProviderPricingRevision {
        self.pricing_revision
    }

    /// Content-free digest of the exact matching pricing schedule.
    #[cfg(test)]
    pub const fn schedule_guard(&self) -> [u8; 32] {
        self.schedule_guard
    }

    /// Content-free digest of the exact effective response identity.
    #[cfg(test)]
    pub const fn response_identity_guard(&self) -> [u8; 32] {
        self.response_identity_guard
    }

    /// Conservative checked catalog cost in micro-USD.
    #[cfg(test)]
    pub const fn cost_ceiling_micro_usd(&self) -> u64 {
        self.cost_ceiling_micro_usd
    }

    pub(crate) fn into_policy_parts(
        self,
    ) -> (AgentProviderUsage, u64, AgentProviderPricingAttribution) {
        let attribution = AgentProviderPricingAttribution {
            provider: self.config.provider(),
            billing_class: self.config.billing_class(),
            pricing_revision: self.pricing_revision,
            schedule_guard: self.schedule_guard,
            response_identity_guard: self.response_identity_guard,
            cached_input_tokens: self.usage.cached_input_tokens(),
            cache_write_input_tokens: self.usage.cache_write_input_tokens(),
            reasoning_output_tokens: self.usage.reasoning_output_tokens(),
        };
        (self.usage, self.cost_ceiling_micro_usd, attribution)
    }
}

impl fmt::Debug for AgentProviderPricedUsage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderPricedUsage")
            .field("config", &self.config)
            .field("usage", &self.usage)
            .field("pricing_revision", &self.pricing_revision)
            .field("schedule_guard", &"[redacted]")
            .field("response_identity_guard", &"[redacted]")
            .field("cost_ceiling_micro_usd", &self.cost_ceiling_micro_usd)
            .finish()
    }
}

/// Content-free attribution retained for one checked catalog-priced receipt.
///
/// This value cannot price or settle anything. It preserves the exact provider
/// billing class, catalog revision, schedule digest, and normalized usage
/// subsets needed for reproducible aggregate accounting without retaining a
/// model label, tokenizer label, rate table, response, prompt, or credential.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct AgentProviderPricingAttribution {
    provider: AgentProviderKind,
    billing_class: AgentProviderBillingClass,
    pricing_revision: AgentProviderPricingRevision,
    schedule_guard: [u8; 32],
    response_identity_guard: [u8; 32],
    cached_input_tokens: u64,
    cache_write_input_tokens: u64,
    reasoning_output_tokens: u64,
}

impl AgentProviderPricingAttribution {
    /// Exact provider protocol whose terminal usage was priced.
    pub const fn provider(self) -> AgentProviderKind {
        self.provider
    }

    /// Exact provider-attested billing class used for pricing.
    pub const fn billing_class(self) -> AgentProviderBillingClass {
        self.billing_class
    }

    /// Exact immutable trusted catalog revision.
    pub const fn pricing_revision(self) -> AgentProviderPricingRevision {
        self.pricing_revision
    }

    /// Digest of the exact provider/model/tokenizer/profile/rate schedule.
    pub const fn schedule_guard(self) -> [u8; 32] {
        self.schedule_guard
    }

    /// Digest of the exact provider-attested effective response identity.
    pub const fn response_identity_guard(self) -> [u8; 32] {
        self.response_identity_guard
    }

    /// Cached-read input subset normalized from provider terminal usage.
    pub const fn cached_input_tokens(self) -> u64 {
        self.cached_input_tokens
    }

    /// Cache-write input subset normalized from provider terminal usage.
    pub const fn cache_write_input_tokens(self) -> u64 {
        self.cache_write_input_tokens
    }

    /// Reasoning-token subset of inclusive provider output usage.
    pub const fn reasoning_output_tokens(self) -> u64 {
        self.reasoning_output_tokens
    }
}

impl fmt::Debug for AgentProviderPricingAttribution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderPricingAttribution")
            .field("provider", &self.provider)
            .field("billing_class", &self.billing_class)
            .field("pricing_revision", &self.pricing_revision)
            .field("schedule_guard", &"[redacted]")
            .field("response_identity_guard", &"[redacted]")
            .field("cached_input_tokens", &self.cached_input_tokens)
            .field("cache_write_input_tokens", &self.cache_write_input_tokens)
            .field("reasoning_output_tokens", &self.reasoning_output_tokens)
            .finish()
    }
}

/// Invalid fixed-catalog profile or rate contract.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentProviderPricingContractError {
    /// Provider, route, reasoning, or allowed-effective catalog identity is invalid.
    #[error("agent provider pricing catalog identity is invalid")]
    CatalogIdentity,
    /// Applicable input range was zero or above the hard model-token ceiling.
    #[error("agent provider pricing input range is invalid")]
    InputRange,
    /// A token rate was zero or above the hard product arithmetic ceiling.
    #[error("agent provider token rate is invalid")]
    Rate,
}

/// Refusal to price one exact terminal provider usage record.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AgentProviderPricingError {
    /// Provider/model/tokenizer/billing/profile identity did not match.
    #[error("agent provider pricing identity does not match")]
    Identity,
    /// Provider usage fell outside this schedule's applicable input range.
    #[error("agent provider usage exceeds the pricing input range")]
    InputRange,
    /// Checked category aggregation, multiplication, rounding, or conversion failed.
    #[error("agent provider pricing arithmetic overflowed")]
    Arithmetic,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AgentProviderProtocolError, AgentProviderReasoningEffort, AgentProviderStreamBudget,
        SemanticTokenizerRevision,
    };

    fn profile(max_input_tokens: u64) -> AgentProviderPricingProfile {
        AgentProviderPricingProfile::try_new(
            AgentProviderPricingRevision::new(7).expect("revision"),
            max_input_tokens,
        )
        .expect("profile")
    }

    fn provider_schedule(
        provider: AgentProviderKind,
        model: &str,
        max_input_tokens: u64,
        rates: AgentProviderTokenRates,
    ) -> AgentProviderPricingSchedule {
        AgentProviderPricingSchedule::try_for_test(
            provider,
            AgentProviderModelRevision::try_new(model.to_owned()).expect("model"),
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            profile(max_input_tokens),
            rates,
        )
        .expect("schedule")
    }

    fn call_config(schedule: &AgentProviderPricingSchedule) -> AgentProviderCallConfig {
        schedule
            .try_call_config(32, 64, AgentProviderStreamBudget::STANDARD)
            .expect("config")
    }

    #[test]
    fn provider_exact_config_is_schedule_bound_openai_only_and_has_no_token_sentinel() {
        let openai = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS,
            rates(),
        );
        let provider_exact = openai
            .try_provider_exact_call_config(64, AgentProviderStreamBudget::STANDARD)
            .expect("provider-exact config");
        assert_eq!(
            provider_exact.input_accounting_mode(),
            AgentProviderInputAccountingMode::ProviderExactAfterConservativeReservation
        );
        assert_eq!(provider_exact.accounting_guard(), openai.accounting_guard());

        let exact = call_config(&openai);
        assert_eq!(
            exact.input_accounting_mode(),
            AgentProviderInputAccountingMode::ExactLocal {
                fixed_input_tokens: 32
            }
        );
        assert_ne!(provider_exact, exact);

        let anthropic = provider_schedule(
            AgentProviderKind::AnthropicMessages,
            "claude-fixed",
            100,
            rates(),
        );
        assert!(matches!(
            anthropic.try_provider_exact_call_config(64, AgentProviderStreamBudget::STANDARD),
            Err(super::super::AgentProviderContractError::InputAccountingMode)
        ));

        let long_context = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS + 1,
            rates(),
        );
        assert!(matches!(
            long_context.try_provider_exact_call_config(64, AgentProviderStreamBudget::STANDARD),
            Err(super::super::AgentProviderContractError::InputAccountingMode)
        ));
    }

    fn model_reported_schedule(max_input_tokens: u64) -> AgentProviderPricingSchedule {
        let model = AgentProviderModelRevision::try_new("chosen-model".to_owned()).expect("model");
        AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            model.clone(),
            vec![model],
            AgentProviderResponseRoute::ModelReported,
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("reported:chosen-model".to_owned())
                .expect("tokenizer"),
            profile(max_input_tokens),
            rates(),
        )
        .expect("schedule")
    }

    #[test]
    fn provider_reported_config_never_takes_an_exact_or_counted_path() {
        let schedule = model_reported_schedule(MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS);
        let reported = schedule
            .try_provider_reported_call_config(64, AgentProviderStreamBudget::STANDARD)
            .expect("reported config");
        assert_eq!(
            reported.input_accounting_mode(),
            AgentProviderInputAccountingMode::ProviderReportedAfterConservativeReservation
        );
        assert!(reported.input_accounting_mode().reserves_whole_request());
        assert_eq!(
            reported.billing_class(),
            AgentProviderBillingClass::ModelReported
        );
        assert_eq!(
            reported.billing_class().provider(),
            AgentProviderKind::OpenAiResponses
        );
        // The model route can be neither exact-counted nor locally exact.
        assert!(matches!(
            schedule.try_provider_exact_call_config(64, AgentProviderStreamBudget::STANDARD),
            Err(super::super::AgentProviderContractError::InputAccountingMode)
        ));
        assert!(matches!(
            schedule.try_call_config(32, 64, AgentProviderStreamBudget::STANDARD),
            Err(super::super::AgentProviderContractError::InputAccountingMode)
        ));
        // And an OpenAI catalog entry cannot be settled on reported usage.
        let openai = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS,
            rates(),
        );
        assert!(matches!(
            openai.try_provider_reported_call_config(64, AgentProviderStreamBudget::STANDARD),
            Err(super::super::AgentProviderContractError::InputAccountingMode)
        ));
        assert!(matches!(
            model_reported_schedule(MAX_AGENT_PROVIDER_EXACT_COUNTED_INPUT_TOKENS + 1)
                .try_provider_reported_call_config(64, AgentProviderStreamBudget::STANDARD),
            Err(super::super::AgentProviderContractError::InputAccountingMode)
        ));
        // Reported usage prices like any other usage, under its own identity.
        let usage = AgentProviderUsage::try_new(100, 10, 20, 30, 0).expect("usage");
        let priced = schedule
            .try_price(&reported, response_identity(&reported), usage)
            .expect("priced");
        let (_, cost, attribution) = priced.into_policy_parts();
        // 50 fresh × $1 + 20 cached × $0.10 + 30 written × $1.25 + 10 out × $5
        // per million tokens is 139.5 micro-USD, rounded up once.
        assert_eq!(cost, 140);
        assert_eq!(
            attribution.billing_class(),
            AgentProviderBillingClass::ModelReported
        );
    }

    fn response_identity(config: &AgentProviderCallConfig) -> AgentProviderResponseIdentity {
        let route = config.response_route();
        AgentProviderResponseIdentity::try_attested(
            config,
            config.model().as_str(),
            route.response_service_tier(),
            route.response_inference_geo(),
        )
        .expect("response identity")
    }

    fn rates() -> AgentProviderTokenRates {
        AgentProviderTokenRates::try_new(1_000_000, 100_000, 1_250_000, 5_000_000).expect("rates")
    }

    #[test]
    fn pricing_uses_four_disjoint_categories_and_one_conservative_rounding() {
        let schedule = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            200,
            rates(),
        );
        let config = call_config(&schedule);
        let identity = response_identity(&config);
        let usage = AgentProviderUsage::try_new(100, 3, 20, 10, 1).expect("usage");
        let priced = schedule.try_price(&config, identity, usage).expect("price");

        assert_eq!(priced.usage(), usage);
        assert_eq!(priced.pricing_revision().value(), 7);
        assert_eq!(priced.schedule_guard(), schedule.accounting_guard());
        assert_eq!(priced.response_identity_guard(), identity.guard());
        assert_eq!(priced.cost_ceiling_micro_usd(), 100);
        assert!(!format!("{priced:?}").contains("gpt-fixed"));

        let (_, _, attribution) = priced.into_policy_parts();
        assert_eq!(attribution.provider(), AgentProviderKind::OpenAiResponses);
        assert_eq!(
            attribution.billing_class(),
            AgentProviderBillingClass::OpenAiDefault
        );
        assert_eq!(attribution.pricing_revision().value(), 7);
        assert_eq!(attribution.schedule_guard(), schedule.accounting_guard());
        assert_eq!(attribution.cached_input_tokens(), 20);
        assert_eq!(attribution.cache_write_input_tokens(), 10);
        assert_eq!(attribution.reasoning_output_tokens(), 1);
        assert!(!format!("{attribution:?}").contains("gpt-fixed"));

        let one_fractional = AgentProviderUsage::try_new(1, 0, 0, 0, 0).expect("usage");
        let fractional_rates =
            AgentProviderTokenRates::try_new(1, 1, 1, 1).expect("fractional rates");
        let fractional_schedule = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            200,
            fractional_rates,
        );
        let fractional_config = call_config(&fractional_schedule);
        let fractional = fractional_schedule
            .try_price(
                &fractional_config,
                response_identity(&fractional_config),
                one_fractional,
            )
            .expect("rounded price");
        assert_eq!(fractional.cost_ceiling_micro_usd(), 1);
        assert_ne!(fractional.schedule_guard(), schedule.accounting_guard());
    }

    #[test]
    fn identity_range_and_arithmetic_mismatches_fail_closed() {
        let schedule = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            100,
            rates(),
        );
        let config = call_config(&schedule);
        let identity = response_identity(&config);
        let usage = AgentProviderUsage::try_new(50, 1, 0, 0, 0).expect("usage");
        let other_schedule = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-other",
            100,
            rates(),
        );
        let other = call_config(&other_schedule);
        assert!(matches!(
            schedule.try_price(&other, response_identity(&other), usage),
            Err(AgentProviderPricingError::Identity)
        ));
        assert!(matches!(
            schedule.try_price(
                &config,
                identity,
                AgentProviderUsage::try_new(101, 0, 0, 0, 0).expect("usage")
            ),
            Err(AgentProviderPricingError::InputRange)
        ));
        let ranged_profile = AgentProviderPricingProfile::try_for_input_range(
            AgentProviderPricingRevision::new(9).expect("revision"),
            51,
            100,
        )
        .expect("ranged profile");
        let ranged = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
            vec![AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model")],
            AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            ranged_profile,
            rates(),
        )
        .expect("ranged schedule");
        let ranged_config = call_config(&ranged);
        assert!(matches!(
            ranged.try_price(
                &ranged_config,
                response_identity(&ranged_config),
                AgentProviderUsage::try_new(50, 1, 0, 0, 0).expect("usage")
            ),
            Err(AgentProviderPricingError::InputRange)
        ));

        let maximum_rates = AgentProviderTokenRates::try_new(
            MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS,
            MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS,
            MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS,
            MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS,
        )
        .expect("maximum rates");
        let arithmetic = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            100,
            maximum_rates,
        );
        let arithmetic_config = call_config(&arithmetic);
        assert!(matches!(
            arithmetic.try_price(
                &arithmetic_config,
                response_identity(&arithmetic_config),
                AgentProviderUsage::try_new(1, u64::MAX - 1, 0, 0, 0).expect("usage")
            ),
            Err(AgentProviderPricingError::Arithmetic)
        ));
    }

    #[test]
    fn schedule_guard_binds_every_pricing_identity_and_rate_dimension() {
        let base = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            100,
            rates(),
        );
        let exact = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            100,
            rates(),
        );
        assert_eq!(base.accounting_guard(), exact.accounting_guard());

        let other_model = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-other",
            100,
            rates(),
        );
        let other_tokenizer = AgentProviderPricingSchedule::try_for_test(
            AgentProviderKind::OpenAiResponses,
            AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v2".to_owned()).expect("tokenizer"),
            profile(100),
            rates(),
        )
        .expect("schedule");
        let other_profile = AgentProviderPricingSchedule::try_for_test(
            AgentProviderKind::OpenAiResponses,
            AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            AgentProviderPricingProfile::try_new(
                AgentProviderPricingRevision::new(8).expect("revision"),
                101,
            )
            .expect("profile"),
            rates(),
        )
        .expect("schedule");
        let other_rates = provider_schedule(
            AgentProviderKind::OpenAiResponses,
            "gpt-fixed",
            100,
            AgentProviderTokenRates::try_new(1_000_001, 100_000, 1_250_000, 5_000_000)
                .expect("rates"),
        );
        let other_provider = AgentProviderPricingSchedule::try_for_test(
            AgentProviderKind::AnthropicMessages,
            AgentProviderModelRevision::try_new("claude-fixed".to_owned()).expect("model"),
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            profile(100),
            rates(),
        )
        .expect("schedule");
        let other_range = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
            vec![AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model")],
            AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            AgentProviderPricingProfile::try_for_input_range(
                AgentProviderPricingRevision::new(7).expect("revision"),
                2,
                100,
            )
            .expect("profile"),
            rates(),
        )
        .expect("schedule");
        let other_reasoning = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
            vec![AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model")],
            AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderReasoningEffort::High,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            profile(100),
            rates(),
        )
        .expect("schedule");
        let allowed_snapshot = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
            vec![
                AgentProviderModelRevision::try_new("gpt-fixed".to_owned()).expect("model"),
                AgentProviderModelRevision::try_new("gpt-fixed-2026-01-01".to_owned())
                    .expect("snapshot"),
            ],
            AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderReasoningEffort::None,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            profile(100),
            rates(),
        )
        .expect("schedule");
        for changed in [
            other_model.accounting_guard(),
            other_tokenizer.accounting_guard(),
            other_profile.accounting_guard(),
            other_rates.accounting_guard(),
            other_provider.accounting_guard(),
            other_range.accounting_guard(),
            other_reasoning.accounting_guard(),
            allowed_snapshot.accounting_guard(),
        ] {
            assert_ne!(base.accounting_guard(), changed);
        }
    }

    #[test]
    fn profile_and_rate_contracts_are_nonzero_and_hard_bounded() {
        let revision = AgentProviderPricingRevision::new(1).expect("revision");
        assert!(AgentProviderPricingRevision::new(0).is_none());
        assert_eq!(
            AgentProviderPricingProfile::try_new(revision, 0),
            Err(AgentProviderPricingContractError::InputRange)
        );
        assert_eq!(
            AgentProviderPricingProfile::try_new(revision, MAX_AGENT_RUN_MODEL_TOKENS + 1),
            Err(AgentProviderPricingContractError::InputRange)
        );
        assert_eq!(
            AgentProviderPricingProfile::try_for_input_range(revision, 0, 1),
            Err(AgentProviderPricingContractError::InputRange)
        );
        assert_eq!(
            AgentProviderPricingProfile::try_for_input_range(revision, 2, 1),
            Err(AgentProviderPricingContractError::InputRange)
        );
        assert_eq!(
            AgentProviderTokenRates::try_new(0, 1, 1, 1),
            Err(AgentProviderPricingContractError::Rate)
        );
        assert_eq!(
            AgentProviderTokenRates::try_new(
                1,
                1,
                1,
                MAX_AGENT_PROVIDER_RATE_MICRO_USD_PER_MILLION_TOKENS + 1,
            ),
            Err(AgentProviderPricingContractError::Rate)
        );
    }

    #[test]
    fn requested_alias_accepts_only_canonical_catalog_attested_effective_identity() {
        let alias = AgentProviderModelRevision::try_new("gpt-alias".to_owned()).expect("alias");
        let snapshot = AgentProviderModelRevision::try_new("gpt-snapshot-2026-01-01".to_owned())
            .expect("snapshot");
        let schedule = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            alias.clone(),
            vec![snapshot.clone(), alias.clone()],
            AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderReasoningEffort::High,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            profile(200),
            rates(),
        )
        .expect("schedule");
        let canonical = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            alias,
            vec![
                AgentProviderModelRevision::try_new("gpt-alias".to_owned()).expect("alias"),
                snapshot.clone(),
            ],
            AgentProviderResponseRoute::OpenAiDefault,
            AgentProviderReasoningEffort::High,
            SemanticTokenizerRevision::try_new("tokenizer-v1".to_owned()).expect("tokenizer"),
            profile(200),
            rates(),
        )
        .expect("canonical schedule");
        assert_eq!(schedule.accounting_guard(), canonical.accounting_guard());
        let config = call_config(&schedule);
        let identity = AgentProviderResponseIdentity::try_attested(
            &config,
            snapshot.as_str(),
            "default",
            None,
        )
        .expect("approved effective identity");
        assert!(schedule
            .try_price(
                &config,
                identity,
                AgentProviderUsage::try_new(100, 1, 0, 0, 0).expect("usage"),
            )
            .is_ok());
        assert_eq!(
            AgentProviderResponseIdentity::try_attested(
                &config,
                "gpt-unapproved-snapshot",
                "default",
                None,
            ),
            Err(AgentProviderProtocolError::Event)
        );
    }
}
