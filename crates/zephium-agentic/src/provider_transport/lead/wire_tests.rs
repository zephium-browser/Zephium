//! Recorded-stream tests for the Anthropic, Gemini and chat wires.

use serde_json::{json, Value};

use super::tests::request;
use super::*;

fn google() -> LeadTarget {
    LeadTarget::direct(WorkModelProvider::Google).unwrap()
}

fn decode(
    wire: WorkModelWire,
    fixture: &str,
) -> (Vec<WorkModelEvent>, Result<Decoded, WorkModelError>) {
    let mut decoder = WireDecoder::new(wire);
    let mut framer = sse::SseFramer::default();
    let mut frames = Vec::new();
    let mut out = Vec::new();
    for chunk in fixture.as_bytes().chunks(29) {
        framer.push(chunk, &mut frames).unwrap();
    }
    framer.finish(&mut frames).unwrap();
    for frame in frames.drain(..) {
        match decoder.event(&frame, &mut out) {
            Ok(Flow::Continue) => {}
            Ok(Flow::Done) => break,
            Err(error) => return (out, Err(error)),
        }
    }
    let decoded = decoder.finish(&mut out);
    (out, decoded)
}

fn named(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

fn data(chunks: &[Value]) -> String {
    let mut stream: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect();
    stream.push_str("data: [DONE]\n\n");
    stream
}

fn text_of(events: &[WorkModelEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            WorkModelEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn call_events(events: &[WorkModelEvent]) -> Vec<(String, String, Value)> {
    events
        .iter()
        .filter_map(|event| match event {
            WorkModelEvent::ToolCall(call) => {
                Some((call.id.clone(), call.name.clone(), call.arguments.clone()))
            }
            _ => None,
        })
        .collect()
}

fn search_events(events: &[WorkModelEvent]) -> Vec<(String, Vec<String>, Vec<String>)> {
    events
        .iter()
        .filter_map(|event| match event {
            WorkModelEvent::Search { query, hits } => Some((
                query.clone(),
                hits.iter().map(|hit| hit.url.clone()).collect(),
                hits.iter().map(|hit| hit.snippet.clone()).collect(),
            )),
            _ => None,
        })
        .collect()
}

fn anthropic_turn() -> String {
    named(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {
            "input_tokens": 200, "cache_read_input_tokens": 9000, "cache_creation_input_tokens": 800, "output_tokens": 1}}}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Need dates."}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQBsig"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "server_tool_use", "id": "srvtoolu_1", "name": "web_search", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"query\": \"SFO BART"}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": " fare\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 1}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "web_search_tool_result", "tool_use_id": "srvtoolu_1", "content": [
            {"type": "web_search_result", "url": "https://www.bart.gov/tickets", "title": "Fares | BART", "encrypted_content": "Eu8B", "page_age": null}]}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 2}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "text", "text": ""}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 3, "delta": {"type": "text_delta", "text": "BART is about $10."}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 3, "delta": {"type": "citations_delta", "citation": {
            "type": "web_search_result_location", "url": "https://www.bart.gov/tickets", "title": "Fares | BART", "cited_text": "SFO to downtown: $10.20", "encrypted_index": "Eo8"}}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 3}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 4, "content_block": {"type": "tool_use", "id": "toolu_a", "name": "web_fetch", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 4, "delta": {"type": "input_json_delta", "partial_json": "{\"url\":\"https://a.example\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 4}),
        ),
        (
            "content_block_start",
            json!({"type": "content_block_start", "index": 5, "content_block": {"type": "tool_use", "id": "toolu_b", "name": "web_fetch", "input": {}}}),
        ),
        (
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 5, "delta": {"type": "input_json_delta", "partial_json": "{\"url\":\"https://b.example\"}"}}),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 5}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {
            "output_tokens": 420, "server_tool_use": {"web_search_requests": 1}}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ])
}

