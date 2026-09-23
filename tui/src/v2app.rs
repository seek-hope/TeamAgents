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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatKind {
    User,
    Assistant,
    Tool,
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
pub struct GoalInfo {
    pub status: String,
    pub known_total: i64,
    pub unknown: bool,
    pub limit_total: Option<i64>,
}

/// Side effects for the main loop to execute through the daemon client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum V2Effect {
    SubmitInput { instance: String, envelope: String, text: String },
    Decide { approval_id: String, decision: &'static str },
    Quit,
}

/// What an applied event batch asks the main loop to refresh.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Refresh {
    pub history: bool,
    pub approvals: bool,
    pub checkpoint: bool,
}

/// Retained system-note cap: notes are UI signals layered over the
/// authoritative history; the cap keeps a long session bounded.
const NOTE_CAP: usize = 200;
/// Tool-result preview cap in the conversation (界面只取预览, §9).
const PREVIEW_CHARS: usize = 400;

pub struct V2App {
    pub session_id: String,
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
            instances: vec![],
            active: 0,
            entries: vec![],
            goal: None,
            approvals: vec![],
            approval_sel: 0,
            focus: Focus::Composer,
            composer: Composer::new(vec![]),
            disconnected: false,
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
        match self.focus {
            Focus::Approvals => self.approval_key(key),
            Focus::Composer => self.composer_key(key),
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
        if up {
            self.scroll_chat(3);
        } else {
            self.scroll_chat_back(3);
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
        format!("会话 {} · {instance_part} · {goal_part}{approvals_part}{link}", self.session_id)
    }

    pub fn footer_hint(&self) -> String {
        match self.focus {
            Focus::Composer => {
                let approvals = if self.approvals.is_empty() {
                    String::new()
                } else {
                    format!(" · F2 批准({})", self.approvals.len())
                };
                format!("Enter 发送 · Tab 切换实例 · PgUp/PgDn 滚动{approvals} · Ctrl+C 退出")
            }
            Focus::Approvals => "a 批准本次 · d 拒绝 · ↑↓ 选择 · Esc 返回".to_string(),
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
