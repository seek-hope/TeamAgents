//! R2 kernel data types (rebuild plan §3, §4.1). These are the no-I/O
//! contracts between the persistent runtime and the model edge: the runtime
//! owns identity assignment, persistence and execution; the kernel only
//! transforms context into requests and responses into intents.

use serde_json::{json, Value as Json};

/// Built-in completion tool: the single CompletionCandidate entry point (§8).
pub const FINISH_TOOL: &str = "finish";
/// Built-in readback tool: pages the full stored output of an earlier call.
pub const READBACK_TOOL: &str = "read_history";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Instructions,
    User,
    Assistant,
    ToolResult,
    Note,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Instructions => "instructions",
            EntryKind::User => "user",
            EntryKind::Assistant => "assistant",
            EntryKind::ToolResult => "tool_result",
            EntryKind::Note => "note",
        }
    }
}

/// One appended context fact. `message` uses the neutral chat shape
/// (role/content/tool_calls/tool_call_id/reasoning_content); provider
/// adapters convert at the edge. Large tool outputs stay here in full — the
/// model-facing masking is view-time only (L1, arXiv 2508.21433), and the
/// storage layer (P2) moves them to artifact references.
#[derive(Debug, Clone)]
pub struct ContextEntry {
    pub id: String,
    pub kind: EntryKind,
    pub message: Json,
    /// Stable references: receipt/request/operation ids this entry consumes.
    pub refs: Vec<String>,
    pub created: f64,
}

impl ContextEntry {
    pub fn new(id: impl Into<String>, kind: EntryKind, message: Json) -> Self {
        ContextEntry { id: id.into(), kind, message, refs: vec![], created: crate::models::now() }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Usage {
    pub prompt: u64,
    pub completion: u64,
    pub total: u64,
}

impl Usage {
    pub fn from_json(data: &Json) -> Option<Usage> {
        // OpenAI/DeepSeek chat shape, then Anthropic shape.
        if let Some(usage) = data.get("usage") {
            let prompt = usage["prompt_tokens"].as_u64().or_else(|| usage["input_tokens"].as_u64())?;
            let completion =
                usage["completion_tokens"].as_u64().or_else(|| usage["output_tokens"].as_u64()).unwrap_or(0);
            let total = usage["total_tokens"].as_u64().unwrap_or(prompt + completion);
            return Some(Usage { prompt, completion, total });
        }
        None
    }
}

/// Provider-neutral model request. `messages`/`tools` are chat-completions
/// shaped; `options` carries the effective generation snapshot so a restart
/// never silently inherits a mutated global profile (§4.1).
#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub request_id: String,
    pub model: String,
    pub messages: Vec<Json>,
    pub tools: Vec<Json>,
    pub options: Json,
    pub est_prompt_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    /// Complete assistant message (chat shape, incl. reasoning_content).
    pub message: Json,
    pub usage: Option<Usage>,
    /// Provider-native extras kept with provenance, never flattened away (§7).
    pub native: Json,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolIntent {
    /// Index inside the owning decision; operation_id = decision_id + index.
    pub index: usize,
    pub call_id: String,
    pub name: String,
    pub args: Json,
    pub args_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Outcome {
    Success,
    Blocked,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Success => "success",
            Outcome::Blocked => "blocked",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompletionCandidate {
    pub outcome: Outcome,
    pub summary: String,
    pub evidence: Vec<String>,
    pub unverified: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum KernelOutput {
    /// Execute these tool intents (decision + operations registered by the
    /// runtime in one transaction with the response import).
    ToolIntents(Vec<ToolIntent>),
    /// Plain reply: does NOT settle any task or goal (§3).
    Reply(String),
    /// Single finish call: triggers the completion checks (§8).
    Completion(CompletionCandidate),
    /// Reserved for jobs/tasks/timers (P2+); unused by the P1 reference.
    Wait(Json),
}

/// What the runtime feeds back into an instance after execution.
#[derive(Debug, Clone)]
pub enum Observation {
    ToolResult {
        call_id: String,
        name: String,
        /// Model-facing content (after per-call cap); full output remains
        /// reachable through the readback tool.
        content: String,
        /// Receipt reference for traceability (§4.1 ToolReceipt).
        receipt_ref: String,
    },
    /// Inter-instance message or system note (P3 uses the full envelope).
    Note(String),
}

/// SHA-256 hex of the canonical JSON args, binding intent to parameters (§6.1).
pub fn args_hash(args: &Json) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(args.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// ponytail: conservative text estimate, not a tokenizer; provider usage wins
/// when larger (ported from the legacy chat loop, same contract).
pub fn estimated_tokens(value: &Json) -> u64 {
    let text = value.to_string();
    let ascii = text.bytes().filter(u8::is_ascii).count();
    (ascii.div_ceil(4) + text.chars().filter(|c| !c.is_ascii()).count()) as u64
}

/// Per-call model-facing output cap (same policy as the legacy loop).
pub const TOOL_OUTPUT_CAP: usize = 24_000;

pub fn cap_tool_output(content: &str) -> String {
    let chars = content.chars().count();
    if chars <= TOOL_OUTPUT_CAP {
        return content.to_string();
    }
    let half = TOOL_OUTPUT_CAP / 2;
    let head: String = content.chars().take(half).collect();
    let tail: String = content.chars().skip(chars.saturating_sub(half)).collect();
    format!(
        "{head}\n[...{} chars truncated...]\n{tail}",
        chars.saturating_sub(head.chars().count() + tail.chars().count())
    )
}

/// Built-in tool schemas every instance sees (finish + readback). `finish`
/// must be the only call in its response; readback pages stored outputs.
pub fn builtin_tool_schemas() -> Vec<Json> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": FINISH_TOOL,
                "description": "Finish the current goal exactly once. Must be the only tool call in this response. Report the real outcome; a summary that admits undelivered work must not claim success.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "status": {"type": "string", "enum": ["success", "blocked", "failed"]},
                        "summary": {"type": "string", "description": "What was done and the final answer."},
                        "evidence": {"type": "array", "items": {"type": "string"}, "description": "References proving the claims (files, commands, receipts)."},
                        "unverified": {"type": "array", "items": {"type": "string"}, "description": "Claims that could not be verified."}
                    },
                    "required": ["status", "summary"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": READBACK_TOOL,
                "description": "Page through the full stored output of an earlier tool call (ids appear in masked outputs).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "tool_call_id": {"type": "string"},
                        "offset": {"type": "integer", "minimum": 0},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 12000}
                    },
                    "required": ["tool_call_id"]
                }
            }
        }),
    ]
}

