//! OpenAI Responses API transport (`TEAMAGENTS_PROVIDER=responses`) and shared strict SSE/JSON decoder.
use super::{
    body, checked_post_request, decode_action, decode_tool_call_str, is_context_overflow_payload,
    is_quota_exceeded_payload, is_retryable_status, note_rate_limit, retry_delay, send_with_redirects, wait_rate_limit,
    AuthHeader, ModelError, ModelSettings, Provider, HTTP_MAX_ATTEMPTS, HTTP_RETRY_BASE, LIMIT, STREAM_LIMIT, TIMEOUT,
};
use crate::protocol::TurnKind;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashSet, io::Read, thread, time::Duration};

const INVALID: &str = "invalid or ambiguous Responses event stream; no action accepted";
const TEXT_ONLY: &str = "Responses must return a single final assistant action tool call or JSON text without refusals";
const MULTI_TOOL: &str = "expected exactly one tool call per turn; emit one action at a time";

// Typed wire fields reject duplicates. Unknown metadata is never interpreted as an action.
#[derive(Deserialize)]
pub(super) struct Part {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Item {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: Option<String>,
    pub role: Option<String>,
    pub phase: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
    #[allow(dead_code)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub content: Vec<Part>,
}

impl Item {
    pub fn is_final_message(&self) -> bool {
        self.kind == "message"
            && self.role.as_deref() == Some("assistant")
            && matches!(self.phase.as_deref(), None | Some("final_answer"))
    }

    pub fn is_commentary(&self) -> bool {
        self.kind == "message"
            && self.role.as_deref() == Some("assistant")
            && self.phase.as_deref() == Some("commentary")
    }

    pub fn is_function_call(&self) -> bool {
        self.kind == "function_call"
            && matches!(self.status.as_deref(), None | Some("completed"))
            && self.name.as_deref().is_some_and(|n| !n.is_empty())
            && self.arguments.is_some()
    }

    pub fn has_refusal(&self) -> bool {
        self.content.iter().any(|p| p.kind == "refusal")
    }

    pub fn output_texts(&self) -> std::result::Result<Vec<&str>, &'static str> {
        if !self.is_final_message() || self.status.as_deref() != Some("completed") || self.content.is_empty() {
            return Err("text_only");
        }
        let mut texts = Vec::with_capacity(self.content.len());
        for part in &self.content {
            if part.kind != "output_text" {
                return Err("text_only");
            }
            texts.push(part.text.as_deref().ok_or("text_only")?);
        }
        Ok(texts)
    }
}

