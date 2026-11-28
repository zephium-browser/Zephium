|wire| {
        let mut request = request(wire, "model-1");
        request.tool_choice = WorkModelToolChoice::Named("web_fetch".into());
        request
    };
    let openai = LeadTarget::direct(WorkModelProvider::OpenAi).unwrap();
    let body = openai::body(&named(WorkModelWire::OpenAiResponses), &openai).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "name": "web_fetch"})
    );

    let anthropic = LeadTarget::direct(WorkModelProvider::Anthropic).unwrap();
    let body = anthropic::body(&named(WorkModelWire::AnthropicMessages), &anthropic).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({"type": "tool", "name": "web_fetch", "disable_parallel_tool_use": false})
    );
    // Anthropic refuses a forced tool with thinking on.
    assert!(body.get("thinking").is_none());
    assert!(body.get("output_config").is_none());

    let body = gemini::body(&named(WorkModelWire::Gemini), &google()).unwrap();
    assert_eq!(
        body["toolConfig"]["functionCallingConfig"],
        json!({"mode": "ANY", "allowedFunctionNames": ["web_fetch"]})
    );
    assert!(body["generationConfig"].get("thinkingConfig").is_some());

    let chat = LeadTarget::direct(WorkModelProvider::DeepSeek).unwrap();
    let body = chat::body(&named(WorkModelWire::ChatCompletions), &chat).unwrap();
    assert_eq!(
        body["tool_choice"],
        json!({"type": "function", "function": {"name": "web_fetch"}})
    );
    // DeepSeek's thinking mode does not take a forced tool.
    assert_eq!(body["thinking"], json!({"type": "disabled"}));
    assert!(body.get("reasoning_effort").is_none());

    let mut required = request(WorkModelWire::AnthropicMessages, "claude-sonnet-5-5");
    required.tool_choice = WorkModelToolChoice::Required;
    let body = anthropic::body(&required, &anthropic).unwrap();
    assert_eq!(body["tool_choice"]["type"], "any");
    assert!(body.get("thinking").is_none());
    required.model.wire = WorkModelWire::OpenAiResponses;
    assert_eq!(
        openai::body(&required, &openai).unwrap()["tool_choice"],
        "required"
    );
    required.model.wire = WorkModelWire::Gemini;
    assert_eq!(
        gemini::body(&required, &google()).unwrap()["toolConfig"]["functionCallingConfig"],
        json!({"mode": "ANY"})
    );
    required.model.wire = WorkModelWire::ChatCompletions;
    assert_eq!(
        chat::body(&required, &chat).unwrap()["tool_choice"],
        "required"
    );
}

#[test]
fn a_forced_tool_the_request_does_not_offer_is_refused() {
    let mut request = request(WorkModelWire::OpenAiResponses, "model-1");
    request.tool_choice = WorkModelToolChoice::Named("navigate".into());
    let openai = LeadTarget::direct(WorkModelProvider::OpenAi).unwrap();
    assert_eq!(
        openai::body(&request, &openai).err(),
        Some(WorkModelError::BadRequest)
    );
    request.tools.clear();
    request.tool_choice = WorkModelToolChoice::Required;
    assert_eq!(
        tool_choice(&request).err(),
        Some(WorkModelError::BadRequest)
    );
    request.tool_choice = WorkModelToolChoice::Auto;
    let anthropic = LeadTarget::direct(WorkModelProvider::Anthropic).unwrap();
    assert!(anthropic::body(&request, &anthropic).is_ok());
}

