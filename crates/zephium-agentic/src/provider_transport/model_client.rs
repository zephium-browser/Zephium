//! The person's chosen model behind the page agent.
//!
//! Admission, disclosure commit, cancellation, the deadline and settlement are
//! the fixed-endpoint transport's own. Only the generation drive differs: the
//! admitted body is translated into one provider-neutral call, and its reply
//! is decoded through the closed browser-tool contract.

use std::fmt;
use std::sync::Arc;

use sha2::{Digest as _, Sha256};
use zephium_core::work::model::{
    HostedRefusal, WorkModelClient, WorkModelEntry, WorkModelError, WorkModelEvent, WorkModelPrice,
    WorkModelRef, WorkModelUsage,
};

use super::*;
use crate::{
    translate_model_client_call, AgentProviderContractError, AgentProviderModelRevision,
    AgentProviderPricingProfile, AgentProviderPricingRevision, AgentProviderPricingSchedule,
    AgentProviderReasoningEffort, AgentProviderResponseRoute, AgentProviderStreamBudget,
    AgentProviderTokenRates, SemanticTokenizerRevision,
};

/// Per-million rates for a model the catalog does not price, in micro-USD:
/// the same conservative table the lead charges such a model.
const UNPRICED: WorkModelPrice = WorkModelPrice {
    input: 15_000_000,
    cached_input: 1_500_000,
    output: 60_000_000,
};
/// Pricing revisions that tell a catalog price from the fallback above.
const CATALOG_PRICE_REVISION: u64 = 1;
const FALLBACK_PRICE_REVISION: u64 = 2;
/// Below every supported provider's long-context price tier.
const MAX_MODEL_INPUT_TOKENS: u64 = 200_000;
const MAX_LABEL_BYTES: usize = 80;

/// One chosen model and its client, bound to a pricing schedule built from
/// the person's catalog entry. Cloning shares the client.
#[derive(Clone)]
pub struct AgentProviderModelClient {
    model: WorkModelRef,
    vision: bool,
    max_output_tokens: u32,
    priced: bool,
    rates: AgentProviderTokenRates,
    schedule: Arc<AgentProviderPricingSchedule>,
    client: Arc<dyn WorkModelClient>,
}

impl AgentProviderModelClient {
    /// Binds `client` to `entry`. A model that cannot call tools cannot drive
    /// a page and is refused.
    pub fn try_new(
        entry: &WorkModelEntry,
        client: Arc<dyn WorkModelClient>,
    ) -> Result<Self, AgentProviderContractError> {
        if !entry.supports.tools || entry.max_output == 0 || entry.context_window == 0 {
            return Err(AgentProviderContractError::InputAccountingMode);
        }
        let label = label(&entry.id);
        let model = AgentProviderModelRevision::try_new(label.clone())?;
        let tokenizer = SemanticTokenizerRevision::try_new(format!("reported:{label}"))
            .map_err(|_| AgentProviderContractError::TokenizerRevision)?;
        let (price, revision) = match &entry.price {
            Some(price) => (price, CATALOG_PRICE_REVISION),
            None => (&UNPRICED, FALLBACK_PRICE_REVISION),
        };
        let rate = |value: u64| value.max(1);
        let rates = if entry.is_free_compatible() {
            AgentProviderTokenRates::free()
        } else {
            AgentProviderTokenRates::try_new(
                rate(price.input),
                rate(price.cached_input),
                rate(price.input.saturating_mul(5).div_ceil(4)),
                rate(price.output),
            )
            .map_err(|_| AgentProviderContractError::InputAccountingMode)?
        };
        let profile = AgentProviderPricingProfile::try_new(
            AgentProviderPricingRevision::new(revision)
                .ok_or(AgentProviderContractError::InputAccountingMode)?,
            u64::from(entry.context_window).min(MAX_MODEL_INPUT_TOKENS),
        )
        .map_err(|_| AgentProviderContractError::InputAccountingMode)?;
        let schedule = AgentProviderPricingSchedule::try_new(
            AgentProviderKind::OpenAiResponses,
            model.clone(),
            vec![model],
            AgentProviderResponseRoute::ModelReported,
            if entry.supports.reasoning {
                AgentProviderReasoningEffort::Low
            } else {
                AgentProviderReasoningEffort::None
            },
            tokenizer,
            profile,
            rates,
        )
        .map_err(|_| AgentProviderContractError::InputAccountingMode)?;
        Ok(Self {
            model: entry.model.clone(),
            vision: entry.supports.vision,
            max_output_tokens: entry.max_output,
            priced: entry.price.is_some(),
            rates,
            schedule: Arc::new(schedule),
            client,
        })
    }