#[derive(Deserialize)]
pub(super) struct Failure {
    pub code: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct Response {
    pub id: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub output: Vec<Item>,
    pub error: Option<Failure>,
    pub incomplete_details: Option<serde::de::IgnoredAny>,
}

#[derive(Deserialize)]
pub(super) struct Event {
    #[serde(rename = "type")]
    pub kind: String,
    pub sequence_number: Option<u64>,
    pub response: Option<Response>,
    pub response_id: Option<String>,
    pub output_index: Option<usize>,
    pub content_index: Option<usize>,
    pub item_id: Option<String>,
    pub item: Option<Item>,
    pub part: Option<Part>,
    pub delta: Option<String>,
    pub text: Option<String>,
    pub arguments: Option<String>,
    pub code: Option<String>,
    pub error: Option<Failure>,
}

pub(super) enum SelectedAction {
    ToolCall(usize),
    FinalMessage(usize),
}

pub(super) enum FinalActionErr {
    Invalid,
    TextOnly,
    MultiTool,
}

pub(super) fn select_action(response: &Response) -> std::result::Result<SelectedAction, FinalActionErr> {
    if response.status.as_deref() != Some("completed")
        || response.id.as_deref().is_none_or(str::is_empty)
        || response.error.is_some()
        || response.incomplete_details.is_some()
    {
        return Err(FinalActionErr::Invalid);
    }
    let mut ids = HashSet::new();
    let mut function_calls = Vec::new();
    let mut final_messages = Vec::new();
    let mut contract_error = false;
    for (idx, item) in response.output.iter().enumerate() {
        if item.id.is_empty() || !ids.insert(&item.id) {
            return Err(FinalActionErr::Invalid);
        }
        if item.status.as_deref() == Some("incomplete") {
            return Err(FinalActionErr::Invalid);
        }
        if item.kind == "reasoning" {
            if !matches!(item.status.as_deref(), None | Some("completed")) {
                return Err(FinalActionErr::Invalid);
            }
            continue;
        }
        if item.is_commentary() {
            if item.has_refusal() {
                contract_error = true;
            } else if !matches!(item.status.as_deref(), None | Some("completed")) {
                return Err(FinalActionErr::Invalid);
            }
            continue;
        }
        if item.kind == "function_call" {
            if item.is_function_call() {
                function_calls.push(idx);
            } else {
                contract_error = true;
            }
            continue;
        }
        if item.output_texts().is_ok() {
            final_messages.push(idx);
        } else {
            contract_error = true;
        }
    }
    if contract_error {
        return Err(FinalActionErr::TextOnly);
    }
    if function_calls.len() == 1 {
        return Ok(SelectedAction::ToolCall(function_calls[0]));
    }
    if function_calls.len() > 1 {
        return Err(FinalActionErr::MultiTool);
    }
    if final_messages.len() == 1 {
        return Ok(SelectedAction::FinalMessage(final_messages[0]));
    }
    Err(FinalActionErr::TextOnly)
}

pub(super) enum StreamDecodeErr {
    Model(ModelError),
    ProviderEvent { code: Option<String>, raw_json: String },
}

fn events(stream: &str, invalid_msg: &str) -> std::result::Result<Vec<Event>, StreamDecodeErr> {
    let mut events = Vec::new();
    let mut data = String::new();
    let mut name = None;
    let mut done = false;
    let mut sequence = None;
    let inv = || StreamDecodeErr::Model(ModelError::Fatal(invalid_msg.into()));
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
                    let event: Event = serde_json::from_str(&data).map_err(|_| inv())?;
                    if name.is_some_and(|s| s != event.kind) {
                        return Err(inv());
                    }
                    if let Some(n) = event.sequence_number {
                        if sequence.is_some_and(|prev| n <= prev) {
                            return Err(inv());
                        }
                        sequence = Some(n);
                    }
                    match event.kind.as_str() {
                        "error" | "response.failed" => {
                            let failure =
                                event.response.as_ref().and_then(|r| r.error.as_ref()).or(event.error.as_ref());
                            let code = event.code.clone().or_else(|| failure.and_then(|e| e.code.clone()));
                            return Err(StreamDecodeErr::ProviderEvent { code, raw_json: data });
                        }
                        "response.incomplete" => {
                            return Err(StreamDecodeErr::Model(ModelError::Fatal(
                                "response incomplete; no action accepted".into(),
                            )));
                        }
                        _ => events.push(event),
                    }
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
    Ok(events)
}

fn concat_texts(parts: &[&str]) -> std::result::Result<String, ModelError> {
    let total: usize = parts.iter().map(|s| s.len()).sum();
    if total > LIMIT {
        return Err(ModelError::Fatal("response exceeds the size limit; no action accepted".into()));
    }
    let mut out = String::with_capacity(total);
    for p in parts {
        out.push_str(p);
    }
    Ok(out)
}

pub(super) fn decode_sse(
    stream: &str,
    kind: TurnKind,
    text_only_msg: &str,
    invalid_msg: &str,
) -> std::result::Result<Value, StreamDecodeErr> {
    let inv = || StreamDecodeErr::Model(ModelError::Fatal(invalid_msg.into()));
    let txt_err = || StreamDecodeErr::Model(ModelError::InvalidAction(text_only_msg.into()));

    let mut events = events(stream, invalid_msg)?;
    let terminal = events.pop().filter(|e| e.kind == "response.completed").ok_or_else(|| {
        StreamDecodeErr::Model(ModelError::Fatal("stream ended without response.completed; no action accepted".into()))
    })?;
    let completed = terminal.response.ok_or_else(inv)?;
    if terminal.response_id.is_some() && terminal.response_id != completed.id {
        return Err(inv());
    }
    let selected = select_action(&completed).map_err(|e| match e {
        FinalActionErr::Invalid => inv(),
        FinalActionErr::TextOnly => txt_err(),
        FinalActionErr::MultiTool => StreamDecodeErr::Model(ModelError::InvalidAction(MULTI_TOOL.into())),
    })?;

    enum Reconcile<'a> {
        Tool { name: &'a str, args: &'a str, remaining: &'a str, saw_delta: bool, idx: usize },
        Msg { parts: Vec<&'a str>, remaining: Vec<&'a str>, saw_delta: Vec<bool>, idx: usize },
    }
    let mut rec = match selected {
        SelectedAction::ToolCall(idx) => {
            let item = &completed.output[idx];
            let name = item.name.as_deref().ok_or_else(txt_err)?;
            let args = item.arguments.as_deref().ok_or_else(txt_err)?;
            if args.len() > LIMIT {
                return Err(StreamDecodeErr::Model(ModelError::Fatal(
                    "response exceeds the size limit; no action accepted".into(),
                )));
            }
            Reconcile::Tool { name, args, remaining: args, saw_delta: false, idx }
        }
        SelectedAction::FinalMessage(idx) => {
            let parts = completed.output[idx].output_texts().map_err(|_| txt_err())?;
            let total: usize = parts.iter().map(|s| s.len()).sum();
            if total > LIMIT {
                return Err(StreamDecodeErr::Model(ModelError::Fatal(
                    "response exceeds the size limit; no action accepted".into(),
                )));
            }
            let remaining = parts.clone();
            let saw_delta = vec![false; parts.len()];
            Reconcile::Msg { parts, remaining, saw_delta, idx }
        }
    };
    let mut position: Option<(usize, u8, usize, u8)> = None;

    for event in events {
        if event.response_id.is_some() && event.response_id != completed.id {
            return Err(StreamDecodeErr::Model(ModelError::Fatal("event refers to a different response ID".into())));
        }
        if matches!(event.kind.as_str(), "response.created" | "response.in_progress" | "response.queued") {
            let start = event.response.as_ref().ok_or_else(inv)?;
            if position.is_some()
                || start.id != completed.id
                || !start.output.is_empty()
                || start.error.is_some()
                || start.incomplete_details.is_some()
                || !matches!(start.status.as_deref(), Some("queued" | "in_progress"))
            {
                return Err(inv());
            }
            continue;
        }
        if event.kind == "response.metadata" {
            // Subscription streams attach metadata to the response, not to an output item.
            if event.response_id != completed.id
                || event.output_index.is_some()
                || event.item.is_some()
                || event.error.is_some()
            {
                return Err(StreamDecodeErr::Model(ModelError::Fatal(
                    "response metadata did not match the completed response".into(),
                )));
            }
            continue;
        }
        let index = event.output_index.ok_or_else(|| {
            StreamDecodeErr::Model(ModelError::Fatal(
                "stream contained an unrecognized response-level event or a missing output index".into(),
            ))
        })?;
        let item = completed.output.get(index).ok_or_else(inv)?;
        if event.item_id.as_ref().is_some_and(|id| id != &item.id) {
            return Err(inv());
        }

        let allow_narration = matches!(selected, SelectedAction::ToolCall(_)) && item.is_final_message();
        let is_non_action_message = item.is_commentary() || allow_narration;

        let (item_stage, content_idx, part_stage) = match event.kind.as_str() {
            "response.output_item.added" | "response.output_item.done" => {
                let got = event.item.as_ref().ok_or_else(inv)?;
                let added = event.kind.ends_with(".added");
                if got.id != item.id || got.kind != item.kind {
                    return Err(inv());
                }
                match &rec {
                    Reconcile::Tool { idx, name, args, .. } if index == *idx => {
                        if got.name.as_deref() != Some(*name) {
                            return Err(inv());
                        }
                        if added {
                            if !matches!(got.status.as_deref(), None | Some("in_progress")) {
                                return Err(inv());
                            }
                        } else if got.arguments.as_deref() != Some(*args)
                            || !matches!(got.status.as_deref(), None | Some("completed"))
                        {
                            return Err(inv());
                        }
                    }
                    Reconcile::Msg { idx, parts, .. } if index == *idx => {
                        if added {
                            if !got.is_final_message()
                                || (got.phase.is_some() && got.phase != item.phase)
                                || !matches!(got.status.as_deref(), None | Some("in_progress"))
                                || !got.content.is_empty()
                            {
                                return Err(inv());
                            }
                        } else if got.output_texts().map_err(|_| txt_err())? != *parts {
                            return Err(inv());
                        }
                    }
                    _ if is_non_action_message => {
                        if added {
                            if got.kind != "message"
                                || got.role.as_deref() != Some("assistant")
                                || (got.phase.is_some() && got.phase != item.phase)
                                || !matches!(got.status.as_deref(), None | Some("in_progress"))
                                || !got.content.is_empty()
                            {
                                return Err(inv());
                            }
                        } else if !(got.is_commentary() || (allow_narration && got.is_final_message()))
                            || got.status.as_deref() != Some("completed")
                            || got.has_refusal()
                        {
                            return Err(inv());
                        }
                    }
                    _ => {
                        if !matches!(got.status.as_deref(), None | Some("in_progress" | "completed")) {
                            return Err(inv());
                        }
                    }
                }
                if added {
                    (1, 0, 0)
                } else {
                    (3, usize::MAX, 6)
                }
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done"
            | "response.reasoning_summary.delta"
                if item.kind == "reasoning" && event.item_id.is_some() =>
            {
                (2, 0, 3)
            }
            _ if is_non_action_message && event.item_id.is_some() => {
                let ci = event.content_index.ok_or_else(txt_err)?;
                if ci >= item.content.len() {
                    return Err(inv());
                }
                match event.kind.as_str() {
                    "response.content_part.added" | "response.content_part.done" => {
                        let part = event.part.as_ref().ok_or_else(inv)?;
                        if part.kind != "output_text" {
                            return Err(txt_err());
                        }
                        if event.kind.ends_with(".added") {
                            if part.text.as_deref().is_some_and(|t| !t.is_empty()) {
                                return Err(inv());
                            }
                            (2, ci, 2)
                        } else {
                            (2, ci, 5)
                        }
                    }
                    "response.output_text.delta" => {
                        if event.delta.is_none() {
                            return Err(inv());
                        }
                        (2, ci, 3)
                    }
                    "response.output_text.annotation.added" => (2, ci, 3),
                    "response.output_text.done" => {
                        if event.text.is_none() {
                            return Err(inv());
                        }
                        (2, ci, 4)
                    }
                    _ => return Err(txt_err()),
                }
            }
            _ if event.item_id.is_none() => return Err(txt_err()),
            _ => match &mut rec {
                Reconcile::Tool { idx, args, remaining, saw_delta, .. } if index == *idx => match event.kind.as_str() {
                    "response.function_call_arguments.delta" => {
                        let delta = event.delta.as_deref().ok_or_else(inv)?;
                        *remaining = remaining.strip_prefix(delta).ok_or_else(inv)?;
                        *saw_delta = true;
                        (2, 0, 3)
                    }
                    "response.function_call_arguments.done" if event.arguments.as_deref() == Some(*args) => (2, 0, 4),
                    _ => return Err(txt_err()),
                },
                Reconcile::Msg { idx, parts, remaining, saw_delta } if index == *idx => {
                    let ci = event.content_index.ok_or_else(txt_err)?;
                    if ci >= parts.len() {
                        return Err(txt_err());
                    }
                    let expected = parts[ci];
                    match event.kind.as_str() {
                        "response.content_part.added" | "response.content_part.done" => {
                            let part = event.part.as_ref().ok_or_else(inv)?;
                            let added = event.kind.ends_with(".added");
                            if part.kind != "output_text"
                                || part.text.as_deref() != Some(if added { "" } else { expected })
                            {
                                return Err(txt_err());
                            }
                            if added {
                                (2, ci, 2)
                            } else {
                                (2, ci, 5)
                            }
                        }
                        "response.output_text.delta" => {
                            let delta = event.delta.as_deref().ok_or_else(inv)?;
                            remaining[ci] = remaining[ci].strip_prefix(delta).ok_or_else(inv)?;
                            saw_delta[ci] = true;
                            (2, ci, 3)
                        }
                        "response.output_text.annotation.added" => (2, ci, 3),
                        "response.output_text.done" if event.text.as_deref() == Some(expected) => (2, ci, 4),
                        _ => return Err(txt_err()),
                    }
                }
                _ => return Err(txt_err()),
            },
        };
        let next = (index, item_stage, content_idx, part_stage);
        if position.is_some_and(|last| next < last || (next == last && part_stage != 3)) {
            return Err(inv());
        }
        position = Some(next);
    }
    match rec {
        Reconcile::Tool { name, args, remaining, saw_delta, .. } => {
            if saw_delta && !remaining.is_empty() {
                return Err(inv());
            }
            decode_tool_call_str(name, args, kind).map_err(StreamDecodeErr::Model)
        }
        Reconcile::Msg { parts, remaining, saw_delta, .. } => {
            for (saw, rem) in saw_delta.into_iter().zip(remaining) {
                if saw && !rem.is_empty() {
                    return Err(inv());
                }
            }
            let combined = concat_texts(&parts).map_err(StreamDecodeErr::Model)?;
            decode_action(&combined, kind).map_err(StreamDecodeErr::Model)
        }
    }
}

fn redacted_api_error(status: Option<u16>, code: Option<&str>, kind: TurnKind) -> ModelError {
    if kind != TurnKind::Reporter && code == Some("context_length_exceeded") {
        return ModelError::NeedsCompaction("Responses context overflow".into());
    }
    if code == Some("insufficient_quota") {
        return ModelError::QuotaExceeded(
            "Responses quota exceeded; pause requests and check provider billing and usage".into(),
        );
    }
    let detail = match code {
        Some("unsupported_value" | "invalid_request_error") => "request rejected (check model ID and reasoning effort)",
        _ => match status {
            Some(401) => "authentication failed (check OPENAI_API_KEY)",
            Some(403) => "access forbidden for this model or endpoint",
            Some(402) => "quota or billing limit exceeded; check provider billing and usage",
            Some(429) => "rate limit exceeded; retry later",
            Some(400 | 404 | 422) => "request rejected (check context size, model ID and reasoning effort)",
            Some(500..=599) => "service unavailable; retry later",
            _ => "request failed; no action accepted",
        },
    };
    match status {
        Some(s) => ModelError::Fatal(format!("Responses HTTP {s}: {detail}")),
        None => ModelError::Fatal(format!("Responses inference failed: {detail}")),
    }
}

pub(super) fn decode_json_response(bytes: &[u8], kind: TurnKind) -> std::result::Result<Value, ModelError> {
    let raw: Value = serde_json::from_slice(bytes).map_err(|_| ModelError::Fatal(INVALID.into()))?;
    if let Some(error) = raw.get("error").filter(|e| !e.is_null()) {
        let code = error.get("code").and_then(Value::as_str);
        if kind != TurnKind::Reporter && (code == Some("context_length_exceeded") || is_context_overflow_payload(bytes))
        {
            return Err(ModelError::NeedsCompaction("Responses context overflow".into()));
        }
        if is_quota_exceeded_payload(bytes) {
            return Err(ModelError::QuotaExceeded(
                "Responses quota exceeded; pause requests and check provider billing and usage".into(),
            ));
        }
        return Err(redacted_api_error(Some(200), code, kind));
    }
    let response: Response = serde_json::from_value(raw).map_err(|_| ModelError::Fatal(INVALID.into()))?;
    match select_action(&response).map_err(|e| match e {
        FinalActionErr::Invalid => ModelError::Fatal(INVALID.into()),
        FinalActionErr::TextOnly => ModelError::InvalidAction(TEXT_ONLY.into()),
        FinalActionErr::MultiTool => ModelError::InvalidAction(MULTI_TOOL.into()),
    })? {
        SelectedAction::ToolCall(idx) => {
            let item = &response.output[idx];
            let name = item.name.as_deref().ok_or_else(|| ModelError::InvalidAction(TEXT_ONLY.into()))?;
            let args = item.arguments.as_deref().ok_or_else(|| ModelError::InvalidAction(TEXT_ONLY.into()))?;
            decode_tool_call_str(name, args, kind)
        }
        SelectedAction::FinalMessage(idx) => {
            let parts = response.output[idx].output_texts().map_err(|_| ModelError::InvalidAction(TEXT_ONLY.into()))?;
            let text = concat_texts(&parts)?;
            decode_action(&text, kind)
        }
    }
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
            let code = details
                .as_ref()
                .and_then(|v| v.pointer("/error/code").or_else(|| v.pointer("/error/type")))
                .and_then(Value::as_str);
            if status == 413
                || (matches!(status, 400 | 422)
                    && (code == Some("context_length_exceeded") || is_context_overflow_payload(&err_buf)))
            {
                return Err(RetryErr::Done(if kind == TurnKind::Reporter {
                    ModelError::Fatal(format!("Responses HTTP {status}: context overflow"))
                } else {
                    ModelError::NeedsCompaction(format!("Responses HTTP {status}: context overflow"))
                }));
            }
            if status == 402 || ((status == 429 || status == 400) && is_quota_exceeded_payload(&err_buf)) {
                return Err(RetryErr::Done(ModelError::QuotaExceeded(
                    "Responses quota exceeded; pause requests and check provider billing and usage".into(),
                )));
            }
            if is_retryable_status(status) && attempt < HTTP_MAX_ATTEMPTS {
                let delay = retry_delay(attempt, retry_after.as_deref(), retry_base);
                note_rate_limit(delay, retry_base);
                return Err(RetryErr::Retryable(delay));
            }
            return Err(RetryErr::Done(redacted_api_error(Some(status), code, kind)));
        }
        Err(ureq::Error::Transport(_)) => {
            if attempt < HTTP_MAX_ATTEMPTS {
                return Err(RetryErr::Retryable(retry_delay(attempt, None, retry_base)));
            }
            return Err(RetryErr::Done(ModelError::Fatal("Responses transport failed".into())));
        }
    };

