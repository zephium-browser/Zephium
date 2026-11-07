//! OpenAI-compatible Chat Completions: DeepSeek, OpenRouter, Mistral, Groq,
//! Cerebras and custom endpoints. Reasoning the provider requires back (DeepSeek
//! `reasoning_content`, OpenRouter `reasoning_details`) rides in a replay part.

use serde_json::{json, Map, Value};
use zephium_core::work::model::*;

use super::sse::SseEvent;
use super::*;

const WIRE: &str = "chat";
/// A replay key, not a field: it becomes the message's leading thinking chunk.
const MISTRAL_THINKING: &str = "thinking";

pub(super) fn body(
    request: &WorkModelRequest,
    target: &LeadTarget,
) -> Result<Value, WorkModelError> {
    let upstream = target.upstream();
    let mut messages = Vec::new();
    let cache_marks =
        upstream == WorkModelProvider::OpenRouter && request.system.iter().any(|block| block.cache);
    if cache_marks {
        // OpenRouter forwards breakpoints to providers that need them.
        let parts: Vec<Value> = request
            .system
            .iter()
            .filter(|block| !block.text.is_empty())
            .map(|block| {
                let mut part = json!({"type": "text", "text": block.text});
                if block.cache {
                    part["cache_control"] = json!({"type": "ephemeral"});
                }
                part
            })
            .collect();
        messages.push(json!({"role": "system", "content": parts}));
    } else {
        let system = system_text(request);
        if !system.is_empty() {
            messages.push(json!({"role": "system", "content": system}));
        }
    }
    for message in &request.messages {
        match message {
            WorkModelMessage::User(parts) => {
                let images = parts
                    .iter()
                    .any(|part| matches!(part, WorkModelPart::Image { .. }));
                if images {
                    let mut content = Vec::new();
                    for part in parts {
                        match part {
                            WorkModelPart::Text(text) => {
                                content.push(json!({"type": "text", "text": text}))
                            }
                            WorkModelPart::Image { media_type, bytes } => {
                                media_type_ok(media_type)?;
                                content.push(json!({"type": "image_url", "image_url": {
                                    "url": format!("data:{media_type};base64,{}", image_base64(bytes)?),
                                }}));
                            }
                            _ => {}
                        }
                    }
                    messages.push(json!({"role": "user", "content": content}));
                } else {
                    let text: Vec<&str> = parts
                        .iter()
                        .filter_map(|part| match part {
                            WorkModelPart::Text(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    if !text.is_empty() {
                        messages.push(json!({"role": "user", "content": text.join("\n\n")}));
                    }
                }
            }
            WorkModelMessage::Assistant(parts) => {
                let mut text = String::new();
                let mut calls = Vec::new();
                let mut thinking = None;
                let mut message = json!({"role": "assistant"});
                for part in parts {
                    match part {
                        WorkModelPart::Text(piece) => text.push_str(piece),
                        WorkModelPart::ToolCall(call) => calls.push(json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": serde_json::to_string(&call.arguments).map_err(|_| WorkModelError::BadRequest)?,
                            },
                        })),
                        part => {
                            if let Some(extra) = replayed(part, WIRE).and_then(Value::as_object) {
                                for (key, value) in extra {
                                    if key == MISTRAL_THINKING {
                                        thinking = value.as_str().map(str::to_owned);
                                    } else {
                                        message[key] = value.clone();
                                    }
                                }
                            }
                        }
                    }
                }
                message["content"] = match thinking {
                    Some(thinking) if upstream == WorkModelProvider::Mistral => {
                        let mut chunks = vec![json!({
                            "type": "thinking",
                            "thinking": [{"type": "text", "text": thinking}],
                        })];
                        if !text.is_empty() {
                            chunks.push(json!({"type": "text", "text": text}));
                        }
                        Value::Array(chunks)
                    }
                    _ if text.is_empty() => Value::Null,
                    _ => json!(text),
                };
                if !calls.is_empty() {
                    message["tool_calls"] = Value::Array(calls);
                }
                if !text.is_empty() || message.get("tool_calls").is_some() {
                    messages.push(message);
                }
            }
            WorkModelMessage::ToolResults(results) => {
                for result in results {
                    let content = if result.is_error {
                        format!("Error: {}", result.content)
                    } else {
                        result.content.clone()
                    };
                    messages.push(
                        json!({"role": "tool", "tool_call_id": result.call, "content": content}),
                    );
                }
            }
        }
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(request.model.model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("stream".into(), json!(true));
    // Mistral refuses fields it does not define, and reports usage on its
    // last chunk without being asked.
    if upstream != WorkModelProvider::Mistral {
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    body.insert("max_tokens".into(), json!(request.max_output_tokens));
    if !request.tools.is_empty() {
        // Held to their schemas, the browser tools cannot come back malformed.
        let strict = target.strict_tools(request);
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut function = json!({
                    "name": tool.name, "description": tool.description, "parameters": tool.schema,
                });
                if strict {
                    function["strict"] = json!(true);
                }
                json!({"type": "function", "function": function})
            })
            .collect();
        let choice = match tool_choice(request)? {
            WorkModelToolChoice::Auto => json!("auto"),
            WorkModelToolChoice::Required => json!("required"),
            WorkModelToolChoice::Named(name) => {
                json!({"type": "function", "function": {"name": name}})
            }
        };
        body.insert("tools".into(), Value::Array(tools));
        body.insert("tool_choice".into(), choice);
        if !request.parallel_tools && upstream != WorkModelProvider::Cerebras {
            body.insert("parallel_tool_calls".into(), json!(false));
        }
    }
    // DeepSeek's thinking mode does not take a forced tool.
    let forced = !request.tools.is_empty() && request.tool_choice != WorkModelToolChoice::Auto;
    let reasoning = request
        .reasoning
        .filter(|_| !(forced && upstream == WorkModelProvider::DeepSeek));
    match (upstream, reasoning) {
        (WorkModelProvider::DeepSeek, None) => {
            body.insert("thinking".into(), json!({"type": "disabled"}));
        }
        (WorkModelProvider::DeepSeek, Some(reasoning)) => {
            body.insert("thinking".into(), json!({"type": "enabled"}));
            let effort = if reasoning == WorkModelReasoning::Low {
                "low"
            } else {
                "high"
            };
            body.insert("reasoning_effort".into(), json!(effort));
        }
        (WorkModelProvider::Groq | WorkModelProvider::Cerebras, Some(reasoning))
            if takes_effort(upstream, &request.model.model) =>
        {
            let effort = match reasoning {
                WorkModelReasoning::Low => "low",
                WorkModelReasoning::Medium => "medium",
                WorkModelReasoning::High | WorkModelReasoning::XHigh | WorkModelReasoning::Max => {
                    "high"
                }
            };
            body.insert("reasoning_effort".into(), json!(effort));
        }
        (WorkModelProvider::OpenRouter, Some(reasoning)) => {
            let effort = match reasoning {
                WorkModelReasoning::Low => "low",
                WorkModelReasoning::Medium => "medium",
                WorkModelReasoning::High | WorkModelReasoning::XHigh | WorkModelReasoning::Max => {
                    "high"
                }
            };
            body.insert("reasoning".into(), json!({"effort": effort}));
        }
        _ => {}
    }
    if upstream == WorkModelProvider::OpenRouter {
        body.insert("usage".into(), json!({"include": true}));
    }
    // Groq's Qwen streams its thinking inline unless told otherwise, and
    // refuses inline thinking beside tools.
    if upstream == WorkModelProvider::Groq && request.model.model.starts_with("qwen/") {
        body.insert("reasoning_format".into(), json!("hidden"));
    }
    Ok(Value::Object(body))
}

/// Groq and Cerebras take an effort for GPT-OSS and Qwen. Any other model
/// there refuses the field.
pub(super) fn takes_effort(upstream: WorkModelProvider, model: &str) -> bool {
    model.contains("gpt-oss")
        || (matches!(
            upstream,
            WorkModelProvider::Groq | WorkModelProvider::Cerebras
        ) && model.trim_start_matches("qwen/").starts_with("qwen"))
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub(super) struct Decoder {
    text: String,
    reasoning: String,
    /// Mistral's reasoning, which it wants back as a thinking chunk.
    thinking: String,
    details: Vec<Value>,
    calls: Vec<PendingCall>,
    finish: Option<String>,
    usage: WorkModelUsage,
    cost_micros: Option<u64>,
}

impl Decoder {
    pub(super) fn event(
        &mut self,
        frame: &SseEvent,
        out: &mut Vec<WorkModelEvent>,
    ) -> Result<Flow, WorkModelError> {
        if frame.data.trim() == "[DONE]" {
            return Ok(Flow::Done);
        }
        let data = json(&frame.data)?;
        if let Some(error) = data.get("error") {
            let code = error["code"]
                .as_str()
                .map(str::to_owned)
                .or_else(|| error["code"].as_u64().map(|code| code.to_string()))
                .or_else(|| error["type"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(stream_error(&code));
        }
        if let Some(usage) = data.get("usage").filter(|usage| !usage.is_null()) {
            self.usage.input_tokens = u64_at(usage, &["prompt_tokens"]);
            self.usage.output_tokens = u64_at(usage, &["completion_tokens"]);
            self.usage.cached_input_tokens =
                u64_at(usage, &["prompt_tokens_details", "cached_tokens"])
                    .max(u64_at(usage, &["prompt_cache_hit_tokens"]));
            self.usage.reasoning_tokens =
                u64_at(usage, &["completion_tokens_details", "reasoning_tokens"]);
            self.cost_micros = usage
                .get("cost")
                .and_then(Value::as_f64)
                .filter(|cost| cost.is_finite() && *cost >= 0.0)
                .map(|cost| (cost * 1_000_000.0).round() as u64);
        }
        let Some(choice) = data.pointer("/choices/0") else {
            return Ok(Flow::Continue);
        };
        let delta = &choice["delta"];
        if let Some(text) = delta["content"].as_str() {
            push_text(&mut self.text, text)?;
            if !text.is_empty() {
                out.push(WorkModelEvent::Text(text.to_owned()));
            }
        }
        // Mistral streams a reasoning model's content as typed chunks.
        for chunk in delta["content"].as_array().into_iter().flatten() {
            match chunk["type"].as_str() {
                Some("text") => {
                    let text = chunk["text"].as_str().unwrap_or_default();
                    push_text(&mut self.text, text)?;
                    if !text.is_empty() {
                        out.push(WorkModelEvent::Text(text.to_owned()));
                    }
                }
                Some("thinking") => {
                    for part in chunk["thinking"].as_array().into_iter().flatten() {
                        push_text(
                            &mut self.thinking,
                            part["text"].as_str().unwrap_or_default(),
                        )?;
                    }
                }
                _ => {}
            }
        }
        if let Some(reasoning) = delta["reasoning_content"].as_str() {
            push_text(&mut self.reasoning, reasoning)?;
        }
        for detail in delta["reasoning_details"].as_array().into_iter().flatten() {
            self.detail(detail)?;
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let index = call["index"].as_u64().unwrap_or(self.calls.len() as u64) as usize;
            if index >= MAX_TOOL_CALLS {
                return Err(WorkModelError::Protocol);
            }
            if self.calls.len() <= index {
                self.calls.resize_with(index + 1, PendingCall::default);
            }
            let pending = &mut self.calls[index];
            if let Some(id) = call["id"].as_str() {
                pending.id = id.to_owned();
            }
            if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                pending.name.push_str(name);
            }
            if let Some(piece) = call.pointer("/function/arguments").and_then(Value::as_str) {
                if pending.arguments.len() + piece.len() > MAX_TOOL_ARGUMENT_BYTES {
                    return Err(WorkModelError::Protocol);
                }
                pending.arguments.push_str(piece);
            }
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish = Some(reason.to_owned());
        }
        Ok(Flow::Continue)
    }

    /// Streamed reasoning details merge by index; strings concatenate.
    fn detail(&mut self, detail: &Value) -> Result<(), WorkModelError> {
        let index = detail.get("index").and_then(Value::as_u64);
        let known_count = self.details.len();
        let existing = index.and_then(|index| {
            self.details
                .iter_mut()
                .find(|known| known.get("index").and_then(Value::as_u64) == Some(index))
        });
        match (existing, detail.as_object()) {
            (Some(Value::Object(known)), Some(fields)) => {
                for (key, value) in fields {
                    match (known.get_mut(key), value) {
                        (Some(Value::String(text)), Value::String(more))
                            if key != "type" && key != "id" && key != "format" =>
                        {
                            push_text(text, more)?
                        }
                        _ => {
                            known.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
            _ if known_count < 256 => self.details.push(detail.clone()),
            _ => return Err(WorkModelError::Protocol),
        }
        Ok(())
    }

    pub(super) fn finish(self, out: &mut Vec<WorkModelEvent>) -> Result<Decoded, WorkModelError> {
        let finish = self.finish.ok_or(WorkModelError::Protocol)?;
        let mut assistant = Vec::new();
        let mut extra = Map::new();
        if !self.reasoning.is_empty() {
            extra.insert("reasoning_content".into(), json!(self.reasoning));
        }
        if !self.thinking.is_empty() {
            extra.insert(MISTRAL_THINKING.into(), json!(self.thinking));
        }
        if !self.details.is_empty() {
            extra.insert("reasoning_details".into(), Value::Array(self.details));
        }
        if !extra.is_empty() {
            assistant.push(replay(WIRE, Value::Object(extra)));
        }
        if !self.text.is_empty() {
            assistant.push(WorkModelPart::Text(self.text));
        }
        let mut calls = 0;
        for (index, pending) in self.calls.into_iter().enumerate() {
            if pending.name.is_empty() {
                continue;
            }
            let id = if pending.id.is_empty() {
                format!("call_{index}")
            } else {
                pending.id
            };
            let call = WorkModelToolCall {
                id,
                name: pending.name,
                arguments: arguments(&pending.arguments),
            };
            out.push(WorkModelEvent::ToolCall(call.clone()));
            assistant.push(WorkModelPart::ToolCall(call));
            calls += 1;
        }
        let stop = match finish.as_str() {
            "length" => WorkModelStop::MaxTokens,
            "content_filter" => WorkModelStop::Refused,
            "error" => return Err(WorkModelError::Overloaded),
            _ if calls > 0 => WorkModelStop::ToolUse,
            _ => WorkModelStop::EndTurn,
        };
        Ok(Decoded {
            stop: DecodedStop::End(stop),
            usage: self.usage,
            assistant,
            provider_cost_micros: self.cost_micros,
            ..Decoded::default()
        })
    }
}
