//! Provider-neutral model calls for the lead agent and its helpers.
//!
//! One request shape for every provider (OpenAI Responses, Anthropic
//! Messages, Gemini, OpenAI-compatible chat, Zephium Cloud). Transports live in
//! `zephium-agentic`; the loop in `zephium-app` sees only these types. Text
//! fields carry model, page and person text, so `Debug` never prints them.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkModelProvider {
    OpenAi,
    Anthropic,
    Google,
    DeepSeek,
    OpenRouter,
    #[serde(rename = "xai")]
    XAi,
    Mistral,
    Groq,
    Cerebras,
    /// Any OpenAI-compatible chat endpoint the person configured.
    Compatible,
    /// Zephium Cloud: the upstream's native body, our base URL and bearer.
    Cloud,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkModelRole {
    Lead,
    Page,
    Light,
    Decision,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum WorkModelReasoning {
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

/// The wire family a model speaks; Cloud models name their upstream's.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Copy, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkModelWire {
    OpenAiResponses,
    AnthropicMessages,
    Gemini,
    ChatCompletions,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct WorkModelRef {
    pub provider: WorkModelProvider,
    pub wire: WorkModelWire,
    /// The provider's model id, sent as is.
    pub model: String,
}

#[derive(Clone)]
pub struct WorkModelSystemBlock {
    pub text: String,
    /// A cache breakpoint closes this block; nothing volatile comes before it.
    pub cache: bool,
}

#[derive(Clone)]
pub struct WorkModelTool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments object.
    pub schema: Value,
    /// The schema is closed and complete: a provider that can hold a call to
    /// it exactly (OpenAI's strict function calling) is asked to.
    pub strict: bool,
}

#[derive(Clone)]
pub struct WorkModelToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Clone)]
pub struct WorkModelToolResult {
    pub call: String,
    /// Compact JSON or text the tool returned.
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone)]
pub enum WorkModelPart {
    Text(String),
    Image {
        media_type: String,
        bytes: Vec<u8>,
    },
    ToolCall(WorkModelToolCall),
    /// A provider item the transport must replay verbatim on the next call
    /// (reasoning items, encrypted thinking, native search blocks).
    Replay(Value),
}

#[derive(Clone)]
pub enum WorkModelMessage {
    User(Vec<WorkModelPart>),
    Assistant(Vec<WorkModelPart>),
    ToolResults(Vec<WorkModelToolResult>),
}

/// Whether the model must call a tool, and which.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum WorkModelToolChoice {
    /// Text or any offered tool.
    #[default]
    Auto,
    /// One of the offered tools.
    Required,
    /// This offered tool. A wire that would refuse it with reasoning on sends
    /// the call without reasoning.
    Named(String),
}

