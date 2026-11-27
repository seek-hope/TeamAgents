//! Anthropic Messages API transport (`TEAMAGENTS_PROVIDER=anthropic`).
use super::{
    checked_post_request, decode_action, decode_tool_call_str, decode_tool_call_value, is_context_overflow_payload,
    is_quota_exceeded_payload, is_retryable_status, note_rate_limit, parse_history, request_limit, retry_delay,
    send_with_redirects, system_prompt, tool_specs, wait_rate_limit, AuthHeader, HistoryItem, ModelError,
    ModelSettings, HTTP_MAX_ATTEMPTS, HTTP_RETRY_BASE, LIMIT, STREAM_LIMIT, TIMEOUT,
};
use crate::{protocol::TurnKind, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{env, io::Read, thread, time::Duration};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const INVALID: &str = "invalid or ambiguous Anthropic response; no action accepted";
const TEXT_ONLY: &str = "Anthropic must return a single assistant tool call or JSON action without refusals";
const MULTI_TOOL: &str = "expected exactly one tool call per turn; emit one action at a time";

pub(super) fn messages_endpoint(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        format!("{trimmed}/messages")
    } else {
        format!("{trimmed}/v1/messages")
    }
}

fn default_max_tokens(model: &str) -> u32 {
    let lower = model.to_ascii_lowercase();
    if lower.starts_with("claude-3-opus") || lower.starts_with("claude-3-sonnet") || lower.starts_with("claude-3-haiku")
    {
        4096
    } else if lower.starts_with("claude-3-5") || lower.starts_with("claude-3.5") {
        8192
    } else {
        16384
    }
}

pub(super) fn resolve_thinking_and_max_tokens(
    model: &str,
    effort: Option<&str>,
    override_max: Option<u32>,
) -> (u32, Option<Value>) {
    let base = override_max.unwrap_or_else(|| default_max_tokens(model));
    let budget = match effort {
        None => return (base, None),
        Some("none") => return (base, Some(json!({"type": "disabled"}))),
        Some("minimal") => 1024u32,
        Some("low") => 2048u32,
        Some("medium") => 8192u32,
        Some("high") => 16384u32,
        Some("xhigh") => 32768u32,
        Some("max") => 48000u32,
        Some(_) => return (base, None),
    };
    let max_tokens = if let Some(explicit) = override_max {
        if explicit > budget {
            explicit
        } else {
            budget.saturating_add(1024)
        }
    } else {
        base.max(budget.saturating_add(16384)).min(64000)
    };
    (max_tokens, Some(json!({"type": "enabled", "budget_tokens": budget})))
}

fn configured_max_tokens() -> Result<Option<u32>> {
    let Ok(raw) = env::var("ANTHROPIC_MAX_TOKENS") else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let val =
        trimmed.parse::<u32>().ok().filter(|&n| n > 0).ok_or("ANTHROPIC_MAX_TOKENS must be a positive integer")?;
    Ok(Some(val))
}

fn push_block<'a>(groups: &mut Vec<(&'a str, Vec<Value>)>, role: &'a str, block: Value) {
    if groups.last().is_some_and(|(r, _)| *r == role) {
        groups.last_mut().unwrap().1.push(block);
    } else {
        groups.push((role, vec![block]));
    }
}