#[test]
fn the_conversation_cache_closes_before_the_facts_that_change_every_call() {
    let mut next = request(WorkModelWire::AnthropicMessages, "claude-haiku-4-5");
    next.messages
        .push(WorkModelMessage::ToolResults(vec![WorkModelToolResult {
            call: "toolu_1".into(),
            content: "Clicked.".into(),
            is_error: false,
        }]));
    next.messages
        .push(WorkModelMessage::User(vec![WorkModelPart::Text(
            "[developer]\nnative_actions_remaining=2".into(),
        )]));
    next.volatile_tail = 1;
    let target = LeadTarget::direct(WorkModelProvider::Anthropic).unwrap();
    let body = anthropic::body(&next, &target).unwrap();
    let closing = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_array()
        .unwrap()
        .clone();
    let [.., result, facts] = closing.as_slice() else {
        panic!("a result and the facts close the conversation");
    };
    assert_eq!(result["type"], "tool_result");
    assert_eq!(result["cache_control"], json!({"type": "ephemeral"}));
    assert!(facts.get("cache_control").is_none());

    // With nothing volatile, the newest block closes the cache as before.
    next.volatile_tail = 0;
    let body = anthropic::body(&next, &target).unwrap();
    let closing = body["messages"].as_array().unwrap().last().unwrap()["content"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(
        closing.last().unwrap()["cache_control"],
        json!({"type": "ephemeral"})
    );
}

#[test]
fn openai_routes_one_conversation_to_one_cache_and_xai_gets_no_key() {
    let mut openai = request(WorkModelWire::OpenAiResponses, "gpt-6-luna");
    openai.cache_key = Some("zk_0123456789abcdef".into());
    let target = LeadTarget::direct(WorkModelProvider::OpenAi).unwrap();
    let body = openai::body(&openai, &target).unwrap();
    assert_eq!(body["prompt_cache_key"], "zk_0123456789abcdef");

    let mut xai = openai.clone();
    xai.model.provider = WorkModelProvider::XAi;
    let target = LeadTarget::direct(WorkModelProvider::XAi).unwrap();
    assert!(openai::body(&xai, &target)
        .unwrap()
        .get("prompt_cache_key")
        .is_none());
}

#[test]
fn deepseek_holds_strict_tools_to_their_schemas_on_its_beta_path() {
    let deepseek = LeadTarget::direct(WorkModelProvider::DeepSeek).unwrap();
    let mut strict = request(WorkModelWire::ChatCompletions, "deepseek-flash");
    strict.tools[0].strict = true;
    assert!(deepseek.strict_tools(&strict));
    assert_eq!(
        deepseek.call_url("deepseek-flash", true).unwrap().as_str(),
        "https://api.deepseek.com/beta/chat/completions"
    );
    let body = chat::body(&strict, &deepseek).unwrap();
    assert_eq!(body["tools"][0]["function"]["strict"], true);
    // Loose tools, other providers and Cloud keep the ordinary path and body.
    let loose = request(WorkModelWire::ChatCompletions, "deepseek-flash");
    assert!(!deepseek.strict_tools(&loose));
    assert!(
        chat::body(&loose, &deepseek).unwrap()["tools"][0]["function"]
            .get("strict")
            .is_none()
    );
    let mistral = LeadTarget::direct(WorkModelProvider::Mistral).unwrap();
    assert!(!mistral.strict_tools(&strict));
    let cloud = LeadTarget::cloud(ZEPHIUM_CLOUD_BASE, WorkModelProvider::DeepSeek).unwrap();
    assert!(!cloud.strict_tools(&strict));
}

#[test]
fn every_offered_effort_reaches_its_wire_request() {
    use WorkModelReasoning as E;
    let at = |provider: WorkModelProvider, wire, model: &str, effort| {
        let mut call = request(wire, model);
        call.model.provider = provider;
        call.reasoning = Some(effort);
        call.max_output_tokens = 32_000;
        let target = LeadTarget::direct(provider).unwrap();
        match wire {
            WorkModelWire::OpenAiResponses => openai::body(&call, &target),
            WorkModelWire::AnthropicMessages => anthropic::body(&call, &target),
            WorkModelWire::Gemini => gemini::body(&call, &google()),
            WorkModelWire::ChatCompletions => chat::body(&call, &target),
        }
        .unwrap()
    };
    let words = ["low", "medium", "high", "xhigh", "max"];
    let all = [E::Low, E::Medium, E::High, E::XHigh, E::Max];
    for (provider, wire, model) in [
        (
            WorkModelProvider::OpenAi,
            WorkModelWire::OpenAiResponses,
            "gpt-6-luna",
        ),
        (
            WorkModelProvider::XAi,
            WorkModelWire::OpenAiResponses,
            "grok-4.7",
        ),
    ] {
        for effort in models::efforts(provider, model) {
            let body = at(provider, wire, model, effort);
            assert_eq!(
                body["reasoning"]["effort"], words[effort as usize],
                "{model}"
            );
        }
    }
    let opus = models::efforts(WorkModelProvider::Anthropic, "claude-opus-5-5");
    assert_eq!(opus, all);
    for effort in opus {
        let body = at(
            WorkModelProvider::Anthropic,
            WorkModelWire::AnthropicMessages,
            "claude-opus-5-5",
            effort,
        );
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"]["effort"], words[effort as usize]);
    }
    let budgets: Vec<_> =
        models::efforts(WorkModelProvider::Anthropic, "claude-haiku-4-5-20251001")
            .into_iter()
            .map(|effort| {
                at(
                    WorkModelProvider::Anthropic,
                    WorkModelWire::AnthropicMessages,
                    "claude-haiku-4-5-20251001",
                    effort,
                )["thinking"]["budget_tokens"]
                    .clone()
            })
            .collect();
    assert_eq!(budgets, [json!(2_048), json!(6_000), json!(16_000)]);
    for model in ["gemini-3.1-pro-preview", "gemini-3.8-flash"] {
        for effort in models::efforts(WorkModelProvider::Google, model) {
            let body = at(
                WorkModelProvider::Google,
                WorkModelWire::Gemini,
                model,
                effort,
            );
            assert_eq!(
                body["generationConfig"]["thinkingConfig"]["thinkingLevel"], words[effort as usize],
                "{model}"
            );
        }
    }
    let deepseek: Vec<_> = models::efforts(WorkModelProvider::DeepSeek, "deepseek-v4-pro")
        .into_iter()
        .map(|effort| {
            at(
                WorkModelProvider::DeepSeek,
                WorkModelWire::ChatCompletions,
                "deepseek-v4-pro",
                effort,
            )["reasoning_effort"]
                .clone()
        })
        .collect();
    assert_eq!(deepseek, [json!("low"), json!("high")]);
    for effort in models::efforts(WorkModelProvider::Groq, "openai/gpt-oss-120b") {
        let body = at(
            WorkModelProvider::Groq,
            WorkModelWire::ChatCompletions,
            "openai/gpt-oss-120b",
            effort,
        );
        assert_eq!(body["reasoning_effort"], words[effort as usize]);
    }
    for effort in models::efforts(WorkModelProvider::OpenRouter, "openai/gpt-6-luna") {
        let body = at(
            WorkModelProvider::OpenRouter,
            WorkModelWire::ChatCompletions,
            "openai/gpt-6-luna",
            effort,
        );
        assert_eq!(body["reasoning"]["effort"], words[effort as usize]);
    }
}

#[test]
fn usage_counters_saturate_in_anthropic() {
    let fixture = named(&[
        (
            "message_start",
            json!({"type":"message_start","message":{"usage":{"input_tokens":u64::MAX,"cache_read_input_tokens":u64::MAX,"cache_creation_input_tokens":u64::MAX}}}),
        ),
        (
            "message_delta",
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"cache_read_input_tokens":u64::MAX}}),
        ),
        ("message_stop", json!({"type":"message_stop"})),
    ]);
    assert_eq!(
        decode(WorkModelWire::AnthropicMessages, &fixture)
            .1
            .unwrap()
            .usage
            .input_tokens,
        u64::MAX
    );
}

