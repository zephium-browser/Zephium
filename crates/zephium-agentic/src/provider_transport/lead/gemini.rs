//! Gemini `streamGenerateContent`: function calls replayed with their thought
//! signatures, JSON Schema parameters, and Google Search grounding.

use std::collections::HashMap;

use serde_json::{json, Map, Value};
use zephium_core::work::model::*;

use super::sse::SseEvent;
use super::*;

const WIRE: &str = "gemini";

pub(super) fn body(
    request: &WorkModelRequest,
    target: &LeadTarget,
) -> Result<Value, WorkModelError> {
    // Function responses name their function; find it from the call's id.
    let mut names: HashMap<&str, &str> = HashMap::new();
    for message in &request.messages {
        if let WorkModelMessage::Assistant(parts) = message {
            for part in parts {
                if let WorkModelPart::ToolCall(call) = part {
                    names.insert(call.id.as_str(), call.name.as_str());
                }
            }
        }
    }
    let mut contents: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message {
            WorkModelMessage::User(parts) => {
                let mut out = Vec::new();
                for part in parts {
                    match part {
                        WorkModelPart::Text(text) if !text.is_empty() => {
                            out.push(json!({"text": text}))
                        }
                        WorkModelPart::Image { media_type, bytes } => {
                            media_type_ok(media_type)?;
                            out.push(json!({"inlineData": {"mimeType": media_type, "data": image_base64(bytes)?}}));
                        }
                        _ => {}
                    }
                }
                push(&mut contents, "user", out);
            }
            WorkModelMessage::Assistant(parts) => {
                // The model's own parts, signatures included, when this wire made them.
                let raw = parts
                    .iter()
                    .find_map(|part| replayed(part, WIRE))
                    .and_then(Value::as_array);
                let out = match raw {
                    Some(raw) => raw.clone(),
                    None => parts
                        .iter()
                        .filter_map(|part| match part {
                            WorkModelPart::Text(text) if !text.is_empty() => {
                                Some(json!({"text": text}))
                            }
                            WorkModelPart::ToolCall(call) => Some(json!({"functionCall": {
                                "id": call.id, "name": call.name, "args": call.arguments,
                            }})),
                            _ => None,
                        })
                        .collect(),
                };
                push(&mut contents, "model", out);
            }
            WorkModelMessage::ToolResults(results) => {
                let out = results
                    .iter()
                    .map(|result| {
                        let name = names.get(result.call.as_str()).copied().unwrap_or("tool");
                        let key = if result.is_error { "error" } else { "result" };
                        let mut response = json!({"name": name, "response": {key: result.content}});
                        if !result.call.starts_with("gemini_call_") {
                            response["id"] = json!(result.call);
                        }
                        json!({"functionResponse": response})
                    })
                    .collect();
                push(&mut contents, "user", out);
            }
        }
    }
    let mut tools = Vec::new();
    if !request.tools.is_empty() {
        let declarations: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| json!({"name": tool.name, "description": tool.description, "parametersJsonSchema": tool.schema}))
            .collect();
        tools.push(json!({"functionDeclarations": declarations}));
    }
    if target.native_search(request) {
        tools.push(json!({"googleSearch": {}}));
    }
    let mut generation = json!({"maxOutputTokens": request.max_output_tokens});
    if let Some(reasoning) = request.reasoning {
        let pro = request.model.model.starts_with("gemini-3-pro");
        let level = match reasoning {
            WorkModelReasoning::Low => "low",
            WorkModelReasoning::Medium if pro => "high",
            WorkModelReasoning::Medium => "medium",
            WorkModelReasoning::High | WorkModelReasoning::XHigh | WorkModelReasoning::Max => {
                "high"
            }
        };
        generation["thinkingConfig"] = json!({"thinkingLevel": level});
    }
    let mut body = Map::new();
    let system = system_text(request);
    if !system.is_empty() {
        body.insert(
            "systemInstruction".into(),
            json!({"parts": [{"text": system}]}),
        );
    }
    body.insert("contents".into(), Value::Array(contents));
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
    }
    if !request.tools.is_empty() {
        let config = match tool_choice(request)? {
            WorkModelToolChoice::Auto => json!({"mode": "AUTO"}),
            WorkModelToolChoice::Required => json!({"mode": "ANY"}),
            WorkModelToolChoice::Named(name) => {
                json!({"mode": "ANY", "allowedFunctionNames": [name]})
            }
        };
        body.insert(
            "toolConfig".into(),
            json!({"functionCallingConfig": config}),
        );
    }
    body.insert("generationConfig".into(), generation);
    Ok(Value::Object(body))
}

fn push(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut() {
        if last["role"] == role {
            if let Some(existing) = last["parts"].as_array_mut() {
                existing.extend(parts);
                return;
            }
        }
    }
    contents.push(json!({"role": role, "parts": parts}));
}

