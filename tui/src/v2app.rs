//! R2-P4 R19-b② v2 conversation interface state and key handling (plan §9).
//! Pure logic like app.rs: side effects leave as `V2Effect`s executed by the
//! main loop through the daemon client or session CLI, so interaction logic
//! is testable without a terminal. Tools and models run in the engine;
//! history is the authoritative conversation, events are signals,
//! and system notes are UI-level annotations on top.

use serde_json::Value as Json;

use crate::text::Composer;

/// The slash commands the composer understands (D-370), shown while a `/` draft is open and by `/help`.
const SLASH_HELP: &str = "commands: /help · /sessions · /interrupt · /quit";

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
    /// The runtime's own closing word (a settlement note, D-71): the instance
    /// did not say it, so it is never rendered as the member's message.
    Runtime,
    /// A runtime compaction summary (R22/A20): the model's view is covered by
    /// it, so the user must see where the conversation was compacted.
    Summary,
    System,
    Error,
    /// Client help and receipts survive checkpoint refreshes (D-433/D-445).
    /// They are requested output, rather than transient runtime chatter.
    Command,
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
    /// The model this member runs on (D-69), available in /help for the Leader.
    /// Empty for an older daemon that does not report it.
    pub model: String,
    /// The last lifecycle transition's own words (D-165): why this instance is not
    /// running — a park reason, or the user's own pause. Empty for an ACTIVE member
    /// and for a transition recorded without words.
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// The goal's absolute deadline in epoch seconds (`[limits] deadline_minutes`,
    /// D-64): shown as the time left, so a configured ceiling is visible.
    pub deadline: Option<f64>,
    /// What the model said it did when it settled the goal (D-431). A model can settle a goal by calling
    /// `finish` without writing a single word of prose (measured on the tau3 retail split: 12 of 114 dialogues
    /// ended that way, and a user read an empty reply), and the summary it recorded with that call is the only
    /// account of the work there is. The status line shows it, so a TUI user is not left with a bare
    /// `goal SUCCEEDED`.
    pub summary: Option<String>,
}

