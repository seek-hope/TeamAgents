//! Chat member backend: a plain tool-calling loop over an OpenAI-compatible
//! endpoint (team semantics stay in the core).

use crate::gateway::{ToolGateway, TurnControl, TEAM_TOOLS};
use crate::runtime::{AgentRunner, Notify};
use serde_json::{json, Value as Json};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{ModelProfile, TurnRun, TurnStatus};

/// The protocol picks the default endpoint
/// (deepseek profiles talk to api.deepseek.com, not to OpenAI).
pub fn resolve_base_url(profile: &ModelProfile) -> String {
    if let Some(base) = &profile.base_url {
        if !base.trim().is_empty() {
            return base.trim_end_matches('/').to_string();
        }
    }
    match profile.provider.as_str() {
        "deepseek" => "https://api.deepseek.com/v1".into(),
        _ => match profile.protocol.as_str() {
            "deepseek" => "https://api.deepseek.com/v1".into(),
            "anthropic" => "https://api.anthropic.com".into(),
            _ => "https://api.openai.com/v1".into(),
        },
    }
}

/// A model that lacks `xhigh` maps to `max`
/// instead of erroring out (user decision: deepseek has no xhigh level).
pub fn normalize_effort(protocol: &str, effort: &str) -> String {
    if effort.eq_ignore_ascii_case("xhigh") && protocol == "deepseek" {
        "max".into()
    } else {
        effort.to_string()
    }
}

/// A provider rejecting the requested
/// reasoning effort; the caller retries once with `max`.
pub fn looks_like_effort_error(text: &str) -> bool {
    let text = text.to_lowercase();
    ["reasoning_effort", "reasoning effort", "effort", "unsupported value"]
        .iter()
        .any(|token| text.contains(token))
}

/// Retry semantics: transient statuses and transport errors only.
fn retryable_status(code: u16) -> bool {
    matches!(code, 408 | 409 | 429) || (500..600).contains(&code)
}

/// Compact text view for the next model call.
pub fn render_view(view: &Json, wake: &Json, workdir: Option<&str>) -> String {
    let mut parts: Vec<String> = vec![];
    let wake_reason = wake.get("reason").and_then(|v| v.as_str()).unwrap_or("new_input");
    if wake_reason != "new_input" {
        let payload = wake.get("payload").cloned().unwrap_or(json!({}));
        parts.push(format!("<wake reason=\"{wake_reason}\">{payload}</wake>"));
    }
    let assignment = view.get("assignment").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if !assignment.is_empty() {
        let tasks: Vec<Json> = assignment
            .iter()
            .map(|t| {
                json!({
                    "task_id": t.get("task_id"),
                    "description": t.get("description"),
                    "acceptance": t.get("acceptance"),
                    "status": t.get("status"),
                    "requester": t.get("requester"),
                })
            })
            .collect();
        parts.push(format!("<your_tasks>{}</your_tasks>", Json::Array(tasks)));
    }
    for item in view.get("inbox_delta").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        parts.push(format!(
            "<inbox from=\"{}\" kind=\"{}\">{}</inbox>",
            item.get("from").and_then(|v| v.as_str()).unwrap_or(""),
            item.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
            item.get("payload").cloned().unwrap_or(json!({}))
        ));
    }
    let shared = view.get("permitted_shared_delta").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if !shared.is_empty() {
        let entries: Vec<Json> = shared
            .iter()
            .map(|e| {
                let content: String = e
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .chars()
                    .take(500)
                    .collect();
                json!({
                    "space": e.get("space_id"),
                    "author": e.get("author"),
                    "kind": e.get("kind"),
                    "content": content,
                    "ref": e.get("ref"),
                })
            })
            .collect();
        parts.push(format!("<shared_space_updates>{}</shared_space_updates>", Json::Array(entries)));
    }
    let topology = view.get("relevant_topology").cloned().unwrap_or(json!({}));
    let team = json!({
        "members": topology.get("members"),
        "you_can_message": topology.get("can_send_to"),
        "you_can_delegate_to": topology.get("can_delegate_to"),
        "shared_spaces": topology.get("shared_spaces"),
    });
    parts.push(format!(
        "<team revision=\"{}\">{team}</team>",
        topology.get("revision").and_then(|v| v.as_i64()).unwrap_or(0)
    ));
    if let Some(workdir) = workdir {
        parts.push(format!("<your_workspace>{workdir}</your_workspace>"));
    }
    parts.join("\n")
}

pub const TEAM_TOOL_DOCS: &[(&str, &str)] = &[
    ("send_message", "Send a message to a teammate you are allowed to reach. target='*' broadcasts where a broadcast channel exists."),
    ("assign_task", "Assign a task to a teammate; returns a task_id immediately and never waits for completion. Include acceptance criteria."),
    ("complete_task", "Report the current task finished with result refs and a short summary; the task becomes SUCCEEDED when your turn ends cleanly."),
    ("wait_for_tasks", "Park this turn until the given tasks finish (or the user sends new input). Releases your execution slot."),
    ("publish_shared", "Append a structured entry (finding/decision/artifact ref) to a shared space you can write to."),
    ("read_shared", "Read shared-space entries after a sequence cursor."),
    ("list_shared", "List shared spaces you can read and their entry counts."),
    ("request_help", "Ask the Leader for help with your current task."),
    ("propose_team_change", "Propose a team/topology change to the Leader; only the Leader can apply it."),
    ("apply_topology_patch", "Leader only: apply (or reject) a topology patch from a base revision. add_agent may omit model_profile: a per-member profile is then auto-created from the Leader's current model."),
    ("cancel_task", "Leader only: cancel an unfinished or blocked task; running work stops first."),
    ("cancel_run", "Leader only: request a turn to stop; side effects are not rolled back."),
    ("signal_done", "Leader only: declare the current user goal complete; the runtime verifies no work, approvals or unknown outcomes are outstanding."),
];

fn team_tool_schemas() -> Json {
    json!([
      {"name": "send_message", "parameters": {"type": "object", "properties": {"target": {"type": "string"}, "text": {"type": "string"}}, "required": ["target", "text"]}},
      {"name": "assign_task", "parameters": {"type": "object", "properties": {"assignee": {"type": "string"}, "description": {"type": "string"}, "acceptance": {"type": "string"}, "dependencies": {"type": "array", "items": {"type": "string"}}}, "required": ["assignee", "description"]}},
      {"name": "complete_task", "parameters": {"type": "object", "properties": {"task_id": {"type": "string"}, "result_refs": {"type": "array", "items": {"type": "string"}}, "summary": {"type": "string"}}, "required": ["task_id"]}},
      {"name": "wait_for_tasks", "parameters": {"type": "object", "properties": {"task_ids": {"type": "array", "items": {"type": "string"}}}, "required": ["task_ids"]}},
      {"name": "publish_shared", "parameters": {"type": "object", "properties": {"space_id": {"type": "string"}, "content": {"type": "string"}, "kind": {"type": "string"}, "ref": {"type": "string"}}, "required": ["space_id"]}},
      {"name": "read_shared", "parameters": {"type": "object", "properties": {"space_id": {"type": "string"}, "after_sequence": {"type": "integer"}, "limit": {"type": "integer"}}}},
      {"name": "list_shared", "parameters": {"type": "object", "properties": {}}},
      {"name": "request_help", "parameters": {"type": "object", "properties": {"message": {"type": "string"}, "task_id": {"type": "string"}}, "required": ["message"]}},
      {"name": "propose_team_change", "parameters": {"type": "object", "properties": {"operations": {"type": "array", "items": {"type": "object"}}, "rationale": {"type": "string"}}, "required": ["operations"]}},
      {"name": "apply_topology_patch", "parameters": {"type": "object", "properties": {"operations": {"type": "array", "items": {"type": "object"}}, "patch_id": {"type": "string"}, "base_revision": {"type": "integer"}, "reject": {"type": "boolean"}}}},
      {"name": "cancel_task", "parameters": {"type": "object", "properties": {"task_id": {"type": "string"}}, "required": ["task_id"]}},
      {"name": "cancel_run", "parameters": {"type": "object", "properties": {"run_id": {"type": "string"}}, "required": ["run_id"]}},
      {"name": "signal_done", "parameters": {"type": "object", "properties": {"summary": {"type": "string"}}}},
    ])
}

/// Execution tools a member sees when its TeamSpec binds the capability
/// (team tools + shell + bound file/web tools).
pub const BOUND_TOOL_DOCS: &[(&str, &str)] = &[
    ("ls", "List files in your workspace (path defaults to '.')."),
    ("read_file", "Read a UTF-8 text file from your workspace (path is relative to the workspace root)."),
    ("write_file", "Write a text file in your workspace, creating parent directories."),
    ("edit_file", "Replace the first occurrence of old_string with new_string in a workspace file."),
    ("delete", "Delete a file (a directory when recursive=true) from your workspace."),
    ("glob", "Find workspace files matching a glob pattern, e.g. '**/*.py' (max 500 hits)."),
    ("grep", "Search workspace files for a pattern; returns matching lines (max 100)."),
    ("shell", "Run a shell command in the isolated Linux sandbox (no network by default; network=true requires user approval)."),
    ("web_search", "Search the web and return title, source URL, snippet, fetch time (and full content when include_content=true)."),
    ("web_fetch", "Fetch a web page and return title, source URL, fetch time and the readable text body (HTML only; capped)."),
    ("skill", "Discover and load agent skills. action='search' with query keywords lists matching skills (name — summary); action='read' with a skill name loads its full instructions. Read a skill before applying it."),
    ("read_history", "Retrieve the original output of an earlier tool call by its tool_call_id. Older tool outputs may be hidden from your context to save space; this fetches them back."),
];

