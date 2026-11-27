//! The built-in model list: a short, curated set of current models per
//! provider, lead and small, checked against each provider's own model and
//! pricing pages (and OpenRouter's catalog) on 2026-09-30; and the Zephium
//! Cloud catalog parser, whose list replaces this one when signed in.

use serde_json::Value;
use zephium_core::work::model::*;

use WorkModelProvider as P;
use WorkModelRole as R;

/// Built-in per-call fees for provider-native search, in micro-USD.
pub fn search_fee_micros(provider: WorkModelProvider) -> u64 {
    match provider {
        // $10 per 1,000 calls; search content bills as input tokens.
        P::OpenAi | P::Anthropic => 10_000,
        // Grounding with Google Search: $14 per 1,000 queries past the free tier.
        P::Google => 14_000,
        _ => 0,
    }
}

const fn usd(dollars_per_million: f64) -> u64 {
    (dollars_per_million * 1_000_000.0 + 0.5) as u64
}

struct Spec {
    provider: WorkModelProvider,
    model: &'static str,
    name: &'static str,
    roles: &'static [WorkModelRole],
    recommended: bool,
    context: u32,
    output: u32,
    vision: bool,
    search: bool,
    price: (f64, f64, f64),
}

const SPECS: &[Spec] = &[
    Spec {
        provider: P::Anthropic,
        model: "claude-opus-5-5",
        name: "Claude Opus 5.5",
        roles: &[R::Lead],
        recommended: true,
        context: 1_000_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (4.0, 0.2, 20.0),
    },
    Spec {
        provider: P::Anthropic,
        model: "claude-sonnet-5-5",
        name: "Claude Sonnet 5.5",
        roles: &[R::Lead, R::Page],
        recommended: true,
        context: 1_000_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (2.0, 0.2, 10.0),
    },
    Spec {
        provider: P::Anthropic,
        model: "claude-haiku-4-5-20251001",
        name: "Claude Haiku 4.5",
        roles: &[R::Page, R::Light],
        recommended: true,
        context: 200_000,
        output: 64_000,
        vision: true,
        search: true,
        price: (1.0, 0.1, 5.0),
    },
    Spec {
        provider: P::Anthropic,
        model: "claude-fable-5-1",
        name: "Claude Fable 5.1",
        roles: &[R::Lead],
        recommended: false,
        context: 1_000_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (10.0, 0.25, 50.0),
    },
    // https://developers.openai.com/api/docs/models/gpt-5.6-terra
    Spec {
        provider: P::OpenAi,
        model: "gpt-5.6-terra",
        name: "GPT-5.6 Terra",
        roles: &[R::Lead, R::Page],
        recommended: false,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (2.0, 0.2, 12.0),
    },
    // https://developers.openai.com/api/docs/models/gpt-6.1-sol
    Spec {
        provider: P::OpenAi,
        model: "gpt-6.1-sol",
        name: "GPT-6.1 Sol",
        roles: &[R::Lead, R::Page],
        recommended: true,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (2.0, 0.1, 10.0),
    },
    Spec {
        provider: P::OpenAi,
        model: "gpt-6-luna",
        name: "GPT-6 Luna",
        roles: &[R::Lead, R::Page, R::Light],
        recommended: true,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (0.1, 0.01, 0.5),
    },
    Spec {
        provider: P::OpenAi,
        model: "gpt-6-sol",
        name: "GPT-6 Sol",
        roles: &[R::Lead, R::Page],
        recommended: false,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (2.0, 0.2, 10.0),
    },
    Spec {
        provider: P::OpenAi,
        model: "gpt-6-astra",
        name: "GPT-6 Astra",
        roles: &[R::Lead],
        recommended: false,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: true,
        price: (10.0, 1.0, 50.0),
    },
    Spec {
        provider: P::Google,
        model: "gemini-3.1-pro-preview",
        name: "Gemini 3.1 Pro",
        roles: &[R::Lead],
        recommended: true,
        context: 1_048_576,
        output: 65_536,
        vision: true,
        search: true,
        price: (2.0, 0.2, 12.0),
    },
    Spec {
        provider: P::Google,
        model: "gemini-3.8-flash",
        name: "Gemini 3.8 Flash",
        roles: &[R::Lead, R::Page],
        recommended: true,
        context: 1_048_576,
        output: 65_536,
        vision: true,
        search: true,
        price: (0.75, 0.075, 3.75),
    },
    Spec {
        provider: P::Google,
        model: "gemini-3.5-flash-lite",
        name: "Gemini 3.5 Flash-Lite",
        roles: &[R::Page, R::Light],
        recommended: true,
        context: 1_048_576,
        output: 65_536,
        vision: true,
        search: true,
        price: (0.3, 0.03, 2.5),
    },
    // https://api-docs.deepseek.com/quick_start/pricing, checked 2026-10-09:
    // deepseek-flash (V4.1 Flash) takes images, deepseek-v4-pro does not.
    // DeepSeek bills a lower off-peak rate; the peak rate keeps cost caps honest.
    Spec {
        provider: P::DeepSeek,
        model: "deepseek-v4-pro",
        name: "DeepSeek V4 Pro",
        roles: &[R::Lead],
        recommended: true,
        context: 1_000_000,
        output: 384_000,
        vision: false,
        search: false,
        price: (1.32, 0.044, 3.96),
    },
    Spec {
        provider: P::DeepSeek,
        model: "deepseek-flash",
        name: "DeepSeek V4.1 Flash",
        roles: &[R::Page, R::Light],
        recommended: true,
        context: 1_000_000,
        output: 384_000,
        vision: true,
        search: false,
        price: (0.3, 0.006, 1.2),
    },
    // https://docs.x.ai/developers/models, checked 2026-10-07. Output
    // ceilings are unpublished; these stay well under any context.
    Spec {
        provider: P::XAi,
        model: "grok-4.7",
        name: "Grok 4.7",
        roles: &[R::Lead],
        recommended: true,
        context: 500_000,
        output: 32_768,
        vision: true,
        search: true,
        price: (2.0, 0.5, 6.0),
    },
    Spec {
        provider: P::XAi,
        model: "grok-4.3",
        name: "Grok 4.3",
        roles: &[R::Lead, R::Page, R::Light],
        recommended: true,
        context: 1_000_000,
        output: 32_768,
        vision: true,
        search: true,
        price: (1.25, 0.2, 2.5),
    },
    // https://docs.mistral.ai/getting-started/models, list prices.
    Spec {
        provider: P::Mistral,
        model: "mistral-medium-3-5",
        name: "Mistral Medium 3.5",
        roles: &[R::Lead, R::Page],
        recommended: true,
        context: 256_000,
        output: 32_768,
        vision: true,
        search: false,
        price: (1.5, 0.15, 7.5),
    },
    Spec {
        provider: P::Mistral,
        model: "mistral-large-4",
        name: "Mistral Large 4",
        roles: &[R::Lead],
        recommended: false,
        context: 1_000_000,
        output: 32_768,
        vision: true,
        search: false,
        price: (1.36, 0.14, 4.18),
    },
    Spec {
        provider: P::Mistral,
        model: "mistral-small-2603",
        name: "Mistral Small",
        roles: &[R::Light],
        recommended: true,
        context: 256_000,
        output: 32_768,
        vision: false,
        search: false,
        price: (0.15, 0.015, 0.6),
    },
    // https://console.groq.com/docs/models
    Spec {
        provider: P::Groq,
        model: "openai/gpt-oss-120b",
        name: "GPT-OSS 120B",
        roles: &[R::Lead, R::Page],
        recommended: true,
        context: 131_072,
        output: 65_536,
        vision: false,
        search: false,
        price: (0.15, 0.075, 0.6),
    },
    Spec {
        provider: P::Groq,
        model: "openai/gpt-oss-20b",
        name: "GPT-OSS 20B",
        roles: &[R::Light],
        recommended: true,
        context: 131_072,
        output: 65_536,
        vision: false,
        search: false,
        price: (0.075, 0.0375, 0.3),
    },
    Spec {
        provider: P::Groq,
        model: "qwen/qwen3.8-27b",
        name: "Qwen 3.8 27B",
        roles: &[R::Lead, R::Page],
        recommended: false,
        context: 131_072,
        output: 16_384,
        vision: false,
        search: false,
        price: (0.8, 0.8, 4.0),
    },
    // https://inference-docs.cerebras.ai/models, paid tier limits.
    Spec {
        provider: P::Cerebras,
        model: "gpt-oss-120b",
        name: "GPT-OSS 120B",
        roles: &[R::Lead, R::Light],
        recommended: true,
        context: 131_072,
        output: 40_000,
        vision: false,
        search: false,
        price: (0.35, 0.35, 0.75),
    },
    Spec {
        provider: P::Cerebras,
        model: "qwen-3.8-27b",
        name: "Qwen 3.8 27B",
        roles: &[R::Page],
        recommended: true,
        context: 128_000,
        output: 40_000,
        vision: true,
        search: false,
        price: (0.99, 0.99, 1.49),
    },
    // OpenRouter serves the same models under its own ids and prices.
    Spec {
        provider: P::OpenRouter,
        model: "anthropic/claude-sonnet-5.5",
        name: "Claude Sonnet 5.5",
        roles: &[R::Lead, R::Page],
        recommended: true,
        context: 1_000_000,
        output: 128_000,
        vision: true,
        search: false,
        price: (2.0, 0.2, 10.0),
    },
    Spec {
        provider: P::OpenRouter,
        model: "anthropic/claude-opus-5.5",
        name: "Claude Opus 5.5",
        roles: &[R::Lead],
        recommended: false,
        context: 1_000_000,
        output: 128_000,
        vision: true,
        search: false,
        price: (4.0, 0.2, 20.0),
    },
    Spec {
        provider: P::OpenRouter,
        model: "openai/gpt-6-luna",
        name: "GPT-6 Luna",
        roles: &[R::Lead, R::Page, R::Light],
        recommended: true,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: false,
        price: (0.1, 0.01, 0.5),
    },
    Spec {
        provider: P::OpenRouter,
        model: "openai/gpt-6-sol",
        name: "GPT-6 Sol",
        roles: &[R::Lead, R::Page],
        recommended: false,
        context: 1_050_000,
        output: 128_000,
        vision: true,
        search: false,
        price: (2.0, 0.2, 10.0),
    },
    Spec {
        provider: P::OpenRouter,
        model: "google/gemini-3.8-flash",
        name: "Gemini 3.8 Flash",
        roles: &[R::Lead, R::Page],
        recommended: false,
        context: 1_048_576,
        output: 65_536,
        vision: true,
        search: false,
        price: (0.75, 0.075, 3.75),
    },
];