/// Side effects for the main loop to execute through the daemon client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum V2Effect {
    Sessions(crate::sessions::SessionAction),
    SubmitInput {
        instance: String,
        envelope: String,
        text: String,
    },
    Decide {
        approval_id: String,
        decision: &'static str,
    },
    /// Interrupt the Leader at the runtime boundary; leaving the TUI is independent.
    Interrupt {
        instance: String,
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
/// Tool-result preview cap in the conversation (the UI only renders previews, §9).
const PREVIEW_CHARS: usize = 400;

pub struct V2App {
    pub session_root: String,
    pub session_name: String,
    pub browser: Option<crate::sessions::SessionBrowser>,
    pub session_id: String,
    pub instances: Vec<InstanceInfo>,
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
    pub last_chat_width: usize,
    /// §9: the transient text each instance is streaming, newest read only. Never an authoritative fact.
    pub previews: Vec<(String, String)>,
    /// Visible chat height at the last render (page scrolling).
    pub last_chat_height: usize,
}

impl V2App {
    pub fn new(session_id: &str) -> V2App {
        V2App {
            session_root: String::new(),
            session_name: String::new(),
            browser: None,
            session_id: session_id.to_string(),
            instances: vec![],
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
            last_chat_width: 0,
            previews: vec![],
            last_chat_height: 1,
        }
    }

    pub fn active_instance(&self) -> Option<&InstanceInfo> {
        self.instances.iter().find(|instance| instance.id == "i-leader")
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

    /// A command's output: it survives a checkpoint refresh, unlike a `System` note, because the user asked a
    /// question and the answer must stay on screen until they scroll away from it (D-433).
    pub fn command_note(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry { kind: ChatKind::Command, who: "note".into(), text: text.into() });
        if self.entries.len() > NOTE_CAP * 4 {
            let drop = self.entries.len() - NOTE_CAP * 4;
            self.entries.drain(..drop);
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry { kind: ChatKind::System, who: "system".into(), text: text.into() });
        if self.entries.len() > NOTE_CAP * 4 {
            let drop = self.entries.len() - NOTE_CAP * 4;
            self.entries.drain(..drop);
        }
    }

    fn note_error(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry { kind: ChatKind::Error, who: "error".into(), text: text.into() });
    }

    // ---- daemon → app application points --------------------------------

    /// Checkpoint (connect/reconnect): snapshot state and the watermark the
    /// event stream resumes from — one consistent read (§9).
    pub fn apply_checkpoint(&mut self, snapshot: Json, watermark: i64) {
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
            })
            .collect();
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
                deadline: g["deadline"].as_f64(),
                summary: g["summary"].as_str().filter(|text| !text.trim().is_empty()).map(|text| {
                    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    collapsed.chars().take(110).collect()
                }),
            })
        });
        self.watermark = self.watermark.max(watermark);
        // a fresh checkpoint supersedes earlier UI-level notes — runtime chatter, not a command's answer
        // (D-433: `Command` entries are what a user asked for and stay until they scroll away)
        self.entries.retain(|e| e.kind != ChatKind::System && e.kind != ChatKind::Error);
    }

    /// History of the active instance: the authoritative conversation (§9 —
    /// only renders previews). System notes stay layered on top, newest last.
    pub fn apply_history(&mut self, history: Json) {
        let mut rebuilt: Vec<ChatEntry> = Vec::new();
        for entry in history["entries"].as_array().cloned().unwrap_or_default() {
            let kind = entry["kind"].as_str().unwrap_or("");
            let message = &entry["message"];
            match kind {
                "user" => rebuilt.push(ChatEntry {
                    kind: ChatKind::User,
                    who: "you".into(),
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
                        who: "result".into(),
                        text: truncate(&content.split_whitespace().collect::<Vec<_>>().join(" "), 120),
                    });
                }
                // the runtime's own closing word for a turn (§8/D-71): it ends
                // the turn, it is not something the member said
                "runtime" => rebuilt.push(ChatEntry {
                    kind: ChatKind::Runtime,
                    who: "runtime".into(),
                    text: message["content"].as_str().unwrap_or("").to_string(),
                }),
                // covered entries stay visible here: the user sees the whole
                // conversation, the model sees the summary (R22/A20)
                "summary" => rebuilt.push(ChatEntry {
                    kind: ChatKind::Summary,
                    who: "compaction".into(),
                    text: truncate(message["content"].as_str().unwrap_or(""), PREVIEW_CHARS),
                }),
                _ => {}
            }
        }
        let notes: Vec<ChatEntry> = self
            .entries
            .drain(..)
            .filter(|e| matches!(e.kind, ChatKind::System | ChatKind::Error | ChatKind::Command))
            .collect();
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
                "goal_completed" => {
                    refresh.history = true;
                    refresh.checkpoint = true;
                    // a runtime block announces itself twice (settlement + block
                    // reason); the `goal_blocked` note below carries the reason,
                    // so this one stays quiet for it (D-71)
                    if payload["blocked_by"].as_str() != Some("runtime") {
                        self.note(format!("goal finished: {}", payload["status"].as_str().unwrap_or("?")));
                    }
                }
                "goal_blocked" => {
                    refresh.checkpoint = true;
                    self.note(format!("goal parked (BLOCKED): {}", payload["reason"].as_str().unwrap_or("")));
                }
                "check_round_registered" => {
                    // name the checks: a user waiting on a goal must see which
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
                "instance_created" => {
                    refresh.checkpoint = true;
                    self.note(format!("instance {} created", event["scope"].as_str().unwrap_or("?")));
                }
                "task_delegated" | "task_started" | "task_completed" => {
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
                }
                "operation_completed" if payload["status"].as_str() == Some("OUTCOME_UNKNOWN") => {
                    self.note(format!(
                        "operation {} has an unknown outcome (not replayed)",
                        event["scope"].as_str().unwrap_or("?")
                    ));
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

    pub fn command_failed(&mut self, error: &str) {
        self.note_error(format!("command failed: {error}"));
    }

    pub fn mark_disconnected(&mut self, error: &str) {
        if !self.disconnected {
            self.disconnected = true;
            self.note_error(format!("disconnected from the daemon ({error}), reconnecting..."));
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
        self.note_error(format!("send failed: {error}"));
    }

    /// The input is accepted but waits for the running turn's boundary (D-63): it
    /// cannot be part of a request that is already fixed, so it enters the
    /// conversation after that turn's own answer. Say so instead of leaving the
    /// user wondering whether the message was sent at all.
    pub fn input_queued(&mut self, instance: &str) {
        self.note(format!("queued for {instance}: it enters when the running turn ends"));
    }

    pub fn decide_failed(&mut self, error: &str) {
        self.note_error(format!("the approval decision failed: {error}"));
    }

    // ---- keys ------------------------------------------------------------

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if self.browser.is_some()
            && !(key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'd')))
        {
            let browser = self.browser.as_mut()?;
            if key.code == KeyCode::Esc && !browser.busy && browser.confirmation.is_none() {
                self.browser = None;
                return None;
            }
            return browser.key(key).map(V2Effect::Sessions);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => return self.open_sessions(),
                KeyCode::Char('c') => return self.interrupt(),
                KeyCode::Char('d') => {
                    self.quit = true;
                    return Some(V2Effect::Quit);
                }
                KeyCode::Char('a') => {
                    if !self.approvals.is_empty() {
                        self.focus = Focus::Approvals;
                    }
                    return None;
                }
                _ => {}
            }
        }
        match self.focus {
            Focus::Approvals => self.approval_key(key),
            Focus::Composer => self.composer_key(key),
        }
    }

    fn open_sessions(&mut self) -> Option<V2Effect> {
        self.browser = Some(crate::sessions::SessionBrowser::default());
        Some(V2Effect::Sessions(crate::sessions::SessionAction::List))
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

    /// Client commands never become model input, including retired management commands.
    fn slash_command(&mut self, text: &str) -> Option<V2Effect> {
        match text.trim() {
            "/help" => self.command_note(format!(
                "Session {} · model {}
Enter sends to the Leader; Ctrl+J or Shift+Enter adds a newline.
Up/Down recalls input; PageUp/PageDown or the wheel reads history.
Ctrl+C or /interrupt cancels the Leader's model request; running tools and delegated work continue.
Ctrl+A focuses pending approvals; a approves once, d denies, Esc returns.
Ctrl+D or /quit exits; the session keeps running.
Ctrl+S or /sessions opens session history, switching and confirmed deletion.
Developer tools: teamagents --help; teamagents status --json; teamagents history --id ID.",
                self.session_id,
                self.active_instance().map(|i| i.model.as_str()).unwrap_or("unavailable")
            )),
            "/sessions" => return self.open_sessions(),
            "/interrupt" => return self.interrupt(),
            "/quit" => {
                self.quit = true;
                return Some(V2Effect::Quit);
            }
            _ => self.command_note(format!("Unknown command {text:?}. {SLASH_HELP}. Management: teamagents --help.")),
        }
        None
    }

    fn composer_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        match key.code {
            // A newline in the composer, as the footer and `text.rs` promise.
            // Ctrl+J is the universal one: in raw mode the terminal's 0x0A
            // arrives as Ctrl+J (crossterm parses it that way), so it works
            // everywhere, while Shift+Enter needs a terminal that reports
            // modifiers (the TUI pushes the keyboard-enhancement flags).
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.insert_newline();
                None
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.composer.insert_newline();
                None
            }
            KeyCode::Enter => {
                if !self.composer.text().starts_with('/') && (self.disconnected || self.active_instance().is_none()) {
                    self.command_note("Message kept in the draft: waiting for the Leader connection.");
                    return None;
                }
                let text = self.composer.submit()?;
                // the arrows walk what was sent (D-77): `text.rs` has kept the
                // history and the recall rules all along, and nothing ever recorded
                // into it, so ↑ did nothing but scroll
                self.composer.record_submission(&text);
                // D-370: a leading `/` is a client command, not an instruction to the Leader (pi's slash-command
                // shape). Handled here, before the active instance is required, so `/help` works with no instance.
                if text.starts_with('/') {
                    return self.slash_command(&text);
                }
                let instance = self.active_instance()?.id.clone();
                let envelope = format!("env-{}", uuid::Uuid::new_v4());
                Some(V2Effect::SubmitInput { instance, envelope, text })
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
            KeyCode::Backspace => {
                self.composer.backspace();
                None
            }
            KeyCode::Delete => {
                self.composer.delete();
                None
            }
            // word-wise editing, the other half of `text.rs` that nothing called
            KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.move_word_left();
                None
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.move_word_right();
                None
            }
            KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.composer.delete_word();
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
            // plain text only: control chords are commands handled above
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
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

    pub fn status_line(&self) -> String {
        if self.disconnected {
            return "Disconnected · reconnecting".into();
        }
        if !self.approvals.is_empty() {
            return format!("Approval needed ({}) · Ctrl+A review", self.approvals.len());
        }
        let Some(leader) = self.active_instance() else { return "Connecting to the Leader".into() };
        if leader.lifecycle != "ACTIVE" {
            return format!("{} · {} · teamagents instances", leader.lifecycle, leader.reason);
        }
        let state = match leader.phase.as_str() {
            "READY" => "Ready",
            "MODEL_PENDING" => "Thinking",
            "TOOLS_PENDING" => "Working",
            "COMPLETION_PENDING" => "Checking",
            "WAITING" => "Waiting for the team",
            other => other,
        };
        let usage = self
            .goal
            .as_ref()
            .map(|g| {
                let limit = g.limit_total.map(|n| format!("/{n}")).unwrap_or_default();
                format!(" · {}{limit} tokens{}", g.known_total, if g.unknown { " + unknown" } else { "" })
            })
            .unwrap_or_default();
        let deadline = self
            .goal
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
        format!("{state}{usage}{deadline}")
    }

    pub fn footer_hint(&self) -> String {
        if self.focus == Focus::Approvals {
            return "a approve once · d deny · up/down select · Esc back".into();
        }
        if self.composer.text().starts_with('/') {
            return SLASH_HELP.into();
        }
        if !self.approvals.is_empty() {
            return "Ctrl+A review · Enter send · Ctrl+C interrupt · Ctrl+D exit".into();
        }
        "Enter send · Ctrl+S sessions · Ctrl+C interrupt · Ctrl+D exit · /help".into()
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
    format!("{kept}...({count} chars total)")
}