pub(super) fn body(
    model: &str,
    effort: Option<&str>,
    agent: &str,
    task: &str,
    history: &[Value],
    kind: TurnKind,
    override_max: Option<u32>,
) -> Result<Vec<u8>> {
    let (max_bytes, too_big) = request_limit(kind);
    let mut size = agent.len().saturating_add(task.len()).saturating_add(model.len());
    if size > max_bytes {
        return Err(too_big.into());
    }
    let (max_tokens, thinking) = resolve_thinking_and_max_tokens(model, effort, override_max);
    let thinking_enabled = thinking.as_ref().and_then(|t| t["type"].as_str()) == Some("enabled");

    let assignment = format!("Agent: {agent}\nTask: {task}");
    let mut groups: Vec<(&str, Vec<Value>)> = vec![("user", vec![json!({"type": "text", "text": assignment})])];
    let items = parse_history(history, &mut size, max_bytes, too_big)?;

    for item in items {
        match item {
            HistoryItem::ToolTurn { call_id, name, raw_assistant, args_value, output, .. } => {
                if !thinking_enabled {
                    push_block(
                        &mut groups,
                        "assistant",
                        json!({"type": "tool_use", "id": call_id, "name": name, "input": args_value}),
                    );
                    push_block(
                        &mut groups,
                        "user",
                        json!({"type": "tool_result", "tool_use_id": call_id, "content": output}),
                    );
                } else {
                    // Anthropic extended thinking rejects unsigned historical tool_use blocks when replayed
                    // without persisted thinking+signature blocks; fall back to plain text replay.
                    push_block(&mut groups, "assistant", json!({"type": "text", "text": raw_assistant}));
                    push_block(&mut groups, "user", json!({"type": "text", "text": output}));
                }
            }
            HistoryItem::Message { role, content } => {
                if !content.is_empty() {
                    push_block(&mut groups, role, json!({"type": "text", "text": content}));
                } else if !groups.last().is_some_and(|(r, _)| *r == role) {
                    groups.push((role, Vec::new()));
                }
            }
        }
    }

    let messages: Vec<Value> = groups
        .into_iter()
        .map(|(role, mut blocks)| {
            if blocks.is_empty() {
                blocks.push(json!({"type": "text", "text": "(empty)"}));
            }
            json!({"role": role, "content": blocks})
        })
        .collect();

    let tools: Vec<Value> = tool_specs(kind)
        .into_iter()
        .map(|s| json!({"name": s.name, "description": s.description, "input_schema": s.parameters}))
        .collect();
    let mut payload = json!({
        "model": model,
        "system": system_prompt(kind),
        "messages": messages,
        "tools": tools,
        "tool_choice": {
            "type": if thinking_enabled { "auto" } else { "any" },
            "disable_parallel_tool_use": true
        },
        "max_tokens": max_tokens,
        "stream": true,
    });
    if let Some(thinking) = thinking {
        payload["thinking"] = thinking;
    }
    let bytes = serde_json::to_vec(&payload).map_err(|_| "cannot encode model context")?;
    if bytes.len() > max_bytes {
        Err(too_big.into())
    } else {
        Ok(bytes)
    }
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    name: Option<String>,
    input: Option<Value>,
}

#[derive(Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct StartMessage {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    kind: String,
    message: Option<StartMessage>,
    index: Option<usize>,
    content_block: Option<ContentBlock>,
    delta: Option<Delta>,
    error: Option<ApiError>,
}

enum BlockKind {
    Text,
    Thinking,
    ToolUse { name: String, input_json: String, initial_input: Option<Value> },
}

enum SseDecodeErr {
    Model(ModelError),
    ProviderEvent { err_type: Option<String>, raw_json: String },
}

fn check_stop_reason(reason: Option<&str>) -> std::result::Result<(), ModelError> {
    match reason {
        None | Some("end_turn" | "stop_sequence" | "tool_use") => Ok(()),
        Some("max_tokens") => {
            Err(ModelError::Fatal("model output truncated by provider token limit; no action accepted".into()))
        }
        Some("refusal" | "pause_turn") => Err(ModelError::InvalidAction(TEXT_ONLY.into())),
        Some(other) => {
            Err(ModelError::Fatal(format!("Anthropic did not complete normally ({other}); no action accepted")))
        }
    }
}