/// The wire a provider's own endpoint speaks.
pub fn wire(provider: WorkModelProvider) -> WorkModelWire {
    match provider {
        P::OpenAi => WorkModelWire::OpenAiResponses,
        P::Anthropic => WorkModelWire::AnthropicMessages,
        P::Google => WorkModelWire::Gemini,
        P::XAi => WorkModelWire::OpenAiResponses,
        P::DeepSeek
        | P::OpenRouter
        | P::Mistral
        | P::Groq
        | P::Cerebras
        | P::Compatible
        | P::Cloud => WorkModelWire::ChatCompletions,
    }
}

/// `provider/model`, the stable catalog id.
pub fn entry_id(provider: WorkModelProvider, model: &str) -> String {
    format!("{}/{model}", provider_slug(provider))
}

/// The lowercase provider name used in ids and settings.
pub fn provider_slug(provider: WorkModelProvider) -> &'static str {
    match provider {
        P::OpenAi => "openai",
        P::Anthropic => "anthropic",
        P::Google => "google",
        P::DeepSeek => "deepseek",
        P::OpenRouter => "openrouter",
        P::XAi => "xai",
        P::Mistral => "mistral",
        P::Groq => "groq",
        P::Cerebras => "cerebras",
        P::Compatible => "compatible",
        P::Cloud => "zephium",
    }
}