    /// A call configuration settled on this model's reported usage. The
    /// output ceiling never exceeds the model's own.
    pub fn try_call_config(
        &self,
        max_output_tokens: u32,
    ) -> Result<AgentProviderCallConfig, AgentProviderContractError> {
        self.schedule.try_provider_reported_call_config(
            max_output_tokens.min(self.max_output_tokens),
            AgentProviderStreamBudget::STANDARD,
        )
    }

    /// The schedule every settlement of this model's calls must use.
    pub fn schedule(&self) -> &AgentProviderPricingSchedule {
        &self.schedule
    }

    /// Whether the person's catalog prices this model; otherwise calls are
    /// priced at the conservative fallback rates.
    pub const fn is_priced(&self) -> bool {
        self.priced
    }

    /// Whether the model accepts images.
    pub const fn supports_vision(&self) -> bool {
        self.vision
    }

    /// Largest input one call may reserve.
    pub fn max_input_tokens(&self) -> u64 {
        self.schedule_profile().max_input_tokens()
    }

    /// Strict cost ceiling for `input` and `output` tokens, every input token
    /// at the highest input rate, rounded up once.
    pub fn reservation_cost_micro_usd(&self, input: u64, output: u64) -> Option<u64> {
        let input_rate = self
            .rates
            .uncached_input()
            .max(self.rates.cached_input())
            .max(self.rates.cache_write_input());
        let numerator = u128::from(input)
            .checked_mul(u128::from(input_rate))?
            .checked_add(u128::from(output).checked_mul(u128::from(self.rates.output()))?)?;
        u64::try_from(numerator.div_ceil(1_000_000)).ok()
    }

    /// What `usage` costs at this model's rates: cached and written input at
    /// their own rates, the rest of the input and all output at theirs.
    pub(super) fn usage_cost_micro_usd(&self, usage: &WorkModelUsage) -> Option<u64> {
        let cached = usage.cached_input_tokens.min(usage.input_tokens);
        let written = usage
            .cache_write_input_tokens
            .min(usage.input_tokens - cached);
        let fresh = usage.input_tokens - cached - written;
        let part = |tokens: u64, rate: u64| u128::from(tokens).checked_mul(u128::from(rate));
        let numerator = part(fresh, self.rates.uncached_input())?
            .checked_add(part(cached, self.rates.cached_input())?)?
            .checked_add(part(written, self.rates.cache_write_input())?)?
            .checked_add(part(usage.output_tokens, self.rates.output())?)?;
        u64::try_from(numerator.div_ceil(1_000_000)).ok()
    }

    pub(super) const fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }

    pub(super) fn client(&self) -> &dyn WorkModelClient {
        self.client.as_ref()
    }

    pub(super) fn binds(&self, config: &AgentProviderCallConfig) -> bool {
        config.accounting_guard() == self.schedule.accounting_guard()
    }

    pub(super) const fn model(&self) -> &WorkModelRef {
        &self.model
    }

    fn schedule_profile(&self) -> AgentProviderPricingProfile {
        self.schedule.profile()
    }
}