fn decode_sse(stream: &str, kind: TurnKind) -> std::result::Result<Value, SseDecodeErr> {
    let mut events = Vec::new();
    let mut data = String::new();
    let mut name = None;
    let mut done = false;
    let inv = || SseDecodeErr::Model(ModelError::Fatal(INVALID.into()));

    for line in stream.lines() {
        if line.is_empty() {
            if data.is_empty() && name.is_some() {
                return Err(inv());
            }
            if !data.is_empty() {
                if done {
                    return Err(inv());
                }
                if data == "[DONE]\n" && name.is_none() {
                    done = true;
                } else {
                    let ev: SseEvent = serde_json::from_str(&data).map_err(|_| inv())?;
                    if name.is_some_and(|n| n != ev.kind) {
                        return Err(inv());
                    }
                    if ev.kind == "error" {
                        let err_type = ev.error.and_then(|e| e.kind);
                        return Err(SseDecodeErr::ProviderEvent { err_type, raw_json: data });
                    }
                    events.push(ev);
                }
            }
            data.clear();
            name = None;
        } else {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => {
                    data.push_str(value);
                    data.push('\n');
                }
                "event" if name.replace(value).is_none() => (),
                "" | "id" | "retry" => (),
                _ => return Err(inv()),
            }
        }
    }
    if !data.is_empty() || name.is_some() {
        return Err(inv());
    }

    let mut saw_start = false;
    let mut saw_stop = false;
    let mut stop_reason: Option<String> = None;
    let mut blocks: Vec<(BlockKind, bool)> = Vec::new(); // (kind, open)
    let mut text = String::new();
    let mut saw_text_block = false;

    for ev in events {
        if saw_stop {
            return Err(inv());
        }
        match ev.kind.as_str() {
            "ping" => {}
            "message_start" => {
                if saw_start {
                    return Err(inv());
                }
                let msg = ev.message.ok_or_else(inv)?;
                if msg.kind.as_deref().is_some_and(|k| k != "message") || msg.role.as_deref() != Some("assistant") {
                    return Err(inv());
                }
                if msg.stop_reason.is_some() {
                    stop_reason = msg.stop_reason;
                }
                saw_start = true;
            }
            "content_block_start" => {
                if !saw_start {
                    return Err(inv());
                }
                let idx = ev.index.ok_or_else(inv)?;
                if idx != blocks.len() {
                    return Err(inv());
                }
                let cb = ev.content_block.ok_or_else(inv)?;
                match cb.kind.as_str() {
                    "text" => {
                        saw_text_block = true;
                        if let Some(t) = cb.text {
                            if text.len().saturating_add(t.len()) > LIMIT {
                                return Err(SseDecodeErr::Model(ModelError::Fatal(
                                    "Anthropic response exceeds 2 MiB; no action accepted".into(),
                                )));
                            }
                            text.push_str(&t);
                        }
                        blocks.push((BlockKind::Text, true));
                    }
                    "thinking" | "redacted_thinking" => {
                        blocks.push((BlockKind::Thinking, true));
                    }
                    "tool_use" => {
                        let tool_name = cb
                            .name
                            .filter(|n| !n.is_empty())
                            .ok_or_else(|| SseDecodeErr::Model(ModelError::InvalidAction(TEXT_ONLY.into())))?;
                        blocks.push((
                            BlockKind::ToolUse { name: tool_name, input_json: String::new(), initial_input: cb.input },
                            true,
                        ));
                    }
                    _ => return Err(SseDecodeErr::Model(ModelError::InvalidAction(TEXT_ONLY.into()))),
                }
            }
            "content_block_delta" => {
                if !saw_start {
                    return Err(inv());
                }
                let idx = ev.index.ok_or_else(inv)?;
                let (bkind, open) = blocks.get_mut(idx).ok_or_else(inv)?;
                if !*open {
                    return Err(inv());
                }
                let delta = ev.delta.ok_or_else(inv)?;
                let dkind = delta.kind.as_deref().ok_or_else(inv)?;
                match (bkind, dkind) {
                    (BlockKind::Text, "text_delta") => {
                        let piece = delta.text.as_deref().ok_or_else(inv)?;
                        if text.len().saturating_add(piece.len()) > LIMIT {
                            return Err(SseDecodeErr::Model(ModelError::Fatal(
                                "Anthropic response exceeds 2 MiB; no action accepted".into(),
                            )));
                        }
                        text.push_str(piece);
                    }
                    (BlockKind::ToolUse { input_json, .. }, "input_json_delta") => {
                        let piece = delta.partial_json.as_deref().ok_or_else(inv)?;
                        if input_json.len().saturating_add(piece.len()) > LIMIT {
                            return Err(SseDecodeErr::Model(ModelError::Fatal(
                                "Anthropic response exceeds 2 MiB; no action accepted".into(),
                            )));
                        }
                        input_json.push_str(piece);
                    }
                    (BlockKind::Text, "citations_delta")
                    | (BlockKind::Thinking, "thinking_delta" | "signature_delta") => {}
                    _ => return Err(SseDecodeErr::Model(ModelError::InvalidAction(TEXT_ONLY.into()))),
                }
            }
            "content_block_stop" => {
                if !saw_start {
                    return Err(inv());
                }
                let idx = ev.index.ok_or_else(inv)?;
                let block = blocks.get_mut(idx).ok_or_else(inv)?;
                if !block.1 {
                    return Err(inv());
                }
                block.1 = false;
            }
            "message_delta" => {
                if !saw_start {
                    return Err(inv());
                }
                if let Some(d) = ev.delta {
                    if let Some(reason) = d.stop_reason {
                        stop_reason = Some(reason);
                    }
                }
            }
            "message_stop" => {
                if !saw_start {
                    return Err(inv());
                }
                saw_stop = true;
            }
            _ => return Err(inv()),
        }
    }

    if !saw_start || !saw_stop || blocks.iter().any(|(_, open)| *open) {
        return Err(SseDecodeErr::Model(ModelError::Fatal(
            "Anthropic stream ended without message_stop; no action accepted".into(),
        )));
    }
    check_stop_reason(stop_reason.as_deref()).map_err(SseDecodeErr::Model)?;
    let tool_uses: Vec<(String, String, Option<Value>)> = blocks
        .into_iter()
        .filter_map(|(bk, _)| match bk {
            BlockKind::ToolUse { name, input_json, initial_input } => Some((name, input_json, initial_input)),
            _ => None,
        })
        .collect();
    if tool_uses.len() > 1 {
        return Err(SseDecodeErr::Model(ModelError::InvalidAction(MULTI_TOOL.into())));
    }
    if let Some((tname, input_json, initial_input)) = tool_uses.into_iter().next() {
        return if !input_json.trim().is_empty() {
            decode_tool_call_str(&tname, &input_json, kind)
        } else {
            decode_tool_call_value(&tname, initial_input.unwrap_or_else(|| json!({})), kind)
        }
        .map_err(SseDecodeErr::Model);
    }
    if !saw_text_block {
        return Err(SseDecodeErr::Model(ModelError::InvalidAction(TEXT_ONLY.into())));
    }
    decode_action(&text, kind).map_err(SseDecodeErr::Model)
}

