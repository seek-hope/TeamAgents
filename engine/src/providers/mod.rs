//! R2 provider edge (rebuild plan §7): thin protocol adapters that perform
//! exactly one transport attempt. Retry ownership lives in the runtime — a
//! provider classifies failures, never loops. Protocol type is separate from
//! vendor name; native fields keep provenance and are never flattened away.

pub mod anthropic;
pub mod chat_completions;
pub mod responses;

use serde_json::{json, Value as Json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use teamagents_core::kernel::{KernelProfile, ModelRequest, ModelResponse, Usage};
use teamagents_core::models::{ModelProfile, UserConfig};

/// Cooperative cancellation for one attempt. Dropping a future is not
/// treated as proof an external effect stopped (RV-05): the runner/process
/// layer owns real teardown; this token only abandons the local read.
#[derive(Clone, Default)]
pub struct Cancel {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.notify.notified().await;
    }
}

/// Retry classification (ported contract): transient statuses and transport
/// failures only; protocol/cancellation are final for this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Transport failure before any visible output, or a retryable status:
    /// safe to re-attempt — nothing from the response executed or was shown.
    Transient,
    /// Endpoint answered with a permanent error, violated the protocol, or
    /// the stream broke after visible output. Not retryable.
    Permanent,
    /// Context window exceeded per the provider's own error text.
    ContextOverflow,
    /// Local cancellation; never retried, message preserved verbatim.
    Interrupted,
}

#[derive(Debug)]
pub struct ProviderError {
    pub class: ErrorClass,
    pub message: String,
    pub retry_after: Option<Duration>,
    pub status: Option<u16>,
}

impl ProviderError {
    pub fn transient(message: impl Into<String>) -> Self {
        ProviderError { class: ErrorClass::Transient, message: message.into(), retry_after: None, status: None }
    }
    pub fn permanent(message: impl Into<String>) -> Self {
        ProviderError { class: ErrorClass::Permanent, message: message.into(), retry_after: None, status: None }
    }
    pub fn interrupted(message: impl Into<String>) -> Self {
        ProviderError { class: ErrorClass::Interrupted, message: message.into(), retry_after: None, status: None }
    }
    pub fn context(message: impl Into<String>) -> Self {
        ProviderError { class: ErrorClass::ContextOverflow, message: message.into(), retry_after: None, status: None }
    }
    pub fn status(code: u16, body: &str, retry_after: Option<Duration>) -> Self {
        let text: String = body.chars().take(500).collect();
        let class = if retryable_status(code) {
            ErrorClass::Transient
        } else if context_overflow(&text) {
            ErrorClass::ContextOverflow
        } else {
            ErrorClass::Permanent
        };
        ProviderError { class, message: format!("chat API {code}: {text}"), retry_after, status: Some(code) }
    }
}

pub fn retryable_status(code: u16) -> bool {
    matches!(code, 408 | 409 | 429) || (500..600).contains(&code)
}

