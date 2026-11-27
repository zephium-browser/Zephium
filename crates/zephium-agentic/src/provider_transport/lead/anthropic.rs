//! Anthropic Messages: streamed tool use, thinking blocks replayed with their
//! signatures, `cache_control` breakpoints and the server web search tool.

use serde_json::{json, Map, Value};
use zephium_core::work::model::*;

use super::sse::SseEvent;
use super::*;

const WIRE: &str = "anthropic";
const MAX_BREAKPOINTS: usize = 4;
const SEARCH_MAX_USES: u32 = 8;

/// Models before adaptive thinking take a token budget instead of effort;
/// they refuse `adaptive`, and every newer model refuses a budget.
pub(super) fn budget_thinking(model: &str) -> bool {
    [
        "claude-3",
        "claude-haiku-4",
        "claude-opus-4-5",
        "claude-sonnet-4-5",
        "claude-opus-4-1",
        "claude-opus-4-2025",
        "claude-sonnet-4-2025",
    ]
    .iter()
    .any(|prefix| model.starts_with(prefix))
}

/// Direct search without dynamic filtering. The filtering versions run search
/// from provisioned code execution, whose blocks a replay must carry exactly,
/// and Zephium Cloud admits only this version.
const SEARCH_TOOL: &str = "web_search_20250305";

