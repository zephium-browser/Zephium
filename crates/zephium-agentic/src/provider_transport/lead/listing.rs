ties/reasoning/effort") {
        Some(Value::Array(efforts)) => !efforts.is_empty(),
        Some(Value::Object(efforts)) => !efforts.is_empty(),
        Some(Value::String(effort)) => !effort.is_empty(),
        _ => false,
    };
    let mut roles = vec![WorkModelRole::Lead, WorkModelRole::Light];
    if vision {
        roles.insert(1, WorkModelRole::Page);
    }
    Some(WorkModelEntry {
        id: entry_id(WorkModelProvider::Compatible, id),
        model: WorkModelRef {
            provider: WorkModelProvider::Compatible,
            wire,
            model: id.to_owned(),
        },
        display_name: name.to_owned(),
        roles,
        recommended: false,
        context_window: context.unwrap_or(128_000),
        max_output: output.unwrap_or(16_384),
        supports: WorkModelSupports {
            tools,
            vision,
            prompt_cache: false,
            reasoning,
            native_search: false,
        },
        price: go_price(row),
    })
}

/// Go prices text per million tokens. Long-context tiers override the base
/// rate; for reservation we keep the highest applicable rate, so a long call
/// is never charged at the entry price.
fn go_price(row: &Value) -> Option<WorkModelPrice> {
    let text = row.pointer("/cost/text")?;
    let micros = |value: Option<&Value>| -> Option<u64> {
        let amount = value.and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
        })?;
        (amount.is_finite() && amount >= 0.0)
            .then(|| (amount * 1_000_000.0).min(u64::MAX as f64).round() as u64)
    };
    let mut input = micros(text.get("input"))?;
    let mut output = micros(text.get("output"))?;
    let mut cached = micros(text.get("cached_input"))
        .or_else(|| micros(text.pointer("/cache/read")))
        .unwrap_or(input);
    for tier in text
        .get("overrides")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let rates = tier.get("cost").unwrap_or(tier);
        if let Some(rate) = micros(rates.get("input")) {
            input = input.max(rate);
        }
        if let Some(rate) = micros(rates.get("output")) {
            output = output.max(rate);
        }
        if let Some(rate) =
            micros(rates.get("cached_input")).or_else(|| micros(rates.pointer("/cache/read")))
        {
            cached = cached.max(rate);
        }
    }
    Some(WorkModelPrice {
        input,
        cached_input: cached,
        output,
    })
}

// Official deprecation notices, checked 2026-10-07. Aliases and snapshots
// sharing a retired family are hidden from the full listing as well.
fn deprecated(provider: WorkModelProvider, id: &str) -> bool {
    let (provider, id) = if provider == WorkModelProvider::OpenRouter {
        match id.split_once('/') {
            Some(("openai", id)) => (WorkModelProvider::OpenAi, id),
            Some(("anthropic", id)) => (WorkModelProvider::Anthropic, id),
            _ => (provider, id),
        }
    } else {
        (provider, id)
    };
    match provider {
        // https://developers.openai.com/api/docs/deprecations
        WorkModelProvider::OpenAi => {
            [
                "gpt-3.5",
                "gpt-4-",
                "gpt-4-turbo",
                "gpt-4.1-nano",
                "gpt-4o-2024-05-13",
                "gpt-5.2-chat",
                "gpt-5.3-chat",
                "gpt-5-2025-08-07",
                "gpt-5-mini",
                "gpt-5-nano",
                "gpt-5-pro",
                "o3-2025",
                "o3-pro",
                "gpt-5.4-cyber",
                "gpt-5-chat",
                "gpt-5.1-chat",
                "gpt-5.1-2",
                "gpt-5.4-nano",
                "o1-",
                "o3-mini",
                "o4-mini",
            ]
            .iter()
            .any(|prefix| id.starts_with(prefix))
                || matches!(id, "gpt-4" | "gpt-5" | "gpt-5.1" | "o1" | "o3")
        }
        // https://platform.claude.com/docs/en/about-claude/model-deprecations
        WorkModelProvider::Anthropic => {
            [
                "claude-1.",
                "claude-2.",
                "claude-instant-",
                "claude-3-",
                "claude-sonnet-4-5",
            ]
            .iter()
            .any(|prefix| id.starts_with(prefix))
                || matches!(
                    id,
                    "claude-mythos-preview"
                        | "claude-opus-4-1-20250805"
                        | "claude-opus-4-20250514"
                        | "claude-sonnet-4-20250514"
                )
        }
        // https://ai.google.dev/gemini-api/docs/deprecations
        WorkModelProvider::Google => {
            id.starts_with("gemini-2.5-pro-preview-")
                || matches!(
                    id,
                    "gemini-2.0-flash"
                        | "gemini-2.0-flash-001"
                        | "gemini-2.0-flash-lite"
                        | "gemini-2.0-flash-lite-001"
                        | "gemini-3.1-flash-lite"
                        | "gemini-3-pro-preview"
                        | "gemini-3.1-flash-lite-preview"
                        | "gemini-3.1-flash-image-preview"
                        | "gemini-3-pro-image-preview"
                        | "gemini-2.5-flash-lite-preview-09-2025"
                        | "gemini-2.5-flash-preview-05-20"
                        | "gemini-2.5-flash-image-preview"
                        | "gemini-2.5-flash-preview-09-25"
                )
        }
        _ => false,
    }
}

