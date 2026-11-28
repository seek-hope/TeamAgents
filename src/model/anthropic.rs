//! Anthropic Messages API transport (`TEAMAGENTS_PROVIDER=anthropic`).
use super::{
    checked_post_request, decode_action, decode_tool_call_str, decode_tool_call_value, is_context_overflow_payload,
    is_quota_exceeded_payload, is_retryable_status, needs_leading_assignment, note_rate_limit, pack_action_meta,
    parse_history, read_stream_with_lines, request_limit, retry_delay, send_with_redirects, system_prompt, tool_specs,
    validate_action_batch, wait_rate_limit, AuthHeader, HistoryItem, ModelAction, ModelError, ModelOutput,
    ModelSettings, StreamCallback, StreamEvent, UsageStats, HTTP_MAX_ATTEMPTS, HTTP_RETRY_BASE, LIMIT, STREAM_LIMIT,
    TIMEOUT,
};
use crate::{protocol::TurnKind, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{env, io::Read, thread, time::Duration};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const INVALID: &str = "invalid or ambiguous Anthropic response; no action accepted";
const TEXT_ONLY: &str = "Anthropic must return a single assistant tool call or JSON action without refusals";

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

pub(super) fn configured_max_tokens() -> Result<Option<u32>> {
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
    let mut groups: Vec<(&str, Vec<Value>)> = if needs_leading_assignment(kind, history) {
        vec![("user", vec![json!({"type": "text", "text": assignment})])]
    } else {
        Vec::new()
    };
    let items = parse_history(history, &mut size, max_bytes, too_big)?;

    for item in items {
        match item {
            HistoryItem::ToolTurn { call_id, name, raw_assistant, args_value, output, meta, .. } => {
                let signed_thinking: Vec<Value> = meta
                    .and_then(|m| m.get("thinking_blocks"))
                    .and_then(Value::as_array)
                    .filter(|arr| !arr.is_empty())
                    .cloned()
                    .unwrap_or_default();
                if !thinking_enabled || !signed_thinking.is_empty() {
                    for tb in signed_thinking {
                        push_block(&mut groups, "assistant", tb);
                    }
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
            HistoryItem::ToolBatch { calls, meta } => {
                let signed_thinking: Vec<Value> = meta
                    .and_then(|m| m.get("thinking_blocks"))
                    .and_then(Value::as_array)
                    .filter(|arr| !arr.is_empty())
                    .cloned()
                    .unwrap_or_default();
                if !thinking_enabled || !signed_thinking.is_empty() {
                    for tb in signed_thinking {
                        push_block(&mut groups, "assistant", tb);
                    }
                    for call in &calls {
                        push_block(
                            &mut groups,
                            "assistant",
                            json!({"type": "tool_use", "id": call.call_id, "name": call.name, "input": call.args_value}),
                        );
                    }
                    for call in &calls {
                        push_block(
                            &mut groups,
                            "user",
                            json!({"type": "tool_result", "tool_use_id": call.call_id, "content": call.output}),
                        );
                    }
                } else {
                    for call in &calls {
                        push_block(&mut groups, "assistant", json!({"type": "text", "text": call.raw_assistant}));
                        push_block(&mut groups, "user", json!({"type": "text", "text": call.output}));
                    }
                }
            }
            HistoryItem::Message { role, content, meta } => {
                if role == "assistant" && thinking_enabled {
                    if let Some(tbs) = meta.and_then(|m| m.get("thinking_blocks")).and_then(Value::as_array) {
                        for tb in tbs {
                            push_block(&mut groups, "assistant", tb.clone());
                        }
                    }
                }
                if !content.is_empty() {
                    push_block(&mut groups, role, json!({"type": "text", "text": content}));
                } else if !groups.last().is_some_and(|(r, _)| *r == role) {
                    groups.push((role, Vec::new()));
                }
            }
        }
    }

    let mut messages: Vec<Value> = groups
        .into_iter()
        .map(|(role, mut blocks)| {
            if blocks.is_empty() {
                blocks.push(json!({"type": "text", "text": "(empty)"}));
            }
            json!({"role": role, "content": blocks})
        })
        .collect();
    // Rolling cache breakpoint on the final content block keeps multi-turn prefixes cached.
    if let Some(last_block) =
        messages.last_mut().and_then(|m| m.get_mut("content")).and_then(Value::as_array_mut).and_then(|c| c.last_mut())
    {
        last_block["cache_control"] = json!({"type": "ephemeral"});
    }

    let mut tools: Vec<Value> = tool_specs(kind)
        .into_iter()
        .map(|s| json!({"name": s.name, "description": s.description, "input_schema": s.parameters}))
        .collect();
    if let Some(last_tool) = tools.last_mut() {
        last_tool["cache_control"] = json!({"type": "ephemeral"});
    }
    let system_blocks = vec![json!({
        "type": "text",
        "text": system_prompt(kind),
        "cache_control": {"type": "ephemeral"}
    })];
    let mut tool_choice = json!({
        "type": if thinking_enabled { "auto" } else { "any" },
    });
    if !matches!(kind, TurnKind::Root | TurnKind::Node) {
        tool_choice["disable_parallel_tool_use"] = json!(true);
    }
    let mut payload = json!({
        "model": model,
        "system": system_blocks,
        "messages": messages,
        "tools": tools,
        "tool_choice": tool_choice,
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
    id: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    data: Option<String>,
    name: Option<String>,
    input: Option<Value>,
}

#[derive(Deserialize)]
struct Delta {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    thinking: Option<String>,
    signature: Option<String>,
    partial_json: Option<String>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct StartMessage {
    #[serde(rename = "type")]
    kind: Option<String>,
    role: Option<String>,
    stop_reason: Option<String>,
    usage: Option<Value>,
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
    usage: Option<Value>,
    error: Option<ApiError>,
}

enum BlockKind {
    Text,
    Thinking { kind: String, thinking: String, signature: Option<String>, data: Option<String> },
    ToolUse { id: Option<String>, name: String, input_json: String, initial_input: Option<Value> },
}

fn parse_anthropic_usage(usage: Option<&Value>) -> Option<UsageStats> {
    let u = usage?.as_object()?;
    let base = u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let cached = u.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let created = u.get("cache_creation_input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output_tokens = u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    Some(UsageStats {
        input_tokens: base.saturating_add(cached).saturating_add(created),
        cached_tokens: cached,
        output_tokens,
    })
}

fn merge_anthropic_usage(acc: &mut Option<Value>, patch: Option<Value>) {
    let Some(Value::Object(patch_map)) = patch else { return };
    match acc {
        Some(Value::Object(base)) => {
            for (k, v) in patch_map {
                base.insert(k, v);
            }
        }
        _ => *acc = Some(Value::Object(patch_map)),
    }
}

fn build_anthropic_batch_meta(thinking_blocks: Vec<Value>, actions: &[ModelAction]) -> Option<Value> {
    let mut map = serde_json::Map::new();
    if !thinking_blocks.is_empty() {
        map.insert("thinking_blocks".into(), Value::Array(thinking_blocks));
    }
    pack_action_meta(actions, map)
}

fn finish_anthropic_output(
    actions: Vec<ModelAction>,
    kind: TurnKind,
    thinking_blocks: Vec<Value>,
    commentary: Option<String>,
    reasoning_summary: Option<String>,
    usage: Option<UsageStats>,
) -> std::result::Result<ModelOutput, ModelError> {
    let actions = validate_action_batch(actions, kind)?;
    let meta = build_anthropic_batch_meta(thinking_blocks, &actions);
    Ok(ModelOutput { actions, commentary, reasoning_summary, meta, usage })
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

fn decode_sse(stream: &str, kind: TurnKind) -> std::result::Result<ModelOutput, SseDecodeErr> {
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
    let mut raw_usage: Option<Value> = None;

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
                merge_anthropic_usage(&mut raw_usage, msg.usage);
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
                        blocks.push((
                            BlockKind::Thinking {
                                kind: cb.kind,
                                thinking: cb.thinking.unwrap_or_default(),
                                signature: cb.signature,
                                data: cb.data,
                            },
                            true,
                        ));
                    }
                    "tool_use" => {
                        let tool_name = cb
                            .name
                            .filter(|n| !n.is_empty())
                            .ok_or_else(|| SseDecodeErr::Model(ModelError::InvalidAction(TEXT_ONLY.into())))?;
                        blocks.push((
                            BlockKind::ToolUse {
                                id: cb.id,
                                name: tool_name,
                                input_json: String::new(),
                                initial_input: cb.input,
                            },
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
                    (BlockKind::Thinking { thinking, .. }, "thinking_delta") => {
                        if let Some(piece) = delta.thinking.as_deref() {
                            if thinking.len().saturating_add(piece.len()) <= LIMIT {
                                thinking.push_str(piece);
                            }
                        }
                    }
                    (BlockKind::Thinking { signature, .. }, "signature_delta") => {
                        if let Some(piece) = delta.signature.as_deref() {
                            signature.get_or_insert_with(String::new).push_str(piece);
                        }
                    }
                    (BlockKind::Text, "citations_delta") => {}
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
                merge_anthropic_usage(&mut raw_usage, ev.usage);
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
    let mut thinking_blocks = Vec::new();
    let mut thinking_texts = Vec::new();
    let mut tool_uses = Vec::new();
    for (bk, _) in blocks {
        match bk {
            BlockKind::Thinking { kind: bk_kind, thinking, signature, data } => {
                if bk_kind == "thinking" {
                    let trimmed = thinking.trim();
                    if !trimmed.is_empty() {
                        thinking_texts.push(trimmed.to_owned());
                    }
                    if let Some(sig) = signature.filter(|s| !s.is_empty()) {
                        thinking_blocks.push(json!({"type": "thinking", "thinking": thinking, "signature": sig}));
                    }
                } else if let Some(d) = data.filter(|s| !s.is_empty()) {
                    thinking_blocks.push(json!({"type": "redacted_thinking", "data": d}));
                }
            }
            BlockKind::ToolUse { id, name, input_json, initial_input } => {
                tool_uses.push((id, name, input_json, initial_input));
            }
            BlockKind::Text => {}
        }
    }
    let usage = parse_anthropic_usage(raw_usage.as_ref());
    let reasoning_summary = (!thinking_texts.is_empty()).then(|| thinking_texts.join("\n"));
    if !tool_uses.is_empty() {
        let mut actions = Vec::with_capacity(tool_uses.len());
        for (tid, tname, input_json, initial_input) in tool_uses {
            let has_raw = !input_json.trim().is_empty();
            let action = if has_raw {
                decode_tool_call_str(&tname, &input_json, kind)
            } else {
                decode_tool_call_value(&tname, initial_input.unwrap_or_else(|| json!({})), kind)
            }
            .map_err(SseDecodeErr::Model)?;
            actions.push(ModelAction {
                action,
                call_id: tid,
                fc_id: None,
                raw_arguments: has_raw.then_some(input_json),
            });
        }
        let commentary = (!text.trim().is_empty()).then(|| text.trim().to_owned());
        return finish_anthropic_output(actions, kind, thinking_blocks, commentary, reasoning_summary, usage)
            .map_err(SseDecodeErr::Model);
    }
    if !saw_text_block {
        return Err(SseDecodeErr::Model(ModelError::InvalidAction(TEXT_ONLY.into())));
    }
    let action = decode_action(&text, kind).map_err(SseDecodeErr::Model)?;
    let meta = build_anthropic_batch_meta(thinking_blocks, &[]);
    Ok(ModelOutput::single(action, meta, usage).with_text(None, reasoning_summary))
}

fn redacted_error(
    status: Option<u16>,
    err_type: Option<&str>,
    kind: TurnKind,
    body: &[u8],
    key_env: &str,
) -> ModelError {
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
    let detail: String = match err_type {
        Some("authentication_error") => format!("authentication failed (check {key_env})"),
        Some("permission_error") => "access forbidden for this model or endpoint".into(),
        Some("rate_limit_error") => "rate limit exceeded; retry later".into(),
        Some("overloaded_error") => "service overloaded; retry later".into(),
        Some("invalid_request_error") => "request rejected (check context size, model ID and thinking budget)".into(),
        Some("not_found_error") => "model or endpoint not found".into(),
        _ => match status {
            Some(401) => format!("authentication failed (check {key_env})"),
            Some(403) => "access forbidden for this model or endpoint".into(),
            Some(429) => "rate limit exceeded; retry later".into(),
            Some(400 | 404 | 422) => "request rejected (check context size, model ID and reasoning effort)".into(),
            Some(500..=599) => "service unavailable; retry later".into(),
            _ => "request failed; no action accepted".into(),
        },
    };
    match status {
        Some(s) => ModelError::Fatal(format!("Anthropic HTTP {s}: {detail}")),
        None => ModelError::Fatal(format!("Anthropic inference failed: {detail}")),
    }
}

pub(super) fn decode_json_response_for(
    bytes: &[u8],
    kind: TurnKind,
    key_env: &str,
) -> std::result::Result<ModelOutput, ModelError> {
    let raw: Value = serde_json::from_slice(bytes).map_err(|_| ModelError::Fatal(INVALID.into()))?;
    if raw["type"] == "error" || raw.get("error").is_some_and(|e| !e.is_null()) {
        let err_type = raw.pointer("/error/type").and_then(Value::as_str);
        return Err(redacted_error(Some(200), err_type, kind, bytes, key_env));
    }
    if raw["type"] != "message" || raw["role"] != "assistant" {
        return Err(ModelError::Fatal(INVALID.into()));
    }
    check_stop_reason(raw["stop_reason"].as_str())?;
    let usage = parse_anthropic_usage(raw.get("usage"));
    let content = raw["content"].as_array().ok_or_else(|| ModelError::Fatal(INVALID.into()))?;
    let mut text = String::new();
    let mut saw_text = false;
    let mut thinking_blocks = Vec::new();
    let mut thinking_texts = Vec::new();
    let mut tool_uses = Vec::new();
    for block in content {
        match block["type"].as_str() {
            Some("thinking") => {
                if let Some(t) = block.get("thinking").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
                {
                    thinking_texts.push(t.to_owned());
                }
                if block.get("signature").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                    thinking_blocks.push(block.clone());
                }
            }
            Some("redacted_thinking") => {
                if block.get("data").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
                    thinking_blocks.push(block.clone());
                }
            }
            Some("text") => {
                let piece = block["text"].as_str().ok_or_else(|| ModelError::InvalidAction(TEXT_ONLY.into()))?;
                if text.len().saturating_add(piece.len()) > LIMIT {
                    return Err(ModelError::Fatal("Anthropic response exceeds 2 MiB; no action accepted".into()));
                }
                text.push_str(piece);
                saw_text = true;
            }
            Some("tool_use") => {
                let id = block.get("id").and_then(Value::as_str).map(str::to_owned);
                let name = block["name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| ModelError::InvalidAction(TEXT_ONLY.into()))?
                    .to_owned();
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_uses.push((id, name, input));
            }
            _ => return Err(ModelError::InvalidAction(TEXT_ONLY.into())),
        }
    }
    let reasoning_summary = (!thinking_texts.is_empty()).then(|| thinking_texts.join("\n"));
    if !tool_uses.is_empty() {
        let mut actions = Vec::with_capacity(tool_uses.len());
        for (tid, name, input) in tool_uses {
            let action = decode_tool_call_value(&name, input, kind)?;
            actions.push(ModelAction { action, call_id: tid, fc_id: None, raw_arguments: None });
        }
        let commentary = (!text.trim().is_empty()).then(|| text.trim().to_owned());
        return finish_anthropic_output(actions, kind, thinking_blocks, commentary, reasoning_summary, usage);
    }
    if !saw_text {
        return Err(ModelError::InvalidAction(TEXT_ONLY.into()));
    }
    let action = decode_action(&text, kind)?;
    let meta = build_anthropic_batch_meta(thinking_blocks, &[]);
    Ok(ModelOutput::single(action, meta, usage).with_text(None, reasoning_summary))
}

#[cfg(test)]
pub(super) fn decode_json_response(bytes: &[u8], kind: TurnKind) -> std::result::Result<ModelOutput, ModelError> {
    decode_json_response_for(bytes, kind, "ANTHROPIC_API_KEY")
}

enum RetryErr {
    Retryable(Duration),
    Done(ModelError),
}

fn emit_anthropic_stream_line(line: &str, on_stream: Option<StreamCallback<'_>>) {
    let Some(cb) = on_stream else {
        return;
    };
    let Some(raw) = line.strip_prefix("data:") else {
        return;
    };
    let trimmed = raw.strip_prefix(' ').unwrap_or(raw).trim();
    if trimmed.is_empty() || trimmed == "[DONE]" {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(trimmed) else {
        return;
    };
    match v["type"].as_str() {
        Some("content_block_start") => {
            if v.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                if let Some(name) = v.pointer("/content_block/name").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    cb(StreamEvent::ToolCallStarted(name.to_owned()));
                }
            }
        }
        Some("content_block_delta") => match v.pointer("/delta/type").and_then(Value::as_str) {
            Some("thinking_delta") => {
                if let Some(t) = v.pointer("/delta/thinking").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    cb(StreamEvent::ReasoningDelta(t.to_owned()));
                }
            }
            Some("text_delta") => {
                if let Some(t) = v.pointer("/delta/text").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    cb(StreamEvent::CommentaryDelta(t.to_owned()));
                }
            }
            _ => {}
        },
        _ => {}
    }
}

fn handle_outcome(
    outcome: std::result::Result<ureq::Response, ureq::Error>,
    kind: TurnKind,
    attempt: usize,
    retry_base: Duration,
    key_env: &str,
    on_stream: Option<StreamCallback<'_>>,
) -> std::result::Result<ModelOutput, RetryErr> {
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
            return Err(RetryErr::Done(redacted_error(Some(status), err_type, kind, &err_buf, key_env)));
        }
        Err(ureq::Error::Transport(_)) => {
            if attempt < HTTP_MAX_ATTEMPTS {
                return Err(RetryErr::Retryable(retry_delay(attempt, None, retry_base)));
            }
            return Err(RetryErr::Done(ModelError::Fatal("Anthropic transport failed".into())));
        }
    };

    let (media, raw) = match read_stream_with_lines(resp, STREAM_LIMIT, |line| {
        emit_anthropic_stream_line(line, on_stream);
    }) {
        Ok(pair) => pair,
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
                    RetryErr::Done(redacted_error(None, err_type.as_deref(), kind, raw_json.as_bytes(), key_env))
                }
            });
        }
        if media.as_deref() == Some("application/json") || text.trim_start().starts_with('{') {
            return decode_json_response_for(&raw, kind, key_env).map_err(RetryErr::Done);
        }
    }
    Err(RetryErr::Done(ModelError::Fatal("Anthropic HTTP 200: expected event stream or JSON response".into())))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn transact_with_retry_for(
    base: &str,
    key: Option<&str>,
    timeout: Duration,
    bytes: &[u8],
    kind: TurnKind,
    retry_base: Duration,
    env_names: (&str, &str),
    on_stream: Option<StreamCallback<'_>>,
) -> std::result::Result<ModelOutput, ModelError> {
    let (url_env, key_env) = env_names;
    let initial = messages_endpoint(base);
    for attempt in 1..=HTTP_MAX_ATTEMPTS {
        wait_rate_limit(retry_base);
        let mut first_hop = true;
        let outcome = send_with_redirects(&initial, Some(bytes), |url, send_auth| {
            let allow_query = !first_hop;
            first_hop = false;
            Ok(checked_post_request(url, key, send_auth, timeout, allow_query, url_env, key_env, AuthHeader::XApiKey)?
                .set("anthropic-version", ANTHROPIC_VERSION)
                .set("Accept", "text/event-stream, application/json"))
        })
        .map_err(ModelError::Fatal)?;
        match handle_outcome(outcome, kind, attempt, retry_base, key_env, on_stream) {
            Ok(out) => return Ok(out),
            Err(RetryErr::Retryable(delay)) => thread::sleep(delay),
            Err(RetryErr::Done(err)) => return Err(err),
        }
    }
    unreachable!("loop always returns within HTTP_MAX_ATTEMPTS")
}