fn redacted_error(status: Option<u16>, err_type: Option<&str>, kind: TurnKind, body: &[u8]) -> ModelError {
    if kind != TurnKind::Reporter
        && (status == Some(413) || (matches!(status, Some(400 | 422) | None) && is_context_overflow_payload(body)))
    {
        return ModelError::NeedsCompaction("Anthropic context overflow".into());
    }
    if status == Some(402) || err_type == Some("billing_error") || is_quota_exceeded_payload(body) {
        return ModelError::QuotaExceeded(
            "Anthropic billing or quota limit exceeded; check provider billing and usage".into(),
        );
    }
    let detail = match err_type {
        Some("authentication_error") => "authentication failed (check ANTHROPIC_API_KEY)",
        Some("permission_error") => "access forbidden for this model or endpoint",
        Some("rate_limit_error") => "rate limit exceeded; retry later",
        Some("overloaded_error") => "service overloaded; retry later",
        Some("invalid_request_error") => "request rejected (check context size, model ID and thinking budget)",
        Some("not_found_error") => "model or endpoint not found",
        _ => match status {
            Some(401) => "authentication failed (check ANTHROPIC_API_KEY)",
            Some(403) => "access forbidden for this model or endpoint",
            Some(429) => "rate limit exceeded; retry later",
            Some(400 | 404 | 422) => "request rejected (check context size, model ID and reasoning effort)",
            Some(500..=599) => "service unavailable; retry later",
            _ => "request failed; no action accepted",
        },
    };
    match status {
        Some(s) => ModelError::Fatal(format!("Anthropic HTTP {s}: {detail}")),
        None => ModelError::Fatal(format!("Anthropic inference failed: {detail}")),
    }
}