/// The inverse of [`provider_slug`].
pub fn provider_from_slug(slug: &str) -> Option<WorkModelProvider> {
    Some(match slug {
        "openai" => P::OpenAi,
        "anthropic" => P::Anthropic,
        "google" => P::Google,
        "deepseek" => P::DeepSeek,
        "openrouter" => P::OpenRouter,
        "xai" => P::XAi,
        "mistral" => P::Mistral,
        "groq" => P::Groq,
        "cerebras" => P::Cerebras,
        "compatible" => P::Compatible,
        "zephium" => P::Cloud,
        _ => return None,
    })
}

/// Every built-in model, recommended first within each provider.
pub fn builtin() -> Vec<WorkModelEntry> {
    SPECS
        .iter()
        .map(|spec| WorkModelEntry {
            id: entry_id(spec.provider, spec.model),
            model: WorkModelRef {
                provider: spec.provider,
                wire: wire(spec.provider),
                model: spec.model.to_owned(),
            },
            display_name: spec.name.to_owned(),
            roles: spec.roles.to_vec(),
            recommended: spec.recommended,
            context_window: spec.context,
            max_output: spec.output,
            supports: WorkModelSupports {
                tools: true,
                vision: spec.vision,
                prompt_cache: true,
                reasoning: true,
                native_search: spec.search,
            },
            price: Some(WorkModelPrice {
                input: usd(spec.price.0),
                cached_input: usd(spec.price.1),
                output: usd(spec.price.2),
            }),
        })
        .collect()
}

