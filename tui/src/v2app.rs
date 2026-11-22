//! R2-P4 R19-b② v2 conversation interface state and key handling (plan §9).
//! Pure logic like app.rs: side effects leave as `V2Effect`s executed by the
//! main loop through the daemon client or session CLI, so interaction logic
//! is testable without a terminal. Tools and models run in the engine;
//! history is the authoritative conversation, events are signals,
//! and system notes are UI-level annotations on top.

use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};

use crate::text::Composer;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Composer,
    Approvals,
    Team,
    Panel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DragTarget {
    Sidebar,
    Inspector,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ChatKind {
    User,
    Assistant,
    Tool,
    /// The runtime's own closing word (a settlement note, D-71): the instance
    /// did not say it, so it is never rendered as the member's message.
    Runtime,
    /// A runtime compaction summary (R22/A20): the model's view is covered by
    /// it, so the user must see where the conversation was compacted.
    Summary,
    /// Stored team/context notices, distinct from transient client annotations.
    Context,
    System,
    Error,
    /// Client help and receipts survive checkpoint refreshes (D-433/D-445).
    /// They are requested output, rather than transient runtime chatter.
    Command,
}

#[derive(Clone, Debug, Hash)]
pub struct ChatEntry {
    pub kind: ChatKind,
    pub who: String,
    pub text: String,
    pub tool: Option<ToolInfo>,
    pub workspace: String,
    pub media: Vec<crate::graphics::Source>,
}

#[derive(Clone, Debug, Hash)]
pub struct ToolInfo {
    pub name: String,
    pub arguments: String,
    pub output: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InstanceInfo {
    pub id: String,
    pub lifecycle: String,
    pub phase: String,
    /// The model this member runs on (D-69), available in /help for the Leader.
    /// Empty for an older daemon that does not report it.
    pub model: String,
    /// The last lifecycle transition's own words (D-165): why this instance is not
    /// running — a park reason, or the user's own pause. Empty for an ACTIVE member
    /// and for a transition recorded without words.
    pub reason: String,
    /// The instance's `reasoning_effort`, when its profile carries one (D-473).
    pub effort: String,
}

/// One entry of the daemon's `[models]` catalog, offered by the Ctrl+M menu (D-473).
#[derive(Clone, Debug)]
pub struct ModelInfo {
    /// The `[models.<key>]` name that `set_instance_profile` accepts.
    pub key: String,
    /// The wire model shown in the picker.
    pub model: String,
    pub provider: String,
    /// The ordered effort levels this model declares (D-476), or empty for the built-in list.
    pub efforts: Vec<String>,
    /// Native context window reported by the daemon catalog (`[models.<key>] context_window`).
    pub context_window: Option<i64>,
}

/// Active Ctrl+F transcript search state (Conversation view).
#[derive(Clone, Debug, Default)]
pub struct ChatSearch {
    pub query: String,
    pub editing: bool,
    pub matches: Vec<usize>,
    pub current: usize,
}

impl ModelInfo {
    pub(crate) fn label(&self) -> String {
        if self.provider.is_empty() {
            self.model.clone()
        } else {
            format!("{}({})", self.model, self.provider)
        }
    }
}

/// Select a Leader model and effort or a shared Worker model policy.
#[derive(Clone, Debug)]
pub struct ProfileMenu {
    pub role: ProfileRole,
    pub instance: String,
    pub all_models: Vec<ModelInfo>,
    pub models: Vec<ModelInfo>,
    pub search: String,
    pub efforts: Vec<String>,
    pub model_ix: usize,
    pub effort_ix: usize,
    pub workers_mode: WorkersMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileRole {
    Leader,
    Workers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkersMode {
    Unresolved,
    Follow,
    Pinned,
}

/// The built-in order used when a model declares no `efforts` list (D-476).
const EFFORT_ORDER: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

fn default_efforts() -> Vec<String> {
    EFFORT_ORDER.iter().map(|effort| (*effort).to_string()).collect()
}

fn model_efforts(model: &ModelInfo, fallback: &[String]) -> Vec<String> {
    if !model.efforts.is_empty() {
        model.efforts.clone()
    } else if !fallback.is_empty() {
        fallback.to_vec()
    } else {
        default_efforts()
    }
}

fn effort_rank(effort: &str) -> Option<usize> {
    EFFORT_ORDER.iter().position(|value| value.eq_ignore_ascii_case(effort))
}

/// Pick the menu index for `current`: keep it when declared; otherwise prefer the highest declared
/// level at or below it (the “adjacent lower” rule), then the lowest declared level, then the
/// first entry. Unknown/custom values fall back to the first declared value.
fn effort_index(efforts: &[String], current: &str) -> usize {
    if let Some(index) = efforts.iter().position(|effort| effort.eq_ignore_ascii_case(current)) {
        return index;
    }
    let Some(current_rank) = effort_rank(current) else { return 0 };
    let mut lower: Option<(usize, usize)> = None;
    let mut lowest: Option<(usize, usize)> = None;
    for (index, effort) in efforts.iter().enumerate() {
        let Some(rank) = effort_rank(effort) else { continue };
        if rank <= current_rank && lower.is_none_or(|(best, _)| rank > best) {
            lower = Some((rank, index));
        }
        if lowest.is_none_or(|(best, _)| rank < best) {
            lowest = Some((rank, index));
        }
    }
    lower.or(lowest).map(|(_, index)| index).unwrap_or(0)
}

pub(crate) fn select_model(menu: &mut ProfileMenu, index: usize, fallback: &[String]) {
    let current = menu.efforts.get(menu.effort_ix).cloned().unwrap_or_default();
    menu.model_ix = index;
    menu.efforts = model_efforts(&menu.models[index], fallback);
    menu.effort_ix = effort_index(&menu.efforts, &current);
}

fn filter_models(menu: &mut ProfileMenu, fallback: &[String]) {
    let selected = menu.models.get(menu.model_ix).map(|model| model.key.clone());
    let query = menu.search.to_lowercase();
    menu.models = menu
        .all_models
        .iter()
        .filter(|model| {
            let fields = format!("{} [{}]", model.label(), model.key).to_lowercase();
            query.split_whitespace().all(|word| fields.contains(word))
        })
        .cloned()
        .collect();
    let index = selected.and_then(|key| menu.models.iter().position(|model| model.key == key)).unwrap_or(0);
    if !menu.models.is_empty() {
        select_model(menu, index, fallback);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalInfo {
    pub id: String,
    pub operation_id: String,
    pub instance_id: String,
    pub tool: String,
    pub preview: String,
    pub args: String,
}

#[derive(Clone, Debug)]
pub struct WorkInfo {
    pub status: String,
    pub known_total: i64,
    pub unknown: bool,
    pub limit_total: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_complete: bool,
    /// The work's absolute deadline in epoch seconds (`[limits] deadline_minutes`,
    /// D-64): shown as the time left, so a configured ceiling is visible.
    pub deadline: Option<f64>,
    /// What the model said it did when it settled the work (D-431). A model can settle a work by calling
    /// `finish` without writing a single word of prose (measured on the tau3 retail split: 12 of 114 dialogues
    /// ended that way, and a user read an empty reply), and the summary it recorded with that call is the only
    /// account of the work there is. The status line shows it, so a TUI user is not left with a bare
    /// `work SUCCEEDED`.
    pub summary: Option<String>,
}

/// Side effects for the main loop to execute through the daemon client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum V2Effect {
    Sessions(crate::sessions::SessionAction),
    Queue(crate::queue::Action),
    InputPaths {
        prefix: String,
    },
    /// Read the one session-level exploration tree.
    SearchTree,
    RewindList {
        before: Option<i64>,
    },
    RewindPreview {
        key: String,
    },
    RewindFiles {
        key: String,
    },
    RewindApply {
        preview: Json,
    },
    RewindDraft,
    RewindSend {
        source: String,
        envelope: String,
        text: String,
    },
    McpStatus,
    McpChange {
        action: String,
        service: String,
    },
    Compact {
        instance: String,
        focus: String,
    },
    /// Open the external editor for this draft (D-454).
    EditDraft {
        initial: String,
    },
    CodeGraph {
        workspace: String,
        action: &'static str,
    },
    OperatorShell {
        workspace: String,
        command: Option<String>,
        draft: bool,
    },
    SubmitInput {
        instance: String,
        envelope: String,
        text: String,
        after_work: bool,
        attachments: Vec<String>,
    },
    Decide {
        approval_id: String,
        decision: &'static str,
    },
    SetLifecycle {
        instance: String,
        lifecycle: &'static str,
        reason: String,
    },
    CancelTask {
        task_id: String,
        reason: String,
    },
    CopyClipboard(String),
    /// Interrupt the Leader at the runtime boundary; leaving the TUI is independent.
    Interrupt {
        instance: String,
    },
    SetRoleProfile {
        role: ProfileRole,
        model: Option<String>,
        effort: Option<String>,
        follow: bool,
    },
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

/// The reference both `?` and `/help` show. One list, so the cheat-sheet and the help text cannot drift apart.
pub const HELP_SECTIONS: &[(&str, &[&str])] = &[
    (
        "Input editing",
        &[
            "Enter send · Shift+Enter newline",
            "Ctrl+Left/Right / Alt+Left/Right move by word",
            "Ctrl+B/F move by character · Home/End line edges",
            "Ctrl+W / Alt+Backspace delete previous word",
            "Ctrl+U/K delete to line start/end · Ctrl+Y restore deletion",
            "Ctrl+E external editor · Up/Down input history",
        ],
    ),
    (
        "Conversation",
        &[
            "Ctrl+K command & goto palette · Ctrl+F find in chat (n/N jump)",
            "PgUp/PgDn / wheel read history · Ctrl+O tool details (or click)",
            "Ctrl+G graphics/source · / commands · Tab complete",
            "Ctrl+C interrupt model request; tools may continue",
            "Ctrl+Q detach; the session keeps running",
        ],
    ),
    (
        "Team workspace",
        &[
            "F6 cycles team, inspection and composer focus",
            "Alt+1 chat · Alt+2 tasks · Alt+3 activity · Alt+4 inspect · Alt+5 tree · Alt+6 wall",
            "Alt+B toggle sidebar · Alt+Z zoom pane · Alt+N next alert",
            "Team: Up/Down or j/k member · Enter inspect · m direct · p pause · r resume · X stop · ! alert",
            "Inspect: d details · h history · P pin · f filter · Space fold tree · x cancel task · z zoom · Esc compose",
            "Direct message (m) sends once to the selected member and returns to Leader",
        ],
    ),
    (
        "Sessions and models",
        &[
            "Ctrl+S sessions · /sessions full stored output",
            "Ctrl+P pending inputs · Ctrl+M model and effort",
            "/attach PATH or @PATH + Tab · /detach INDEX",
        ],
    ),
    (
        "Approvals",
        &["Ctrl+A focus approvals · Up/Down select · Enter detail", "a approve once · d deny · Esc return to draft"],
    ),
    ("More", &["/help session, environment and command list", "Esc close · Up/Down scroll · PgUp/PgDn page"]),
];

/// One line of feedback about an action the user just took. It replaces the previous notice and takes no room
/// from the conversation, because it is status, not content.
#[derive(Clone, Debug, Default)]
pub struct Notice {
    pub text: String,
    pub error: bool,
}

/// Text a command produced for the user to read (`/tree`, `/mcp status`, a file preview). It opens in a
/// dismissible scrollable overlay rather than the transcript, so reading it never costs the model's output its
/// space (D-531).
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub title: String,
    pub text: String,
    pub scroll: usize,
}

pub struct V2App {
    pub session_root: String,
    pub session_name: String,
    pub browser: Option<crate::sessions::SessionBrowser>,
    pub queue: Option<crate::queue::Queue>,
    /// D-473: the daemon's `[models]` catalog and the open profile menu.
    pub models: Vec<ModelInfo>,
    pub leader_catalog_key: String,
    pub worker_policy_mode: String,
    pub worker_catalog_key: String,
    pub efforts: Vec<String>,
    pub profile: Option<ProfileMenu>,
    /// The reference overlay `?` and `/help` open: shortcuts plus live session detail.
    pub help_open: bool,
    pub help_scroll: usize,
    /// A command's readable output, shown in its own dismissible overlay.
    pub report: Option<Report>,
    /// One line of feedback about an action. Replaced by the next notice and cleared by typing.
    pub notice: Option<Notice>,
    pub session_id: String,
    pub instances: Vec<InstanceInfo>,
    pub team: crate::team::Team,
    /// Read-only checkpoint observations, never instructions to the model.
    work_diagnostics: Vec<(String, Json)>,
    environment: Json,
    pub rewind_preview: Option<Json>,
    pending_input_receipts: HashSet<String>,
    pub entries: Vec<ChatEntry>,
    pub transcript: crate::transcript::Transcript,
    pub graphics: crate::graphics::Graphics,
    pub pictures: Vec<crate::graphics::Placement>,
    pub expanded_tools: bool,
    pub work: Option<WorkInfo>,
    pub approvals: Vec<ApprovalInfo>,
    pub approval_sel: usize,
    pub focus: Focus,
    pub composer: Composer,
    pub completion: crate::completion::Completion,
    pub attachments: crate::attachments::Attachments,
    pub disconnected: bool,
    /// A successful socket call does not repair a previously failed conversation refresh.
    pub history_dirty: bool,
    /// Scroll offset in wrapped lines from the bottom (0 = following).
    pub chat_scroll: usize,
    pub watermark: i64,
    pub quit: bool,
    /// Total wrapped chat lines at the last render (scroll clamping).
    pub last_chat_lines: usize,
    pub last_chat_width: usize,
    /// §9: the transient text each instance is streaming, newest read only. Never an authoritative fact.
    pub previews: Vec<(String, String)>,
    /// Visible chat height at the last render (page scrolling).
    pub last_chat_height: usize,
    pub sidebar_hidden: bool,
    pub zoomed: bool,
    pub sidebar_width: u16,
    pub inspector_pct: u16,
    pub drag: Option<DragTarget>,
    pub direct_target: Option<String>,
    pub palette: Option<crate::overlays::Palette>,
    pub rewind_modal: Option<crate::overlays::RewindModal>,
    pub mcp_modal: Option<crate::overlays::McpModal>,
    pub pending_mcp_modal: bool,
    pub approval_detail: Option<crate::overlays::ApprovalDetail>,
    pub confirm: Option<crate::overlays::ConfirmAction>,
    pub chat_search: Option<ChatSearch>,
    pub expanded_entries: HashSet<usize>,
    pub live_sessions: Vec<crate::live_sessions::LiveSession>,
    pub mcp_snapshot: Json,
    pub mcp_policy: Json,
    pub mcp_drivers: Json,
    pub sibling_sessions: Vec<(String, String, String)>,
}

impl V2App {
    pub fn new(session_id: &str) -> V2App {
        V2App {
            session_root: String::new(),
            session_name: String::new(),
            browser: None,
            queue: None,
            models: vec![],
            leader_catalog_key: String::new(),
            worker_policy_mode: "follow".into(),
            worker_catalog_key: String::new(),
            efforts: default_efforts(),
            profile: None,
            help_open: false,
            help_scroll: 0,
            report: None,
            notice: None,
            session_id: session_id.to_string(),
            instances: vec![],
            team: crate::team::Team::default(),
            work_diagnostics: vec![],
            environment: Json::Null,
            rewind_preview: None,
            pending_input_receipts: HashSet::new(),
            entries: vec![],
            transcript: crate::transcript::Transcript::default(),
            graphics: crate::graphics::Graphics::default(),
            pictures: vec![],
            expanded_tools: false,
            work: None,
            approvals: vec![],
            approval_sel: 0,
            focus: Focus::Composer,
            composer: Composer::new(vec![]),
            completion: crate::completion::Completion::default(),
            attachments: crate::attachments::Attachments::default(),
            disconnected: false,
            history_dirty: true,
            chat_scroll: 0,
            watermark: 0,
            quit: false,
            last_chat_lines: 0,
            last_chat_width: 0,
            previews: vec![],
            last_chat_height: 1,
            sidebar_hidden: false,
            zoomed: false,
            sidebar_width: 26,
            inspector_pct: 40,
            drag: None,
            direct_target: None,
            palette: None,
            rewind_modal: None,
            mcp_modal: None,
            pending_mcp_modal: false,
            approval_detail: None,
            confirm: None,
            chat_search: None,
            expanded_entries: HashSet::new(),
            live_sessions: vec![],
            mcp_snapshot: Json::Null,
            mcp_policy: Json::Null,
            mcp_drivers: Json::Null,
            sibling_sessions: vec![],
        }
    }

    pub fn toggle_sidebar(&mut self) {
        self.sidebar_hidden = !self.sidebar_hidden;
        if self.sidebar_hidden && self.focus == Focus::Team {
            self.focus = Focus::Composer;
        }
    }

    pub fn toggle_zoom(&mut self) {
        self.zoomed = !self.zoomed;
    }

    pub fn jump_next_attention(&mut self) {
        if self.team.next_attention().is_some() {
            if self.team.view != crate::team::View::Conversation {
                self.team.show(crate::team::View::Inspect);
            }
            self.focus = Focus::Panel;
        } else if !self.approvals.is_empty() {
            self.focus = Focus::Approvals;
        } else {
            self.flash("No members currently need attention.");
        }
    }

    pub fn active_instance(&self) -> Option<&InstanceInfo> {
        self.instances.iter().find(|instance| instance.id == self.team.leader_id)
    }

    /// Replace the transient streaming previews from the daemon's `previews` read (§9). Empty text is not stored,
    /// so a finished turn leaves no preview behind.
    pub fn apply_previews(&mut self, rows: &Json) {
        let mut previews = Vec::new();
        for row in rows.as_array().cloned().unwrap_or_default() {
            let id = row["instance_id"].as_str().unwrap_or("").to_string();
            let text = row["text"].as_str().unwrap_or("").to_string();
            if !id.is_empty() && !text.is_empty() {
                previews.push((id, text));
            }
        }
        self.previews = previews;
    }

    /// The active instance's streaming text, if it has an attempt in flight.
    pub fn active_preview(&self) -> Option<&str> {
        let id = &self.active_instance()?.id;
        self.previews.iter().find(|(instance, _)| instance == id).map(|(_, text)| text.as_str())
    }

    /// A command's answer, routed by shape rather than dumped in one place. One line is feedback about the
    /// action and becomes the transient notice; several lines are output the user asked to read and open the
    /// scrollable overlay. Neither takes space from the conversation, so asking for help cannot push the model's
    /// answer off screen (D-531). Input receipts are the exception: they use `receipt_note` because they
    /// annotate a message the user sent.
    pub fn command_note(&mut self, text: impl Into<String>) {
        self.team.show(crate::team::View::Conversation);
        if matches!(self.focus, Focus::Team | Focus::Panel) {
            self.focus = Focus::Composer;
        }
        let text = text.into();
        if text.lines().count() <= 1 && unicode_width::UnicodeWidthStr::width(text.as_str()) <= 120 {
            self.flash(text);
        } else {
            self.open_report("Output", text);
        }
    }

    /// What the reference overlay prints below the shortcuts: this session's environment, diagnostics and the
    /// command list. `/help` and `?` share it, so both destinations answer the same question.
    pub fn help_detail(&self) -> String {
        let mut out = vec![format!(
            "Session {} · model {}",
            self.session_id,
            self.active_instance()
                .map(|i| self.member_model_label(&i.id, &i.model))
                .unwrap_or_else(|| "unavailable".into())
        )];
        for (title, hints) in HELP_SECTIONS {
            out.push(String::new());
            out.push((*title).to_string());
            out.extend(hints.iter().map(|hint| format!("  {hint}")));
        }
        out.push(String::new());
        out.push(self.environment_details());
        out.push(self.diagnostic_details());
        out.push(String::new());
        out.push(
            "To stop running commands: teamagents instances terminate --id INSTANCE --yes. \
             Reconnect with the same --state-root/--session."
                .into(),
        );
        out.push("Commands:".into());
        out.extend(crate::completion::COMMANDS.iter().map(|(name, help)| format!("  {name}: {help}")));
        out.join("\n")
    }

    /// One line of feedback about the last action. The next notice replaces it and editing the draft clears it.
    pub fn flash(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), error: false });
    }

    fn flash_error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), error: true });
    }

    /// Read text in a dismissible scrollable overlay instead of the conversation (D-531).
    pub fn open_report(&mut self, title: impl Into<String>, text: impl Into<String>) {
        self.report = Some(Report { title: title.into(), text: text.into(), scroll: 0 });
        self.help_open = false;
        self.pictures.clear();
    }

    /// `?` and `/help` share one reference overlay: shortcuts plus this session's live detail.
    fn open_help(&mut self) {
        self.help_open = true;
        self.help_scroll = 0;
        self.report = None;
        self.pictures.clear();
    }

    /// A receipt for a sent input. It stays in the transcript because it annotates a message the user sent, and
    /// it is updated in place rather than appended.
    fn receipt_note(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry {
            kind: ChatKind::Command,
            who: "note".into(),
            text: text.into(),
            tool: None,
            workspace: String::new(),
            media: vec![],
        });
        if self.entries.len() > NOTE_CAP * 4 {
            let drop = self.entries.len() - NOTE_CAP * 4;
            self.entries.drain(..drop);
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry {
            kind: ChatKind::System,
            who: "system".into(),
            text: text.into(),
            tool: None,
            workspace: String::new(),
            media: vec![],
        });
        if self.entries.len() > NOTE_CAP * 4 {
            let drop = self.entries.len() - NOTE_CAP * 4;
            self.entries.drain(..drop);
        }
    }

    // ---- daemon → app application points --------------------------------

    /// Checkpoint (connect/reconnect): snapshot state and the watermark the
    /// event stream resumes from — one consistent read (§9).
    pub fn apply_checkpoint(&mut self, snapshot: Json, watermark: i64) {
        self.environment = snapshot["environment"].clone();
        self.team.checkpoint(&snapshot);
        for row in snapshot["input_delivery"].as_array().into_iter().flatten() {
            let Some(envelope) = row["id"].as_str() else { continue };
            let state = match row["state"].as_str() {
                Some("APPLIED") => "received into the Leader's context (not a completion verdict)",
                Some("AFTER_WORK") => "accepted; waiting for the current work to settle",
                Some("DEFERRED") => "accepted; waiting for the current turn to end",
                Some("ACCEPTED") => "accepted; queued for the next safe boundary",
                Some("CANCELLED") => "pending delivery cancelled; no model interruption was requested",
                Some("SUPERSEDED") => "undelivered: the instance epoch changed",
                _ => continue,
            };
            self.update_input_receipt(
                envelope,
                state,
                false,
                matches!(row["state"].as_str(), Some("ACCEPTED" | "DEFERRED" | "AFTER_WORK")),
            );
        }
        if self.session_name.is_empty() {
            if let Some(row) = snapshot["sessions"].as_array().into_iter().flatten().find(|row| row["active"] == true) {
                self.session_name = if row["id"] == "default" {
                    "default".into()
                } else {
                    row["name"].as_str().unwrap_or("").to_string()
                };
            }
        }
        self.instances = snapshot["instances"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|i| InstanceInfo {
                id: i["id"].as_str().unwrap_or("").to_string(),
                lifecycle: i["lifecycle"].as_str().unwrap_or("").to_string(),
                phase: i["phase"].as_str().unwrap_or("").to_string(),
                model: i["model"].as_str().unwrap_or("").to_string(),
                reason: i["reason"].as_str().unwrap_or("").to_string(),
                effort: i["effort"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        self.work_diagnostics = snapshot["instances"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(snapshot["tasks"].as_array().into_iter().flatten().filter(|task| task["status"] == "BLOCKED"))
            .filter(|row| row["diagnosis"].is_object())
            .map(|row| (row["id"].as_str().unwrap_or("?").to_string(), row["diagnosis"].clone()))
            .collect();
        self.work = snapshot.get("work").and_then(|g| {
            if g.is_null() {
                return None;
            }
            let known = &g["known_usage"];
            let cost = &g["cost"];
            let has_cost = cost.is_object()
                && (cost["attempts"].as_i64().unwrap_or(0) > 0
                    || cost["known_estimated_usd"].as_f64().unwrap_or(0.0) > 0.0);
            Some(WorkInfo {
                status: g["status"].as_str().unwrap_or("").to_string(),
                known_total: known["total"].as_i64().unwrap_or(0),
                unknown: g["unknown_usage"].as_i64().unwrap_or(0) != 0,
                limit_total: g["limits"]["max_total_tokens"].as_i64(),
                cost_usd: has_cost.then(|| cost["known_estimated_usd"].as_f64()).flatten(),
                cost_complete: has_cost && cost["complete"].as_bool().unwrap_or(false),
                deadline: g["deadline"].as_f64(),
                summary: g["summary"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| text.trim().to_string()),
            })
        });
        let role_models = &snapshot["role_models"];
        self.leader_catalog_key = role_models["leader"].as_str().unwrap_or("").to_string();
        self.worker_policy_mode = role_models["workers"]["mode"].as_str().unwrap_or("follow").to_string();
        self.worker_catalog_key = role_models["workers"]["key"].as_str().unwrap_or("").to_string();
        if let Some(rows) = snapshot["models"].as_array() {
            self.models = rows
                .iter()
                .filter_map(|row| {
                    let key = row["key"].as_str()?.to_string();
                    let efforts = row["efforts"]
                        .as_array()
                        .map(|values| values.iter().filter_map(|e| e.as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    Some(ModelInfo {
                        model: row["model"].as_str().filter(|name| !name.is_empty()).unwrap_or("Unknown").to_string(),
                        provider: row["provider"].as_str().unwrap_or("").to_string(),
                        key,
                        efforts,
                        context_window: row["context_window"].as_i64(),
                    })
                })
                .collect();
            self.models.sort_by(|a, b| (&a.model, &a.provider, &a.key).cmp(&(&b.model, &b.provider, &b.key)));
        }
        self.mcp_snapshot = snapshot["mcp"].clone();
        self.mcp_policy = snapshot["mcp_policy"].clone();
        self.mcp_drivers = snapshot["mcp_drivers"].clone();
        if let Some(modal) = self.mcp_modal.as_mut() {
            modal.refresh_from(&self.mcp_snapshot, &self.mcp_policy, &self.mcp_drivers);
        }
        if let Some(rows) = snapshot["sessions"].as_array() {
            self.sibling_sessions = rows
                .iter()
                .filter(|row| row["active"] != true)
                .filter_map(|row| {
                    let id = row["id"].as_str()?.to_string();
                    let name = row["name"].as_str().unwrap_or(&id).to_string();
                    let root = row["root"].as_str().or_else(|| row["path"].as_str())?.to_string();
                    Some((id, name, root))
                })
                .collect();
        }
        self.watermark = self.watermark.max(watermark);
        // a fresh checkpoint supersedes earlier UI-level notes — runtime chatter, not a command's answer
        // (D-433: `Command` entries are what a user asked for and stay until they scroll away)
        self.entries.retain(|e| e.kind != ChatKind::System && e.kind != ChatKind::Error);
    }

    /// History of the active instance: the authoritative conversation (§9 —
    /// only renders previews). System notes stay layered on top, newest last.
    pub fn apply_history(&mut self, history: Json) {
        if !history["entries"].is_array() {
            self.mark_disconnected("history.entries missing");
            return;
        }
        self.history_dirty = false;
        for row in history["entries"].as_array().into_iter().flatten() {
            if let Some(envelope) = row["envelope_id"].as_str() {
                self.update_input_receipt(
                    envelope,
                    "received into the Leader's context (not a completion verdict)",
                    false,
                    false,
                );
            }
        }
        let mut receipts = HashMap::new();
        let mut receipt_order = Vec::new();
        let mut notes = Vec::new();
        for entry in self.entries.drain(..) {
            if entry.kind == ChatKind::Command && entry.who.starts_with("input:") {
                let id = entry.who[6..].to_string();
                receipt_order.push(id.clone());
                receipts.insert(id, entry);
            } else if matches!(entry.kind, ChatKind::System | ChatKind::Error | ChatKind::Command) {
                notes.push(entry);
            }
        }
        let rebuilt = crate::transcript::project_with_receipts(
            history["entries"].as_array().map(Vec::as_slice).unwrap_or(&[]),
            &self.team.leader_id,
            true,
            &mut receipts,
        );
        let mut pending = Vec::new();
        for id in receipt_order {
            if let Some(receipt) = receipts.remove(&id) {
                if self.pending_input_receipts.contains(&id) {
                    pending.push(receipt);
                } else {
                    self.entries.push(receipt);
                }
            }
        }
        self.entries.extend(rebuilt);
        self.entries.extend(pending);
        self.entries.extend(notes);
        let entry_len = self.entries.len();
        self.expanded_entries.retain(|&idx| idx < entry_len);
        if self.chat_search.is_some() {
            self.update_chat_search();
        }
        if self.chat_scroll == 0 {
            self.last_chat_lines = 0; // follow: force the render to re-clamp
        }
    }

    /// Live events: conversation content stays in history (fetched on
    /// demand); events drive refreshes and UI-level system notes.
    pub fn apply_events(&mut self, events: &[Json]) -> Refresh {
        // Member and task diagnoses are checkpoint projections; any persisted change may invalidate them.
        let mut refresh = Refresh { checkpoint: !events.is_empty(), ..Refresh::default() };
        self.team.events(events);
        for event in events {
            if let Some(sequence) = event["sequence"].as_i64() {
                self.watermark = self.watermark.max(sequence);
            }
            let kind = event["kind"].as_str().unwrap_or("");
            let payload = &event["payload"];
            match kind {
                "input" | "response_imported" | "decision_consumed" | "completion_closed" => {
                    refresh.history = true;
                    refresh.checkpoint = true;
                }
                "request_began"
                | "request_failed"
                | "operation_dispatched"
                | "operation_completed"
                | "request_cancelled"
                    if payload["status"].as_str() != Some("OUTCOME_UNKNOWN") =>
                {
                    refresh.checkpoint = true;
                }
                "work_completed" => {
                    refresh.history = true;
                    refresh.checkpoint = true;
                    // a runtime block announces itself twice (settlement + block
                    // reason); the `work_blocked` note below carries the reason,
                    // so this one stays quiet for it (D-71)
                    if payload["blocked_by"].as_str() != Some("runtime") {
                        self.note(format!("work finished: {}", payload["status"].as_str().unwrap_or("?")));
                    }
                }
                "work_blocked" => {
                    refresh.checkpoint = true;
                    self.note(format!("work parked (BLOCKED): {}", payload["reason"].as_str().unwrap_or("")));
                }
                "check_round_registered" => {
                    // name the checks: a user waiting on a work must see which
                    // contracts are being verified, not just how many
                    let ids: Vec<String> = payload["checks"]
                        .as_array()
                        .map(|checks| {
                            checks.iter().filter_map(|check| check["id"].as_str().map(str::to_string)).collect()
                        })
                        .unwrap_or_default();
                    let what = if ids.is_empty() { "no items".to_string() } else { ids.join(", ") };
                    self.note(format!(
                        "completion check round {} started: {what}",
                        payload["round"].as_i64().unwrap_or(0),
                    ));
                }
                "completion_repair" => {
                    refresh.history = true;
                    // say what failed: the failing check ids and their classes
                    // are the actionable part of a repair round
                    let failures: Vec<String> = payload["failures"]
                        .as_array()
                        .map(|failures| {
                            failures
                                .iter()
                                .map(|failure| {
                                    format!(
                                        "{} ({})",
                                        failure["check_id"].as_str().unwrap_or("?"),
                                        failure["class"].as_str().unwrap_or("failed")
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let why = if failures.is_empty() { String::new() } else { format!(": {}", failures.join(", ")) };
                    self.note(format!(
                        "completion check round {} failed{why}, entering a repair turn",
                        payload["round"].as_i64().unwrap_or(0),
                    ));
                }
                "instance_spawned" => {
                    refresh.checkpoint = true;
                    self.note(format!(
                        "instance {} spawned by {}",
                        event["scope"].as_str().unwrap_or("?"),
                        payload["spawner"].as_str().unwrap_or("?")
                    ));
                }
                "instance_lifecycle" => {
                    refresh.checkpoint = true;
                    // the reason rides along (D-164): a park is the runtime saying why nobody drives the
                    // instance (a required MCP service that cannot start, a driver that failed to boot), and
                    // the note was the only place a user could have seen it without reading `daemon.log`
                    let reason = payload["reason"].as_str().unwrap_or("");
                    self.note(match reason.is_empty() {
                        true => format!(
                            "instance {} lifecycle -> {}",
                            event["scope"].as_str().unwrap_or("?"),
                            payload["lifecycle"].as_str().unwrap_or("?")
                        ),
                        false => format!(
                            "instance {} lifecycle -> {}: {}",
                            event["scope"].as_str().unwrap_or("?"),
                            payload["lifecycle"].as_str().unwrap_or("?"),
                            reason
                        ),
                    });
                }
                "conversation_rewound" | "instance_reset" => {
                    refresh.checkpoint = true;
                    refresh.history = true;
                    self.rewind_preview = None;
                }
                "instance_created" => {
                    refresh.checkpoint = true;
                    self.note(format!("instance {} created", event["scope"].as_str().unwrap_or("?")));
                }
                "input_queued"
                | "input_cancelled"
                | "input_edited"
                | "task_delegated"
                | "task_started"
                | "task_completed"
                | "wait_satisfied"
                | "inbox_drained"
                | "capability_requested"
                | "capability_approved"
                | "capability_denied" => {
                    refresh.checkpoint = true;
                }
                "task_blocked" => {
                    refresh.checkpoint = true;
                    self.note(format!(
                        "task {} parked (BLOCKED): {}",
                        payload["task_id"].as_str().unwrap_or("?"),
                        payload["reason"].as_str().unwrap_or("")
                    ));
                }
                "task_cancelled" => {
                    refresh.checkpoint = true;
                    self.note(format!(
                        "task {} cancelled: {}",
                        payload["task_id"].as_str().unwrap_or("?"),
                        payload["reason"].as_str().unwrap_or("")
                    ));
                }
                "approval_requested" | "approval_granted" | "approval_denied" => {
                    refresh.approvals = true;
                    refresh.checkpoint = true;
                }
                "operation_completed" if payload["status"].as_str() == Some("OUTCOME_UNKNOWN") => {
                    refresh.checkpoint = true;
                    self.note(format!(
                        "operation {} has an unknown outcome (not replayed)",
                        event["scope"].as_str().unwrap_or("?")
                    ));
                }
                _ => {}
            }
        }
        self.history_dirty |= refresh.history;
        refresh
    }

    pub fn apply_approvals(&mut self, result: Json) {
        let selected = self.approvals.get(self.approval_sel).map(|approval| approval.id.clone());
        self.approvals = result["approvals"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|a| ApprovalInfo {
                id: a["id"].as_str().unwrap_or("").to_string(),
                operation_id: a["operation_id"].as_str().unwrap_or("").to_string(),
                instance_id: a["instance_id"].as_str().unwrap_or("").to_string(),
                tool: a["tool"].as_str().unwrap_or("").to_string(),
                preview: a["preview"].as_str().unwrap_or("").to_string(),
                args: a["args"].as_str().unwrap_or("").to_string(),
            })
            .collect();
        let retained = selected.as_ref().and_then(|id| self.approvals.iter().position(|approval| &approval.id == id));
        self.approval_sel = retained.unwrap_or(0);
        if let Some(detail) = &self.approval_detail {
            if !self.approvals.iter().any(|a| a.id == detail.approval_id) {
                self.approval_detail = None;
            }
        }
        if self.focus == Focus::Approvals && (self.approvals.is_empty() || (selected.is_some() && retained.is_none())) {
            self.focus = Focus::Composer;
        }
    }

    pub fn command_failed(&mut self, error: &str) {
        self.flash_error(format!("command failed: {error}"));
    }

    /// Settle an external-editor attempt. The `/edit TEXT` path takes the
    /// draft out of the composer before the effect is returned, so a cancel or
    /// editor failure must put it back; Ctrl+E supplies the same initial and is
    /// idempotent here.
    pub fn finish_editor(&mut self, initial: &str, result: Result<String, String>) {
        match result {
            Ok(text) => self.composer.set_text(&text),
            Err(error) => {
                self.composer.set_text(initial);
                self.command_failed(&error);
            }
        }
    }

    pub fn open_rewind_draft(&mut self, reply: &Json) {
        let (Some(source), Some(text)) = (reply["envelope"].as_str(), reply["draft"]["text"].as_str()) else {
            self.command_note("No current rewind draft.");
            return;
        };
        self.attachments.accepted();
        self.attachments.rewind_source = Some(source.to_owned());
        self.composer.set_text(text);
        self.flash(format!(
            "Rewind draft reopened ({} attachment(s), {} check(s)) · Enter sends · Esc discards",
            reply["draft"]["attachments"], reply["draft"]["checks"]
        ));
    }

    pub fn mark_disconnected(&mut self, error: &str) {
        self.history_dirty = true;
        if !self.disconnected {
            self.disconnected = true;
            self.flash_error(format!("Disconnected ({error}) · reconnecting"));
        }
    }

    pub fn mark_connected(&mut self) {
        if self.disconnected {
            self.disconnected = false;
            self.note("reconnected to the daemon");
        }
    }

    /// A submitted input the daemon refused (write-surface error).
    pub fn submit_failed(&mut self, error: &str) {
        self.flash_error(format!("send failed: {error}"));
    }

    /// D-479: retain the acknowledged envelope through refresh/reconnect. History or a checkpoint,
    /// never a timer, changes 'queued' into 'received'. Retry identity remains owned by Attachments.
    pub fn input_receipt(&mut self, envelope: &str, reply: &Json) {
        let state = if reply["after"] == "work" {
            "accepted; waiting for the current work to settle"
        } else if reply["deferred"] == true {
            "accepted; waiting for the current turn to end"
        } else if reply["queued"] == true {
            "accepted; queued for the next safe boundary (Ctrl+P inspects pending inputs)"
        } else if reply["applied"] == true {
            "received into the Leader's context (not a completion verdict)"
        } else {
            "accepted by the daemon; delivery not yet confirmed"
        };
        self.update_input_receipt(envelope, state, true, reply["applied"] != true);
    }

    fn update_input_receipt(&mut self, envelope: &str, state: &str, create: bool, pending: bool) {
        let who = format!("input:{envelope}");
        let text = format!("Input {envelope}: {state}.");
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.kind == ChatKind::Command && entry.who == who) {
            entry.text = text;
        } else if create {
            self.receipt_note(text);
            if let Some(entry) = self.entries.last_mut() {
                entry.who = who;
            }
        }
        if pending {
            self.pending_input_receipts.insert(envelope.to_string());
        } else {
            self.pending_input_receipts.remove(envelope);
        }
        self.pending_input_receipts.retain(|id| self.entries.iter().any(|entry| entry.who == format!("input:{id}")));
    }

    pub fn decide_failed(&mut self, error: &str) {
        self.flash_error(format!("the approval decision failed: {error}"));
    }

    // ---- keys ------------------------------------------------------------

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        let modal_open = self.confirm.is_some()
            || self.approval_detail.is_some()
            || self.rewind_modal.is_some()
            || self.mcp_modal.is_some();
        if key.kind == crossterm::event::KeyEventKind::Release
            || (key.kind == crossterm::event::KeyEventKind::Repeat
                && (matches!(key.code, KeyCode::Enter | KeyCode::Esc | KeyCode::F(6))
                    || (key.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(
                            key.code,
                            KeyCode::Char('a' | 'c' | 'e' | 'f' | 'g' | 'k' | 'm' | 'o' | 'p' | 'q' | 's')
                        ))
                    || (key.modifiers == KeyModifiers::ALT
                        && matches!(key.code, KeyCode::Char('1'..='6' | 'b' | 'z' | 'n')))
                    || ((self.focus != Focus::Composer || modal_open)
                        && self.palette.is_none()
                        && !self.chat_search.as_ref().is_some_and(|s| s.editing)
                        && matches!(
                            key.code,
                            KeyCode::Char(
                                ' ' | 'a'
                                    | 'c'
                                    | 'd'
                                    | 'f'
                                    | 'h'
                                    | 'm'
                                    | 'n'
                                    | 'p'
                                    | 'r'
                                    | 's'
                                    | 'x'
                                    | 'y'
                                    | 'N'
                                    | 'P'
                                    | 'X'
                            )
                        ))))
        {
            return None;
        }
        if key.code == KeyCode::Esc && !key.modifiers.is_empty() {
            return None;
        }
        if let Some(confirm) = self.confirm.clone() {
            if key.modifiers == KeyModifiers::CONTROL {
                match key.code {
                    KeyCode::Char('q') => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    KeyCode::Char('c') => return self.interrupt(),
                    _ => {}
                }
            }
            match confirm.key(key) {
                crate::overlays::ModalKey::Close => self.confirm = None,
                crate::overlays::ModalKey::Submit(action) => {
                    self.confirm = None;
                    return self.apply_confirm_action(action);
                }
                crate::overlays::ModalKey::Consumed | crate::overlays::ModalKey::Ignored => {}
            }
            return None;
        }
        if let Some(detail) = self.approval_detail.as_mut() {
            if key.modifiers == KeyModifiers::CONTROL {
                match key.code {
                    KeyCode::Char('q') => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    KeyCode::Char('c') => return self.interrupt(),
                    _ => {}
                }
            }
            match detail.key(key) {
                crate::overlays::ModalKey::Close => self.approval_detail = None,
                crate::overlays::ModalKey::Submit(decision) => {
                    self.approval_detail = None;
                    return self.apply_approval_decision(decision);
                }
                crate::overlays::ModalKey::Consumed | crate::overlays::ModalKey::Ignored => {}
            }
            return None;
        }
        if self.report.is_some() {
            let step: Option<isize> = match key.code {
                KeyCode::Esc => {
                    self.report = None;
                    return None;
                }
                KeyCode::Up => Some(-1),
                KeyCode::Down => Some(1),
                KeyCode::PageUp => Some(-8),
                KeyCode::PageDown => Some(8),
                _ => None,
            };
            if let (Some(step), Some(report)) = (step, self.report.as_mut()) {
                report.scroll = if step < 0 {
                    report.scroll.saturating_sub(step.unsigned_abs())
                } else {
                    report.scroll.saturating_add(step as usize)
                };
            }
            match (key.code, key.modifiers) {
                (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                    self.quit = true;
                    return Some(V2Effect::Quit);
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => return self.interrupt(),
                _ => {}
            }
            if step.is_some() || key.code == KeyCode::Esc {
                return None;
            }
            // Light dismissal (D-531): anything else closes the overlay and then acts, so reading output never
            // costs the next keystroke.
            self.report = None;
        }
        if self.help_open {
            if key.kind == crossterm::event::KeyEventKind::Repeat && key.code == KeyCode::Char('?') {
                return None;
            }
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('?'), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                    self.help_open = false;
                }
                (KeyCode::Up, _) => self.help_scroll = self.help_scroll.saturating_sub(1),
                (KeyCode::Down, _) => self.help_scroll = self.help_scroll.saturating_add(1),
                (KeyCode::PageUp, _) => self.help_scroll = self.help_scroll.saturating_sub(8),
                (KeyCode::PageDown, _) => self.help_scroll = self.help_scroll.saturating_add(8),
                (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                    self.quit = true;
                    return Some(V2Effect::Quit);
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => return self.interrupt(),
                (KeyCode::Char('g'), KeyModifiers::CONTROL) => self.graphics.toggle(),
                _ => {}
            }
            return None;
        }
        if self.profile.is_some() {
            if key.modifiers == KeyModifiers::CONTROL {
                match key.code {
                    KeyCode::Char('q') => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    KeyCode::Char('c') => return self.interrupt(),
                    KeyCode::Char('g') => {
                        self.graphics.toggle();
                        return None;
                    }
                    KeyCode::Char('m') | KeyCode::Enter => {
                        self.profile = None;
                        return None;
                    }
                    _ => {}
                }
            }
            return self.profile_key(key);
        }
        if self.queue.is_some()
            && !(key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q' | 'g')))
        {
            let queue = self.queue.as_mut()?;
            if key.code == KeyCode::Esc {
                if queue.escape() {
                    self.queue = None;
                }
                return None;
            }
            return queue.key(key).map(V2Effect::Queue);
        }
        if self.browser.is_some()
            && !(key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'q' | 'g')))
        {
            let browser = self.browser.as_mut()?;
            if key.code == KeyCode::Esc && !browser.busy && browser.confirmation.is_none() && !browser.search_active() {
                self.browser = None;
                return None;
            }
            return browser.key(key).map(V2Effect::Sessions);
        }
        if let Some(palette) = self.palette.as_mut() {
            if key.modifiers == KeyModifiers::CONTROL {
                match key.code {
                    KeyCode::Char('q') => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    KeyCode::Char('c') => return self.interrupt(),
                    KeyCode::Char('k') => {
                        self.palette = None;
                        return None;
                    }
                    _ => {}
                }
            }
            match palette.key(key) {
                crate::overlays::ModalKey::Close => self.palette = None,
                crate::overlays::ModalKey::Submit(action) => {
                    self.palette = None;
                    return self.apply_palette_action(action);
                }
                crate::overlays::ModalKey::Consumed | crate::overlays::ModalKey::Ignored => {}
            }
            return None;
        }
        if let Some(rewind) = self.rewind_modal.as_mut() {
            if key.modifiers == KeyModifiers::CONTROL {
                match key.code {
                    KeyCode::Char('q') => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    KeyCode::Char('c') => return self.interrupt(),
                    _ => {}
                }
            }
            match rewind.key(key) {
                crate::overlays::ModalKey::Close => self.rewind_modal = None,
                crate::overlays::ModalKey::Submit(action) => return self.apply_rewind_action(action),
                crate::overlays::ModalKey::Consumed | crate::overlays::ModalKey::Ignored => {}
            }
            return None;
        }
        if let Some(mcp) = self.mcp_modal.as_mut() {
            if key.modifiers == KeyModifiers::CONTROL {
                match key.code {
                    KeyCode::Char('q') => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    KeyCode::Char('c') => return self.interrupt(),
                    _ => {}
                }
            }
            match mcp.key(key) {
                crate::overlays::ModalKey::Close => self.mcp_modal = None,
                crate::overlays::ModalKey::Submit(action) => return self.apply_mcp_action(action),
                crate::overlays::ModalKey::Consumed | crate::overlays::ModalKey::Ignored => {}
            }
            return None;
        }
        if self.team.view == crate::team::View::Conversation {
            if self.chat_search.as_ref().is_some_and(|s| s.editing) {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                        self.quit = true;
                        return Some(V2Effect::Quit);
                    }
                    (KeyCode::Char('c'), KeyModifiers::CONTROL) => return self.interrupt(),
                    (KeyCode::Esc, _) => {
                        self.chat_search = None;
                        return None;
                    }
                    (KeyCode::Enter, _) => {
                        if let Some(search) = self.chat_search.as_mut() {
                            if search.query.is_empty() {
                                self.chat_search = None;
                            } else {
                                search.editing = false;
                                self.scroll_chat_to_search_match();
                            }
                        }
                        return None;
                    }
                    (KeyCode::Up, _) => {
                        self.jump_chat_search(false);
                        return None;
                    }
                    (KeyCode::Down, _) => {
                        self.jump_chat_search(true);
                        return None;
                    }
                    (KeyCode::Backspace, _) => {
                        if let Some(search) = self.chat_search.as_mut() {
                            search.query.pop();
                        }
                        self.update_chat_search();
                        self.scroll_chat_to_search_match();
                        return None;
                    }
                    (KeyCode::Char(c), m) if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                        if let Some(search) = self.chat_search.as_mut() {
                            search.query.push(c);
                        }
                        self.update_chat_search();
                        self.scroll_chat_to_search_match();
                        return None;
                    }
                    _ => return None,
                }
            } else if self.chat_search.is_some() {
                let empty_draft = self.focus != Focus::Composer || self.composer.text().is_empty();
                match key.code {
                    KeyCode::Esc
                        if key.modifiers.is_empty()
                            && self.attachments.rewind_source.is_none()
                            && self.direct_target.is_none() =>
                    {
                        self.chat_search = None;
                        return None;
                    }
                    KeyCode::Char('n') if key.modifiers.is_empty() && empty_draft => {
                        self.jump_chat_search(true);
                        return None;
                    }
                    KeyCode::Char('N')
                        if (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) && empty_draft =>
                    {
                        self.jump_chat_search(false);
                        return None;
                    }
                    _ => {}
                }
            }
        }
        if key.modifiers == KeyModifiers::CONTROL {
            match key.code {
                KeyCode::Char('k') => {
                    self.palette = Some(crate::overlays::Palette::open(self));
                    return None;
                }
                KeyCode::Char('f') if self.team.view == crate::team::View::Conversation => {
                    let mut search = self.chat_search.take().unwrap_or_default();
                    search.editing = true;
                    self.chat_search = Some(search);
                    self.update_chat_search();
                    return None;
                }
                KeyCode::Char('s') => return self.open_sessions(),
                KeyCode::Char('p') if self.focus == Focus::Composer => return self.open_queue(),
                KeyCode::Char('m') if self.focus == Focus::Composer => return self.open_profile(),
                KeyCode::Enter if self.focus == Focus::Composer => return self.open_profile(),
                KeyCode::Char('q') => {
                    self.quit = true;
                    return Some(V2Effect::Quit);
                }
                KeyCode::Char('g') => {
                    self.graphics.toggle();
                    return None;
                }
                KeyCode::Char('o') => {
                    self.expanded_tools = !self.expanded_tools;
                    return None;
                }
                KeyCode::Char('c') => return self.interrupt(),
                KeyCode::Char('a') => {
                    if !self.approvals.is_empty() {
                        self.focus = Focus::Approvals;
                    }
                    return None;
                }
                _ => {}
            }
        }
        if key.code == KeyCode::F(6) && key.modifiers.is_empty() {
            self.focus = match self.focus {
                Focus::Composer | Focus::Approvals => Focus::Team,
                Focus::Team => {
                    self.team.show(crate::team::View::Inspect);
                    Focus::Panel
                }
                Focus::Panel => Focus::Composer,
            };
            return None;
        }
        if key.modifiers == KeyModifiers::ALT {
            match key.code {
                KeyCode::Char(number @ '1'..='6') => {
                    let view = crate::team::VIEWS[number as usize - '1' as usize].0;
                    self.team.show(view);
                    self.focus = if number == '1' { Focus::Composer } else { Focus::Panel };
                    if view == crate::team::View::Tree {
                        self.team.tree_dirty = true;
                        if self.team.tree.is_none() && !self.disconnected {
                            return Some(V2Effect::SearchTree);
                        }
                    }
                    return None;
                }
                KeyCode::Char('b') => {
                    self.toggle_sidebar();
                    return None;
                }
                KeyCode::Char('z') => {
                    self.toggle_zoom();
                    return None;
                }
                KeyCode::Char('n') => {
                    self.jump_next_attention();
                    return None;
                }
                _ => {}
            }
        }
        match self.focus {
            Focus::Team | Focus::Panel => {
                crate::team::key(self, key);
                None
            }
            Focus::Approvals => self.approval_key(key),
            Focus::Composer => self.composer_key(key),
        }
    }

    pub fn active_context_window(&self) -> Option<i64> {
        let key = if !self.leader_catalog_key.is_empty() {
            self.leader_catalog_key.as_str()
        } else {
            self.active_instance().map(|i| i.model.as_str()).unwrap_or("")
        };
        self.models.iter().find(|m| m.key == key || m.model == key).and_then(|m| m.context_window)
    }

    pub fn open_mcp_modal(&mut self) {
        self.mcp_modal =
            Some(crate::overlays::McpModal::from_snapshot(&self.mcp_snapshot, &self.mcp_policy, &self.mcp_drivers));
    }

    pub fn apply_palette_action(&mut self, action: crate::overlays::PaletteAction) -> Option<V2Effect> {
        use crate::overlays::PaletteAction;
        match action {
            PaletteAction::SelectMember(id) => {
                self.team.select(&id);
                self.team.show(crate::team::View::Inspect);
                self.focus = Focus::Panel;
                None
            }
            PaletteAction::OpenView(view) => {
                self.team.show(view);
                self.focus = if view == crate::team::View::Conversation { Focus::Composer } else { Focus::Panel };
                if view == crate::team::View::Tree {
                    self.team.tree_dirty = true;
                    if !self.disconnected && self.team.tree.is_none() {
                        return Some(V2Effect::SearchTree);
                    }
                }
                None
            }
            PaletteAction::SelectTask(idx) => {
                self.team.selected_task = idx;
                self.team.show(crate::team::View::Tasks);
                self.focus = Focus::Panel;
                None
            }
            PaletteAction::FocusTreeNode(prefix) => {
                let current = self.composer.text();
                let next = if current.trim().is_empty() { prefix } else { format!("{prefix}{current}") };
                self.composer.set_text(&next);
                self.team.show(crate::team::View::Conversation);
                self.focus = Focus::Composer;
                None
            }
            PaletteAction::OpenSession(row) => Some(V2Effect::Sessions(crate::sessions::SessionAction::Open(row))),
            PaletteAction::OpenRewind => {
                if self.disconnected {
                    self.command_note("Reconnect before inspecting or changing the conversation.");
                    return None;
                }
                if !self.attachments.paths.is_empty() {
                    self.command_note("Remove staged files before inspecting rewind points.");
                    return None;
                }
                self.rewind_modal = Some(crate::overlays::RewindModal::loading());
                Some(V2Effect::RewindList { before: None })
            }
            PaletteAction::OpenMcp => {
                if self.disconnected {
                    self.command_note("Disconnected: no MCP request sent.");
                    return None;
                }
                self.open_mcp_modal();
                Some(V2Effect::McpStatus)
            }
            PaletteAction::OpenModel => self.open_profile(),
            PaletteAction::OpenQueue => self.open_queue(),
            PaletteAction::OpenSessions => self.open_sessions(),
            PaletteAction::OpenHelp => {
                self.open_help();
                None
            }
            PaletteAction::ToggleZoom => {
                self.toggle_zoom();
                None
            }
            PaletteAction::ToggleSidebar => {
                self.toggle_sidebar();
                None
            }
            PaletteAction::CopyLastCode => {
                if let Some((_, code)) = self.transcript.code_blocks.last() {
                    Some(V2Effect::CopyClipboard(code.clone()))
                } else {
                    self.flash("No code block in visible conversation.");
                    None
                }
            }
            PaletteAction::Compact => self.dispatch_command(crate::completion::Command::Compact, None),
            PaletteAction::Interrupt => self.interrupt(),
        }
    }

    pub fn apply_rewind_action(&mut self, action: crate::overlays::RewindAction) -> Option<V2Effect> {
        match action {
            crate::overlays::RewindAction::LoadMore(before) => Some(V2Effect::RewindList { before }),
            crate::overlays::RewindAction::Preview { key, files } => {
                if files {
                    Some(V2Effect::RewindFiles { key })
                } else {
                    Some(V2Effect::RewindPreview { key })
                }
            }
            crate::overlays::RewindAction::Apply(preview) => Some(V2Effect::RewindApply { preview }),
        }
    }

    pub fn apply_mcp_action(&mut self, action: crate::overlays::McpModalAction) -> Option<V2Effect> {
        match action {
            crate::overlays::McpModalAction::Refresh => Some(V2Effect::McpStatus),
            crate::overlays::McpModalAction::Change { action, service } => {
                Some(V2Effect::McpChange { action, service })
            }
        }
    }

    pub fn apply_confirm_action(&mut self, action: crate::overlays::ConfirmAction) -> Option<V2Effect> {
        if self.disconnected {
            self.command_note("Disconnected: no control command sent.");
            return None;
        }
        match action {
            crate::overlays::ConfirmAction::PauseMember(instance) => Some(V2Effect::SetLifecycle {
                instance,
                lifecycle: "PAUSED",
                reason: "PAUSED by the user (TUI)".into(),
            }),
            crate::overlays::ConfirmAction::ResumeMember(instance) => Some(V2Effect::SetLifecycle {
                instance,
                lifecycle: "ACTIVE",
                reason: "ACTIVE by the user (TUI)".into(),
            }),
            crate::overlays::ConfirmAction::TerminateMember(instance) => Some(V2Effect::SetLifecycle {
                instance,
                lifecycle: "TERMINATED",
                reason: "TERMINATED by the user (TUI)".into(),
            }),
            crate::overlays::ConfirmAction::CancelTask(task_id) => {
                Some(V2Effect::CancelTask { task_id, reason: "cancelled by the user (TUI)".into() })
            }
        }
    }

    pub fn apply_approval_decision(&mut self, decision: crate::overlays::ApprovalDecision) -> Option<V2Effect> {
        match decision {
            crate::overlays::ApprovalDecision::Approve(approval_id) => {
                Some(V2Effect::Decide { approval_id, decision: "approve" })
            }
            crate::overlays::ApprovalDecision::Deny(approval_id) => {
                Some(V2Effect::Decide { approval_id, decision: "deny" })
            }
        }
    }

    pub fn update_chat_search(&mut self) {
        let Some(search) = self.chat_search.as_mut() else { return };
        let q = search.query.trim().to_lowercase();
        search.matches.clear();
        if !q.is_empty() {
            for (idx, entry) in self.entries.iter().enumerate() {
                let mut hay = format!("{} {}", entry.who, entry.text).to_lowercase();
                if let Some(tool) = &entry.tool {
                    hay.push(' ');
                    hay.push_str(&tool.name.to_lowercase());
                    hay.push(' ');
                    hay.push_str(&tool.arguments.to_lowercase());
                    if let Some(out) = &tool.output {
                        hay.push(' ');
                        hay.push_str(&out.to_lowercase());
                    }
                }
                if hay.contains(&q) {
                    search.matches.push(idx);
                }
            }
        }
        if search.matches.is_empty() {
            search.current = 0;
        } else {
            search.current = search.current.min(search.matches.len() - 1);
        }
    }

    pub fn jump_chat_search(&mut self, next: bool) {
        if let Some(search) = self.chat_search.as_mut() {
            if search.matches.is_empty() {
                return;
            }
            if next {
                search.current = (search.current + 1) % search.matches.len();
            } else if search.current == 0 {
                search.current = search.matches.len() - 1;
            } else {
                search.current -= 1;
            }
        }
        self.scroll_chat_to_search_match();
    }

    fn scroll_chat_to_search_match(&mut self) {
        let Some(entry_idx) = self.chat_search.as_ref().and_then(|s| s.matches.get(s.current).copied()) else {
            return;
        };
        if let Some((_, range)) = self.transcript.entry_ranges.iter().find(|(ix, _)| *ix == entry_idx) {
            let max_scroll = self.last_chat_lines.saturating_sub(self.last_chat_height);
            let target_start = range.start.saturating_sub(self.last_chat_height / 3);
            self.chat_scroll = max_scroll.saturating_sub(target_start).min(max_scroll);
        }
    }

    fn open_queue(&mut self) -> Option<V2Effect> {
        self.queue = Some(crate::queue::Queue::default());
        Some(V2Effect::Queue(crate::queue::Action::List { after: 0 }))
    }

    fn open_sessions(&mut self) -> Option<V2Effect> {
        self.browser = Some(crate::sessions::SessionBrowser::default());
        Some(V2Effect::Sessions(crate::sessions::SessionAction::List))
    }

    /// Resolve display text by the exact selection key, including for same-wire accounts.
    pub fn model_label(&self, key: &str) -> String {
        self.models.iter().find(|m| m.key == key).map(ModelInfo::label).unwrap_or_else(|| "Unknown".into())
    }

    pub(crate) fn member_model_label(&self, id: &str, model: &str) -> String {
        if id == self.team.leader_id && !self.leader_catalog_key.is_empty() {
            return self.model_label(&self.leader_catalog_key);
        }
        // Older snapshots can identify a member by wire name alone.
        if self.models.iter().any(|m| m.model == model) && !self.models.iter().any(|m| m.key == model) {
            return model.to_owned();
        }
        self.model_label(model)
    }

    pub fn paste_model_search(&mut self, text: &str) {
        if let Some(menu) = &mut self.profile {
            menu.search
                .extend(text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control()));
            filter_models(menu, &self.efforts);
        }
    }

    /// Edit the role selection locally; only a confirmed catalogue row becomes a command.
    pub(crate) fn open_profile(&mut self) -> Option<V2Effect> {
        let Some(leader) = self.active_instance().cloned() else {
            self.command_note("No Leader is connected, so there is no profile to change.");
            return None;
        };
        let models = self.models.clone();
        let current = if self.leader_catalog_key.is_empty() { &leader.model } else { &self.leader_catalog_key };
        let model_ix = models.iter().position(|model| &model.key == current).unwrap_or(0);
        let efforts = models.get(model_ix).map(|m| model_efforts(m, &self.efforts)).unwrap_or_default();
        let effort_ix = effort_index(&efforts, &leader.effort);
        self.profile = Some(ProfileMenu {
            role: ProfileRole::Leader,
            instance: leader.id,
            all_models: models.clone(),
            models,
            search: String::new(),
            efforts,
            model_ix,
            effort_ix,
            workers_mode: WorkersMode::Pinned,
        });
        None
    }

    fn profile_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('u') {
            let menu = self.profile.as_mut()?;
            menu.search.clear();
            filter_models(menu, &self.efforts);
            return None;
        }
        if !(key.modifiers.is_empty() || (key.modifiers == KeyModifiers::SHIFT && matches!(key.code, KeyCode::Char(_))))
        {
            return None;
        }
        let fallback = self.efforts.clone();
        let menu = self.profile.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.profile = None;
                None
            }
            KeyCode::Up => {
                if menu.models.is_empty() {
                    return None;
                }
                let index = (menu.model_ix + menu.models.len() - 1) % menu.models.len();
                select_model(menu, index, &fallback);
                None
            }
            KeyCode::Down => {
                if menu.models.is_empty() {
                    return None;
                }
                let index = (menu.model_ix + 1) % menu.models.len();
                select_model(menu, index, &fallback);
                None
            }
            KeyCode::Tab if key.kind == crossterm::event::KeyEventKind::Press => {
                menu.search.clear();
                menu.models.clone_from(&menu.all_models);
                menu.role = match menu.role {
                    ProfileRole::Leader => ProfileRole::Workers,
                    ProfileRole::Workers => ProfileRole::Leader,
                };
                menu.workers_mode = match self.worker_policy_mode.as_str() {
                    "follow" => WorkersMode::Follow,
                    "pinned" => WorkersMode::Pinned,
                    _ => WorkersMode::Unresolved,
                };
                let key = if menu.role == ProfileRole::Workers && menu.workers_mode == WorkersMode::Pinned {
                    &self.worker_catalog_key
                } else {
                    &self.leader_catalog_key
                };
                if !menu.models.is_empty() {
                    let index = menu.models.iter().position(|m| &m.key == key).unwrap_or(0);
                    select_model(menu, index, &fallback);
                }
                if menu.role == ProfileRole::Leader {
                    let effort =
                        self.instances.iter().find(|i| i.id == menu.instance).map(|i| i.effort.as_str()).unwrap_or("");
                    menu.effort_ix = effort_index(&menu.efforts, effort);
                }
                None
            }
            KeyCode::Left | KeyCode::Right if menu.role == ProfileRole::Workers => {
                menu.workers_mode = if key.code == KeyCode::Left { WorkersMode::Follow } else { WorkersMode::Pinned };
                None
            }
            KeyCode::Left | KeyCode::Right if !menu.efforts.is_empty() => {
                let len = menu.efforts.len();
                menu.effort_ix = (menu.effort_ix + if key.code == KeyCode::Left { len - 1 } else { 1 }) % len;
                None
            }
            KeyCode::Char(c) if !c.is_control() => {
                menu.search.push(c);
                filter_models(menu, &fallback);
                None
            }
            KeyCode::Backspace => {
                menu.search.pop();
                filter_models(menu, &fallback);
                None
            }
            KeyCode::Enter => {
                let model = menu.models.get(menu.model_ix)?.key.clone();
                let effect = match menu.role {
                    ProfileRole::Leader => V2Effect::SetRoleProfile {
                        role: menu.role,
                        model: Some(model),
                        effort: Some(menu.efforts.get(menu.effort_ix)?.clone()),
                        follow: false,
                    },
                    ProfileRole::Workers => match menu.workers_mode {
                        WorkersMode::Unresolved => return None,
                        WorkersMode::Follow if self.leader_catalog_key.is_empty() => return None,
                        WorkersMode::Follow => {
                            V2Effect::SetRoleProfile { role: menu.role, model: None, effort: None, follow: true }
                        }
                        WorkersMode::Pinned => V2Effect::SetRoleProfile {
                            role: menu.role,
                            model: Some(model),
                            effort: None,
                            follow: false,
                        },
                    },
                };
                self.profile = None;
                Some(effect)
            }
            _ => None,
        }
    }

    fn interrupt(&mut self) -> Option<V2Effect> {
        if self.disconnected {
            self.command_note("Disconnected: interrupt was not sent. Reconnect before retrying.");
            return None;
        }
        let instance = self.active_instance()?.id.clone();
        Some(V2Effect::Interrupt { instance })
    }

    fn approval_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyEventKind};
        if !key.modifiers.is_empty()
            || (key.kind == KeyEventKind::Repeat && matches!(key.code, KeyCode::Char('a' | 'd' | ' ') | KeyCode::Enter))
        {
            return None;
        }
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
            KeyCode::Enter | KeyCode::Char(' ') => {
                if let Some(approval) = self.approvals.get(self.approval_sel) {
                    self.approval_detail =
                        Some(crate::overlays::ApprovalDetail { approval_id: approval.id.clone(), scroll: 0 });
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

    /// Client commands never become model input, including retired management commands.
    fn slash_command(&mut self, text: &str) -> Option<V2Effect> {
        let Some((command, argument)) = crate::completion::Command::parse(text) else {
            self.flash(format!("Unknown command {text:?} · / lists commands · management: teamagents --help"));
            return None;
        };
        self.dispatch_command(command, argument)
    }

    fn dispatch_command(&mut self, command: crate::completion::Command, argument: Option<&str>) -> Option<V2Effect> {
        use crate::completion::Command;
        if !matches!(
            command,
            Command::Attach
                | Command::Detach
                | Command::Edit
                | Command::Compact
                | Command::Later
                | Command::Shell
                | Command::CodeGraph
                | Command::Mcp
                | Command::Rewind
        ) && argument.is_some_and(|text| !text.trim().is_empty())
        {
            self.command_note(
                "This command takes no arguments. Use /help for controls; management: teamagents --help.",
            );
            return None;
        }
        if let (Command::Attach, Some(path)) = (command, argument) {
            if self.attachments.paths.len() >= 8 {
                self.command_note("At most eight attachments per input.");
            } else if path.is_empty() {
                self.command_note("Use /attach PATH; Tab lists workspace paths.");
            } else {
                self.attachments.paths.push(path.to_string());
                self.command_note(format!(
                    "Attachment {} staged: {path:?}. Send a message to submit it; /detach INDEX removes it.",
                    self.attachments.paths.len()
                ));
            }
            return None;
        }
        if let (Command::Detach, Some(index)) = (command, argument) {
            match index.trim().parse::<usize>().ok().filter(|i| *i > 0 && *i <= self.attachments.paths.len()) {
                Some(index) => {
                    let path = self.attachments.paths.remove(index - 1);
                    self.command_note(format!("Attachment removed from draft: {path:?}"));
                }
                None => self.command_note("Use /detach INDEX with a staged attachment's one-based index."),
            }
            return None;
        }
        match command {
            Command::Help => {
                let detail = self.help_detail();
                self.open_report("Help", detail);
            }
            Command::Attach => self.command_note(format!(
                "Staged attachments: {:?}. Use /attach PATH or @PATH then Tab to complete.",
                self.attachments.paths
            )),
            Command::Detach => self.command_note("Use /detach INDEX with a staged attachment's one-based index."),
            Command::Edit => return Some(V2Effect::EditDraft { initial: argument.unwrap_or("").to_string() }),
            Command::Rewind => {
                if self.disconnected {
                    self.command_note("Reconnect before inspecting or changing the conversation.");
                    return None;
                }
                let arg = argument.unwrap_or("").trim();
                if !self.attachments.paths.is_empty() {
                    self.command_note("Remove staged files before opening a rewind draft.");
                    return None;
                }
                if arg.is_empty() {
                    self.rewind_modal = Some(crate::overlays::RewindModal::loading());
                    return Some(V2Effect::RewindList { before: None });
                }
                if arg == "draft" {
                    return Some(V2Effect::RewindDraft);
                }
                if let Some(before) = arg.strip_prefix("before ").and_then(|v| v.parse().ok()) {
                    return Some(V2Effect::RewindList { before: Some(before) });
                }
                if let Some(key) = arg.strip_prefix("files ").map(str::trim).filter(|s| !s.is_empty()) {
                    self.rewind_preview = None;
                    return Some(V2Effect::RewindFiles { key: key.into() });
                }
                if let Some(key) = arg.strip_prefix("preview ").filter(|s| !s.is_empty()) {
                    self.rewind_preview = None;
                    return Some(V2Effect::RewindPreview { key: key.into() });
                }
                if arg == "apply" {
                    if let Some(preview) = self.rewind_preview.take() {
                        return Some(V2Effect::RewindApply { preview });
                    }
                }
                self.command_note("Use /rewind preview KEY for conversation only or /rewind files KEY to include tracked files. Confirm with /rewind apply. /rewind draft reopens the unsent prompt.");
            }
            Command::Mcp => {
                if self.disconnected {
                    self.command_note("Disconnected: no MCP request sent.");
                    return None;
                }
                let arg = argument.unwrap_or("").trim();
                if arg.is_empty() {
                    self.pending_mcp_modal = true;
                    return Some(V2Effect::McpStatus);
                }
                if arg == "status" {
                    self.pending_mcp_modal = false;
                    return Some(V2Effect::McpStatus);
                }
                if let Some((action, key)) = arg.split_once(char::is_whitespace) {
                    let key = key.trim();
                    if !key.is_empty() && matches!(action, "start" | "stop" | "hide" | "expose" | "reconnect") {
                        return Some(V2Effect::McpChange { action: action.into(), service: key.into() });
                    }
                }
                self.command_note("Use /mcp status, or /mcp start|stop|hide|expose|reconnect KEY.");
            }
            Command::CodeGraph => {
                let action = match argument.map(str::trim).unwrap_or("") {
                    "" | "status" => "status",
                    "sync" => "sync",
                    "init" => "init",
                    _ => {
                        self.command_note("Use /codegraph status, init or sync.");
                        return None;
                    }
                };
                let Some(workspace) =
                    self.workspace().filter(|w| std::path::Path::new(w).is_absolute() && !self.disconnected)
                else {
                    self.command_note("Reconnect to select the workspace for CodeGraph.");
                    return None;
                };
                return Some(V2Effect::CodeGraph { workspace: workspace.to_owned(), action });
            }
            Command::Shell => {
                let Some(workspace) =
                    self.workspace().filter(|w| std::path::Path::new(w).is_absolute() && !self.disconnected)
                else {
                    self.command_note("Local shell not started: reconnect to a session with a known workspace.");
                    return None;
                };
                let (command, draft) = match argument.map(str::trim_start).filter(|s| !s.is_empty()) {
                    None => (None, false),
                    Some(s) => match s.split_once(char::is_whitespace) {
                        Some((mode @ ("view" | "draft"), cmd)) if !cmd.trim().is_empty() => {
                            (Some(cmd.to_owned()), mode == "draft")
                        }
                        _ => {
                            self.command_note("Use /shell, /shell view COMMAND, or /shell draft COMMAND. Nothing is sent automatically.");
                            return None;
                        }
                    },
                };
                return Some(V2Effect::OperatorShell { workspace: workspace.to_owned(), command, draft });
            }
            Command::Team => {
                self.focus = Focus::Team;
            }
            Command::Chat => self.team.show(crate::team::View::Conversation),
            Command::Tasks => {
                self.team.show(crate::team::View::Tasks);
                self.focus = Focus::Panel;
            }
            Command::Activity => {
                self.team.show(crate::team::View::Activity);
                self.focus = Focus::Panel;
            }
            Command::Inspect => {
                self.team.show(crate::team::View::Inspect);
                self.focus = Focus::Panel;
            }
            Command::Sessions => return self.open_sessions(),
            Command::Queue => return self.open_queue(),
            Command::Tree => {
                self.team.show(crate::team::View::Tree);
                self.focus = Focus::Panel;
                self.team.tree_dirty = true;
                return Some(V2Effect::SearchTree);
            }
            Command::Later => {
                let Some(text) = argument.filter(|s| !s.trim().is_empty()) else {
                    self.command_note("Use /later TEXT.");
                    return None;
                };
                let draft = format!("/later {text}");
                if self.disconnected || self.active_instance().is_none() {
                    self.composer.set_text(&draft);
                    self.command_note("Message kept in the draft: waiting for the Leader connection.");
                    return None;
                }
                self.composer.record_submission(&draft);
                return Some(V2Effect::SubmitInput {
                    instance: self.active_instance()?.id.clone(),
                    envelope: self.attachments.envelope(&draft),
                    text: text.into(),
                    attachments: self.attachments.paths.clone(),
                    after_work: true,
                });
            }
            Command::Compact => {
                if self.disconnected {
                    self.command_note("Disconnected: compaction not requested.");
                } else {
                    return Some(V2Effect::Compact {
                        instance: self.active_instance()?.id.clone(),
                        focus: argument.unwrap_or("").to_string(),
                    });
                }
            }
            Command::Interrupt => return self.interrupt(),
            Command::Model => return self.open_profile(),
            Command::Quit => {
                self.quit = true;
                return Some(V2Effect::Quit);
            }
        }
        None
    }

    fn composer_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if matches!(key.code, KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete)
            && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.notice = None;
        }
        if key.code == KeyCode::Enter && key.kind == crossterm::event::KeyEventKind::Repeat {
            return None;
        }
        if key.code == KeyCode::Char('?')
            && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.composer.text().is_empty()
            && key.kind == crossterm::event::KeyEventKind::Press
        {
            self.open_help();
            return None;
        }
        if self.attachments.rewind_source.is_some() && key.code == KeyCode::Esc {
            self.composer.clear();
            self.attachments.accepted();
            self.command_note(
                "Local rewind draft discarded; nothing sent. /rewind draft can reopen it until the context advances.",
            );
            return None;
        }
        if self.direct_target.is_some()
            && key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && self.attachments.rewind_source.is_none()
        {
            self.direct_target = None;
            self.flash("Returned composer target to Leader.");
            return None;
        }
        if self.attachments.rewind_source.is_none() && self.attachments.key(&mut self.composer, key) {
            if let Some(note) = self.attachments.take_notice() {
                self.command_note(note);
            }
            return None;
        }
        if self.attachments.rewind_source.is_none() && key.code == KeyCode::Tab && key.modifiers.is_empty() {
            if let Some(prefix) = self.attachments.request(&self.composer) {
                return Some(V2Effect::InputPaths { prefix });
            }
        }
        if self.attachments.rewind_source.is_none() && self.completion.key(&mut self.composer, key) {
            return None;
        }
        if key.code == KeyCode::Char('e') && key.modifiers == KeyModifiers::CONTROL {
            return self.dispatch_command(crate::completion::Command::Edit, Some(&self.composer.text()));
        }
        match key.code {
            // Shift+Enter edits the draft without sending it. The terminal must
            // report the modifier; startup enables supported keyboard enhancements.
            KeyCode::Enter if key.modifiers == KeyModifiers::SHIFT => {
                self.composer.insert_newline();
                None
            }
            KeyCode::Enter if key.modifiers.is_empty() => {
                self.notice = None;
                let raw = self.composer.text();
                if self.attachments.rewind_source.is_none() && raw.starts_with('/') {
                    self.composer.clear();
                    self.direct_target = None;
                    return self.slash_command(&raw);
                }
                if self.disconnected || self.active_instance().is_none() {
                    self.command_note("Message kept in the draft: waiting for the Leader connection.");
                    return None;
                }
                let text = self.composer.submit()?;
                self.composer.record_submission(&text);
                let envelope = self.attachments.envelope(&text);
                if let Some(source) = self.attachments.rewind_source.clone() {
                    self.direct_target = None;
                    return Some(V2Effect::RewindSend { source, envelope, text });
                }
                let instance = self.direct_target.take().unwrap_or_else(|| self.team.leader_id.clone());
                if instance != self.team.leader_id {
                    self.flash(format!("Direct message sent to {instance}; composer returned to Leader."));
                }
                Some(V2Effect::SubmitInput {
                    instance,
                    envelope,
                    text,
                    after_work: false,
                    attachments: self.attachments.paths.clone(),
                })
            }
            KeyCode::PageUp => {
                self.scroll_chat(self.last_chat_height.max(1));
                None
            }
            KeyCode::PageDown => {
                self.scroll_chat_back(self.last_chat_height.max(1));
                None
            }
            // ↑↓ are the composer's: inside a multi-line draft they walk its rows,
            // and at the first/last row they recall history — the contract `text.rs`
            // documents. Scrolling keeps PageUp/PageDown and the mouse wheel.
            KeyCode::Up => {
                if self.composer.row > 0 {
                    self.composer.move_up();
                } else {
                    self.composer.recall(-1);
                }
                None
            }
            KeyCode::Down => {
                if self.composer.row + 1 < self.composer.lines.len() {
                    self.composer.move_down();
                } else {
                    self.composer.recall(1);
                }
                None
            }
            _ => {
                self.composer.edit_key(key);
                None
            }
        }
    }

    pub fn scroll_chat(&mut self, lines: usize) {
        let max = self.last_chat_lines.saturating_sub(self.last_chat_height);
        self.chat_scroll = (self.chat_scroll + lines).min(max);
    }

    pub fn scroll_chat_back(&mut self, lines: usize) {
        self.chat_scroll = self.chat_scroll.saturating_sub(lines);
    }

    pub fn environment_details(&self) -> String {
        if !self.environment.is_object() {
            return "Execution environment: run teamagents doctor.".into();
        }
        let mut lines = vec![format!(
            "Workspace: {}\nExecution: {} ({})",
            self.environment["workspace"].as_str().unwrap_or("unknown"),
            self.environment["execution"].as_str().unwrap_or("unknown"),
            self.environment["permissions"].as_str().unwrap_or("unknown")
        )];
        if self.environment["configuration"]["restart_required"] == true {
            lines.push(
                "Configuration changed since startup; explicitly stop this daemon and reconnect to apply it.".into(),
            );
        }
        if self.environment["configuration"]["comparison_available"] == false {
            lines.push("Configuration comparison unavailable; teamagents status --json lists affected paths.".into());
        }
        lines.push("Boot-selected sources (contents read per turn; skills loaded on demand):".into());
        for key in ["instruction_files", "skills_paths"] {
            let rows = self.environment["sources"][key].as_array();
            for row in rows.into_iter().flatten().take(20) {
                lines.push(format!("  {key}: {}", row.as_str().unwrap_or("unknown")));
            }
            if rows.is_some_and(|rows| rows.len() > 20) {
                lines.push(format!("  More {key}: teamagents status --json"));
            }
        }
        let loaded = &self.environment["leader_instruction_load"];
        if loaded.is_object() {
            lines.push(format!(
                "Last Leader prompt assembly: epoch {}, observed at {}.",
                loaded["epoch"], loaded["observed_at"]
            ));
            for file in loaded["files"].as_array().into_iter().flatten().take(20) {
                lines.push(format!(
                    "  {}: {}{}",
                    file["path"].as_str().unwrap_or("?"),
                    file["status"].as_str().unwrap_or("unknown"),
                    file["reason"].as_str().map(|reason| format!(" ({reason})")).unwrap_or_default()
                ));
            }
        } else {
            lines.push("No instruction load observed yet.".into());
        }
        lines.push(
            "teamagents doctor reports source readability; doctor environment probes the host and configured tools."
                .into(),
        );
        lines.join("\n")
    }

    pub fn diagnostic_details(&self) -> String {
        let mut lines = vec!["Diagnostics (read-only):".to_string()];
        for (id, diagnostic) in self.work_diagnostics.iter().take(20) {
            lines.push(format!("{id}: {}", diagnostic["summary"].as_str().unwrap_or("unknown")));
            if let Some(reason) = diagnostic["reason"].as_str() {
                lines.push(format!("  reason: {reason}"));
            }
            lines.push(format!(
                "  {}",
                diagnostic["next_steps"].as_str().unwrap_or("Inspect teamagents status --json.")
            ));
            for wait in diagnostic["waits"].as_array().into_iter().flatten().take(4) {
                lines.push(format!(
                    "  wait {} mode={} timer_at={} conditions={}",
                    wait["id"], wait["mode"], wait["timer_at"], wait["conditions"]
                ));
            }
        }
        if self.work_diagnostics.len() > 20 {
            lines.push(format!(
                "{} more diagnostic rows omitted; use the CLI for all rows.",
                self.work_diagnostics.len() - 20
            ));
        }
        lines.push("INSTANCE/TASK are row ids. Full diagnostics: teamagents instances · teamagents tasks · teamagents status --json.".into());
        lines.join("\n")
    }

    pub(crate) fn workspace(&self) -> Option<&str> {
        self.environment["workspace"].as_str().filter(|s| !s.is_empty())
    }

    pub fn status_line(&self) -> String {
        if self.disconnected {
            return "Disconnected · reconnecting".into();
        }
        if !self.approvals.is_empty() {
            return format!("Approval needed ({}) · Ctrl+A review", self.approvals.len());
        }
        if let Some(waiting) = self
            .instances
            .iter()
            .find(|instance| instance.lifecycle == "ACTIVE" && instance.phase == "WAITING_CAPABILITY")
        {
            return format!("{} · waiting for capability approval · teamagents instances", waiting.id);
        }
        let Some(leader) = self.active_instance() else { return "Connecting to the Leader".into() };
        if leader.lifecycle != "ACTIVE" {
            let reason = self
                .work_diagnostics
                .iter()
                .find(|(id, _)| id == &leader.id)
                .and_then(|(_, diagnostic)| diagnostic["reason"].as_str())
                .unwrap_or(&leader.reason);
            return format!("{} · {} · /inspect", leader.lifecycle, reason);
        }
        if let Some((_, diagnostic)) = self.work_diagnostics.iter().find(|(id, _)| id == &leader.id) {
            match diagnostic["code"].as_str() {
                Some("needs_input") => {
                    return format!(
                        "Needs your decision · {} · /help",
                        diagnostic["reason"].as_str().unwrap_or("Reply to the recorded question")
                    )
                }
                Some("work_blocked") => return "Work blocked · /help for the saved reason".into(),
                Some("stopped") => return "Stopped · Enter send · /help for recovery".into(),
                _ => {}
            }
        }
        if let Some((id, diagnostic)) = self.work_diagnostics.iter().find(|(_, d)| d["attention"] == true) {
            return format!(
                "{id}: {} · /help",
                diagnostic["code"].as_str().unwrap_or("inspect status").replace('_', " ")
            );
        }
        let state = match leader.phase.as_str() {
            "READY"
                if self.work_diagnostics.iter().any(|(id, d)| id == &leader.id && d["work"]["state"] == "RUNNING") =>
            {
                "Working"
            }
            "READY" => "Ready",
            "MODEL_PENDING" => "Thinking",
            "TOOLS_PENDING" => "Working",
            "COMPLETION_PENDING" => "Checking",
            "WAITING" => "Waiting for the team",
            other => other,
        };
        let usage = self
            .work
            .as_ref()
            .map(|g| {
                let limit = g.limit_total.map(|n| format!("/{n}")).unwrap_or_default();
                format!(" · {}{limit} tokens{}", g.known_total, if g.unknown { " + unknown" } else { "" })
            })
            .unwrap_or_default();
        let deadline = self
            .work
            .as_ref()
            .and_then(|g| g.deadline)
            .map(|deadline| {
                let left = deadline - crate::now_epoch();
                if left > 0.0 {
                    format!(" · ends in {}m", (left / 60.0) as i64)
                } else {
                    " · past its deadline".into()
                }
            })
            .unwrap_or_default();
        let wait = self
            .work_diagnostics
            .iter()
            .find(|(id, d)| id == &self.team.leader_id && d["code"] == "waiting")
            .map(|(_, d)| format!(" · {} · /help", d["summary"].as_str().unwrap_or("")))
            .unwrap_or_default();
        format!("{state}{usage}{deadline}{wait}")
    }

    /// Concise runtime state for the top status bar (excludes transient flash notices so
    /// action feedback shows solely in `activity_line` above the composer).
    pub fn runtime_status_line(&self) -> String {
        if let Some(status) = self.attention_line() {
            return status;
        }
        let status = self.status_line();
        if ["Ready", "Working", "Thinking", "Checking"].iter().any(|state| status.starts_with(state)) {
            return status.split(" · ").next().unwrap_or(&status).to_string();
        }
        status
    }

    /// The one line above the composer: a decision that needs the user outranks feedback, because feedback is
    /// transient and a pending approval is not.
    pub fn activity_line(&self) -> String {
        if let Some(status) = self.attention_line() {
            return status;
        }
        if let Some(notice) = &self.notice {
            return notice.text.clone();
        }
        self.runtime_status_line()
    }

    /// The notice the activity row is currently showing, when it is showing one.
    pub fn shown_notice(&self) -> Option<&Notice> {
        self.attention_line().is_none().then_some(())?;
        self.notice.as_ref()
    }

    fn attention_line(&self) -> Option<String> {
        if !self.approvals.is_empty() {
            return Some(format!("Approval needed ({})", self.approvals.len()));
        }
        let status = self.status_line();
        [":Needs", ":Approval", ":Stopped", ":Work blocked", ":Disconnected", ":PARKED", ":TERMINATED"]
            .iter()
            .any(|prefix| status.starts_with(&prefix[1..]))
            .then_some(status)
    }

    pub fn footer_hint(&self) -> String {
        if self.confirm.is_some() {
            return "y/Enter confirm · n/Esc cancel".into();
        }
        if self.approval_detail.is_some() {
            return "a approve · d deny · Up/Down scroll · Esc close".into();
        }
        if self.palette.is_some() {
            return "Type filter · Up/Down choose · Enter run · Esc close".into();
        }
        if self.rewind_modal.is_some() {
            return "Up/Down choose · r chat preview · f file diff · Enter apply · Esc close".into();
        }
        if self.mcp_modal.is_some() {
            return "s start/stop · h hide/expose · r reconnect · Enter refresh · Esc close".into();
        }
        if self.report.is_some() {
            return "Esc close · Up/Down scroll · PgUp/PgDn page".into();
        }
        if self.help_open {
            return "Esc close · Up/Down scroll · PgUp/PgDn page".into();
        }
        if self.profile.is_some() {
            return "↑/↓ choose · Enter save · Esc back · Ctrl+U clear search".into();
        }
        if self.team.view == crate::team::View::Conversation {
            if let Some(search) = &self.chat_search {
                if search.editing {
                    return "Type query · Enter apply · Up/Down jump · Esc cancel".into();
                }
                return "n next match · N prev match · Ctrl+F edit · Esc clear search".into();
            }
        }
        if self.focus == Focus::Team {
            return "Up/Down member · Enter inspect · m direct · p pause · r resume · X stop · ! alert · Esc compose"
                .into();
        }
        if self.focus == Focus::Panel {
            return match self.team.view {
                crate::team::View::Activity => "PgUp/PgDn scroll · f filter · z zoom · F6 focus · Esc compose",
                crate::team::View::Tasks => "Up/Down select · Enter inspect assignee · x cancel · z zoom · Esc compose",
                crate::team::View::Tree => "Up/Down select · Space fold · Enter focus in draft · z zoom · Esc compose",
                crate::team::View::Wall => "Up/Down or 1-9 member · Enter history · m direct · z zoom · Esc compose",
                _ => "PgUp/PgDn scroll · d details · h history · P pin · m direct · z zoom · Esc compose",
            }
            .into();
        }
        if self.focus == Focus::Approvals {
            return "Enter detail · a approve once · d deny · Up/Down select · Esc back".into();
        }
        if self.attachments.visible(&self.composer) || !self.completion.candidates(&self.composer).is_empty() {
            return "Tab/Enter fill · Esc cancel · Up/Down select · Enter again submits".into();
        }
        if self.disconnected || self.active_instance().is_none() {
            return "Draft kept · Ctrl+Q quit · /help".into();
        }
        if !self.approvals.is_empty() {
            return "Ctrl+A review · Enter send · Ctrl+Q quit".into();
        }
        if self.chat_scroll > 0 {
            return format!("PgDn newer · {} lines above latest · PgUp older · Enter send", self.chat_scroll);
        }
        if self.direct_target.is_some() {
            return "Enter send direct · Esc back to Leader · Ctrl+Q quit".into();
        }
        if self.active_instance().is_some_and(|i| i.lifecycle == "ACTIVE" && i.phase != "READY") {
            return "Ctrl+C interrupt · Enter send · Ctrl+P queue · Ctrl+K palette · Ctrl+Q quit".into();
        }
        if self.composer.text().is_empty() {
            "Enter send · F6 team · ? shortcuts · Ctrl+S sessions · Ctrl+K palette · Ctrl+Q quit".into()
        } else {
            "Enter send · Ctrl+S sessions · Ctrl+E editor · Ctrl+K palette · Ctrl+Q quit".into()
        }
    }
}
