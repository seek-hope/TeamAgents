//! OpenAI Responses API adapter (plan §7, R17). Stateless continuation
//! (`store: false`) replays the original `responses_output` items so ordering
//! and opaque reasoning survive; the assembled chat-shaped message keeps them
//! for the next request. Protocol type stays separate from vendor name: any
//! endpoint speaking this wire shape is served by this one adapter.

use super::{
    clamp_max_tokens, normalize_tool_call_ids, pump_sse, stream_failure_msg, AttemptOutcome, Cancel, ErrorClass,
    Provider, ProviderError, ProviderEvent, SseEnd,
};
use serde_json::{json, Value as Json};
use std::ops::ControlFlow;
use std::time::{Duration, Instant};
use teamagents_core::kernel::{ModelRequest, ModelResponse, Usage};

/// Cap on a non-SSE (error or non-streaming) response body.
const MAX_RESPONSE: usize = 64 * 1024 * 1024;

pub struct Responses {
    client: reqwest::Client,
    base: String,
    api_key: String,
    timeout: Duration,
    context_window: Option<u64>,
}

impl Responses {
    /// `base` is the API root without a trailing slash (e.g.
    /// https://api.openai.com/v1). Credentials arrive already resolved —
    /// the config layer reads env/local credentials, this adapter never
    /// touches the environment itself.
    pub fn new(base: impl Into<String>, api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("responses client: {e}"))?;
        Ok(Responses { client, base: base.into(), api_key: api_key.into(), timeout, context_window: None })
    }

    /// Attach the catalog-declared context window: max_output_tokens clamps
    /// to the remaining budget at the wire boundary (pi-ai simple-options).
    pub fn with_context_window(mut self, window: Option<u64>) -> Self {
        self.context_window = window;
        self
    }

    pub fn openai(api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        Self::new("https://api.openai.com/v1", api_key, timeout)
    }

    fn body(&self, request: &ModelRequest) -> Json {
        // Cross-protocol continuation safety (pi-ai transform-messages).
        let (instructions, input) = to_responses_input(&normalize_tool_call_ids(&request.messages));
        let tools: Vec<Json> = request
            .tools
            .iter()
            .filter_map(|tool| {
                let f = tool.get("function")?;
                Some(json!({
                    "type": "function",
                    "name": f.get("name")?,
                    "description": f.get("description").cloned().unwrap_or(Json::Null),
                    "parameters": f.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
                }))
            })
            .collect();
        let mut body = json!({
            "model": request.model,
            "input": input,
            "stream": true,
            "store": false,
        });
        if !instructions.is_empty() {
            body["instructions"] = json!(instructions);
        }
        if !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        if let (Some(target), Some(options)) = (body.as_object_mut(), request.options.as_object()) {
            for (key, value) in options {
                target.insert(key.clone(), value.clone());
            }
        }
        // Wire-name mapping (ported contract): Responses names the same knobs
        // differently; values arrive already resolved by the config layer.
        if let Some(map) = body.as_object_mut() {
            for (from, to) in [("max_tokens", "max_output_tokens"), ("max_completion_tokens", "max_output_tokens")] {
                if let Some(value) = map.remove(from) {
                    map.insert(to.into(), value);
                }
            }
            if let Some(effort) = map.remove("reasoning_effort") {
                map.insert("reasoning".into(), json!({"effort": effort}));
            }
            // Clamp to what the context window still has left (pi-ai
            // simple-options); absent means the API default applies.
            if let Some(value) = map.get("max_output_tokens").and_then(|v| v.as_u64()) {
                let clamped = clamp_max_tokens(value, request.est_prompt_tokens, self.context_window);
                map.insert("max_output_tokens".into(), json!(clamped));
            }
        }
        body
    }
}