/// Additional process-local revocation fence for model-bound page images.
/// This does not authorize pixels: native source and consent admission remain
/// separate. It is never serialized into provider requests, IPC or artifacts.
pub trait WorkModelImageFence: Send + Sync {
    /// Runs one synchronous future poll only while the original permission is
    /// current. Revocation must serialize with this callback; no await occurs
    /// inside it. False means the callback was not run.
    fn while_current(&self, operation: &mut dyn FnMut()) -> bool;
    /// Wakes when this original permission is permanently invalid. Enabling a
    /// setting again must never revive a fence created before revocation.
    fn revoked(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

#[derive(Clone)]
pub struct WorkModelRequest {
    /// Native-validated image history's additional live permission fence.
    /// Image requests without one fail closed at transport dispatch.
    pub image_fence: Option<std::sync::Arc<dyn WorkModelImageFence>>,
    pub model: WorkModelRef,
    pub system: Vec<WorkModelSystemBlock>,
    pub tools: Vec<WorkModelTool>,
    pub messages: Vec<WorkModelMessage>,
    pub max_output_tokens: u32,
    pub reasoning: Option<WorkModelReasoning>,
    /// Offer the provider's own web search tool when it has one.
    pub native_search: bool,
    pub parallel_tools: bool,
    pub tool_choice: WorkModelToolChoice,
    /// Parts closing the last message that change on every call, such as the
    /// host's running facts; a prompt cache breakpoint goes before them.
    pub volatile_tail: usize,
    /// Stable across one conversation's calls and free of its content, so a
    /// provider that routes caches by key keeps them on one cache.
    pub cache_key: Option<String>,
}

#[derive(Clone)]
pub struct WorkModelSearchHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
}

/// Streamed while a call runs, in order.
#[derive(Clone)]
pub enum WorkModelEvent {
    Text(String),
    ToolCall(WorkModelToolCall),
    /// A provider-native search the model ran inside the call.
    Search {
        query: String,
        hits: Vec<WorkModelSearchHit>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkModelStop {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refused,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WorkModelUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    /// Of `input_tokens`, those written to the provider's prompt cache.
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Cost in millionths of a US dollar, including provider tool fees, when
    /// the catalog prices the model.
    pub cost_micros: Option<u64>,
}

pub struct WorkModelOutcome {
    pub stop: WorkModelStop,
    pub usage: WorkModelUsage,
    /// The assistant turn to append to the conversation, replay items included.
    pub assistant: Vec<WorkModelPart>,
}

/// Closed failures: no provider-authored text crosses this boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkModelError {
    MissingKey,
    Unauthorized,
    RateLimited {
        retry_after_ms: Option<u64>,
    },
    Overloaded,
    ContextTooLong,
    BadRequest,
    Network,
    Protocol,
    Cancelled,
    /// The person's own provider account is out of credit.
    OverBudget,
    /// One request is larger than the provider plan allows per minute, so
    /// waiting cannot help.
    PlanLimit,
    /// Zephium Cloud's own refusal. `retry_after_ms` is the server's word on
    /// when to ask again, never a guess.
    Hosted {
        refusal: HostedRefusal,
        retry_after_ms: Option<u64>,
    },
}

/// Why Zephium Cloud refused, by its stable error codes. None of these is
/// retried on its own: each needs the person, or the clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostedRefusal {
    /// The selected model needs another plan.
    PlanRequired,
    /// The account is under billing review.
    BillingReview,
    /// Included and purchased usage are spent.
    OutOfCredits,
    /// The account's daily limit is reached; it resets at 00:00 UTC.
    DailyLimit,
    /// The free usage pool is exhausted for now.
    FreeUsagePaused,
    /// Too many earlier requests are still being confirmed.
    ConfirmationLimited,
    /// The hosted service is stopped for spending protection.
    Paused,
    /// The same logical call already reached the server.
    AlreadySent,
    /// A recording the server could not read.
    InvalidAudio,
    /// A recording in an encoding the server does not take.
    UnsupportedAudio,
    /// A recording longer than the server takes.
    AudioTooLong,
}

/// A cache key for one conversation, from what stays fixed across its calls:
/// the system text and the tools' names. A hash, so no text leaves in it.
pub fn work_model_cache_key(system: &[WorkModelSystemBlock], tools: &[WorkModelTool]) -> String {
    // FNV-1a: the same key on every build, unlike the standard hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let parts = system
        .iter()
        .map(|block| block.text.as_str())
        .chain(tools.iter().map(|tool| tool.name.as_str()));
    for part in parts {
        for byte in part.bytes().chain([0]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("zk_{hash:016x}")
}

impl fmt::Display for WorkModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for WorkModelError {}

pub type WorkModelFuture<'a> =
    Pin<Box<dyn Future<Output = Result<WorkModelOutcome, WorkModelError>> + Send + 'a>>;

pub trait WorkModelClient: Send + Sync {
    /// One model call. `events` receives text, tool calls and native searches
    /// as they stream; dropping the future cancels the call.
    fn call<'a>(
        &'a self,
        request: WorkModelRequest,
        events: &'a (dyn Fn(WorkModelEvent) + Send + Sync),
    ) -> WorkModelFuture<'a>;
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct WorkModelSupports {
    pub tools: bool,
    pub vision: bool,
    pub prompt_cache: bool,
    pub reasoning: bool,
    pub native_search: bool,
}

/// Per million tokens, in millionths of a US dollar.
#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct WorkModelPrice {
    #[cfg_attr(feature = "ipc-types", specta(type = u32))]
    pub input: u64,
    #[cfg_attr(feature = "ipc-types", specta(type = u32))]
    pub cached_input: u64,
    #[cfg_attr(feature = "ipc-types", specta(type = u32))]
    pub output: u64,
}

#[cfg_attr(feature = "ipc-types", derive(specta::Type))]
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct WorkModelEntry {
    /// Stable catalog id, such as `anthropic/claude-sonnet-5`.
    pub id: String,
    pub model: WorkModelRef,
    pub display_name: String,
    pub roles: Vec<WorkModelRole>,
    pub recommended: bool,
    pub context_window: u32,
    pub max_output: u32,
    pub supports: WorkModelSupports,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<WorkModelPrice>,
}

impl WorkModelEntry {
    /// A compatible model whose resolved price carries no usage charge.
    pub fn is_free_compatible(&self) -> bool {
        self.model.provider == WorkModelProvider::Compatible
            && self.price.as_ref().is_some_and(|price| {
                price.input == 0 && price.cached_input == 0 && price.output == 0
            })
    }
}

/// Pricing classification only; this grants no transport or network permission.
pub fn local_model_endpoint(base: &str) -> bool {
    fn local_ipv4(ip: std::net::Ipv4Addr) -> bool {
        ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || matches!(ip.octets(), [100, 64..=127, _, _])
    }
    let Ok(url) = url::Url::parse(base) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => local_ipv4(ip),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(local_ipv4)
        }
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.');
            host == "localhost"
                || host == "host.docker.internal"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
        }
        None => false,
    }
}

macro_rules! redacted_debug {
    ($($ty:ty),* $(,)?) => {$(
        impl fmt::Debug for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($ty), "([content redacted])"))
            }
        }
    )*};
}

redacted_debug!(
    WorkModelSystemBlock,
    WorkModelTool,
    WorkModelToolCall,
    WorkModelToolResult,
    WorkModelPart,
    WorkModelMessage,
    WorkModelRequest,
    WorkModelSearchHit,
    WorkModelEvent,
    WorkModelOutcome,
);

#[cfg(test)]
mod local_endpoint_tests {
    use super::local_model_endpoint;

    #[test]
    fn tailscale_and_docker_endpoints_are_local_for_pricing_only() {
        for base in [
            "http://100.64.0.0:11434/v1",
            "https://100.100.1.2/v1",
            "https://100.127.255.255/v1",
            "https://[::ffff:100.100.1.2]/v1",
            "http://host.docker.internal:1234/v1",
            "https://HOST.DOCKER.INTERNAL./v1",
        ] {
            assert!(local_model_endpoint(base), "{base}");
        }
        for base in [
            "https://100.63.255.255/v1",
            "https://100.128.0.0/v1",
            "https://[::ffff:100.128.0.0]/v1",
            "https://host.docker.internal.example/v1",
            "ftp://host.docker.internal/v1",
        ] {
            assert!(!local_model_endpoint(base), "{base}");
        }
    }
}
