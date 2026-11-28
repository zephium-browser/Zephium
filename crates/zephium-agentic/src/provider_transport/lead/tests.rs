t record = records.lock().unwrap()[0].clone();
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
