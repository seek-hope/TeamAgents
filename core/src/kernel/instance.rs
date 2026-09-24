//! KernelInstance: the no-I/O state-transition core (plan §3). It holds the
//! instance profile snapshot and converts between the persisted context and
//! the model edge. All identity, operation ids, permission revisions and
//! delivery sequence numbers are filled in by the runtime — never here.

use super::types::*;
use serde_json::{json, Value as Json};
use std::collections::HashMap;

/// Static profile snapshot for one instance revision. A profile change makes
/// a new KernelInstance (new profile_revision), never a silent mutation.
#[derive(Debug, Clone)]
pub struct KernelProfile {
    pub model: String,
    pub instructions: String,
    /// Model-visible tool schemas (chat-completions shape), without the
    /// built-ins — the kernel appends finish/readback itself.
    pub tools: Vec<Json>,
    /// Effective generation options snapshot (temperature, effort, …).
    pub options: Json,
    /// Native context window; None is invalid for real runs (D-36), the
    /// driver must resolve a reliable value before constructing requests.
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Interpretation {
    /// The assistant entry to append in the same transaction as the decision.
    pub entry: ContextEntry,
    pub output: KernelOutput,
    /// Non-fatal protocol notes the runtime should surface as an observation
    /// (e.g. finish combined with other calls and therefore ignored).
    pub notes: Vec<String>,
}

pub struct KernelInstance {
    pub instance_id: String,
    pub epoch: u64,
    profile: KernelProfile,
}

/// L1 view-masking budget bounds (ported from the legacy loop).
const MASK_KEEP_RECENT: usize = 16_000;
const MASK_KEEP_MAX: usize = 256_000;
/// L2 trigger: last prompt over this fraction of the window (~90%, as before).
pub const COMPACT_AT: f64 = 0.9;

impl KernelInstance {
    pub fn new(instance_id: impl Into<String>, epoch: u64, profile: KernelProfile) -> Self {
        KernelInstance { instance_id: instance_id.into(), epoch, profile }
    }

    pub fn profile(&self) -> &KernelProfile {
        &self.profile
    }

    /// ContextView → ModelRequest (plan §3 `prepare_request`). The request is
    /// fixed once returned: the runtime reserves budget, then registers the
    /// request and moves the phase to MODEL_PENDING in one transaction.
    pub fn prepare_request(&self, entries: &[ContextEntry], request_id: &str) -> ModelRequest {
        let mut messages = Vec::with_capacity(entries.len() + 1);
        messages.push(json!({"role": "system", "content": self.profile.instructions}));
        messages.extend(materialize(entries, self.profile.context_window));
        let mut tools = self.profile.tools.clone();
        tools.extend(builtin_tool_schemas());
        let est = estimated_tokens(&json!({"messages": messages, "tools": tools}));
        ModelRequest {
            request_id: request_id.to_string(),
            model: self.profile.model.clone(),
            messages,
            tools,
            options: self.profile.options.clone(),
            est_prompt_tokens: est,
        }
    }

