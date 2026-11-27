use std::sync::Mutex;

use serde_json::json;
use zephium_core::work::model::{
    WorkModelEntry, WorkModelFuture, WorkModelMessage, WorkModelOutcome, WorkModelPart,
    WorkModelPrice, WorkModelProvider, WorkModelRequest, WorkModelStop, WorkModelSupports,
    WorkModelToolCall, WorkModelToolChoice, WorkModelUsage, WorkModelWire,
};

use super::super::tests::{model_reported_fixture, provider_exact_fixture};
use super::*;
use crate::{
    AgentBrowserToolKind, AgentModelCallSettlement, AgentModelUsageAccounting,
    AgentProviderBillingClass, AgentProviderFailureClass,
};

fn entry(id: &str) -> WorkModelEntry {
    WorkModelEntry {
        id: id.to_owned(),
        model: WorkModelRef {
            provider: WorkModelProvider::Anthropic,
            wire: WorkModelWire::AnthropicMessages,
            model: "claude-test-5".to_owned(),
        },
        display_name: "Claude Test".to_owned(),
        roles: Vec::new(),
        recommended: false,
        context_window: 200_000,
        max_output: 64_000,
        supports: WorkModelSupports {
            tools: true,
            vision: true,
            prompt_cache: true,
            reasoning: true,
            native_search: false,
        },
        price: Some(WorkModelPrice {
            input: 1_000,
            cached_input: 100,
            output: 5_000,
        }),
    }
}

/// Answers with scripted replies in turn and keeps the requests it was sent.
struct Scripted {
    replies: Mutex<std::collections::VecDeque<Result<WorkModelOutcome, WorkModelError>>>,
    seen: Mutex<Vec<WorkModelRequest>>,
    stall: bool,
}

impl Scripted {
    fn answering(reply: Result<WorkModelOutcome, WorkModelError>) -> Arc<Self> {
        Self::answering_each(vec![reply])
    }

    fn answering_each(replies: Vec<Result<WorkModelOutcome, WorkModelError>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
            stall: false,
        })
    }

    fn stalled() -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(Default::default()),
            seen: Mutex::new(Vec::new()),
            stall: true,
        })
    }

    fn seen(&self) -> WorkModelRequest {
        self.seen
            .lock()
            .expect("seen")
            .last()
            .cloned()
            .expect("the client was called")
    }

    fn calls(&self) -> Vec<WorkModelRequest> {
        self.seen.lock().expect("seen").clone()
    }
}

impl WorkModelClient for Scripted {
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        _events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a> {
        self.seen.lock().expect("seen").push(request);
        if self.stall {
            return Box::pin(std::future::pending());
        }
        let reply = self
            .replies
            .lock()
            .expect("replies")
            .pop_front()
            .expect("a scripted reply for every call");
        Box::pin(async move { reply })
    }
}

fn outcome(stop: WorkModelStop, assistant: Vec<WorkModelPart>) -> WorkModelOutcome {
    WorkModelOutcome {
        stop,
        usage: WorkModelUsage {
            input_tokens: 900,
            cached_input_tokens: 400,
            cache_write_input_tokens: 100,
            output_tokens: 5,
            reasoning_tokens: 4,
            cost_micros: None,
        },
        assistant,
    }
}

fn tool(name: &str, arguments: serde_json::Value) -> WorkModelPart {
    WorkModelPart::ToolCall(WorkModelToolCall {
        id: format!("toolu_{name}"),
        name: name.to_owned(),
        arguments,
    })
}

fn locate() -> WorkModelPart {
    tool(
        "locate",
        json!({"semantic_query": "synthetic marker", "scope": {"kind": "initial"}}),
    )
}

fn transport() -> AgentProviderTransport {
    AgentProviderTransport::try_new(
        AgentProviderTransportConfig::try_new(
            Duration::from_millis(400),
            Duration::from_millis(400),
            Duration::from_millis(400),
        )
        .expect("config"),
    )
    .expect("transport")
}

