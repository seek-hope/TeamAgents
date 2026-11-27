//! Typed wire projections, daemon client, sanitization, and clarification safety tracker.
use crate::{daemon, Result};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnKind {
    Legacy,
    Root,
    Node,
    Reporter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TaskStatus {
    Queued,
    Running,
    Waiting,
    Input,
    Approval,
    Done,
    Failed,
    Cancelled,
    Unknown,
    Other(String),
}

impl TaskStatus {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "input" => Self::Input,
            "approval" => Self::Approval,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "unknown" => Self::Unknown,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Input => "input",
            Self::Approval => "approval",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled | Self::Unknown)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Waiting)
    }

    pub fn is_cancellable(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Waiting | Self::Input | Self::Approval)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verification {
    Passed,
    Failed,
    Pending,
    NotRequested,
    Unknown,
    Other(String),
}

impl Verification {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "pending" => Self::Pending,
            "not_requested" => Self::NotRequested,
            "unknown" => Self::Unknown,
            other => Self::Other(other.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Policy {
    Auto,
    Review,
}

impl Policy {
    pub fn parse(raw: Option<&str>) -> Option<Self> {
        match raw? {
            "auto" => Some(Self::Auto),
            "review" => Some(Self::Review),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Review => "review",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StageKind {
    Legacy,
    Root,
    Node,
    Repair,
    Summarizer,
    Other(String),
}

impl StageKind {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "legacy" => Self::Legacy,
            "root" => Self::Root,
            "node" => Self::Node,
            "repair" => Self::Repair,
            "summarizer" => Self::Summarizer,
            other => Self::Other(other.to_owned()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Legacy => "legacy",
            Self::Root => "root",
            Self::Node => "node",
            Self::Repair => "repair",
            Self::Summarizer => "summarizer",
            Self::Other(s) => s.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActionPreview {
    ReadFile { path: String, offset: Option<u64>, limit: Option<u64> },
    EditFile { path: String, old: String, new: String },
    Shell { command: String, timeout_ms: Option<u64> },
    Verify { command: String },
    Other { op: String, summary: String },
}

impl ActionPreview {
    pub fn from_value(value: &Value) -> Option<Self> {
        if value.is_null() {
            return None;
        }
        let op = value["op"].as_str().unwrap_or("?");
        Some(match op {
            "read_file" => Self::ReadFile {
                path: value["path"].as_str().unwrap_or("?").to_owned(),
                offset: value.get("offset").and_then(Value::as_u64),
                limit: value.get("limit").and_then(Value::as_u64),
            },
            "edit_file" => Self::EditFile {
                path: value["path"].as_str().unwrap_or("?").to_owned(),
                old: value["old"].as_str().unwrap_or("").to_owned(),
                new: value["new"].as_str().unwrap_or("").to_owned(),
            },
            "shell" => Self::Shell {
                command: value["command"].as_str().unwrap_or("").to_owned(),
                timeout_ms: value.get("timeout_ms").and_then(Value::as_u64),
            },
            "verify" => Self::Verify { command: value["command"].as_str().unwrap_or("").to_owned() },
            other => Self::Other { op: other.to_owned(), summary: value.to_string() },
        })
    }

    pub fn op(&self) -> &str {
        match self {
            Self::ReadFile { .. } => "read_file",
            Self::EditFile { .. } => "edit_file",
            Self::Shell { .. } => "shell",
            Self::Verify { .. } => "verify",
            Self::Other { op, .. } => op.as_str(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::ReadFile { path, .. } | Self::EditFile { path, .. } => path.as_str(),
            Self::Shell { command, .. } | Self::Verify { command } => command.as_str(),
            Self::Other { summary, .. } => summary.as_str(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TreeNode {
    pub task: String,
    pub parent: Option<String>,
    pub agent: String,
    pub kind: StageKind,
    pub status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LeafResult {
    pub task: String,
    pub agent: String,
    pub status: TaskStatus,
    pub result: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubtreeReceipt {
    pub task: String,
    pub agent: String,
    pub op: String,
    pub label: String,
    pub ok: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EvidenceEntry {
    pub action: ActionPreview,
    pub ok: Option<bool>,
    pub result: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Clarification {
    pub question: String,
    pub answer: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoundRecord {
    pub round: u64,
    pub subtree_root: String,
    pub prompt: String,
    pub check: Option<String>,
    pub outcome: Option<TaskStatus>,
    pub verification: Option<Verification>,
    pub result: Option<String>,
    pub continuation_node: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WhiteboardEntry {
    pub key: String,
    pub kind: String,
    pub content: String,
    pub author_agent: String,
    pub round: u64,
    pub version: u64,
    pub seq: u64,
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskRecord {
    pub id: String,
    pub root: String,
    pub parent: Option<String>,
    pub agent: String,
    pub status: TaskStatus,
    pub stage_status: Option<TaskStatus>,
    pub kind: StageKind,
    pub prompt: Option<String>,
    pub result: Option<String>,
    pub check: Option<String>,
    pub proposal: Option<String>,
    pub action: Option<ActionPreview>,
    pub approval_id: Option<String>,
    pub tool_state: Option<String>,
    pub turns_used: Option<u64>,
    pub turn_budget: Option<u64>,
    pub policy: Option<Policy>,
    pub sandbox: Option<String>,
    pub question: Option<String>,
    pub stage_error: Option<String>,
    pub verification: Option<Verification>,
    pub verification_note: Option<String>,
    pub round: Option<u64>,
    pub active_root: Option<String>,
    pub continuation_node: Option<String>,
    pub continuation_agent: Option<String>,
    pub continuation_kind: Option<String>,
    pub whiteboard_seq: u64,
    pub whiteboard: Vec<WhiteboardEntry>,
    pub evidence: Vec<EvidenceEntry>,
    pub tree_nodes: Vec<TreeNode>,
    pub leaf_results: Vec<LeafResult>,
    pub subtree_receipts: Vec<SubtreeReceipt>,
    pub clarifications: Vec<Clarification>,
    pub rounds: Vec<RoundRecord>,
}

impl TaskRecord {
    pub fn from_value(value: &Value) -> Result<Self> {
        let id = field(value, "id")?.to_owned();
        let status = TaskStatus::parse(field(value, "status")?);
        let parent = opt_str(value, "parent");
        let root = opt_str(value, "root").or_else(|| parent.clone()).unwrap_or_else(|| id.clone());
        let agent = opt_str(value, "agent").unwrap_or_else(|| "?".into());
        let kind = StageKind::parse(value["kind"].as_str().unwrap_or(if parent.is_none() { "root" } else { "node" }));
        let evidence = value["evidence"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        let action = ActionPreview::from_value(&r["action"])?;
                        Some(EvidenceEntry {
                            action,
                            ok: r["ok"].as_bool(),
                            result: r["result"].as_str().unwrap_or("").to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tree_nodes = value["tree_nodes"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        let task = r["task"].as_str().or_else(|| r["id"].as_str())?.to_owned();
                        let status = r["stage_status"].as_str().or_else(|| r["status"].as_str()).unwrap_or("?");
                        Some(TreeNode {
                            task,
                            parent: opt_str(r, "parent"),
                            agent: r["agent"].as_str().unwrap_or("?").to_owned(),
                            kind: StageKind::parse(r["kind"].as_str().unwrap_or("node")),
                            status: TaskStatus::parse(status),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let leaf_results = value["leaf_results"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| LeafResult {
                        task: r["task"].as_str().unwrap_or("?").to_owned(),
                        agent: r["agent"].as_str().unwrap_or("?").to_owned(),
                        status: TaskStatus::parse(r["status"].as_str().unwrap_or("?")),
                        result: opt_str(r, "result"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let subtree_receipts = value["subtree_receipts"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| SubtreeReceipt {
                        task: r["task"].as_str().unwrap_or("?").to_owned(),
                        agent: r["agent"].as_str().unwrap_or("?").to_owned(),
                        op: r["op"].as_str().unwrap_or("?").to_owned(),
                        label: r["label"].as_str().unwrap_or("").to_owned(),
                        ok: r["ok"].as_bool(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let clarifications = value["clarifications"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some(Clarification {
                            question: r["question"].as_str()?.to_owned(),
                            answer: r["answer"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rounds = value["rounds"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some(RoundRecord {
                            round: r["round"].as_u64()?,
                            subtree_root: r["subtree_root"].as_str()?.to_owned(),
                            prompt: r["prompt"].as_str()?.to_owned(),
                            check: opt_str(r, "check"),
                            outcome: opt_str(r, "outcome").as_deref().map(TaskStatus::parse),
                            verification: opt_str(r, "verification").as_deref().map(Verification::parse),
                            result: opt_str(r, "result"),
                            continuation_node: opt_str(r, "continuation_node"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let whiteboard = value["whiteboard"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| {
                        Some(WhiteboardEntry {
                            key: r["key"].as_str()?.to_owned(),
                            kind: r["kind"].as_str().unwrap_or("note").to_owned(),
                            content: r["content"].as_str().unwrap_or("").to_owned(),
                            author_agent: r["author_agent"].as_str().unwrap_or("?").to_owned(),
                            round: r["round"].as_u64().unwrap_or(1),
                            version: r["version"].as_u64().unwrap_or(1),
                            seq: r["seq"].as_u64().unwrap_or(0),
                            status: r["status"].as_str().unwrap_or("active").to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(Self {
            id,
            root,
            parent,
            agent,
            status,
            stage_status: opt_str(value, "stage_status").as_deref().map(TaskStatus::parse),
            kind,
            prompt: opt_str(value, "prompt"),
            result: opt_str(value, "result"),
            check: opt_str(value, "check"),
            proposal: opt_str(value, "proposal"),
            action: ActionPreview::from_value(&value["action"]),
            approval_id: opt_str(value, "approval_id"),
            tool_state: opt_str(value, "tool_state"),
            turns_used: value["turns_used"].as_u64(),
            turn_budget: value["turn_budget"].as_u64(),
            policy: Policy::parse(value["policy"].as_str()),
            sandbox: opt_str(value, "sandbox"),
            question: opt_str(value, "question"),
            stage_error: opt_str(value, "stage_error"),
            verification: opt_str(value, "verification").as_deref().map(Verification::parse),
            verification_note: opt_str(value, "verification_note"),
            round: value["round"].as_u64(),
            active_root: opt_str(value, "active_root"),
            continuation_node: opt_str(value, "continuation_node"),
            continuation_agent: opt_str(value, "continuation_agent"),
            continuation_kind: opt_str(value, "continuation_kind"),
            whiteboard_seq: value["whiteboard_seq"].as_u64().unwrap_or(0),
            whiteboard,
            evidence,
            tree_nodes,
            leaf_results,
            subtree_receipts,
            clarifications,
            rounds,
        })
    }

    pub fn is_root(&self) -> bool {
        self.parent.is_none()
    }

    pub fn is_waiting_input(&self) -> bool {
        self.status == TaskStatus::Input && self.is_root()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Member {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelConfig {
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl ModelConfig {
    pub fn from_value(value: &Value) -> Result<Self> {
        Ok(Self {
            provider: field(value, "provider")?.to_owned(),
            model: opt_str(value, "model"),
            effort: opt_str(value, "effort"),
        })
    }
}

pub(crate) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("invalid daemon response: missing {key}"))
}

fn opt_str(value: &Value, key: &str) -> Option<String> {
    value[key].as_str().map(str::to_owned)
}

pub(crate) fn is_terminal_str(status: &str) -> bool {
    matches!(status, "done" | "failed" | "cancelled" | "unknown")
}

pub(crate) fn outcome_label(ok: Option<bool>) -> &'static str {
    match ok {
        Some(true) => "ok",
        Some(false) => "failed",
        None => "unknown",
    }
}

/// Neutralize terminal controls without altering JSON structure or intentional newlines/tabs.
pub(crate) fn sanitize_terminal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() && !matches!(c, '\n' | '\t') {
            out.push_str(&format!("\\u{:04x}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
}

/// Normalize line breaks and expand tabs while neutralizing terminal control sequences.
pub(crate) fn sanitize_multiline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out
}

/// Flatten text to one terminal-safe single line.
pub(crate) fn sanitize_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            if matches!(c, '\n' | '\r' | '\t') {
                out.push(' ');
            } else {
                out.push_str(&format!("\\u{:04x}", u32::from(c)));
            }
        } else if c.is_whitespace() {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Keep human feedback readable even for commands with pages of output. JSON keeps the full receipt.
pub(crate) fn preview(text: &str, max_chars: usize) -> String {
    let mut chars = text.trim().chars().map(|c| if c.is_whitespace() { ' ' } else { c });
    let mut short: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        short.push('…');
    }
    short
}

// FNV-1a is deliberately fixed so upgrading Rust does not change the default session.
fn workspace_key(path: &Path) -> String {
    let hash = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3));
    format!("{hash:016x}")
}

#[derive(Clone)]
pub(crate) struct Client {
    pub session: PathBuf,
    pub workspace: PathBuf,
}

impl Client {
    pub fn new(session: Option<&Path>, workspace: Option<&Path>) -> Result<Self> {
        let cwd = env::current_dir().map_err(|e| e.to_string())?;
        let workspace = cwd.join(workspace.unwrap_or(Path::new(".")));
        let workspace = workspace.canonicalize().map_err(|e| format!("workspace {}: {e}", workspace.display()))?;
        let session = if let Some(path) = session {
            cwd.join(path)
        } else {
            let state = env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| env::var_os("HOME").filter(|p| !p.is_empty()).map(|p| cwd.join(p).join(".local/state")))
                .ok_or("set XDG_STATE_HOME, HOME, or --session")?;
            state.join("teamagents-swarm/sessions").join(workspace_key(&workspace))
        };
        Ok(Self { session, workspace })
    }

    pub fn call(&self, body: Value) -> Result<Value> {
        daemon::request(&self.session, &self.workspace, &body)
    }

    pub fn tasks_raw(&self) -> Result<Vec<Value>> {
        match self.call(json!({"op":"tasks"}))? {
            Value::Array(rows) => Ok(rows),
            _ => Err("invalid tasks response".into()),
        }
    }

    pub fn tasks(&self) -> Result<Vec<TaskRecord>> {
        self.tasks_raw()?.iter().map(TaskRecord::from_value).collect()
    }

    pub fn status(&self, id: &str) -> Result<TaskRecord> {
        TaskRecord::from_value(&self.call(json!({"op":"status", "id":id}))?)
    }

    pub fn members(&self) -> Result<Vec<Member>> {
        let Value::Array(rows) = self.call(json!({"op":"members"}))? else {
            return Err("invalid members response".into());
        };
        rows.into_iter().map(|r| Ok(Member { name: field(&r, "name")?.to_owned() })).collect()
    }

    pub fn model_get(&self) -> Result<ModelConfig> {
        ModelConfig::from_value(&self.call(json!({"op":"model"}))?)
    }

    pub fn model_set(&self, model: Option<Option<&str>>, effort: Option<Option<&str>>) -> Result<ModelConfig> {
        let mut body = json!({"op":"model"});
        if let Some(model) = model {
            body["model"] = match model {
                Some(id) => json!(id),
                None => Value::Null,
            };
        }
        if let Some(effort) = effort {
            body["effort"] = match effort {
                Some(e) => json!(e),
                None => Value::Null,
            };
        }
        ModelConfig::from_value(&self.call(body)?)
    }

    pub fn submit_with_options(
        &self,
        agent: &str,
        text: &str,
        review: bool,
        check: Option<&str>,
        staged: bool,
    ) -> Result<Value> {
        if text.trim().is_empty() {
            return Err("a nonempty task prompt is required".into());
        }
        let mut body = json!({"op":"submit", "agent":agent, "text":text, "review":review});
        if staged {
            body["staged"] = json!(true);
        }
        if let Some(command) = check {
            body["check"] = json!(command);
        }
        let receipt = self.call(body)?;
        field(&receipt, "id")?;
        Ok(receipt)
    }

    pub fn continue_with_options(
        &self,
        id: &str,
        text: &str,
        review: bool,
        check: Option<&str>,
        staged: bool,
    ) -> Result<Value> {
        if text.trim().is_empty() {
            return Err("a nonempty task prompt is required".into());
        }
        let mut body = json!({"op":"continue", "id":id, "text":text, "review":review});
        if staged {
            body["staged"] = json!(true);
        }
        if let Some(command) = check {
            body["check"] = json!(command);
        }
        let receipt = self.call(body)?;
        field(&receipt, "id")?;
        Ok(receipt)
    }

    pub fn wb_write(
        &self,
        id: &str,
        key: &str,
        mode: &str,
        kind: Option<&str>,
        content: Option<&str>,
    ) -> Result<Value> {
        let mut body = json!({"op":"wb_write", "id":id, "key":key, "mode":mode});
        if let Some(k) = kind {
            body["kind"] = json!(k);
        }
        if let Some(c) = content {
            body["content"] = json!(c);
        }
        self.call(body)
    }

    pub fn answer(&self, id: &str, text: &str) -> Result<Value> {
        self.call(json!({"op":"answer", "id":id, "text":text}))
    }

    pub fn approve(&self, approval_id: &str, allow: bool) -> Result<Value> {
        let op = if allow { "approve" } else { "deny" };
        self.call(json!({"op":op, "id":approval_id}))
    }

    pub fn cancel(&self, id: &str) -> Result<Value> {
        self.call(json!({"op":"cancel", "id":id}))
    }

    pub fn delete(&self, id: &str) -> Result<Value> {
        self.call(json!({"op":"delete", "id":id}))
    }

    pub fn stop(&self) -> Result<Value> {
        self.call(json!({"op":"stop"}))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TrackerNotice {
    Input { id: String, agent: String, question: String },
    Approval { id: String, agent: String, summary: String },
    Terminal { id: String, agent: String, status: TaskStatus },
}

/// Track waiting clarification questions across clients so plain text replies
/// never get silently rerouted when a question closes in another client.
#[derive(Default)]
pub(crate) struct QuestionTracker {
    seen: HashMap<String, (String, Option<String>)>,
    questions: HashMap<String, String>, // announced root ID -> recipient
    initialized: bool,
}

impl QuestionTracker {
    fn record(
        &mut self,
        id: String,
        status: &str,
        is_root: bool,
        agent: Option<&str>,
        detail: Option<&str>,
    ) -> Result<bool> {
        let waiting = status == "input" && is_root;
        let key = (status.to_owned(), detail.map(str::to_owned));
        if self.seen.get(&id) == Some(&key) {
            return Ok(false);
        }
        let notify = waiting || status == "approval" || (self.initialized && is_root && is_terminal_str(status));
        if waiting {
            let recipient = agent.filter(|s| !s.is_empty()).ok_or("invalid daemon response: missing agent")?;
            self.questions.insert(id.clone(), recipient.to_owned());
        }
        self.seen.insert(id, key);
        Ok(notify)
    }

    pub fn update_raw(&mut self, tasks: Vec<Value>, mut show: impl FnMut(&Value) -> Result<()>) -> Result<()> {
        for task in tasks {
            let id = field(&task, "id")?.to_owned();
            let status = field(&task, "status")?;
            let is_root = task["parent"].is_null();
            let detail = if status == "input" && is_root { &task["question"] } else { &task["approval_id"] };
            if self.record(id, status, is_root, task["agent"].as_str(), detail.as_str())? {
                show(&task)?;
            }
        }
        self.initialized = true;
        Ok(())
    }

    pub fn sync(&mut self, tasks: &[TaskRecord]) -> Vec<TrackerNotice> {
        let mut notices = Vec::new();
        for task in tasks {
            let status = task.status.as_str();
            let is_root = task.is_root();
            let detail = if task.is_waiting_input() { task.question.as_deref() } else { task.approval_id.as_deref() };
            if self.record(task.id.clone(), status, is_root, Some(task.agent.as_str()), detail).unwrap_or(false) {
                if task.is_waiting_input() {
                    notices.push(TrackerNotice::Input {
                        id: task.id.clone(),
                        agent: task.agent.clone(),
                        question: task.question.clone().unwrap_or_default(),
                    });
                } else if task.status == TaskStatus::Approval {
                    let summary = task
                        .action
                        .as_ref()
                        .map(|a| format!("{} {}", a.op(), preview(a.label(), 60)))
                        .unwrap_or_else(|| "pending action".into());
                    notices.push(TrackerNotice::Approval { id: task.id.clone(), agent: task.agent.clone(), summary });
                } else if is_root && task.status.is_terminal() {
                    notices.push(TrackerNotice::Terminal {
                        id: task.id.clone(),
                        agent: task.agent.clone(),
                        status: task.status.clone(),
                    });
                }
            }
        }
        self.initialized = true;
        notices
    }

    pub fn clear_answered(&mut self, id: &str) {
        self.questions.remove(id);
        self.seen.remove(id);
    }

    pub fn consume_stale_root(&mut self, root_id: &str) -> bool {
        if self.questions.contains_key(root_id) && !self.seen.get(root_id).is_some_and(|(status, _)| status == "input")
        {
            self.questions.remove(root_id);
            return true;
        }
        false
    }

    pub fn reply_target(&mut self, agent: &str) -> Result<Option<String>> {
        // A different client may have answered or cancelled while this user typed.
        // Warn once before treating a line as a new task or redirecting it to the remaining root.
        let announced = self.questions.len();
        self.questions.retain(|id, _| self.seen.get(id).is_some_and(|(status, _)| status == "input"));
        if self.questions.len() != announced {
            return Err(
                "A waiting question closed in another client. Nothing was sent. Use /answer ROOT_ID TEXT or /new TEXT."
                    .into(),
            );
        }
        let mut questions = self.questions.iter();
        match questions.next() {
            None => Ok(None),
            Some((id, recipient)) if recipient == agent && questions.next().is_none() => Ok(Some(id.clone())),
            _ => Err("Specify /answer ROOT_ID TEXT for a waiting question or /new TEXT for a separate task.".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rich_root_status_with_clarifications_and_unknown_future_states() {
        let raw = json!({
            "id": "r1",
            "root": "r1",
            "parent": null,
            "agent": "default",
            "status": "done",
            "stage_status": "done",
            "kind": "root",
            "prompt": "Fix parser",
            "result": "Parser fixed",
            "check": "cargo test",
            "policy": "review",
            "turns_used": 3,
            "turn_budget": 32,
            "verification": "passed",
            "round": 2,
            "active_root": "s1",
            "continuation_node": "s1",
            "continuation_agent": "summarizer",
            "continuation_kind": "summarizer",
            "clarifications": [{"question": "Which target?", "answer": "Linux"}],
            "rounds": [{
                "round": 1,
                "subtree_root": "r1",
                "prompt": "Fix parser",
                "check": "cargo test",
                "outcome": "done",
                "verification": "passed",
                "result": "Parser fixed",
                "continuation_node": "s1"
            }],
            "evidence": [{"action": {"op": "verify", "command": "cargo test"}, "ok": true, "result": "ok"}],
            "tree_nodes": [
                {"task": "r1", "parent": null, "agent": "default", "kind": "root", "status": "done"},
                {"task": "c1", "parent": "r1", "agent": "tester", "kind": "node", "status": "future_state"},
                {"task": "s1", "parent": "r1", "agent": "summarizer-1", "kind": "summarizer", "status": "done"}
            ],
            "leaf_results": [{"task": "c1", "agent": "tester", "status": "done", "result": "tested"}],
            "subtree_receipts": [{"task": "c1", "agent": "tester", "op": "shell", "label": "cargo test", "ok": true}]
        });
        let record = TaskRecord::from_value(&raw).unwrap();
        assert_eq!(record.status, TaskStatus::Done);
        assert_eq!(record.policy, Some(Policy::Review));
        assert_eq!(record.verification, Some(Verification::Passed));
        assert_eq!(record.round, Some(2));
        assert_eq!(record.active_root.as_deref(), Some("s1"));
        assert_eq!(record.continuation_node.as_deref(), Some("s1"));
        assert_eq!(record.continuation_agent.as_deref(), Some("summarizer"));
        assert_eq!(record.continuation_kind.as_deref(), Some("summarizer"));
        assert_eq!(record.rounds.len(), 1);
        assert_eq!(record.rounds[0].outcome, Some(TaskStatus::Done));
        assert_eq!(record.clarifications.len(), 1);
        assert_eq!(record.clarifications[0].answer, "Linux");
        assert_eq!(record.tree_nodes[1].status, TaskStatus::Other("future_state".into()));
        assert_eq!(record.tree_nodes[2].kind, StageKind::Summarizer);
    }

    #[test]
    fn sanitizers_neutralize_ansi_and_control_sequences() {
        let dirty = "hello\x1b[2J\u{9b}31m\r\n\tworld\x07";
        assert_eq!(sanitize_terminal(dirty), "hello\\u001b[2J\\u009b31m\\u000d\n\tworld\\u0007");
        assert_eq!(sanitize_multiline(dirty), "hello\\u001b[2J\\u009b31m\n    world\\u0007");
        assert_eq!(sanitize_inline(dirty), "hello\\u001b[2J\\u009b31m   world\\u0007");
    }

    #[test]
    fn tracker_prevents_stale_reply_rerouting_across_sync_and_raw() {
        let mut tracker = QuestionTracker::default();
        let first = TaskRecord::from_value(&json!({"id":"r1","agent":"a","status":"input","question":"Q1?"})).unwrap();
        let second = TaskRecord::from_value(&json!({"id":"r2","agent":"a","status":"input","question":"Q2?"})).unwrap();
        let notices = tracker.sync(&[first.clone(), second]);
        assert_eq!(notices.len(), 2);
        assert!(tracker.reply_target("a").is_err());
        let second_closed = TaskRecord::from_value(&json!({"id":"r2","agent":"a","status":"queued"})).unwrap();
        tracker.sync(&[first, second_closed]);
        assert!(tracker.reply_target("a").unwrap_err().contains("Nothing was sent"));
        assert_eq!(tracker.reply_target("a").unwrap().as_deref(), Some("r1"));
        tracker.clear_answered("r1");
        assert!(tracker.reply_target("a").unwrap().is_none());
    }
}