#[test]
fn anthropic_streams_thinking_search_and_parallel_tool_use() {
    let (events, decoded) = decode(WorkModelWire::AnthropicMessages, &anthropic_turn());
    let decoded = decoded.unwrap();
    assert_eq!(text_of(&events), "BART is about $10.");
    assert_eq!(
        call_events(&events),
        vec![
            (
                "toolu_a".into(),
                "web_fetch".into(),
                json!({"url": "https://a.example"})
            ),
            (
                "toolu_b".into(),
                "web_fetch".into(),
                json!({"url": "https://b.example"})
            ),
        ]
    );
    assert_eq!(
        search_events(&events),
        vec![(
            "SFO BART fare".into(),
            vec!["https://www.bart.gov/tickets".into()],
            vec!["SFO to downtown: $10.20".into()]
        )]
    );
    assert_eq!(decoded.stop, DecodedStop::End(WorkModelStop::ToolUse));
    assert_eq!(decoded.usage.input_tokens, 10_000);
    assert_eq!(decoded.usage.cached_input_tokens, 9_000);
    assert_eq!(decoded.cache_write_tokens, 800);
    assert_eq!(decoded.usage.output_tokens, 420);
    assert_eq!(decoded.searches, 1);

    // The next request replays thinking (with its signature) and the search
    // blocks verbatim, then answers both tool calls in one user turn.
    let mut next = request(WorkModelWire::AnthropicMessages, "claude-opus-5-5");
    next.messages
        .push(WorkModelMessage::Assistant(decoded.assistant));
    next.messages.push(WorkModelMessage::ToolResults(vec![
        WorkModelToolResult {
            call: "toolu_a".into(),
            content: "A".into(),
            is_error: false,
        },
        WorkModelToolResult {
            call: "toolu_b".into(),
            content: "gone".into(),
            is_error: true,
        },
    ]));
    let target = LeadTarget::direct(WorkModelProvider::Anthropic).unwrap();
    let body = anthropic::body(&next, &target).unwrap();
    let assistant = body["messages"][1]["content"].as_array().unwrap();
    let kinds: Vec<&str> = assistant
        .iter()
        .map(|block| block["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "thinking",
            "server_tool_use",
            "web_search_tool_result",
            "text",
            "tool_use",
            "tool_use"
        ]
    );
    assert_eq!(assistant[0]["signature"], "EqQBsig");
    assert_eq!(assistant[1]["input"], json!({"query": "SFO BART fare"}));
    assert_eq!(assistant[2]["content"][0]["encrypted_content"], "Eu8B");
    let results = body["messages"][2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[1]["is_error"], true);
    assert_eq!(results[1]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(
        body["system"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert!(body["system"][1].get("cache_control").is_none());
    assert_eq!(body["messages"][0]["content"][1]["source"]["data"], "AQID");
    assert_eq!(body["thinking"], json!({"type": "adaptive"}));
    assert_eq!(body["output_config"]["effort"], "medium");
    assert_eq!(body["tools"][1]["type"], "web_search_20250305");
    assert_eq!(body["tool_choice"]["disable_parallel_tool_use"], false);

    let haiku = anthropic::body(
        &request(
            WorkModelWire::AnthropicMessages,
            "claude-haiku-4-5-20251001",
        ),
        &target,
    )
    .unwrap();
    assert_eq!(
        haiku["thinking"],
        json!({"type": "enabled", "budget_tokens": 3_072})
    );
    assert_eq!(haiku["tools"][1]["type"], "web_search_20250305");

    // Listed 4.5 models refuse adaptive thinking; 4.6 and later refuse a budget.
    for (model, budget) in [
        ("claude-sonnet-4-5-20250929", true),
        ("claude-opus-4-5-20251101", true),
        ("claude-opus-4-6", false),
        ("claude-sonnet-5-5", false),
    ] {
        let body =
            anthropic::body(&request(WorkModelWire::AnthropicMessages, model), &target).unwrap();
        assert_eq!(body["thinking"]["type"] == "enabled", budget, "{model}");
    }
}

#[test]
fn anthropic_pauses_refusals_and_errors_close() {
    let paused = named(&[
        (
            "message_start",
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 5, "output_tokens": 1}}}),
        ),
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "pause_turn"}, "usage": {"output_tokens": 3}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    assert_eq!(
        decode(WorkModelWire::AnthropicMessages, &paused)
            .1
            .unwrap()
            .stop,
        DecodedStop::Paused
    );

    let refused = named(&[
        (
            "message_delta",
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal"}, "usage": {"output_tokens": 0}}),
        ),
        ("message_stop", json!({"type": "message_stop"})),
    ]);
    assert_eq!(
        decode(WorkModelWire::AnthropicMessages, &refused)
            .1
            .unwrap()
            .stop,
        DecodedStop::End(WorkModelStop::Refused)
    );

    let overloaded = named(&[(
        "error",
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}),
    )]);
    assert_eq!(
        decode(WorkModelWire::AnthropicMessages, &overloaded)
            .1
            .err(),
        Some(WorkModelError::Overloaded)
    );
}

