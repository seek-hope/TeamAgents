//! Anthropic Messages API adapter (plan §7, R17). Signed thinking blocks
//! stay in `anthropic_blocks` so continuation replays them verbatim; the
//! assembled chat-shaped message carries them for the next request. Protocol
//! type stays separate from vendor name: any endpoint speaking this wire
//! shape is served by this one adapter.

use super::{
    append, clamp_max_tokens, normalize_tool_call_ids, pump_sse, stream_failure_msg, AttemptOutcome, Cancel,
    ErrorClass, Provider, ProviderError, ProviderEvent, SseEnd,
};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};
use teamagents_core::kernel::{ModelRequest, ModelResponse, Usage};

/// Cap on a non-SSE (error or non-streaming) response body.
const MAX_RESPONSE: usize = 64 * 1024 * 1024;

pub struct Anthropic {
    client: reqwest::Client,
    base: String,
    api_key: String,
    timeout: Duration,
    context_window: Option<u64>,
}

impl Anthropic {
    /// `base` is the API root without a trailing slash or `/v1` suffix (the
    /// Messages endpoint lives at `{base}/v1/messages`). Credentials arrive
    /// already resolved — the config layer reads env/local credentials, this
    /// adapter never touches the environment itself.
    pub fn new(base: impl Into<String>, api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("anthropic client: {e}"))?;
        let base = base.into().trim_end_matches('/').trim_end_matches("/v1").to_string();
        Ok(Anthropic { client, base, api_key: api_key.into(), timeout, context_window: None })
    }

    /// Attach the catalog-declared context window: max_tokens clamps to the
    /// remaining budget at the wire boundary (pi-ai simple-options).
    pub fn with_context_window(mut self, window: Option<u64>) -> Self {
        self.context_window = window;
        self
    }

    pub fn official(api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        Self::new("https://api.anthropic.com", api_key, timeout)
    }

    fn body(&self, request: &ModelRequest) -> Json {
        // Cross-protocol continuation safety (pi-ai transform-messages): ids
        // minted under another protocol may violate this API's id rules.
        let (system, messages) = to_anthropic_messages(&normalize_tool_call_ids(&request.messages));
        let tools: Vec<Json> = request
            .tools
            .iter()
            .filter_map(|tool| {
                let f = tool.get("function")?;
                Some(json!({
                    "name": f.get("name")?,
                    "description": f.get("description").cloned().unwrap_or(Json::Null),
                    "input_schema": f.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
                }))
            })
            .collect();
        // max_tokens is required by this API (ported default), clamped to
        // what the context window still has left (pi-ai simple-options).
        let configured_max = request.options.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(8192);
        let max_tokens = clamp_max_tokens(configured_max, request.est_prompt_tokens, self.context_window);
        let mut body = json!({
            "model": request.model,
            "max_tokens": max_tokens,
            "messages": messages,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        // Wire-name mapping (ported contract): values arrive already resolved
        // by the config layer; effort moves under output_config.
        if let Some(options) = request.options.as_object() {
            for (key, value) in options {
                if key != "max_tokens" && key != "reasoning_effort" {
                    body[key] = value.clone();
                }
            }
        }
        if let Some(effort) = request.options.get("reasoning_effort") {
            body["output_config"] = json!({"effort": effort});
        }
        body["stream"] = json!(true);
        body
    }
}

impl Provider for Anthropic {
    fn protocol(&self) -> &str {
        "anthropic"
    }

    async fn complete(
        &self,
        request: &ModelRequest,
        cancel: &Cancel,
        on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::interrupted("turn interrupted"));
        }
        let started = Instant::now();
        let mut http = self
            .client
            .post(format!("{}/v1/messages", self.base))
            .header("content-type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .timeout(self.timeout);
        if !self.api_key.is_empty() {
            http = http.header("x-api-key", &self.api_key);
        }
        let response = tokio::select! {
            send = http.body(self.body(request).to_string()).send() => {
                send.map_err(|e| {
                    if e.is_timeout() || e.is_connect() || e.is_request() {
                        ProviderError::transient(format!("anthropic API: {e}"))
                    } else {
                        ProviderError::permanent(format!("anthropic API: {e}"))
                    }
                })?
            }
            _ = cancel.cancelled() => return Err(ProviderError::interrupted("turn interrupted")),
        };
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::status(status.as_u16(), &body, retry_after));
        }
        let is_sse = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("text/event-stream"))
            .unwrap_or(false);
        let data: Json;
        if is_sse {
            let mut blocks: BTreeMap<usize, Json> = BTreeMap::new();
            let mut arguments: BTreeMap<usize, String> = BTreeMap::new();
            let mut usage = json!({});
            let mut stop_reason = Json::Null;
            let mut complete = false;
            let mut emitted = false;
            let mut on_frame = |data: &str| -> Result<ControlFlow<()>, ProviderError> {
                let Ok(data) = serde_json::from_str::<Json>(data) else { return Ok(ControlFlow::Continue(())) };
                if data.get("error").is_some() || data["type"] == "error" {
                    return Err(ProviderError::permanent(format!("model stream error: {}", data["error"])));
                }
                let index = data["index"].as_u64().unwrap_or(0) as usize;
                match data["type"].as_str().unwrap_or("") {
                    "message_start" => usage = data["message"]["usage"].clone(),
                    "content_block_start" => {
                        blocks.insert(index, data["content_block"].clone());
                    }
                    "content_block_delta" => {
                        let block = blocks
                            .get_mut(&index)
                            .ok_or_else(|| ProviderError::permanent("model stream: delta without content block"))?;
                        let delta = &data["delta"];
                        match delta["type"].as_str().unwrap_or("") {
                            "text_delta" => {
                                append(block, "text", &delta["text"]);
                                if let Some(text) = delta["text"].as_str() {
                                    emitted = true;
                                    on_event(ProviderEvent::TextDelta(text.to_string()));
                                }
                            }
                            "input_json_delta" => arguments
                                .entry(index)
                                .or_default()
                                .push_str(delta["partial_json"].as_str().unwrap_or("")),
                            // Thinking + its signature stay private and verbatim.
                            "thinking_delta" => append(block, "thinking", &delta["thinking"]),
                            "signature_delta" => append(block, "signature", &delta["signature"]),
                            _ => {}
                        }
                    }
                    "message_delta" => {
                        if !data["delta"]["stop_reason"].is_null() {
                            stop_reason = data["delta"]["stop_reason"].clone();
                        }
                        if !usage.is_object() {
                            usage = json!({});
                        }
                        if let Some(fields) = data["usage"].as_object() {
                            usage.as_object_mut().unwrap().extend(fields.clone());
                        }
                    }
                    "message_stop" => {
                        complete = true;
                        return Ok(ControlFlow::Break(()));
                    }
                    _ => {}
                }
                Ok(ControlFlow::Continue(()))
            };
            match pump_sse(response, cancel, &mut on_frame).await? {
                SseEnd::Closed => {}
                SseEnd::Cancelled => return Err(ProviderError::interrupted("turn interrupted")),
                SseEnd::Transport(message) => return Err(stream_failure_msg(&message, emitted)),
            }
            if !complete {
                // Clean EOF without a terminator means the connection gave up
                // mid-turn; retryable only before any visible output.
                return Err(stream_failure_msg(
                    "model stream ended before completion; partial tool calls were not executed",
                    emitted,
                ));
            }
            for (index, args) in arguments {
                let block = blocks
                    .get_mut(&index)
                    .ok_or_else(|| ProviderError::permanent("model stream: tool block missing"))?;
                block["input"] = serde_json::from_str(&args).map_err(|e| {
                    ProviderError::permanent(format!("model stream: invalid streamed tool arguments: {e}"))
                })?;
            }
            data = json!({
                "content": blocks.into_values().collect::<Vec<_>>(),
                "usage": usage,
                "stop_reason": stop_reason,
            });
            ensure_complete(&data)?;
        } else {
            let bytes = tokio::select! {
                body = response.bytes() => body.map_err(|e| ProviderError::transient(format!("model stream: {e}")))?,
                _ = cancel.cancelled() => return Err(ProviderError::interrupted("turn interrupted")),
            };
            if bytes.len() > MAX_RESPONSE {
                return Err(ProviderError::permanent("anthropic API: response too large"));
            }
            data = serde_json::from_slice(&bytes)
                .map_err(|e| ProviderError::permanent(format!("anthropic API: invalid response: {e}")))?;
            if let Some(error) = data.get("error") {
                let text = error["message"].as_str().unwrap_or("unknown error");
                let class =
                    if super::context_overflow(text) { ErrorClass::ContextOverflow } else { ErrorClass::Permanent };
                return Err(ProviderError {
                    class,
                    message: format!("anthropic API: {text}"),
                    retry_after: None,
                    status: None,
                });
            }
            ensure_complete(&data)?;
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::interrupted("turn interrupted"));
        }
        let message = from_anthropic_message(&data);
        if message["content"].is_null() && message["tool_calls"].is_null() {
            return Err(ProviderError::permanent("anthropic API: empty content"));
        }
        Ok(AttemptOutcome {
            response: ModelResponse {
                message,
                usage: Usage::from_json(&json!({"usage": data["usage"]})),
                native: json!({"protocol": "anthropic", "base": self.base}),
            },
            raw: data,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// Chat-completions history → Anthropic `system` + `messages`.
/// ponytail: v2 has no image flow yet (view_image stays on the legacy
/// chat.rs path). When it lands, port pi-ai transform-messages: catalog
/// entries declare input modalities, and at this boundary a non-vision
/// model gets every image block replaced by one placeholder text
/// ("(image omitted: model does not support images)"; consecutive image
/// blocks collapse into a single placeholder) instead of erroring out.
fn to_anthropic_messages(history: &[Json]) -> (String, Vec<Json>) {
    let mut system = String::new();
    let mut out: Vec<Json> = vec![];
    for message in history {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" => {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(content);
            }
            "user" if message.get("tool_call_id").is_none() => {
                out.push(json!({"role": "user", "content": [{"type": "text", "text": content}]}));
            }
            "assistant" => {
                if let Some(blocks) = message["anthropic_blocks"].as_array() {
                    // Signed thinking and redacted blocks must replay verbatim.
                    out.push(json!({"role": "assistant", "content": blocks}));
                    continue;
                }
                let calls = message.get("tool_calls").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let mut blocks: Vec<Json> = vec![];
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                for call in calls {
                    let name = call.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                    let arguments =
                        call.get("function").and_then(|f| f.get("arguments")).and_then(|v| v.as_str()).unwrap_or("{}");
                    let input: Json = serde_json::from_str(arguments).unwrap_or(json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.get("id").cloned().unwrap_or(Json::Null),
                        "name": name,
                        "input": input,
                    }));
                }
                if !blocks.is_empty() {
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            "tool" => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": message.get("tool_call_id").cloned().unwrap_or(Json::Null),
                    "content": content,
                });
                // Consecutive tool results must share one user message (API rule).
                match out.last_mut() {
                    Some(last)
                        if last.get("role").and_then(|v| v.as_str()) == Some("user")
                            && last
                                .get("content")
                                .and_then(|c| c.as_array())
                                .map(|blocks| {
                                    blocks.iter().all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                                })
                                .unwrap_or(false) =>
                    {
                        last["content"].as_array_mut().unwrap().push(block);
                    }
                    _ => out.push(json!({"role": "user", "content": [block]})),
                }
            }
            _ => {}
        }
    }
    (system, out)
}