/// Provider-reported context exhaustion (ported token list).
pub fn context_overflow(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "context_length_exceeded",
        "maximum context length",
        "context window",
        "prompt is too long",
        "too many tokens",
        "input is too long",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// pi-ai style tool-call id normalization (transform-messages): Anthropic
/// requires tool ids matching ^[a-zA-Z0-9_-]{1,64}$, but Responses-protocol
/// ids run 450+ chars with '|'. Ids failing the rule rewrite to a
/// deterministic `tc_<hash16>`; the same original maps identically within a
/// request, so assistant tool_calls and their tool results stay paired.
/// Stored context keeps the original ids — only the outbound copy rewrites.
pub(crate) fn normalize_tool_call_ids(messages: &[Json]) -> Vec<Json> {
    use sha2::{Digest, Sha256};
    let valid = |id: &str| {
        !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    let mut rewritten: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut normalized_for = |id: &str| -> String {
        if valid(id) {
            return id.to_string();
        }
        rewritten
            .entry(id.to_string())
            .or_insert_with(|| {
                let digest = format!("{:x}", Sha256::digest(id.as_bytes()));
                format!("tc_{}", &digest[..16])
            })
            .clone()
    };
    messages
        .iter()
        .map(|message| {
            let mut message = message.clone();
            if let Some(calls) = message.get_mut("tool_calls").and_then(|v| v.as_array_mut()) {
                for call in calls {
                    if let Some(id) = call.get("id").and_then(|v| v.as_str()).map(str::to_string) {
                        call["id"] = json!(normalized_for(&id));
                    }
                }
            }
            if let Some(id) = message.get("tool_call_id").and_then(|v| v.as_str()).map(str::to_string) {
                message["tool_call_id"] = json!(normalized_for(&id));
            }
            message
        })
        .collect()
}

/// pi-ai style max_tokens clamp (simple-options): never request more than
/// the context window has left after the estimated prompt and a 4k safety
/// margin; an unknown window means no clamp.
pub(crate) fn clamp_max_tokens(configured: u64, est_prompt_tokens: u64, context_window: Option<u64>) -> u64 {
    const SAFETY_TOKENS: u64 = 4096;
    match context_window {
        Some(window) if window > 0 => {
            let available = window.saturating_sub(est_prompt_tokens).saturating_sub(SAFETY_TOKENS);
            configured.min(available.max(1))
        }
        _ => configured,
    }
}

/// Streaming preview events. Previews are never authoritative facts (§9):
/// slow clients may drop them; only the complete AttemptOutcome matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    TextDelta(String),
}

#[derive(Debug)]
pub struct AttemptOutcome {
    pub response: ModelResponse,
    /// Raw assembled wire body for the trace (no credentials, no auth headers).
    pub raw: Json,
    pub elapsed_ms: u64,
}

/// One transport attempt against one endpoint. Implementations must stream
/// (or read) to completion and only then yield a ModelResponse — partial
/// streams stay archived attempts and never produce tool intents.
pub trait Provider: Send + Sync {
    fn protocol(&self) -> &str;
    fn complete(
        &self,
        request: &ModelRequest,
        cancel: &Cancel,
        on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> impl std::future::Future<Output = Result<AttemptOutcome, ProviderError>> + Send;
}

/// One transport-attempt provider over the supported wire protocols (R17).
/// The runtime holds this single concrete type; protocol dispatch lives
/// here, not in the agent loop (§7 — protocol type is separate from vendor
/// name; pi-ai routes the same way via `model.api`).
pub enum AnyProvider {
    ChatCompletions(chat_completions::ChatCompletions),
    Responses(responses::Responses),
    Anthropic(anthropic::Anthropic),
}

impl Provider for AnyProvider {
    fn protocol(&self) -> &str {
        match self {
            AnyProvider::ChatCompletions(provider) => provider.protocol(),
            AnyProvider::Responses(provider) => provider.protocol(),
            AnyProvider::Anthropic(provider) => provider.protocol(),
        }
    }

    async fn complete(
        &self,
        request: &ModelRequest,
        cancel: &Cancel,
        on_event: &mut (dyn FnMut(ProviderEvent) + Send),
    ) -> Result<AttemptOutcome, ProviderError> {
        match self {
            AnyProvider::ChatCompletions(provider) => provider.complete(request, cancel, on_event).await,
            AnyProvider::Responses(provider) => provider.complete(request, cancel, on_event).await,
            AnyProvider::Anthropic(provider) => provider.complete(request, cancel, on_event).await,
        }
    }
}

/// Resolve a catalog-keyed instance profile into the effective kernel
/// profile (R17): the wire model id and generation options come from the
/// catalog entry; instance-stored option overrides win; the context window
/// follows the resolved model (D-36). Unknown keys pass through verbatim so
/// scripted/test providers never consult the catalog.
pub fn resolve_profile(profile: KernelProfile, catalog: &UserConfig) -> KernelProfile {
    let Some(entry) = catalog.models.get(&profile.model) else { return profile };
    let mut options = entry.generation_options.clone();
    if let Some(overrides) = profile.options.as_object() {
        options.extend(overrides.iter().map(|(key, value)| (key.clone(), value.clone())));
    }
    KernelProfile {
        model: entry.model.clone(),
        instructions: profile.instructions.clone(),
        tools: profile.tools.clone(),
        options: serde_json::to_value(options).unwrap_or_else(|_| serde_json::json!({})),
        context_window: entry.context_window.or(profile.context_window),
    }
}

/// Build the wire adapter for one catalog model id (R17). The catalog entry
/// owns protocol/base/credential *references*; credentials resolve from the
/// environment here at the config boundary, never inside an adapter (§7).
/// ponytail: pi-ai style compat auto-detection is deliberately not ported —
/// the catalog declares the protocol explicitly and only contract-verified
/// wire behaviors ship. Effort value normalization (deepseek xhigh→max)
/// belongs to the config layer's options snapshot, not the adapters.
pub fn build_for_model(catalog: &UserConfig, model: &str) -> Result<AnyProvider, String> {
    let profile = catalog.models.get(model).ok_or_else(|| format!("model {model} is not in the user catalog"))?;
    build_for_profile(profile)
}

pub fn build_for_profile(profile: &ModelProfile) -> Result<AnyProvider, String> {
    let api_key = match &profile.api_key_env {
        Some(env) => std::env::var(env).map_err(|_| format!("missing API key env {env}"))?,
        None => String::new(),
    };
    let timeout = Duration::from_secs(profile.timeout.max(1) as u64);
    let configured = profile.base_url.as_deref().map(str::trim).filter(|base| !base.is_empty());
    match profile.protocol.as_str() {
        "anthropic" => Ok(AnyProvider::Anthropic(
            anthropic::Anthropic::new(configured.unwrap_or("https://api.anthropic.com"), api_key, timeout)?
                .with_context_window(profile.context_window),
        )),
        "responses" => Ok(AnyProvider::Responses(
            responses::Responses::new(configured.unwrap_or("https://api.openai.com/v1"), api_key, timeout)?
                .with_context_window(profile.context_window),
        )),
        // "openai" (legacy), "chat/completions" and "deepseek" share the
        // chat-completions wire shape (ported contract).
        _ => {
            let default = if profile.protocol == "deepseek" || profile.provider == "deepseek" {
                "https://api.deepseek.com/v1"
            } else {
                "https://api.openai.com/v1"
            };
            Ok(AnyProvider::ChatCompletions(
                chat_completions::ChatCompletions::new(configured.unwrap_or(default), api_key, timeout)?
                    .with_context_window(profile.context_window),
            ))
        }
    }
}

/// Assemble the final chat-shaped response from accumulated stream state.
pub(crate) fn finish_chat_message(
    mut message: Json,
    blocks: std::collections::BTreeMap<usize, Json>,
    finish_reason: Json,
    usage: Json,
) -> Result<(Json, Option<Usage>, Json), ProviderError> {
    if !blocks.is_empty() {
        message["tool_calls"] = json!(blocks.into_values().collect::<Vec<_>>());
    }
    if message["content"].is_null() && message["tool_calls"].is_null() {
        return Err(ProviderError::permanent("chat API: empty choices"));
    }
    let usage_parsed = Usage::from_json(&json!({"usage": usage}));
    let raw = json!({"choices": [{"message": message, "finish_reason": finish_reason}], "usage": usage});
    let response_message = raw["choices"][0]["message"].clone();
    Ok((response_message, usage_parsed, raw))
}

/// Shared SSE framing for streaming adapters (R17): chunk buffering, frame
/// split on blank lines, `data:` extraction. Protocol events stay with the
/// caller; the handler returns `ControlFlow::Break` to stop the pump early
/// (e.g. after a terminal event while the server keeps the socket open).
pub(crate) enum SseEnd {
    /// Clean EOF, or the handler asked to stop after a terminal event.
    Closed,
    /// Local cancellation while reading.
    Cancelled,
    /// Transport failure with the reqwest error text; retry safety depends
    /// on caller-owned emission state, so classification stays at the call
    /// site via `stream_failure_msg`.
    Transport(String),
}

pub(crate) async fn pump_sse(
    response: reqwest::Response,
    cancel: &Cancel,
    mut on_frame: impl FnMut(&str) -> Result<std::ops::ControlFlow<()>, ProviderError> + Send,
) -> Result<SseEnd, ProviderError> {
    use std::ops::ControlFlow;
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    loop {
        let chunk = tokio::select! {
            chunk = futures_next(&mut stream) => chunk,
            _ = cancel.cancelled() => return Ok(SseEnd::Cancelled),
        };
        let Some(chunk) = chunk else { return Ok(SseEnd::Closed) };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => return Ok(SseEnd::Transport(format!("model stream: {error}"))),
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        // SSE frames are separated by a blank line.
        while let Some(pos) = buffer.find("\n\n") {
            let frame = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();
            for line in frame.lines() {
                let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
                if let ControlFlow::Break(()) = on_frame(data)? {
                    return Ok(SseEnd::Closed);
                }
            }
        }
    }
}

/// Append a string field from a wire delta onto an assembled JSON object.
pub(crate) fn append(target: &mut Json, key: &str, delta: &Json) {
    if let Some(text) = delta.as_str() {
        let merged = format!("{}{}", target[key].as_str().unwrap_or(""), text);
        target[key] = Json::String(merged);
    }
}

/// Mid-stream failure: retryable only before any visible output.
pub(crate) fn stream_failure_msg(message: &str, emitted: bool) -> ProviderError {
    if emitted {
        ProviderError::permanent(message)
    } else {
        ProviderError::transient(message)
    }
}

/// reqwest 0.12 stream helper (futures-util is only used for StreamExt).
pub(crate) async fn futures_next<S, T>(stream: &mut S) -> Option<Result<T, reqwest::Error>>
where
    S: futures_util::Stream<Item = Result<T, reqwest::Error>> + Unpin,
{
    use futures_util::StreamExt;
    stream.next().await
}