#[test]
fn gemini_streams_signed_calls_grounding_and_usage() {
    let fixture = data(&[
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "Flights from WAW ", "thoughtSignature": "sig-text"}]}}]}),
        json!({"candidates": [{"content": {"role": "model", "parts": [{"text": "start at $610."}]}}]}),
        json!({"candidates": [{"content": {"role": "model", "parts": [
            {"functionCall": {"name": "web_fetch", "args": {"url": "https://a.example"}}, "thoughtSignature": "sig-call"},
            {"functionCall": {"name": "web_fetch", "args": {"url": "https://b.example"}}}
        ]}, "finishReason": "STOP", "groundingMetadata": {
            "webSearchQueries": ["WAW SFO flights January"],
            "groundingChunks": [{"web": {"uri": "https://vertexaisearch.cloud.google.com/grounding-api-redirect/abc", "title": "google.com"}}],
            "groundingSupports": [{"segment": {"text": "start at $610."}, "groundingChunkIndices": [0]}]}}],
         "usageMetadata": {"promptTokenCount": 3000, "cachedContentTokenCount": 2048, "candidatesTokenCount": 80, "thoughtsTokenCount": 120, "toolUsePromptTokenCount": 400}}),
    ]);
    let (events, decoded) = decode(WorkModelWire::Gemini, &fixture);
    let decoded = decoded.unwrap();
    assert_eq!(text_of(&events), "Flights from WAW start at $610.");
    let calls = call_events(&events);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, "gemini_call_0");
    assert_eq!(calls[1].2, json!({"url": "https://b.example"}));
    assert_eq!(
        search_events(&events),
        vec![(
            "WAW SFO flights January".into(),
            vec!["https://vertexaisearch.cloud.google.com/grounding-api-redirect/abc".into()],
            vec!["start at $610.".into()]
        )]
    );
    assert_eq!(decoded.stop, DecodedStop::End(WorkModelStop::ToolUse));
    assert_eq!(decoded.usage.input_tokens, 3_400);
    assert_eq!(decoded.usage.cached_input_tokens, 2_048);
    assert_eq!(decoded.usage.output_tokens, 200);
    assert_eq!(decoded.searches, 1);

    let mut next = request(WorkModelWire::Gemini, "gemini-3.8-flash");
    next.messages
        .push(WorkModelMessage::Assistant(decoded.assistant));
    next.messages.push(WorkModelMessage::ToolResults(vec![
        WorkModelToolResult {
            call: "gemini_call_0".into(),
            content: "A".into(),
            is_error: false,
        },
        WorkModelToolResult {
            call: "gemini_call_1".into(),
            content: "gone".into(),
            is_error: true,
        },
    ]));
    let body = gemini::body(&next, &google()).unwrap();
    let model = body["contents"][1]["parts"].as_array().unwrap();
    assert_eq!(model.len(), 3);
    assert_eq!(model[0]["text"], "Flights from WAW start at $610.");
    assert_eq!(model[0]["thoughtSignature"], "sig-text");
    assert_eq!(model[1]["thoughtSignature"], "sig-call");
    let responses = body["contents"][2]["parts"].as_array().unwrap();
    assert_eq!(
        responses[0]["functionResponse"],
        json!({"name": "web_fetch", "response": {"result": "A"}})
    );
    assert_eq!(
        responses[1]["functionResponse"]["response"],
        json!({"error": "gone"})
    );
    assert_eq!(
        body["contents"][0]["parts"][1]["inlineData"]["data"],
        "AQID"
    );
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        "Core prompt.\n\nWork context."
    );
    assert_eq!(
        body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]["required"],
        json!(["url"])
    );
    assert_eq!(body["tools"][1], json!({"googleSearch": {}}));
    assert_eq!(
        body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "medium"
    );
}