/// The price the built-in list knows for `model` from `provider`.
pub fn builtin_price(provider: WorkModelProvider, model: &str) -> Option<WorkModelPrice> {
    builtin()
        .into_iter()
        .find(|entry| entry.model.provider == provider && entry.model.model == model)
        .and_then(|entry| entry.price)
}

/// What a provider family runs for each role when the person chose nothing.
pub fn family_default(provider: WorkModelProvider, role: WorkModelRole) -> Option<&'static str> {
    Some(match (provider, role) {
        (P::Anthropic, R::Lead) => "claude-opus-5-5",
        (P::Anthropic, R::Page | R::Light) => "claude-haiku-4-5-20251001",
        (P::OpenAi, R::Lead | R::Page | R::Light) => "gpt-6-luna",
        (P::Google, R::Lead) => "gemini-3.1-pro-preview",
        (P::Google, R::Page) => "gemini-3.8-flash",
        (P::Google, R::Light) => "gemini-3.5-flash-lite",
        (P::DeepSeek, R::Lead) => "deepseek-v4-pro",
        (P::DeepSeek, R::Page | R::Light) => "deepseek-flash",
        (P::OpenRouter, R::Lead) => "anthropic/claude-sonnet-5.5",
        (P::OpenRouter, R::Page | R::Light) => "openai/gpt-6-luna",
        (P::XAi, R::Lead) => "grok-4.7",
        (P::XAi, R::Page | R::Light) => "grok-4.3",
        (P::Mistral, R::Lead | R::Page) => "mistral-medium-3-5",
        (P::Mistral, R::Light) => "mistral-small-2603",
        (P::Groq, R::Lead | R::Page) => "openai/gpt-oss-120b",
        (P::Groq, R::Light) => "openai/gpt-oss-20b",
        (P::Cerebras, R::Lead | R::Light) => "gpt-oss-120b",
        (P::Cerebras, R::Page) => "qwen-3.8-27b",
        _ => return None,
    })
}

/// The reasoning efforts `model` takes from `upstream` that change its
/// request, weakest first; empty when its wire sends none. Checked against
/// each provider's model pages on 2026-10-14.
pub fn efforts(upstream: WorkModelProvider, model: &str) -> Vec<WorkModelReasoning> {
    use WorkModelReasoning::*;
    match upstream {
        P::OpenAi if model.starts_with("gpt-6") || model.starts_with("gpt-5.6") => {
            vec![Low, Medium, High, XHigh, Max]
        }
        P::OpenAi => vec![Low, Medium, High],
        P::Anthropic if super::anthropic::budget_thinking(model) => vec![Low, Medium, High],
        P::Anthropic
            if model.starts_with("claude-opus-4-6") || model.starts_with("claude-sonnet-4-6") =>
        {
            vec![Low, Medium, High, Max]
        }
        P::Anthropic => vec![Low, Medium, High, XHigh, Max],
        P::Google if model.starts_with("gemini-3-pro") => vec![Low, High],
        P::Google => vec![Low, Medium, High],
        P::XAi => match model
            .strip_prefix("grok-4.")
            .and_then(|minor| minor.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|minor| minor.parse::<u32>().ok())
        {
            Some(6..) => vec![Low, Medium, High, XHigh],
            Some(5) => vec![Low, Medium, High],
            _ => Vec::new(),
        },
        P::DeepSeek => vec![Low, High],
        P::Groq | P::Cerebras if super::chat::takes_effort(upstream, model) => {
            vec![Low, Medium, High]
        }
        P::OpenRouter => vec![Low, Medium, High],
        _ => Vec::new(),
    }
}