fn bound_tool_schemas() -> Json {
    json!([
      {"name": "ls", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}},
      {"name": "read_file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}},
      {"name": "write_file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}},
      {"name": "edit_file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "old_string": {"type": "string"}, "new_string": {"type": "string"}}, "required": ["path", "old_string", "new_string"]}},
      {"name": "delete", "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "recursive": {"type": "boolean"}}, "required": ["path"]}},
      {"name": "glob", "parameters": {"type": "object", "properties": {"pattern": {"type": "string"}}, "required": ["pattern"]}},
      {"name": "grep", "parameters": {"type": "object", "properties": {"pattern": {"type": "string"}, "path": {"type": "string"}}, "required": ["pattern"]}},
      {"name": "shell", "parameters": {"type": "object", "properties": {"command": {"type": "string"}, "timeout": {"type": "integer"}, "network": {"type": "boolean"}}, "required": ["command"]}},
      {"name": "web_search", "parameters": {"type": "object", "properties": {"query": {"type": "string"}, "max_results": {"type": "integer"}, "include_content": {"type": "boolean"}}, "required": ["query"]}},
      {"name": "web_fetch", "parameters": {"type": "object", "properties": {"url": {"type": "string"}, "max_bytes": {"type": "integer"}}, "required": ["url"]}},
      {"name": "skill", "parameters": {"type": "object", "properties": {"action": {"type": "string", "enum": ["search", "read"]}, "query": {"type": "string"}, "name": {"type": "string"}}, "required": ["action"]}},
      {"name": "read_history", "parameters": {"type": "object", "properties": {"tool_call_id": {"type": "string"}}, "required": ["tool_call_id"]}},
    ])
}

/// Which execution tools a member's bindings expose. `files`/`shell` are the
/// built-ins; the web flags come from the resolved bindings (tools.rs web_tools),
/// so explicit service names work too, not just the literal `web`. MCP tools are
/// advertised from the bound tool set separately.
fn bound_tool_names(bindings: &[String], web: (bool, bool)) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = vec![];
    if bindings.iter().any(|b| b == "files") {
        names.extend(["ls", "read_file", "write_file", "edit_file", "delete", "glob", "grep"]);
    }
    if bindings.iter().any(|b| b == "shell") {
        names.push("shell");
    }
    if web.0 {
        names.push("web_search");
    }
    if web.1 {
        names.push("web_fetch");
    }
    if bindings.iter().any(|b| b == "skills") {
        names.push("skill");
    }
    // Runtime-provided, not a capability: every chat-runtime member has a
    // history tree its masked outputs can be read back from.
    names.push("read_history");
    names
}

fn tools_payload(bindings: &[String], web: (bool, bool), bound: &[Json]) -> Json {
    let docs: HashMap<&str, &str> = TEAM_TOOL_DOCS.iter().chain(BOUND_TOOL_DOCS).copied().collect();
    let allowed: Vec<&str> = TEAM_TOOL_DOCS
        .iter()
        .map(|(name, _)| *name)
        .chain(bound_tool_names(bindings, web))
        .chain(bound.iter().filter_map(|tool| tool.get("name").and_then(Json::as_str)))
        .collect();
    let mut schemas = team_tool_schemas().as_array().cloned().unwrap_or_default();
    schemas.extend(bound_tool_schemas().as_array().cloned().unwrap_or_default());
    schemas.extend(bound.iter().cloned());
    Json::Array(
        schemas
            .into_iter()
            .filter(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
                allowed.contains(&name)
            })
            .map(|tool| {
                let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
                json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": tool.get("description").and_then(Json::as_str)
                            .unwrap_or_else(|| docs.get(name).copied().unwrap_or(name)),
                        "parameters": tool.get("parameters").cloned().unwrap_or(json!({})),
                    }
                })
            })
            .collect(),
    )
}


/// An assistant message that carries `tool_calls` must be followed by one tool
/// message per call id, otherwise the provider rejects the whole conversation.
/// Pausing/interrupting mid-batch would leave the tail unanswered, so the
/// skipped calls get an explicit "not executed" result.
fn fill_unanswered_tool_calls(history: &mut Vec<Json>, calls: &[Json], answered: &[String], kind: &str) {
    let reason = if kind == "TurnInterrupted" {
        "not executed: the turn was interrupted before this call"
    } else {
        "not executed: the turn paused (approval or waiting) before this call"
    };
    for call in calls {
        let Some(call_id) = call.get("id").and_then(|v| v.as_str()) else { continue };
        if answered.iter().any(|id| id == call_id) {
            continue;
        }
        history.push(json!({"role": "tool", "tool_call_id": call_id, "content": reason}));
    }
}

/// L0 (Codex/Claude Code's first defence): cap one tool result at write
/// time, keeping head+tail with a marker.
/// ponytail: fixed 50k-char cap; per-tool budgets if specific tools dominate.
const TOOL_OUTPUT_CAP: usize = 50_000;

fn cap_tool_output(content: String) -> String {
    if content.len() <= TOOL_OUTPUT_CAP {
        return content;
    }
    let half = TOOL_OUTPUT_CAP / 2;
    let head: String = content.chars().take(half).collect();
    let tail: String = content.chars().skip(content.chars().count().saturating_sub(half)).collect();
    format!("{head}\n[...{} chars truncated...]\n{tail}", content.chars().count().saturating_sub(head.chars().count() + tail.chars().count()))
}

/// L1: view-only masking of old tool outputs (Complexity Trap, arXiv
/// 2508.21433: masking is as efficient as LLM summarization). The checkpoint
/// and history tree keep the originals — only the wire copy sent to the model
/// is masked, so `read_history` can always fetch the full output back.
/// ponytail: fixed 16k trailing budget; make it window-relative if needed.
const MASK_KEEP_RECENT: usize = 16_000;

fn mask_old_tool_outputs(messages: &[Json]) -> Vec<Json> {
    let last_assistant = messages.iter().rposition(|m| m["role"] == "assistant").unwrap_or(0);
    let mut out = messages.to_vec();
    let mut budget = MASK_KEEP_RECENT;
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
            "content": format!("[tool output hidden ({len} bytes) — call read_history with tool_call_id={id:?} to retrieve it]"),
        });
    }
    out
}

/// L2 trigger: last prompt over this fraction of the configured context
/// window (Codex's model_auto_compact_token_limit defaults to ~90%).
const COMPACT_AT: f64 = 0.9;
/// ponytail: the summary request itself is capped head+tail; a history larger
/// than this summarizes the middle away before the model ever sees it.
const SUMMARY_INPUT_CAP: usize = 100_000;

const SUMMARY_PROMPT: &str = "You are compacting an agent conversation to free context space. Summarize it for continuation, in this exact structure:\n1. Goal: the user's overall objective and constraints\n2. Progress: what has been done, with key decisions and why\n3. Files: paths created/modified/read that matter, one line each\n4. Errors: unresolved errors and what was tried\n5. Tasks: pending tasks and their ids/assignees if mentioned\n6. Next: the immediate next step\nMention tool_call_ids of tool calls whose full output may be needed later. Be dense; omit small talk.\n\nConversation to compact:\n\n";

/// A private execution checkpoint, saved before tools and after each result.
/// An external call without a recorded result requires reconciliation; team
/// actions can safely replay their persisted call IDs through core receipts.
/// ponytail: full snapshots per turn; compact finalized snapshots if long
/// sessions make checkpoint storage or serialization dominate.
#[derive(Serialize, Deserialize, Default)]
struct ChatCheckpoint {
    history: Vec<Json>,
    model_steps: i64,
    pending_external: Option<String>,
    outcome: Option<TurnOutcome>,
    input_events: HashSet<String>,
    delivery_ids: HashSet<i64>,
    /// Materialized history length already committed to the tree (delta base)
    /// and its leaf. Only rewind_epoch invalidates execution identity.
    #[serde(default)]
    tree_base: usize,
    #[serde(default)]
    tree_leaf: Option<String>,
    /// Only an explicit rewind invalidates a checkpoint's execution identity.
    #[serde(default)]
    rewind_epoch: u64,
    /// Write-ahead record for a tree append, replayed idempotently on recovery.
    #[serde(default)]
    tree_pending: Vec<TreeNode>,
}