pub(super) fn decode_json_response(bytes: &[u8], kind: TurnKind) -> std::result::Result<Value, ModelError> {
    let raw: Value = serde_json::from_slice(bytes).map_err(|_| ModelError::Fatal(INVALID.into()))?;
    if raw["type"] == "error" || raw.get("error").is_some_and(|e| !e.is_null()) {
        let err_type = raw.pointer("/error/type").and_then(Value::as_str);
        return Err(redacted_error(Some(200), err_type, kind, bytes));
    }
    if raw["type"] != "message" || raw["role"] != "assistant" {
        return Err(ModelError::Fatal(INVALID.into()));
    }
    check_stop_reason(raw["stop_reason"].as_str())?;
    let content = raw["content"].as_array().ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
    let mut text = String::new();
    let mut saw_text = false;
    let mut tool_uses = Vec::new();
    for block in content {
        match block["type"].as_str() {
            Some("thinking" | "redacted_thinking") => {}
            Some("text") => {
                let piece = block["text"].as_str().ok_or_else(|| ModelError::InvalidAction(TEXT_ONLY.into()))?;
                if text.len().saturating_add(piece.len()) > LIMIT {
                    return Err(ModelError::Fatal("Anthropic response exceeds 2 MiB; no action accepted".into()));
                }
                text.push_str(piece);
                saw_text = true;
            }
            Some("tool_use") => {
                let name = block["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ModelError::InvalidAction(TEXT_ONLY.into()))?
                    .to_owned();
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_uses.push((name, input));
            }
            _ => return Err(ModelError::InvalidAction(TEXT_ONLY.into())),
        }
    }
    if tool_uses.len() > 1 {
        return Err(ModelError::InvalidAction(MULTI_TOOL.into()));
    }
    if let Some((name, input)) = tool_uses.into_iter().next() {
        return decode_tool_call_value(&name, input, kind);
    }
    if !saw_text {
        return Err(ModelError::InvalidAction(TEXT_ONLY.into()));
    }
    decode_action(&text, kind)
}

enum RetryErr {
    Retryable(Duration),
    Done(ModelError),
}

fn read_bounded_stream(r: ureq::Response) -> std::result::Result<Vec<u8>, bool> {
    let mut bytes = Vec::new();
    if r.into_reader().take((STREAM_LIMIT + 1) as u64).read_to_end(&mut bytes).is_err() {
        return Err(true);
    }
    if bytes.len() > STREAM_LIMIT {
        return Err(false);
    }
    Ok(bytes)
}

fn handle_outcome(
    outcome: std::result::Result<ureq::Response, ureq::Error>,
    kind: TurnKind,
    attempt: usize,
    retry_base: Duration,
) -> std::result::Result<Value, RetryErr> {
    let resp = match outcome {
        Ok(r) if (200..300).contains(&r.status()) => r,
        Ok(r) | Err(ureq::Error::Status(_, r)) => {
            let status = r.status();
            let retry_after = r.header("Retry-After").map(str::to_owned);
            let mut err_buf = Vec::new();
            let _ = r.into_reader().take(16 * 1024).read_to_end(&mut err_buf);
            let details = serde_json::from_slice::<Value>(&err_buf).ok();
            let err_type = details.as_ref().and_then(|v| v.pointer("/error/type")).and_then(Value::as_str);

            if kind != TurnKind::Reporter
                && (status == 413 || (matches!(status, 400 | 422) && is_context_overflow_payload(&err_buf)))
            {
                return Err(RetryErr::Done(ModelError::NeedsCompaction(format!(
                    "Anthropic HTTP {status}: context overflow"
                ))));
            }
            if status == 402 || err_type == Some("billing_error") || is_quota_exceeded_payload(&err_buf) {
                return Err(RetryErr::Done(ModelError::QuotaExceeded(
                    "Anthropic billing or quota limit exceeded; check provider billing and usage".into(),
                )));
            }
            if (is_retryable_status(status) || err_type == Some("overloaded_error")) && attempt < HTTP_MAX_ATTEMPTS {
                let delay = retry_delay(attempt, retry_after.as_deref(), retry_base);
                note_rate_limit(delay, retry_base);
                return Err(RetryErr::Retryable(delay));
            }
            return Err(RetryErr::Done(redacted_error(Some(status), err_type, kind, &err_buf)));
        }
        Err(ureq::Error::Transport(_)) => {
            if attempt < HTTP_MAX_ATTEMPTS {
                return Err(RetryErr::Retryable(retry_delay(attempt, None, retry_base)));
            }
            return Err(RetryErr::Done(ModelError::Fatal("Anthropic transport failed".into())));
        }
    };

    let media = resp.header("Content-Type").map(|_| resp.content_type().trim().to_ascii_lowercase());
    let raw = match read_bounded_stream(resp) {
        Ok(b) => b,
        Err(true) => {
            if attempt < HTTP_MAX_ATTEMPTS {
                return Err(RetryErr::Retryable(retry_delay(attempt, None, retry_base)));
            }
            return Err(RetryErr::Done(ModelError::Fatal("Anthropic response interrupted".into())));
        }
        Err(false) => {
            return Err(RetryErr::Done(ModelError::Fatal(
                "Anthropic stream exceeds 32 MiB; no action accepted".into(),
            )));
        }
    };
    let text = std::str::from_utf8(&raw).ok().map(|s| s.strip_prefix('\u{feff}').unwrap_or(s));
    if let Some(text) = text {
        let first = text.lines().find(|line| !line.is_empty()).unwrap_or("");
        let framed = ["event:", "data:", ":"].iter().any(|prefix| first.starts_with(prefix));
        if media.as_deref() == Some("text/event-stream") || (media.as_deref() != Some("text/html") && framed) {
            return decode_sse(text, kind).map_err(|err| match err {
                SseDecodeErr::Model(m) => RetryErr::Done(m),
                SseDecodeErr::ProviderEvent { err_type, raw_json } => {
                    if kind != TurnKind::Reporter && is_context_overflow_payload(raw_json.as_bytes()) {
                        return RetryErr::Done(ModelError::NeedsCompaction(
                            "Anthropic HTTP 200: context overflow".into(),
                        ));
                    }
                    if err_type.as_deref() == Some("billing_error") || is_quota_exceeded_payload(raw_json.as_bytes()) {
                        return RetryErr::Done(ModelError::QuotaExceeded(
                            "Anthropic billing or quota limit exceeded; check provider billing and usage".into(),
                        ));
                    }
                    if matches!(err_type.as_deref(), Some("overloaded_error" | "rate_limit_error" | "api_error"))
                        && attempt < HTTP_MAX_ATTEMPTS
                    {
                        let delay = retry_delay(attempt, None, retry_base);
                        note_rate_limit(delay, retry_base);
                        return RetryErr::Retryable(delay);
                    }
                    RetryErr::Done(redacted_error(None, err_type.as_deref(), kind, raw_json.as_bytes()))
                }
            });
        }
        if media.as_deref() == Some("application/json") || text.trim_start().starts_with('{') {
            return decode_json_response(&raw, kind).map_err(RetryErr::Done);
        }
    }
    Err(RetryErr::Done(ModelError::Fatal("Anthropic HTTP 200: expected event stream or JSON response".into())))
}

pub(super) fn transact_with_retry(
    base: &str,
    key: Option<&str>,
    timeout: Duration,
    bytes: &[u8],
    kind: TurnKind,
    retry_base: Duration,
) -> std::result::Result<Value, ModelError> {
    let initial = messages_endpoint(base);
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        wait_rate_limit(retry_base);
        let mut first_hop = true;
        let outcome = send_with_redirects(&initial, Some(bytes), |url, send_auth| {
            let allow_query = !first_hop;
            first_hop = false;
            Ok(checked_post_request(
                url,
                key,
                send_auth,
                timeout,
                allow_query,
                "ANTHROPIC_BASE_URL",
                "ANTHROPIC_API_KEY",
                AuthHeader::XApiKey,
            )?
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("Accept", "text/event-stream, application/json"))
        })
        .map_err(ModelError::Fatal)?;
        match handle_outcome(outcome, kind, attempt, retry_base) {
            Ok(action) => return Ok(action),
            Err(RetryErr::Retryable(delay)) => thread::sleep(delay),
            Err(RetryErr::Done(err)) => return Err(err),
        }
    }
    unreachable!("loop always returns within HTTP_MAX_ATTEMPTS")
}

