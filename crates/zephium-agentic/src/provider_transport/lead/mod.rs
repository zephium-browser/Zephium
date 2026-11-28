    }
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds.min(86_400)));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(
        at.duration_since(std::time::SystemTime::now())
            .unwrap_or_default(),
    )
}

/// A failed answer as a closed error, with the wait the server named: the
/// larger of its header and, for Cloud, its body.
fn refused(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    cloud: bool,
) -> (WorkModelError, Option<Duration>) {
    let wait = cloud::server_wait(headers, body, cloud);
    let error = if cloud {
        classify_cloud(status, body)
    } else {
        classify(status, body)
    };
    (cloud::with_wait(error, wait, cloud), wait)
}

/// Maps a failed HTTP answer to a closed error. The body is read only for
/// provider error codes; none of its text leaves this function.
fn classify(status: StatusCode, body: &[u8]) -> WorkModelError {
    let json: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = json.get("error").unwrap_or(&json);
    let code = [
        error.get("code").and_then(Value::as_str),
        error.get("type").and_then(Value::as_str),
        error.get("status").and_then(Value::as_str),
    ];
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let too_long = code.contains(&Some("context_length_exceeded"))
        || message.contains("prompt is too long")
        || message.contains("context length")
        || message.contains("context window")
        || message.contains("maximum context")
        || message.contains("input token count");
    // Groq and OpenAI refuse a single request above the plan's tokens per
    // minute as "Request too large ... on tokens per minute (TPM)".
    let over_plan = message.contains("request too large")
        && (message.contains("per minute")
            || message.contains("per min")
            || message.contains("(tpm)"));
    match status.as_u16() {
        401 | 403 => WorkModelError::Unauthorized,
        402 => WorkModelError::OverBudget,
        413 | 429 if over_plan => WorkModelError::PlanLimit,
        413 => WorkModelError::ContextTooLong,
        429 if code.contains(&Some("insufficient_quota")) => WorkModelError::OverBudget,
        400..=499
            if code.iter().flatten().any(|code| {
                matches!(
                    *code,
                    "credit_balance_exhausted"
                        | "billing_hard_limit_reached"
                        | "billing_not_active"
                )
            }) || message.contains("credit balance") =>
        {
            WorkModelError::OverBudget
        }
        429 => WorkModelError::RateLimited {
            retry_after_ms: None,
        },
        500..=599 => WorkModelError::Overloaded,
        400 | 404 | 422 if too_long => WorkModelError::ContextTooLong,
        400..=499 => WorkModelError::BadRequest,
        _ => WorkModelError::Protocol,
    }
}

/// A random UUID v4, as Cloud requires for `Idempotency-Key`.
fn idempotency_key() -> Result<HeaderValue, WorkModelError> {
    let mut bits = u128::from(ulid::Ulid::new());
    bits ^= u128::from(ulid::Ulid::new()).rotate_left(48);
    bits = (bits & !(0xF << 76)) | (0x4 << 76);
    bits = (bits & !(0x3 << 62)) | (0x2 << 62);
    let hex = format!("{bits:032x}");
    let text = format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    HeaderValue::from_str(&text).map_err(|_| WorkModelError::Protocol)
}

/// Closed mapping of in-stream error codes shared by the wires.
pub(crate) fn stream_error(code: &str) -> WorkModelError {
    match code {
        "rate_limit_exceeded" | "rate_limit_error" | "RESOURCE_EXHAUSTED" | "429" => {
            WorkModelError::RateLimited {
                retry_after_ms: None,
            }
        }
        "overloaded_error"
        | "server_error"
        | "api_error"
        | "server_is_overloaded"
        | "slow_down"
        | "UNAVAILABLE"
        | "INTERNAL"
        | "500"
        | "502"
        | "503"
        | "529" => WorkModelError::Overloaded,
        "context_length_exceeded" => WorkModelError::ContextTooLong,
        "authentication_error"
        | "permission_error"
        | "PERMISSION_DENIED"
        | "UNAUTHENTICATED"
        | "401"
        | "403" => WorkModelError::Unauthorized,
        "insufficient_quota"
        | "credit_balance_exhausted"
        | "billing_hard_limit_reached"
        | "billing_not_active"
        | "402" => WorkModelError::OverBudget,
        "invalid_request_error" | "invalid_prompt" | "INVALID_ARGUMENT" | "400" => {
            WorkModelError::BadRequest
        }
        _ => WorkModelError::Protocol,
    }
}