#[derive(Default)]
pub(super) struct Decoder {
    raw: Vec<Value>,
    signed: bool,
    text: String,
    typed: Vec<WorkModelPart>,
    tool_calls: usize,
    usage: WorkModelUsage,
    finish: Option<String>,
    blocked: bool,
    grounding: Option<Value>,
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
            let status = error["status"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| error["code"].to_string());
            return Err(stream_error(&status));
        }
        if data.pointer("/promptFeedback/blockReason").is_some() {
            self.blocked = true;
        }
        if let Some(usage) = data.get("usageMetadata") {
            let prompt = u64_at(usage, &["promptTokenCount"])
                .saturating_add(u64_at(usage, &["toolUsePromptTokenCount"]));
            let thoughts = u64_at(usage, &["thoughtsTokenCount"]);
            self.usage.input_tokens = prompt;
            self.usage.cached_input_tokens = u64_at(usage, &["cachedContentTokenCount"]);
            self.usage.output_tokens =
                u64_at(usage, &["candidatesTokenCount"]).saturating_add(thoughts);
            self.usage.reasoning_tokens = thoughts;
        }
        let Some(candidate) = data.pointer("/candidates/0") else {
            return Ok(Flow::Continue);
        };
        for part in candidate
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.part(part, out)?;
        }
        if let Some(grounding) = candidate.get("groundingMetadata") {
            self.grounding = Some(grounding.clone());
        }
        if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
            self.finish = Some(reason.to_owned());
        }
        Ok(Flow::Continue)
    }

    fn part(&mut self, part: &Value, out: &mut Vec<WorkModelEvent>) -> Result<(), WorkModelError> {
        let signature = part.get("thoughtSignature").is_some();
        self.signed |= signature;
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            if signature {
                self.raw
                    .push(json!({"thoughtSignature": part["thoughtSignature"], "text": ""}));
            }
            return Ok(());
        }
        if let Some(call) = part.get("functionCall") {
            if self.tool_calls >= MAX_TOOL_CALLS {
                return Err(WorkModelError::Protocol);
            }
            self.flush_text();
            let name = call["name"]
                .as_str()
                .ok_or(WorkModelError::Protocol)?
                .to_owned();
            let id = call["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("gemini_call_{}", self.tool_calls));
            let arguments = call.get("args").cloned().unwrap_or_else(|| json!({}));
            if arguments.to_string().len() > MAX_TOOL_ARGUMENT_BYTES {
                return Err(WorkModelError::Protocol);
            }
            self.tool_calls += 1;
            let call = WorkModelToolCall {
                id,
                name,
                arguments,
            };
            out.push(WorkModelEvent::ToolCall(call.clone()));
            self.typed.push(WorkModelPart::ToolCall(call));
            self.raw.push(part.clone());
            return Ok(());
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            push_text(&mut self.text, text)?;
            if !text.is_empty() {
                out.push(WorkModelEvent::Text(text.to_owned()));
            }
            // Streamed text arrives in pieces; keep one raw part per run of text.
            match self.raw.last_mut() {
                Some(last)
                    if last.get("text").is_some()
                        && last.get("functionCall").is_none()
                        && !signature =>
                {
                    let joined = format!("{}{text}", last["text"].as_str().unwrap_or_default());
                    last["text"] = json!(joined);
                }
                _ => self.raw.push(part.clone()),
            }
        }
        Ok(())
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            self.typed
                .push(WorkModelPart::Text(std::mem::take(&mut self.text)));
        }
    }

    pub(super) fn finish(
        mut self,
        out: &mut Vec<WorkModelEvent>,
    ) -> Result<Decoded, WorkModelError> {
        self.flush_text();
        let mut searches = Searches::default();
        if let Some(grounding) = &self.grounding {
            let queries: Vec<&str> = grounding["webSearchQueries"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !queries.is_empty() {
                searches.begin(None, &queries.join(" · "));
                searches.count = u32::try_from(queries.len()).unwrap_or(u32::MAX);
                let chunks = grounding["groundingChunks"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let mut passages: HashMap<usize, String> = HashMap::new();
                for support in grounding["groundingSupports"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    let text = support
                        .pointer("/segment/text")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    for index in support["groundingChunkIndices"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(index) = index.as_u64() {
                            passages
                                .entry(index as usize)
                                .or_insert_with(|| text.to_owned());
                        }
                    }
                }
                for (index, chunk) in chunks.iter().enumerate() {
                    if let Some(url) = chunk.pointer("/web/uri").and_then(Value::as_str) {
                        let title = chunk
                            .pointer("/web/title")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let passage = passages.get(&index).map(String::as_str).unwrap_or_default();
                        searches.hit(None, url, title, passage);
                    }
                }
            }
        }
        searches.flush(out);
        let stop = match self.finish.as_deref() {
            _ if self.blocked => WorkModelStop::Refused,
            Some("STOP") | None if self.tool_calls > 0 => WorkModelStop::ToolUse,
            Some("STOP") => WorkModelStop::EndTurn,
            Some("MAX_TOKENS") => WorkModelStop::MaxTokens,
            Some(
                "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII"
                | "IMAGE_SAFETY",
            ) => WorkModelStop::Refused,
            Some("MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL") => {
                return Err(WorkModelError::Protocol)
            }
            Some(_) if self.tool_calls > 0 => WorkModelStop::ToolUse,
            Some(_) => WorkModelStop::EndTurn,
            None => return Err(WorkModelError::Protocol),
        };
        let mut assistant = self.typed;
        if self.signed {
            assistant.push(replay(WIRE, Value::Array(self.raw)));
        }
        Ok(Decoded {
            stop: DecodedStop::End(stop),
            usage: self.usage,
            searches: searches.count,
            assistant,
            ..Decoded::default()
        })
    }
}
