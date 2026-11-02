use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::Mutex;

use serde_json::{json, Value};

use super::*;

/// Runs one recorded stream through the wire's decoder, split into odd-sized
/// chunks so framing across reads is exercised too.
fn decode(
    wire: WorkModelWire,
    fixture: &str,
) -> (Vec<WorkModelEvent>, Result<Decoded, WorkModelError>) {
    let mut decoder = WireDecoder::new(wire);
    let mut framer = sse::SseFramer::default();
    let mut frames = Vec::new();
    let mut out = Vec::new();
    for chunk in fixture.as_bytes().chunks(37) {
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

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| format!("event: {name}\ndata: {data}\n\n"))
        .collect()
}

pub(super) fn request(wire: WorkModelWire, model: &str) -> WorkModelRequest {
    WorkModelRequest {
        image_fence: ModelImageGate::new(Some(true)).fence(),
        model: WorkModelRef {
            provider: WorkModelProvider::OpenAi,
            wire,
            model: model.into(),
        },
        system: vec![
            WorkModelSystemBlock {
                text: "Core prompt.".into(),
                cache: true,
            },
            WorkModelSystemBlock {
                text: "Work context.".into(),
                cache: false,
            },
        ],
        tools: vec![WorkModelTool {
            name: "web_fetch".into(),
            description: "Read a page.".into(),
            schema: json!({"type": "object", "properties": {"url": {"type": "string"}}, "required": ["url"]}),
            strict: false,
        }],
        messages: vec![WorkModelMessage::User(vec![
            WorkModelPart::Text("Plan my trip".into()),
            WorkModelPart::Image {
                media_type: "image/png".into(),
                bytes: vec![1, 2, 3],
            },
        ])],
        max_output_tokens: 4_096,
        reasoning: Some(WorkModelReasoning::Medium),
        native_search: true,
        parallel_tools: true,
        tool_choice: WorkModelToolChoice::Auto,
        volatile_tail: 0,
        cache_key: None,
    }
}

fn texts(events: &[WorkModelEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            WorkModelEvent::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn calls(parts: &[WorkModelPart]) -> Vec<(String, String, Value)> {
    parts
        .iter()
        .filter_map(|part| match part {
            WorkModelPart::ToolCall(call) => {
                Some((call.id.clone(), call.name.clone(), call.arguments.clone()))
            }
            _ => None,
        })
        .collect()
}

type Hits = Vec<(String, String, String)>;

fn searches(events: &[WorkModelEvent]) -> Vec<(String, Hits)> {
    events
        .iter()
        .filter_map(|event| match event {
            WorkModelEvent::Search { query, hits } => Some((
                query.clone(),
                hits.iter()
                    .map(|hit| (hit.url.clone(), hit.title.clone(), hit.snippet.clone()))
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn openai_streams_text_parallel_calls_reasoning_and_search() {
    let reasoning =
        json!({"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "gAAA"});
    let search = json!({"type": "web_search_call", "id": "ws_1", "status": "completed",
        "action": {"type": "search", "query": "YC batch dates 2027",
            "sources": [{"type": "url", "url": "https://www.ycombinator.com/apply"}]}});
    let fixture = sse(&[
        (
            "response.created",
            json!({"type": "response.created", "response": {"status": "in_progress"}}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 0, "item": reasoning}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 1, "item": search}),
        ),
        (
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "Batch starts "}),
        ),
        (
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "in January."}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 2, "item": {
            "type": "message", "role": "assistant", "content": [{"type": "output_text",
            "text": "Batch starts in January.", "annotations": [{"type": "url_citation",
            "url": "https://www.ycombinator.com/apply", "title": "Apply to Y Combinator",
            "start_index": 0, "end_index": 24}]}]}}),
        ),
        (
            "response.function_call_arguments.delta",
            json!({"type": "response.function_call_arguments.delta", "delta": "{\"url\""}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 3, "item": {
            "type": "function_call", "call_id": "call_a", "name": "web_fetch", "arguments": "{\"url\":\"https://a.example\"}"}}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "output_index": 4, "item": {
            "type": "function_call", "call_id": "call_b", "name": "web_fetch", "arguments": "{\"url\":\"https://b.example\"}"}}),
        ),
        (
            "response.completed",
            json!({"type": "response.completed", "response": {"status": "completed", "usage": {
            "input_tokens": 12000, "input_tokens_details": {"cached_tokens": 10000},
            "output_tokens": 900, "output_tokens_details": {"reasoning_tokens": 600}}}}),
        ),
    ]);
    let (events, decoded) = decode(WorkModelWire::OpenAiResponses, &fixture);
    let decoded = decoded.unwrap();
    assert_eq!(texts(&events), "Batch starts in January.");
    assert_eq!(
        searches(&events),
        vec![(
            "YC batch dates 2027".to_owned(),
            vec![(
                "https://www.ycombinator.com/apply".to_owned(),
                "Apply to Y Combinator".to_owned(),
                "Batch starts in January.".to_owned()
            )]
        )]
    );
    assert_eq!(decoded.stop, DecodedStop::End(WorkModelStop::ToolUse));
    assert_eq!(decoded.searches, 1);
    assert_eq!(
        calls(&decoded.assistant),
        vec![
            (
                "call_a".into(),
                "web_fetch".into(),
                json!({"url": "https://a.example"})
            ),
            (
                "call_b".into(),
                "web_fetch".into(),
                json!({"url": "https://b.example"})
            ),
        ]
    );
    let tool_events = events
        .iter()
        .filter(|event| matches!(event, WorkModelEvent::ToolCall(_)))
        .count();
    assert_eq!(tool_events, 2);
    assert_eq!(decoded.usage.input_tokens, 12_000);
    assert_eq!(decoded.usage.cached_input_tokens, 10_000);
    assert_eq!(decoded.usage.reasoning_tokens, 600);
    // The reasoning and search items come back first, verbatim, on the next call.
    assert!(matches!(&decoded.assistant[0], part if replayed(part, "openai") == Some(&reasoning)));
    let mut next = request(WorkModelWire::OpenAiResponses, "gpt-6-sol");
    next.messages
        .push(WorkModelMessage::Assistant(decoded.assistant.clone()));
    next.messages.push(WorkModelMessage::ToolResults(vec![
        WorkModelToolResult {
            call: "call_a".into(),
            content: "A".into(),
            is_error: false,
        },
        WorkModelToolResult {
            call: "call_b".into(),
            content: "gone".into(),
            is_error: true,
        },
    ]));
    let target = LeadTarget::direct(WorkModelProvider::OpenAi).unwrap();
    let body = openai::body(&next, &target).unwrap();
    let input = body["input"].as_array().unwrap();
    let kinds: Vec<&str> = input
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "message",
            "reasoning",
            "web_search_call",
            "message",
            "function_call",
            "function_call",
            "function_call_output",
            "function_call_output"
        ]
    );
    assert_eq!(input[1]["encrypted_content"], "gAAA");
    assert_eq!(
        input[0]["content"][1]["image_url"],
        "data:image/png;base64,AQID"
    );
    assert_eq!(input[7]["output"], "Error: gone");
    assert_eq!(body["store"], false);
    assert_eq!(body["instructions"], "Core prompt.\n\nWork context.");
    assert_eq!(body["reasoning"]["effort"], "medium");
    assert_eq!(body["tools"][1]["type"], "web_search");
    assert_eq!(
        body["include"],
        json!([
            "reasoning.encrypted_content",
            "web_search_call.action.sources"
        ])
    );
}