pub(super) fn complete(
    base: &str,
    key: Option<&str>,
    settings: &ModelSettings,
    agent: &str,
    task: &str,
    history: &[Value],
    kind: TurnKind,
) -> std::result::Result<Value, ModelError> {
    let model = settings.model.as_deref().ok_or_else(|| {
        ModelError::Fatal("set TEAMAGENTS_MODEL or select an Anthropic model with /model MODEL".into())
    })?;
    let max_override = configured_max_tokens().map_err(ModelError::Fatal)?;
    let bytes =
        body(model, settings.effort.as_deref(), agent, task, history, kind, max_override).map_err(ModelError::Fatal)?;
    transact_with_retry(base, key, TIMEOUT, &bytes, kind, HTTP_RETRY_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    fn frames(events: &[Value]) -> String {
        events.iter().map(|e| format!("event: {}\r\ndata: {e}\r\n\r\n", e["type"].as_str().unwrap())).collect()
    }

    #[test]
    fn builds_endpoints_thinking_and_coalesces_same_role_messages() {
        assert_eq!(messages_endpoint("https://api.anthropic.com"), "https://api.anthropic.com/v1/messages");
        assert_eq!(messages_endpoint("https://api.anthropic.com/"), "https://api.anthropic.com/v1/messages");
        assert_eq!(messages_endpoint("https://proxy.example.com/v1"), "https://proxy.example.com/v1/messages");
        assert_eq!(messages_endpoint("https://proxy.example.com/v1/"), "https://proxy.example.com/v1/messages");

        let history = [
            json!({"role":"user","content":"extra context"}),
            json!({"role":"assistant","content":r#"{"op":"read_file","path":"a"}"#}),
            json!({"role":"user","content":r#"{"tool":"read_file","ok":true,"result":"data"}"#}),
            json!({"role":"user","content":"Continue task t1."}),
        ];
        let raw = body("claude-sonnet-4-5", Some("high"), "w1", "t1", &history, TurnKind::Node, None).unwrap();
        let v: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["thinking"], json!({"type":"enabled","budget_tokens":16384}));
        assert_eq!(v["max_tokens"], 32768);
        assert_eq!(v["stream"], true);
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"].as_array().unwrap().len(), 2);
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);

        // When extended thinking is disabled (`effort = None`), tool turns replay as `tool_use` + `tool_result`.
        let native_raw = body("claude-sonnet-4-5", None, "w1", "t1", &history, TurnKind::Node, None).unwrap();
        let native_v: Value = serde_json::from_slice(&native_raw).unwrap();
        let native_msgs = native_v["messages"].as_array().unwrap();
        assert_eq!(native_msgs.len(), 3);
        assert_eq!(native_msgs[1]["role"], "assistant");
        assert_eq!(
            native_msgs[1]["content"],
            json!([{"type":"tool_use","id":"call_1","name":"read_file","input":{"path":"a"}}])
        );
        assert_eq!(native_msgs[2]["role"], "user");
        assert_eq!(
            native_msgs[2]["content"],
            json!([
                {"type":"tool_result","tool_use_id":"call_1","content":history[2]["content"]},
                {"type":"text","text":"Continue task t1."}
            ])
        );

        let none_raw = body("claude-sonnet-4-5", Some("none"), "w1", "t1", &[], TurnKind::Root, Some(4096)).unwrap();
        let none_v: Value = serde_json::from_slice(&none_raw).unwrap();
        assert_eq!(none_v["thinking"], json!({"type":"disabled"}));
        assert_eq!(none_v["max_tokens"], 4096);
        assert_eq!(none_v["tool_choice"], json!({"type":"any","disable_parallel_tool_use":true}));
        assert_eq!(v["tool_choice"], json!({"type":"auto","disable_parallel_tool_use":true}));
    }

    #[test]
    fn decodes_thinking_and_text_sse_as_well_as_json_and_errors() {
        let sse = frames(&[
            json!({"type":"message_start","message":{"id":"m1","type":"message","role":"assistant","stop_reason":null}}),
            json!({"type":"ping"}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"{\"op\":\"wrong\"}"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":r#"{"op":"finish","#}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":r#""text":"anthropic ok"}"#}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
            json!({"type":"message_stop"}),
        ]);
        let decoded = decode_sse(&sse, TurnKind::Root).ok().unwrap();
        assert_eq!(decoded, json!({"op":"finish","text":"anthropic ok"}));

        let sse_tool = frames(&[
            json!({"type":"message_start","message":{"id":"m2","type":"message","role":"assistant","stop_reason":null}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"planning"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tu1","name":"edit_file","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":r#"{"path":"doc.md","#}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":r#""old":"a","new":"b"}"#}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(
            decode_sse(&sse_tool, TurnKind::Node).ok().unwrap(),
            json!({"op":"edit_file","path":"doc.md","old":"a","new":"b"})
        );

        let sse_zero_arg = frames(&[
            json!({"type":"message_start","message":{"id":"m3","type":"message","role":"assistant","stop_reason":null}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu0","name":"members","input":{}}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(decode_sse(&sse_zero_arg, TurnKind::Root).ok().unwrap(), json!({"op":"members"}));

        let tool_json_valid = json!({
            "type": "message",
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [{"type":"tool_use","id":"t1","name":"finish","input":{"text":"json tool ok"}}]
        })
        .to_string();
        assert_eq!(
            decode_json_response(tool_json_valid.as_bytes(), TurnKind::Node).unwrap(),
            json!({"op":"finish","text":"json tool ok"})
        );

        let tool_json = json!({
            "type": "message",
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [{"type":"tool_use","id":"t1","name":"bash","input":{}}]
        })
        .to_string();
        assert!(matches!(
            decode_json_response(tool_json.as_bytes(), TurnKind::Node),
            Err(ModelError::InvalidAction(_))
        ));

        let multi_tool_json = json!({
            "type": "message",
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [
                {"type":"tool_use","id":"t1","name":"members","input":{}},
                {"type":"tool_use","id":"t2","name":"finish","input":{"text":"x"}}
            ]
        })
        .to_string();
        assert!(matches!(
            decode_json_response(multi_tool_json.as_bytes(), TurnKind::Node),
            Err(ModelError::InvalidAction(ref msg)) if msg.contains("exactly one tool call")
        ));

        let truncated_json = json!({
            "type": "message",
            "role": "assistant",
            "stop_reason": "max_tokens",
            "content": [{"type":"text","text":r#"{"op":"finish","text":"cut"#}]
        })
        .to_string();
        assert!(matches!(decode_json_response(truncated_json.as_bytes(), TurnKind::Node), Err(ModelError::Fatal(_))));
    }

    #[test]
    fn retries_529_overloaded_and_surfaces_quota_and_overflow() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for idx in 0..4 {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(&s);
                let line = r.by_ref().lines().next().unwrap().unwrap();
                assert_eq!(line, "POST /v1/messages HTTP/1.1");
                let mut saw_key = false;
                let mut saw_ver = false;
                let mut len = 0usize;
                for h in r.by_ref().lines() {
                    let h = h.unwrap();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        if k.eq_ignore_ascii_case("x-api-key") && v.trim() == "ant-key" {
                            saw_key = true;
                        }
                        if k.eq_ignore_ascii_case("anthropic-version") && v.trim() == ANTHROPIC_VERSION {
                            saw_ver = true;
                        }
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap();
                        }
                    }
                }
                assert!(saw_key && saw_ver);
                let mut req_body = vec![0; len];
                r.read_exact(&mut req_body).unwrap();
                match idx {
                    0 => {
                        let overloaded =
                            json!({"type":"error","error":{"type":"overloaded_error","message":"busy"}}).to_string();
                        write!(
                            s,
                            "HTTP/1.1 529 Overloaded\r\nRetry-After: 0\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{overloaded}",
                            overloaded.len()
                        )
                        .unwrap();
                    }
                    1 => {
                        let ok = json!({
                            "type":"message","role":"assistant","stop_reason":"end_turn",
                            "content":[{"type":"text","text":r#"{"op":"members"}"#}]
                        })
                        .to_string();
                        write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ok}",
                            ok.len()
                        )
                        .unwrap();
                    }
                    2 => {
                        let bill = json!({"type":"error","error":{"type":"billing_error","message":"Your credit balance is too low (secret)"}}).to_string();
                        write!(
                            s,
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bill}",
                            bill.len()
                        )
                        .unwrap();
                    }
                    _ => {
                        let over = json!({"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}).to_string();
                        write!(
                            s,
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{over}",
                            over.len()
                        )
                        .unwrap();
                    }
                }
            }
        });
        let got = transact_with_retry(
            &base,
            Some("ant-key"),
            Duration::from_secs(3),
            b"{}",
            TurnKind::Node,
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(got, json!({"op":"members"}));
        let bill_err = transact_with_retry(
            &base,
            Some("ant-key"),
            Duration::from_secs(3),
            b"{}",
            TurnKind::Node,
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(matches!(bill_err, ModelError::QuotaExceeded(_)) && !bill_err.to_string().contains("secret"));
        let over_err = transact_with_retry(
            &base,
            Some("ant-key"),
            Duration::from_secs(3),
            b"{}",
            TurnKind::Node,
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(matches!(over_err, ModelError::NeedsCompaction(_)));
        server.join().unwrap();
    }
}