pub(super) fn body(
    request: &WorkModelRequest,
    target: &LeadTarget,
) -> Result<Value, WorkModelError> {
    let mut breakpoints = 0;
    let system: Vec<Value> = request
        .system
        .iter()
        .filter(|block| !block.text.is_empty())
        .map(|block| {
            let mut value = json!({"type": "text", "text": block.text});
            if block.cache && breakpoints < MAX_BREAKPOINTS - 1 {
                breakpoints += 1;
                value["cache_control"] = json!({"type": "ephemeral"});
            }
            value
        })
        .collect();
    let mut messages: Vec<Value> = Vec::new();
    for message in &request.messages {
        match message {
            WorkModelMessage::User(parts) => {
                let mut content = Vec::new();
                for part in parts {
                    match part {
                        WorkModelPart::Text(text) if !text.is_empty() => {
                            content.push(json!({"type": "text", "text": text}))
                        }
                        WorkModelPart::Image { media_type, bytes } => {
                            media_type_ok(media_type)?;
                            content.push(json!({"type": "image", "source": {
                                "type": "base64", "media_type": media_type, "data": image_base64(bytes)?,
                            }}));
                        }
                        _ => {}
                    }
                }
                push(&mut messages, "user", content);
            }
            WorkModelMessage::Assistant(parts) => {
                let mut content = Vec::new();
                for part in parts {
                    match part {
                        WorkModelPart::Text(text) if !text.is_empty() => {
                            content.push(json!({"type": "text", "text": text}))
                        }
                        WorkModelPart::ToolCall(call) => content.push(json!({
                            "type": "tool_use", "id": call.id, "name": call.name, "input": call.arguments,
                        })),
                        part => {
                            if let Some(block) = replayed(part, WIRE) {
                                content.push(block.clone());
                            }
                        }
                    }
                }
                push(&mut messages, "assistant", content);
            }
            WorkModelMessage::ToolResults(results) => {
                let content = results
                    .iter()
                    .map(|result| {
                        let mut block = json!({
                            "type": "tool_result", "tool_use_id": result.call, "content": result.content,
                        });
                        if result.is_error {
                            block["is_error"] = json!(true);
                        }
                        block
                    })
                    .collect();
                push(&mut messages, "user", content);
            }
        }
    }
    // Conversation caching: the newest cacheable block closes a breakpoint,
    // short of the volatile blocks that close the conversation.
    if breakpoints < MAX_BREAKPOINTS {
        if let Some(block) = messages
            .iter_mut()
            .rev()
            .filter_map(|message| message["content"].as_array_mut())
            .flat_map(|content| content.iter_mut().rev())
            .skip(request.volatile_tail)
            .find(|block| {
                matches!(
                    block["type"].as_str(),
                    Some("text" | "tool_result" | "image" | "tool_use")
                )
            })
        {
            block["cache_control"] = json!({"type": "ephemeral"});
        }
    }
    let mut tools: Vec<Value> = request
        .tools
        .iter()
        .map(|tool| json!({"name": tool.name, "description": tool.description, "input_schema": tool.schema}))
        .collect();
    let model = request.model.model.as_str();
    if target.native_search(request) {
        let uses = target
            .search_calls
            .map_or(SEARCH_MAX_USES, |calls| calls.min(SEARCH_MAX_USES));
        tools.push(json!({"type": SEARCH_TOOL, "name": "web_search", "max_uses": uses}));
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(model));
    body.insert("max_tokens".into(), json!(request.max_output_tokens));
    body.insert("stream".into(), json!(true));
    if !system.is_empty() {
        body.insert("system".into(), Value::Array(system));
    }
    body.insert("messages".into(), Value::Array(messages));
    if !tools.is_empty() {
        let mut choice = match tool_choice(request)? {
            WorkModelToolChoice::Auto => json!({"type": "auto"}),
            WorkModelToolChoice::Required => json!({"type": "any"}),
            WorkModelToolChoice::Named(name) => json!({"type": "tool", "name": name}),
        };
        choice["disable_parallel_tool_use"] = json!(!request.parallel_tools);
        body.insert("tools".into(), Value::Array(tools));
        body.insert("tool_choice".into(), choice);
    }
    let forced = !request.tools.is_empty() && request.tool_choice != WorkModelToolChoice::Auto;
    if let Some(reasoning) = request.reasoning.filter(|_| !forced) {
        if budget_thinking(model) {
            let budget: u32 = match reasoning {
                WorkModelReasoning::Low => 2_048,
                WorkModelReasoning::Medium => 6_000,
                WorkModelReasoning::High | WorkModelReasoning::XHigh | WorkModelReasoning::Max => {
                    16_000
                }
            };
            if request.max_output_tokens > 2_048 {
                let budget = budget.min(request.max_output_tokens - 1_024).max(1_024);
                body.insert(
                    "thinking".into(),
                    json!({"type": "enabled", "budget_tokens": budget}),
                );
            }
        } else {
            let effort = match reasoning {
                WorkModelReasoning::Low => "low",
                WorkModelReasoning::Medium => "medium",
                WorkModelReasoning::High => "high",
                WorkModelReasoning::XHigh => "xhigh",
                WorkModelReasoning::Max => "max",
            };
            body.insert("thinking".into(), json!({"type": "adaptive"}));
            body.insert("output_config".into(), json!({"effort": effort}));
        }
    }
    Ok(Value::Object(body))
}

/// Consecutive same-role turns merge, as the API would.
fn push(messages: &mut Vec<Value>, role: &str, content: Vec<Value>) {
    if content.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut() {
        if last["role"] == role {
            if let Some(existing) = last["content"].as_array_mut() {
                existing.extend(content);
                return;
            }
        }
    }
    messages.push(json!({"role": role, "content": content}));
}

enum Block {
    Text {
        text: String,
        citations: Vec<(String, String, String)>,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        json: String,
        server: bool,
    },
    Raw(Value),
}

#[derive(Default)]
pub(super) struct Decoder {
    blocks: Vec<Option<Block>>,
    assistant: Vec<WorkModelPart>,
    searches: Searches,
    usage: WorkModelUsage,
    cache_write: u64,
    stop: Option<DecodedStop>,
    tool_calls: usize,
    billed_searches: Option<u32>,
}