#[test]
fn openai_limits_and_failures_close() {
    let incomplete = sse(&[(
        "response.incomplete",
        json!({"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"},
            "usage": {"input_tokens": 5, "output_tokens": 9}}}),
    )]);
    let (_, decoded) = decode(WorkModelWire::OpenAiResponses, &incomplete);
    assert_eq!(
        decoded.unwrap().stop,
        DecodedStop::End(WorkModelStop::MaxTokens)
    );

    let failed = sse(&[(
        "response.failed",
        json!({"type": "response.failed", "response": {"error": {"code": "server_error", "message": "secret words"}}}),
    )]);
    let (_, decoded) = decode(WorkModelWire::OpenAiResponses, &failed);
    assert_eq!(decoded.err(), Some(WorkModelError::Overloaded));

    let refused = sse(&[
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "item": {
            "type": "message", "content": [{"type": "refusal", "refusal": "no"}]}}),
        ),
        (
            "response.completed",
            json!({"type": "response.completed", "response": {"usage": {}}}),
        ),
    ]);
    let (_, decoded) = decode(WorkModelWire::OpenAiResponses, &refused);
    assert_eq!(
        decoded.unwrap().stop,
        DecodedStop::End(WorkModelStop::Refused)
    );

    let truncated = sse(&[(
        "response.output_text.delta",
        json!({"type": "response.output_text.delta", "delta": "half"}),
    )]);
    let (_, decoded) = decode(WorkModelWire::OpenAiResponses, &truncated);
    assert_eq!(decoded.err(), Some(WorkModelError::Protocol));

    let malformed = sse(&[
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "item": {"type": "function_call",
            "call_id": "c", "name": "web_fetch", "arguments": "{\"url\": "}}),
        ),
        (
            "response.completed",
            json!({"type": "response.completed", "response": {}}),
        ),
    ]);
    let (_, decoded) = decode(WorkModelWire::OpenAiResponses, &malformed);
    // The loop's schema check hands the model a fault it can correct.
    assert_eq!(calls(&decoded.unwrap().assistant)[0].2, json!("{\"url\": "));
}