/// pi-style tree-structured conversation history (D-26): per thread an
/// append-only node tree plus the live tip in `leaf`. `rewind` moves the leaf
/// to an ancestor; new turns branch off it, so rewinding never destroys the
/// abandoned branch (unlike truncation, it is undoable).
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ChatTree {
    nodes: Vec<TreeNode>,
    leaf: Option<String>,
    #[serde(default)]
    rewind_epoch: u64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct TreeNode {
    id: String,
    parent: Option<String>,
    /// Compaction summary nodes only: resume walking at this ancestor instead
    /// of `parent`, hiding the covered range from the materialized view while
    /// keeping it in the tree (lossless compaction; rewind onto the covered
    /// branch still sees the full history).
    #[serde(default)]
    skip_to: Option<String>,
    message: Json,
}

impl ChatTree {
    /// Linear history for the model API: walk leaf -> root, reversed.
    fn materialize(&self) -> Vec<Json> {
        let mut out = vec![];
        let mut cur = self.leaf.as_deref();
        while let Some(id) = cur {
            let Some(node) = self.nodes.iter().find(|n| n.id == id) else { break };
            out.push(node.message.clone());
            cur = match &node.skip_to {
                // "" means the covered range runs to the root.
                Some(target) if target.is_empty() => None,
                Some(target) => Some(target.as_str()),
                None => node.parent.as_deref(),
            };
        }
        out.reverse();
        out
    }

    /// Chain-append messages under the current leaf; returns the new leaf.
    fn append(&mut self, messages: &[Json]) {
        for message in messages {
            let id = format!("n{}", self.nodes.len() + 1);
            self.nodes.push(TreeNode { id: id.clone(), parent: self.leaf.take(), skip_to: None, message: message.clone() });
            self.leaf = Some(id);
        }
    }

    /// Append a compaction summary covering everything above `skip_to`
    /// ("" = the whole current chain).
    fn append_summary(&mut self, message: Json, skip_to: &str) {
        let id = format!("n{}", self.nodes.len() + 1);
        self.nodes.push(TreeNode { id: id.clone(), parent: self.leaf.take(), skip_to: Some(skip_to.to_string()), message });
        self.leaf = Some(id);
    }

    /// Include earlier compactions' calls, but never an abandoned branch.
    fn tool_references(&self) -> Vec<String> {
        let mut references = vec![];
        let mut cur = self.leaf.as_deref();
        // Parents precede their children in the append-only node array.
        for node in self.nodes.iter().rev() {
            if cur != Some(node.id.as_str()) { continue; }
            for call in node.message["tool_calls"].as_array().into_iter().flatten() {
                if let Some(id) = call["id"].as_str() {
                    references.push(format!("- {id}: {}", call["function"]["name"].as_str().unwrap_or("?")));
                }
            }
            cur = node.parent.as_deref();
        }
        references.reverse();
        references
    }

    /// Rewind targets: user-input nodes, newest first, as (id, depth, preview).
    fn rewind_points(&self) -> Vec<Json> {
        let chain: Vec<&TreeNode> = {
            let mut out = vec![];
            let mut cur = self.leaf.as_deref();
            while let Some(id) = cur {
                let Some(node) = self.nodes.iter().find(|n| n.id == id) else { break };
                out.push(node);
                cur = node.parent.as_deref();
            }
            out
        };
        chain
            .iter()
            .enumerate()
            .filter(|(_, n)| n.message["role"] == "user")
            .map(|(depth, n)| {
                let preview: String = n.message["content"].as_str().unwrap_or("").chars().take(80).collect();
                json!({"id": n.id, "depth": depth, "preview": preview})
            })
            .collect()
    }

    fn rewind_to(&mut self, node_id: Option<&str>) -> Result<usize, String> {
        let depth = match node_id {
            None => {
                self.leaf = None;
                0
            }
            Some(id) => {
                if !self.nodes.iter().any(|n| n.id == id) {
                    return Err(format!("unknown history node {id:?}"));
                }
                self.leaf = Some(id.to_string());
                self.materialize().len()
            }
        };
        self.rewind_epoch = self.rewind_epoch.checked_add(1).ok_or("rewind epoch exhausted")?;
        Ok(depth)
    }
}

fn write_json_atomic(path: &std::path::Path, value: &Json) -> Result<(), String> {
    let parent = path.parent().ok_or("checkpoint parent missing")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(value.to_string().as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    std::fs::File::open(parent).and_then(|dir| dir.sync_all()).map_err(|e| e.to_string())
}

fn pending_tool_calls(history: &[Json]) -> Vec<Json> {
    let Some(index) = history.iter().rposition(|m| m["role"] == "assistant") else { return vec![] };
    let answered: HashSet<&str> = history[index + 1..].iter()
        .filter_map(|m| m.get("tool_call_id").and_then(Json::as_str)).collect();
    history[index]["tool_calls"].as_array().cloned().unwrap_or_default().into_iter()
        .filter(|c| !c["id"].as_str().map(|id| answered.contains(id)).unwrap_or(false)).collect()
}

/// Token usage of one model response, both wire protocols normalized
/// (OpenAI `usage.{prompt,completion,total}_tokens`, Anthropic
/// `usage.{input,output}_tokens` with the total synthesized).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub calls: u64,
    pub prompt: u64,
    pub completion: u64,
    pub total: u64,
    /// Prompt size of the latest call in this thread — the best proxy for
    /// current context fill (compares against ModelProfile::context_window).
    pub last_prompt: u64,
}

/// None when the provider omitted usage (some proxies do).
pub fn parse_usage(data: &Json) -> Option<(u64, u64, u64)> {
    let usage = data.get("usage")?;
    let num = |key: &str| usage.get(key).and_then(Json::as_u64);
    let (prompt, completion) = match (num("prompt_tokens"), num("completion_tokens")) {
        (Some(p), Some(c)) => (p, c),
        _ => (num("input_tokens")?, num("output_tokens")?),
    };
    let total = num("total_tokens").unwrap_or(prompt + completion);
    Some((prompt, completion, total))
}

pub struct ChatRunner {
    agent: serde_json::Value,
    profile: ModelProfile,
    workdir: Option<String>,
    notify: Arc<Notify>,
    bound: crate::bound::BoundTools,
    /// Resolved web capability (explicit binding names or the `web` umbrella).
    has_web_search: bool,
    has_web_fetch: bool,
    /// (label, content) pairs from skills/instruction files
    context: Vec<(String, String)>,
    /// Member conversation history survives a restart (USER-GUIDE §5).
    history_path: Option<std::path::PathBuf>,
    states: Mutex<HashMap<String, TurnStatus>>,
    mid_turn: Mutex<HashMap<String, Vec<Json>>>,
    interrupted: Mutex<HashSet<String>>,
    controls: Mutex<HashMap<String, Arc<TurnControl>>>,
    /// pi-style history trees per thread (D-26), the rewind/fork substrate.
    trees: Mutex<HashMap<String, ChatTree>>,
    /// Session-memory token usage per thread.
    /// ponytail: not persisted; add to the session ledger if users ask for
    /// cross-restart accounting.
    usage: Mutex<HashMap<String, Usage>>,
    /// Prompt size of the latest call across threads (current context fill).
    last_prompt: AtomicU64,
    closed: AtomicBool,
    /// At most one effort fallback per runner
    effort_max: AtomicBool,
    effort_fallback_used: AtomicBool,
    /// Claude Code's circuit breaker: stop auto-compacting for this runner
    /// after 3 consecutive failures (the failure cause may be the oversized
    /// context itself — retrying forever deadlocks the turn).
    compact_failures: AtomicU64,
}

impl ChatRunner {
    pub fn new(
        agent: &Json,
        profile: ModelProfile,
        workdir: Option<String>,
        notify: Arc<Notify>,
        bound: crate::bound::BoundTools,
        context: Vec<(String, String)>,
        web: (bool, bool),
    ) -> Arc<Self> {
        let agent_id = agent.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let history_path = (!agent_id.is_empty()).then(|| {
            crate::sessions::session_paths(&notify.core().session_id)
                .base
                .join("members")
                .join(agent_id)
                .join("chat_history.json")
        });
        Arc::new(Self {
            agent: agent.clone(),
            profile,
            workdir,
            notify,
            bound,
            has_web_search: web.0,
            has_web_fetch: web.1,
            context,
            history_path,
            trees: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            mid_turn: Mutex::new(HashMap::new()),
            interrupted: Mutex::new(HashSet::new()),
            controls: Mutex::new(HashMap::new()),
            usage: Mutex::new(HashMap::new()),
            last_prompt: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            effort_max: AtomicBool::new(false),
            effort_fallback_used: AtomicBool::new(false),
            compact_failures: AtomicU64::new(0),
        })
    }

    /// Conversation history from disk: `{thread_id: [messages]}`. Best effort —
    /// an unreadable file degrades to a fresh conversation, never a failure.
    fn load_history(&self, thread: &str) -> Vec<Json> {
        let Some(path) = &self.history_path else { return vec![] };
        let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
        let Ok(data) = serde_json::from_str::<Json>(&text) else { return vec![] };
        data.get(thread)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    }

    /// Atomic write (tmp + rename) so a crash never truncates the history.
    /// ponytail: the whole conversation is kept, unbounded like the in-memory
    /// map; trim to a message window if a long session ever hits provider limits.
    fn save_history(&self, thread: &str, history: &[Json]) -> Result<(), String> {
        let path = self.history_path.as_ref().ok_or("member history path missing")?;
        let mut data = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str::<Json>(&text).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.to_string()),
        };
        if !data.is_object() { return Err("invalid member history".into()); }
        data[thread] = Json::Array(history.to_vec());
        write_json_atomic(path, &data)
    }

    fn tree_path(&self) -> Option<std::path::PathBuf> {
        self.history_path.as_ref().map(|p| p.with_file_name("chat_tree.json"))
    }

    /// Tree file is `{thread: {nodes, leaf}}` per member. A member with only a
    /// legacy linear chat_history.json migrates lazily: chain it in memory,
    /// freeze the migration before the first execution checkpoint is written.
    fn load_tree(&self, thread: &str) -> Result<ChatTree, String> {
        if let Some(tree) = self.trees.lock().unwrap().get(thread) {
            return Ok(tree.clone());
        }
        if let Some(path) = self.tree_path() {
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let data: Json = serde_json::from_str(&text).map_err(|e| format!("invalid history tree: {e}"))?;
                    if !data.is_object() { return Err("invalid member history tree".into()); }
                    if let Some(value) = data.get(thread) {
                        return serde_json::from_value(value.clone()).map_err(|e| format!("invalid history tree: {e}"));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut tree = ChatTree::default();
        tree.append(&self.load_history(thread));
        Ok(tree)
    }

    fn save_tree(&self, thread: &str, tree: &ChatTree) -> Result<(), String> {
        let Some(path) = self.tree_path() else { return Ok(()) };
        let mut data = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Json>(&text).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
            Err(e) => return Err(e.to_string()),
        };
        if !data.is_object() { return Err("invalid member history tree".into()); }
        data[thread] = serde_json::to_value(tree).map_err(|e| e.to_string())?;
        write_json_atomic(&path, &data)?;
        self.trees.lock().unwrap().insert(thread.to_string(), tree.clone());
        Ok(())
    }

    /// Rewind targets for a thread (user inputs, newest first) — the /rewind picker.
    pub fn rewind_points(&self, thread: &str) -> Vec<Json> {
        self.load_tree(thread).map(|t| t.rewind_points()).unwrap_or_default()
    }

    /// Move the thread's live tip to `node_id` (None = empty conversation).
    /// The abandoned branch stays in the tree. Only this explicit epoch change
    /// discards checkpoints; a partially committed append must be recovered.
    pub fn rewind(&self, thread: &str, node_id: Option<&str>) -> Result<usize, String> {
        let mut tree = self.load_tree(thread)?;
        let depth = tree.rewind_to(node_id)?;
        self.save_tree(thread, &tree)?;
        Ok(depth)
    }

    /// Directory holding this member's history files (fork copies the tree file).
    pub fn history_dir(&self) -> Option<std::path::PathBuf> {
        self.history_path.as_ref().and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    fn checkpoint_path(&self, run: &TurnRun) -> Result<std::path::PathBuf, String> {
        if run.run_id.is_empty() || !run.run_id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return Err("invalid checkpoint run id".into());
        }
        Ok(self.history_path.as_ref().and_then(|p| p.parent()).ok_or("member history path missing")?
            .join("turns").join(format!("{}.json", run.run_id)))
    }

    fn load_checkpoint(&self, run: &TurnRun) -> Result<Option<ChatCheckpoint>, String> {
        match std::fs::read(self.checkpoint_path(run)?) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("invalid turn checkpoint: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn save_checkpoint(&self, run: &TurnRun, checkpoint: &ChatCheckpoint, gateway: &ToolGateway) -> Result<(), (String, String)> {
        let _execution = gateway.control.enter().map_err(|e| ("TurnInterrupted".into(), e))?;
        self.write_checkpoint(run, checkpoint).map_err(|e| ("CheckpointError".into(), e))
    }

    /// Caller holds the turn's execution guard across all private-file writes.
    fn write_checkpoint(&self, run: &TurnRun, checkpoint: &ChatCheckpoint) -> Result<(), String> {
        write_json_atomic(&self.checkpoint_path(run)?, &serde_json::to_value(checkpoint).map_err(|e| e.to_string())?)?;
        self.save_history(run.context_ref.as_deref().unwrap_or(&run.run_id), &checkpoint.history)
    }

    fn commit_tree(&self, run: &TurnRun, checkpoint: &mut ChatCheckpoint, tree: &ChatTree, base: usize, control: &TurnControl) -> Result<(), String> {
        let _execution = control.enter()?;
        checkpoint.tree_pending = tree.nodes[base..].to_vec();
        checkpoint.tree_base = checkpoint.history.len();
        checkpoint.tree_leaf = tree.leaf.clone();
        checkpoint.rewind_epoch = tree.rewind_epoch;
        self.write_checkpoint(run, checkpoint)?;
        self.save_tree(run.context_ref.as_deref().unwrap_or(&run.run_id), tree)?;
        checkpoint.tree_pending.clear();
        self.write_checkpoint(run, checkpoint)
    }

    /// Recover either side of the journal -> atomic tree rename boundary.
    /// Caller holds the execution guard; mismatched content fails closed.
    fn restore_tree_commit(&self, run: &TurnRun, checkpoint: &mut ChatCheckpoint, tree: &mut ChatTree) -> Result<(), String> {
        let Some(first) = checkpoint.tree_pending.first() else { return Ok(()) };
        if checkpoint.tree_pending.last().map(|n| &n.id) != checkpoint.tree_leaf.as_ref() {
            return Err("invalid pending tree leaf".into());
        }
        if tree.leaf != checkpoint.tree_leaf || !tree.nodes.ends_with(&checkpoint.tree_pending) {
            if tree.leaf != first.parent || checkpoint.tree_pending.iter().any(|n| tree.nodes.iter().any(|old| old.id == n.id)) {
                return Err("pending history commit conflicts with the tree".into());
            }
            tree.nodes.extend(checkpoint.tree_pending.iter().cloned());
            tree.leaf = checkpoint.tree_leaf.clone();
            self.save_tree(run.context_ref.as_deref().unwrap_or(&run.run_id), tree)?;
        }
        checkpoint.tree_pending.clear();
        self.write_checkpoint(run, checkpoint)
    }

    fn record_usage(&self, thread: &str, data: &Json) {
        let Some((prompt, completion, total)) = parse_usage(data) else { return };
        let mut usage = self.usage.lock().unwrap();
        let entry = usage.entry(thread.to_string()).or_default();
        entry.calls += 1;
        entry.prompt += prompt;
        entry.completion += completion;
        entry.total += total;
        entry.last_prompt = prompt;
        self.last_prompt.store(prompt, Ordering::SeqCst);
    }

    /// Aggregated per-agent counters over all threads of this runner.
    pub fn usage_snapshot(&self) -> Json {
        let mut total = Usage::default();
        for entry in self.usage.lock().unwrap().values() {
            total.calls += entry.calls;
            total.prompt += entry.prompt;
            total.completion += entry.completion;
            total.total += entry.total;
        }
        json!({
            "calls": total.calls,
            "prompt_tokens": total.prompt,
            "completion_tokens": total.completion,
            "total_tokens": total.total,
            "last_prompt_tokens": self.last_prompt.load(Ordering::SeqCst),
        })
    }

    fn bindings(&self) -> Vec<String> {
        self.agent
            .get("tool_bindings")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    fn agent_id(&self) -> String {
        self.agent.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string()
    }

    /// Whether the member's resolved bindings include each web tool.
    fn web_flags(&self) -> (bool, bool) {
        (self.has_web_search, self.has_web_fetch)
    }

    fn system_prompt(&self) -> String {
        let name = self.agent.get("name").and_then(|v| v.as_str()).unwrap_or("member");
        let role = self.agent.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
        let instructions = self.agent.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
        let head = if instructions.is_empty() {
            format!("You are {name}, role {role}, in a team.")
        } else {
            instructions.to_string()
        };
        let allowed = bound_tool_names(&self.bindings(), self.web_flags());
        let bound_docs = self.bound.docs();
        let tools = TEAM_TOOL_DOCS
            .iter()
            .map(|(n, d)| (*n, *d))
            .chain(BOUND_TOOL_DOCS.iter().filter(|(n, _)| allowed.contains(n)).map(|(n, d)| (*n, *d)))
            .chain(bound_docs.iter().map(|(n, d)| (n.as_str(), d.as_str())))
            .map(|(n, d)| format!("- {n}: {d}"))
            .collect::<Vec<_>>()
            .join("\n");
        let context = if self.context.is_empty() {
            String::new()
        } else {
            let mut blocks = String::new();
            for (label, content) in &self.context {
                blocks.push_str(&format!("\n<{label}>\n{content}\n</{}>\n", label.split(' ').next().unwrap_or("context")));
            }
            blocks
        };
        format!(
            "{head}\n\nTeam tools available:\n{tools}\n\nRules: use complete_task to finish your assigned task; use signal_done only when the whole user goal is complete (Leader only).\n{context}"
        )
    }

    /// The system prompt is refreshed from the agent config on every segment
    /// (and after compaction rebases the history from the tree).
    fn refresh_system(&self, history: &mut Vec<Json>) {
        let instructions = self.agent["instructions"].as_str().unwrap_or("");
        if history.first().map(|m| m["role"] == "system").unwrap_or(false) {
            history[0]["content"] = json!(self.system_prompt());
        } else if !instructions.is_empty() || !self.context.is_empty() {
            history.insert(0, json!({"role":"system", "content":self.system_prompt()}));
        }
    }

    fn has_paused(&self, run_id: &str) -> bool {
        self.closed.load(Ordering::SeqCst) || self.interrupted.lock().unwrap().contains(run_id)
    }

    /// generation_options for the request body, with the effort rewritten when
    /// the provider rejected it once (`_switch_effort_to_max`).
    fn apply_generation_options(&self, body: &mut Json) {
        let Json::Object(map) = body else { return };
        for (key, value) in &self.profile.generation_options {
            if key == "reasoning_effort" {
                if let Some(effort) = value.as_str() {
                    let effort = if self.effort_max.load(Ordering::SeqCst) {
                        "max".to_string()
                    } else {
                        normalize_effort(&self.profile.protocol, effort)
                    };
                    map.insert(key.clone(), json!(effort));
                    continue;
                }
            }
            map.insert(key.clone(), value.clone());
        }
    }

    fn configured_effort(&self) -> bool {
        self.profile
            .generation_options
            .get("reasoning_effort")
            .map(|v| v.is_string())
            .unwrap_or(false)
    }

    /// The session TeamSpec limit for model requests per turn (read once per turn).
    fn max_model_steps(&self) -> i64 {
        self.notify
            .core()
            .state_brief()
            .ok()
            .and_then(|state| {
                state
                    .get("limits")
                    .and_then(|limits| limits.get("max_model_steps_per_turn"))
                    .and_then(|v| v.as_i64())
            })
            .unwrap_or(200)
    }

    fn chat(&self, thread: &str, messages: &[Json], tools: &Json, control: &TurnControl) -> Result<Json, String> {
        if self.profile.protocol == "anthropic" {
            return self.chat_anthropic(thread, messages, tools, control);
        }
        let api_key = match &self.profile.api_key_env {
            Some(env) => std::env::var(env).map_err(|_| format!("missing API key env {env}"))?,
            None => String::new(),
        };
        let base = resolve_base_url(&self.profile);
        let mut body = json!({
            "model": self.profile.model,
            "messages": messages,
            "tools": tools,
        });
        self.apply_generation_options(&mut body);
        let url = format!("{base}/chat/completions");
        let mut last_error = "chat call failed".to_string();
        let retries = self.profile.max_retries.max(0);
        for attempt in 0..=retries {
            control.check()?;
            let mut request = ureq::post(&url)
                .set("content-type", "application/json")
                .timeout(std::time::Duration::from_secs(self.profile.timeout.max(1) as u64));
            if !api_key.is_empty() {
                request = request.set("authorization", &format!("Bearer {api_key}"));
            }
            let mut retry_in: Option<std::time::Duration> = None;
            match request.send_string(&body.to_string()) {
                Ok(response) => match response.into_json::<Json>() {
                    Ok(data) => {
                        self.record_usage(thread, &data);
                        let message = data
                            .get("choices")
                            .and_then(|c| c.get(0))
                            .and_then(|c| c.get("message"))
                            .cloned()
                            .ok_or_else(|| "chat API: empty choices".to_string())?;
                        return Ok(message);
                    }
                    Err(e) => last_error = format!("chat API: bad json: {e}"),
                },
                Err(ureq::Error::Status(code, response)) => {
                    let retry_after = response
                        .header("retry-after")
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(std::time::Duration::from_secs);
                    let text = response.into_string().unwrap_or_default();
                    let text: String = text.chars().take(500).collect();
                    last_error = format!("chat API {code}: {text}");
                    if !retryable_status(code) {
                        return Err(last_error);
                    }
                    retry_in = retry_after;
                }
                Err(e) => last_error = format!("chat API: {e}"),
            }
            // never sleep after the final attempt
            if attempt < retries {
                let backoff = retry_in
                    .unwrap_or_else(|| std::time::Duration::from_millis((500u64 << attempt.min(5)).min(8000)));
                std::thread::sleep(backoff.min(std::time::Duration::from_secs(30)));
            }
        }
        Err(last_error)
    }

    /// Anthropic Messages API.
    /// ponytail: text/tool_use/tool_result blocks only — no images or thinking blocks.
    fn chat_anthropic(&self, thread: &str, messages: &[Json], tools: &Json, control: &TurnControl) -> Result<Json, String> {
        let api_key = match &self.profile.api_key_env {
            Some(env) => std::env::var(env).map_err(|_| format!("missing API key env {env}"))?,
            None => String::new(),
        };
        let base = self
            .profile
            .base_url
            .clone()
            .unwrap_or_else(|| "https://api.anthropic.com".into())
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .to_string();
        let (system, converted) = to_anthropic_messages(messages);
        let tool_specs: Vec<Json> = tools
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|tool| {
                let f = tool.get("function")?;
                Some(json!({
                    "name": f.get("name")?,
                    "description": f.get("description").cloned().unwrap_or(Json::Null),
                    "input_schema": f.get("parameters").cloned().unwrap_or(json!({"type": "object"})),
                }))
            })
            .collect();
        let mut body = json!({
            "model": self.profile.model,
            "max_tokens": self.profile.generation_options.get("max_tokens").and_then(|v| v.as_i64()).unwrap_or(8192),
            "messages": converted,
        });
        if !system.is_empty() {
            body["system"] = json!(system);
        }
        if !tool_specs.is_empty() {
            body["tools"] = json!(tool_specs);
        }
        for (key, value) in &self.profile.generation_options {
            if key != "max_tokens" && key != "reasoning_effort" {
                body[key] = value.clone();
            }
        }
        let mut options = json!({});
        self.apply_generation_options(&mut options);
        if let Some(effort) = options.get("reasoning_effort") {
            if body["output_config"].is_null() { body["output_config"] = json!({}); }
            body["output_config"]["effort"] = effort.clone();
        }
        let url = format!("{base}/v1/messages");
        let mut last_error = "chat call failed".to_string();
        let retries = self.profile.max_retries.max(0);
        for attempt in 0..=retries {
            control.check()?;
            let mut request = ureq::post(&url)
                .set("content-type", "application/json")
                .set("anthropic-version", "2023-06-01")
                .timeout(std::time::Duration::from_secs(self.profile.timeout.max(1) as u64));
            if !api_key.is_empty() {
                request = request.set("x-api-key", &api_key);
            }
            let mut retry_in: Option<std::time::Duration> = None;
            match request.send_string(&body.to_string()) {
                Ok(response) => match response.into_json::<Json>() {
                    Ok(data) => {
                        self.record_usage(thread, &data);
                        return Ok(from_anthropic_message(&data));
                    }
                    Err(e) => last_error = format!("chat API: bad json: {e}"),
                },
                Err(ureq::Error::Status(code, response)) => {
                    let retry_after = response
                        .header("retry-after")
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(std::time::Duration::from_secs);
                    let text = response.into_string().unwrap_or_default();
                    let text: String = text.chars().take(500).collect();
                    last_error = format!("chat API {code}: {text}");
                    if !retryable_status(code) {
                        return Err(last_error);
                    }
                    retry_in = retry_after;
                }
                Err(e) => last_error = format!("chat API: {e}"),
            }
            if attempt < retries {
                let backoff = retry_in
                    .unwrap_or_else(|| std::time::Duration::from_millis((500u64 << attempt.min(5)).min(8000)));
                std::thread::sleep(backoff.min(std::time::Duration::from_secs(30)));
            }
        }
        Err(last_error)
    }

    /// L2 trigger (pre-turn/between-steps only — never with tool calls
    /// pending, so no assistant message is orphaned mid-batch).
    fn over_threshold(&self, thread: &str) -> bool {
        let Some(window) = self.profile.context_window else { return false };
        if self.compact_failures.load(Ordering::SeqCst) >= 3 {
            return false;
        }
        let last = self.usage.lock().unwrap().get(thread).map(|u| u.last_prompt).unwrap_or(0);
        last > 0 && last as f64 > window as f64 * COMPACT_AT
    }

    /// Codex-style handoff compaction: one LLM call condenses the history to a
    /// structured summary node with `skip_to` set, so the covered messages
    /// stay in the tree (lossless — /rewind and read_history can still reach
    /// them) while materialize() jumps over them.
    fn compact(&self, run: &TurnRun, checkpoint: &mut ChatCheckpoint, control: &TurnControl) -> Result<(), (String, String)> {
        let thread = run.context_ref.as_deref().unwrap_or(&run.run_id);
        let mut tree = self.load_tree(thread).map_err(|e| ("CheckpointError".into(), e))?;
        let base = tree.nodes.len();
        // The tail and summary are committed together after the model reply.
        if checkpoint.history.len() > checkpoint.tree_base {
            tree.append(&checkpoint.history[checkpoint.tree_base..]);
        }
        let mut blob = String::new();
        for message in checkpoint.history.iter().skip_while(|m| m["role"] == "system") {
            let role = message["role"].as_str().unwrap_or("?");
            let mut content = message["content"].as_str().unwrap_or("").to_string();
            if content.len() > 2_000 {
                content = format!("{}…[{} chars]", content.chars().take(2_000).collect::<String>(), content.len());
            }
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    if let Some(id) = call["id"].as_str() {
                        let name = call["function"]["name"].as_str().unwrap_or("?");
                        let args: String = call["function"]["arguments"].as_str().unwrap_or("").chars().take(200).collect();
                        content.push_str(&format!("\n[tool_call_id={id}: {name} {args}]"));
                    }
                }
            }
            if let Some(id) = message["tool_call_id"].as_str() {
                content = format!("[tool_call_id={id}] {content}");
            }
            blob.push_str(&format!("{role}: {content}\n\n"));
        }
        if blob.len() > SUMMARY_INPUT_CAP {
            let half = SUMMARY_INPUT_CAP / 2;
            blob = format!(
                "{}\n[...middle omitted...]\n{}",
                blob.chars().take(half).collect::<String>(),
                blob.chars().skip(blob.chars().count().saturating_sub(half)).collect::<String>()
            );
        }
        // Keep the covered calls discoverable even when the model omits IDs
        // from its prose or the summary input's middle was truncated.
        // ponytail: inline the full live-branch index; paginate if IDs alone
        // become a significant part of the model's context window.
        let index = format!("Tool output index (read_history tool_call_id):\n{}", tree.tool_references().join("\n"));
        let ask = vec![json!({"role": "user", "content": format!("{SUMMARY_PROMPT}{blob}\n{index}")})];
        let reply = self.chat(thread, &ask, &json!([]), control).map_err(|e| ("ChatError".into(), e))?;
        let summary = reply["content"].as_str().unwrap_or("").trim().to_string();
        if summary.is_empty() {
            return Err(("ChatError".into(), "compaction returned an empty summary".into()));
        }
        // Keep the system root verbatim; everything else is covered.
        let keep = tree.nodes.first().filter(|n| n.message["role"] == "system").map(|n| n.id.clone()).unwrap_or_default();
        tree.append_summary(
            json!({"role": "user", "content": format!("[Compacted conversation summary]\n{summary}\n\n{index}\n\n[Earlier tool outputs and replies were removed from context. Call read_history with a tool_call_id to retrieve a tool output.]")}),
            &keep,
        );
        checkpoint.history = tree.materialize();
        self.refresh_system(&mut checkpoint.history);
        self.commit_tree(run, checkpoint, &tree, base, control).map_err(|e| ("CheckpointError".into(), e))?;
        // The summary call's huge prompt must not retrigger compaction; the
        // next real call overwrites this with the true value.
        if let Some(entry) = self.usage.lock().unwrap().get_mut(thread) {
            entry.last_prompt = (summary.len() / 4 + 1024) as u64;
        }
        Ok(())
    }

    /// ④ read-back pointer: the original output of an earlier tool call,
    /// looked up in the live history first, then every branch of the tree.
    fn read_history(&self, thread: &str, history: &[Json], tool_call_id: &str) -> Result<Json, String> {
        if tool_call_id.is_empty() {
            return Err("read_history requires tool_call_id".into());
        }
        for message in history {
            if message["tool_call_id"].as_str() == Some(tool_call_id) {
                return Ok(json!({"output": message["content"].as_str().unwrap_or("")}));
            }
        }
        let tree = self.load_tree(thread)?;
        for node in &tree.nodes {
            if node.message["tool_call_id"].as_str() == Some(tool_call_id) {
                return Ok(json!({"output": node.message["content"].as_str().unwrap_or("")}));
            }
        }
        Err(format!("no tool output recorded for tool_call_id {tool_call_id:?}"))
    }

    fn run_loop(
        &self, run: &TurnRun, checkpoint: &mut ChatCheckpoint, gateway: &ToolGateway,
        view: &Json, wake: &Json,
    ) -> Result<String, (String, String)> {
        let tools = tools_payload(&self.bindings(), self.web_flags(), &self.bound.schemas());
        let max_steps = self.max_model_steps();
        let mut input = Some(view.clone());
        loop {
            if self.has_paused(&run.run_id) || gateway.control.check().is_err() {
                return Err(("TurnInterrupted".into(), "interrupted".into()));
            }
            let calls = pending_tool_calls(&checkpoint.history);
            if calls.is_empty() {
                // Includes recovery after the final model response was saved but
                // before the runtime archived its outcome. Never ask again.
                if let Some(last) = checkpoint.history.last().filter(|m| m["role"] == "assistant") {
                    if last["tool_calls"].as_array().map(|c| c.is_empty()).unwrap_or(true) {
                        return Ok(last["content"].as_str().unwrap_or("").to_string());
                    }
                }
                if let Some(input) = input.take() {
                    self.append_input(checkpoint, input, wake, false);
                }
                let mid = self.mid_turn.lock().unwrap().remove(&run.run_id).unwrap_or_default();
                if !mid.is_empty() {
                    self.append_input(checkpoint, json!({"inbox_delta": mid}), &Json::Null, false);
                }
                if checkpoint.model_steps >= max_steps {
                    return Err(("TurnLimitExceeded".into(), format!("model-step limit {max_steps} reached for this turn")));
                }
                let thread = run.context_ref.as_deref().unwrap_or(&run.run_id);
                if self.over_threshold(thread) {
                    match self.compact(run, checkpoint, &gateway.control) {
                        Ok(()) => {
                            self.compact_failures.store(0, Ordering::SeqCst);
                            self.save_checkpoint(run, checkpoint, gateway)?;
                        }
                        Err((kind, message)) if kind == "CheckpointError" => return Err((kind, message)),
                        // A failed model summary must not kill the turn: continue
                        // uncompacted and let any provider error surface; the
                        // breaker stops hammering after 3 failures.
                        Err(_) => {
                            self.compact_failures.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                }
                checkpoint.model_steps += 1;
                self.save_checkpoint(run, checkpoint, gateway)?;
                let wire = mask_old_tool_outputs(&checkpoint.history);
                let message = match self.chat(thread, &wire, &tools, &gateway.control) {
                    Ok(message) => message,
                    Err(e) if looks_like_effort_error(&e) && self.configured_effort()
                        && !self.effort_fallback_used.swap(true, Ordering::SeqCst) => {
                        self.effort_max.store(true, Ordering::SeqCst);
                        self.chat(thread, &wire, &tools, &gateway.control).map_err(|e| ("ChatError".into(), e))?
                    }
                    Err(e) => return Err(("ChatError".into(), e)),
                };
                checkpoint.history.push(message.clone());
                // Persist model-assigned tool IDs BEFORE any team/external call.
                self.save_checkpoint(run, checkpoint, gateway)?;
                if let Some(text) = message["content"].as_str() {
                    if !self.has_paused(&run.run_id) && gateway.control.check().is_ok() {
                        self.notify.note_stream_chunk(&run.run_id, &self.agent_id(), text);
                    }
                }
                continue;
            }
            for call in calls {
                if self.has_paused(&run.run_id) || gateway.control.check().is_err() {
                    return Err(("TurnInterrupted".into(), "interrupted".into()));
                }
                let call_id = call["id"].as_str().filter(|id| !id.is_empty())
                    .ok_or_else(|| ("ChatError".to_string(), "tool call id missing".to_string()))?.to_string();
                let name = call["function"]["name"].as_str().unwrap_or("");
                let arguments = call["function"]["arguments"].as_str().unwrap_or("{}");
                let args: Json = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(_) => {
                        checkpoint.history.push(json!({"role":"tool", "tool_call_id":call_id, "content":"invalid JSON arguments"}));
                        self.save_checkpoint(run, checkpoint, gateway)?;
                        continue;
                    }
                };
                if !TEAM_TOOLS.contains(&name) {
                    checkpoint.pending_external = Some(call_id.clone());
                    self.save_checkpoint(run, checkpoint, gateway)?;
                }
                let thread = run.context_ref.as_deref().unwrap_or(&run.run_id);
                let receipt = if name == "read_history" {
                    match self.read_history(thread, &checkpoint.history, args["tool_call_id"].as_str().unwrap_or("")) {
                        Ok(result) => teamagents_core::models::Receipt {
                            action_id: call_id.clone(), ok: true, kind: teamagents_core::models::ActionKind::CompleteTask,
                            result, error: None,
                        },
                        Err(e) => teamagents_core::models::Receipt {
                            action_id: call_id.clone(), ok: false, kind: teamagents_core::models::ActionKind::CompleteTask,
                            result: json!({}), error: Some(e),
                        },
                    }
                } else if self.bound.names().contains(name) {
                    let _execution = gateway.control.enter().map_err(|e| ("TurnInterrupted".into(), e))?;
                    match self.bound.call(name, &args).expect("bound tool has a client") {
                        Ok(output) => teamagents_core::models::Receipt {
                            action_id: call_id.clone(), ok: true, kind: teamagents_core::models::ActionKind::CompleteTask,
                            result: json!({"output":output}), error: None,
                        },
                        Err(e) => teamagents_core::models::Receipt {
                            action_id: call_id.clone(), ok: false, kind: teamagents_core::models::ActionKind::CompleteTask,
                            result: json!({}), error: Some(e),
                        },
                    }
                } else { gateway.call(name, &args, &call_id) };
                checkpoint.pending_external = None;
                let approval = receipt.error.as_deref() == Some("approval_required");
                let waiting = name == "wait_for_tasks" && receipt.result["waiting"].as_bool().unwrap_or(false);
                let step_limit = receipt.error.as_deref().map(|e| e.contains("step limit")).unwrap_or(false);
                let content = if receipt.ok { receipt.result.to_string() } else { json!({"error":receipt.error}).to_string() };
                checkpoint.history.push(json!({"role":"tool", "tool_call_id":call_id, "content":cap_tool_output(content)}));
                if approval || waiting || step_limit {
                    let remaining = pending_tool_calls(&checkpoint.history);
                    fill_unanswered_tool_calls(&mut checkpoint.history, &remaining, &[], "TurnPaused");
                    if !step_limit {
                        let status = if approval { TurnStatus::WaitingApproval } else { TurnStatus::WaitingTask };
                        let note = if approval { receipt.result["approval_id"].as_str().unwrap_or("").to_string() } else { "waiting".into() };
                        checkpoint.outcome = Some(TurnOutcome { status, error: None, note: Some(note.clone()), reply_text: None });
                        self.save_checkpoint(run, checkpoint, gateway)?;
                        return Err(("TurnPaused".into(), note));
                    }
                }
                self.save_checkpoint(run, checkpoint, gateway)?;
                if step_limit { return Err(("TurnLimitExceeded".into(), receipt.error.unwrap_or_default())); }
            }
        }
    }

    fn append_input(&self, checkpoint: &mut ChatCheckpoint, mut view: Json, wake: &Json, force: bool) {
        let items = view["inbox_delta"].as_array().cloned().unwrap_or_default();
        let fresh: Vec<Json> = items.into_iter().filter(|item| {
            if let Some(id) = item["event_id"].as_str() {
                if !checkpoint.input_events.insert(id.to_string()) { return false; }
            }
            if let Some(id) = item["delivery_id"].as_i64() { checkpoint.delivery_ids.insert(id); }
            true
        }).collect();
        if force || !fresh.is_empty() {
            view["inbox_delta"] = json!(fresh);
            checkpoint.history.push(json!({"role":"user", "content":render_view(&view, wake, self.workdir.as_deref())}));
        }
    }

    fn run_segment(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> Result<TurnOutcome, (String, String)> {
        let loaded = self.load_checkpoint(run).map_err(|e| ("CheckpointError".into(), e))?;
        let thread = run.context_ref.as_deref().unwrap_or(&run.run_id);
        let mut tree = self.load_tree(thread).map_err(|e| ("CheckpointError".into(), e))?;
        // An epoch changes only on explicit rewind; a leaf mismatch alone can
        // instead be a partially committed segment and must never cause replay.
        let mut loaded = loaded.filter(|cp| cp.rewind_epoch == tree.rewind_epoch);
        {
            let _execution = gateway.control.enter().map_err(|e| ("TurnInterrupted".into(), e))?;
            if let Some(checkpoint) = &mut loaded {
                self.restore_tree_commit(run, checkpoint, &mut tree).map_err(|e| ("CheckpointError".into(), e))?;
                if checkpoint.tree_leaf != tree.leaf {
                    return Err(("CheckpointError".into(), "history tree differs from checkpoint without an explicit rewind".into()));
                }
            }
            // Freeze a legacy/empty tree BEFORE chat_history receives this turn.
            if !self.trees.lock().unwrap().contains_key(thread) {
                self.save_tree(thread, &tree).map_err(|e| ("CheckpointError".into(), e))?;
            }
        }
        let fresh = loaded.is_none();
        let mut checkpoint = loaded.unwrap_or_default();
        if checkpoint.pending_external.is_some() {
            return Err(("OutcomeUnknown".into(), "external tool result missing; inspect its side effects before retrying".into()));
        }
        if let Some(outcome) = checkpoint.outcome.clone().filter(|o| o.status.is_terminal()) {
            self.save_checkpoint(run, &checkpoint, gateway)?;
            return Ok(outcome);
        }
        let resumed = checkpoint.outcome.take().is_some();
        if fresh {
            checkpoint.history = tree.materialize();
            checkpoint.tree_base = checkpoint.history.len();
            checkpoint.tree_leaf = tree.leaf.clone();
            checkpoint.rewind_epoch = tree.rewind_epoch;
            // A different, terminated turn may have left an unanswered tool in
            // the shared thread. It must not become work for this new run ID.
            for call in pending_tool_calls(&checkpoint.history) {
                checkpoint.history.push(json!({"role":"tool", "tool_call_id":call["id"],
                    "content":"Previous turn ended without a recorded result; outcome unknown. Inspect before retrying."}));
            }
        }
        let before_refresh = checkpoint.history.len();
        self.refresh_system(&mut checkpoint.history);
        if checkpoint.tree_base > 0 {
            checkpoint.tree_base += checkpoint.history.len() - before_refresh;
        }
        // New turns/explicit resumes need their new input before examining an
        // old final assistant message from the preceding turn/segment.
        if fresh || resumed {
            self.append_input(&mut checkpoint, view.clone(), wake, true);
            self.save_checkpoint(run, &checkpoint, gateway)?;
        }
        let result = self.run_loop(run, &mut checkpoint, gateway, view, wake);
        let outcome = match result {
            Ok(reply) => TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: Some(reply) },
            Err((name, _)) if name == "TurnPaused" => checkpoint.outcome.clone().expect("pause checkpoint"),
            Err((name, message)) if name == "TurnLimitExceeded" => TurnOutcome {
                status: TurnStatus::Failed, error: Some(message), note: Some("turn_limit".into()), reply_text: None,
            },
            Err(e) => return Err(e),
        };
        checkpoint.outcome = Some(outcome.clone());
        // Journal the append before the tree rename; either crash boundary
        // recovers the same outcome and IDs without asking the model again.
        let mut tree = self.load_tree(thread).map_err(|e| ("CheckpointError".into(), e))?;
        let base = tree.nodes.len();
        if checkpoint.history.len() > checkpoint.tree_base {
            tree.append(&checkpoint.history[checkpoint.tree_base..]);
        }
        self.commit_tree(run, &mut checkpoint, &tree, base, &gateway.control).map_err(|e| ("CheckpointError".into(), e))?;
        Ok(outcome)
    }
}