impl Decoder {
    pub(super) fn event(
        &mut self,
        frame: &SseEvent,
        out: &mut Vec<WorkModelEvent>,
    ) -> Result<Flow, WorkModelError> {
        let data = json(&frame.data)?;
        let kind = data
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(frame.event.as_str());
        match kind {
            "message_start" => {
                if let Some(usage) = data.pointer("/message/usage") {
                    self.read_usage(usage);
                }
            }
            "content_block_start" => {
                let index = u64_at(&data, &["index"]) as usize;
                let block = data.get("content_block").ok_or(WorkModelError::Protocol)?;
                if index > 4_096 {
                    return Err(WorkModelError::Protocol);
                }
                if self.blocks.len() <= index {
                    self.blocks.resize_with(index + 1, || None);
                }
                self.blocks[index] = Some(match block["type"].as_str().unwrap_or_default() {
                    "text" => Block::Text {
                        text: String::new(),
                        citations: Vec::new(),
                    },
                    "thinking" => Block::Thinking {
                        thinking: block["thinking"].as_str().unwrap_or_default().to_owned(),
                        signature: block["signature"].as_str().unwrap_or_default().to_owned(),
                    },
                    kind @ ("tool_use" | "server_tool_use") => Block::ToolUse {
                        id: block["id"]
                            .as_str()
                            .ok_or(WorkModelError::Protocol)?
                            .to_owned(),
                        name: block["name"]
                            .as_str()
                            .ok_or(WorkModelError::Protocol)?
                            .to_owned(),
                        json: String::new(),
                        server: kind == "server_tool_use",
                    },
                    _ => Block::Raw(block.clone()),
                });
            }
            "content_block_delta" => {
                let index = u64_at(&data, &["index"]) as usize;
                let delta = data.get("delta").ok_or(WorkModelError::Protocol)?;
                let block = self
                    .blocks
                    .get_mut(index)
                    .and_then(Option::as_mut)
                    .ok_or(WorkModelError::Protocol)?;
                match (block, delta["type"].as_str().unwrap_or_default()) {
                    (Block::Text { text, .. }, "text_delta") => {
                        let piece = delta["text"].as_str().unwrap_or_default();
                        push_text(text, piece)?;
                        if !piece.is_empty() {
                            out.push(WorkModelEvent::Text(piece.to_owned()));
                        }
                    }
                    (Block::Text { citations, .. }, "citations_delta") => {
                        let citation = &delta["citation"];
                        if let Some(url) = citation["url"].as_str() {
                            citations.push((
                                url.to_owned(),
                                citation["title"].as_str().unwrap_or_default().to_owned(),
                                citation["cited_text"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned(),
                            ));
                        }
                    }
                    (Block::Thinking { thinking, .. }, "thinking_delta") => {
                        push_text(thinking, delta["thinking"].as_str().unwrap_or_default())?;
                    }
                    (Block::Thinking { signature, .. }, "signature_delta") => {
                        push_text(signature, delta["signature"].as_str().unwrap_or_default())?;
                    }
                    (Block::ToolUse { json, .. }, "input_json_delta") => {
                        let piece = delta["partial_json"].as_str().unwrap_or_default();
                        if json.len() + piece.len() > MAX_TOOL_ARGUMENT_BYTES {
                            return Err(WorkModelError::Protocol);
                        }
                        json.push_str(piece);
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = u64_at(&data, &["index"]) as usize;
                let block = self
                    .blocks
                    .get_mut(index)
                    .and_then(Option::take)
                    .ok_or(WorkModelError::Protocol)?;
                self.close(block, out)?;
            }
            "message_delta" => {
                if let Some(usage) = data.get("usage") {
                    self.read_usage(usage);
                }
                if let Some(reason) = data.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop = Some(match reason {
                        "tool_use" => DecodedStop::End(WorkModelStop::ToolUse),
                        "max_tokens" | "model_context_window_exceeded" => {
                            DecodedStop::End(WorkModelStop::MaxTokens)
                        }
                        "refusal" => DecodedStop::End(WorkModelStop::Refused),
                        "pause_turn" => DecodedStop::Paused,
                        _ => DecodedStop::End(WorkModelStop::EndTurn),
                    });
                }
            }
            "message_stop" => return Ok(Flow::Done),
            "error" => {
                let code = data
                    .pointer("/error/type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Err(stream_error(code));
            }
            _ => {}
        }
        Ok(Flow::Continue)
    }

    fn close(&mut self, block: Block, out: &mut Vec<WorkModelEvent>) -> Result<(), WorkModelError> {
        match block {
            Block::Text { text, citations } => {
                for (url, title, passage) in &citations {
                    self.searches.cite(url, title, passage);
                }
                self.searches.flush(out);
                if !text.is_empty() {
                    self.assistant.push(WorkModelPart::Text(text));
                }
            }
            Block::Thinking {
                thinking,
                signature,
            } => self.assistant.push(replay(
                WIRE,
                json!({"type": "thinking", "thinking": thinking, "signature": signature}),
            )),
            Block::ToolUse {
                id,
                name,
                json,
                server: false,
            } => {
                if self.tool_calls >= MAX_TOOL_CALLS {
                    return Err(WorkModelError::Protocol);
                }
                self.tool_calls += 1;
                let call = WorkModelToolCall {
                    id,
                    name,
                    arguments: arguments(&json),
                };
                out.push(WorkModelEvent::ToolCall(call.clone()));
                self.assistant.push(WorkModelPart::ToolCall(call));
            }
            Block::ToolUse {
                id,
                name,
                json,
                server: true,
            } => {
                let input = arguments(&json);
                if name == "web_search" {
                    let query = input
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.searches.begin(Some(id.clone()), query);
                }
                self.assistant.push(replay(
                    WIRE,
                    json!({"type": "server_tool_use", "id": id, "name": name, "input": input}),
                ));
            }
            Block::Raw(block) => {
                if block["type"] == "web_search_tool_result" {
                    let key = block["tool_use_id"].as_str();
                    for hit in block["content"].as_array().into_iter().flatten() {
                        if let Some(url) = hit["url"].as_str() {
                            self.searches.hit(
                                key,
                                url,
                                hit["title"].as_str().unwrap_or_default(),
                                "",
                            );
                        }
                    }
                }
                self.assistant.push(replay(WIRE, block));
            }
        }
        Ok(())
    }

    fn read_usage(&mut self, usage: &Value) {
        let fresh = usage.get("input_tokens").and_then(Value::as_u64);
        let read = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
        let written = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64);
        if fresh.is_some() || read.is_some() || written.is_some() {
            let read = read.unwrap_or(self.usage.cached_input_tokens);
            let written = written.unwrap_or(self.cache_write);
            let fresh = fresh.unwrap_or_else(|| {
                self.usage.input_tokens.saturating_sub(
                    self.usage
                        .cached_input_tokens
                        .saturating_add(self.cache_write),
                )
            });
            self.usage.cached_input_tokens = read;
            self.cache_write = written;
            self.usage.input_tokens = fresh.saturating_add(read).saturating_add(written);
        }
        if let Some(output) = usage.get("output_tokens").and_then(Value::as_u64) {
            self.usage.output_tokens = output;
        }
        if let Some(searches) = usage
            .pointer("/server_tool_use/web_search_requests")
            .and_then(Value::as_u64)
        {
            self.billed_searches = Some(u32::try_from(searches).unwrap_or(u32::MAX));
        }
    }

    pub(super) fn finish(
        mut self,
        out: &mut Vec<WorkModelEvent>,
    ) -> Result<Decoded, WorkModelError> {
        let stop = self.stop.ok_or(WorkModelError::Protocol)?;
        self.searches.flush(out);
        let searches = self.billed_searches.unwrap_or(self.searches.count);
        Ok(Decoded {
            stop,
            usage: self.usage,
            cache_write_tokens: self.cache_write,
            searches,
            assistant: self.assistant,
            ..Decoded::default()
        })
    }
}