    let media = resp.header("Content-Type").map(|_| resp.content_type().trim().to_ascii_lowercase());
    let raw = match read_bounded_stream(resp) {
        Ok(b) => b,
        Err(true) => {
            if attempt < HTTP_MAX_ATTEMPTS {
                return Err(RetryErr::Retryable(retry_delay(attempt, None, retry_base)));
            }
            return Err(RetryErr::Done(ModelError::Fatal("Responses stream interrupted".into())));
        }
        Err(false) => {
            return Err(RetryErr::Done(ModelError::Fatal(
                "Responses stream exceeds 32 MiB; no action accepted".into(),
            )));
        }
    };
    let text = std::str::from_utf8(&raw).ok().map(|s| s.strip_prefix('\u{feff}').unwrap_or(s));
    if let Some(text) = text {
        let first = text.lines().find(|line| !line.is_empty()).unwrap_or("");
        let framed = ["event:", "data:", ":"].iter().any(|prefix| first.starts_with(prefix));
        if media.as_deref() == Some("text/event-stream") || (media.as_deref() != Some("text/html") && framed) {
            return decode_sse(text, kind, TEXT_ONLY, INVALID).map_err(|err| match err {
                StreamDecodeErr::Model(m) => RetryErr::Done(m),
                StreamDecodeErr::ProviderEvent { code, raw_json } => {
                    if kind != TurnKind::Reporter
                        && (code.as_deref() == Some("context_length_exceeded")
                            || is_context_overflow_payload(raw_json.as_bytes()))
                    {
                        return RetryErr::Done(ModelError::NeedsCompaction(
                            "Responses HTTP 200: context overflow".into(),
                        ));
                    }
                    if code.as_deref() == Some("insufficient_quota") || is_quota_exceeded_payload(raw_json.as_bytes()) {
                        return RetryErr::Done(ModelError::QuotaExceeded(
                            "Responses quota exceeded; pause requests and check provider billing and usage".into(),
                        ));
                    }
                    if matches!(code.as_deref(), Some("rate_limit_exceeded" | "server_error" | "timeout"))
                        && attempt < HTTP_MAX_ATTEMPTS
                    {
                        let delay = retry_delay(attempt, None, retry_base);
                        note_rate_limit(delay, retry_base);
                        return RetryErr::Retryable(delay);
                    }
                    RetryErr::Done(redacted_api_error(None, code.as_deref(), kind))
                }
            });
        }
        if media.as_deref() == Some("application/json") || text.trim_start().starts_with('{') {
            return decode_json_response(&raw, kind).map_err(RetryErr::Done);
        }
    }
    Err(RetryErr::Done(ModelError::Fatal("Responses HTTP 200: expected event stream or JSON response".into())))
}

