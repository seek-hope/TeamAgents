//! Application state and key handling.
//! Pure logic: side effects leave as `Effect`s executed by the main loop, so
//! every key path is unit-testable without a terminal or worker process.

use serde_json::{json, Value as Json};
use std::time::{Duration, Instant};

use crate::i18n::{status_label_id, tr};
use crate::text::Composer;

pub const PANELS: [&str; 6] = ["team", "tasks", "shared", "approvals", "sessions", "log"];
const PANEL_TAB_LABELS: [&str; 6] = ["团队", "任务", "共享空间", "批准", "会话", "日志"];
pub const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Composer,
    Panel, // the active tab's table/list owns keys
}

/// One entry of the `/` command menu (Codex-style: type "/" to list them all).
#[derive(Clone, Copy, Debug)]
pub struct SlashCommand {
    pub name: &'static str,
    pub description: &'static str, // message id, translated on render
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand { name: "/help", description: "显示快捷键与斜杠命令说明" },
    SlashCommand { name: "/quit", description: "退出 TeamAgents" },
    SlashCommand { name: "/settings", description: "打开设置浮层（界面语言）" },
    SlashCommand { name: "/status", description: "查看 token 用量与上下文窗口" },
    SlashCommand { name: "/rewind", description: "回退对话到历史节点（/rewind 列出，/rewind <序号> 回退）" },
    SlashCommand { name: "/fork", description: "从当前对话分叉新会话（团队事实不复制）" },
    SlashCommand { name: "/model", description: "查看或切换成员模型与推理档位" },
];

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub severity: Severity,
    pub until: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub enum Effect {
    /// Submit an action; ok_msg/err_msg are written to chat from the receipt
    /// once submit returns. {error} fills the error.
    Submit { action: Json, ok_msg: Option<String>, err_msg: Option<String> },
    /// Cancel a task — message depends on the receipt result status.
    CancelTask(String),
    /// Acknowledge an OUTCOME_UNKNOWN turn (a run interrupted mid-command).
    AcknowledgeRun(String),
    /// Decide an approval — toast '批准决定 {v0}：{v1}' from the receipt.
    DecideApproval { approval_id: String, decision: String },
    UserMessage(String),
    /// /status: main loop calls the worker's "usage" method and feeds the
    /// result to App::show_usage (rendering stays in app.rs for testability).
    UsageStatus,
    /// /rewind: list targets (main loop calls "rewind_points") / move the tip
    RewindPoints,
    Rewind { node: Option<String> },
    /// /fork: branch the session with the conversation tree (D-26)
    Fork,
    /// /model with no args: worker "model" → App::show_models.
    ModelStatus,
    DiscoverModels { provider: String },
    /// /model <member> <model> [effort]; None/None clears the override.
    SetModel { agent_id: String, profile: Option<String>, model: Option<String>, effort: Option<String> },
    SwitchSession(String),
    NewSession,
    ArchiveSession(String),
    DeleteSession(String),
    Quit,
    Bell,
}

#[derive(Clone, Debug)]
pub struct RunInfo {
    pub run_id: String,
    pub agent_id: String,
    pub status: String,
    pub task_id: Option<String>,
    pub created_at: f64,
}