/// The level of `levels` nearest to `wanted`, a tie going to the stronger.
pub fn nearest_effort(
    levels: &[WorkModelReasoning],
    wanted: WorkModelReasoning,
) -> Option<WorkModelReasoning> {
    let rank = |level: WorkModelReasoning| level as i32;
    levels
        .iter()
        .copied()
        .min_by_key(|level| ((rank(*level) - rank(wanted)).abs(), -rank(*level)))
}

/// What the lead runs at when the person chose no effort: medium, or the
/// provider's own default where medium used to mean more.
pub fn default_effort(
    upstream: WorkModelProvider,
    model: &str,
    levels: &[WorkModelReasoning],
) -> Option<WorkModelReasoning> {
    let wanted = if upstream == P::Google && model.contains("-pro") {
        WorkModelReasoning::High
    } else {
        WorkModelReasoning::Medium
    };
    nearest_effort(levels, wanted)
}

/// A Cloud catalog model with the upstream whose path it travels.
#[derive(Clone, Debug)]
pub struct CloudModel {
    /// The entry as the picker shows it; its provider is Cloud.
    pub entry: WorkModelEntry,
    /// The provider whose native wire and path it uses.
    pub upstream: WorkModelProvider,
    /// Smallest plan that may use it, as the server names it.
    pub plan_min: Option<String>,
    /// False when listed for this account but locked by its plan: shown,
    /// never called.
    pub available: bool,
    /// One line about the model, as the server states it.
    pub description: Option<String>,
    /// `fast`, `balanced` or `frontier`.
    pub tier: Option<String>,
    /// The largest request body the server admits for it.
    pub max_request_bytes: Option<u64>,
    /// Native search calls one request may make; zero means the row has none.
    pub max_search_calls: Option<u32>,
    /// How long the server waits on a silent provider stream before ending it.
    pub idle_timeout_ms: Option<u64>,
    /// The longest the server lets one call run.
    pub max_duration_ms: Option<u64>,
}

/// The server's model per role for this account; `None` where it names none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloudDefaults {
    /// The lead agent.
    pub lead: Option<String>,
    /// Page agents.
    pub page: Option<String>,
    /// Light work: titles, summaries, picks.
    pub light: Option<String>,
    /// Typed decisions.
    pub decision: Option<String>,
}

impl CloudDefaults {
    /// The entry id of the default for `role`.
    pub fn get(&self, role: WorkModelRole) -> Option<&str> {
        match role {
            R::Lead => self.lead.as_deref(),
            R::Page => self.page.as_deref(),
            R::Light => self.light.as_deref(),
            R::Decision => self.decision.as_deref(),
        }
    }
}

const MAX_CLOUD_MODELS: usize = 200;

/// Parses `GET /v1/models`. Unknown providers, roles or malformed rows are
/// skipped; the server decides which models exist. Cloud prices are credits,
/// so cost estimates use the upstream's built-in price where one is known.
pub fn parse_cloud(body: &Value) -> Vec<CloudModel> {
    let rows = body
        .get("data")
        .or_else(|| body.get("models"))
        .unwrap_or(body)
        .as_array()
        .cloned()
        .unwrap_or_default();
    rows.iter()
        .filter_map(cloud_row)
        .take(MAX_CLOUD_MODELS)
        .collect()
}

/// The catalog's `defaults`, as entry ids. `None` when the answer has none
/// (an older server); a role the server leaves null stays empty.
pub fn parse_cloud_defaults(body: &Value) -> Option<CloudDefaults> {
    let defaults = body.get("defaults")?.as_object()?;
    let role = |key: &str| {
        defaults
            .get(key)
            .and_then(Value::as_str)
            .filter(|id| valid_text(id, 128))
            .map(|id| format!("zephium/{id}"))
    };
    Some(CloudDefaults {
        lead: role("lead"),
        page: role("page"),
        light: role("light"),
        decision: role("decision"),
    })
}