#[cfg(feature = "probe-harness")]
impl AgentProviderModelClient {
    /// Replays one recorded page-agent body on this model outside any run,
    /// for the release-excluded probe. Returns closed facts, never model text.
    pub async fn probe_replay(&self, body: &[u8], thinking: bool) -> String {
        let Ok(config) = self.try_call_config(8_192) else {
            return "config=refused".to_owned();
        };
        let Ok((mut request, contract)) =
            translate_model_client_call(&config, self.model(), self.supports_vision(), body)
        else {
            return "translate=refused".to_owned();
        };
        if !thinking {
            request.reasoning = None;
        }
        let started = Instant::now();
        // As the page agent does: a reply with no usable call is answered
        // with its reason and asked again, at most MAX_REASKS times.
        let mut asked_again = 0_u8;
        let outcome = loop {
            let outcome = match self.client.call(request.clone(), &|_| {}).await {
                Ok(outcome) => outcome,
                Err(error) => {
                    return format!(
                        "error={error:?} elapsed_ms={}",
                        started.elapsed().as_millis()
                    )
                }
            };
            match (asked_again < MAX_REASKS)
                .then(|| contract.reask(&outcome, &request.tools))
                .flatten()
            {
                Some(messages) => {
                    asked_again += 1;
                    request.messages.extend(messages);
                    request.volatile_tail = 0;
                }
                None => break outcome,
            }
        };
        let elapsed = started.elapsed().as_millis();
        let usage = outcome.usage;
        let call = crate::AgentProviderCallIdentity::probe_fixture();
        let decoded = match contract.finish(call, &config, outcome) {
            Ok(reply) => format!("decoded=ok text={}", reply.text.is_some()),
            Err(error) => format!("decoded={error:?}"),
        };
        format!(
            "elapsed_ms={elapsed} asked_again={asked_again} input={} cached={} output={} reasoning={} {decoded}",
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.output_tokens,
            usage.reasoning_tokens
        )
    }
}

impl fmt::Debug for AgentProviderModelClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentProviderModelClient")
            .field("wire", &self.model.wire)
            .field("vision", &self.vision)
            .field("priced", &self.priced)
            .field("model", &"[redacted]")
            .finish()
    }
}

/// A catalog id as a closed model label: its safe bytes, or a digest when it
/// has none or too many.
fn label(id: &str) -> String {
    let safe: String = id
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                char::from(byte)
            } else {
                '_'
            }
        })
        .collect();
    if !safe.is_empty() && safe.len() <= MAX_LABEL_BYTES {
        return safe;
    }
    let digest = Sha256::digest(id.as_bytes());
    let hex: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("model-{hex}")
}

/// Asks again after a reply with no usable browser call, per model call.
const MAX_REASKS: u8 = 2;
/// Output a browser turn keeps back for one re-ask after a runaway reply.
const RUNAWAY_REASK_OUTPUT: u64 = 512;
/// Tokens the reason and the model's own failed turn may add to a re-ask's
/// input beyond the last call's input and output.
const REASK_INPUT_ALLOWANCE: u64 = 1_024;

fn added(spent: WorkModelUsage, more: WorkModelUsage) -> WorkModelUsage {
    WorkModelUsage {
        input_tokens: spent.input_tokens.saturating_add(more.input_tokens),
        cached_input_tokens: spent
            .cached_input_tokens
            .saturating_add(more.cached_input_tokens),
        cache_write_input_tokens: spent
            .cache_write_input_tokens
            .saturating_add(more.cache_write_input_tokens),
        output_tokens: spent.output_tokens.saturating_add(more.output_tokens),
        reasoning_tokens: spent.reasoning_tokens.saturating_add(more.reasoning_tokens),
        cost_micros: spent
            .cost_micros
            .zip(more.cost_micros)
            .map(|(spent, more)| spent.saturating_add(more)),
    }
}

const fn failure_for(error: WorkModelError) -> AgentProviderFailure {
    let class = match error {
        WorkModelError::MissingKey | WorkModelError::Unauthorized => {
            AgentProviderFailureClass::Authentication
        }
        WorkModelError::RateLimited { .. } => AgentProviderFailureClass::RateLimited,
        WorkModelError::Overloaded => AgentProviderFailureClass::Overloaded,
        WorkModelError::ContextTooLong | WorkModelError::BadRequest => {
            AgentProviderFailureClass::InvalidRequest
        }
        WorkModelError::Network => AgentProviderFailureClass::Transport,
        WorkModelError::Protocol => AgentProviderFailureClass::Protocol,
        WorkModelError::Cancelled => AgentProviderFailureClass::Cancelled,
        WorkModelError::OverBudget | WorkModelError::PlanLimit => {
            AgentProviderFailureClass::Provider
        }
        WorkModelError::Hosted { refusal, .. } => match refusal {
            HostedRefusal::PlanRequired | HostedRefusal::BillingReview => {
                AgentProviderFailureClass::Permission
            }
            HostedRefusal::AlreadySent => AgentProviderFailureClass::Conflict,
            HostedRefusal::InvalidAudio
            | HostedRefusal::UnsupportedAudio
            | HostedRefusal::AudioTooLong => AgentProviderFailureClass::InvalidRequest,
            _ => AgentProviderFailureClass::Provider,
        },
    };
    AgentProviderFailure::new(class)
}