pub(super) fn transact_with_retry(
    base: &str,
    key: Option<&str>,
    timeout: Duration,
    bytes: &[u8],
    kind: TurnKind,
    retry_base: Duration,
) -> std::result::Result<Value, ModelError> {
    let initial = format!("{}/responses", base.trim_end_matches('/'));
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
                "OPENAI_BASE_URL",
                "OPENAI_API_KEY",
                AuthHeader::Bearer,
            )?
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
    let model = settings
        .model
        .as_deref()
        .ok_or_else(|| ModelError::Fatal("set TEAMAGENTS_MODEL or select an API model with /model MODEL".into()))?;
    let bytes = body(model, settings.effort.as_deref(), agent, task, history, kind, Provider::Responses)
        .map_err(ModelError::Fatal)?;
    transact_with_retry(base, key, TIMEOUT, &bytes, kind, HTTP_RETRY_BASE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    const ACTION: &str = r#"{"op":"finish","text":"ok"}"#;

    fn frames(events: &[Value]) -> String {
        events.iter().map(|e| format!("event: {}\r\ndata: {e}\r\n\r\n", e["type"].as_str().unwrap())).collect()
    }

    fn msg(id: &str, phase: Option<&str>, text: &str) -> Value {
        let mut obj = json!({
            "type": "message",
            "id": id,
            "role": "assistant",
            "status": "completed",
            "content": [{"type": "output_text", "text": text}]
        });
        if let Some(p) = phase {
            obj["phase"] = json!(p);
        }
        obj
    }

    #[test]
    fn accepts_commentary_alongside_final_answer_and_rejects_commentary_only_as_invalid_action() {
        let c_text = "Checking files first.";
        let stream = frames(&[
            json!({"type":"response.created","response":{"id":"r1","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"m1","role":"assistant","phase":"commentary","status":"in_progress","content":[]}}),
            json!({"type":"response.content_part.added","output_index":0,"item_id":"m1","content_index":0,"part":{"type":"output_text","text":""}}),
            json!({"type":"response.output_text.delta","output_index":0,"item_id":"m1","content_index":0,"delta":c_text}),
            json!({"type":"response.output_text.annotation.added","output_index":0,"item_id":"m1","content_index":0}),
            json!({"type":"response.output_text.done","output_index":0,"item_id":"m1","content_index":0,"text":c_text}),
            json!({"type":"response.content_part.done","output_index":0,"item_id":"m1","content_index":0,"part":{"type":"output_text","text":c_text}}),
            json!({"type":"response.output_item.done","output_index":0,"item":msg("m1", Some("commentary"), c_text)}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"message","id":"m2","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.content_part.added","output_index":1,"item_id":"m2","content_index":0,"part":{"type":"output_text","text":""}}),
            json!({"type":"response.output_text.delta","output_index":1,"item_id":"m2","content_index":0,"delta":&ACTION[..10]}),
            json!({"type":"response.output_text.delta","output_index":1,"item_id":"m2","content_index":0,"delta":&ACTION[10..]}),
            json!({"type":"response.output_text.done","output_index":1,"item_id":"m2","content_index":0,"text":ACTION}),
            json!({"type":"response.content_part.done","output_index":1,"item_id":"m2","content_index":0,"part":{"type":"output_text","text":ACTION}}),
            json!({"type":"response.output_item.done","output_index":1,"item":msg("m2", Some("final_answer"), ACTION)}),
            json!({"type":"response.completed","response":{"id":"r1","status":"completed","output":[msg("m1", Some("commentary"), c_text), msg("m2", Some("final_answer"), ACTION)]}}),
        ]);
        let parsed = decode_sse(&stream, TurnKind::Root, TEXT_ONLY, INVALID).ok().unwrap();
        assert_eq!(parsed, json!({"op":"finish","text":"ok"}));

        let commentary_only = frames(&[
            json!({"type":"response.completed","response":{"id":"r2","status":"completed","output":[msg("m1", Some("commentary"), "Thinking...")]}}),
        ]);
        assert!(matches!(
            decode_sse(&commentary_only, TurnKind::Node, TEXT_ONLY, INVALID),
            Err(StreamDecodeErr::Model(ModelError::InvalidAction(_)))
        ));

        let args = r#"{"path":"README.md","offset":1,"limit":50}"#;
        let fc_stream = frames(&[
            json!({"type":"response.created","response":{"id":"r3","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"n0","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"item_id":"n0","content_index":0,"delta":"Reading file"}),
            json!({"type":"response.output_item.done","output_index":0,"item":msg("n0", None, "Reading file")}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"call_1","name":"read_file","arguments":"","status":"in_progress"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc1","delta":&args[..15]}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"item_id":"fc1","delta":&args[15..]}),
            json!({"type":"response.function_call_arguments.done","output_index":1,"item_id":"fc1","arguments":args}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc1","call_id":"call_1","name":"read_file","arguments":args,"status":"completed"}}),
            json!({"type":"response.completed","response":{"id":"r3","status":"completed","output":[
                msg("n0", None, "Reading file"),
                {"type":"function_call","id":"fc1","call_id":"call_1","name":"read_file","arguments":args,"status":"completed"}
            ]}}),
        ]);
        assert_eq!(
            decode_sse(&fc_stream, TurnKind::Node, TEXT_ONLY, INVALID).ok().unwrap(),
            json!({"op":"read_file","path":"README.md","offset":1,"limit":50})
        );

        let multi_fc =
            frames(&[json!({"type":"response.completed","response":{"id":"r4","status":"completed","output":[
                {"type":"function_call","id":"fc1","name":"members","arguments":"{}","status":"completed"},
                {"type":"function_call","id":"fc2","name":"finish","arguments":"{\"text\":\"x\"}","status":"completed"}
            ]}})]);
        assert!(matches!(
            decode_sse(&multi_fc, TurnKind::Node, TEXT_ONLY, INVALID),
            Err(StreamDecodeErr::Model(ModelError::InvalidAction(ref msg))) if msg.contains("exactly one tool call")
        ));
    }

    #[test]
    fn responses_api_handles_json_sse_quota_and_overflow() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for idx in 0..3 {
                let (mut s, _) = listener.accept().unwrap();
                let mut r = BufReader::new(&s);
                let line = r.by_ref().lines().next().unwrap().unwrap();
                assert_eq!(line, "POST /v1/responses HTTP/1.1");
                let mut len = 0usize;
                for h in r.by_ref().lines() {
                    let h = h.unwrap();
                    if h.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap();
                        }
                    }
                }
                let mut req_body = vec![0; len];
                r.read_exact(&mut req_body).unwrap();
                match idx {
                    0 => {
                        let json_resp = json!({
                            "id": "r_json",
                            "status": "completed",
                            "output": [msg("m1", Some("final_answer"), r#"{"op":"members"}"#)]
                        })
                        .to_string();
                        write!(
                            s,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_resp}",
                            json_resp.len()
                        )
                        .unwrap();
                    }
                    1 => {
                        let quota = json!({"error":{"code":"insufficient_quota","message":"secret-acct"}}).to_string();
                        write!(
                            s,
                            "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{quota}",
                            quota.len()
                        )
                        .unwrap();
                    }
                    _ => {
                        let overflow =
                            json!({"error":{"code":"context_length_exceeded","message":"too long"}}).to_string();
                        write!(
                            s,
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{overflow}",
                            overflow.len()
                        )
                        .unwrap();
                    }
                }
            }
        });
        let got = transact_with_retry(
            &base,
            Some("k"),
            Duration::from_secs(3),
            b"{}",
            TurnKind::Node,
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(got, json!({"op":"members"}));
        let err_q = transact_with_retry(
            &base,
            Some("k"),
            Duration::from_secs(3),
            b"{}",
            TurnKind::Node,
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(matches!(err_q, ModelError::QuotaExceeded(_)) && !err_q.to_string().contains("secret-acct"));
        let err_c = transact_with_retry(
            &base,
            Some("k"),
            Duration::from_secs(3),
            b"{}",
            TurnKind::Node,
            Duration::from_millis(1),
        )
        .unwrap_err();
        assert!(matches!(err_c, ModelError::NeedsCompaction(_)));
        server.join().unwrap();
    }
}
