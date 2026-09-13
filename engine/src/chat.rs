//! Chat member backend: a plain tool-calling loop over an OpenAI-compatible
//! endpoint (runners.py::DeepAgentsRunner, with the graph framework replaced
//! by this loop — team semantics stay in the core).

use crate::gateway::ToolGateway;
use crate::runtime::{AgentRunner, Notify};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{ModelProfile, TurnRun, TurnStatus};

/// providers.py::build_chat_model — the protocol picks the default endpoint
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

/// providers.py::normalize_effort — a model that lacks `xhigh` maps to `max`
/// instead of erroring out (user decision: deepseek has no xhigh level).
pub fn normalize_effort(protocol: &str, effort: &str) -> String {
    if effort.eq_ignore_ascii_case("xhigh") && protocol == "deepseek" {
        "max".into()
    } else {
        effort.to_string()
    }
}

/// runners.py::_looks_like_effort_error — a provider rejecting the requested
/// reasoning effort; the caller retries once with `max`.
pub fn looks_like_effort_error(text: &str) -> bool {
    let text = text.to_lowercase();
    ["reasoning_effort", "reasoning effort", "effort", "unsupported value"]
        .iter()
        .any(|token| text.contains(token))
}

/// Python SDK retry semantics: transient statuses and transport errors only.
fn retryable_status(code: u16) -> bool {
    matches!(code, 408 | 409 | 429) || (500..600).contains(&code)
}

/// runners.py::render_view — compact text view for the next model call.
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
    ("apply_topology_patch", "Leader only: apply (or reject) a topology patch from a base revision."),
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
/// (runners.py::_ensure_graph: team tools + shell + bound file/web tools).
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
    names
}