#[test]
fn http_failures_classify_without_provider_text() {
    let body = br#"{"error":{"type":"invalid_request_error","message":"prompt is too long: 1200000 tokens"}}"#;
    assert_eq!(
        classify(StatusCode::BAD_REQUEST, body),
        WorkModelError::ContextTooLong
    );
    assert_eq!(
        classify(StatusCode::BAD_REQUEST, b"{}"),
        WorkModelError::BadRequest
    );
    assert_eq!(
        classify(StatusCode::UNAUTHORIZED, b""),
        WorkModelError::Unauthorized
    );
    assert_eq!(
        classify(StatusCode::from_u16(529).unwrap(), b""),
        WorkModelError::Overloaded
    );
    let quota = br#"{"error":{"code":"insufficient_quota"}}"#;
    assert_eq!(
        classify(StatusCode::TOO_MANY_REQUESTS, quota),
        WorkModelError::OverBudget
    );
    let credit = br#"{"error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the API."}}"#;
    assert_eq!(
        classify(StatusCode::BAD_REQUEST, credit),
        WorkModelError::OverBudget
    );
    assert_eq!(
        stream_error("credit_balance_exhausted"),
        WorkModelError::OverBudget
    );
    let groq_plan = br#"{"error":{"message":"Request too large for model `openai/gpt-oss-120b` in organization `org_x` service tier `on_demand` on tokens per minute (TPM): Limit 8000, Requested 15847, please reduce your message size and try again.","type":"tokens","code":"rate_limit_exceeded"}}"#;
    assert_eq!(
        classify(StatusCode::PAYLOAD_TOO_LARGE, groq_plan),
        WorkModelError::PlanLimit
    );
    let openai_plan = br#"{"error":{"message":"Request too large for gpt-5 in organization org-x on tokens per min (TPM): Limit 30000, Requested 52000.","type":"tokens","code":"rate_limit_exceeded"}}"#;
    assert_eq!(
        classify(StatusCode::TOO_MANY_REQUESTS, openai_plan),
        WorkModelError::PlanLimit
    );
    let reached = br#"{"error":{"message":"Rate limit reached for gpt-5 on tokens per min (TPM): Limit 30000, Used 29000, Requested 2000.","type":"tokens","code":"rate_limit_exceeded"}}"#;
    assert_eq!(
        classify(StatusCode::TOO_MANY_REQUESTS, reached),
        WorkModelError::RateLimited {
            retry_after_ms: None
        }
    );
    assert_eq!(
        classify(StatusCode::PAYLOAD_TOO_LARGE, b"{}"),
        WorkModelError::ContextTooLong
    );
    let mut headers = HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("7"));
    assert_eq!(retry_after(&headers), Some(Duration::from_secs(7)));
    headers.insert("retry-after-ms", HeaderValue::from_static("250"));
    assert_eq!(retry_after(&headers), Some(Duration::from_millis(250)));
}

#[test]
fn cost_follows_the_catalog_price_and_search_fees() {
    let client = LeadClient::new(
        LeadTarget::direct(WorkModelProvider::Anthropic).unwrap(),
        Arc::new(LeadStaticCredential::new(
            LeadSecret::new("k".into()).unwrap(),
        )),
        models::builtin_price(WorkModelProvider::Anthropic, "claude-opus-5-5"),
    );
    let decoded = Decoded {
        usage: WorkModelUsage {
            input_tokens: 1_000_000,
            cached_input_tokens: 500_000,
            cache_write_input_tokens: 0,
            output_tokens: 100_000,
            ..Default::default()
        },
        cache_write_tokens: 100_000,
        searches: 2,
        ..Default::default()
    };
    // 400k fresh × $4 + 500k cached × $0.20 + 100k written × $5 + 100k out × $20 + 2 × $0.01
    let expected = 1_600_000 + 100_000 + 500_000 + 2_000_000 + 20_000;
    let usage = client.usage(&decoded);
    assert_eq!(usage.cost_micros, Some(expected));
    assert_eq!(usage.cache_write_input_tokens, 100_000);
}

#[test]
fn targets_build_exact_urls_for_direct_and_cloud() {
    let gemini = LeadTarget::direct(WorkModelProvider::Google).unwrap();
    assert_eq!(
        gemini.call_url("gemini-3.8-flash", false).unwrap().as_str(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:streamGenerateContent?alt=sse"
    );
    assert!(gemini.call_url("../../x", false).is_err());
    let cloud = LeadTarget::cloud(ZEPHIUM_CLOUD_BASE, WorkModelProvider::DeepSeek).unwrap();
    assert_eq!(
        cloud.call_url("deepseek-flash", false).unwrap().as_str(),
        "https://api.zephium.app/deepseek/v1/chat/completions"
    );
    let cloud = LeadTarget::cloud(ZEPHIUM_CLOUD_BASE, WorkModelProvider::OpenAi).unwrap();
    assert_eq!(
        cloud.call_url("gpt-6-sol", false).unwrap().as_str(),
        "https://api.zephium.app/openai/v1/responses"
    );
    assert!(LeadTarget::compatible("http://example.com/v1").is_err());
    assert!(LeadTarget::compatible("http://localhost:11434/v1").is_ok());
    assert!(LeadTarget::compatible("https://user:pw@example.com/v1").is_err());
    let mut headers = HeaderMap::new();
    cloud
        .authorize(&mut headers, &LeadSecret::new("tok".into()).unwrap())
        .unwrap();
    assert_eq!(headers["authorization"], "Bearer tok");
    assert!(headers["authorization"].is_sensitive());
}

/// A loopback server answering each connection with the next canned reply.
pub(super) fn serve(replies: Vec<String>) -> (String, std::sync::Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}/v1", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 65_536];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        log.lock().unwrap().push(text);
                        break;
                    }
                }
            }
            stream.write_all(reply.as_bytes()).unwrap();
        }
    });
    (address, seen)
}

