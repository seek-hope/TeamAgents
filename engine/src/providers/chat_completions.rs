//! OpenAI-compatible Chat Completions adapter, covering DeepSeek (plan §7,
//! RV-27). DeepSeek native fields such as `reasoning_content` are preserved
//! in the assembled message; usage comes from the terminal stream frame.

use super::{
    append, clamp_max_tokens, normalize_tool_call_ids, pump_sse, stream_failure_msg, AttemptOutcome, Cancel,
    ErrorClass, Provider, ProviderError, ProviderEvent, SseEnd,
};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};
use teamagents_core::kernel::{ModelRequest, ModelResponse};

/// Cap on a non-SSE (error or non-streaming) response body.
const MAX_RESPONSE: usize = 64 * 1024 * 1024;

pub struct ChatCompletions {
    client: reqwest::Client,
    base: String,
    api_key: String,
    timeout: Duration,
    context_window: Option<u64>,
    /// Test knob: overrides the derived stream-stall bound.
    stall: Option<Duration>,
}

impl ChatCompletions {
    /// `base` is the API root without a trailing slash (e.g.
    /// https://api.deepseek.com/v1). Credentials arrive already resolved —
    /// the config layer reads env/local credentials, this adapter never
    /// touches the environment itself.
    pub fn new(base: impl Into<String>, api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        let client = super::http_client()?;
        Ok(ChatCompletions {
            client,
            base: base.into(),
            api_key: api_key.into(),
            timeout,
            context_window: None,
            stall: None,
        })
    }

    /// Attach the catalog-declared context window: max_tokens clamps to the
    /// remaining budget at the wire boundary (pi-ai simple-options).
    pub fn with_context_window(mut self, window: Option<u64>) -> Self {
        self.context_window = window;
        self
    }

    /// Override the stream-stall bound (a stream with no event for this long
    /// fails as a transport error). Tests use it to keep the bound short.
    pub fn with_stream_stall(mut self, stall: Duration) -> Self {
        self.stall = Some(stall);
        self
    }

    fn stall_bound(&self) -> Duration {
        self.stall.unwrap_or_else(|| super::stream_stall_bound(self.timeout))
    }

    pub fn deepseek(api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        Self::new("https://api.deepseek.com/v1", api_key, timeout)
    }

    fn body(&self, request: &ModelRequest) -> Json {
        // Native continuation blocks belong to their own protocol: switching
        // protocols must not forward them on this wire (§7, ported contract).
        // Cross-protocol tool-call ids normalize the same way (pi-ai
        // transform-messages) — harmless pass-through for already-valid ids.
        let messages: Vec<Json> = normalize_tool_call_ids(&request.messages)
            .iter()
            .map(|message| {
                let mut message = message.clone();
                if let Some(fields) = message.as_object_mut() {
                    fields.remove("responses_output");
                    fields.remove("anthropic_blocks");
                }
                message
            })
            .collect();
        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "tools": request.tools,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if let (Some(target), Some(options)) = (body.as_object_mut(), request.options.as_object()) {
            for (key, value) in options {
                target.insert(key.clone(), value.clone());
            }
            // Clamp to what the context window still has left (pi-ai
            // simple-options); absent means the API default applies.
            for key in ["max_tokens", "max_completion_tokens"] {
                if let Some(value) = target.get(key).and_then(|v| v.as_u64()) {
                    let clamped = clamp_max_tokens(value, request.est_prompt_tokens, self.context_window);
                    target.insert(key.into(), json!(clamped));
                }
            }
        }
        body
    }
}