#[cfg(test)]
pub(super) fn transact_with_retry(
    base: &str,
    key: Option<&str>,
    timeout: Duration,
    bytes: &[u8],
    kind: TurnKind,
    retry_base: Duration,
) -> std::result::Result<ModelOutput, ModelError> {
    transact_with_retry_for(
        base,
        key,
        timeout,
        bytes,
        kind,
        retry_base,
        ("ANTHROPIC_BASE_URL", "ANTHROPIC_API_KEY"),
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn complete_for(
    base: &str,
    key: Option<&str>,
    settings: &ModelSettings,
    agent: &str,
    task: &str,
    history: &[Value],
    kind: TurnKind,
    env_names: (&str, &str),
    max_override: Option<u32>,
    on_stream: Option<StreamCallback<'_>>,
) -> std::result::Result<ModelOutput, ModelError> {
    let model = settings.model.as_deref().ok_or_else(|| {
        ModelError::Fatal("set TEAMAGENTS_MODEL or select an Anthropic model with /model MODEL".into())
    })?;
    let bytes =
        body(model, settings.effort.as_deref(), agent, task, history, kind, max_override).map_err(ModelError::Fatal)?;
    transact_with_retry_for(base, key, TIMEOUT, &bytes, kind, HTTP_RETRY_BASE, env_names, on_stream)
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
        assert_eq!(msgs[0]["content"].as_array().unwrap().len(), 1);
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "text");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"].as_array().unwrap().len(), 2);
        assert_eq!(msgs[2]["content"][1]["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(v["system"][0]["cache_control"], json!({"type":"ephemeral"}));
        assert_eq!(v["tools"].as_array().unwrap().last().unwrap()["cache_control"], json!({"type":"ephemeral"}));

        // Signed thinking_blocks persisted in meta replay natively even with extended thinking enabled.
        let signed_history = [
            json!({"role":"user","content":"extra context"}),
            json!({
                "role":"assistant",
                "content":r#"{"op":"read_file","path":"a"}"#,
                "meta":{
                    "call_id":"tu_signed",
                    "thinking_blocks":[{"type":"thinking","thinking":"plan","signature":"sig123"}]
                }
            }),
            json!({"role":"user","content":r#"{"tool":"read_file","ok":true,"result":"data"}"#}),
        ];
        let signed_raw =
            body("claude-sonnet-4-5", Some("high"), "w1", "t1", &signed_history, TurnKind::Node, None).unwrap();
        let signed_v: Value = serde_json::from_slice(&signed_raw).unwrap();
        assert_eq!(
            signed_v["messages"][1]["content"],
            json!([
                {"type":"thinking","thinking":"plan","signature":"sig123"},
                {"type":"tool_use","id":"tu_signed","name":"read_file","input":{"path":"a"}}
            ])
        );

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
                {"type":"text","text":"Continue task t1.","cache_control":{"type":"ephemeral"}}
            ])
        );

        let none_raw = body("claude-sonnet-4-5", Some("none"), "w1", "t1", &[], TurnKind::Root, Some(4096)).unwrap();
        let none_v: Value = serde_json::from_slice(&none_raw).unwrap();
        assert_eq!(none_v["thinking"], json!({"type":"disabled"}));
        assert_eq!(none_v["max_tokens"], 4096);
        assert_eq!(none_v["tool_choice"], json!({"type":"any"}));
        assert_eq!(v["tool_choice"], json!({"type":"auto"}));
        let legacy_raw = body("claude-sonnet-4-5", None, "w1", "t1", &[], TurnKind::Legacy, Some(4096)).unwrap();
        let legacy_v: Value = serde_json::from_slice(&legacy_raw).unwrap();
        assert_eq!(legacy_v["tool_choice"], json!({"type":"any","disable_parallel_tool_use":true}));
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
        assert_eq!(decoded.primary_action(), &json!({"op":"finish","text":"anthropic ok"}));
        assert_eq!(decoded.meta.as_ref().unwrap()["thinking_blocks"][0]["signature"], "sig");

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
            decode_sse(&sse_tool, TurnKind::Node).ok().unwrap().primary_action(),
            &json!({"op":"edit_file","path":"doc.md","old":"a","new":"b"})
        );

        let sse_zero_arg = frames(&[
            json!({"type":"message_start","message":{"id":"m3","type":"message","role":"assistant","stop_reason":null}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tu0","name":"members","input":{}}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
            json!({"type":"message_stop"}),
        ]);
        assert_eq!(decode_sse(&sse_zero_arg, TurnKind::Root).ok().unwrap().primary_action(), &json!({"op":"members"}));

        let tool_json_valid = json!({
            "type": "message",
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [{"type":"tool_use","id":"t1","name":"finish","input":{"text":"json tool ok"}}]
        })
        .to_string();
        assert_eq!(
            decode_json_response(tool_json_valid.as_bytes(), TurnKind::Node).unwrap().primary_action(),
            &json!({"op":"finish","text":"json tool ok"})
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

        let multi_solo_json = json!({
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
            decode_json_response(multi_solo_json.as_bytes(), TurnKind::Legacy),
            Err(ModelError::InvalidAction(ref msg)) if msg.contains("exactly one tool call")
        ));
        assert!(matches!(
            decode_json_response(multi_solo_json.as_bytes(), TurnKind::Node),
            Err(ModelError::InvalidAction(ref msg)) if msg.contains("solo control-flow")
        ));
        let parallel_json = json!({
            "type": "message",
            "role": "assistant",
            "stop_reason": "tool_use",
            "content": [
                {"type":"thinking","thinking":"Inspect both","signature":"sig1"},
                {"type":"text","text":"Running two reads"},
                {"type":"tool_use","id":"t1","name":"read_file","input":{"path":"a.rs"}},
                {"type":"tool_use","id":"t2","name":"list_dir","input":{"path":"src"}}
            ]
        })
        .to_string();
        let parallel_out = decode_json_response(parallel_json.as_bytes(), TurnKind::Node).unwrap();
        assert_eq!(parallel_out.actions.len(), 2);
        assert_eq!(parallel_out.commentary.as_deref(), Some("Running two reads"));
        assert_eq!(parallel_out.reasoning_summary.as_deref(), Some("Inspect both"));
        assert_eq!(parallel_out.meta.as_ref().unwrap()["calls"].as_array().unwrap().len(), 2);

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
        assert_eq!(got.primary_action(), &json!({"op":"members"}));
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