pub(super) fn http_reply(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n{headers}\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn a_rate_limited_call_retries_after_the_hint_then_streams() {
    let stream = sse(&[
        (
            "response.output_text.delta",
            json!({"type": "response.output_text.delta", "delta": "Hi"}),
        ),
        (
            "response.output_item.done",
            json!({"type": "response.output_item.done", "item": {
            "type": "message", "content": [{"type": "output_text", "text": "Hi", "annotations": []}]}}),
        ),
        (
            "response.completed",
            json!({"type": "response.completed", "response": {"usage": {
            "input_tokens": 1000, "output_tokens": 10}}}),
        ),
    ]);
    let (base, seen) = serve(vec![
        http_reply(
            "429 Too Many Requests",
            "retry-after: 1\r\n",
            "{\"error\":{\"message\":\"slow\"}}",
        ),
        http_reply("200 OK", "content-type: text/event-stream\r\n", &stream),
    ]);
    let target = LeadTarget {
        wire: WorkModelWire::OpenAiResponses,
        upstream: WorkModelProvider::OpenAi,
        base: Url::parse(&base).unwrap(),
        cloud: false,
        search_calls: None,
    };
    let client = LeadClient::new(
        target,
        Arc::new(LeadStaticCredential::new(
            LeadSecret::new("sk-test".into()).unwrap(),
        )),
        models::builtin_price(WorkModelProvider::OpenAi, "gpt-6-sol"),
    );
    let events = Mutex::new(Vec::new());
    let sink = |event: WorkModelEvent| {
        if let WorkModelEvent::Text(text) = event {
            events.lock().unwrap().push(text);
        }
    };
    let outcome = client
        .call(request(WorkModelWire::OpenAiResponses, "gpt-6-sol"), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.stop, WorkModelStop::EndTurn);
    assert_eq!(*events.lock().unwrap(), vec!["Hi".to_owned()]);
    // 1,000 in × $2 + 10 out × $10, rounded up to whole micro-dollars.
    assert_eq!(outcome.usage.cost_micros, Some(2_100));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].starts_with("POST /v1/responses "));
    assert!(seen[1]
        .to_ascii_lowercase()
        .contains("authorization: bearer sk-test"));
}