pub(crate) enum Flow {
    Continue,
    Done,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DecodedStop {
    #[default]
    Open,
    End(WorkModelStop),
    /// Anthropic `pause_turn`: a server tool loop wants the turn sent back.
    Paused,
}

#[derive(Default)]
pub(crate) struct Decoded {
    pub(crate) stop: DecodedStop,
    pub(crate) usage: WorkModelUsage,
    pub(crate) cache_write_tokens: u64,
    pub(crate) searches: u32,
    pub(crate) assistant: Vec<WorkModelPart>,
    /// The provider's own cost report (OpenRouter), in micro-USD.
    pub(crate) provider_cost_micros: Option<u64>,
    resumed: bool,
}

impl Decoded {
    fn absorb(&mut self, next: Decoded) {
        self.stop = next.stop;
        self.usage.input_tokens = self
            .usage
            .input_tokens
            .saturating_add(next.usage.input_tokens);
        self.usage.cached_input_tokens = self
            .usage
            .cached_input_tokens
            .saturating_add(next.usage.cached_input_tokens);
        self.usage.output_tokens = self
            .usage
            .output_tokens
            .saturating_add(next.usage.output_tokens);
        self.usage.reasoning_tokens = self
            .usage
            .reasoning_tokens
            .saturating_add(next.usage.reasoning_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(next.cache_write_tokens);
        self.searches = self.searches.saturating_add(next.searches);
        self.provider_cost_micros = match (self.provider_cost_micros, next.provider_cost_micros) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (None, b) if !self.resumed => b,
            _ => None,
        };
        self.assistant.extend(next.assistant);
    }
}

enum WireDecoder {
    OpenAi(openai::Decoder),
    Anthropic(anthropic::Decoder),
    Gemini(gemini::Decoder),
    Chat(chat::Decoder),
}

impl WireDecoder {
    fn new(wire: WorkModelWire) -> Self {
        Self::for_target(wire, false)
    }

    fn for_target(wire: WorkModelWire, go: bool) -> Self {
        match wire {
            WorkModelWire::OpenAiResponses => Self::OpenAi(Default::default()),
            WorkModelWire::AnthropicMessages => Self::Anthropic(Default::default()),
            WorkModelWire::Gemini => Self::Gemini(Default::default()),
            WorkModelWire::ChatCompletions => Self::Chat(chat::Decoder::new(go)),
        }
    }

    fn event(
        &mut self,
        frame: &sse::SseEvent,
        out: &mut Vec<WorkModelEvent>,
    ) -> Result<Flow, WorkModelError> {
        match self {
            Self::OpenAi(decoder) => decoder.event(frame, out),
            Self::Anthropic(decoder) => decoder.event(frame, out),
            Self::Gemini(decoder) => decoder.event(frame, out),
            Self::Chat(decoder) => decoder.event(frame, out),
        }
    }

    fn finish(self, out: &mut Vec<WorkModelEvent>) -> Result<Decoded, WorkModelError> {
        match self {
            Self::OpenAi(decoder) => decoder.finish(out),
            Self::Anthropic(decoder) => decoder.finish(out),
            Self::Gemini(decoder) => decoder.finish(out),
            Self::Chat(decoder) => decoder.finish(out),
        }
    }
}

/// Parses one SSE data payload.
pub(crate) fn json(data: &str) -> Result<Value, WorkModelError> {
    serde_json::from_str(data).map_err(|_| WorkModelError::Protocol)
}

pub(crate) fn u64_at(value: &Value, path: &[&str]) -> u64 {
    let mut cursor = value;
    for key in path {
        match cursor.get(key) {
            Some(next) => cursor = next,
            None => return 0,
        }
    }
    cursor.as_u64().unwrap_or(0)
}

/// Tool arguments as JSON; unparsable arguments reach the loop as a string so
/// its schema check can hand the model a correctable fault.
pub(crate) fn arguments(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return Value::Object(Default::default());
    }
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

pub(crate) fn push_text(buffer: &mut String, text: &str) -> Result<(), WorkModelError> {
    if buffer.len() + text.len() > MAX_TEXT_BYTES {
        return Err(WorkModelError::Protocol);
    }
    buffer.push_str(text);
    Ok(())
}

pub(crate) fn image_base64(bytes: &[u8]) -> Result<String, WorkModelError> {
    use base64::Engine as _;
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(WorkModelError::BadRequest);
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

pub(crate) fn media_type_ok(media_type: &str) -> Result<(), WorkModelError> {
    match media_type {
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" => Ok(()),
        _ => Err(WorkModelError::BadRequest),
    }
}

/// Replay items are tagged with the wire that produced them; another wire
/// drops them, so switching models mid-work stays valid.
/// Drops provider items another provider produced. Wires are shared (DeepSeek
/// and Groq both speak Chat Completions, OpenAI and xAI both Responses), but
/// reasoning items and signatures only make sense to the provider that made
/// them. Items from before origins were recorded stay.
fn strip_foreign_replays(request: &mut WorkModelRequest, origin: &str) {
    for message in &mut request.messages {
        if let WorkModelMessage::Assistant(parts) = message {
            parts.retain(|part| match part {
                WorkModelPart::Replay(value) => value
                    .get("origin")
                    .and_then(Value::as_str)
                    .is_none_or(|from| from == origin),
                _ => true,
            });
        }
    }
}

pub(crate) fn replay(wire: &str, value: Value) -> WorkModelPart {
    WorkModelPart::Replay(serde_json::json!({ "wire": wire, "value": value }))
}

pub(crate) fn replayed<'a>(part: &'a WorkModelPart, wire: &str) -> Option<&'a Value> {
    match part {
        WorkModelPart::Replay(value) if value.get("wire").and_then(Value::as_str) == Some(wire) => {
            value.get("value")
        }
        _ => None,
    }
}