impl Provider for Responses {
    fn protocol(&self) -> &str {
        "responses"
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
            .post(format!("{}/responses", self.base))
            .header("content-type", "application/json")
            .timeout(self.timeout);
        if !self.api_key.is_empty() {
            http = http.bearer_auth(&self.api_key);
        }
        let response = tokio::select! {
            send = http.body(self.body(request).to_string()).send() => {
                send.map_err(|e| {
                    if e.is_timeout() || e.is_connect() || e.is_request() {
                        ProviderError::transient(format!("responses API: {e}"))
                    } else {
                        ProviderError::permanent(format!("responses API: {e}"))
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
        let items: Vec<Json>;
        let usage: Json;
        if is_sse {
            let mut text = String::new();
            let mut streamed: Vec<Json> = vec![];
            let mut stream_usage = json!({});
            let mut complete = false;
            let mut emitted = false;
            let mut on_frame = |data: &str| -> Result<ControlFlow<()>, ProviderError> {
                let Ok(data) = serde_json::from_str::<Json>(data) else { return Ok(ControlFlow::Continue(())) };
                if data.get("error").is_some() || data["type"] == "error" {
                    return Err(ProviderError::permanent(format!("model stream error: {}", data["error"])));
                }
                match data["type"].as_str().unwrap_or("") {
                    "response.output_text.delta" => {
                        if let Some(part) = data["delta"].as_str() {
                            text.push_str(part);
                            emitted = true;
                            on_event(ProviderEvent::TextDelta(part.to_string()));
                        }
                    }
                    // Reasoning stays private: it is not part of the reply.
                    "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {}
                    "response.output_item.done" if !data["item"].is_null() => streamed.push(data["item"].clone()),
                    "response.incomplete" => {
                        return Err(ProviderError::permanent(format!(
                            "model response incomplete ({}); tool calls in this response were not executed",
                            data["response"]["incomplete_details"]
                        )));
                    }
                    "response.completed" => {
                        ensure_complete(&data["response"])?;
                        if let Some(output) = data["response"]["output"].as_array() {
                            if !output.is_empty() {
                                streamed = output.clone();
                            }
                        }
                        if data["response"]["usage"].is_object() {
                            stream_usage = data["response"]["usage"].clone();
                        }
                        complete = true;
                        return Ok(ControlFlow::Break(()));
                    }
                    "response.failed" => {
                        return Err(ProviderError::permanent(format!(
                            "model stream error: {}",
                            data["response"]["error"]
                        )));
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
            if streamed.is_empty() && !text.is_empty() {
                // Some servers only stream text deltas without item events.
                streamed.push(json!({"type": "message", "role": "assistant",
                                     "content": [{"type": "output_text", "text": text}]}));
            }
            let data = json!({"output": streamed, "usage": stream_usage});
            ensure_complete(&data)?;
            items = streamed;
            usage = stream_usage;
        } else {
            let bytes = tokio::select! {
                body = response.bytes() => body.map_err(|e| ProviderError::transient(format!("model stream: {e}")))?,
                _ = cancel.cancelled() => return Err(ProviderError::interrupted("turn interrupted")),
            };
            if bytes.len() > MAX_RESPONSE {
                return Err(ProviderError::permanent("responses API: response too large"));
            }
            let data: Json = serde_json::from_slice(&bytes)
                .map_err(|e| ProviderError::permanent(format!("responses API: invalid response: {e}")))?;
            if let Some(error) = data.get("error") {
                let text = error["message"].as_str().unwrap_or("unknown error");
                let class =
                    if super::context_overflow(text) { ErrorClass::ContextOverflow } else { ErrorClass::Permanent };
                return Err(ProviderError {
                    class,
                    message: format!("responses API: {text}"),
                    retry_after: None,
                    status: None,
                });
            }
            ensure_complete(&data)?;
            items = data["output"].as_array().cloned().unwrap_or_default();
            usage = data["usage"].clone();
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::interrupted("turn interrupted"));
        }
        let data = json!({"output": items, "usage": usage});
        let message = from_responses_output(&data);
        if message["content"].is_null() && message["tool_calls"].is_null() {
            return Err(ProviderError::permanent("responses API: empty output"));
        }
        Ok(AttemptOutcome {
            response: ModelResponse {
                message,
                usage: Usage::from_json(&json!({"usage": usage})),
                native: json!({"protocol": "responses", "base": self.base}),
            },
            raw: data,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}

/// Chat-completions history → Responses `instructions` + `input` items.
/// ponytail: image tool results (view_image) travel as `input_image` content
/// in the legacy chat.rs path; add an ImageLoader here once the v2 tool layer
/// can produce image references.
fn to_responses_input(history: &[Json]) -> (String, Vec<Json>) {
    let mut instructions = String::new();
    let mut out: Vec<Json> = vec![];
    for message in history {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" => {
                if !instructions.is_empty() {
                    instructions.push_str("\n\n");
                }
                instructions.push_str(content);
            }
            "user" if message.get("tool_call_id").is_none() => {
                out.push(json!({"role": "user", "content": [{"type": "input_text", "text": content}]}));
            }
            "assistant" => {
                if let Some(items) = message.get("responses_output").and_then(Json::as_array) {
                    // Stateless Responses continuation requires the original
                    // ordering and opaque reasoning, including tool item IDs.
                    out.extend(items.iter().cloned());
                    continue;
                }
                if !content.is_empty() {
                    out.push(json!({"role": "assistant", "content": [{"type": "output_text", "text": content}]}));
                }
                for call in message.get("tool_calls").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                    out.push(json!({
                        "type": "function_call",
                        "call_id": call.get("id").cloned().unwrap_or(Json::Null),
                        "name": call.get("function").and_then(|f| f.get("name")).cloned().unwrap_or(Json::Null),
                        "arguments": call.get("function").and_then(|f| f.get("arguments")).cloned().unwrap_or(json!("{}")),
                    }));
                }
            }
            "tool" => {
                out.push(json!({
                    "type": "function_call_output",
                    "call_id": message.get("tool_call_id").cloned().unwrap_or(Json::Null),
                    "output": content,
                }));
            }
            _ => {}
        }
    }
    (instructions, out)
}

/// Responses `output` items → the chat-shaped assistant message the kernel
/// expects; the original items stay on `responses_output` for continuation.
fn from_responses_output(data: &Json) -> Json {
    let mut text = String::new();
    let mut calls: Vec<Json> = vec![];
    for item in data.get("output").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        match item.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "message" => {
                for part in item.get("content").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                    if let Some(part) = part.get("text").and_then(|v| v.as_str()) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(part);
                    }
                }
            }
            "function_call" => calls.push(json!({
                "id": item.get("call_id").cloned().unwrap_or(Json::Null),
                "type": "function",
                "function": {
                    "name": item.get("name").cloned().unwrap_or(Json::Null),
                    "arguments": item.get("arguments").cloned().unwrap_or(json!("{}")),
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
    if let Some(output) = data.get("output").filter(|output| output.is_array()) {
        message["responses_output"] = output.clone();
    }
    message
}

/// A complete transport response is not necessarily a usable one (ported
/// contract): incomplete status or unfinished output items never produce
/// tool intents — they fail the attempt instead.
fn ensure_complete(data: &Json) -> Result<(), ProviderError> {
    if let Some(status) = data["status"].as_str().filter(|status| *status != "completed") {
        return Err(ProviderError::permanent(format!(
            "model response incomplete ({status}, {}); tool calls in this response were not executed",
            data.get("incomplete_details").unwrap_or(&Json::Null)
        )));
    }
    if data["output"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| item["status"].as_str().is_some_and(|status| status != "completed"))
    {
        return Err(ProviderError::permanent(
            "model response contains incomplete output items; tool calls in this response were not executed",
        ));
    }
    Ok(())
}