async fn run(
    client: Arc<Scripted>,
) -> (
    AgentRunPolicy,
    AgentProviderModelClient,
    AgentProviderTransportResult,
) {
    let model = AgentProviderModelClient::try_new(&entry("anthropic/claude-test-5"), client)
        .expect("model client");
    let (mut policy, input) = model_reported_fixture(&model);
    let mut attempt = transport()
        .try_admit_model(input, &mut policy, &model, AgentProviderCancellation::new())
        .expect("admitted");
    let result = attempt
        .execute_model_client(|_| AgentProviderBatchDisposition::Continue)
        .await
        .expect("terminal");
    (policy, model, result)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scripted_model_round_trip_settles_one_tool_turn() {
    let client = Scripted::answering(Ok(outcome(WorkModelStop::ToolUse, vec![locate()])));
    let (mut policy, model, result) = run(client.clone()).await;
    assert_eq!(
        result.disclosure_stage(),
        AgentProviderDisclosureStage::ModelRequestMayHaveDispatched
    );
    assert_eq!(
        result.model_reply().map(|facts| facts.dropped_tool_calls()),
        Some(0)
    );
    let AgentProviderPolicySettlement::PricingRequired(settlement) =
        result.into_policy_settlement()
    else {
        panic!("reported usage must be priced");
    };
    let terminal = settlement
        .settle(&mut policy, model.schedule())
        .expect("settled");
    let receipt = terminal.receipt();
    assert_eq!(receipt.settlement(), AgentModelCallSettlement::Completed);
    assert_eq!(
        receipt.usage_accounting(),
        AgentModelUsageAccounting::PricedCeiling
    );
    assert_eq!(receipt.input_tokens(), 900);
    assert_eq!(
        receipt
            .pricing_attribution()
            .map(|attribution| attribution.billing_class()),
        Some(AgentProviderBillingClass::ModelReported)
    );
    let turn = terminal.into_tool_turn().expect("one tool turn");
    assert_eq!(turn.proposal().kind(), AgentBrowserToolKind::Locate);

    let sent = client.seen();
    assert_eq!(sent.model.model, "claude-test-5");
    assert_eq!(sent.tool_choice, WorkModelToolChoice::Auto);
    assert!(!sent.parallel_tools && !sent.native_search);
    assert!(sent.tools.iter().any(|tool| tool.name == "locate"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_first_tool_call_is_decoded() {
    let client = Scripted::answering(Ok(outcome(
        WorkModelStop::ToolUse,
        vec![
            WorkModelPart::Text("I will look first.".to_owned()),
            locate(),
            tool("navigate", json!({"url": "https://elsewhere.test/"})),
            tool("not_a_tool", json!({})),
        ],
    )));
    let (mut policy, model, result) = run(client).await;
    let facts = result.model_reply().expect("reply facts");
    assert_eq!(facts.dropped_tool_calls(), 2);
    assert_eq!(facts.dropped_text_bytes(), 18);
    let AgentProviderPolicySettlement::PricingRequired(settlement) =
        result.into_policy_settlement()
    else {
        panic!("reported usage must be priced");
    };
    let turn = settlement
        .settle(&mut policy, model.schedule())
        .expect("settled")
        .into_tool_turn()
        .expect("first call");
    assert_eq!(turn.proposal().kind(), AgentBrowserToolKind::Locate);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_arguments_fail_at_the_reservation() {
    let query = "word ".repeat(4_000);
    let oversized = || {
        Ok(outcome(
            WorkModelStop::ToolUse,
            vec![tool(
                "locate",
                json!({"semantic_query": query, "scope": {"kind": "initial"}}),
            )],
        ))
    };
    // Asked again twice, told why, and still oversized.
    let client = Scripted::answering_each(vec![oversized(), oversized(), oversized()]);
    let (mut policy, _, result) = run(client.clone()).await;
    assert_eq!(client.calls().len(), 3);
    assert!(matches!(
        result.outcome(),
        AgentProviderTransportOutcome::Failed(failure)
            if failure.protocol_error() == Some(crate::AgentProviderProtocolError::Limit)
    ));
    let AgentProviderPolicySettlement::Immediate(settlement) = result.into_policy_settlement()
    else {
        panic!("an undecodable reply is never priced");
    };
    assert_eq!(
        settlement.usage_accounting(),
        AgentModelUsageAccounting::ReservationCeiling
    );
    settlement.settle(&mut policy).expect("charged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_settles_without_a_tool_turn() {
    let client = Scripted::answering(Ok(outcome(
        WorkModelStop::Refused,
        vec![WorkModelPart::Text("I can't help with that.".to_owned())],
    )));
    let (mut policy, model, result) = run(client).await;
    let AgentProviderPolicySettlement::PricingRequired(settlement) =
        result.into_policy_settlement()
    else {
        panic!("a refusal's usage is priced");
    };
    let terminal = settlement
        .settle(&mut policy, model.schedule())
        .expect("settled");
    assert!(matches!(
        terminal.conclusion(),
        AgentProviderStreamConclusion::Completed(completion)
            if completion.stop() == AgentProviderStopReason::Refused
                && completion.stats().output_text_bytes() == 0
    ));
    assert!(!terminal.has_tool_turn());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_above_the_reservation_is_charged_the_reservation() {
    let mut reply = outcome(WorkModelStop::ToolUse, vec![locate()]);
    reply.usage.input_tokens = u64::from(u32::MAX);
    let (mut policy, _, result) = run(Scripted::answering(Ok(reply))).await;
    assert_eq!(
        result.usage_knowledge(),
        AgentProviderUsageKnowledge::UnknownAfterDispatch
    );
    let AgentProviderPolicySettlement::Immediate(settlement) = result.into_policy_settlement()
    else {
        panic!("usage above the reservation is never priced");
    };
    assert_eq!(
        settlement.usage_accounting(),
        AgentModelUsageAccounting::ReservationCeiling
    );
    assert_eq!(
        settlement.settlement(),
        AgentModelCallSettlement::ProviderFailed
    );
    settlement.settle(&mut policy).expect("charged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deadline_wins_over_a_stalled_model() {
    let (mut policy, _, result) = run(Scripted::stalled()).await;
    assert!(matches!(
        result.outcome(),
        AgentProviderTransportOutcome::Failed(failure)
            if failure.class() == AgentProviderFailureClass::Timeout
    ));
    let AgentProviderPolicySettlement::Immediate(settlement) = result.into_policy_settlement()
    else {
        panic!("a lost call is never priced");
    };
    assert_eq!(
        settlement.usage_accounting(),
        AgentModelUsageAccounting::ReservationCeiling
    );
    settlement.settle(&mut policy).expect("charged");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_failure_is_closed_and_charged_the_reservation() {
    let client = Scripted::answering(Err(WorkModelError::RateLimited {
        retry_after_ms: Some(5_000),
    }));
    let (_, _, result) = run(client).await;
    assert!(matches!(
        result.outcome(),
        AgentProviderTransportOutcome::Failed(failure)
            if failure.class() == AgentProviderFailureClass::RateLimited
                && failure.retry_after().is_none()
    ));
    assert_eq!(
        result.usage_knowledge(),
        AgentProviderUsageKnowledge::UnknownAfterDispatch
    );
}

#[test]
fn a_chosen_model_and_a_fixed_endpoint_never_take_each_others_requests() {
    let client = Scripted::answering(Err(WorkModelError::Network));
    let model = AgentProviderModelClient::try_new(&entry("anthropic/claude-test-5"), client)
        .expect("model client");
    let transport = transport();

    let (mut policy, input) = model_reported_fixture(&model);
    let credential =
        AgentProviderCredential::try_new(AgentProviderKind::OpenAiResponses, "sk-test".into())
            .expect("credential");
    assert!(matches!(
        transport.try_admit(
            input,
            &mut policy,
            &credential,
            AgentProviderCancellation::new()
        ),
        Err(AgentProviderAdmissionError::Endpoint)
    ));

    let other = AgentProviderModelClient::try_new(
        &entry("openrouter/anthropic/claude-test-5"),
        Scripted::stalled(),
    )
    .expect("other model");
    let (mut policy, input) = model_reported_fixture(&model);
    assert!(matches!(
        transport.try_admit_model(input, &mut policy, &other, AgentProviderCancellation::new()),
        Err(AgentProviderAdmissionError::ModelBinding)
    ));
    let (mut policy, input) = provider_exact_fixture();
    assert!(matches!(
        transport.try_admit_model(input, &mut policy, &model, AgentProviderCancellation::new()),
        Err(AgentProviderAdmissionError::ModelBinding)
    ));
    assert_eq!(transport.snapshot().expect("snapshot").active_attempts(), 0);
}

#[test]
fn model_entries_bind_closed_labels_and_conservative_prices() {
    let client = Scripted::stalled();
    let mut unpriced = entry("compatible/http://localhost:11434/v1#llama 3");
    unpriced.price = None;
    let model = AgentProviderModelClient::try_new(&unpriced, client.clone()).expect("unpriced");
    assert!(!model.is_priced());
    let config = model.try_call_config(100_000).expect("config");
    assert_eq!(config.max_output_tokens(), 64_000);
    assert!(config
        .model()
        .as_str()
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')));
    assert_eq!(model.max_input_tokens(), 200_000);
    // 1,000 input tokens at the fallback write rate and 100 output tokens.
    assert_eq!(
        model.reservation_cost_micro_usd(1_000, 100),
        Some(18_750 + 6_000)
    );

    let long = entry(&"x".repeat(200));
    let model = AgentProviderModelClient::try_new(&long, client.clone()).expect("long id");
    assert!(model
        .try_call_config(10)
        .expect("config")
        .model()
        .as_str()
        .starts_with("model-"));

    let mut toolless = entry("local/model");
    toolless.supports.tools = false;
    assert!(AgentProviderModelClient::try_new(&toolless, client).is_err());
}

fn reasked_reason(request: &WorkModelRequest) -> String {
    match request.messages.last() {
        Some(WorkModelMessage::ToolResults(results)) => {
            assert!(results.iter().all(|result| result.is_error));
            results[0].content.clone()
        }
        Some(WorkModelMessage::User(parts)) => match parts.last() {
            Some(WorkModelPart::Text(text)) => text.clone(),
            _ => panic!("a re-ask ends in its reason"),
        },
        _ => panic!("a re-ask ends in its reason"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malformed_call_is_asked_again_told_why_and_charged_as_reported() {
    let client = Scripted::answering_each(vec![
        // This observation offers only scroll: a click is refused by its schema.
        Ok(outcome(
            WorkModelStop::ToolUse,
            vec![tool(
                "act",
                json!({"actions": [{"kind": "click", "effect": "read",
                    "wait": {"kind": "immediate"},
                    "verification": {"kind": "page_changed"}, "settle_millis": 2000}]}),
            )],
        )),
        Ok(outcome(WorkModelStop::ToolUse, vec![locate()])),
    ]);
    let (mut policy, model, result) = run(client.clone()).await;
    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    let reason = reasked_reason(&calls[1]);
    assert!(
        reason.starts_with("invalid_tool_call: nothing ran."),
        "{reason}"
    );
    assert!(
        reason.contains("arguments.actions[0].kind") && reason.contains("scroll"),
        "{reason}"
    );
    assert!(matches!(
        calls[1].messages.iter().rev().nth(1),
        Some(WorkModelMessage::Assistant(parts))
            if parts.iter().any(|part| matches!(part, WorkModelPart::ToolCall(call) if call.name == "act"))
    ));
    assert_eq!(
        result.model_reply().map(|facts| facts.asked_again()),
        Some(1)
    );
    let AgentProviderPolicySettlement::PricingRequired(settlement) =
        result.into_policy_settlement()
    else {
        panic!("a corrected call is priced as reported");
    };
    let terminal = settlement
        .settle(&mut policy, model.schedule())
        .expect("settled");
    // Both asks are charged: the usage is their sum.
    assert_eq!(terminal.receipt().input_tokens(), 1_800);
    assert_eq!(
        terminal
            .into_tool_turn()
            .expect("tool turn")
            .proposal()
            .kind(),
        AgentBrowserToolKind::Locate
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reply_without_a_call_or_with_an_unoffered_tool_is_asked_again() {
    let client = Scripted::answering_each(vec![
        Ok(outcome(
            WorkModelStop::EndTurn,
            vec![WorkModelPart::Text("The page shows prices.".to_owned())],
        )),
        Ok(outcome(
            WorkModelStop::ToolUse,
            vec![tool("click_element", json!({"index": 3}))],
        )),
        Ok(outcome(WorkModelStop::ToolUse, vec![locate()])),
    ]);
    let (mut policy, model, result) = run(client.clone()).await;
    let calls = client.calls();
    assert_eq!(calls.len(), 3);
    assert!(reasked_reason(&calls[1]).contains("no_tool_call"));
    let unoffered = reasked_reason(&calls[2]);
    assert!(unoffered.contains("`click_element` is not a tool offered"));
    assert!(unoffered.contains("locate"));
    assert_eq!(
        result.model_reply().map(|facts| facts.asked_again()),
        Some(2)
    );
    let AgentProviderPolicySettlement::PricingRequired(settlement) =
        result.into_policy_settlement()
    else {
        panic!("priced");
    };
    let terminal = settlement
        .settle(&mut policy, model.schedule())
        .expect("settled");
    assert_eq!(terminal.receipt().input_tokens(), 2_700);
    assert!(terminal.has_tool_turn());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_runaway_reply_is_asked_once_more_briefly_inside_its_kept_back_output() {
    let mut runaway = outcome(
        WorkModelStop::MaxTokens,
        vec![WorkModelPart::Text(
            "Let me think about every listing".to_owned(),
        )],
    );
    runaway.usage.output_tokens = 6;
    let client = Scripted::answering_each(vec![
        Ok(runaway),
        Ok(outcome(WorkModelStop::ToolUse, vec![locate()])),
    ]);
    let (mut policy, model, result) = run(client.clone()).await;
    let calls = client.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].reasoning.is_some());
    let reason = reasked_reason(&calls[1]);
    assert!(reason.contains("output_limit: nothing ran"), "{reason}");
    assert!(reason.contains("Be brief"), "{reason}");
    // The runaway text is not handed back, thinking is off, and the second
    // ask may spend what the first left of the call's whole allowance.
    assert!(!calls[1].messages.iter().any(|message| matches!(
        message,
        WorkModelMessage::Assistant(parts)
            if parts.iter().any(|part| matches!(part, WorkModelPart::Text(text) if text.contains("every listing")))
    )));
    assert!(calls[1].reasoning.is_none());
    assert!(calls[0].max_output_tokens < 20);
    assert_eq!(calls[1].max_output_tokens, 20 - 6);
    assert_eq!(
        result.model_reply().map(|facts| facts.asked_again()),
        Some(1)
    );
    let AgentProviderPolicySettlement::PricingRequired(settlement) =
        result.into_policy_settlement()
    else {
        panic!("a re-asked runaway is priced as reported");
    };
    let terminal = settlement
        .settle(&mut policy, model.schedule())
        .expect("settled");
    assert_eq!(terminal.receipt().input_tokens(), 1_800);
    assert!(terminal.has_tool_turn());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_actions_have_one_repair_and_keep_failure_counts_when_terminal() {
    let bad = || {
        outcome(
            WorkModelStop::ToolUse,
            vec![tool(
                "act",
                json!({"actions":[{"kind":"scroll","target":"@a1","direction":"down","amount":"page","effect":"read","wait":{"kind":"immediate"},"verification":{"kind":"scroll_position_changed"},"settle_millis":0}]}),
            )],
        )
    };
    let client = Scripted::answering_each(vec![
        Ok(bad()),
        Ok(bad()),
        Ok(outcome(WorkModelStop::ToolUse, vec![locate()])),
    ]);
    let (mut policy, _, result) = run(client.clone()).await;
    assert_eq!(
        client.calls().len(),
        2,
        "third answer must never be requested"
    );
    let facts = result.model_reply().unwrap();
    assert_eq!(facts.asked_again(), 1);
    assert_eq!(
        facts.action_rejections(),
        [Some(crate::AgentBrowserToolContractError::Action); 2]
    );
    let AgentProviderPolicySettlement::Immediate(settlement) = result.into_policy_settlement()
    else {
        panic!("invalid call grants no tool");
    };
    settlement.settle(&mut policy).unwrap();
    assert_eq!(policy.pending_model_calls(), 0);
}

#[test]
fn local_page_models_reserve_and_settle_zero_cost() {
    let mut entry = entry("compatible/local");
    entry.model.provider = WorkModelProvider::Compatible;
    entry.price = Some(WorkModelPrice {
        input: 0,
        cached_input: 0,
        output: 0,
    });
    let model = AgentProviderModelClient::try_new(&entry, Scripted::stalled()).unwrap();
    assert_eq!(model.reservation_cost_micro_usd(100_000, 10_000), Some(0));
    assert_eq!(
        model.usage_cost_micro_usd(&WorkModelUsage {
            input_tokens: 1000,
            output_tokens: 100,
            cost_micros: Some(9000),
            ..Default::default()
        }),
        Some(0)
    );
}
