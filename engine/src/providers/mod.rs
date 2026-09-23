//! R2 provider edge (rebuild plan §7): thin protocol adapters that perform
//! exactly one transport attempt. Retry ownership lives in the runtime — a
//! provider classifies failures, never loops. Protocol type is separate from
//! vendor name; native fields keep provenance and are never flattened away.

pub mod chat_completions;

use serde_json::{json, Value as Json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use teamagents_core::kernel::{ModelRequest, ModelResponse, Usage};

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