impl AgentRunner for ChatRunner {
    fn start_or_resume(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> TurnOutcome {
        {
            let mut controls = self.controls.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) { gateway.control.cancel(); }
            controls.insert(run.run_id.clone(), gateway.control.clone());
        }
        self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Running);
        self.interrupted.lock().unwrap().remove(&run.run_id);
        let outcome = match self.run_segment(run, view, gateway, wake) {
            Ok(outcome) => outcome,
            Err((name, message)) => TurnOutcome {
                status: match name.as_str() {
                    "TurnInterrupted" => TurnStatus::Cancelled,
                    "OutcomeUnknown" | "CheckpointError" => TurnStatus::OutcomeUnknown,
                    _ => TurnStatus::Failed,
                },
                error: Some(format!("{name}: {message}")), note: None, reply_text: None,
            },
        };
        self.states.lock().unwrap().insert(run.run_id.clone(), outcome.status);
        self.controls.lock().unwrap().remove(&run.run_id);
        if !self.closed.load(Ordering::SeqCst) { self.notify.wake(); }
        outcome
    }

    fn request_interrupt(&self, run_id: &str) -> TurnStatus {
        self.interrupted.lock().unwrap().insert(run_id.to_string());
        let control = self.controls.lock().unwrap().get(run_id).cloned();
        if let Some(control) = control {
            control.cancel();
            let timeout = self.notify.core().state_brief().ok()
                .and_then(|s| s["limits"]["cancel_confirm_timeout_s"].as_u64()).unwrap_or(60);
            if !control.wait_idle(std::time::Duration::from_secs(timeout)) {
                return TurnStatus::OutcomeUnknown;
            }
        }
        self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::Cancelled);
        TurnStatus::Cancelled
    }

    fn query_state(&self, run_id: &str) -> Option<TurnStatus> {
        self.states.lock().unwrap().get(run_id).copied()
    }

    fn rewind_points(&self, thread: &str) -> Vec<Json> {
        ChatRunner::rewind_points(self, thread)
    }

    fn rewind(&self, thread: &str, node: Option<&str>) -> Result<usize, String> {
        ChatRunner::rewind(self, thread, node)
    }

    fn applied_delivery_ids(&self, run: &TurnRun) -> Option<Vec<i64>> {
        Some(self.load_checkpoint(run).ok().flatten()
            .map(|c| c.delivery_ids.into_iter().collect()).unwrap_or_default())
    }

    fn reconcile(&self, run: &TurnRun) -> Option<TurnStatus> {
        Some(match self.load_checkpoint(run) {
            Ok(Some(checkpoint)) if checkpoint.pending_external.is_none() => {
                if matches!(run.status, TurnStatus::WaitingTask | TurnStatus::WaitingApproval) {
                    run.status
                } else { TurnStatus::Queued }
            }
            // Old/corrupt/missing checkpoints cannot prove a safe replay.
            _ => TurnStatus::OutcomeUnknown,
        })
    }

    fn deliver_mid_turn(&self, run_id: &str, items: Vec<Json>) {
        self.mid_turn.lock().unwrap().entry(run_id.to_string()).or_default().extend(items);
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let controls: Vec<_> = self.controls.lock().unwrap().values().cloned().collect();
        for control in &controls { control.cancel(); }
        self.bound.close();
        // Also drain a tool whose timeout already removed its runtime wrapper.
        for control in controls {
            while !control.wait_idle(std::time::Duration::from_millis(50)) {}
        }
    }
}