impl AgentProviderAttempt {
    /// Drives one turn on the chosen model this attempt was admitted for,
    /// retaining every terminal authority exactly as the fixed-endpoint
    /// drives do. Usage above the committed reservation is never priced: the
    /// call fails and is charged its reservation.
    pub async fn execute_model_client<F>(
        &mut self,
        mut consume: F,
    ) -> Result<AgentProviderTransportResult, AgentProviderAttemptStateError>
    where
        F: FnMut(AgentProviderStreamBatch) -> AgentProviderBatchDisposition,
    {
        if matches!(
            self.phase,
            AgentProviderAttemptPhase::CountDriving
                | AgentProviderAttemptPhase::GenerationDriving
                | AgentProviderAttemptPhase::Terminal
        ) {
            return Err(self.state_error());
        }
        let Some(model) = self.model.clone() else {
            return self.finish_terminal(protocol_failure());
        };
        let (call, config, endpoint, body) = self.begin_generation_drive()?;
        let reserved = match self.core.as_ref() {
            Some(AgentProviderAttemptCore::Generation(terminal)) => {
                terminal.input.input_token_limit()
            }
            _ => return self.finish_terminal(protocol_failure()),
        };
        if !self.generation_matches(call)
            || endpoint != AgentProviderEndpoint::OpenAiResponses
            || !model.binds(&config)
        {
            return self.finish_terminal(protocol_failure());
        }
        if Instant::now() >= self.deadline {
            return self.finish_terminal(timeout_failure());
        }
        if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {
            return self.finish_terminal(cancelled_failure());
        }
        let (mut request, contract) = match translate_model_client_call(
            &config,
            model.model(),
            model.supports_vision(),
            &body,
        ) {
            Ok(translated) => translated,
            Err(error) => return self.finish_terminal(protocol_decoder_failure(error, None)),
        };
        request.image_fence = self.image_fence.clone();
        let image_fence = self.image_fence.clone();
        drop(body);
        if self.cancellation.is_cancelled() || self.shutdown.is_cancelled() {
            return self.finish_terminal(cancelled_failure());
        }
        if image_fence
            .as_ref()
            .is_some_and(|fence| !fence.while_current(&mut || {}))
        {
            return self.finish_terminal(cancelled_failure());
        }
        // This is immediately before the client can first send anything.
        self.mark_model_may_have_dispatched();
        let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(self.deadline));
        tokio::pin!(deadline);
        // Streamed events are not trusted before the call ends; the outcome
        // alone is decoded.
        let events = |_: WorkModelEvent| {};
        let max_output = u64::from(config.max_output_tokens());
        // A browser turn keeps part of its output allowance back, so a reply
        // that runs past the rest can be asked once more inside the call's
        // own reservation instead of ending the page.
        let runaway_reserve = if contract.is_structured() {
            0
        } else {
            RUNAWAY_REASK_OUTPUT.min(max_output / 4)
        };
        request.max_output_tokens = request
            .max_output_tokens
            .min(u32::try_from(max_output - runaway_reserve).unwrap_or(u32::MAX));
        let mut ran_away = false;
        let mut spent = WorkModelUsage::default();
        let mut reasks = 0_u8;
        let mut action_repair_used = false;
        let mut action_rejections = [None; 2];
        let outcome = loop {
            let outcome = tokio::select! {
                biased;
                () = self.cancellation.cancelled() => return self.finish_terminal(cancelled_failure()),
                () = self.shutdown.cancelled() => return self.finish_terminal(cancelled_failure()),
                () = lead::image_fence::revoked(image_fence.as_ref()) => return self.finish_terminal(cancelled_failure()),
                () = &mut deadline => return self.finish_terminal(timeout_failure()),
                outcome = model.client.call(request.clone(), &events) => outcome,
            };
            let mut outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => {
                    return self.finish_terminal(AgentProviderTransportOutcomeOwned::Failed(
                        failure_for(error),
                    ))
                }
            };
            let last = outcome.usage;
            spent = if reasks == 0 {
                last
            } else {
                added(spent, last)
            };
            if spent.input_tokens > reserved || spent.output_tokens > max_output {
                return self.finish_terminal(protocol_decoder_failure(
                    crate::AgentProviderProtocolError::Usage,
                    None,
                ));
            }
            outcome.usage = spent;
            let action_error = contract.action_error(&outcome);
            if let Some(error) = action_error {
                if let Some(slot) = action_rejections.iter_mut().find(|slot| slot.is_none()) {
                    *slot = Some(error);
                }
            }
            self.model_reply = Some(
                crate::AgentProviderModelReplyFacts::default()
                    .with_asked_again(reasks)
                    .with_action_rejections(action_rejections),
            );
            // DeepSeek can think past the whole allowance and say nothing.
            // That reply is asked once more, told to be brief, with thinking
            // off and only the kept-back output; it is charged as reported.
            if !ran_away
                && !action_repair_used
                && runaway_reserve > 0
                && max_output - spent.output_tokens >= runaway_reserve
                && spent
                    .input_tokens
                    .saturating_add(last.input_tokens)
                    .saturating_add(REASK_INPUT_ALLOWANCE)
                    <= reserved
            {
                if let Some(messages) = contract.reask_runaway(&outcome) {
                    ran_away = true;
                    reasks += 1;
                    request.messages.extend(messages);
                    request.volatile_tail = 0;
                    request.reasoning = None;
                    request.max_output_tokens =
                        u32::try_from(max_output - spent.output_tokens).unwrap_or(u32::MAX);
                    continue;
                }
            }
            // A provider without strict tool calling may answer with no call
            // or a malformed one. It is told exactly why and asked again,
            // inside this call's own reservation; every ask is charged as
            // reported, and a reply that still fails ends as before.
            let left_output = max_output - spent.output_tokens;
            let next_input = spent
                .input_tokens
                .saturating_add(last.input_tokens)
                .saturating_add(last.output_tokens)
                .saturating_add(REASK_INPUT_ALLOWANCE);
            let Some(messages) = (reasks < MAX_REASKS
                && !action_repair_used
                && next_input <= reserved
                && left_output >= last.output_tokens.max(1))
            .then(|| contract.reask(&outcome, &request.tools))
            .flatten() else {
                break outcome;
            };
            reasks += 1;
            action_repair_used |= action_error.is_some();
            request.messages.extend(messages);
            request.volatile_tail = 0;
            request.max_output_tokens = u32::try_from(left_output).unwrap_or(u32::MAX);
        };
        let reply = match contract.finish(call, &config, outcome) {
            Ok(reply) => reply,
            Err(error) => return self.finish_terminal(protocol_decoder_failure(error, None)),
        };
        self.model_reply = Some(
            reply
                .facts
                .with_asked_again(reasks)
                .with_action_rejections(action_rejections),
        );
        if let Some(text) = reply.text {
            let batch = AgentProviderStreamBatch::new(call, vec![text]);
            let disposition =
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| consume(batch))) {
                    Ok(disposition) => disposition,
                    Err(_) => {
                        self.fail_stop();
                        return self.finish_terminal(integration_failure());
                    }
                };
            if disposition == AgentProviderBatchDisposition::Cancel {
                return self.finish_terminal(cancelled_failure());
            }
        }
        self.finish_terminal(AgentProviderTransportOutcomeOwned::Stream(Box::new(
            reply.finished,
        )))
    }
}

#[cfg(test)]
mod tests;
