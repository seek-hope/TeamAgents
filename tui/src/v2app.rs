//! R2-P4 R19-b② v2 conversation interface state and key handling (plan §9).
//! Pure logic like app.rs: side effects leave as `V2Effect`s executed by the
//! main loop through the daemon client, so every path is unit-testable
//! without a terminal or daemon process. The TUI never executes anything
//! itself — history is the authoritative conversation, events are signals,
//! and system notes are UI-level annotations on top.

use serde_json::Value as Json;

use crate::text::Composer;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Composer,
    Approvals,
}

/// Top-level views (§9: 实例、任务/权限视图、拓扑边列表). F1/F3/F4/F5
/// switch globally; Esc from a panel returns to the conversation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Chat,
    Instances,
    Tasks,
    Topology,
}

impl View {
    pub fn name(self) -> &'static str {
        match self {
            View::Chat => "对话",
            View::Instances => "实例",
            View::Tasks => "任务",
            View::Topology => "拓扑",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatKind {
    User,
    Assistant,
    Tool,
    /// A runtime compaction summary (R22/A20): the model's view is covered by
    /// it, so the user must see where the conversation was compacted.
    Summary,
    System,
    Error,
}

#[derive(Clone, Debug)]
pub struct ChatEntry {
    pub kind: ChatKind,
    pub who: String,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct InstanceInfo {
    pub id: String,
    pub lifecycle: String,
    pub phase: String,
}

#[derive(Clone, Debug)]
pub struct ApprovalInfo {
    pub id: String,
    pub operation_id: String,
    pub tool: String,
    pub preview: String,
}

#[derive(Clone, Debug)]
pub struct TaskInfo {
    pub id: String,
    pub goal_id: String,
    pub assignee: String,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct GrantInfo {
    pub subject: String,
    pub action: String,
    pub scope: String,
    pub revoked: bool,
}

/// Pending destructive confirmation (termination stays deliberate, §5.4):
/// y confirms, n/Esc cancels; every other key is swallowed meanwhile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    TerminateInstance { instance: String },
}

#[derive(Clone, Debug)]
pub struct GoalInfo {
    pub status: String,
    pub known_total: i64,
    pub unknown: bool,
    pub limit_total: Option<i64>,
}

/// Side effects for the main loop to execute through the daemon client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum V2Effect {
    SubmitInput {
        instance: String,
        envelope: String,
        text: String,
    },
    Decide {
        approval_id: String,
        decision: &'static str,
    },
    /// Instance lifecycle intervention from the panel (§5.4): the main loop
    /// sends set_lifecycle as Identity::User with a fresh command id.
    SetLifecycle {
        instance: String,
        lifecycle: &'static str,
    },
    /// Cancel a non-terminal task from the panel (§5.3).
    CancelTask {
        task_id: String,
    },
    Quit,
}

/// What an applied event batch asks the main loop to refresh.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Refresh {
    pub history: bool,
    pub approvals: bool,
    pub checkpoint: bool,
    pub tasks: bool,
    pub grants: bool,
}

/// Retained system-note cap: notes are UI signals layered over the
/// authoritative history; the cap keeps a long session bounded.
const NOTE_CAP: usize = 200;
/// Tool-result preview cap in the conversation (界面只取预览, §9).
const PREVIEW_CHARS: usize = 400;

pub struct V2App {
    pub session_id: String,
    /// Current top-level view (§9 panels); Chat is the default.
    pub view: View,
    pub instances: Vec<InstanceInfo>,
    /// Index into `instances`: the conversation shown and input target.
    pub active: usize,
    pub entries: Vec<ChatEntry>,
    pub goal: Option<GoalInfo>,
    pub approvals: Vec<ApprovalInfo>,
    pub approval_sel: usize,
    pub focus: Focus,
    pub composer: Composer,
    pub disconnected: bool,
    /// Selection inside the instances panel (Enter promotes it to `active`).
    pub instance_sel: usize,
    pub tasks: Vec<TaskInfo>,
    pub task_sel: usize,
    pub grants: Vec<GrantInfo>,
    /// Topology edge-list scroll offset (§9: 拓扑先用边列表表达).
    pub topo_scroll: usize,
    pub confirm: Option<Confirm>,
    /// Scroll offset in wrapped lines from the bottom (0 = following).
    pub chat_scroll: usize,
    pub watermark: i64,
    pub quit: bool,
    /// Total wrapped chat lines at the last render (scroll clamping).
    pub last_chat_lines: usize,
    /// Visible chat height at the last render (page scrolling).
    pub last_chat_height: usize,
}

impl V2App {
    pub fn new(session_id: &str) -> V2App {
        V2App {
            session_id: session_id.to_string(),
            view: View::Chat,
            instances: vec![],
            active: 0,
            entries: vec![],
            goal: None,
            approvals: vec![],
            approval_sel: 0,
            focus: Focus::Composer,
            composer: Composer::new(vec![]),
            disconnected: false,
            instance_sel: 0,
            tasks: vec![],
            task_sel: 0,
            grants: vec![],
            topo_scroll: 0,
            confirm: None,
            chat_scroll: 0,
            watermark: 0,
            quit: false,
            last_chat_lines: 0,
            last_chat_height: 1,
        }
    }