/// Anthropic content blocks → the chat-shaped assistant message the kernel
/// expects; the original blocks stay on `anthropic_blocks` for continuation.
fn from_anthropic_message(data: &Json) -> Json {
    let mut text = String::new();
    let mut calls: Vec<Json> = vec![];
    for block in data.get("content").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        match block.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "text" => {
                if let Some(part) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(part);
                }
            }
            "tool_use" => calls.push(json!({
                "id": block.get("id").cloned().unwrap_or(Json::Null),
                "type": "function",
                "function": {
                    "name": block.get("name").cloned().unwrap_or(Json::Null),
                    "arguments": block.get("input").cloned().unwrap_or(json!({})).to_string(),
                },
            })),
            _ => {}
        }
    }
    let mut message = json!({
        "role": "assistant",
        "content": if text.is_empty() { Json::Null } else { Json::String(text) },
    });
    if !calls.is_empty() {
        message["tool_calls"] = json!(calls);
    }
    message["anthropic_blocks"] = data["content"].clone();
    message
}

/// A complete transport response is not necessarily a usable one (ported
/// contract): truncated or paused turns never produce tool intents — they
/// fail the attempt instead.
fn ensure_complete(data: &Json) -> Result<(), ProviderError> {
    if let Some(reason) = data["stop_reason"]
        .as_str()
        .filter(|reason| matches!(*reason, "max_tokens" | "model_context_window_exceeded" | "pause_turn"))
    {
        return Err(ProviderError::permanent(format!(
            "model response incomplete ({reason}); tool calls in this response were not executed"
        )));
    }
    Ok(())
}