#[test]
fn gemini_blocks_and_errors_close() {
    let blocked = data(&[json!({"promptFeedback": {"blockReason": "SAFETY"}})]);
    assert_eq!(
        decode(WorkModelWire::Gemini, &blocked).1.unwrap().stop,
        DecodedStop::End(WorkModelStop::Refused)
    );
    let capped = data(&[
        json!({"candidates": [{"content": {"parts": [{"text": "a"}]}, "finishReason": "MAX_TOKENS"}]}),
    ]);
    assert_eq!(
        decode(WorkModelWire::Gemini, &capped).1.unwrap().stop,
        DecodedStop::End(WorkModelStop::MaxTokens)
    );
    let exhausted = data(&[
        json!({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "message": "quota"}}),
    ]);
    assert_eq!(
        decode(WorkModelWire::Gemini, &exhausted).1.err(),
        Some(WorkModelError::RateLimited {
            retry_after_ms: None
        })
    );
}

#[test]
fn deepseek_streams_reasoning_and_parallel_calls_and_replays_reasoning() {
    let fixture = data(&[
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "reasoning_content": "Two pages "}}]}),
        json!({"choices": [{"index": 0, "delta": {"reasoning_content": "to read."}}]}),
        json!({"choices": [{"index": 0, "delta": {"content": "Reading both."}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [
            {"index": 0, "id": "call_0", "type": "function", "function": {"name": "web_fetch", "arguments": "{\"url\":"}},
            {"index": 1, "id": "call_1", "type": "function", "function": {"name": "web_fetch", "arguments": ""}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [
            {"index": 0, "function": {"arguments": "\"https://a.example\"}"}},
            {"index": 1, "function": {"arguments": "{\"url\":\"https://b.example\"}"}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
        json!({"choices": [], "usage": {"prompt_tokens": 5000, "completion_tokens": 300,
            "prompt_cache_hit_tokens": 4800, "prompt_cache_miss_tokens": 200,
            "completion_tokens_details": {"reasoning_tokens": 120}}}),
    ]);
    let (events, decoded) = decode(WorkModelWire::ChatCompletions, &fixture);
    let decoded = decoded.unwrap();
    assert_eq!(text_of(&events), "Reading both.");
    assert_eq!(
        call_events(&events),
        vec![
            (
                "call_0".into(),
                "web_fetch".into(),
                json!({"url": "https://a.example"})
            ),
            (
                "call_1".into(),
                "web_fetch".into(),
                json!({"url": "https://b.example"})
            ),
        ]
    );
    assert_eq!(decoded.stop, DecodedStop::End(WorkModelStop::ToolUse));
    assert_eq!(decoded.usage.cached_input_tokens, 4_800);
    assert_eq!(decoded.usage.reasoning_tokens, 120);

    let target = LeadTarget::direct(WorkModelProvider::DeepSeek).unwrap();
    let mut next = request(WorkModelWire::ChatCompletions, "deepseek-v4-pro");
    next.native_search = false;
    next.messages
        .push(WorkModelMessage::Assistant(decoded.assistant));
    next.messages.push(WorkModelMessage::ToolResults(vec![
        WorkModelToolResult {
            call: "call_0".into(),
            content: "A".into(),
            is_error: false,
        },
        WorkModelToolResult {
            call: "call_1".into(),
            content: "B".into(),
            is_error: false,
        },
    ]));
    let body = chat::body(&next, &target).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages[0],
        json!({"role": "system", "content": "Core prompt.\n\nWork context."})
    );
    assert_eq!(
        messages[1]["content"][1]["image_url"]["url"],
        "data:image/png;base64,AQID"
    );
    assert_eq!(messages[2]["reasoning_content"], "Two pages to read.");
    assert_eq!(messages[2]["content"], "Reading both.");
    assert_eq!(
        messages[2]["tool_calls"][1]["function"]["arguments"],
        "{\"url\":\"https://b.example\"}"
    );
    assert_eq!(
        messages[3],
        json!({"role": "tool", "tool_call_id": "call_0", "content": "A"})
    );
    assert_eq!(body["thinking"], json!({"type": "enabled"}));
    assert_eq!(body["reasoning_effort"], "high");
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

#[test]
fn openrouter_reports_its_own_cost_and_keeps_reasoning_details() {
    let fixture = data(&[
        json!({"choices": [{"delta": {"reasoning_details": [{"type": "reasoning.text", "index": 0, "text": "Check ", "format": "anthropic-claude-v1"}]}}]}),
        json!({"choices": [{"delta": {"reasoning_details": [{"type": "reasoning.text", "index": 0, "text": "prices", "signature": "sig"}]}}]}),
        json!({"choices": [{"delta": {"content": "Done."}, "finish_reason": "stop"}]}),
        json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 10, "cost": 0.00042,
            "prompt_tokens_details": {"cached_tokens": 60}}}),
    ]);
    let (_, decoded) = decode(WorkModelWire::ChatCompletions, &fixture);
    let decoded = decoded.unwrap();
    assert_eq!(decoded.stop, DecodedStop::End(WorkModelStop::EndTurn));
    assert_eq!(decoded.provider_cost_micros, Some(420));
    assert_eq!(decoded.usage.cached_input_tokens, 60);
    let details = replayed(&decoded.assistant[0], "chat").unwrap();
    assert_eq!(
        details["reasoning_details"],
        json!([{"type": "reasoning.text", "index": 0, "text": "Check prices", "format": "anthropic-claude-v1", "signature": "sig"}])
    );
    let target = LeadTarget::direct(WorkModelProvider::OpenRouter).unwrap();
    let body = chat::body(
        &request(WorkModelWire::ChatCompletions, "moonshotai/kimi-k3"),
        &target,
    )
    .unwrap();
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
    assert_eq!(body["reasoning"], json!({"effort": "medium"}));

    let failed = data(&[json!({"error": {"code": 502, "message": "upstream"}})]);
    assert_eq!(
        decode(WorkModelWire::ChatCompletions, &failed).1.err(),
        Some(WorkModelError::Overloaded)
    );
    let truncated = "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n";
    assert_eq!(
        decode(WorkModelWire::ChatCompletions, truncated).1.err(),
        Some(WorkModelError::Protocol)
    );
}

#[test]
fn replay_from_another_wire_is_dropped() {
    let mut next = request(WorkModelWire::AnthropicMessages, "claude-sonnet-5-5");
    next.messages.push(WorkModelMessage::Assistant(vec![
        replay(
            "openai",
            json!({"type": "reasoning", "encrypted_content": "x"}),
        ),
        WorkModelPart::Text("Hi".into()),
    ]));
    let target = LeadTarget::direct(WorkModelProvider::Anthropic).unwrap();
    let body = anthropic::body(&next, &target).unwrap();
    assert_eq!(
        body["messages"][1]["content"],
        json!([{"type": "text", "text": "Hi", "cache_control": {"type": "ephemeral"}}])
    );
}

#[test]
fn mistral_streams_thinking_chunks_and_gets_them_back_as_a_chunk() {
    let fixture = data(&[
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": [{"type": "text", "text": "Check the page."}]}]}}]}),
        json!({"choices": [{"index": 0, "delta": {"content": [{"type": "text", "text": "Done."}]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20}}),
    ]);
    let (events, decoded) = decode(WorkModelWire::ChatCompletions, &fixture);
    let decoded = decoded.unwrap();
    assert_eq!(text_of(&events), "Done.");
    assert_eq!(decoded.usage.input_tokens, 100);

    let target = LeadTarget::direct(WorkModelProvider::Mistral).unwrap();
    let mut next = request(WorkModelWire::ChatCompletions, "mistral-medium-3-5");
    next.native_search = false;
    next.messages
        .push(WorkModelMessage::Assistant(decoded.assistant));
    let body = chat::body(&next, &target).unwrap();
    let last = body["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(
        last["content"],
        json!([
            {"type": "thinking", "thinking": [{"type": "text", "text": "Check the page."}]},
            {"type": "text", "text": "Done."}
        ])
    );
    assert!(last.get("thinking").is_none());
    assert!(body.get("stream_options").is_none());
    assert!(body.get("reasoning_effort").is_none());
}

#[test]
fn groq_and_cerebras_take_effort_only_where_their_models_accept_it() {
    let body = |provider, model: &str| {
        let mut call = request(WorkModelWire::ChatCompletions, model);
        call.native_search = false;
        call.reasoning = Some(WorkModelReasoning::Medium);
        call.parallel_tools = false;
        call.tools = vec![WorkModelTool {
            name: "read".into(),
            description: "Read a page.".into(),
            schema: json!({"type": "object"}),
            strict: false,
        }];
        chat::body(&call, &LeadTarget::direct(provider).unwrap()).unwrap()
    };
    let groq = body(WorkModelProvider::Groq, "openai/gpt-oss-120b");
    assert_eq!(groq["reasoning_effort"], "medium");
    assert_eq!(groq["parallel_tool_calls"], false);
    let groq_qwen = body(WorkModelProvider::Groq, "qwen/qwen3.8-27b");
    assert_eq!(groq_qwen["reasoning_effort"], "medium");
    assert_eq!(groq_qwen["reasoning_format"], "hidden");
    assert!(groq.get("reasoning_format").is_none());
    let cerebras = body(WorkModelProvider::Cerebras, "qwen-3.8-27b");
    assert_eq!(cerebras["reasoning_effort"], "medium");
    assert!(cerebras.get("parallel_tool_calls").is_none());
    assert_eq!(cerebras["stream_options"], json!({"include_usage": true}));
}

#[test]
fn new_providers_reach_their_own_endpoints() {
    for (provider, url) in [
        (WorkModelProvider::XAi, "https://api.x.ai/v1/responses"),
        (
            WorkModelProvider::Mistral,
            "https://api.mistral.ai/v1/chat/completions",
        ),
        (
            WorkModelProvider::Groq,
            "https://api.groq.com/openai/v1/chat/completions",
        ),
        (
            WorkModelProvider::Cerebras,
            "https://api.cerebras.ai/v1/chat/completions",
        ),
    ] {
        let target = LeadTarget::direct(provider).unwrap();
        assert_eq!(target.call_url("model", false).unwrap().as_str(), url);
        assert!(LeadTarget::cloud(ZEPHIUM_CLOUD_BASE, provider).is_err());
    }
    let xai = openai::body(
        &request(WorkModelWire::OpenAiResponses, "grok-4.7"),
        &LeadTarget::direct(WorkModelProvider::XAi).unwrap(),
    )
    .unwrap();
    assert_eq!(xai["store"], false);
    assert_eq!(xai["include"], json!(["reasoning.encrypted_content"]));
}

#[test]
fn every_wire_forces_the_tool_it_is_told_to() {
    let named = |wire| {
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