impl Provider for ChatCompletions {
    fn protocol(&self) -> &str {
        "chat_completions"
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
            .post(format!("{}/chat/completions", self.base))
            .header("content-type", "application/json")
            .timeout(self.timeout);
        if !self.api_key.is_empty() {
            http = http.bearer_auth(&self.api_key);
        }
        let response = tokio::select! {
            send = http.body(self.body(request).to_string()).send() => {
                send.map_err(|e| {
                    if e.is_timeout() || e.is_connect() || e.is_request() {
                        ProviderError::transient(format!("chat API: {e}"))
                    } else {
                        ProviderError::permanent(format!("chat API: {e}"))
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
        let mut message = json!({"role": "assistant"});
        let mut blocks: BTreeMap<usize, Json> = BTreeMap::new();
        let mut finish_reason = Json::Null;
        let mut usage = json!({});
        let mut emitted = false;
        let mut complete = false;
        if is_sse {
            let mut on_frame = |data: &str| -> Result<ControlFlow<()>, ProviderError> {
                if data == "[DONE]" {
                    complete = true;
                    return Ok(ControlFlow::Break(()));
                }
                let Ok(data) = serde_json::from_str::<Json>(data) else { return Ok(ControlFlow::Continue(())) };
                if data["usage"].is_object() {
                    usage = data["usage"].clone();
                }
                let choice = data["choices"]
                    .as_array()
                    .and_then(|choices| choices.iter().find(|c| c["index"].as_u64().unwrap_or(0) == 0));
                let Some(choice) = choice else { return Ok(ControlFlow::Continue(())) };
                if !choice["finish_reason"].is_null() {
                    finish_reason = choice["finish_reason"].clone();
                }
                let delta = &choice["delta"];
                append(&mut message, "content", &delta["content"]);
                // DeepSeek native reasoning stays in the message (§7).
                append(&mut message, "reasoning_content", &delta["reasoning_content"]);
                if let Some(text) = delta["content"].as_str() {
                    emitted = true;
                    on_event(ProviderEvent::TextDelta(text.to_string()));
                }
                for tool in delta["tool_calls"].as_array().into_iter().flatten() {
                    let index =
                        tool["index"].as_u64().ok_or_else(|| ProviderError::permanent("stream tool index missing"))?
                            as usize;
                    let block = blocks
                        .entry(index)
                        .or_insert_with(|| json!({"id":"", "type":"function", "function":{"name":"", "arguments":""}}));
                    append(block, "id", &tool["id"]);
                    append(&mut block["function"], "name", &tool["function"]["name"]);
                    append(&mut block["function"], "arguments", &tool["function"]["arguments"]);
                }
                Ok(ControlFlow::Continue(()))
            };
            match pump_sse(response, cancel, self.stall_bound(), &mut on_frame).await? {
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
        } else {
            let bytes = tokio::select! {
                body = response.bytes() => body.map_err(|e| ProviderError::transient(format!("model stream: {e}")))?,
                _ = cancel.cancelled() => return Err(ProviderError::interrupted("turn interrupted")),
            };
            if bytes.len() > MAX_RESPONSE {
                return Err(ProviderError::permanent("chat API: response too large"));
            }
            let data: Json = serde_json::from_slice(&bytes)
                .map_err(|e| ProviderError::permanent(format!("chat API: invalid response: {e}")))?;
            if let Some(error) = data.get("error") {
                let text = error["message"].as_str().unwrap_or("unknown error");
                let class =
                    if super::context_overflow(text) { ErrorClass::ContextOverflow } else { ErrorClass::Permanent };
                return Err(ProviderError {
                    class,
                    message: format!("chat API: {text}"),
                    retry_after: None,
                    status: None,
                });
            }
            let assembled = data["choices"]
                .as_array()
                .and_then(|choices| choices.first())
                .ok_or_else(|| ProviderError::permanent("chat API: empty choices"))?;
            message = assembled["message"].clone();
            if message.is_null() {
                return Err(ProviderError::permanent("chat API: empty choices"));
            }
            finish_reason = assembled["finish_reason"].clone();
            usage = data["usage"].clone();
            if let Some(text) = message["content"].as_str() {
                if !text.is_empty() {
                    on_event(ProviderEvent::TextDelta(text.to_string()));
                }
            }
            complete = true;
            let _ = complete;
        }
        if cancel.is_cancelled() {
            return Err(ProviderError::interrupted("turn interrupted"));
        }
        let (message, usage, raw) = super::finish_chat_message(message, blocks, finish_reason, usage)?;
        Ok(AttemptOutcome {
            response: ModelResponse {
                message,
                usage,
                native: json!({"protocol": "chat_completions", "base": self.base}),
            },
            raw,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }
}