#[tokio::test]
async fn an_unauthorized_call_fails_closed_without_retrying() {
    let (base, seen) = serve(vec![http_reply("401 Unauthorized", "", "{}")]);
    let target = LeadTarget {
        wire: WorkModelWire::OpenAiResponses,
        upstream: WorkModelProvider::OpenAi,
        base: Url::parse(&base).unwrap(),
        cloud: false,
        search_calls: None,
    };
    let client = LeadClient::new(
        target,
        Arc::new(LeadStaticCredential::new(
            LeadSecret::new("sk-test".into()).unwrap(),
        )),
        None,
    );
    let error = client
        .call(
            request(WorkModelWire::OpenAiResponses, "gpt-6-sol"),
            &|_| {},
        )
        .await
        .err();
    assert_eq!(error, Some(WorkModelError::Unauthorized));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

fn hosted(refusal: HostedRefusal) -> WorkModelError {
    WorkModelError::Hosted {
        refusal,
        retry_after_ms: None,
    }
}

#[test]
fn cloud_refusals_never_read_as_a_bad_session_unless_they_are() {
    use HostedRefusal::*;
    let refused = |status: u16, code: &str| {
        let body = format!("{{\"error\":{{\"code\":\"{code}\"}},\"request_id\":\"r\"}}");
        classify_cloud(StatusCode::from_u16(status).unwrap(), body.as_bytes())
    };
    assert_eq!(refused(401, "unauthorized"), WorkModelError::Unauthorized);
    assert_eq!(
        refused(403, "model_not_allowed"),
        WorkModelError::BadRequest
    );
    for (status, code, refusal) in [
        (403, "plan_required", PlanRequired),
        (403, "billing_review_required", BillingReview),
        (402, "insufficient_credits", OutOfCredits),
        (402, "daily_limit_reached", DailyLimit),
        (429, "free_usage_limit_reached", FreeUsagePaused),
        (429, "usage_confirmation_limited", ConfirmationLimited),
        (503, "cloud_budget_exhausted", Paused),
        (409, "request_in_progress", AlreadySent),
        (409, "request_already_recorded", AlreadySent),
        (400, "invalid_audio", InvalidAudio),
        (415, "unsupported_audio_format", UnsupportedAudio),
        (422, "audio_too_long", AudioTooLong),
    ] {
        assert_eq!(refused(status, code), hosted(refusal), "{status} {code}");
    }
    assert_eq!(
        refused(400, "input_limit_exceeded"),
        WorkModelError::ContextTooLong
    );
    assert_eq!(
        refused(400, "context_limit_exceeded"),
        WorkModelError::ContextTooLong
    );
    assert_eq!(refused(503, "cloud_disabled"), WorkModelError::Overloaded);
    assert!(matches!(
        refused(429, "concurrency_limited"),
        WorkModelError::RateLimited { .. }
    ));
}

#[test]
fn the_larger_server_wait_wins_and_never_shortens() {
    let mut headers = HeaderMap::new();
    headers.insert("retry-after", HeaderValue::from_static("7"));
    let body = br#"{"error":{"code":"x","retry_after":30},"request_id":null}"#;
    assert_eq!(
        cloud::server_wait(&headers, body, true),
        Some(Duration::from_secs(30))
    );
    let small = br#"{"error":{"code":"x","retry_after":2}}"#;
    assert_eq!(
        cloud::server_wait(&headers, small, true),
        Some(Duration::from_secs(7))
    );
    headers.insert("retry-after-ms", HeaderValue::from_static("45000"));
    assert_eq!(
        cloud::server_wait(&headers, small, true),
        Some(Duration::from_secs(45))
    );
    // A provider's own body is not Cloud's contract.
    assert_eq!(cloud::server_wait(&HeaderMap::new(), body, false), None);
    assert_eq!(
        cloud::server_wait(&HeaderMap::new(), b"{\"error\":{\"retry_after\":-1}}", true),
        None
    );
}

#[test]
fn a_cloud_wait_lands_on_the_error_that_carries_it() {
    let wait = Some(Duration::from_secs(90));
    assert_eq!(
        cloud::with_wait(hosted(HostedRefusal::FreeUsagePaused), wait, true),
        WorkModelError::Hosted {
            refusal: HostedRefusal::FreeUsagePaused,
            retry_after_ms: Some(90_000)
        }
    );
    // A 503 that says when to come back is a wait, not an outage.
    assert_eq!(
        cloud::with_wait(WorkModelError::Overloaded, wait, true),
        WorkModelError::RateLimited {
            retry_after_ms: Some(90_000)
        }
    );
    assert_eq!(
        cloud::with_wait(WorkModelError::Overloaded, wait, false),
        WorkModelError::Overloaded
    );
    assert_eq!(
        cloud::with_wait(WorkModelError::Overloaded, None, true),
        WorkModelError::Overloaded
    );
    // The daily limit resets at 00:00 UTC without being told.
    let noon = std::time::UNIX_EPOCH + Duration::from_secs(19_000 * 86_400 + 12 * 3_600);
    assert_eq!(
        cloud::with_wait_at(hosted(HostedRefusal::DailyLimit), None, true, noon),
        WorkModelError::Hosted {
            refusal: HostedRefusal::DailyLimit,
            retry_after_ms: Some(12 * 3_600 * 1_000)
        }
    );
}

#[tokio::test]
async fn hosted_refusals_are_not_retried_and_a_budget_stop_is_not_an_outage() {
    for (status, code, refusal) in [
        (
            "503 Service Unavailable",
            "cloud_budget_exhausted",
            HostedRefusal::Paused,
        ),
        (
            "429 Too Many Requests",
            "free_usage_limit_reached",
            HostedRefusal::FreeUsagePaused,
        ),
        (
            "429 Too Many Requests",
            "usage_confirmation_limited",
            HostedRefusal::ConfirmationLimited,
        ),
    ] {
        let (base, seen) = serve(vec![
            http_reply(
                status,
                "retry-after: 0\r\n",
                &format!("{{\"error\":{{\"code\":\"{code}\",\"retry_after\":3600}},\"request_id\":null}}"),
            ),
            http_reply("200 OK", "content-type: text/event-stream\r\n", &completed_stream()),
        ]);
        let (client, _) = cloud_client(&base);
        let error = client
            .call(
                request(WorkModelWire::OpenAiResponses, "gpt-6-luna"),
                &|_| {},
            )
            .await
            .err();
        assert_eq!(
            error,
            Some(WorkModelError::Hosted {
                refusal,
                retry_after_ms: Some(3_600_000)
            })
        );
        assert_eq!(seen.lock().unwrap().len(), 1, "{code} was sent again");
    }
}

#[tokio::test]
async fn a_cloud_503_that_names_a_long_wait_is_not_retried_here() {
    let (base, seen) = serve(vec![
        http_reply(
            "503 Service Unavailable",
            "retry-after: 120\r\n",
            "{\"error\":{\"code\":\"provider_unavailable\"},\"request_id\":null}",
        ),
        http_reply(
            "200 OK",
            "content-type: text/event-stream\r\n",
            &completed_stream(),
        ),
    ]);
    let (client, _) = cloud_client(&base);
    let error = client
        .call(
            request(WorkModelWire::OpenAiResponses, "gpt-6-luna"),
            &|_| {},
        )
        .await
        .err();
    assert_eq!(
        error,
        Some(WorkModelError::RateLimited {
            retry_after_ms: Some(120_000)
        })
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_409_on_the_key_is_already_sent_and_is_never_sent_again() {
    let id = "0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c";
    let (base, seen) = serve(vec![
        http_reply(
            "409 Conflict",
            &format!("x-zephium-request-id: {id}\r\n"),
            &format!("{{\"error\":{{\"code\":\"request_in_progress\"}},\"request_id\":\"{id}\"}}"),
        ),
        http_reply(
            "200 OK",
            "content-type: text/event-stream\r\n",
            &completed_stream(),
        ),
    ]);
    let (client, records) = cloud_client(&base);
    let error = client
        .call(
            request(WorkModelWire::OpenAiResponses, "gpt-6-luna"),
            &|_| {},
        )
        .await
        .err();
    assert_eq!(error, Some(hosted(HostedRefusal::AlreadySent)));
    assert_eq!(seen.lock().unwrap().len(), 1);
    let record = records.lock().unwrap()[0].clone();
    assert_eq!(record.request_id.as_deref(), Some(id));
    assert!(record.unsettled(), "the receipt check still runs");
}

fn completed_stream() -> String {
    sse(&[(
        "response.completed",
        json!({"type": "response.completed", "response": {"usage": {
        "input_tokens": 10, "output_tokens": 1}}}),
    )])
}

type Records = std::sync::Arc<Mutex<Vec<LeadCloudRecord>>>;

fn cloud_client(base: &str) -> (LeadClient, Records) {
    let root = base.trim_end_matches("/v1");
    let target = LeadTarget::cloud(root, WorkModelProvider::OpenAi).unwrap();
    let records: Records = Default::default();
    let log = records.clone();
    let client = LeadClient::new(
        target,
        Arc::new(LeadStaticCredential::new(
            LeadSecret::new("session-token".into()).unwrap(),
        )),
        None,
    )
    .with_cloud_observer(Arc::new(move |record| log.lock().unwrap().push(record)));
    (client, records)
}

fn idempotency_keys(seen: &Mutex<Vec<String>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .map(|request| {
            let request = request.to_ascii_lowercase();
            assert!(request.starts_with("post /openai/v1/responses "));
            assert!(request.contains("authorization: bearer session-token"));
            let line = request
                .lines()
                .find_map(|line| line.strip_prefix("idempotency-key: "))
                .unwrap()
                .to_owned();
            assert_eq!(line.len(), 36);
            assert_eq!(&line[14..15], "4");
            assert!(matches!(&line[19..20], "8" | "9" | "a" | "b"));
            line
        })
        .collect()
}

#[tokio::test]
async fn cloud_keeps_one_key_per_generation_unless_an_attempt_was_admitted() {
    let (base, seen) = serve(vec![
        http_reply(
            "429 Too Many Requests",
            "retry-after: 0\r\n",
            "{\"error\":{\"code\":\"concurrency_limited\",\"retry_after\":1},\"request_id\":null}",
        ),
        http_reply(
            "200 OK",
            "content-type: text/event-stream\r\n",
            &completed_stream(),
        ),
    ]);
    let (client, _) = cloud_client(&base);
    let call = request(WorkModelWire::OpenAiResponses, "gpt-6-luna");
    client.call(call.clone(), &|_| {}).await.unwrap();
    let keys = idempotency_keys(&seen);
    assert_eq!(keys.len(), 2);
    assert_eq!(
        keys[0], keys[1],
        "nothing was admitted, so it is the same call"
    );

    // The provider's own 429 came after admission and was released unspent:
    // trying again is a new logical call with its own key.
    let (base, seen) = serve(vec![
        http_reply(
            "429 Too Many Requests",
            "retry-after: 0\r\nx-zephium-request-id: 0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c\r\n",
            "{\"error\":{\"code\":\"provider_rate_limited\"},\"request_id\":\"0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c\"}",
        ),
        http_reply(
            "200 OK",
            "content-type: text/event-stream\r\n",
            &completed_stream(),
        ),
    ]);
    let (client, records) = cloud_client(&base);
    client.call(call, &|_| {}).await.unwrap();
    let keys = idempotency_keys(&seen);
    assert_ne!(keys[0], keys[1]);
    let records = records.lock().unwrap();
    assert_eq!(records[0].accounting, LeadCloudAccounting::Refused);
    assert_eq!(records[0].idempotency_key, keys[0]);
}

#[tokio::test]
async fn a_cloud_stream_is_read_to_its_end_for_the_accounting_trailer() {
    let stream = format!(
        "{}: zephium {{\"request_id\":\"0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c\",\"usage\":\"settled\",\"charged\":\"812\"}}\n\n",
        sse(&[
            (
                "response.output_text.delta",
                json!({"type": "response.output_text.delta", "delta": "Hi"}),
            ),
            (
                "response.output_item.done",
                json!({"type": "response.output_item.done", "item": {
                "type": "message", "content": [{"type": "output_text", "text": "Hi", "annotations": []}]}}),
            ),
            (
                "response.completed",
                json!({"type": "response.completed", "response": {"usage": {
                "input_tokens": 10, "output_tokens": 1}}}),
            ),
        ])
    );
    let (base, _) = serve(vec![http_reply(
        "200 OK",
        "content-type: text/event-stream\r\nx-zephium-request-id: 0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c\r\nx-zephium-policy-revision: 7\r\n",
        &stream,
    )]);
    let (client, records) = cloud_client(&base);
    let texts = Mutex::new(String::new());
    let sink = |event: WorkModelEvent| {
        if let WorkModelEvent::Text(text) = event {
            texts.lock().unwrap().push_str(&text);
        }
    };
    let outcome = client
        .call(request(WorkModelWire::OpenAiResponses, "gpt-6-luna"), &sink)
        .await
        .unwrap();
    assert_eq!(outcome.stop, WorkModelStop::EndTurn);
    assert_eq!(
        *texts.lock().unwrap(),
        "Hi",
        "the trailer is not model text"
    );
    let records = records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].request_id.as_deref(),
        Some("0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c")
    );
    assert_eq!(records[0].policy_revision.as_deref(), Some("7"));
    assert_eq!(
        records[0].accounting,
        LeadCloudAccounting::Settled {
            charged: Some("812".into())
        }
    );
    assert!(!records[0].unsettled());
}

#[tokio::test]
async fn a_cloud_call_that_may_have_run_is_never_sent_again() {
    let id = "0b3c6a3e-8f7d-4a59-9a3c-1d2e3f4a5b6c";
    // The provider failed after admission: the server holds the charge open.
    let (base, seen) = serve(vec![
        http_reply(
            "502 Bad Gateway",
            &format!("x-zephium-request-id: {id}\r\n"),
            &format!(
                "{{\"error\":{{\"code\":\"provider_request_failed\"}},\"request_id\":\"{id}\"}}"
            ),
        ),
        http_reply(
            "200 OK",
            "content-type: text/event-stream\r\n",
            &completed_stream(),
        ),
    ]);
    let (client, records) = cloud_client(&base);
    let call = request(WorkModelWire::OpenAiResponses, "gpt-6-luna");
    let error = client.call(call.clone(), &|_| {}).await.err();
    assert_eq!(error, Some(WorkModelError::Protocol));
    assert_eq!(seen.lock().unwrap().len(), 1);
    let record = records.lock().unwrap()[0].clone();
    assert_eq!(record.accounting, LeadCloudAccounting::Unknown);
    assert!(record.unsettled());

    // A stream cut after admission, before any output, is not retried either,
    // and its interruption trailer still reaches the record.
    let cut = format!(
        "event: response.created\ndata: {}\n\n: zephium {{\"request_id\":\"{id}\",\"error\":\"provider_idle_timeout\"}}\n\n",
        json!({"type": "response.created", "response": {}})
    );
    let (base, seen) = serve(vec![
        http_reply("200 OK", "content-type: text/event-stream\r\n", &cut),
        http_reply(
            "200 OK",
            "content-type: text/event-stream\r\n",
            &completed_stream(),
        ),
    ]);
    let (client, records) = cloud_client(&base);
    let error = client.call(call.clone(), &|_| {}).await.err();
    assert_eq!(error, Some(WorkModelError::Protocol));
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(
        records.lock().unwrap()[0].accounting,
        LeadCloudAccounting::Interrupted
    );

    // A 409 on the key means an earlier send was recorded: nothing replays.
    let (base, seen) = serve(vec![http_reply(
        "409 Conflict",
        "",
        "{\"error\":{\"code\":\"request_in_progress\"},\"request_id\":null}",
    )]);
    let (client, _) = cloud_client(&base);
    assert_eq!(
        client.call(call, &|_| {}).await.err(),
        Some(hosted(HostedRefusal::AlreadySent))
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cloud_credit_and_cap_refusals_are_out_of_credits_and_final() {
    let (base, seen) = serve(vec![http_reply(
        "402 Payment Required",
        "",
        "{\"error\":{\"code\":\"insufficient_credits\"},\"request_id\":null}",
    )]);
    let (client, records) = cloud_client(&base);
    let error = client
        .call(
            request(WorkModelWire::OpenAiResponses, "gpt-6-luna"),
            &|_| {},
        )
        .await
        .err();
    assert_eq!(error, Some(hosted(HostedRefusal::OutOfCredits)));
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert!(records.lock().unwrap().is_empty());
}

#[test]
fn another_providers_reasoning_is_never_replayed() {
    let mut request = request(WorkModelWire::ChatCompletions, "deepseek-flash");
    let from = |origin: Option<&str>| {
        let mut part = json!({"wire": "chat", "value": {"reasoning_content": "thought"}});
        if let Some(origin) = origin {
            part["origin"] = json!(origin);
        }
        WorkModelPart::Replay(part)
    };
    request.messages.push(WorkModelMessage::Assistant(vec![
        from(Some("deepseek")),
        from(Some("groq")),
        from(Some("deepseek@zephium")),
        from(None),
        WorkModelPart::Text("answer".into()),
    ]));
    strip_foreign_replays(&mut request, "deepseek");
    let Some(WorkModelMessage::Assistant(parts)) = request.messages.last() else {
        panic!("assistant turn kept");
    };
    let origins: Vec<Option<&str>> = parts
        .iter()
        .filter_map(|part| match part {
            WorkModelPart::Replay(value) => Some(value.get("origin").and_then(Value::as_str)),
            _ => None,
        })
        .collect();
    assert_eq!(origins, vec![Some("deepseek"), None]);
    assert_eq!(parts.len(), 3);
}

fn cloud_target(upstream: WorkModelProvider, calls: Option<u32>) -> LeadTarget {
    let mut target = LeadTarget::cloud("https://api.zephium.app", upstream).unwrap();
    target.search_calls = calls;
    target
}

fn search_tools(body: &Value, tool_type: &str) -> Vec<Value> {
    body["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["type"] == tool_type)
        .cloned()
        .collect()
}

#[test]
fn cloud_native_search_follows_the_rows_call_cap() {
    let openai_call = request(WorkModelWire::OpenAiResponses, "gpt-6-luna");
    let capped = openai::body(
        &openai_call,
        &cloud_target(WorkModelProvider::OpenAi, Some(3)),
    )
    .unwrap();
    assert_eq!(search_tools(&capped, "web_search").len(), 1);
    assert_eq!(capped["max_tool_calls"], 3);
    let uncapped =
        openai::body(&openai_call, &cloud_target(WorkModelProvider::OpenAi, None)).unwrap();
    assert!(uncapped.get("max_tool_calls").is_none());
    let none = openai::body(
        &openai_call,
        &cloud_target(WorkModelProvider::OpenAi, Some(0)),
    )
    .unwrap();
    assert!(search_tools(&none, "web_search").is_empty());
    assert!(none.get("max_tool_calls").is_none());
    assert!(!none["include"].to_string().contains("web_search"));
    let direct = openai::body(
        &openai_call,
        &LeadTarget::direct(WorkModelProvider::OpenAi).unwrap(),
    )
    .unwrap();
    assert!(direct.get("max_tool_calls").is_none());

    let anthropic_call = request(WorkModelWire::AnthropicMessages, "claude-sonnet-5");
    let uses = |calls| {
        let body = anthropic::body(
            &anthropic_call,
            &cloud_target(WorkModelProvider::Anthropic, calls),
        )
        .unwrap();
        search_tools(&body, "web_search_20250305")
            .first()
            .map(|tool| tool["max_uses"].clone())
    };
    assert_eq!(uses(Some(3)), Some(json!(3)));
    assert_eq!(
        uses(Some(20)),
        Some(json!(8)),
        "never above the local ceiling"
    );
    assert_eq!(uses(None), Some(json!(8)));
    assert_eq!(uses(Some(0)), None);
}

#[test]
fn hosted_google_never_sends_google_search() {
    let call = request(WorkModelWire::Gemini, "gemini-3.5-flash");
    let has_search = |target: &LeadTarget| {
        gemini::body(&call, target).unwrap()["tools"]
            .to_string()
            .contains("googleSearch")
    };
    assert!(has_search(
        &LeadTarget::direct(WorkModelProvider::Google).unwrap()
    ));
    assert!(!has_search(&cloud_target(WorkModelProvider::Google, None)));
    assert!(!has_search(&cloud_target(
        WorkModelProvider::Google,
        Some(5)
    )));
}

#[test]
fn a_rows_stream_limits_wait_a_little_past_the_servers_own() {
    let client = LeadClient::new(
        cloud_target(WorkModelProvider::OpenAi, None),
        Arc::new(LeadStaticCredential::new(
            LeadSecret::new("session-token".into()).unwrap(),
        )),
        None,
    )
    .with_call_limits(Some(90_000), Some(600_000));
    assert_eq!(client.idle_timeout, Some(Duration::from_secs(105)));
    assert_eq!(client.call_timeout, Some(Duration::from_secs(615)));
}

#[test]
fn local_endpoint_usage_preserves_explicit_rates_and_keeps_tokens() {
    for base in ["http://localhost:11434/v1", "https://192.168.1.2/v1"] {
        let client = LeadClient::new(
            LeadTarget::compatible(base).unwrap(),
            Arc::new(LeadStaticCredential::new(
                LeadSecret::new("fixture".into()).unwrap(),
            )),
            Some(WorkModelPrice {
                input: 1000,
                cached_input: 100,
                output: 5000,
            }),
        );
        let result = client.usage(&Decoded {
            usage: WorkModelUsage {
                input_tokens: 100,
                output_tokens: 10,
                ..Default::default()
            },
            provider_cost_micros: Some(9000),
            searches: 1,
            ..Default::default()
        });
        assert_eq!(result.cost_micros, Some(9000));
        assert_eq!(result.input_tokens, 100);
        assert_eq!(
            client
                .usage(&Decoded {
                    usage: WorkModelUsage {
                        input_tokens: 1_000_000,
                        output_tokens: 100_000,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .cost_micros,
            Some(1500)
        );
        let free = LeadClient::new(
            LeadTarget::compatible(base).unwrap(),
            Arc::new(LeadStaticCredential::new(
                LeadSecret::new("fixture".into()).unwrap(),
            )),
            None,
        );
        assert_eq!(
            free.usage(&Decoded {
                usage: WorkModelUsage {
                    input_tokens: 100,
                    output_tokens: 10,
                    ..Default::default()
                },
                ..Default::default()
            })
            .cost_micros,
            Some(0)
        );
    }
}
