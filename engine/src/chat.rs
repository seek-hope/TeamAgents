//! Chat member backend: a plain tool-calling loop over an OpenAI-compatible
//! endpoint (runners.py::DeepAgentsRunner, with the graph framework replaced
//! by this loop — team semantics stay in the core).

use crate::gateway::ToolGateway;
use crate::runtime::{AgentRunner, Notify};
use serde_json::{json, Value as Json};
use std::collections::{HashMap, HashSet};
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
            // ponytail: anthropic-native wire protocol is not implemented; such
            // profiles must point base_url at an OpenAI-compatible gateway.
            _ => "https://api.openai.com/v1".into(),
        },
    }
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

fn tools_payload() -> Json {
    let docs: HashMap<&str, &str> = TEAM_TOOL_DOCS.iter().copied().collect();
    Json::Array(
        team_tool_schemas()
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
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

pub struct ChatRunner {
    agent: serde_json::Value,
    profile: ModelProfile,
    workdir: Option<String>,
    notify: Arc<Notify>,
    messages: Mutex<HashMap<String, Vec<Json>>>,
    states: Mutex<HashMap<String, TurnStatus>>,
    paused_kind: Mutex<HashMap<String, String>>,
    mid_turn: Mutex<HashMap<String, Vec<Json>>>,
    interrupted: Mutex<HashSet<String>>,
}

impl ChatRunner {
    pub fn new(agent: &Json, profile: ModelProfile, workdir: Option<String>, notify: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            agent: agent.clone(),
            profile,
            workdir,
            notify,
            messages: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            paused_kind: Mutex::new(HashMap::new()),
            mid_turn: Mutex::new(HashMap::new()),
            interrupted: Mutex::new(HashSet::new()),
        })
    }

    fn agent_id(&self) -> String {
        self.agent.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string()
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
        let tools = TEAM_TOOL_DOCS
            .iter()
            .map(|(n, d)| format!("- {n}: {d}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{head}\n\nTeam tools available:\n{tools}\n\nRules: use complete_task to finish your assigned task; use signal_done only when the whole user goal is complete (Leader only)."
        )
    }

    fn has_paused(&self, run_id: &str) -> bool {
        self.interrupted.lock().unwrap().contains(run_id)
    }

    fn chat(&self, messages: &[Json], tools: &Json) -> Result<Json, String> {
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
        if let (Json::Object(base_map), Json::Object(options)) = (&mut body, json!(self.profile.generation_options)) {
            for (k, v) in options {
                base_map.insert(k, v);
            }
        }
        let url = format!("{base}/chat/completions");
        let mut last_error = "chat call failed".to_string();
        for attempt in 0..=self.profile.max_retries.max(0) {
            let mut request = ureq::post(&url)
                .set("content-type", "application/json")
                .timeout(std::time::Duration::from_secs(self.profile.timeout.max(1) as u64));
            if !api_key.is_empty() {
                request = request.set("authorization", &format!("Bearer {api_key}"));
            }
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
                    let text = response.into_string().unwrap_or_default();
                    let text: String = text.chars().take(500).collect();
                    last_error = format!("chat API {code}: {text}");
                }
                Err(e) => last_error = format!("chat API: {e}"),
            }
            let backoff = std::time::Duration::from_millis((500u64 << attempt.min(5)).min(8000));
            std::thread::sleep(backoff);
        }
        Err(last_error)
    }

    fn run_loop(&self, run: &TurnRun, history: &mut Vec<Json>, gateway: &ToolGateway) -> Result<String, (String, String)> {
        let tools = tools_payload();
        let agent_id = self.agent_id();
        let max_steps = 200; // the core enforces the configured cap via the gateway executor
        for _ in 0..max_steps {
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
                Err(e) => return Err(("ChatError".into(), e)),
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
            for call in calls {
                if self.has_paused(&run.run_id) {
                    return Err(("TurnInterrupted".into(), "interrupted".into()));
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
                        history.push(json!({"role": "tool", "tool_call_id": call_id, "content": "invalid JSON arguments"}));
                        continue;
                    }
                };
                let receipt = gateway.call(&name, &args, &call_id);
                if receipt.error.as_deref() == Some("approval_required") {
                    self.paused_kind.lock().unwrap().insert(run.run_id.clone(), "approval".into());
                    let note = receipt.result.get("approval_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    return Err(("TurnPaused".into(), note));
                }
                if name == "wait_for_tasks"
                    && receipt.result.get("waiting").and_then(|v| v.as_bool()).unwrap_or(false)
                {
                    self.paused_kind.lock().unwrap().insert(run.run_id.clone(), "waiting".into());
                    return Err(("TurnPaused".into(), "waiting".into()));
                }
                let content = if receipt.ok {
                    receipt.result.to_string()
                } else {
                    json!({"error": receipt.error}).to_string()
                };
                history.push(json!({"role": "tool", "tool_call_id": call_id, "content": content}));
            }
        }
        Err(("TurnLimitExceeded".into(), format!("step limit {max_steps} reached")))
    }
}

impl AgentRunner for ChatRunner {
    fn start_or_resume(&self, run: &TurnRun, view: &Json, gateway: &ToolGateway, wake: &Json) -> TurnOutcome {
        self.states.lock().unwrap().insert(run.run_id.clone(), TurnStatus::Running);
        self.interrupted.lock().unwrap().remove(&run.run_id);
        let thread = run.context_ref.clone().unwrap_or_else(|| run.run_id.clone());
        let mut history = self.messages.lock().unwrap().get(&thread).cloned().unwrap_or_default();
        let instructions = self.agent.get("instructions").and_then(|v| v.as_str()).unwrap_or("");
        if history.is_empty() && !instructions.is_empty() {
            history.push(json!({"role": "system", "content": self.system_prompt()}));
        }
        // the rendered view is authoritative for what this segment saw
        history.push(json!({"role": "user", "content": render_view(view, wake, self.workdir.as_deref())}));

        let result = self.run_loop(run, &mut history, gateway);
        self.messages.lock().unwrap().insert(thread, history);
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // tool payload carries every documented name
        assert_eq!(tools_payload().as_array().unwrap().len(), TEAM_TOOL_DOCS.len());
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
    }
}