fn cloud_row(row: &Value) -> Option<CloudModel> {
    let text = |key: &str| row.get(key).and_then(Value::as_str);
    let upstream = provider_from_slug(text("provider")?)?;
    if matches!(upstream, P::Cloud | P::Compatible) {
        return None;
    }
    let id = text("id").filter(|id| valid_text(id, 128))?;
    let model = text("upstream_model").filter(|m| valid_text(m, 128))?;
    let roles: Vec<WorkModelRole> = row
        .get("roles")?
        .as_array()?
        .iter()
        .filter_map(|role| match role.as_str()? {
            "lead" => Some(R::Lead),
            "page" => Some(R::Page),
            "light" => Some(R::Light),
            "decision" => Some(R::Decision),
            _ => None,
        })
        .collect();
    if roles.is_empty() {
        return None;
    }
    let supports = row.get("supports");
    let flag = |key: &str| {
        supports
            .and_then(|s| s.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let number = |key: &str| {
        row.get(key)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let limit = |key: &str| {
        row.get("limits")
            .and_then(|limits| limits.get(key))
            .and_then(Value::as_u64)
    };
    Some(CloudModel {
        entry: WorkModelEntry {
            id: format!("zephium/{id}"),
            model: WorkModelRef {
                provider: P::Cloud,
                wire: wire(upstream),
                model: model.to_owned(),
            },
            display_name: text("display_name")
                .filter(|name| valid_text(name, 64))
                .unwrap_or(model)
                .to_owned(),
            roles,
            recommended: row
                .get("recommended")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            context_window: number("context_window").unwrap_or(128_000),
            max_output: number("max_output").unwrap_or(8_192),
            supports: WorkModelSupports {
                tools: flag("tools"),
                vision: flag("vision"),
                prompt_cache: flag("prompt_cache"),
                reasoning: flag("reasoning"),
                native_search: flag("native_search"),
            },
            price: builtin_price(upstream, model),
        },
        upstream,
        plan_min: text("plan_min")
            .filter(|plan| valid_text(plan, 32))
            .map(str::to_owned),
        // A server from before the flag listed only what the account may use.
        available: row
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        description: text("description")
            .filter(|line| valid_text(line, 300))
            .map(str::to_owned),
        tier: text("tier")
            .filter(|tier| matches!(*tier, "fast" | "balanced" | "frontier"))
            .map(str::to_owned),
        max_request_bytes: limit("max_request_bytes").filter(|bytes| *bytes > 0),
        max_search_calls: limit("max_search_calls").and_then(|n| u32::try_from(n).ok()),
        idle_timeout_ms: limit("idle_timeout_ms").filter(|ms| *ms > 0),
        max_duration_ms: limit("max_duration_ms").filter(|ms| *ms > 0),
    })
}

/// A hosted speech-to-text model: a catalog row whose roles name
/// `transcribe`. It travels OpenAI's `audio/transcriptions` wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CloudTranscriber {
    /// The catalog id, sent as the upload's `model`.
    pub id: String,
    /// The upstream model, for display and the stored catalog.
    pub upstream_model: String,
    /// The row's `limits.max_request_bytes`, when it states one.
    pub max_request_bytes: Option<u64>,
}

/// The transcriber in `GET /v1/models`: `defaults.transcribe` when it names
/// an available row, else the first available one. Locked rows
/// (`available: false`) and non-OpenAI upstreams are skipped.
pub fn parse_cloud_transcriber(body: &Value) -> Option<CloudTranscriber> {
    let rows = body
        .get("data")
        .or_else(|| body.get("models"))
        .unwrap_or(body)
        .as_array()?;
    let preferred = body
        .get("defaults")
        .and_then(|defaults| defaults.get("transcribe"))
        .and_then(Value::as_str);
    let usable: Vec<CloudTranscriber> = rows
        .iter()
        .take(MAX_CLOUD_MODELS)
        .filter_map(|row| {
            let text = |key: &str| row.get(key).and_then(Value::as_str);
            let transcribes = row
                .get("roles")?
                .as_array()?
                .iter()
                .any(|role| role.as_str() == Some("transcribe"));
            let available = row.get("available").and_then(Value::as_bool) != Some(false);
            if !transcribes || !available || text("provider")? != "openai" {
                return None;
            }
            Some(CloudTranscriber {
                id: text("id").filter(|id| valid_text(id, 100))?.to_owned(),
                upstream_model: text("upstream_model")
                    .filter(|model| valid_text(model, 128))?
                    .to_owned(),
                max_request_bytes: row
                    .get("limits")
                    .and_then(|limits| limits.get("max_request_bytes"))
                    .and_then(Value::as_u64),
            })
        })
        .collect();
    preferred
        .and_then(|id| usable.iter().find(|row| row.id == id).cloned())
        .or_else(|| usable.into_iter().next())
}

pub(crate) fn valid_text(text: &str, max: usize) -> bool {
    !text.trim().is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_builtin_is_unique_priced_and_has_a_family_default_for_lead() {
        let entries = builtin();
        let mut ids: Vec<_> = entries.iter().map(|e| e.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), entries.len());
        for entry in &entries {
            assert!(entry.price.is_some());
            assert_eq!(entry.id, entry_id(entry.model.provider, &entry.model.model));
        }
        for provider in [
            P::OpenAi,
            P::Anthropic,
            P::Google,
            P::DeepSeek,
            P::OpenRouter,
        ] {
            for role in [R::Lead, R::Light] {
                let model = family_default(provider, role).unwrap();
                let entry = entries
                    .iter()
                    .find(|e| e.model.provider == provider && e.model.model == model)
                    .unwrap();
                assert!(entry.roles.contains(&role));
            }
        }
        assert_eq!(family_default(P::OpenAi, R::Lead), Some("gpt-6-luna"));
        for provider in [
            P::OpenAi,
            P::Anthropic,
            P::Google,
            P::DeepSeek,
            P::OpenRouter,
        ] {
            let listed = entries
                .iter()
                .filter(|e| e.model.provider == provider)
                .count();
            assert!((2..=6).contains(&listed), "{provider:?} lists {listed}");
        }
        assert_eq!(
            builtin_price(P::Anthropic, "claude-opus-5-5")
                .unwrap()
                .input,
            4_000_000
        );
    }

    #[test]
    fn efforts_offer_only_levels_each_wire_sends_and_defaults_keep_todays_request() {
        use WorkModelReasoning::*;
        let all = vec![Low, Medium, High, XHigh, Max];
        assert_eq!(efforts(P::OpenAi, "gpt-6-luna"), all);
        assert_eq!(efforts(P::OpenAi, "gpt-5.6-terra"), all);
        assert_eq!(efforts(P::Anthropic, "claude-opus-5-5"), all);
        assert_eq!(
            efforts(P::Anthropic, "claude-haiku-4-5-20251001"),
            vec![Low, Medium, High]
        );
        assert_eq!(efforts(P::XAi, "grok-4.7"), vec![Low, Medium, High, XHigh]);
        assert!(efforts(P::XAi, "grok-4.3").is_empty());
        assert_eq!(efforts(P::DeepSeek, "deepseek-v4-pro"), vec![Low, High]);
        assert!(efforts(P::Mistral, "mistral-medium-3-5").is_empty());
        assert_eq!(
            efforts(P::Groq, "qwen/qwen3.8-27b"),
            vec![Low, Medium, High]
        );
        assert!(efforts(P::Groq, "llama-3.3-70b-versatile").is_empty());
        assert!(efforts(P::Compatible, "llama").is_empty());

        assert_eq!(nearest_effort(&[Low, High], Medium), Some(High));
        assert_eq!(nearest_effort(&[Low, Medium, High], Max), Some(High));
        assert_eq!(
            nearest_effort(&[Low, Medium, High, XHigh], Max),
            Some(XHigh)
        );
        assert_eq!(nearest_effort(&[], Medium), None);
        let pro = efforts(P::Google, "gemini-3.1-pro-preview");
        assert_eq!(
            default_effort(P::Google, "gemini-3.1-pro-preview", &pro),
            Some(High)
        );
        assert_eq!(default_effort(P::OpenAi, "gpt-6-luna", &all), Some(Medium));
        assert_eq!(
            default_effort(P::DeepSeek, "deepseek-v4-pro", &[Low, High]),
            Some(High)
        );
    }

    #[test]
    fn the_hosted_transcriber_is_the_default_or_the_first_available_row() {
        let catalog = json!({
            "defaults": {"lead": "sonnet", "transcribe": "listen-pro"},
            "models": [
                {"id": "sonnet", "provider": "anthropic", "upstream_model": "claude-sonnet-5-5",
                 "roles": ["lead"]},
                {"id": "listen", "provider": "openai", "upstream_model": "gpt-4o-mini-transcribe",
                 "roles": ["transcribe"], "available": true,
                 "limits": {"max_request_bytes": 8388608}},
                {"id": "listen-pro", "provider": "openai", "upstream_model": "gpt-4o-transcribe",
                 "roles": ["transcribe"], "available": false}
            ]
        });
        let transcriber = parse_cloud_transcriber(&catalog).unwrap();
        assert_eq!(transcriber.id, "listen");
        assert_eq!(transcriber.upstream_model, "gpt-4o-mini-transcribe");
        assert_eq!(transcriber.max_request_bytes, Some(8_388_608));
        assert!(parse_cloud(&catalog)
            .iter()
            .all(|model| model.entry.id != "zephium/listen"));

        let mut unlocked = catalog.clone();
        unlocked["models"][2]["available"] = json!(true);
        assert_eq!(parse_cloud_transcriber(&unlocked).unwrap().id, "listen-pro");
        assert_eq!(parse_cloud_transcriber(&json!({"models": []})), None);
        assert_eq!(
            parse_cloud_transcriber(&json!({"models": [{"id": "g", "provider": "google",
                "upstream_model": "chirp", "roles": ["transcribe"]}]})),
            None
        );
    }

    #[test]
    fn the_cloud_catalog_keeps_known_rows_and_skips_the_rest() {
        let parsed = parse_cloud(&json!({"data": [
            {"id": "sonnet", "provider": "anthropic", "upstream_model": "claude-sonnet-5-5",
             "display_name": "Claude Sonnet 5.5", "roles": ["lead", "page"], "recommended": true,
             "context_window": 1000000, "max_output": 64000,
             "supports": {"tools": true, "vision": true, "native_search": true}, "plan_min": "free"},
            {"id": "x", "provider": "mystery", "upstream_model": "m", "roles": ["lead"]},
            {"id": "y", "provider": "openai", "upstream_model": "gpt-6-sol", "roles": []}
        ]}));
        assert_eq!(parsed.len(), 1);
        let model = &parsed[0];
        assert_eq!(model.entry.id, "zephium/sonnet");
        assert_eq!(model.entry.model.provider, P::Cloud);
        assert_eq!(model.entry.model.wire, WorkModelWire::AnthropicMessages);
        assert_eq!(model.upstream, P::Anthropic);
        assert!(model.entry.supports.native_search);
        assert_eq!(model.entry.price.as_ref().unwrap().output, 10_000_000);
        assert!(model.available);
    }

    #[test]
    fn the_cloud_catalog_reads_locks_limits_and_null_defaults() {
        let body = json!({
            "schema_version": 1, "policy_revision": 4, "cloud_enabled": true,
            "defaults": {"lead": "fast", "page": null, "light": "fast", "decision": "gone/../x\u{7}"},
            "models": [
                {"id": "fast", "provider": "openai", "upstream_model": "gpt-6-luna",
                 "display_name": "Fast", "description": "Quick everyday work.", "tier": "fast",
                 "available": true, "roles": ["lead", "light"],
                 "limits": {"max_request_bytes": 1048576, "max_search_calls": 2,
                            "idle_timeout_ms": 90000, "max_duration_ms": 600000}},
                {"id": "deep", "provider": "anthropic", "upstream_model": "claude-opus-5-5",
                 "display_name": "Deep", "tier": "frontier", "available": false, "roles": ["lead"]}
            ]
        });
        let parsed = parse_cloud(&body);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].available);
        assert_eq!(parsed[0].max_request_bytes, Some(1_048_576));
        assert_eq!(parsed[0].tier.as_deref(), Some("fast"));
        assert_eq!(parsed[0].max_search_calls, Some(2));
        assert_eq!(parsed[0].idle_timeout_ms, Some(90_000));
        assert_eq!(parsed[0].max_duration_ms, Some(600_000));
        assert_eq!(parsed[1].max_search_calls, None);
        assert!(!parsed[1].available);
        let defaults = parse_cloud_defaults(&body).unwrap();
        assert_eq!(defaults.get(R::Lead), Some("zephium/fast"));
        assert_eq!(defaults.get(R::Page), None);
        assert_eq!(defaults.get(R::Decision), None);
        assert_eq!(parse_cloud_defaults(&json!({"data": []})), None);
    }
}