    /// ModelResponse → assistant entry + intents (§3 `interpret_response`).
    /// Only a complete response reaches this function; half streams stay
    /// archived attempts. A sole `finish` call is the CompletionCandidate;
    /// combined with other calls it is ignored with a note.
    pub fn interpret_response(&self, response: &ModelResponse, entry_id: &str) -> Interpretation {
        let entry = ContextEntry::new(entry_id, EntryKind::Assistant, response.message.clone());
        let calls = response.message["tool_calls"].as_array().cloned().unwrap_or_default();
        let mut notes = vec![];
        let finish = calls.iter().position(|call| call["function"]["name"] == FINISH_TOOL);
        if let Some(pos) = finish {
            if calls.len() == 1 {
                return Interpretation {
                    entry,
                    output: KernelOutput::Completion(parse_completion(&calls[pos])),
                    notes,
                };
            }
            notes.push(format!(
                "ignored {FINISH_TOOL}: it must be the only tool call in its response; the other calls ran normally"
            ));
        }
        // a sole wait call is the Wait output (§5.3): it excludes every
        // other action in the same response, exactly like finish
        let wait = calls.iter().position(|call| call["function"]["name"] == WAIT_TOOL);
        if let Some(pos) = wait {
            if calls.len() == 1 {
                let args: Json = serde_json::from_str(calls[pos]["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or_else(|_| json!({"_invalid_arguments": calls[pos]["function"]["arguments"].as_str()}));
                return Interpretation { entry, output: KernelOutput::Wait(args), notes };
            }
            notes.push(format!(
                "ignored {WAIT_TOOL}: it must be the only tool call in its response; the other calls ran normally"
            ));
        }
        if calls.is_empty() {
            let reply = response.message["content"].as_str().unwrap_or("").to_string();
            return Interpretation { entry, output: KernelOutput::Reply(reply), notes };
        }
        let intents = calls
            .iter()
            .enumerate()
            .filter(|(i, _)| Some(*i) != finish && Some(*i) != wait)
            .map(|(index, call)| {
                let args: Json = serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}"))
                    .unwrap_or_else(|_| json!({"_invalid_arguments": call["function"]["arguments"].as_str()}));
                ToolIntent {
                    index,
                    call_id: call["id"].as_str().unwrap_or("").to_string(),
                    name: call["function"]["name"].as_str().unwrap_or("").to_string(),
                    args_hash: args_hash(&args),
                    args,
                }
            })
            .collect::<Vec<_>>();
        if intents.is_empty() {
            // finish was ignored above and nothing else remains.
            return Interpretation { entry, output: KernelOutput::Reply(String::new()), notes };
        }
        Interpretation { entry, output: KernelOutput::ToolIntents(intents), notes }
    }

    /// Observation → context entry (§3 `apply_observation`). The runtime
    /// appends the returned entry and advances the phase in one transaction,
    /// deduplicated by (instance, epoch, envelope/operation id).
    pub fn apply_observation(&self, observation: &Observation, entry_id: &str) -> ContextEntry {
        match observation {
            Observation::ToolResult { call_id, name: _, content, receipt_ref } => {
                let mut entry = ContextEntry::new(
                    entry_id,
                    EntryKind::ToolResult,
                    json!({"role": "tool", "tool_call_id": call_id, "content": content}),
                );
                entry.refs.push(receipt_ref.clone());
                entry
            }
            Observation::Note(text) => {
                ContextEntry::new(entry_id, EntryKind::Note, json!({"role": "user", "content": text}))
            }
        }
    }

    /// Context-entry user input (accept boundary, §5.4): appended with the
    /// dedup record and phase advance in the same transaction.
    pub fn user_entry(&self, text: &str, entry_id: &str) -> ContextEntry {
        ContextEntry::new(entry_id, EntryKind::User, json!({"role": "user", "content": text}))
    }
}

fn parse_completion(call: &Json) -> CompletionCandidate {
    let args: Json = serde_json::from_str(call["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
    let outcome = match args["status"].as_str().unwrap_or("failed") {
        "success" => Outcome::Success,
        "blocked" => Outcome::Blocked,
        _ => Outcome::Failed,
    };
    let strings =
        |key: &str| args[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
    CompletionCandidate {
        outcome,
        summary: args["summary"].as_str().unwrap_or("").to_string(),
        evidence: strings("evidence"),
        unverified: strings("unverified"),
    }
}

/// Strict wire endpoints (OpenAI-style Responses servers, Anthropic) require
/// every assistant entry's `tool_calls` to be answered by the tool messages
/// that immediately follow it. The runtime appends facts in storage order, so
/// an arriving message or a note can land between a call and its answer; the
/// wire copy moves each answer up next to its call. Stored order is untouched
/// — only what the model reads is reordered.
fn pair_tool_results(messages: &[Json]) -> Vec<Json> {
    let mut out: Vec<Json> = Vec::with_capacity(messages.len());
    let mut placed = vec![false; messages.len()];
    for (index, message) in messages.iter().enumerate() {
        if placed[index] {
            continue;
        }
        out.push(message.clone());
        placed[index] = true;
        let Some(calls) = message["tool_calls"].as_array() else { continue };
        let ids: Vec<&str> = calls.iter().filter_map(|call| call["id"].as_str()).collect();
        if ids.is_empty() {
            continue;
        }
        for (later, candidate) in messages.iter().enumerate().skip(index + 1) {
            if placed[later] || candidate["role"] != json!("tool") {
                continue;
            }
            if let Some(id) = candidate["tool_call_id"].as_str() {
                if ids.contains(&id) {
                    out.push(candidate.clone());
                    placed[later] = true;
                }
            }
        }
    }
    out
}

/// L1 view-only masking of old tool outputs (ported contract): originals stay
/// in the context store; only the wire copy is masked, with a readback recipe
/// that preserves page coordinates.
fn materialize(entries: &[ContextEntry], context_window: Option<u64>) -> Vec<Json> {
    let stored: Vec<Json> = entries.iter().map(|entry| entry.message.clone()).collect();
    let messages = pair_tool_results(&stored);
    let last_assistant = messages.iter().rposition(|m| m["role"] == "assistant").unwrap_or(0);
    let readbacks: HashMap<&str, Json> = messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .flat_map(|message| message["tool_calls"].as_array().into_iter().flatten())
        .filter(|call| call["function"]["name"] == READBACK_TOOL)
        .filter_map(|call| {
            let id = call["id"].as_str()?;
            let args: Json = serde_json::from_str(call["function"]["arguments"].as_str()?).ok()?;
            args["tool_call_id"].as_str().filter(|source| !source.is_empty())?;
            let recipe: serde_json::Map<String, Json> = args
                .as_object()?
                .iter()
                .filter(|(key, _)| matches!(key.as_str(), "tool_call_id" | "offset" | "limit"))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            Some((id, Json::Object(recipe)))
        })
        .collect();
    let read_hint = |id: &str| match readbacks.get(id) {
        Some(recipe) => format!("call {READBACK_TOOL} with {recipe}"),
        None => format!("call {READBACK_TOOL} with tool_call_id={id:?}"),
    };
    let mut out = messages.to_vec();
    for message in &mut out {
        if message["role"] == "tool" {
            if let Some(content) = message["content"].as_str() {
                let capped = cap_tool_output(content);
                if capped != content {
                    message["content"] = json!(format!(
                        "{capped}\n[Full output: {}]",
                        read_hint(message["tool_call_id"].as_str().unwrap_or(""))
                    ));
                }
            }
        }
    }
    if out.is_empty() {
        return out;
    }
    let mut budget = context_window
        .map(|window| (window / 4).clamp(MASK_KEEP_RECENT as u64, MASK_KEEP_MAX as u64) as usize)
        .unwrap_or(MASK_KEEP_RECENT);
    for i in (0..=last_assistant).rev() {
        let message = &out[i];
        if message["role"] != "tool" {
            continue;
        }
        let len = message["content"].as_str().map(|c| c.len()).unwrap_or(0);
        if budget >= len {
            budget -= len;
            continue;
        }
        budget = 0;
        let id = message["tool_call_id"].as_str().unwrap_or("");
        out[i] = json!({
            "role": "tool",
            "tool_call_id": id,
            "content": format!("[tool output hidden ({len} bytes) — {} to retrieve it]", read_hint(id)),
        });
    }
    out
}