    pub fn active_instance(&self) -> Option<&InstanceInfo> {
        self.instances.get(self.active)
    }

    fn note(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry { kind: ChatKind::System, who: "系统".into(), text: text.into() });
        if self.entries.len() > NOTE_CAP * 4 {
            let drop = self.entries.len() - NOTE_CAP * 4;
            self.entries.drain(..drop);
        }
    }

    fn note_error(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry { kind: ChatKind::Error, who: "错误".into(), text: text.into() });
    }

    // ---- daemon → app application points --------------------------------

    /// Checkpoint (connect/reconnect): snapshot state and the watermark the
    /// event stream resumes from — one consistent read (§9).
    pub fn apply_checkpoint(&mut self, snapshot: Json, watermark: i64) {
        let previous = self.active_instance().map(|i| i.id.clone());
        self.instances = snapshot["instances"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|i| InstanceInfo {
                id: i["id"].as_str().unwrap_or("").to_string(),
                lifecycle: i["lifecycle"].as_str().unwrap_or("").to_string(),
                phase: i["phase"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        self.active = previous
            .and_then(|id| self.instances.iter().position(|i| i.id == id))
            // the leader is the conversation default (daemon CLI: i-leader)
            .or_else(|| self.instances.iter().position(|i| i.id == "i-leader"))
            .unwrap_or(0);
        self.goal = snapshot.get("goal").and_then(|g| {
            if g.is_null() {
                return None;
            }
            let known = &g["known_usage"];
            Some(GoalInfo {
                status: g["status"].as_str().unwrap_or("").to_string(),
                known_total: known["total"].as_i64().unwrap_or(0),
                unknown: g["unknown_usage"].as_i64().unwrap_or(0) != 0,
                limit_total: g["limits"]["max_total_tokens"].as_i64(),
            })
        });
        if self.instance_sel >= self.instances.len() {
            self.instance_sel = self.instances.len().saturating_sub(1);
        }
        self.watermark = self.watermark.max(watermark);
        // a fresh checkpoint supersedes earlier UI-level notes
        self.entries.retain(|e| e.kind != ChatKind::System && e.kind != ChatKind::Error);
    }

    /// History of the active instance: the authoritative conversation (§9 —
    /// 界面只取预览). System notes stay layered on top, newest last.
    pub fn apply_history(&mut self, history: Json) {
        let mut rebuilt: Vec<ChatEntry> = Vec::new();
        for entry in history["entries"].as_array().cloned().unwrap_or_default() {
            let kind = entry["kind"].as_str().unwrap_or("");
            let message = &entry["message"];
            match kind {
                "user" => rebuilt.push(ChatEntry {
                    kind: ChatKind::User,
                    who: "你".into(),
                    text: message["content"].as_str().unwrap_or("").to_string(),
                }),
                "assistant" => {
                    let who = self.active_instance().map(|i| i.id.clone()).unwrap_or_default();
                    let content = message["content"].as_str().unwrap_or("").trim().to_string();
                    if !content.is_empty() {
                        rebuilt.push(ChatEntry { kind: ChatKind::Assistant, who: who.clone(), text: content });
                    }
                    for call in message["tool_calls"].as_array().cloned().unwrap_or_default() {
                        let name = call["function"]["name"].as_str().unwrap_or("?");
                        let args = call["function"]["arguments"].as_str().unwrap_or("");
                        let preview = tool_preview(name, args);
                        rebuilt.push(ChatEntry { kind: ChatKind::Tool, who: who.clone(), text: preview });
                    }
                }
                "tool_result" => {
                    let content = message["content"].as_str().unwrap_or("");
                    rebuilt.push(ChatEntry {
                        kind: ChatKind::Tool,
                        who: "结果".into(),
                        text: truncate(content, PREVIEW_CHARS),
                    });
                }
                // covered entries stay visible here: the user sees the whole
                // conversation, the model sees the summary (R22/A20)
                "summary" => rebuilt.push(ChatEntry {
                    kind: ChatKind::Summary,
                    who: "压缩".into(),
                    text: truncate(message["content"].as_str().unwrap_or(""), PREVIEW_CHARS),
                }),
                _ => {}
            }
        }
        let notes: Vec<ChatEntry> =
            self.entries.drain(..).filter(|e| e.kind == ChatKind::System || e.kind == ChatKind::Error).collect();
        self.entries = rebuilt;
        self.entries.extend(notes);
        if self.chat_scroll == 0 {
            self.last_chat_lines = 0; // follow: force the render to re-clamp
        }
    }

    /// Live events: conversation content stays in history (fetched on
    /// demand); events drive refreshes and UI-level system notes.
    pub fn apply_events(&mut self, events: &[Json]) -> Refresh {
        let mut refresh = Refresh::default();
        for event in events {
            if let Some(sequence) = event["sequence"].as_i64() {
                self.watermark = self.watermark.max(sequence);
            }
            let kind = event["kind"].as_str().unwrap_or("");
            let payload = &event["payload"];
            match kind {
                "input" | "response_imported" | "decision_consumed" | "completion_closed" => {
                    refresh.history = true;
                }
                "goal_completed" => {
                    refresh.history = true;
                    refresh.checkpoint = true;
                    self.note(format!("目标完成：{}", payload["status"].as_str().unwrap_or("?")));
                }
                "goal_blocked" => {
                    refresh.checkpoint = true;
                    self.note(format!("目标停放（BLOCKED）：{}", payload["reason"].as_str().unwrap_or("")));
                }
                "check_round_registered" => {
                    let count = payload["checks"].as_array().map(|c| c.len()).unwrap_or(0);
                    self.note(format!("完成检查第 {} 轮开始（{count} 项）", payload["round"].as_i64().unwrap_or(0)));
                }
                "completion_repair" => {
                    refresh.history = true;
                    self.note(format!("完成检查第 {} 轮未过，进入修复回合", payload["round"].as_i64().unwrap_or(0)));
                }
                "instance_spawned" => {
                    refresh.checkpoint = true;
                    self.note(format!(
                        "实例 {} 由 {} 派出",
                        event["scope"].as_str().unwrap_or("?"),
                        payload["spawner"].as_str().unwrap_or("?")
                    ));
                }
                "instance_lifecycle" => {
                    refresh.checkpoint = true;
                    self.note(format!(
                        "实例 {} 生命周期 → {}",
                        event["scope"].as_str().unwrap_or("?"),
                        payload["lifecycle"].as_str().unwrap_or("?")
                    ));
                }
                "instance_created" => {
                    refresh.checkpoint = true;
                    self.note(format!("实例 {} 创建", event["scope"].as_str().unwrap_or("?")));
                }
                "task_delegated" | "task_started" | "task_completed" => {
                    refresh.tasks = true;
                }
                "task_blocked" => {
                    refresh.tasks = true;
                    self.note(format!(
                        "任务 {} 停放（BLOCKED）：{}",
                        payload["task_id"].as_str().unwrap_or("?"),
                        payload["reason"].as_str().unwrap_or("")
                    ));
                }
                "task_cancelled" => {
                    refresh.tasks = true;
                    self.note(format!(
                        "任务 {} 已取消：{}",
                        payload["task_id"].as_str().unwrap_or("?"),
                        payload["reason"].as_str().unwrap_or("")
                    ));
                }
                "grant_issued" | "grant_revoked" => {
                    refresh.grants = true;
                }
                "approval_requested" | "approval_granted" | "approval_denied" => {
                    refresh.approvals = true;
                }
                "operation_completed" if payload["status"].as_str() == Some("OUTCOME_UNKNOWN") => {
                    self.note(format!("操作 {} 结果不明（未重放）", event["scope"].as_str().unwrap_or("?")));
                }
                _ => {}
            }
        }
        refresh
    }

    pub fn apply_approvals(&mut self, result: Json) {
        self.approvals = result["approvals"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| ApprovalInfo {
                id: a["id"].as_str().unwrap_or("").to_string(),
                operation_id: a["operation_id"].as_str().unwrap_or("").to_string(),
                tool: a["tool"].as_str().unwrap_or("").to_string(),
                preview: a["preview"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        if self.approval_sel >= self.approvals.len() {
            self.approval_sel = self.approvals.len().saturating_sub(1);
        }
        if self.approvals.is_empty() && self.focus == Focus::Approvals {
            self.focus = Focus::Composer;
        }
    }

    pub fn apply_tasks(&mut self, result: Json) {
        self.tasks = result["tasks"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|t| TaskInfo {
                id: t["id"].as_str().unwrap_or("").to_string(),
                goal_id: t["goal_id"].as_str().unwrap_or("").to_string(),
                assignee: t["assignee"].as_str().unwrap_or("").to_string(),
                status: t["status"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        if self.task_sel >= self.tasks.len() {
            self.task_sel = self.tasks.len().saturating_sub(1);
        }
    }

    pub fn apply_grants(&mut self, result: Json) {
        self.grants = result["grants"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|g| GrantInfo {
                subject: g["subject"].as_str().unwrap_or("").to_string(),
                action: g["action"].as_str().unwrap_or("").to_string(),
                scope: g["resource_scope"].as_str().unwrap_or("").to_string(),
                revoked: g["revoked"].as_bool().unwrap_or(false),
            })
            .collect();
        // the topology render clamps topo_scroll against the visible height
    }

    /// A panel command (lifecycle/task) the daemon refused (write-surface error).
    pub fn command_failed(&mut self, error: &str) {
        self.note_error(format!("命令失败：{error}"));
    }

    pub fn mark_disconnected(&mut self, error: &str) {
        if !self.disconnected {
            self.disconnected = true;
            self.note_error(format!("与 daemon 断开（{error}），重连中…"));
        }
    }

    pub fn mark_connected(&mut self) {
        if self.disconnected {
            self.disconnected = false;
            self.note("已重新连接 daemon");
        }
    }

    /// A submitted input the daemon refused (write-surface error).
    pub fn submit_failed(&mut self, error: &str) {
        self.note_error(format!("发送失败：{error}"));
    }

    pub fn decide_failed(&mut self, error: &str) {
        self.note_error(format!("批准决定失败：{error}"));
    }

    // ---- keys ------------------------------------------------------------

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            self.quit = true;
            return Some(V2Effect::Quit);
        }
        // a pending destructive confirmation swallows keys until resolved
        if self.confirm.is_some() {
            return self.confirm_key(key);
        }
        // view switching is global (F1 returns to the conversation)
        match key.code {
            KeyCode::F(1) => {
                self.view = View::Chat;
                return None;
            }
            KeyCode::F(3) => {
                self.view = View::Instances;
                self.instance_sel = self.active;
                return None;
            }
            KeyCode::F(4) => {
                self.view = View::Tasks;
                return None;
            }
            KeyCode::F(5) => {
                self.view = View::Topology;
                return None;
            }
            _ => {}
        }
        match self.view {
            View::Chat => match self.focus {
                Focus::Approvals => self.approval_key(key),
                Focus::Composer => self.composer_key(key),
            },
            View::Instances => self.instances_key(key),
            View::Tasks => self.tasks_key(key),
            View::Topology => self.topology_key(key),
        }
    }

    fn confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        let pending = self.confirm.clone()?;
        match key.code {
            KeyCode::Char('y') => {
                self.confirm = None;
                match pending {
                    Confirm::TerminateInstance { instance } => {
                        Some(V2Effect::SetLifecycle { instance, lifecycle: "TERMINATED" })
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirm = None;
                None
            }
            _ => None,
        }
    }

    fn instances_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => {
                self.view = View::Chat;
                None
            }
            KeyCode::Up => {
                self.instance_sel = self.instance_sel.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.instance_sel + 1 < self.instances.len() {
                    self.instance_sel += 1;
                }
                None
            }
            // choose the conversation target and jump back to the chat
            KeyCode::Enter => {
                if self.instance_sel < self.instances.len() {
                    self.active = self.instance_sel;
                    self.chat_scroll = 0;
                    self.view = View::Chat;
                }
                None
            }
            KeyCode::Char('p') => self.lifecycle_selected("PAUSED"),
            KeyCode::Char('r') => self.lifecycle_selected("ACTIVE"),
            // termination is irreversible for the session: confirm first (§5.4)
            KeyCode::Char('t') => {
                if let Some(instance) = self.instances.get(self.instance_sel) {
                    self.confirm = Some(Confirm::TerminateInstance { instance: instance.id.clone() });
                }
                None
            }
            _ => None,
        }
    }

    fn lifecycle_selected(&mut self, lifecycle: &'static str) -> Option<V2Effect> {
        let instance = self.instances.get(self.instance_sel)?.id.clone();
        Some(V2Effect::SetLifecycle { instance, lifecycle })
    }

    fn tasks_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => {
                self.view = View::Chat;
                None
            }
            KeyCode::Up => {
                self.task_sel = self.task_sel.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.task_sel + 1 < self.tasks.len() {
                    self.task_sel += 1;
                }
                None
            }
            KeyCode::Char('c') => {
                let task = self.tasks.get(self.task_sel)?;
                // terminal tasks have nothing left to cancel
                if matches!(task.status.as_str(), "SUCCEEDED" | "FAILED" | "CANCELLED") {
                    return None;
                }
                Some(V2Effect::CancelTask { task_id: task.id.clone() })
            }
            _ => None,
        }
    }

    fn topology_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => {
                self.view = View::Chat;
                None
            }
            KeyCode::Up => {
                self.topo_scroll = self.topo_scroll.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                self.topo_scroll += 1; // the render clamps against the height
                None
            }
            KeyCode::PageUp => {
                self.topo_scroll = self.topo_scroll.saturating_sub(self.last_chat_height.max(1));
                None
            }
            KeyCode::PageDown => {
                self.topo_scroll += self.last_chat_height.max(1);
                None
            }
            _ => None,
        }
    }

    fn approval_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Composer;
                None
            }
            KeyCode::Up => {
                self.approval_sel = self.approval_sel.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.approval_sel + 1 < self.approvals.len() {
                    self.approval_sel += 1;
                }
                None
            }
            KeyCode::Char('a') => self.decide_selected("approve"),
            KeyCode::Char('d') => self.decide_selected("deny"),
            _ => None,
        }
    }

    fn decide_selected(&mut self, decision: &'static str) -> Option<V2Effect> {
        let approval = self.approvals.get(self.approval_sel)?;
        Some(V2Effect::Decide { approval_id: approval.id.clone(), decision })
    }

    fn composer_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Enter => {
                let text = self.composer.submit()?;
                let instance = self.active_instance()?.id.clone();
                let envelope = format!("env-{}", uuid::Uuid::new_v4());
                Some(V2Effect::SubmitInput { instance, envelope, text })
            }
            KeyCode::Tab => {
                if !self.instances.is_empty() {
                    self.active = (self.active + 1) % self.instances.len();
                    self.chat_scroll = 0;
                }
                None
            }
            KeyCode::F(2) => {
                if !self.approvals.is_empty() {
                    self.focus = Focus::Approvals;
                }
                None
            }
            KeyCode::PageUp => {
                self.scroll_chat(self.last_chat_height.max(1));
                None
            }
            KeyCode::PageDown => {
                self.scroll_chat_back(self.last_chat_height.max(1));
                None
            }
            KeyCode::Up => {
                self.scroll_chat(1);
                None
            }
            KeyCode::Down => {
                self.scroll_chat_back(1);
                None
            }
            KeyCode::Backspace => {
                self.composer.backspace();
                None
            }
            KeyCode::Delete => {
                self.composer.delete();
                None
            }
            KeyCode::Left => {
                self.composer.move_left();
                None
            }
            KeyCode::Right => {
                self.composer.move_right();
                None
            }
            KeyCode::Home => {
                self.composer.move_home();
                None
            }
            KeyCode::End => {
                self.composer.move_end();
                None
            }
            KeyCode::Char(c) => {
                self.composer.insert_char(c);
                None
            }
            _ => None,
        }
    }

    pub fn scroll_chat(&mut self, lines: usize) {
        let max = self.last_chat_lines.saturating_sub(self.last_chat_height);
        self.chat_scroll = (self.chat_scroll + lines).min(max);
    }

    pub fn scroll_chat_back(&mut self, lines: usize) {
        self.chat_scroll = self.chat_scroll.saturating_sub(lines);
    }

    pub fn wheel(&mut self, up: bool) {
        match self.view {
            View::Instances => {
                if up {
                    self.instance_sel = self.instance_sel.saturating_sub(1);
                } else if self.instance_sel + 1 < self.instances.len() {
                    self.instance_sel += 1;
                }
            }
            View::Tasks => {
                if up {
                    self.task_sel = self.task_sel.saturating_sub(1);
                } else if self.task_sel + 1 < self.tasks.len() {
                    self.task_sel += 1;
                }
            }
            View::Topology => {
                if up {
                    self.topo_scroll = self.topo_scroll.saturating_sub(3);
                } else {
                    self.topo_scroll += 3;
                }
            }
            View::Chat => {
                if up {
                    self.scroll_chat(3);
                } else {
                    self.scroll_chat_back(3);
                }
            }
        }
    }

    pub fn status_line(&self) -> String {
        let instance = self.active_instance();
        let instance_part =
            instance.map(|i| format!("{} [{}]", i.id, i.phase)).unwrap_or_else(|| "（无实例）".to_string());
        let goal_part = self
            .goal
            .as_ref()
            .map(|g| {
                let mut text = format!("目标 {}", g.status);
                match g.limit_total {
                    Some(limit) => text.push_str(&format!(" · 用量 {}/{}", g.known_total, limit)),
                    None => text.push_str(&format!(" · 用量 {}", g.known_total)),
                }
                if g.unknown {
                    text.push_str("（含未知）");
                }
                text
            })
            .unwrap_or_else(|| "无目标".to_string());
        let approvals_part =
            if self.approvals.is_empty() { String::new() } else { format!(" · 待批准 {}", self.approvals.len()) };
        let link = if self.disconnected { " · 已断开，重连中…" } else { "" };
        let view = self.view.name();
        format!("会话 {} · 视图 {view} · {instance_part} · {goal_part}{approvals_part}{link}", self.session_id)
    }

    pub fn footer_hint(&self) -> String {
        if self.confirm.is_some() {
            return "确认终止该实例？y 确认 / n 取消".to_string();
        }
        match self.view {
            View::Chat => match self.focus {
                Focus::Composer => {
                    let approvals = if self.approvals.is_empty() {
                        String::new()
                    } else {
                        format!(" · F2 批准({})", self.approvals.len())
                    };
                    format!("Enter 发送 · Tab 切实例 · F3/F4/F5 面板{approvals} · Ctrl+C 退出")
                }
                Focus::Approvals => "a 批准本次 · d 拒绝 · ↑↓ 选择 · Esc 返回".to_string(),
            },
            View::Instances => "Enter 切换对话目标 · p 暂停 · r 恢复 · t 终止 · ↑↓ 选择 · Esc 返回".to_string(),
            View::Tasks => "c 取消任务 · ↑↓ 选择 · Esc 返回".to_string(),
            View::Topology => "↑↓ 滚动 · Esc 返回".to_string(),
        }
    }
}

/// One-line preview of a tool call: shell shows the command, everything
/// else the tool name with a bounded args preview.
fn tool_preview(name: &str, args_json: &str) -> String {
    let args: Json = serde_json::from_str(args_json).unwrap_or(Json::Null);
    let detail = args["command"]
        .as_str()
        .map(str::to_string)
        .or_else(|| args["text"].as_str().map(str::to_string))
        .unwrap_or_else(|| truncate(&args_json.replace('\n', " "), 120));
    format!("[{name}] {}", truncate(&detail, 160))
}

fn truncate(text: &str, cap: usize) -> String {
    let count = text.chars().count();
    if count <= cap {
        return text.to_string();
    }
    let kept: String = text.chars().take(cap).collect();
    format!("{kept}…（共 {count} 字符）")
}
