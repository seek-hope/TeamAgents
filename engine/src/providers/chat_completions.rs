//! OpenAI-compatible Chat Completions adapter, covering DeepSeek (plan §7,
//! RV-27). DeepSeek native fields such as `reasoning_content` are preserved
//! in the assembled message; usage comes from the terminal stream frame.

use super::{AttemptOutcome, Cancel, ErrorClass, Provider, ProviderError, ProviderEvent};
use serde_json::{json, Value as Json};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use teamagents_core::kernel::{ModelRequest, ModelResponse};

/// Cap on a non-SSE (error or non-streaming) response body.
const MAX_RESPONSE: usize = 64 * 1024 * 1024;

pub struct ChatCompletions {
    client: reqwest::Client,
    base: String,
    api_key: String,
    timeout: Duration,
}

impl ChatCompletions {
    /// `base` is the API root without a trailing slash (e.g.
    /// https://api.deepseek.com/v1). Credentials arrive already resolved —
    /// the config layer reads env/local credentials, this adapter never
    /// touches the environment itself.
    pub fn new(base: impl Into<String>, api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| format!("chat client: {e}"))?;
        Ok(ChatCompletions { client, base: base.into(), api_key: api_key.into(), timeout })
    }

    pub fn deepseek(api_key: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        Self::new("https://api.deepseek.com/v1", api_key, timeout)
    }

    fn body(request: &ModelRequest) -> Json {
        let mut body = json!({
            "model": request.model,
            "messages": request.messages,
            "tools": request.tools,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        if let (Some(target), Some(options)) = (body.as_object_mut(), request.options.as_object()) {
            for (key, value) in options {
                target.insert(key.clone(), value.clone());
            }
        }
        body
    }
}

fn append(target: &mut Json, key: &str, delta: &Json) {
    if let Some(text) = delta.as_str() {
        let merged = format!("{}{}", target[key].as_str().unwrap_or(""), text);
        target[key] = Json::String(merged);
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
            send = http.body(Self::body(request).to_string()).send() => {
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
            let mut stream = response.bytes_stream();
            let mut buffer = String::new();
            loop {
                let chunk = tokio::select! {
                    chunk = futures_next(&mut stream) => chunk,
                    _ = cancel.cancelled() => return Err(ProviderError::interrupted("turn interrupted")),
                };
                let Some(chunk) = chunk else { break };
                let chunk = chunk.map_err(|e| stream_failure(e, emitted))?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                // SSE frames are separated by a blank line.
                while let Some(pos) = buffer.find("\n\n") {
                    let frame = buffer[..pos].to_string();
                    buffer = buffer[pos + 2..].to_string();
                    for line in frame.lines() {
                        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
                        if data == "[DONE]" {
                            complete = true;
                            continue;
                        }
                        let Ok(data) = serde_json::from_str::<Json>(data) else { continue };
                        if data["usage"].is_object() {
                            usage = data["usage"].clone();
                        }
                        let choice = data["choices"]
                            .as_array()
                            .and_then(|choices| choices.iter().find(|c| c["index"].as_u64().unwrap_or(0) == 0));
                        let Some(choice) = choice else { continue };
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
                            let index = tool["index"]
                                .as_u64()
                                .ok_or_else(|| ProviderError::permanent("stream tool index missing"))?
                                as usize;
                            let block = blocks.entry(index).or_insert_with(
                                || json!({"id":"", "type":"function", "function":{"name":"", "arguments":""}}),
                            );
                            append(block, "id", &tool["id"]);
                            append(&mut block["function"], "name", &tool["function"]["name"]);
                            append(&mut block["function"], "arguments", &tool["function"]["arguments"]);
                        }
                    }
                }
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
                body = response.bytes() => body.map_err(|e| stream_failure(e, false))?,
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

fn stream_failure(error: reqwest::Error, emitted: bool) -> ProviderError {
    stream_failure_msg(&format!("model stream: {error}"), emitted)
}

fn stream_failure_msg(message: &str, emitted: bool) -> ProviderError {
    if emitted {
        ProviderError::permanent(message)
    } else {
        ProviderError::transient(message)
    }
}

/// reqwest 0.12 stream helper (futures-util is only used for StreamExt).
async fn futures_next<S, T>(stream: &mut S) -> Option<Result<T, reqwest::Error>>
where
    S: futures_util::Stream<Item = Result<T, reqwest::Error>> + Unpin,
{
    use futures_util::StreamExt;
    stream.next().await
}