/// OpenAI-style history → (system prompt, Anthropic messages).
fn to_anthropic_messages(history: &[Json]) -> (String, Vec<Json>) {
    let mut system = String::new();
    let mut out: Vec<Json> = vec![];
    for message in history {
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "system" => {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(content);
            }
            "user" => {
                // consecutive tool results must share one user message (API rule)
                if message.get("tool_call_id").is_none() {
                    out.push(json!({"role": "user", "content": [{"type": "text", "text": content}]}));
                }
            }
            "assistant" => {
                let calls = message.get("tool_calls").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                let mut blocks: Vec<Json> = vec![];
                if !content.is_empty() {
                    blocks.push(json!({"type": "text", "text": content}));
                }
                for call in calls {
                    let name = call.get("function").and_then(|f| f.get("name")).and_then(|v| v.as_str()).unwrap_or("");
                    let arguments = call
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let input: Json = serde_json::from_str(arguments).unwrap_or(json!({}));
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.get("id").cloned().unwrap_or(Json::Null),
                        "name": name,
                        "input": input,
                    }));
                }
                if !blocks.is_empty() {
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            "tool" => {
                let block = json!({
                    "type": "tool_result",
                    "tool_use_id": message.get("tool_call_id").cloned().unwrap_or(Json::Null),
                    "content": content,
                });
                match out.last_mut() {
                    Some(last) if last.get("role").and_then(|v| v.as_str()) == Some("user")
                        && last.get("content").and_then(|c| c.as_array())
                            .map(|blocks| blocks.iter().all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result")))
                            .unwrap_or(false) =>
                    {
                        last["content"].as_array_mut().unwrap().push(block);
                    }
                    _ => out.push(json!({"role": "user", "content": [block]})),
                }
            }
            _ => {}
        }
    }
    (system, out)
}