#[test]
fn usage_counters_saturate_in_gemini() {
    let fixture = data(&[
        json!({"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":u64::MAX,"toolUsePromptTokenCount":1,"candidatesTokenCount":u64::MAX,"thoughtsTokenCount":1}}),
    ]);
    let usage = decode(WorkModelWire::Gemini, &fixture).1.unwrap().usage;
    assert_eq!(usage.input_tokens, u64::MAX);
    assert_eq!(usage.output_tokens, u64::MAX);
}

#[test]
fn usage_counters_saturate_across_turns() {
    let make = || Decoded {
        usage: WorkModelUsage {
            input_tokens: u64::MAX,
            cached_input_tokens: u64::MAX,
            output_tokens: u64::MAX,
            reasoning_tokens: u64::MAX,
            ..Default::default()
        },
        cache_write_tokens: u64::MAX,
        searches: u32::MAX,
        provider_cost_micros: Some(u64::MAX),
        ..Default::default()
    };
    let mut total = make();
    total.absorb(make());
    assert_eq!(total.usage, make().usage);
    assert_eq!(total.cache_write_tokens, u64::MAX);
    assert_eq!(total.searches, u32::MAX);
    assert_eq!(total.provider_cost_micros, Some(u64::MAX));
}

#[test]
fn go_chat_forwards_every_effort_and_records_the_plan_allowance() {
    let go = LeadTarget::compatible("https://api.opencode.ai/inference/go/openai/v1").unwrap();
    let mut request = request(WorkModelWire::ChatCompletions, "glm-5.3-flash");
    request.model.provider = WorkModelProvider::Compatible;
    request.reasoning = Some(WorkModelReasoning::XHigh);
    let body = chat::body(&request, &go).unwrap();
    assert_eq!(body["reasoning_effort"], "xhigh");
    request.reasoning = Some(WorkModelReasoning::Max);
    assert_eq!(
        chat::body(&request, &go).unwrap()["reasoning_effort"],
        "max"
    );
    // A non-Go compatible endpoint keeps its old request shape.
    let plain = LeadTarget::compatible("https://host.example/v1").unwrap();
    assert!(chat::body(&request, &plain)
        .unwrap()
        .get("reasoning_effort")
        .is_none());

    let stream = data(&[json!({
        "choices": [{"delta": {"content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 2, "cost": 0,
                  "opencode": {"billing_source": "go-plus", "allowance": 0.00042}}
    })]);
    let mut decoder = WireDecoder::for_target(WorkModelWire::ChatCompletions, true);
    let mut framer = sse::SseFramer::default();
    let mut frames = Vec::new();
    for chunk in stream.as_bytes().chunks(29) {
        framer.push(chunk, &mut frames).unwrap();
    }
    framer.finish(&mut frames).unwrap();
    let mut out = Vec::new();
    for frame in frames {
        if matches!(decoder.event(&frame, &mut out), Ok(Flow::Done)) {
            break;
        }
    }
    let decoded = decoder.finish(&mut out).unwrap();
    assert_eq!(decoded.provider_cost_micros, Some(420));

    // An ordinary compatible endpoint never reads Go's usage metadata.
    let mut other = WireDecoder::new(WorkModelWire::ChatCompletions);
    let mut framer = sse::SseFramer::default();
    let mut frames = Vec::new();
    for chunk in stream.as_bytes().chunks(29) {
        framer.push(chunk, &mut frames).unwrap();
    }
    framer.finish(&mut frames).unwrap();
    let mut out = Vec::new();
    for frame in frames {
        if matches!(other.event(&frame, &mut out), Ok(Flow::Done)) {
            break;
        }
    }
    assert_eq!(
        other.finish(&mut out).unwrap().provider_cost_micros,
        Some(0)
    );
}