fn chat_model(id: &str, prefixes: &[&str]) -> bool {
    const NOT_CHAT: &[&str] = &[
        "audio",
        "realtime",
        "transcribe",
        "tts",
        "image",
        "embedding",
        "moderation",
        "instruct",
        "search",
        "live",
        "dall-e",
        "whisper",
        "codex",
        "computer-use",
        "robotics",
    ];
    prefixes.iter().any(|prefix| id.starts_with(prefix))
        && !NOT_CHAT.iter().any(|word| id.contains(word))
}

/// OpenRouter prices are USD per token, as decimal strings.
fn openrouter_price(row: &Value) -> Option<WorkModelPrice> {
    let per_token = |key: &str| -> Option<u64> {
        let value: f64 = row
            .pointer(&format!("/pricing/{key}"))?
            .as_str()?
            .parse()
            .ok()?;
        (value.is_finite() && value >= 0.0).then(|| (value * 1e12).round() as u64)
    };
    let input = per_token("prompt")?;
    let output = per_token("completion")?;
    Some(WorkModelPrice {
        input,
        cached_input: per_token("input_cache_read").unwrap_or(input),
        output,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn current_builtins_survive_and_retired_models_are_hidden() {
        use zephium_core::work::model::WorkModelProvider as P;
        for entry in super::super::models::builtin() {
            assert!(!super::deprecated(entry.model.provider, &entry.model.model));
        }
        for (provider, id) in [
            (P::OpenAi, "gpt-3.5-turbo"),
            (P::Anthropic, "claude-opus-4-1-20250805"),
            (P::Google, "gemini-3-pro-preview"),
            (P::OpenAi, "o4-mini-2025-04-16"),
            (P::OpenAi, "o1"),
            (P::OpenAi, "gpt-5.1"),
            (P::Anthropic, "claude-sonnet-4-5-20250929"),
            (P::Google, "gemini-3.1-flash-lite"),
            (P::OpenRouter, "openai/o3-mini"),
        ] {
            assert!(super::deprecated(provider, id), "{id}");
        }
        for (provider, id) in [
            (P::Google, "gemini-2.5-pro"),
            (P::OpenAi, "gpt-5.1-codex"),
            (P::OpenAi, "gpt-4.1-mini"),
            (P::Anthropic, "claude-opus-4-5-20251101"),
        ] {
            assert!(!super::deprecated(provider, id), "{id}");
        }
    }

    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn key_checks_are_free_and_distinguish_service_failures() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (status, body, expected) in [
            (
                401,
                r#"{"error":{"message":"secret provider text"}}"#,
                LeadKeyCheck::Invalid,
            ),
            (403, "{}", LeadKeyCheck::Invalid),
            (402, "{}", LeadKeyCheck::Billing),
            (
                429,
                r#"{"error":{"code":"insufficient_quota"}}"#,
                LeadKeyCheck::Billing,
            ),
            (429, "{}", LeadKeyCheck::RateLimited),
            (501, "{}", LeadKeyCheck::ProviderDown),
            (200, r#"{"data":[]}"#, LeadKeyCheck::Valid(vec![])),
            (200, "{}", LeadKeyCheck::Failed),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let n = socket.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with("GET /v1/models "));
                socket.write_all(format!("HTTP/1.1 {status} Response\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            });
            let target = LeadTarget::compatible(&format!("http://{address}/v1")).unwrap();
            assert_eq!(
                check_key(&target, &LeadSecret::new("test-only".into()).unwrap()).await,
                expected
            );
            server.await.unwrap();
        }
    }

    #[test]
    fn go_profile_derives_its_catalog_and_session_hint() {
        let go = LeadTarget::compatible("https://api.opencode.ai/inference/go/openai/v1").unwrap();
        assert!(go.is_go());
        assert_eq!(
            go.catalog_url().unwrap().as_str(),
            "https://api.opencode.ai/inference/go/v1/models"
        );
        assert_eq!(
            go.call_url("glm-5.3-flash", false).unwrap().as_str(),
            "https://api.opencode.ai/inference/go/openai/v1/chat/completions"
        );
        let mut request = super::super::tests::request(WorkModelWire::ChatCompletions, "go-model");
        request.cache_key = Some("zk_0123456789abcdef".into());
        assert_eq!(
            go.go_session(&request).unwrap().to_str().unwrap(),
            "ses_0123456789abcdef"
        );
        request.cache_key = Some("not-a-cache-key".into());
        assert!(go.go_session(&request).is_none());

        let plain = LeadTarget::compatible("https://host.example/v1").unwrap();
        assert!(!plain.is_go());
        assert_eq!(
            plain.catalog_url().unwrap().as_str(),
            "https://host.example/v1/models"
        );
        assert!(plain.go_session(&request).is_none());
    }

    #[test]
    fn go_catalog_rows_carry_limits_modalities_reasoning_and_tiered_prices() {
        let go = LeadTarget::compatible("https://api.opencode.ai/inference/go/openai/v1").unwrap();
        let listed = parse_list(
            &go,
            &json!({"data": [
                {"id": "glm-5.3-flash", "name": "GLM 5.3 Flash",
                 "limit": {"context": 1_000_000, "input": 500_000, "output": 64_000},
                 "modalities": {"input": ["text", "image"]},
                 "capabilities": {"tools": true,
                                  "reasoning": {"effort": ["low", "medium", "high", "xhigh", "max"]}},
                 "cost": {"text": {"input": "2", "output": "8", "cached_input": "0.5",
                     "overrides": [
                         {"up_to": 200000, "input": "2", "output": "8"},
                         {"input": "6", "output": "24", "cached_input": "1.5"}]}}},
                {"id": "text-only",
                 "limit": {"context": 128000, "input": 100000},
                 "capabilities": {"reasoning": {"effort": []}}}
            ]}),
        );
        assert_eq!(listed.len(), 2);
        let vision = listed
            .iter()
            .find(|entry| entry.model.model == "glm-5.3-flash")
            .unwrap();
        assert_eq!(vision.context_window, 1_000_000);
        assert_eq!(vision.max_output, 64_000);
        assert!(vision.roles.contains(&WorkModelRole::Page));
        assert!(vision.supports.vision && vision.supports.tools && vision.supports.reasoning);
        assert_eq!(
            vision.price,
            Some(WorkModelPrice {
                input: 6_000_000,
                cached_input: 1_500_000,
                output: 24_000_000,
            })
        );
        let plain = listed
            .iter()
            .find(|entry| entry.model.model == "text-only")
            .unwrap();
        assert!(!plain.supports.vision && !plain.supports.reasoning);
        assert_eq!(plain.max_output, 28_000);
        assert!(plain.price.is_none());
    }

    #[test]
    fn listings_keep_chat_models_with_their_limits_and_prices() {
        let openai = LeadTarget::direct(WorkModelProvider::OpenAi).unwrap();
        let listed = parse_list(
            &openai,
            &json!({"data": [{"id": "gpt-6-sol"}, {"id": "gpt-realtime"}, {"id": "text-embedding-4"}]}),
        );
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "openai/gpt-6-sol");
        assert_eq!(listed[0].price.as_ref().unwrap().input, 2_000_000);

        let google = LeadTarget::direct(WorkModelProvider::Google).unwrap();
        let listed = parse_list(
            &google,
            &json!({"models": [
                {"name": "models/gemini-3.8-flash", "displayName": "Gemini 3.8 Flash",
                 "inputTokenLimit": 1048576, "outputTokenLimit": 65536,
                 "supportedGenerationMethods": ["generateContent", "countTokens"]},
                {"name": "models/gemini-embedding-2", "supportedGenerationMethods": ["embedContent"]}
            ]}),
        );
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].max_output, 65_536);

        let router = LeadTarget::direct(WorkModelProvider::OpenRouter).unwrap();
        let listed = parse_list(
            &router,
            &json!({"data": [
                {"id": "moonshotai/kimi-k3", "name": "Kimi K3", "context_length": 262144,
                 "supported_parameters": ["tools", "temperature"],
                 "architecture": {"input_modalities": ["text"]},
                 "pricing": {"prompt": "0.0000006", "completion": "0.0000025"}},
                {"id": "no/tools", "supported_parameters": []}
            ]}),
        );
        assert_eq!(listed.len(), 1);
        let price = listed[0].price.as_ref().unwrap();
        assert_eq!((price.input, price.output), (600_000, 2_500_000));
        assert!(!listed[0].roles.contains(&WorkModelRole::Page));
    }
}