/// Anthropic response → the OpenAI-style assistant message the loop expects.
fn from_anthropic_message(data: &Json) -> Json {
    let mut text = String::new();
    let mut calls: Vec<Json> = vec![];
    for block in data.get("content").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        match block.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "text" => {
                if let Some(part) = block.get("text").and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(part);
                }
            }
            "tool_use" => calls.push(json!({
                "id": block.get("id").cloned().unwrap_or(Json::Null),
                "type": "function",
                "function": {
                    "name": block.get("name").cloned().unwrap_or(Json::Null),
                    "arguments": block.get("input").cloned().unwrap_or(json!({})).to_string(),
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
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_usage_reads_openai_and_anthropic_bodies() {
        // OpenAI chat.completions shape
        let openai = json!({"usage": {"prompt_tokens": 120, "completion_tokens": 30, "total_tokens": 150}});
        assert_eq!(parse_usage(&openai), Some((120, 30, 150)));
        // total synthesized when absent
        let no_total = json!({"usage": {"prompt_tokens": 10, "completion_tokens": 4}});
        assert_eq!(parse_usage(&no_total), Some((10, 4, 14)));
        // Anthropic Messages shape
        let anthropic = json!({"usage": {"input_tokens": 200, "output_tokens": 45}});
        assert_eq!(parse_usage(&anthropic), Some((200, 45, 245)));
        // no usage block (some proxies) -> no record
        assert_eq!(parse_usage(&json!({"choices": []})), None);
        assert_eq!(parse_usage(&json!({"usage": {"other": 1}})), None);
    }

    #[test]
    fn usage_accumulates_per_thread_and_snapshots_totals() {
        let runner = ChatRunner::new(
            &json!({"id": "m", "name": "M", "role": "worker"}),
            ModelProfile {
                provider: "openai".into(), protocol: "openai".into(), model: "test".into(),
                base_url: None, api_key_env: None, timeout: 30, max_retries: 0,
                generation_options: Default::default(), context_window: None,
            },
            None,
            crate::runtime::Notify::new(crate::core_client::CoreClient::open(":memory:", "usage-test").unwrap()),
            crate::bound::BoundTools { tools: vec![] },
            vec![],
            (false, false),
        );
        let openai = json!({"usage": {"prompt_tokens": 100, "completion_tokens": 20, "total_tokens": 120}});
        let anthropic = json!({"usage": {"input_tokens": 50, "output_tokens": 10}});
        runner.record_usage("t1", &openai);
        runner.record_usage("t1", &openai);
        runner.record_usage("t2", &anthropic);
        runner.record_usage("t2", &json!({})); // missing usage: ignored
        let snap = runner.usage_snapshot();
        assert_eq!(snap["calls"], 3);
        assert_eq!(snap["prompt_tokens"], 250);
        assert_eq!(snap["completion_tokens"], 50);
        assert_eq!(snap["total_tokens"], 300);
        assert_eq!(snap["last_prompt_tokens"], 50, "latest call wins across threads");
    }

    #[test]
    fn anthropic_conversion_keeps_tool_pairs_and_merges_results() {
        let history = vec![
            json!({"role": "system", "content": "be brief"}),
            json!({"role": "user", "content": "go"}),
            json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "assign_task", "arguments": "{\"assignee\":\"b\"}"}},
                {"id": "c2", "type": "function", "function": {"name": "send_message", "arguments": "{}"}},
            ]}),
            json!({"role": "tool", "tool_call_id": "c1", "content": "{\"ok\":true}"}),
            json!({"role": "tool", "tool_call_id": "c2", "content": "{\"ok\":true}"}),
        ];
        let (system, messages) = to_anthropic_messages(&history);
        assert_eq!(system, "be brief");
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"][0]["type"], "tool_use");
        assert_eq!(messages[1]["content"][0]["input"]["assignee"], "b");
        // both tool results share one user message (Anthropic requires it)
        assert_eq!(messages[2]["role"], "user");
        let blocks = messages[2]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");

        let reply = from_anthropic_message(&json!({"content": [
            {"type": "text", "text": "done"},
            {"type": "tool_use", "id": "t1", "name": "signal_done", "input": {"summary": "s"}},
        ]}));
        assert_eq!(reply["content"], "done");
        assert_eq!(reply["tool_calls"][0]["function"]["name"], "signal_done");
        assert_eq!(reply["tool_calls"][0]["function"]["arguments"], "{\"summary\":\"s\"}");
    }

    #[test]
    fn render_view_matches_runners_py_shape() {
        let view = json!({
            "agent_id": "b",
            "assignment": [{"task_id": "t1", "description": "d", "acceptance": "a", "status": "PENDING", "requester": "leader"}],
            "inbox_delta": [{"from": "leader", "kind": "message", "payload": {"text": "hi"}}],
            "permitted_shared_delta": [],
            "relevant_topology": {"revision": 3, "members": ["leader"], "can_send_to": ["leader"], "can_delegate_to": [], "shared_spaces": []},
            "delivery_ids": [1], "batch_no": 1,
        });
        let rendered = render_view(&view, &json!({"reason": "user_input", "payload": {"kinds": ["user_message"]}}), Some("/w"));
        assert!(rendered.contains("<wake reason=\"user_input\">"));
        assert!(rendered.contains("<your_tasks>["));
        assert!(rendered.contains("<inbox from=\"leader\" kind=\"message\">{\"text\":\"hi\"}</inbox>"));
        assert!(rendered.contains("<team revision=\"3\">"));
        assert!(rendered.contains("<your_workspace>/w</your_workspace>"));
        // a plain new_input wake adds no wake block
        assert!(!render_view(&view, &json!({"reason": "new_input"}), None).contains("<wake"));
        // tool payload: team tools always, execution tools per binding
        // +1: read_history is runtime-provided, not a capability binding
        assert_eq!(tools_payload(&[], (false, false), &[]).as_array().unwrap().len(), TEAM_TOOL_DOCS.len() + 1);
        let bound = tools_payload(&["files".into(), "shell".into(), "web".into()], (true, true), &[]);
        let names: Vec<&str> = bound
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        for expected in ["read_file", "shell", "web_search", "web_fetch", "signal_done"] {
            assert!(names.contains(&expected), "{expected} missing from {names:?}");
        }
        // an explicit binding name (not the literal "web") advertises its tools:
        // the flags come from tools.rs::web_tools over the real catalog
        let explicit = tools_payload(&["anysearch".into()], (true, false), &[]);
        let names: Vec<&str> = explicit
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"web_search"), "explicit web binding missing from {names:?}");
        assert!(!names.contains(&"web_fetch"), "only the bound kind is advertised: {names:?}");
        // a paused/interrupted batch never leaves an assistant tool_call unanswered
        let calls = vec![
            json!({"id": "c1", "type": "function", "function": {"name": "shell", "arguments": "{}"}}),
            json!({"id": "c2", "type": "function", "function": {"name": "shell", "arguments": "{}"}}),
        ];
        let mut history = vec![json!({"role": "assistant", "content": null, "tool_calls": calls.clone()})];
        history.push(json!({"role": "tool", "tool_call_id": "c1", "content": "{}"}));
        fill_unanswered_tool_calls(&mut history, &calls, &["c1".to_string()], "TurnPaused");
        let answered: Vec<&str> = history
            .iter()
            .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("tool"))
            .filter_map(|m| m.get("tool_call_id").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(answered, vec!["c1", "c2"], "every tool_call must be answered exactly once");

        let files_only = tools_payload(&["files".into()], (false, false), &[]);
        let names: Vec<&str> = files_only
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"read_file") && !names.contains(&"shell"));
        // `skills` binding advertises the discovery/read tool (binding = authorization)
        let with_skills = tools_payload(&["files".into(), "skills".into()], (false, false), &[]);
        let names: Vec<&str> = with_skills
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.pointer("/function/name").and_then(|v| v.as_str()))
            .collect();
        assert!(names.contains(&"skill"));
        let plain = tools_payload(&["files".into()], (false, false), &[]).to_string();
        assert!(!plain.contains("\"skill\""));
        // the profile's protocol decides the endpoint (D-8 deepseek default)
        let profile = |base: Option<&str>, protocol: &str, provider: &str| ModelProfile {
            provider: provider.into(),
            protocol: protocol.into(),
            model: "deepseek-flash".into(),
            base_url: base.map(str::to_string),
            api_key_env: None,
            timeout: 120,
            max_retries: 1,
            generation_options: Default::default(),
            context_window: None,
        };
        assert_eq!(resolve_base_url(&profile(None, "deepseek", "deepseek")), "https://api.deepseek.com/v1");
        assert_eq!(resolve_base_url(&profile(Some("https://x/v1/"), "deepseek", "deepseek")), "https://x/v1");
        assert_eq!(resolve_base_url(&profile(None, "openai", "openai")), "https://api.openai.com/v1");
        assert_eq!(resolve_base_url(&profile(None, "anthropic", "anthropic")), "https://api.anthropic.com");
    }

    #[test]
    fn effort_normalization_and_retry_classification() {
        // normalize_effort — deepseek has no xhigh level
        assert_eq!(normalize_effort("deepseek", "xhigh"), "max");
        assert_eq!(normalize_effort("deepseek", "XHIGH"), "max");
        assert_eq!(normalize_effort("openai", "xhigh"), "xhigh");
        assert_eq!(normalize_effort("deepseek", "low"), "low");
        // looks_like_effort_error
        assert!(looks_like_effort_error("chat API 400: unsupported value: xhigh"));
        assert!(looks_like_effort_error("Reasoning effort 'xhigh' is not supported"));
        assert!(!looks_like_effort_error("chat API 400: bad request"));
        // retry semantics: transient statuses only
        for code in [408, 409, 429, 500, 503] {
            assert!(retryable_status(code), "{code} is transient");
        }
        for code in [400, 401, 403, 404, 422] {
            assert!(!retryable_status(code), "{code} must not be retried");
        }
    }

    #[test]
    fn chat_tree_materialize_rewind_and_points() {
        // D-26: append-only tree + movable leaf = rewind without data loss
        let mut tree = ChatTree::default();
        tree.append(&[json!({"role":"system","content":"s"})]);
        tree.append(&[json!({"role":"user","content":"第一问"}), json!({"role":"assistant","content":"a1"})]);
        tree.append(&[json!({"role":"user","content":"第二问"}), json!({"role":"assistant","content":"a2"})]);
        assert_eq!(tree.materialize().len(), 5);
        let points = tree.rewind_points();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0]["preview"], "第二问"); // newest first
        // rewind to the first user message: later exchange stays in the tree
        let target = points[1]["id"].as_str().unwrap().to_string();
        assert_eq!(tree.rewind_to(Some(&target)).unwrap(), 2);
        assert_eq!(tree.materialize().last().unwrap()["content"], "第一问");
        // branch off the rewound point: old branch nodes remain addressable
        tree.append(&[json!({"role":"assistant","content":"a1-alt"})]);
        assert_eq!(tree.nodes.len(), 6);
        assert_eq!(tree.materialize().last().unwrap()["content"], "a1-alt");
        assert!(tree.rewind_to(Some("n999")).is_err());
        assert_eq!(tree.rewind_to(None).unwrap(), 0);
    }

    #[test]
    fn cap_tool_output_keeps_head_and_tail() {
        let short = "x".repeat(100);
        assert_eq!(cap_tool_output(short.clone()), short);
        let long = format!("{}MID{}", "H".repeat(30_000), "T".repeat(30_000));
        let capped = cap_tool_output(long);
        assert!(capped.starts_with('H') && capped.ends_with('T'));
        assert!(capped.contains("chars truncated"));
        assert!(capped.len() < 51_000, "cap plus marker stays near the cap");
        assert!(!capped.contains("MID"));
    }

    #[test]
    fn mask_old_tool_outputs_masks_beyond_trailing_budget() {
        let big = "x".repeat(MASK_KEEP_RECENT + 1_000);
        let history = vec![
            json!({"role":"system","content":"s"}),
            json!({"role":"user","content":"u"}),
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"old","type":"function","function":{"name":"shell","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"old","content":big}),
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"new","type":"function","function":{"name":"shell","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"new","content":"fresh"}),
        ];
        let masked = mask_old_tool_outputs(&history);
        assert!(masked[3]["content"].as_str().unwrap().contains("tool_call_id=\"old\""), "old output masked with its id");
        assert_eq!(masked[5]["content"], "fresh", "latest answers never masked");
        // a small old output within budget stays verbatim
        let small = vec![
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"a","type":"function","function":{"name":"shell","arguments":"{}"}}]}),
            json!({"role":"tool","tool_call_id":"a","content":"tiny"}),
        ];
        assert_eq!(mask_old_tool_outputs(&small)[1]["content"], "tiny");
    }

    #[test]
    fn summary_node_hides_covered_range_but_tree_keeps_it() {
        let mut tree = ChatTree::default();
        tree.append(&[json!({"role":"system","content":"s"})]);
        tree.append(&[json!({"role":"user","content":"u1"}), json!({"role":"assistant","content":"a1"})]);
        tree.append(&[json!({"role":"user","content":"u2"}), json!({"role":"assistant","content":"a2"})]);
        // compact: keep the system root, cover the rest
        tree.append_summary(json!({"role":"user","content":"[Compacted conversation summary] SUM"}), "n1");
        assert_eq!(tree.materialize().len(), 2, "view = system + summary");
        assert_eq!(tree.materialize()[1]["content"].as_str().unwrap(), "[Compacted conversation summary] SUM");
        // new turns chain on top of the summary
        tree.append(&[json!({"role":"assistant","content":"a3"})]);
        let view = tree.materialize();
        assert_eq!(view.len(), 3);
        assert_eq!(view[2]["content"], "a3");
        // rewinding onto the covered branch restores the full history
        assert_eq!(tree.rewind_to(Some("n3")).unwrap(), 3, "u1/a1 visible again");
        assert_eq!(tree.materialize()[2]["content"], "a1");
        // skip_to: "" covers back to the root (no system kept)
        let mut bare = ChatTree::default();
        bare.append(&[json!({"role":"user","content":"u"})]);
        bare.append_summary(json!({"role":"user","content":"SUM"}), "");
        assert_eq!(bare.materialize().len(), 1);
    }

    #[test]
    fn read_history_finds_outputs_in_history_and_tree() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-readhist-{}", std::process::id()));
        std::env::set_var("XDG_STATE_HOME", root.join("state"));
        std::fs::create_dir_all(root.join("state")).unwrap();
        let runner = ChatRunner::new(
            &json!({"id": "lead", "name": "L", "role": "leader"}),
            ModelProfile {
                provider: "openai".into(), protocol: "openai".into(), model: "test".into(),
                base_url: None, api_key_env: None, timeout: 30, max_retries: 0,
                generation_options: Default::default(), context_window: None,
            },
            None,
            crate::runtime::Notify::new(crate::core_client::CoreClient::open(":memory:", "readhist-test").unwrap()),
            crate::bound::BoundTools { tools: vec![] },
            vec![],
            (false, false),
        );
        let live = vec![json!({"role":"tool","tool_call_id":"c1","content":"live output"})];
        assert_eq!(runner.read_history("t", &live, "c1").unwrap()["output"], "live output");
        // committed to the tree but no longer in the live view (compacted away)
        let mut tree = runner.load_tree("t").unwrap();
        tree.append(&[json!({"role":"tool","tool_call_id":"c2","content":"archived output"})]);
        runner.save_tree("t", &tree).unwrap();
        assert_eq!(runner.read_history("t", &live, "c2").unwrap()["output"], "archived output");
        assert!(runner.read_history("t", &live, "nope").is_err());
        assert!(runner.read_history("t", &live, "").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn runner_tree_persists_and_rewind_invalidates() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-tree-{}", std::process::id()));
        std::env::set_var("XDG_STATE_HOME", root.join("state"));
        std::fs::create_dir_all(root.join("state")).unwrap();
        let runner = ChatRunner::new(
            &json!({"id": "lead", "name": "L", "role": "leader"}),
            ModelProfile {
                provider: "openai".into(), protocol: "openai".into(), model: "test".into(),
                base_url: None, api_key_env: None, timeout: 30, max_retries: 0,
                generation_options: Default::default(), context_window: None,
            },
            None,
            crate::runtime::Notify::new(crate::core_client::CoreClient::open(":memory:", "tree-test").unwrap()),
            crate::bound::BoundTools { tools: vec![] },
            vec![],
            (false, false),
        );
        let mut tree = runner.load_tree("user-dialog").unwrap();
        tree.append(&[json!({"role":"user","content":"u1"}), json!({"role":"assistant","content":"a1"})]);
        runner.save_tree("user-dialog", &tree).unwrap();
        // fresh load sees the persisted tree
        let reloaded = runner.load_tree("user-dialog").unwrap();
        assert_eq!(reloaded.materialize().len(), 2);
        assert_eq!(runner.rewind_points("user-dialog").len(), 1);
        // rewind to empty; the epoch distinguishes it from a pending commit
        assert_eq!(runner.rewind("user-dialog", None).unwrap(), 0);
        assert_eq!(runner.load_tree("user-dialog").unwrap().materialize().len(), 0);
        assert_eq!(runner.load_tree("user-dialog").unwrap().rewind_epoch, 1);
        assert!(runner.history_dir().is_some());
        std::fs::remove_dir_all(&root).ok();
    }

}