/// The request's tool choice, refused when it forces a tool it does not offer.
pub(crate) fn tool_choice(
    request: &WorkModelRequest,
) -> Result<&WorkModelToolChoice, WorkModelError> {
    let offered = |name: &str| request.tools.iter().any(|tool| tool.name == name);
    match &request.tool_choice {
        WorkModelToolChoice::Required if request.tools.is_empty() => {
            Err(WorkModelError::BadRequest)
        }
        WorkModelToolChoice::Named(name) if !offered(name) => Err(WorkModelError::BadRequest),
        choice => Ok(choice),
    }
}

pub(crate) fn system_text(request: &WorkModelRequest) -> String {
    request
        .system
        .iter()
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Native searches a call ran, completed with the titles and passages the
/// answer cited, then released as events once the answer text settles.
#[derive(Default)]
pub(crate) struct Searches {
    pending: Vec<(Option<String>, WorkModelSearchHitSet)>,
    pub(crate) count: u32,
}

#[derive(Default)]
pub(crate) struct WorkModelSearchHitSet {
    query: String,
    hits: Vec<WorkModelSearchHit>,
}

impl Searches {
    pub(crate) fn begin(&mut self, key: Option<String>, query: &str) {
        self.count = self.count.saturating_add(1);
        if self.pending.len() >= MAX_SEARCHES {
            return;
        }
        self.pending.push((
            key,
            WorkModelSearchHitSet {
                query: clip(query, 400),
                hits: Vec::new(),
            },
        ));
    }

    pub(crate) fn hit(&mut self, key: Option<&str>, url: &str, title: &str, snippet: &str) {
        let Some(url) = web_url(url) else { return };
        let set = match key {
            Some(key) => self
                .pending
                .iter_mut()
                .rev()
                .find(|(pending, _)| pending.as_deref() == Some(key)),
            None => self.pending.last_mut(),
        };
        let Some((_, set)) = set else { return };
        if let Some(existing) = set.hits.iter_mut().find(|hit| hit.url == url) {
            if existing.title.is_empty() {
                existing.title = clip(title, 300);
            }
            if existing.snippet.is_empty() {
                existing.snippet = clip(snippet, 400);
            }
        } else if set.hits.len() < MAX_HITS {
            set.hits.push(WorkModelSearchHit {
                url,
                title: clip(title, 300),
                snippet: clip(snippet, 400),
            });
        }
    }

    /// A citation in the answer: fills the hit it names, or joins the last search.
    pub(crate) fn cite(&mut self, url: &str, title: &str, passage: &str) {
        let Some(clean) = web_url(url) else { return };
        let known = self
            .pending
            .iter()
            .rev()
            .find(|(_, set)| set.hits.iter().any(|hit| hit.url == clean))
            .map(|(key, _)| key.clone());
        match known {
            Some(key) => self.hit(key.as_deref(), url, title, passage),
            None => self.hit(None, url, title, passage),
        }
    }

    pub(crate) fn flush(&mut self, out: &mut Vec<WorkModelEvent>) {
        for (_, set) in self.pending.drain(..) {
            out.push(WorkModelEvent::Search {
                query: set.query,
                hits: set.hits,
            });
        }
    }
}

fn web_url(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    (matches!(parsed.scheme(), "https" | "http") && url.len() <= 2048).then(|| url.to_owned())
}

pub(crate) fn clip(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    match text.char_indices().nth(max_chars) {
        Some((end, _)) => text[..end].to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_tests;