fn tools_payload(bindings: &[String], web: (bool, bool), bound: &[Json]) -> Json {
    let docs: HashMap<&str, &str> = TEAM_TOOL_DOCS.iter().chain(BOUND_TOOL_DOCS).copied().collect();
    let allowed: Vec<&str> = TEAM_TOOL_DOCS
        .iter()
        .map(|(name, _)| *name)
        .chain(bound_tool_names(bindings, web))
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
                        "description": docs.get(name).copied().unwrap_or(name),
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

pub struct ChatRunner {
    agent: serde_json::Value,
    profile: ModelProfile,
    workdir: Option<String>,
    notify: Arc<Notify>,
    bound: crate::bound::BoundTools,
    /// Resolved web capability (explicit binding names or the `web` umbrella).
    has_web_search: bool,
    has_web_fetch: bool,
    /// (label, content) pairs from skills/instruction files (session.py::_skills_and_memory)
    context: Vec<(String, String)>,
    /// Member conversation history survives a restart (USER-GUIDE §5).
    history_path: Option<std::path::PathBuf>,
    messages: Mutex<HashMap<String, Vec<Json>>>,
    states: Mutex<HashMap<String, TurnStatus>>,
    paused_kind: Mutex<HashMap<String, String>>,
    mid_turn: Mutex<HashMap<String, Vec<Json>>>,
    interrupted: Mutex<HashSet<String>>,
    /// runners.py::_switch_effort_to_max — at most one effort fallback per runner
    effort_max: AtomicBool,
    effort_fallback_used: AtomicBool,
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
            messages: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            paused_kind: Mutex::new(HashMap::new()),
            mid_turn: Mutex::new(HashMap::new()),
            interrupted: Mutex::new(HashSet::new()),
            effort_max: AtomicBool::new(false),
            effort_fallback_used: AtomicBool::new(false),
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
    fn save_history(&self, thread: &str, history: &[Json]) {
        let Some(path) = &self.history_path else { return };
        let mut data = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
            .unwrap_or_else(|| json!({}));
        if !data.is_object() {
            data = json!({});
        }
        data[thread] = Json::Array(history.to_vec());
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, data.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
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

    fn has_paused(&self, run_id: &str) -> bool {
        self.interrupted.lock().unwrap().contains(run_id)
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

    /// The session TeamSpec limit for model requests per turn (runners.py reads
    /// it when the graph is built; here once per turn).
    fn max_model_steps(&self) -> i64 {
        self.notify
            .core()
            .state()
            .ok()
            .and_then(|state| {
                state
                    .get("limits")
                    .and_then(|limits| limits.get("max_model_steps_per_turn"))
                    .and_then(|v| v.as_i64())
            })
            .unwrap_or(200)
    }

    fn chat(&self, messages: &[Json], tools: &Json) -> Result<Json, String> {
        if self.profile.protocol == "anthropic" {
            return self.chat_anthropic(messages, tools);
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

    /// Anthropic Messages API (providers.py uses langchain-anthropic natively).
    /// ponytail: text/tool_use/tool_result blocks only — no images or thinking blocks.
    fn chat_anthropic(&self, messages: &[Json], tools: &Json) -> Result<Json, String> {
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
        let url = format!("{base}/v1/messages");
        let mut last_error = "chat call failed".to_string();
        let retries = self.profile.max_retries.max(0);
        for attempt in 0..=retries {
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
                    Ok(data) => return Ok(from_anthropic_message(&data)),
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

    fn run_loop(
        &self,
        run: &TurnRun,
        history: &mut Vec<Json>,
        gateway: &ToolGateway,
        max_steps: i64,
    ) -> Result<String, (String, String)> {
        let tools = tools_payload(&self.bindings(), self.web_flags(), &self.bound.schemas());
        let agent_id = self.agent_id();
        // Python has two gates on the same budget: the tool-call executor in the
        // runtime and a model-request counter (runners.py TurnAgentMiddleware).
        // This is the model-request gate — it also covers bound MCP tools, which
        // never reach the gateway's executor.
        let mut model_steps = 0i64;
        loop {
            model_steps += 1;
            if model_steps > max_steps {
                return Err((
                    "TurnLimitExceeded".into(),
                    format!("model-step limit {max_steps} reached for this turn"),
                ));
            }
            if self.has_paused(&run.run_id) {
                return Err(("TurnInterrupted".into(), "interrupted".into()));
            }
            let mid = self.mid_turn.lock().unwrap().remove(&run.run_id).unwrap_or_default();
            if !mid.is_empty() {
                let text = mid
                    .iter()
                    .map(|i| {
                        format!(
                            "<inbox from=\"{}\" kind=\"{}\">{}</inbox>",
                            i.get("from").and_then(|v| v.as_str()).unwrap_or(""),
                            i.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                            i.get("payload").cloned().unwrap_or(json!({}))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                history.push(json!({"role": "user", "content": text}));
            }
            let message = match self.chat(history, &tools) {
                Ok(message) => message,
                Err(e) => {
                    // a provider that rejects the configured effort maps to
                    // `max` once, then the call is retried (runners.py:614-627)
                    if looks_like_effort_error(&e)
                        && self.configured_effort()
                        && !self.effort_fallback_used.swap(true, Ordering::SeqCst)
                    {
                        self.effort_max.store(true, Ordering::SeqCst);
                        match self.chat(history, &tools) {
                            Ok(message) => message,
                            Err(retry_error) => return Err(("ChatError".into(), retry_error)),
                        }
                    } else {
                        return Err(("ChatError".into(), e));
                    }
                }
            };
            history.push(message.clone());
            if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
                if !content.is_empty() {
                    self.notify.note_stream_chunk(&run.run_id, &agent_id, content);
                }
            }
            let calls = message.get("tool_calls").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if calls.is_empty() {
                return Ok(message.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string());
            }
            // One tool message per tool_call is mandatory: an unanswered call
            // makes the provider reject the next request (live-reproduced 400).
            let mut answered: Vec<String> = vec![];
            let mut paused: Option<(String, String)> = None;
            for call in &calls {
                if self.has_paused(&run.run_id) {
                    paused = Some(("TurnInterrupted".into(), "interrupted".into()));
                    break;
                }
                let call_id = call.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let name = call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = call
                    .get("function")
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let args: Json = match serde_json::from_str(arguments) {
                    Ok(args) => args,
                    Err(_) => {
                        history.push(json!({"role": "tool", "tool_call_id": call_id.clone(), "content": "invalid JSON arguments"}));
                        answered.push(call_id);
                        continue;
                    }
                };
                // A configured+bound service is the authorization for its tools
                // (plan §12.1); everything else goes through the gateway.
                let receipt = match self.bound.call(&name, &args) {
                    Some(Ok(output)) => teamagents_core::models::Receipt {
                        action_id: call_id.clone(),
                        ok: true,
                        kind: teamagents_core::models::ActionKind::CompleteTask,
                        result: json!({"output": output}),
                        error: None,
                    },
                    Some(Err(e)) => teamagents_core::models::Receipt {
                        action_id: call_id.clone(),
                        ok: false,
                        kind: teamagents_core::models::ActionKind::CompleteTask,
                        result: json!({}),
                        error: Some(e),
                    },
                    None => gateway.call(&name, &args, &call_id),
                };
                if receipt.error.as_deref() == Some("approval_required") {
                    self.paused_kind.lock().unwrap().insert(run.run_id.clone(), "approval".into());
                    let note = receipt.result.get("approval_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    history.push(json!({"role": "tool", "tool_call_id": call_id.clone(),
                                        "content": json!({"approval_required": note}).to_string()}));
                    answered.push(call_id);
                    paused = Some(("TurnPaused".into(), note));
                    break;
                }
                // the runtime's step guard failing ends the turn (Python raises
                // TurnLimitExceeded from the middleware)
                if let Some(error) = receipt.error.as_deref() {
                    if error.contains("step limit") {
                        return Err(("TurnLimitExceeded".into(), error.to_string()));
                    }
                }
                let waiting = name == "wait_for_tasks"
                    && receipt.result.get("waiting").and_then(|v| v.as_bool()).unwrap_or(false);
                let content = if receipt.ok {
                    receipt.result.to_string()
                } else {
                    json!({"error": receipt.error}).to_string()
                };
                history.push(json!({"role": "tool", "tool_call_id": call_id.clone(), "content": content}));
                answered.push(call_id);
                if waiting {
                    self.paused_kind.lock().unwrap().insert(run.run_id.clone(), "waiting".into());
                    paused = Some(("TurnPaused".into(), "waiting".into()));
                    break;
                }
            }
            if let Some((kind, note)) = paused {
                fill_unanswered_tool_calls(history, &calls, &answered, &kind);
                return Err((kind, note));
            }
        }
    }
}

impl AgentRunner for ChatRunner {
    fn start_or_resume(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> TurnOutcome {
        self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Running);
        self.interrupted.lock().unwrap().remove(&run.run_id);
        let thread = run.context_ref.clone().unwrap_or_else(|| run.run_id.clone());
        // the spec limit is read once per turn, like the Python graph build
        let max_steps = self.max_model_steps();
        let mut history = match self.messages.lock().unwrap().get(&thread).cloned() {
            Some(history) => history,
            None => self.load_history(&thread),
        };
        let instructions = self.agent.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
        if history.is_empty() && (!instructions.is_empty() || !self.context.is_empty()) {
            history.push(json!({"role": "system", "content": self.system_prompt()}));
        }
        // the rendered view is authoritative for what this segment saw
        history.push(json!({"role": "user", "content": render_view(view, wake, self.workdir.as_deref())}));

        let result = self.run_loop(run, &mut history, gateway, max_steps);
        self.messages.lock().unwrap().insert(thread.clone(), history.clone());
        self.save_history(&thread, &history);
        let outcome = match result {
            Ok(reply) => {
                self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Completed);
                TurnOutcome {
                    status: TurnStatus::Completed,
                    error: None,
                    note: None,
                    reply_text: Some(reply),
                }
            }
            Err((name, message)) => match name.as_str() {
                "TurnInterrupted" => {
                    self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Cancelled);
                    TurnOutcome { status: TurnStatus::Cancelled, error: None, note: None, reply_text: None }
                }
                "TurnPaused" => {
                    let paused = self.paused_kind.lock().unwrap().get(&run.run_id).cloned().unwrap_or_default();
                    let status = if paused == "approval" { TurnStatus::WaitingApproval } else { TurnStatus::WaitingTask };
                    self.states.lock().unwrap().insert(run.run_id.clone(), status);
                    TurnOutcome {
                        status,
                        error: None,
                        note: if message.is_empty() { None } else { Some(message) },
                        reply_text: None,
                    }
                }
                // Python raises TurnLimitExceeded for both gates; the note is what
                // makes the core emit `limit_reached` (runners.py:608-610)
                "TurnLimitExceeded" => {
                    self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Failed);
                    TurnOutcome {
                        status: TurnStatus::Failed,
                        error: Some(message),
                        note: Some("turn_limit".into()),
                        reply_text: None,
                    }
                }
                other => {
                    self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Failed);
                    TurnOutcome {
                        status: TurnStatus::Failed,
                        error: Some(format!("{other}: {message}")),
                        note: None,
                        reply_text: None,
                    }
                }
            },
        };
        self.notify.wake();
        outcome
    }

    fn request_interrupt(&self, run_id: &str) -> TurnStatus {
        self.interrupted.lock().unwrap().insert(run_id.to_string());
        self.states.lock().unwrap().insert(run_id.to_string(), TurnStatus::Cancelled);
        TurnStatus::Cancelled
    }

    fn query_state(&self, run_id: &str) -> Option<TurnStatus> {
        self.states.lock().unwrap().get(run_id).copied()
    }

    fn deliver_mid_turn(&self, run_id: &str, items: Vec<Json>) {
        self.mid_turn.lock().unwrap().entry(run_id.to_string()).or_default().extend(items);
    }

    fn close(&self) {
        self.bound.close();
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
        assert_eq!(tools_payload(&[], (false, false), &[]).as_array().unwrap().len(), TEAM_TOOL_DOCS.len());
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
        };
        assert_eq!(resolve_base_url(&profile(None, "deepseek", "deepseek")), "https://api.deepseek.com/v1");
        assert_eq!(resolve_base_url(&profile(Some("https://x/v1/"), "deepseek", "deepseek")), "https://x/v1");
        assert_eq!(resolve_base_url(&profile(None, "openai", "openai")), "https://api.openai.com/v1");
        assert_eq!(resolve_base_url(&profile(None, "anthropic", "anthropic")), "https://api.anthropic.com");
    }

    #[test]
    fn effort_normalization_and_retry_classification_match_python() {
        // providers.py::normalize_effort — deepseek has no xhigh level
        assert_eq!(normalize_effort("deepseek", "xhigh"), "max");
        assert_eq!(normalize_effort("deepseek", "XHIGH"), "max");
        assert_eq!(normalize_effort("openai", "xhigh"), "xhigh");
        assert_eq!(normalize_effort("deepseek", "low"), "low");
        // runners.py::_looks_like_effort_error
        assert!(looks_like_effort_error("chat API 400: unsupported value: xhigh"));
        assert!(looks_like_effort_error("Reasoning effort 'xhigh' is not supported"));
        assert!(!looks_like_effort_error("chat API 400: bad request"));
        // Python SDK retry semantics: transient statuses only
        for code in [408, 409, 429, 500, 503] {
            assert!(retryable_status(code), "{code} is transient");
        }
        for code in [400, 401, 403, 404, 422] {
            assert!(!retryable_status(code), "{code} must not be retried");
        }
    }
}