#[derive(Clone, Debug)]
pub enum OpResult {
    Switched { session_id: String, catalog: Json, config_path: String },
    Failed { op: &'static str, target: String, error: String },
    Archived { session_id: String, target: String, was_current: bool },
    Deleted { session_id: String, was_current: bool },
}

/// (text, style name from activity_status) — None = plain foreground.
pub type Cell = (String, Option<&'static str>);

/// Real session id behind a sessions row key: duplicate ids get a cursor-only
/// `#archived`/`#active` suffix (sessions_rows) that is never a session id.
fn session_row_id(key: &str) -> &str {
    key.strip_suffix("#archived").or_else(|| key.strip_suffix("#active")).unwrap_or(key)
}

impl App {
    /// Is the resolved sessions row archived? Rows are index-aligned with
    /// self.sessions (sessions_rows pushes one row per entry, in order).
    fn session_row_archived(&self, sel: Option<usize>) -> bool {
        sel.and_then(|i| self.sessions.get(i))
            .and_then(|s| s.get("archived"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
}

pub fn cell(text: String) -> Cell {
    (text, None)
}

fn jstr(v: &Json, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// last 8 chars (text[-8:] style)
pub fn tail8(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars.iter().skip(chars.len().saturating_sub(8)).collect()
}

fn head_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    chars.iter().take(n).collect()
}

/// time.strftime on a unix float, local time.
pub fn fmt_ts(ts: f64, with_seconds: bool) -> String {
    let Ok(utc) = time::OffsetDateTime::from_unix_timestamp(ts as i64) else {
        return "-".into();
    };
    let local = time::UtcOffset::current_local_offset()
        .map(|off| utc.to_offset(off))
        .unwrap_or(utc);
    let fmt = if with_seconds { "[month]-[day] [hour]:[minute]:[second]" } else { "[month]-[day] [hour]:[minute]" };
    let format = time::format_description::parse_borrowed::<2>(fmt).expect("static fmt");
    local.format(&format).unwrap_or_else(|_| "-".into())
}

/// Returns (label, color-name).
pub fn activity_status(lang: &str, animations: bool, frame: usize, status: &str, run: Option<&RunInfo>) -> (String, &'static str) {
    let mut status = status.to_string();
    if let Some(r) = run {
        if matches!(status.as_str(), "BUSY" | "WAITING" | "RUNNING" | "PENDING" | "IDLE") {
            status = r.status.clone();
        }
    }
    let busy = matches!(status.as_str(), "BUSY" | "RUNNING");
    let mut icon = if busy {
        if animations { SPINNER[frame % 10].to_string() } else { "●".into() }
    } else {
        "○".into()
    };
    let mut style = if busy { "accent" } else { "notice" };
    match status.as_str() {
        "FAILED" | "OUTCOME_UNKNOWN" => {
            icon = "!".into();
            style = "error";
        }
        "BLOCKED" | "WAITING_APPROVAL" | "DRAINING" => {
            icon = "!".into();
            style = "warning";
        }
        "SUCCEEDED" | "COMPLETED" => {
            icon = "✓".into();
            style = "success";
        }
        "WAITING" | "WAITING_TASK" | "QUEUED" | "PENDING" => {
            icon = "◷".into();
        }
        _ => {}
    }
    let mut label = format!("{} {}", icon, tr(lang, status_label_id(&status), &[]));
    if let Some(r) = run {
        let elapsed = now_ts() - r.created_at;
        let elapsed = elapsed.max(0.0) as u64;
        label += &format!(" {:02}:{:02}", elapsed / 60, elapsed % 60);
    }
    (label, style)
}

fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub struct App {
    pub lang: &'static str,
    pub animations: bool,
    pub session_id: String,
    pub catalog: Json,
    pub user_config_path: String,
    pub state: Option<Json>,
    pub cursor: i64,
    pub chat: Vec<(String, String)>,
    pub delta_buffers: std::collections::HashMap<String, String>,
    pub stream_text: String,
    stream_dirty: bool,
    last_stream_render: Instant,
    pub activity_frame: usize,
    pub latest_activity: (String, Vec<(String, String)>),
    pub activity_runs: Vec<RunInfo>,
    pub panel: usize,
    pub focus: Focus,
    /// per-panel saved cursor: panel -> (row key, row index)
    pub table_cursors: std::collections::HashMap<&'static str, (Option<String>, usize)>,
    pub composer: Composer,
    pub log_cursor: i64,
    pub log_member: Option<String>,
    pub log_lines: Vec<String>,
    /// Last tool each member ran, for the team panel's activity column.
    tool_activity: std::collections::HashMap<String, ToolActivity>,
    /// Per-member working plans (the upper status strip shows one).
    pub plans: std::collections::HashMap<String, Vec<Json>>,
    /// Per-member review material: the diff lines of its recent edits.
    reviews: std::collections::HashMap<String, Vec<String>>,
    /// Per-member context usage: (context window, last prompt tokens).
    usage: std::collections::HashMap<String, (Option<u64>, u64)>,
    /// member id -> run id whose outcome is still unknown
    pub unknown_runs: std::collections::HashMap<String, String>,
    /// `v` review overlay (Esc closes; Ctrl+U/D scroll)
    pub review_open: bool,
    pub review_agent: String,
    /// Title of whatever the shared overlay is showing (diff review or plan)
    pub overlay_title: String,
    pub review_lines: Vec<String>,
    pub review_scroll: usize,
    /// chat lines scrolled up from the bottom (0 = pinned to the newest entry)
    pub chat_scroll: usize,
    /// log panel lines scrolled up from the bottom
    pub log_scroll: usize,
    /// `/settings` overlay (Esc closes; ↑↓ move, Enter toggles)
    pub settings_open: bool,
    pub model_picker: Option<crate::model_picker::ModelPicker>,
    model_labels: std::collections::HashMap<String, String>,
    pub model_generation: u64,
    /// node ids of the last shown /rewind list (newest first)
    rewind_list: Vec<String>,
    /// last mouse position, for hover feedback (row, column)
    pub pointer: Option<(u16, u16)>,
    /// the `/` menu: highlighted entry, and the query it was dismissed for
    pub slash_index: usize,
    pub slash_dismissed_for: Option<String>,
    /// tab strip row recorded by the renderer (hover + hit-test)
    pub tab_row: u16,
    pub toasts: Vec<Toast>,
    pub sessions: Vec<Json>,
    pub shared: Vec<Json>,
    pub pending_delete: Option<String>,
    pub lang_open: bool,
    pub lang_choice: usize, // 0 = en, 1 = zh-CN
    pub should_quit: bool,
    /// state poll failed repeatedly: status chip until a poll succeeds again
    pub disconnected: bool,
}

impl App {
    pub fn new(session_id: &str, catalog: Json, user_config_path: String, lang: &'static str, animations: bool, history: Vec<String>) -> App {
        App {
            lang,
            animations,
            session_id: session_id.into(),
            catalog,
            user_config_path,
            state: None,
            cursor: 0,
            chat: vec![],
            delta_buffers: Default::default(),
            stream_text: String::new(),
            stream_dirty: false,
            last_stream_render: Instant::now(),
            activity_frame: 0,
            latest_activity: ("等待输入".into(), vec![]),
            activity_runs: vec![],
            panel: 0,
            focus: Focus::Composer,
            table_cursors: Default::default(),
            composer: Composer::new(history),
            log_cursor: 0,
            log_member: None,
            log_lines: vec![],
            tool_activity: std::collections::HashMap::new(),
            plans: std::collections::HashMap::new(),
            reviews: std::collections::HashMap::new(),
            usage: std::collections::HashMap::new(),
            unknown_runs: std::collections::HashMap::new(),
            review_open: false,
            review_agent: String::new(),
            overlay_title: String::new(),
            review_lines: vec![],
            review_scroll: 0,
            chat_scroll: 0,
            log_scroll: 0,
            settings_open: false,
            model_picker: None,
            model_labels: Default::default(),
            model_generation: 0,
            rewind_list: vec![],
            pointer: None,
            slash_index: 0,
            slash_dismissed_for: None,
            tab_row: 0,
            toasts: vec![],
            sessions: vec![],
            shared: vec![],
            pending_delete: None,
            lang_open: false,
            lang_choice: if lang == "zh-CN" { 1 } else { 0 },
            should_quit: false,
            disconnected: false,
        }
    }

    pub fn t(&self, msg: &str, args: &[(&str, &str)]) -> String {
        tr(self.lang, msg, args)
    }

    pub fn notify(&mut self, text: String, severity: Severity, timeout_s: u64) {
        self.toasts.push(Toast { text, severity, until: Instant::now() + Duration::from_secs(timeout_s) });
    }

    fn write_chat(&mut self, who: &str, text: &str) {
        self.chat.push((who.to_string(), text.to_string()));
    }

    // ------------------------------------------------------------- state in

    pub fn spec(&self) -> &Json {
        self.state.as_ref().and_then(|s| s.get("spec")).unwrap_or(&Json::Null)
    }

    pub fn session(&self) -> &Json {
        self.state.as_ref().and_then(|s| s.get("session")).unwrap_or(&Json::Null)
    }

    pub fn leader_id(&self) -> String {
        self.state
            .as_ref()
            .and_then(|s| s.get("leader_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("leader")
            .to_string()
    }

    /// New committed state snapshot + event drain.
    pub fn apply_state(&mut self, st: &Json) -> Vec<Effect> {
        let mut effects = vec![];
        let events = st.get("events").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for ev in &events {
            let seq = ev.get("sequence").and_then(|v| v.as_i64()).unwrap_or(0);
            if seq <= self.cursor {
                continue;
            }
            self.cursor = self.cursor.max(seq);
            effects.extend(self.apply_event(ev, st));
        }
        // per-member context usage (the worker merges it into the snapshot)
        self.usage.clear();
        if let Some(agents) = st.get("usage").and_then(Json::as_array) {
            for agent in agents {
                let id = jstr(agent, "agent_id");
                if id.is_empty() {
                    continue;
                }
                let window = agent.get("context_window").and_then(Json::as_u64);
                let last = agent.get("usage").and_then(|u| u.get("last_prompt_tokens")).and_then(Json::as_u64).unwrap_or(0);
                self.usage.insert(id, (window, last));
            }
        }
        // runs whose outcome nobody has accepted yet: they block goal completion
        self.unknown_runs.clear();
        if let Some(runs) = st.get("runs").and_then(|v| v.as_array()) {
            for run in runs {
                if jstr(run, "status") == "OUTCOME_UNKNOWN" {
                    self.unknown_runs.insert(jstr(run, "agent_id"), jstr(run, "run_id"));
                }
            }
        }
        // active runs for the activity line / status cells
        self.activity_runs = st
            .get("runs")
            .and_then(|v| v.as_array())
            .map(|runs| {
                runs.iter()
                    .filter(|r| {
                        matches!(jstr(r, "status").as_str(), "QUEUED" | "RUNNING" | "WAITING_TASK" | "WAITING_APPROVAL")
                    })
                    .map(|r| RunInfo {
                        run_id: jstr(r, "run_id"),
                        agent_id: jstr(r, "agent_id"),
                        status: jstr(r, "status"),
                        task_id: r.get("task_id").and_then(|v| v.as_str()).map(str::to_string),
                        created_at: r.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.state = Some(st.clone());
        // the state snapshot carries every member's plan (worker merges them in)
        if let Some(plans) = st.get("plans").and_then(Json::as_array) {
            for plan in plans {
                let Some(agent) = plan.get("agent_id").and_then(|v| v.as_str()) else { continue };
                let items = plan.get("items").and_then(Json::as_array).cloned().unwrap_or_default();
                self.plans.insert(agent.to_string(), items);
            }
        }
        // prune expired toasts opportunistically
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        effects
    }

    fn apply_event(&mut self, ev: &Json, st: &Json) -> Vec<Effect> {
        let mut effects = vec![];
        let kind = jstr(ev, "kind");
        let payload = ev.get("payload").cloned().unwrap_or(Json::Null);
        let p = &payload;
        let actor = jstr(ev, "actor_id");
        match kind.as_str() {
            "run_started" => {
                let agent = p.get("agent_id").and_then(|v| v.as_str()).unwrap_or("Leader").to_string();
                self.latest_activity = ("{agent} 开始处理".into(), vec![("agent".to_string(), agent)]);
            }
            "task_completed" => self.latest_activity = ("任务已完成".into(), vec![]),
            "approval_requested" => self.latest_activity = ("需要用户批准".into(), vec![]),
            "run_failed" => self.latest_activity = ("失败".into(), vec![]),
            "run_cancelled" => self.latest_activity = ("已取消".into(), vec![]),
            "goal_done" => self.latest_activity = ("已完成".into(), vec![]),
            _ => {}
        }
        match kind.as_str() {
            "user_message" => self.write_chat("user", &jstr(p, "text")),
            "leader_reply" => {
                let run_id = jstr(p, "run_id");
                self.delta_buffers.remove(&run_id);
                self.stream_text.clear();
                self.stream_dirty = false;
                self.write_chat("Leader", &jstr(p, "text"));
            }
            "message" => {
                self.write_chat(&format!("{}→{}", actor, jstr(p, "target")), &jstr(p, "text"));
            }
            "task_created" => {
                self.write_chat("system", &self.t("任务 {v0} → {v1}：{v2}", &[
                    ("v0", &tail8(&jstr(p, "task_id"))),
                    ("v1", &jstr(p, "assignee")),
                    ("v2", &jstr(p, "description")),
                ]));
            }
            "task_completed" | "task_failed" | "task_blocked" | "task_cancelled" => {
                let detail = ["summary", "reason", "error"]
                    .iter()
                    .find_map(|k| p.get(k).and_then(|v| v.as_str()))
                    .unwrap_or("");
                self.write_chat("system", &format!("[{kind}] {} {detail}", tail8(&jstr(p, "task_id"))));
            }
            "approval_requested" => {
                let line = self.t("需要批准：{v0}（按 Ctrl+G 处理）", &[("v0", &approval_line(p))]);
                self.write_chat("system", &line);
                // judge against the snapshot the event arrived with: self.state is
                // still the previous one here (it is swapped in after the events)
                let still_pending = st
                    .get("pending_approvals")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter().any(|x| {
                            jstr(x, "approval_id") == jstr(p, "approval_id")
                                && jstr(x, "status") == "PENDING"
                        })
                    })
                    .unwrap_or(true);
                if still_pending {
                    self.notify(line, Severity::Warning, 10);
                    effects.push(Effect::Bell);
                }
            }
            "approval_decided" => {
                self.write_chat("system", &self.t("批准 {v0} → {v1}", &[
                    ("v0", &jstr(p, "approval_id")),
                    ("v1", &jstr(p, "status")),
                ]));
            }
            "run_failed" => {
                self.write_chat("system", &self.t("✗ 成员 {v0} 的回合失败：{v1}", &[
                    ("v0", &jstr(p, "agent_id")),
                    ("v1", &jstr(p, "error")),
                ]));
            }
            "run_cancelled" => {
                self.write_chat("system", &self.t("回合已停止：{v0} ({v1})", &[
                    ("v0", &jstr(p, "agent_id")),
                    ("v1", &jstr(p, "status")),
                ]));
            }
            "run_waiting" => {
                let waiting = p.get("waiting_on").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                if waiting > 0 {
                    let n = waiting.to_string();
                    self.write_chat("system", &self.t("{v0} 正在等待 {v1} 个任务完成", &[
                        ("v0", &jstr(p, "agent_id")),
                        ("v1", &n),
                    ]));
                }
            }
            "member_status" => {
                self.write_chat("system", &self.t("成员状态：{v0} {v1} {v2}", &[
                    ("v0", &jstr(p, "agent_id")),
                    ("v1", &jstr(p, "status")),
                    ("v2", &jstr(p, "error")),
                ]));
            }
            "session_status" => {
                self.write_chat("system", &self.t("会话状态：{v0}", &[("v0", &compact_json(p))]));
            }
            "run_progress" => {
                if p.get("final").and_then(|v| v.as_bool()).unwrap_or(false) {
                    let who = self.t("{v0}（完成）", &[("v0", &jstr(p, "agent_id"))]);
                    self.write_chat(&who, &jstr(p, "text"));
                }
            }
            "limit_reached" => {
                self.write_chat("system", &self.t("[达到上限] {v0}", &[("v0", &compact_json(p))]));
            }
            "goal_done" => {
                self.write_chat("system", &self.t("目标完成：{v0}", &[("v0", &jstr(p, "summary"))]));
            }
            _ => {}
        }
        effects
    }

    /// Stream deltas from the worker.
    pub fn on_delta(&mut self, run_id: &str, agent_id: &str, text: &str) {
        self.latest_activity = ("{agent} 正在回复".into(), vec![("agent".to_string(), agent_id.to_string())]);
        if agent_id != self.leader_id() {
            return;
        }
        let buf = self.delta_buffers.entry(run_id.to_string()).or_default();
        buf.push_str(text);
        if buf.len() > 32000 {
            // cut is a byte offset; back off to a char boundary or CJK deltas panic
            let mut cut = buf.len() - 32000;
            while !buf.is_char_boundary(cut) {
                cut += 1;
            }
            buf.drain(..cut);
        }
        self.stream_dirty = true;
    }

    pub fn flush_deltas(&mut self) {
        let terminal: Vec<String> = self
            .delta_buffers
            .keys()
            .filter(|rid| {
                self.state
                    .as_ref()
                    .and_then(|s| s.get("runs"))
                    .and_then(|v| v.as_array())
                    .and_then(|runs| runs.iter().find(|r| jstr(r, "run_id") == **rid))
                    .map(|r| matches!(jstr(r, "status").as_str(), "COMPLETED" | "FAILED" | "CANCELLED" | "OUTCOME_UNKNOWN"))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        for rid in terminal {
            self.delta_buffers.remove(&rid);
            self.stream_dirty = true;
        }
        if self.stream_dirty {
            // throttle redraws of the preview to 5/s; clear immediately
            if self.delta_buffers.is_empty() || self.last_stream_render.elapsed() >= Duration::from_millis(200) {
                self.stream_text = self
                    .delta_buffers
                    .values()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n\n");
                self.last_stream_render = Instant::now();
                self.stream_dirty = false;
            }
        }
    }

    // ------------------------------------------------------------ activity

    /// Activity line: one chip per visible run (or a single idle chip) plus the
    /// latest-activity text. Chips carry (text, colour-name).
    pub fn activity_chips(&mut self) -> (Vec<(String, &'static str)>, String) {
        if self.animations
            && self.activity_runs.iter().any(|r| r.status == "RUNNING")
        {
            self.activity_frame = (self.activity_frame + 1) % 10;
        } else {
            self.activity_frame = 0;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let elapsed = |created: f64| -> String {
            let total = (now - created).max(0.0) as i64;
            format!("{:02}:{:02}", total / 60, total % 60)
        };
        let mut chips: Vec<(String, &'static str)> = vec![];
        for run in self.activity_runs.iter() {
            let (icon, style) = activity_status(self.lang, self.animations, self.activity_frame, &run.status, Some(run));
            let icon_char = icon.chars().next().unwrap_or('○');
            chips.push((format!("{icon_char} {} {}", run.agent_id, elapsed(run.created_at)), style));
        }
        if chips.is_empty() {
            chips.push((format!("○ {}", self.t("就绪", &[])), "notice"));
        }
        if self.activity_runs.iter().any(|r| r.status == "WAITING_APPROVAL") {
            chips.push((format!("! {}", self.t("等待批准", &[])), "warning"));
        }
        let (msg, args) = self.latest_activity.clone();
        let arg_refs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        (chips, tr(self.lang, &msg, &arg_refs))
    }

    /// Short form for the composer's border title (the hint row explains keys).
    pub fn composer_title(&self) -> String {
        let leader = self.leader_id();
        let run = self.activity_runs.iter().find(|r| r.agent_id == leader);
        let (state, _) = activity_status(self.lang, self.animations, self.activity_frame, "IDLE", run);
        let profile = self
            .spec()
            .get("agents")
            .and_then(|v| v.as_array())
            .and_then(|a| a.iter().find(|x| jstr(x, "id") == leader))
            .map(|a| self.model_label(a))
            .unwrap_or_default();
        format!("{state} · Leader / {profile}")
    }

    // ------------------------------------------------------------ status bar

    pub fn status_bar(&self) -> String {
        let session = self.session();
        let mode = jstr(session, "permissions_mode");
        let mode_label = match mode.as_str() {
            "approved_scope" => "预授权",
            "full_auto" => "全自动",
            _ => mode.as_str(),
        };
        let mut parts = vec![format!("TeamAgents · {}", self.session_id), self.t(mode_label, &[])];
        let state = jstr(session, "status");
        if !matches!(state.as_str(), "ACTIVE" | "IDLE" | "") {
            let label = match state.as_str() {
                "PAUSED" => "已暂停",
                "CLOSED" => "已关闭",
                other => other,
            };
            parts.push(self.t(label, &[]));
        }
        let approvals = self
            .state
            .as_ref()
            .and_then(|s| s.get("pending_approvals"))
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if approvals > 0 {
            parts.push(self.t("待批准 {count}", &[("count", &approvals.to_string())]));
        }
        let open_tasks = self
            .state
            .as_ref()
            .and_then(|s| s.get("tasks"))
            .and_then(|v| v.as_array())
            .map(|t| {
                t.iter()
                    .filter(|x| matches!(jstr(x, "status").as_str(), "PENDING" | "RUNNING" | "BLOCKED"))
                    .count()
            })
            .unwrap_or(0);
        if open_tasks > 0 {
            parts.push(self.t("未完成任务 {count}", &[("count", &open_tasks.to_string())]));
        }
        parts.join(" | ")
    }

    // ---------------------------------------------------------- startup checks

    pub fn startup_warnings(&self) -> Vec<String> {
        let mut out = vec![];
        let agents = self.spec().get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let models = self.catalog.get("models").cloned().unwrap_or(Json::Null);
        for agent in agents {
            let profile = jstr(&agent, "model_profile");
            match models.get(&profile) {
                None => out.push(self.t(
                    "⚠ 成员 {v0} 的模型 profile {v1!r} 未配置：请创建 {v2}（可直接复制仓库里的 examples/config.toml），然后重开会话。在此之前发出的消息都会失败。",
                    &[("v0", &jstr(&agent, "id")), ("v1", &profile), ("v2", &self.user_config_path)],
                )),
                Some(m) => {
                    let env = jstr(m, "api_key_env");
                    if !env.is_empty() && std::env::var(&env).unwrap_or_default().is_empty() {
                        out.push(self.t(
                            "⚠ 模型 profile {v0!r} 需要环境变量 {v1}，当前未设置：请 export 后重开会话。",
                            &[("v0", &profile), ("v1", &env)],
                        ));
                    }
                }
            }
        }
        out
    }

    pub fn push_startup_warnings(&mut self) {
        for w in self.startup_warnings() {
            self.write_chat("system", &w);
        }
    }

    // ------------------------------------------------------------ panel data

    fn agent_status_map(&self) -> std::collections::HashMap<String, String> {
        self.state
            .as_ref()
            .and_then(|s| s.get("agents"))
            .and_then(|v| v.as_array())
            .map(|a| a.iter().map(|x| (jstr(x, "id"), jstr(x, "status"))).collect())
            .unwrap_or_default()
    }

    /// TeamPanel::refresh_from. Rows carry their row key (agent id).
    pub fn team_rows(&self) -> Vec<(String, Vec<Cell>)> {
        let spec = self.spec();
        let agents = spec.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let channels = spec.get("channels").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let observers = spec.get("observers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let statuses = self.agent_status_map();
        let ids: Vec<String> = agents.iter().map(|a| jstr(a, "id")).collect();
        let leader = jstr(spec, "leader_id");
        let can = |src: &str, tgt: &str, task: bool| {
            if task && src == leader {
                return true; // the leader delegates to anyone
            }
            channels.iter().any(|c| {
                jstr(c, "source") == src && {
                    let mode = jstr(c, "mode");
                    let targeted = c.get("targets").and_then(|v| v.as_array())
                        .map(|t| t.iter().any(|x| x.as_str() == Some(tgt))).unwrap_or(false);
                    if task {
                        mode == "task" && targeted
                    } else {
                        mode == "broadcast" || (mode == "message" && targeted)
                    }
                }
            })
        };
        let mut rows = vec![];
        for agent in &agents {
            let id = jstr(agent, "id");
            let mut can_send: Vec<&str> = ids.iter().filter(|t| can(&id, t, false)).map(|s| s.as_str()).collect();
            let mut can_delegate: Vec<&str> = ids.iter().filter(|t| can(&id, t, true)).map(|s| s.as_str()).collect();
            can_send.sort();
            can_delegate.sort();
            let observed_by: Vec<&str> = observers
                .iter()
                .filter(|o| {
                    o.get("subjects").and_then(|v| v.as_array())
                        .map(|s| s.iter().any(|x| x.as_str() == Some(id.as_str()))).unwrap_or(false)
                })
                .map(|o| o.get("agent_id").and_then(|v| v.as_str()).unwrap_or(""))
                .collect();
            let mut reach = vec![];
            if !can_send.is_empty() {
                reach.push(format!("{}{}", self.t("消息→", &[]), can_send.join(",")));
            }
            if !can_delegate.is_empty() {
                reach.push(format!("{}{}", self.t("任务→", &[]), can_delegate.join(",")));
            }
            if !observed_by.is_empty() {
                reach.push(format!("{}{}", self.t("被观察:", &[]), observed_by.join(",")));
            }
            let status = statuses.get(&id).cloned().unwrap_or_else(|| "IDLE".into());
            let run = self.activity_runs.iter().find(|r| r.agent_id == id);
            let (status_label, style) = match self.unknown_runs.get(&id) {
                // a turn whose outcome nobody accepted: it blocks signal_done
                // until it is acknowledged
                Some(_) => (self.t("结果不明（c 结清）", &[]), "warning"),
                None => activity_status(self.lang, self.animations, self.activity_frame, &status, run),
            };
            let activity = self
                .tool_activity
                .get(&id)
                .map(|last| {
                    format!(
                        "{}{} {}",
                        if last.ok { "" } else { "✗ " },
                        last.tool,
                        elapsed_short(last.at.elapsed())
                    )
                })
                .unwrap_or_else(|| "-".into());
            rows.push((id.clone(), vec![
                cell(id.clone()),
                cell(jstr(agent, "role")),
                cell(jstr(agent, "runtime_kind")),
                self.model_cell(agent, &id),
                (status_label, Some(style)),
                cell(agent.get("workspace_policy").and_then(|v| v.as_str()).unwrap_or("shared").to_string()),
                cell(if reach.is_empty() { "-".into() } else { reach.join(" ") }),
                cell(activity),
            ]));
        }
        rows
    }

    /// Model label plus context usage when the window is known: the team panel is
    /// where a user notices a member approaching compaction.
    fn model_cell(&self, agent: &Json, agent_id: &str) -> Cell {
        let label = self.model_label(agent);
        let Some((Some(window), last)) = self.usage.get(agent_id).copied() else {
            return cell(label);
        };
        if window == 0 || last == 0 {
            return cell(label);
        }
        let percent = ((last as f64 / window as f64) * 100.0).round().min(100.0) as u64;
        let style = if percent >= 80 { "warning" } else { "notice" };
        (format!("{label} {percent}%"), Some(style))
    }

    /// TasksPanel::refresh_from — newest first, parent-indented.
    pub fn tasks_rows(&self) -> Vec<(String, Vec<Cell>)> {
        let tasks = self.state.as_ref().and_then(|s| s.get("tasks")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut sorted = tasks.clone();
        sorted.sort_by(|a, b| {
            let ka = (a.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0), jstr(a, "task_id"));
            let kb = (b.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0), jstr(b, "task_id"));
            kb.partial_cmp(&ka).unwrap()
        });
        let level = |task: &Json| -> usize {
            let mut seen = std::collections::HashSet::new();
            let mut cur = task.clone();
            let mut depth = 0;
            loop {
                let Some(parent) = cur.get("parent_task_id").and_then(|v| v.as_str()) else { break };
                if parent.is_empty() || !seen.insert(jstr(&cur, "task_id")) {
                    break;
                }
                match tasks.iter().find(|t| jstr(t, "task_id") == parent) {
                    Some(p) => {
                        cur = p.clone();
                        depth += 1;
                    }
                    None => break,
                }
            }
            depth
        };
        sorted
            .iter()
            .map(|t| {
                let indent = "  ".repeat(level(t));
                // _animate_activity live-updates the status cell from the active run
                let run = self.activity_runs.iter().find(|r| r.task_id.as_deref() == Some(jstr(t, "task_id").as_str()));
                let status = run.map(|r| r.status.clone()).unwrap_or_else(|| jstr(t, "status"));
                let (status_label, style) = activity_status(self.lang, self.animations, self.activity_frame, &status, run);
                let deps = t.get("dependencies").and_then(|v| v.as_array()).map(|d| {
                    d.iter().filter_map(|x| x.as_str()).map(tail8).collect::<Vec<_>>().join(",")
                }).unwrap_or_default();
                let results = t.get("result_refs").and_then(|v| v.as_array()).map(|r| {
                    r.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")
                }).unwrap_or_default();
                (jstr(t, "task_id"), vec![
                    cell(format!("{indent}{}", tail8(&jstr(t, "task_id")))),
                    cell(jstr(t, "requester")),
                    cell(jstr(t, "assignee")),
                    (status_label, Some(style)),
                    cell(head_chars(&jstr(t, "description"), 60)),
                    cell(if deps.is_empty() { "-".into() } else { deps }),
                    cell(if results.is_empty() { "-".into() } else { head_chars(&results, 60) }),
                    cell(fmt_ts(t.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0), true)),
                ])
            })
            .collect()
    }

    /// ApprovalsPanel::refresh_from.
    pub fn approvals_rows(&self) -> Vec<(String, Vec<Cell>)> {
        self.state
            .as_ref()
            .and_then(|s| s.get("pending_approvals"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| {
                let scope = a.get("requested_scope").cloned().unwrap_or(Json::Null);
                let tool = ["tool", "kind"].iter().find_map(|k| scope.get(k).and_then(|v| v.as_str())).unwrap_or("?");
                let args = scope.get("args").or_else(|| scope.get("request")).cloned().unwrap_or(Json::Null);
                let args = py_repr(&args);
                (jstr(a, "approval_id"), vec![
                    cell(jstr(a, "agent_id")),
                    cell(tool.to_string()),
                    cell(head_chars(&args, 60)),
                    cell(head_chars(&jstr(&scope, "reason"), 40)),
                ])
            })
            .collect()
    }

    /// SessionsPanel::refresh_from (list from the worker).
    pub fn sessions_rows(&self) -> Vec<(String, Vec<Cell>)> {
        let mut used = std::collections::HashSet::new();
        let mut rows = vec![];
        for info in &self.sessions {
            let sid = jstr(info, "sessionId");
            let mut marks = vec![];
            if sid == self.session_id {
                marks.push(self.t("当前", &[]));
            }
            if info.get("locked").and_then(|v| v.as_bool()).unwrap_or(false) && sid != self.session_id {
                marks.push(self.t("运行中", &[]));
            }
            if info.get("archived").and_then(|v| v.as_bool()).unwrap_or(false) {
                marks.push(self.t("已归档", &[]));
            }
            if info.get("error").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false) {
                marks.push(self.t("读取异常", &[]));
            }
            let updated = info.get("updatedAt").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let size = info.get("sizeMb").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let mut key = sid.clone();
            if used.contains(&key) {
                key = format!("{sid}#{}", if info.get("archived").and_then(|v| v.as_bool()).unwrap_or(false) { "archived" } else { "active" });
            }
            used.insert(key.clone());
            rows.push((key, vec![
                cell(sid),
                cell(jstr(info, "status")),
                cell(jstr(info, "goalState")),
                cell(info.get("events").map(|v| v.to_string()).unwrap_or_else(|| "0".into())),
                cell(format!("{size:.1}MB")),
                cell(if updated > 0.0 { fmt_ts(updated, false) } else { "-".into() }),
                cell(if marks.is_empty() { "-".into() } else { marks.join(" ") }),
            ]));
        }
        rows
    }

    /// SharedPanel::refresh_from (entries fetched separately). Row key is
    /// `space_id:sequence` so the panel can scroll like the other tables.
    pub fn shared_rows(&self) -> Vec<(String, Vec<Cell>)> {
        let spaces = self.spec().get("shared_spaces").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let mut rows = vec![];
        for space in &spaces {
            let sid = jstr(space, "id");
            let mut entries: Vec<&Json> = self.shared.iter().filter(|e| jstr(e, "space_id") == sid).collect();
            entries.sort_by_key(|e| e.get("sequence").and_then(|v| v.as_i64()).unwrap_or(0));
            for entry in entries {
                let content = jstr(entry, "content");
                let reference = jstr(entry, "ref");
                let sequence = entry.get("sequence").and_then(|v| v.as_i64()).unwrap_or(0);
                rows.push((format!("{sid}:{sequence}"), vec![
                    cell(sid.clone()),
                    cell(jstr(entry, "author")),
                    cell(jstr(entry, "kind")),
                    cell(if !content.is_empty() { head_chars(&content, 80) } else if !reference.is_empty() { reference } else { "-".into() }),
                    cell(entry.get("sequence").map(|v| v.to_string()).unwrap_or_default()),
                ]));
            }
        }
        rows
    }

    /// Public wrapper for the mouse hit-test on the tab strip.
    pub fn tab_badge(&self, index: usize) -> Option<String> {
        crate::ui::tab_badge_for(self, index)
    }

    /// Clicking a table row selects it: the click lands on a *visible* row, so
    /// the window offset the renderer used has to be added back.
    pub fn select_row_visible(&mut self, visible_row: usize, view: usize) {
        let panel = PANELS[self.panel];
        let rows = self.panel_row_keys(panel);
        // same selection resolution as the renderer (ui.rs render_panel): the
        // saved key wins, the stale index is only a fallback — rows may have
        // been reordered since the cursor was stored
        let saved = self.table_cursors.get(panel).cloned().unwrap_or((None, 0));
        let sel = saved
            .0
            .and_then(|k| rows.iter().position(|rk| *rk == k))
            .unwrap_or_else(|| saved.1.min(rows.len().saturating_sub(1)));
        let start = crate::ui::table_start(rows.len(), sel, view);
        self.select_row(start + visible_row);
    }

    /// Clicking a table row selects it (the renderer keeps the row key).
    pub fn select_row(&mut self, index: usize) {
        let panel = PANELS[self.panel];
        let key = self.panel_row_keys(panel).get(index).cloned();
        if let Some(key) = key {
            self.table_cursors.insert(panel, (Some(key), index));
        }
    }

    /// Move the active panel's selection by `delta`: the wheel does this over the
    /// table rect, ↑/↓ do it while the panel has the focus. It never touches the
    /// composer (the wheel used to fall through to the history recall).
    pub fn move_table_selection(&mut self, delta: i64) {
        let panel = PANELS[self.panel];
        let rows = self.panel_row_keys(panel);
        if rows.is_empty() {
            return;
        }
        let (_, idx) = self.table_cursors.get(panel).cloned().unwrap_or((None, 0));
        let idx = if delta < 0 {
            idx.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            (idx + delta as usize).min(rows.len() - 1)
        };
        let key = rows.get(idx).cloned();
        self.table_cursors.insert(panel, (key.clone(), idx));
        if panel == "team" {
            // on_data_table_row_highlighted: highlighting a member filters the log
            if let Some(member) = key {
                self.log_member = Some(member);
            }
        }
    }

    /// "(member X · Enter 取消)" suffix for the log title, empty without a filter.
    pub fn log_filter_suffix(&self) -> String {
        match &self.log_member {
            Some(m) => self.t("（成员 {v0} · Enter 取消）", &[("v0", m)]),
            None => String::new(),
        }
    }

    /// LogPanel::refresh_from — append event lines after the log cursor.
    pub fn append_log(&mut self, events: &[Json]) {
        if let Some(m) = &self.log_member {
            let member = m.clone();
            for ev in events {
                self.log_cursor = self.log_cursor.max(ev.get("sequence").and_then(|v| v.as_i64()).unwrap_or(0));
                let payload = py_json_dumps(ev.get("payload").unwrap_or(&Json::Null));
                if jstr(ev, "actor_id") != member && !payload.contains(&member) {
                    continue;
                }
                self.push_log_line(format_log_line(ev, &payload));
            }
        } else {
            for ev in events {
                self.log_cursor = self.log_cursor.max(ev.get("sequence").and_then(|v| v.as_i64()).unwrap_or(0));
                let payload = py_json_dumps(ev.get("payload").unwrap_or(&Json::Null));
                self.push_log_line(format_log_line(ev, &payload));
            }
        }
    }

    /// Tool activity from the engine: log lines then show what each member really
    /// ran (name + arguments), not just the core event stream.
    pub fn on_plan(&mut self, agent_id: &str, items: &Json) {
        let items = items.as_array().cloned().unwrap_or_default();
        self.plans.insert(agent_id.to_string(), items);
    }

    /// Which member's plan the status strip shows: the panel selection, else the
    /// member with a running turn, else the leader.
    fn plan_agent(&self) -> Option<String> {
        if self.focus == Focus::Panel {
            if let Some((Some(key), _)) = self.table_cursors.get(PANELS[self.panel]).cloned() {
                if self.plans.contains_key(&key) {
                    return Some(key);
                }
            }
        }
        if let Some(run) = self.activity_runs.first() {
            if self.plans.contains_key(&run.agent_id) {
                return Some(run.agent_id.clone());
            }
        }
        let leader = self.state.as_ref().and_then(|s| s.get("spec")).and_then(|spec| spec.get("leader_id")).and_then(|v| v.as_str()).map(str::to_string);
        match leader {
            Some(leader) if self.plans.contains_key(&leader) => Some(leader),
            _ => self.plans.keys().next().cloned(),
        }
    }

    /// One-line plan status: `计划 2/5  [~] 跑测试`  (empty when nobody has a plan).
    pub fn plan_status(&self) -> Option<(String, String)> {
        let agent = self.plan_agent()?;
        let items = self.plans.get(&agent)?;
        if items.is_empty() {
            return None;
        }
        let done = items.iter().filter(|item| item["status"] == "done").count();
        let current = items
            .iter()
            .find(|item| item["status"] == "in_progress")
            .or_else(|| items.iter().find(|item| item["status"] == "pending"))
            .and_then(|item| item["text"].as_str())
            .unwrap_or("");
        let summary = format!(
            "{} {}/{}",
            self.t("计划", &[]),
            done,
            items.len()
        );
        Some((format!("{summary} · {agent}"), current.to_string()))
    }

    /// Review material for `v`: the diff a member's edit reported back.
    /// ponytail: newest edit batch per member, not a full history.
    fn record_review(&mut self, agent_id: &str, tool: &str, result: &str) {
        if !matches!(tool, "edit_file" | "edit_files" | "write_file") || result.trim().is_empty() {
            return;
        }
        self.reviews.insert(agent_id.to_string(), result.lines().map(str::to_string).collect());
    }

    /// `v` on a member row: show what that member last changed.
    pub fn open_review(&mut self, agent_id: &str) -> bool {
        let Some(lines) = self.reviews.get(agent_id) else { return false };
        let title = self.t("改动审查：{v0}", &[("v0", agent_id)]);
        self.show_overlay(agent_id, title, lines.clone());
        true
    }

    /// `p` on a member row: the whole plan, not just the strip's progress line.
    pub fn open_plan(&mut self, agent_id: &str) -> bool {
        let Some(items) = self.plans.get(agent_id) else { return false };
        if items.is_empty() {
            return false;
        }
        let lines: Vec<String> = items
            .iter()
            .map(|item| {
                let mark = match item["status"].as_str().unwrap_or("pending") {
                    "done" => "[x]",
                    "in_progress" => "[~]",
                    _ => "[ ]",
                };
                format!("{mark} {}", item["text"].as_str().unwrap_or(""))
            })
            .collect();
        let title = self.t("计划：{v0}", &[("v0", agent_id)]);
        self.show_overlay(agent_id, title, lines);
        true
    }

    fn show_overlay(&mut self, agent_id: &str, title: String, lines: Vec<String>) {
        self.review_agent = agent_id.to_string();
        self.overlay_title = title;
        self.review_lines = lines;
        self.review_scroll = 0;
        self.review_open = true;
    }

    pub fn review_title(&self) -> String {
        self.overlay_title.clone()
    }

    fn review_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        use crossterm::event::{KeyCode, KeyModifiers as Mod};
        let ctrl = key.modifiers.contains(Mod::CONTROL);
        let max = self.review_lines.len().saturating_sub(1);
        match (key.code, ctrl) {
            (KeyCode::Esc, _) | (KeyCode::Char('q'), false) => self.review_open = false,
            (KeyCode::Char('u'), true) | (KeyCode::Up, _) => self.review_scroll = self.review_scroll.saturating_sub(5).max(0).min(max),
            (KeyCode::Char('d'), true) | (KeyCode::Down, _) => self.review_scroll = (self.review_scroll + 5).min(max),
            _ => {}
        }
        vec![]
    }

    pub fn on_tool(&mut self, agent_id: &str, tool: &str, ok: bool, arguments: &str) {
        self.on_tool_result(agent_id, tool, ok, arguments, "");
    }

    pub fn on_tool_result(&mut self, agent_id: &str, tool: &str, ok: bool, arguments: &str, result: &str) {
        if ok {
            self.record_review(agent_id, tool, result);
        }
        if !agent_id.is_empty() {
            self.tool_activity.insert(
                agent_id.to_string(),
                ToolActivity { tool: tool.to_string(), ok, at: Instant::now() },
            );
        }
        if self.log_member.as_deref().is_some_and(|member| member != agent_id) {
            return;
        }
        let mark = if ok { "·" } else { "✗" };
        let preview: String = arguments.trim().chars().take(120).collect();
        self.push_log_line(format!("      {mark} {tool:<16} {agent_id:<12} {preview}"));
    }

    /// The log panel is a ring: a long session must not grow the UI without bound.
    fn push_log_line(&mut self, line: String) {
        self.log_lines.push(line);
        if self.log_lines.len() > MAX_LOG_LINES {
            let drop = self.log_lines.len() - MAX_LOG_LINES;
            self.log_lines.drain(..drop);
        }
    }

    /// LogPanel::replay_from — full rebuild on tab activation / filter change.
    pub fn replay_log(&mut self, events: &[Json]) {
        self.log_lines.clear();
        self.tool_activity.clear();
        self.reviews.clear();
        self.log_cursor = 0;
        self.append_log(events);
    }

    // ------------------------------------------------------------ settings

    pub fn settings_lines(&self) -> Vec<String> {
        let session = self.session();
        let spec = self.spec();
        let limits = self.state.as_ref().and_then(|s| s.get("limits")).cloned().unwrap_or(Json::Null);
        let num = |v: &Json, k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0).to_string();
        let members = spec.get("agents").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        let revision = self.state.as_ref().and_then(|s| s.get("revision")).map(|v| v.to_string()).unwrap_or_else(|| "0".into());
        let mut lines = vec![
            self.t("会话：{v0}", &[("v0", &self.session_id)]),
            self.t("状态：{v0}    权限模式：{v1}", &[
                ("v0", &jstr(session, "status")),
                ("v1", &jstr(session, "permissions_mode")),
            ]),
            self.t("工作目录：{v0}", &[("v0", &jstr(session, "cwd"))]),
            self.t("团队：{v0} 名成员，拓扑修订 {v1}", &[("v0", &members.to_string()), ("v1", &revision)]),
            self.t("上限：并发 {v0}、成员 {v1}、单目标回合 {v2}、单回合步骤 {v3}、回合超时 {v4}s", &[
                ("v0", &num(&limits, "max_parallel_workers")),
                ("v1", &num(&limits, "max_members")),
                ("v2", &num(&limits, "max_turns_per_goal")),
                ("v3", &num(&limits, "max_model_steps_per_turn")),
                ("v4", &num(&limits, "turn_active_timeout_s")),
            ]),
            self.t("用户配置：{v0}", &[("v0", &self.user_config_path)]),
        ];
        let models = self.catalog.get("models").and_then(|v| v.as_object());
        let model_list = models
            .map(|m| {
                m.iter()
                    .map(|(name, p)| format!("{name}({}/{})", jstr(p, "provider"), jstr(p, "model")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        lines.push(self.t("模型 profiles：", &[]) + &(if model_list.is_empty() { self.t("无", &[]) } else { model_list }));
        let tools = self.catalog.get("tools").and_then(|v| v.as_object());
        let tool_list = tools
            .map(|t| {
                t.iter()
                    .map(|(name, b)| format!("{name}[{}]", b.get("kind").and_then(|v| v.as_str()).unwrap_or("?")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        lines.push(self.t("工具绑定：", &[]) + &(if tool_list.is_empty() { self.t("无（files/shell/web 为内置）", &[]) } else { tool_list }));
        let join_arr = |key: &str| {
            self.catalog.get(key).and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default()
        };
        let skills = join_arr("skills_paths");
        lines.push(self.t("Skills 目录：", &[]) + &(if skills.is_empty() { self.t("未配置", &[]) } else { skills }));
        let instr = join_arr("instruction_files");
        lines.push(self.t("指令文件：", &[]) + &(if instr.is_empty() { self.t("未配置", &[]) } else { instr }));
        lines.push(String::new());
        lines.push(self.t("恢复：teamagents --resume ", &[]) + &self.session_id + &self.t("    新建：换 --cwd 或删掉会话目录", &[]));
        lines
    }

    // ------------------------------------------------------------ keys

    /// Global priority bindings fire before widget keys.
    /// Bracketed paste goes straight into the composer.
    /// Any deliberate input snaps the chat back to the newest entry.
    pub fn pin_to_bottom(&mut self) {
        self.chat_scroll = 0;
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Some(picker) = &mut self.model_picker {
            picker.query.extend(text.chars().filter(|c| !c.is_control()));
            picker.index = 0;
            return;
        }
        self.pin_to_bottom();
        for c in text.chars() {
            if c == '\n' {
                self.composer.insert_newline();
            } else if c != '\r' {
                self.composer.insert_char(c);
            }
        }
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        use crossterm::event::{KeyCode, KeyModifiers as Mod};
        let ctrl = key.modifiers.contains(Mod::CONTROL);
        if let Some(mut picker) = self.model_picker.take() {
            let effects = picker.handle_key(key, self.lang);
            if effects.iter().any(|e| matches!(e, Effect::DiscoverModels { .. })) { self.model_generation += 1; }
            if !picker.closed { self.model_picker = Some(picker); }
            return effects;
        }
        // overlays swallow keys while open; the picker is the innermost layer
        if self.lang_open {
            return self.settings_dropdown_key(key);
        }
        if self.review_open {
            return self.review_key(key);
        }
        if self.settings_open {
            return self.settings_overlay_key(key);
        }
        match (key.code, ctrl) {
            (KeyCode::Char('q'), true) => return vec![Effect::Quit],
            (KeyCode::Char('p'), true) => return self.action_pause(),
            (KeyCode::Char('r'), true) => {
                // force a repaint (the poll loop is already live)
                return vec![];
            }
            (KeyCode::Char('f'), true) => return self.action_toggle_full_auto(),
            (KeyCode::Char('t'), true) => {
                self.panel = (self.panel + 1) % PANELS.len();
                self.focus = Focus::Composer;
                return vec![];
            }
            (KeyCode::Char('g'), true) => {
                self.panel = PANELS.iter().position(|p| *p == "approvals").unwrap();
                self.focus = Focus::Panel;
                return vec![];
            }
            (KeyCode::Char('n'), true) => {
                self.focus = Focus::Composer;
                return vec![];
            }
            (KeyCode::Esc, false) => {
                if self.focus == Focus::Composer && self.slash_open() {
                    // close the command menu, keep what was typed
                    self.slash_dismissed_for = self.slash_query();
                    return vec![];
                }
                // one key, two obvious meanings: leave the panel, or stop the Leader
                if self.focus == Focus::Panel {
                    self.focus = Focus::Composer;
                    return vec![];
                }
                return self.action_interrupt_leader();
            }
            // scrolling belongs to the pane under the pointer of attention; the
            // Ctrl+D/U chords scroll in every focus (the docs bind them to the
            // scroll, so a panel-focused Ctrl+D must never reach a table action)
            (KeyCode::PageUp, false) | (KeyCode::Char('u'), true) => {
                self.scroll_up(10);
                return vec![];
            }
            (KeyCode::PageDown, _) | (KeyCode::Char('d'), true) => {
                self.scroll_down(10);
                return vec![];
            }
            (KeyCode::Home, _) if ctrl => {
                self.scroll_to_top();
                return vec![];
            }
            (KeyCode::End, _) if ctrl => {
                self.scroll_to_bottom();
                return vec![];
            }
            _ => {}
        }
        if self.focus == Focus::Panel {
            return self.panel_key(key);
        }
        self.composer_key(key)
    }

    /// Wheel/keyboard scrolling: the log panel scrolls its stream, everywhere
    /// else the chat history scrolls (0 = newest entry pinned to the bottom).
    /// Keyboard scrolling follows the focus; the mouse wheel passes the pane
    /// under the pointer explicitly (`scroll_up_target`).
    pub fn scroll_target(&self) -> &'static str {
        if self.focus == Focus::Panel && PANELS[self.panel] == "log" {
            "log"
        } else {
            "chat"
        }
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_up_target(self.scroll_target(), lines);
    }

    pub fn scroll_up_target(&mut self, target: &str, lines: usize) {
        match target {
            "log" => self.log_scroll = self.log_scroll.saturating_add(lines),
            _ => self.chat_scroll = self.chat_scroll.saturating_add(lines),
        }
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_down_target(self.scroll_target(), lines);
    }

    pub fn scroll_down_target(&mut self, target: &str, lines: usize) {
        match target {
            "log" => self.log_scroll = self.log_scroll.saturating_sub(lines),
            _ => self.chat_scroll = self.chat_scroll.saturating_sub(lines),
        }
    }

    /// Ctrl+Home: ask for "everything"; the renderer clamps the request to the
    /// wrapped height it actually drew (the width is only known there).
    pub fn scroll_to_top(&mut self) {
        match self.scroll_target() {
            "log" => self.log_scroll = usize::MAX,
            _ => self.chat_scroll = usize::MAX,
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.chat_scroll = 0;
        self.log_scroll = 0;
    }

    /// The token being typed after "/" (None when the composer is not a command).
    pub fn slash_query(&self) -> Option<String> {
        let text = self.composer.text();
        let first = text.lines().next().unwrap_or("").trim_start();
        if !first.starts_with('/') || !text.trim().contains('\n') && first.contains(' ') {
            return None;
        }
        if text.lines().count() > 1 || first.contains(' ') {
            return None;
        }
        Some(first.to_string())
    }

    /// Commands matching the current query (all of them when it is just "/").
    pub fn slash_matches(&self) -> Vec<&'static SlashCommand> {
        let Some(query) = self.slash_query() else { return vec![] };
        SLASH_COMMANDS
            .iter()
            .filter(|c| c.name.starts_with(query.as_str()))
            .collect()
    }

    /// The menu is open while a query is typed and Esc has not dismissed it.
    pub fn slash_open(&self) -> bool {
        let Some(query) = self.slash_query() else { return false };
        if self.slash_dismissed_for.as_deref() == Some(query.as_str()) {
            return false;
        }
        !self.slash_matches().is_empty()
    }

    pub fn slash_selected(&self) -> Option<&'static SlashCommand> {
        let matches = self.slash_matches();
        if matches.is_empty() {
            return None;
        }
        let index = self.slash_index.min(matches.len() - 1);
        Some(matches[index])
    }

    /// Run the highlighted command (Enter in the menu) and clear the composer.
    fn run_slash(&mut self) -> Vec<Effect> {
        let Some(command) = self.slash_selected() else { return vec![] };
        self.composer.clear();
        self.slash_index = 0;
        match command.name {
            "/settings" => {
                self.settings_open = true;
                vec![]
            }
            "/quit" => vec![Effect::Quit],
            "/status" => vec![Effect::UsageStatus],
            "/rewind" => vec![Effect::RewindPoints],
            "/fork" => vec![Effect::Fork],
            "/model" => vec![Effect::ModelStatus],
            "/help" => {
                let lines = vec![
                    self.t("斜杠命令：/help 本说明 · /settings 设置 · /status 用量 · /model 模型 · /rewind 回退 · /fork 分叉 · /quit 退出", &[]),
                    self.t("输入：Enter 发送 · Shift+Enter 换行 · ↑↓ 历史 · PgUp/PgDn 滚动", &[]),
                    self.t("界面：Ctrl+T 切面板 · Ctrl+G 批准 · Ctrl+F 全自动 · Ctrl+P 暂停 · Esc 停止 Leader · Ctrl+Q 退出", &[]),
                ];
                self.write_chat("system", &lines.join("\n"));
                vec![]
            }
            _ => vec![],
        }
    }

    /// `/rewind <n>` picks from the last shown list; `/rewind <id>` rewinds to
    /// a node id directly; `/rewind 0` empties the conversation.
    fn run_rewind_args(&mut self, rest: &str) -> Vec<Effect> {
        let arg = rest.trim();
        if arg == "0" {
            return vec![Effect::Rewind { node: None }];
        }
        if let Ok(n) = arg.parse::<usize>() {
            match self.rewind_list.get(n.saturating_sub(1)) {
                Some(node) => return vec![Effect::Rewind { node: Some(node.clone()) }],
                None => {
                    let msg = self.t("没有这个序号（先用 /rewind 列出可回退点）", &[]);
                    self.write_chat("system", &msg);
                    return vec![];
                }
            }
        }
        vec![Effect::Rewind { node: Some(arg.to_string()) }]
    }

    pub fn show_rewind_points(&mut self, report: Result<Json, String>) {
        let report = match report {
            Ok(report) => report,
            Err(e) => {
                let msg = self.t("获取回退点失败：{v0}", &[("v0", &e)]);
                self.write_chat("system", &msg);
                return;
            }
        };
        let points = report.get("points").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        self.rewind_list = points.iter().filter_map(|p| p.get("id").and_then(|v| v.as_str()).map(str::to_string)).collect();
        if points.is_empty() {
            let msg = self.t("暂无可回退的节点（leader 还没有对话历史）", &[]);
            self.write_chat("system", &msg);
            return;
        }
        let mut lines = vec![self.t("可回退点（/rewind <序号> 保留该条输入，移开后续对话）：", &[])];
        for (i, point) in points.iter().enumerate() {
            let preview = point.get("preview").and_then(|v| v.as_str()).unwrap_or("");
            let line = self.t("  {v0}. {v1}", &[("v0", &(i + 1).to_string()), ("v1", preview)]);
            lines.push(line);
        }
        self.write_chat("system", &lines.join("\n"));
    }

    pub fn show_rewind_done(&mut self, result: Result<Json, String>) {
        match result {
            Ok(v) => {
                let depth = v.get("depth").and_then(|v| v.as_u64()).unwrap_or(0);
                let msg = self.t("已回退对话（当前 {v0} 条消息；被放弃的分支仍保留，可再次 /rewind）", &[("v0", &depth.to_string())]);
                self.write_chat("system", &msg);
            }
            Err(e) => {
                let msg = self.t("回退失败：{v0}", &[("v0", &e)]);
                self.write_chat("system", &msg);
            }
        }
    }

    /// Local reset shared by session switch and fork (the worker has already
    /// opened the target session by the time this runs).
    fn apply_switched(&mut self, session_id: String, catalog: Json, config_path: String) {
        self.session_id = session_id;
        self.catalog = catalog;
        self.user_config_path = config_path;
        self.cursor = 0;
        self.chat.clear();
        self.delta_buffers.clear();
        self.stream_text.clear();
        self.activity_runs.clear();
        self.latest_activity = (self.t("等待输入", &[]), vec![]);
        self.composer.clear_composer();
        self.log_cursor = 0;
        self.log_lines.clear();
        self.tool_activity.clear();
        self.reviews.clear();
        self.state = None;
        self.pending_delete = None;
        self.rewind_list.clear();
        self.table_cursors.clear();
        self.log_member = None;
        self.model_picker = None;
        self.model_labels.clear();
    }

    pub fn show_fork_done(&mut self, result: Result<Json, String>) {
        match result {
            Ok(v) => {
                let session_id = v.get("session_id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let catalog = v.get("catalog").cloned().unwrap_or(Json::Null);
                let config_path = v.get("user_config_path").and_then(|x| x.as_str()).unwrap_or("").to_string();
                // the worker already switched; reset local state like a session switch
                self.apply_switched(session_id.clone(), catalog, config_path);
                let from = v.get("forked_from").and_then(|x| x.as_str()).unwrap_or("");
                let msg = self.t("已从 {v0} 分叉到 {v1}（对话已带上，团队状态全新）", &[("v0", from), ("v1", &session_id)]);
                self.write_chat("system", &msg);
            }
            Err(e) => {
                let msg = self.t("分叉失败：{v0}", &[("v0", &e)]);
                self.write_chat("system", &msg);
            }
        }
    }

    /// `/model <member> <model> [effort]` or `<member> clear` (Codex /model parity).
    fn run_model_args(&mut self, rest: &str) -> Vec<Effect> {
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        match tokens.as_slice() {
            [member, "clear"] => vec![Effect::SetModel { agent_id: member.to_string(), profile: None, model: None, effort: None }],
            [member, model] => vec![Effect::SetModel { agent_id: member.to_string(), profile: None, model: Some(model.to_string()), effort: None }],
            [member, model, effort] => vec![Effect::SetModel {
                agent_id: member.to_string(),
                profile: None,
                model: Some(model.to_string()),
                effort: Some(effort.to_string()),
            }],
            _ => {
                let msg = self.t("用法：/model <成员> <模型> [档位] · /model <成员> clear 恢复默认", &[]);
                self.write_chat("system", &msg);
                vec![]
            }
        }
    }

    /// `/model` with no args: effective model/effort per member, `*` marks a
    /// session-level override.
    pub fn show_models(&mut self, report: Result<Json, String>) {
        let report = match report {
            Ok(report) => report,
            Err(e) => {
                let msg = self.t("获取模型信息失败：{v0}", &[("v0", &e)]);
                self.write_chat("system", &msg);
                return;
            }
        };
        self.model_labels.clear();
        for agent in report["agents"].as_array().into_iter().flatten() { self.record_model_label(agent); }
        self.model_generation += 1;
        self.model_picker = Some(crate::model_picker::ModelPicker::new(&report));
        let mut lines = vec![self.t("成员模型（* = 会话内覆盖，重开会话失效）：", &[])];
        for agent in report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let not_configured = self.t("未配置", &[]);
            let name = agent.get("name").and_then(|v| v.as_str()).unwrap_or("?").to_string();
            let model = agent.get("model").and_then(|v| v.as_str()).unwrap_or(not_configured.as_str()).to_string();
            let effort = agent.get("effort").and_then(|v| v.as_str()).unwrap_or(not_configured.as_str()).to_string();
            let mark = if agent.get("overridden").and_then(|v| v.as_bool()).unwrap_or(false) { "*" } else { "" };
            let args: Vec<(&str, String)> = vec![("v0", name), ("v1", model), ("v2", effort), ("v3", mark.into())];
            let refs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
            lines.push(self.t("{v0} | 模型 {v1} | 档位 {v2}{v3}", &refs));
        }
        self.write_chat("system", &lines.join("\n"));
    }

    pub fn show_discovered_models(&mut self, session: &str, generation: u64, provider: &str, result: Result<Json, String>) {
        if session != self.session_id || generation != self.model_generation { return; }
        if let Some(picker) = &mut self.model_picker { picker.merge_discovered(provider, result, self.lang); }
    }

    /// `/model <member> …` result: the effective values after the change.
    pub fn show_model_set(&mut self, result: Result<Json, String>) {
        let v = match result {
            Ok(v) => v,
            Err(e) => {
                let msg = self.t("模型切换失败：{v0}", &[("v0", &e)]);
                self.write_chat("system", &msg);
                return;
            }
        };
        self.record_model_label(&v);
        let not_configured = self.t("未配置", &[]);
        let get = |key: &str| {
            let value = v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string();
            if value.is_empty() { not_configured.clone() } else { value }
        };
        let overridden = v.get("overridden").and_then(|x| x.as_bool()).unwrap_or(false);
        let args: Vec<(&str, String)> = vec![("v0", get("agent_id")), ("v1", get("model")), ("v2", get("effort"))];
        let refs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let id = if overridden {
            "已切换 {v0}：模型 {v1} · 档位 {v2}（下一回合生效）"
        } else {
            "已恢复 {v0} 的 profile 默认：模型 {v1} · 档位 {v2}"
        };
        let mut msg = self.t(id, &refs);
        if let Some(provider) = v.get("provider").and_then(Json::as_str) {
            msg.push_str(&self.t(" · 供应商 {v0}", &[("v0", provider)]));
        }
        self.write_chat("system", &msg);
    }

    fn record_model_label(&mut self, agent: &Json) {
        let id = jstr(agent, "agent_id");
        if agent["overridden"] == true {
            self.model_labels.insert(id, format!("{} / {} · {} *", jstr(agent, "provider"), jstr(agent, "model"), jstr(agent, "effort")));
        } else {
            self.model_labels.remove(&id);
        }
    }

    fn model_label(&self, agent: &Json) -> String {
        self.model_labels.get(&jstr(agent, "id")).cloned().unwrap_or_else(|| jstr(agent, "model_profile"))
    }

    /// `/status` result: one line per member — model | context window |
    /// cumulative tokens (prompt/completion) | remaining context.
    pub fn show_usage(&mut self, report: Result<Json, String>) {
        let report = match report {
            Ok(report) => report,
            Err(e) => {
                let msg = self.t("获取用量失败：{v0}", &[("v0", &e)]);
                self.write_chat("system", &msg);
                return;
            }
        };
        let mut lines = vec![self.t("Token 用量（本次会话累计，重启归零）：", &[])];
        for agent in report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let get = |key: &str| agent.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string();
            let usage = agent.get("usage").cloned().unwrap_or(Json::Null);
            let num = |key: &str| usage.get(key).and_then(Json::as_u64).unwrap_or(0);
            let not_configured = self.t("未配置", &[]);
            let window = agent.get("context_window").and_then(Json::as_u64);
            let window_s = window.map(|w| w.to_string()).unwrap_or_else(|| not_configured.clone());
            let remaining = window
                .map(|w| w.saturating_sub(num("last_prompt_tokens")).to_string())
                .unwrap_or(not_configured);
            let name = get("name");
            let model = agent.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let model = if model.is_empty() { self.t("未配置", &[]) } else { model };
            let (total, prompt, completion) = (num("total_tokens"), num("prompt_tokens"), num("completion_tokens"));
            let args: Vec<(&str, String)> = vec![
                ("v0", name), ("v1", model), ("v2", window_s),
                ("v3", total.to_string()), ("v4", prompt.to_string()),
                ("v5", completion.to_string()), ("v6", remaining),
            ];
            let refs: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
            lines.push(self.t("{v0} | {v1} | 窗口 {v2} | {v3} ({v4}/{v5}) | 剩余 {v6}", &refs));
        }
        self.write_chat("system", &lines.join("
"));
    }

    /// `/settings`: Enter opens the language picker, Esc closes the overlay.
    fn settings_overlay_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.settings_open = false,
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.lang_open = true;
                self.lang_choice = if self.lang == "zh-CN" { 1 } else { 0 };
            }
            _ => {}
        }
        vec![]
    }

    fn settings_dropdown_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc => self.lang_open = false,
            KeyCode::Up => self.lang_choice = self.lang_choice.saturating_sub(1),
            KeyCode::Down => self.lang_choice = (self.lang_choice + 1).min(1),
            KeyCode::Enter => {
                self.lang_open = false;
                let lang = if self.lang_choice == 1 { "zh-CN" } else { "en" };
                if lang != self.lang {
                    self.lang = lang;
                    if let Err(e) = crate::i18n::write_preferences(self.lang, self.animations) {
                        let msg = self.t("偏好保存失败：{v0}", &[("v0", &e.to_string())]);
                        self.notify(msg, Severity::Error, 10);
                    }
                }
            }
            _ => {}
        }
        vec![]
    }

    fn composer_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        use crossterm::event::{KeyCode, KeyModifiers as Mod};
        // any interaction with the composer means "show me the latest"
        self.pin_to_bottom();
        if self.slash_open() {
            match key.code {
                KeyCode::Up => {
                    self.slash_index = self.slash_index.saturating_sub(1);
                    return vec![];
                }
                KeyCode::Down => {
                    let last = self.slash_matches().len().saturating_sub(1);
                    self.slash_index = (self.slash_index + 1).min(last);
                    return vec![];
                }
                KeyCode::Tab => {
                    if let Some(command) = self.slash_selected() {
                        self.composer.set_text(command.name);
                        self.slash_index = 0;
                    }
                    return vec![];
                }
                KeyCode::Enter => return self.run_slash(),
                _ => {}
            }
        }
        match key.code {
            KeyCode::Enter if key.modifiers.contains(Mod::SHIFT) || key.modifiers.contains(Mod::CONTROL) => {
                self.composer.insert_newline();
            }
            KeyCode::Char('j') if key.modifiers.contains(Mod::CONTROL) => self.composer.insert_newline(),
            KeyCode::Enter => {
                if let Some(text) = self.composer.submit() {
                    let trimmed = text.trim();
                    if trimmed.starts_with('/') {
                        // /model carries arguments; the menu only covers bare names
                        if let Some(rest) = trimmed.strip_prefix("/model ") {
                            return self.run_model_args(rest);
                        }
                        if let Some(rest) = trimmed.strip_prefix("/rewind ") {
                            return self.run_rewind_args(rest);
                        }
                        // an unknown or argument-carrying command must never be
                        // sent to the Leader as a normal message
                        return match SLASH_COMMANDS.iter().find(|c| c.name == trimmed) {
                            Some(command) => {
                                self.composer.set_text(command.name);
                                self.run_slash()
                            }
                            None => {
                                self.composer.set_text(&text); // keep the draft for editing
                                let msg = self.t("未知命令：{v0}（/help 查看可用命令）", &[("v0", trimmed)]);
                                self.write_chat("system", &msg);
                                vec![]
                            }
                        };
                    }
                    return vec![Effect::UserMessage(text)];
                }
            }
            KeyCode::Up if self.composer.row == 0 => self.composer.recall(-1),
            KeyCode::Down if self.composer.row + 1 == self.composer.lines.len() => self.composer.recall(1),
            KeyCode::Up => self.composer.move_up(),
            KeyCode::Down => self.composer.move_down(),
            KeyCode::Char('w') if key.modifiers.contains(Mod::CONTROL) => self.composer.delete_word(),
            KeyCode::Left if key.modifiers.contains(Mod::CONTROL) || key.modifiers.contains(Mod::ALT) => {
                self.composer.move_word_left()
            }
            KeyCode::Right if key.modifiers.contains(Mod::CONTROL) || key.modifiers.contains(Mod::ALT) => {
                self.composer.move_word_right()
            }
            KeyCode::Backspace if key.modifiers.contains(Mod::ALT) => self.composer.delete_word(),
            KeyCode::Left => self.composer.move_left(),
            KeyCode::Right => self.composer.move_right(),
            KeyCode::Home => self.composer.move_home(),
            KeyCode::End => self.composer.move_end(),
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            // Tab enters the panel whatever tab is shown (the removal of the
            // settings tab left shared/log unreachable by keyboard otherwise);
            // the panel's own Tab/Esc returns to the composer.
            KeyCode::Tab | KeyCode::BackTab => self.focus = Focus::Panel,
            KeyCode::Char('a') if key.modifiers.contains(Mod::CONTROL) => self.composer.move_home(),
            KeyCode::Char('e') if key.modifiers.contains(Mod::CONTROL) => self.composer.move_end(),
            // an unhandled control chord must never type a letter into the composer
            KeyCode::Char(_) if key.modifiers.contains(Mod::CONTROL) => {}
            KeyCode::Char(c) => self.composer.insert_char(c),
            _ => {}
        }
        vec![]
    }

    fn panel_key(&mut self, key: crossterm::event::KeyEvent) -> Vec<Effect> {
        use crossterm::event::{KeyCode, KeyModifiers as Mod};
        // panel actions are plain keys: Ctrl+A/E are line motions and Ctrl+D/U
        // scroll (docs), so a chord must never archive/deny/cancel a row
        if key.modifiers.intersects(Mod::CONTROL | Mod::ALT)
            && !matches!(key.code, KeyCode::Tab | KeyCode::BackTab)
        {
            return vec![];
        }
        let panel = PANELS[self.panel];
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = Focus::Composer;
                return vec![];
            }
            KeyCode::Up => {
                if panel == "log" {
                    self.cycle_log_member(-1);
                } else {
                    self.move_table_selection(-1);
                }
                return vec![];
            }
            KeyCode::Down => {
                if panel == "log" {
                    self.cycle_log_member(1);
                } else {
                    self.move_table_selection(1);
                }
                return vec![];
            }
            _ => {}
        }
        let rows: Vec<String> = self.panel_row_keys(panel);
        let saved = self.table_cursors.get(panel).cloned().unwrap_or((None, 0));
        // same selection resolution as the mouse path (select_row_visible): the
        // saved key wins, the stale index is only a fallback — the 250ms poll
        // may have reordered or removed rows since the cursor was stored
        let sel = saved
            .0
            .and_then(|k| rows.iter().position(|rk| *rk == k))
            .or_else(|| (saved.1 < rows.len()).then_some(saved.1));
        // sessions row keys may carry a cursor-only dedup suffix
        // (`sid#archived`, sessions_rows); actions always use the real id
        let selected = sel.map(|i| {
            let key = &rows[i];
            if panel == "sessions" { session_row_id(key).to_string() } else { key.clone() }
        });
        match (panel, key.code) {
            ("log", KeyCode::Enter) => {
                self.log_member = None; // Enter clears the member filter
            }
            ("team", KeyCode::Enter) => {
                if selected.is_some() && self.log_member == selected {
                    self.log_member = None; // Enter on the highlighted member clears the filter
                }
            }
            ("team", KeyCode::Char('v')) | ("log", KeyCode::Char('v')) => {
                match selected {
                    Some(agent) if self.open_review(&agent) => {}
                    Some(agent) => {
                        let translated = self.t("{v0} 还没有可审查的改动", &[("v0", &agent)]);
                        self.notify(translated, Severity::Info, 3);
                    }
                    None => {}
                }
            }
            ("tasks", KeyCode::Enter) => {
                if let Some(task_id) = selected {
                    if let Some(t) = self.find_task(&task_id) {
                        let deps = t.get("dependencies").and_then(|v| v.as_array())
                            .map(|d| d.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(","))
                            .unwrap_or_default();
                        let refs = t.get("result_refs").and_then(|v| v.as_array())
                            .map(|d| d.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(","))
                            .unwrap_or_default();
                        let msg = self.t("任务 {v0} | {v1} | {v2} | 依赖 {v3} | 成果 {v4}", &[
                            ("v0", &task_id),
                            ("v1", &jstr(&t, "status")),
                            ("v2", &jstr(&t, "description")),
                            ("v3", &deps),
                            ("v4", &refs),
                        ]);
                        self.notify(msg, Severity::Info, 10);
                    }
                }
            }
            ("team", KeyCode::Char('p')) => {
                match selected {
                    Some(agent) if self.open_plan(&agent) => {}
                    Some(agent) => {
                        let msg = self.t("{v0} 还没有计划", &[("v0", &agent)]);
                        self.notify(msg, Severity::Info, 3);
                    }
                    None => {}
                }
            }
            ("team", KeyCode::Char('c')) => {
                match selected.as_ref().and_then(|agent| self.unknown_runs.get(agent)) {
                    Some(run_id) => return vec![Effect::AcknowledgeRun(run_id.clone())],
                    None => {
                        let msg = self.t("该成员没有结果不明的回合", &[]);
                        self.notify(msg, Severity::Info, 3);
                    }
                }
            }
            ("tasks", KeyCode::Char('c')) => {
                if let Some(task_id) = selected {
                    return vec![Effect::CancelTask(task_id)];
                }
            }
            ("approvals", KeyCode::Char(c @ ('a' | 's' | 'd'))) => {
                if let Some(approval_id) = selected {
                    let decision = match c {
                        'a' => "once",
                        's' => "session",
                        _ => "deny",
                    };
                    return vec![Effect::DecideApproval { approval_id, decision: decision.into() }];
                }
            }
            ("sessions", KeyCode::Enter) | ("sessions", KeyCode::Char('s')) => {
                if let Some(target) = selected {
                    if self.session_row_archived(sel) {
                        // the engine would silently open a fresh empty session
                        // for the unknown archived id — block at the source
                        let msg = self.t("会话已归档，不能切换", &[]);
                        self.notify(msg, Severity::Warning, 5);
                    } else if target != self.session_id {
                        return vec![Effect::SwitchSession(target)];
                    }
                }
            }
            ("sessions", KeyCode::Char('n')) => return vec![Effect::NewSession],
            ("sessions", KeyCode::Char('a')) => {
                if let Some(target) = selected {
                    if self.session_row_archived(sel) {
                        let msg = self.t("会话已归档，不能归档/删除", &[]);
                        self.notify(msg, Severity::Warning, 5);
                    } else {
                        return vec![Effect::ArchiveSession(target)];
                    }
                }
            }
            ("sessions", KeyCode::Char('d')) => {
                if let Some(target) = selected {
                    // re-checked on the confirm press too: a row archived
                    // between the two d presses must not delete its active twin
                    if self.session_row_archived(sel) {
                        self.pending_delete = None;
                        let msg = self.t("会话已归档，不能归档/删除", &[]);
                        self.notify(msg, Severity::Warning, 5);
                    } else if self.pending_delete.as_deref() != Some(target.as_str()) {
                        self.pending_delete = Some(target.clone());
                        let msg = self.t("再按一次 d 确认删除会话 {v0}", &[("v0", &target)]);
                        self.write_chat("system", &msg);
                    } else {
                        self.pending_delete = None;
                        return vec![Effect::DeleteSession(target)];
                    }
                }
            }
            _ => {}
        }
        vec![]
    }

    pub fn panel_row_keys(&self, panel: &str) -> Vec<String> {
        match panel {
            "team" => self.team_rows().into_iter().map(|(k, _)| k).collect(),
            "tasks" => self.tasks_rows().into_iter().map(|(k, _)| k).collect(),
            "approvals" => self.approvals_rows().into_iter().map(|(k, _)| k).collect(),
            "sessions" => self.sessions_rows().into_iter().map(|(k, _)| k).collect(),
            "shared" => self.shared_rows().into_iter().map(|(k, _)| k).collect(),
            _ => vec![],
        }
    }

    /// Log panel filter: ↑↓ walks "all → each member → all"; the log
    /// stream is filtered by the highlighted member; Enter clears the filter.
    fn cycle_log_member(&mut self, delta: isize) {
        let mut ring: Vec<Option<String>> = vec![None];
        ring.extend(self.panel_row_keys("team").into_iter().map(Some));
        if ring.len() <= 1 {
            return;
        }
        let current = ring.iter().position(|member| *member == self.log_member).unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(ring.len() as isize) as usize;
        self.log_member = ring[next].take();
    }

    fn find_task(&self, task_id: &str) -> Option<Json> {
        self.state
            .as_ref()?
            .get("tasks")?
            .as_array()?
            .iter()
            .find(|t| jstr(t, "task_id") == task_id)
            .cloned()
    }

    // ------------------------------------------------------------ actions

    fn action_interrupt_leader(&mut self) -> Vec<Effect> {
        let leader = self.leader_id();
        if let Some(run) = self.activity_runs.iter().find(|r| r.agent_id == leader) {
            let run_id = run.run_id.clone();
            return vec![Effect::Submit {
                action: json!({
                    "action_id": format!("ui-stop-{:x}", now_ts() as u64),
                    "actor_id": "user",
                    "kind": "cancel_run",
                    "payload": {"run_id": run_id},
                }),
                ok_msg: Some(self.t("已请求停止 Leader，等待执行结束", &[])),
                err_msg: Some(self.t("停止失败：{error}", &[])),
            }];
        }
        vec![]
    }

    fn action_pause(&mut self) -> Vec<Effect> {
        if jstr(self.session(), "status") == "PAUSED" {
            let text = self.t("继续执行", &[]);
            return vec![Effect::UserMessage(text)];
        }
        vec![Effect::Submit {
            action: json!({
                "action_id": format!("ui-pause-{}", self.cursor),
                "actor_id": "user",
                "kind": "pause_session",
                "payload": {},
            }),
            ok_msg: Some(self.t("会话已暂停（输入新消息即恢复）", &[])),
            err_msg: None,
        }]
    }

    fn action_toggle_full_auto(&mut self) -> Vec<Effect> {
        let mode = if jstr(self.session(), "permissions_mode") != "full_auto" { "full_auto" } else { "approved_scope" };
        vec![Effect::Submit {
            action: json!({
                "action_id": format!("ui-mode-{mode}-{}", self.cursor),
                "actor_id": "user",
                "kind": "set_permission_mode",
                "payload": {"mode": mode},
            }),
            ok_msg: Some(self.t("权限模式切换为 {v0}", &[("v0", mode)])),
            err_msg: None,
        }]
    }

    // ---------------------------------------------------- async op results

    pub fn on_op_result(&mut self, result: OpResult) -> Vec<Effect> {
        match result {
            OpResult::Switched { session_id, catalog, config_path } => {
                self.apply_switched(session_id.clone(), catalog, config_path);
                let msg = self.t("已切换到会话 {v0}", &[("v0", &session_id)]);
                self.write_chat("system", &msg);
            }
            OpResult::Failed { op, target, error } => {
                let msg = match op {
                    "switch" => self.t("✗ 无法打开会话 {v0}：{v1}", &[("v0", &target), ("v1", &error)]),
                    "archive" => self.t("✗ 归档失败：{v0}", &[("v0", &error)]),
                    _ => self.t("✗ 删除失败：{v0}", &[("v0", &error)]),
                };
                self.write_chat("system", &msg);
            }
            OpResult::Archived { session_id, target, was_current } => {
                let msg = self.t("已归档会话 {v0} → {v1}", &[("v0", &session_id), ("v1", &target)]);
                self.write_chat("system", &msg);
                if was_current {
                    return vec![Effect::Quit];
                }
            }
            OpResult::Deleted { session_id, was_current } => {
                let msg = self.t("已删除会话 {v0}", &[("v0", &session_id)]);
                self.write_chat("system", &msg);
                if was_current {
                    return vec![Effect::Quit];
                }
            }
        }
        vec![]
    }
}

fn compact_json(v: &Json) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

/// `str(value)`-style repr for JSON data (single quotes, True/False/None) —
/// the approvals panel shows `str(args)[:60]`, not JSON.
pub fn py_repr(value: &Json) -> String {
    match value {
        Json::Null => "None".into(),
        Json::Bool(b) => if *b { "True".into() } else { "False".into() },
        Json::Number(n) => n.to_string(),
        Json::String(s) => format!("'{s}'"),
        Json::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Json::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("'{k}': {}", py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// JSON text with `", "` / `": "` separators and non-ASCII escaped — the log
/// panel stores and shows exactly that text.
pub fn py_json_dumps(value: &Json) -> String {
    fn escape(text: &str) -> String {
        let mut out = String::new();
        for ch in text.chars() {
            if ch.is_ascii() {
                match ch {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            } else {
                let mut buf = [0u16; 2];
                for unit in ch.encode_utf16(&mut buf).iter() {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
        out
    }
    match value {
        Json::Null => "null".into(),
        Json::Bool(b) => if *b { "true".into() } else { "false".into() },
        Json::Number(n) => n.to_string(),
        Json::String(s) => format!("\"{}\"", escape(s)),
        Json::Array(items) => format!(
            "[{}]",
            items.iter().map(py_json_dumps).collect::<Vec<_>>().join(", ")
        ),
        Json::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("\"{}\": {}", escape(k), py_json_dumps(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// One approval row's display line.
pub fn approval_line(payload: &Json) -> String {
    let scope = payload.get("scope").cloned().unwrap_or(Json::Null);
    let tool = ["tool", "kind"].iter().find_map(|k| scope.get(k).and_then(|v| v.as_str())).unwrap_or("?");
    let args = scope.get("args").or_else(|| scope.get("request")).cloned().unwrap_or(Json::Null);
    let args = py_repr(&args);
    format!("{} {tool} {}", jstr(payload, "agent_id"), head_chars(&args, 60))
}

/// Log line format: `{seq:>5} {kind:<18} {actor:<10} {payload_json[:160]}`
/// Last tool a member ran (team panel column).
pub struct ToolActivity {
    tool: String,
    ok: bool,
    at: Instant,
}

fn elapsed_short(age: Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m", seconds / 60)
    }
}

/// Newest log lines kept in memory (the panel scrolls; older lines are dropped).
const MAX_LOG_LINES: usize = 2000;

fn format_log_line(ev: &Json, payload: &str) -> String {
    let seq = ev.get("sequence").and_then(|v| v.as_i64()).unwrap_or(0);
    let kind = jstr(ev, "kind");
    let actor = jstr(ev, "actor_id");
    let payload = if payload.chars().count() > 160 {
        format!("{}…", head_chars(payload, 160))
    } else {
        payload.to_string()
    };
    format!("{seq:>5} {kind:<18} {actor:<10} {payload}")
}

pub fn panel_tab_label(lang: &str, index: usize) -> String {
    tr(lang, PANEL_TAB_LABELS[index], &[])
}

// ---------------------------------------------------------- receipt feedback

/// TasksPanel::cancel — the message depends on the receipt's result status.
pub fn cancel_task_feedback(lang: &str, task_id: &str, receipt: &Json) -> String {
    let short = tail8(task_id);
    let ok = receipt.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    if !ok {
        let err = receipt.get("error").and_then(|v| v.as_str()).unwrap_or("").to_string();
        return tr(lang, "取消任务失败：{v0}", &[("v0", &err)]);
    }
    let status = receipt
        .get("result")
        .and_then(|r| r.get("status"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match status.as_str() {
        "CANCELLED" => tr(lang, "任务 {v0} 已取消", &[("v0", &short)]),
        "CANCEL_REQUESTED" => tr(lang, "任务 {v0} 已请求取消（活动回合结束后生效）", &[("v0", &short)]),
        _ => tr(lang, "任务 {v0} 已处于终态（{v1}），无需取消", &[("v0", &short), ("v1", &status)]),
    }
}

/// ApprovalsPanel::_decide toast.
pub fn decide_feedback(lang: &str, decision: &str, ok: bool) -> String {
    let outcome = tr(lang, if ok { "已提交" } else { "提交失败" }, &[]);
    tr(lang, "批准决定 {v0}：{v1}", &[("v0", decision), ("v1", &outcome)])
}

pub fn rejected_feedback(lang: &str, error: &str) -> String {
    tr(lang, "[输入被拒绝] {v0}", &[("v0", error)])
}