/// Readback paging (ported contract: 1..=12000 chars, coordinates preserved).
pub fn page_output(output: &str, args: &Json) -> Result<Json, String> {
    let integer = |key: &str, default: u64| -> Result<usize, String> {
        let value = match args.get(key) {
            Some(value) => value.as_u64().ok_or_else(|| format!("{key} must be a nonnegative integer"))?,
            None => default,
        };
        usize::try_from(value).map_err(|_| format!("{key} is too large"))
    };
    let offset = integer("offset", 0)?;
    let limit = integer("limit", 12_000)?;
    if !(1..=12_000).contains(&limit) {
        return Err("limit must be between 1 and 12000".into());
    }
    let total = output.chars().count();
    if offset > total {
        return Err(format!("offset exceeds output length {total}"));
    }
    let content: String = output.chars().skip(offset).take(limit).collect();
    let next = offset + content.chars().count();
    Ok(json!({
        "output": content,
        "offset": offset,
        "next_offset": if next < total { Some(next) } else { None },
        "eof": next == total,
        "total_chars": total,
    }))
}

/// Structured tool receipt (plan §4.1 ToolReceipt, §7 shell fields). Identity
/// fields (operation_id, decision, grant revision) are filled by the runtime;
/// `content` is the model-facing text, `error` the classified failure.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolReceipt {
    pub operation_id: String,
    pub tool: String,
    pub args_hash: String,
    pub ok: bool,
    /// The command actually started (isolation failure reports false).
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub duration_ms: u64,
    /// Large output artifact reference (P2 storage formalizes artifacts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<String>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ReceiptError>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReceiptError {
    pub class: String,
    pub reason: String,
}
