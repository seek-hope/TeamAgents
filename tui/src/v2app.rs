//! R2-P4 R19-b② v2 conversation interface state and key handling (plan §9).
//! Pure logic like app.rs: side effects leave as `V2Effect`s executed by the
//! main loop through the daemon client, so every path is unit-testable
//! without a terminal or daemon process. The TUI never executes anything
//! itself — history is the authoritative conversation, events are signals,
//! and system notes are UI-level annotations on top.

use serde_json::Value as Json;

use crate::text::Composer;

/// The slash commands the composer understands (D-370), shown while a `/` draft is open and by `/help`.
const SLASH_HELP: &str =
    "commands: /help · /status · /chat · /instances · /tasks · /topology · /approvals · /mcp · /sessions · \
     /switch · /goals · /interrupt · /quit";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Composer,
    Approvals,
}

/// Top-level views (§9: conversation, instances, tasks, topology edge list).
/// `Ctrl+N` cycles through them (no function keys: many keyboards lack them).
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
            View::Chat => "Chat",
            View::Instances => "Instances",
            View::Tasks => "Tasks",
            View::Topology => "Topology",
        }
    }
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
    /// Output a **command** produced (`/status`, `/mcp`, `/sessions`, `/goals`): the user asked for it, so it is
    /// not transient runtime chatter and a checkpoint refresh must not wipe it before it is read. Measured
    /// 2026-10-06 with the pty smoke: notes lived in `System`, and `apply_checkpoint`'s "a fresh checkpoint
    /// supersedes earlier UI-level notes" removed them within one poll — so `/mcp` (D-413) and `/sessions`
    /// (D-429) printed a report that nobody could read, which is how the `/goals` step found it (D-433).
    Command,
}

#[derive(Clone, Debug)]
pub struct ChatEntry {
    pub kind: ChatKind,
    pub who: String,
    pub text: String,
}

/// D-429: one named session under this base, as the daemon reports it. The `root` is what makes switching possible
/// from a client that has no config loader of its own: it names the directory whose daemon owns that session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub root: String,
    /// The session *this* daemon serves.
    pub active: bool,
}

/// One configured MCP service, as the daemon reports it (D-413). The TUI cannot read the machine's config (it is
/// a client, not the loader), so this is what lets `/mcp` answer "what did this session bind, and what is the next
/// step for it?" without starting a second copy of any server.
#[derive(Debug, Clone, PartialEq)]
pub struct McpInfo {
    pub name: String,
    pub transport: String,
    pub required: bool,
    /// `Some(state)` for an OAuth binding (`signed in`, `expired (will refresh)`, `not signed in`), `None` for a
    /// binding whose credential comes from the environment.
    pub credential: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InstanceInfo {
    pub id: String,
    pub lifecycle: String,
    pub phase: String,
    /// The model this member runs on (D-69): a team can span providers
    /// (`spawn(model = …)`), so the panel says which one each member uses. Empty
    /// for an older daemon that does not report it.
    pub model: String,
    /// The last lifecycle transition's own words (D-165): why this instance is not
    /// running — a park reason, or the user's own pause. Empty for an ACTIVE member
    /// and for a transition recorded without words.
    pub reason: String,
    /// Whether this member may run commands in the shared workspace (D-442). A member the Leader *spawns* does
    /// not inherit it (D-61), and the tool surface hides `shell` from it (D-60) — so nothing is refused and the
    /// member simply never runs anything, which is exactly what D-256 measured. The daemon computes it with the
    /// runtime's own covering rule; an older daemon that does not report it reads as `true`, so the panel only
    /// ever adds a marker it can justify.
    pub can_run_shell: bool,
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
    /// The grant's id as the daemon reports it (D-61): the topology panel shows
    /// its short form, and it is what `teamagents authority revoke` takes.
    pub id: String,
    /// Who issued it: `user` for a grant the user issued, otherwise the instance
    /// that narrowed its own authority.
    pub issuer: String,
    pub subject: String,
    pub action: String,
    pub scope: String,
    pub revoked: bool,
}

impl GrantInfo {
    /// The short form shown in the topology panel: enough to be recognisable and
    /// to type into `authority revoke` (which takes any unambiguous prefix).
    pub fn short_id(&self) -> &str {
        let end = self.id.char_indices().nth(10).map(|(at, _)| at).unwrap_or(self.id.len());
        &self.id[..end]
    }
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
    /// Interrupt the selected instance's running turn (D-363): not destructive, so no confirmation. A
    /// queued instruction enters at the boundary and gets its own turn.
    Interrupt {
        instance: String,
    },
    /// D-429: attach this client to another named session (the main loop connects to that session's own daemon).
    SwitchSession {
        id: String,
    },
    /// Cancel a non-terminal task from the panel (§5.3).
    CancelTask {
        task_id: String,
    },
    /// D-269: open the next goal, attached to the selected instance — the panel's own path to the lever the CLI
    /// got in D-267. A settled goal cannot be reopened, so a session longer than one goal needs this; the
    /// *session's* `[limits]`/`[[checks]]` bound the new goal, applied by the daemon (D-268), so the panel needs
    /// no configuration of its own.
    OpenGoal {
        attach: String,
    },
    /// D-433: read the session's goals (`goals`), because the checkpoint reports only one of them (D-266) — the
    /// same reason `/sessions` reads the registry (D-429). The handler prints them through `goals_report`.
    RefreshGoals,
    /// D-433: close a goal by id (`cancel_goal`), which is the TUI's missing half of the `g` key that opens one:
    /// without it a session accumulates ACTIVE goals and the status line keeps showing the newest of them.
    CancelGoal {
        id: String,
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
/// Tool-result preview cap in the conversation (the UI only renders previews, §9).
const PREVIEW_CHARS: usize = 400;

pub struct V2App {
    pub session_id: String,
    /// Current top-level view (§9 panels); Chat is the default.
    pub view: View,
    pub instances: Vec<InstanceInfo>,
    /// D-413: the session's configured MCP services and their credential state.
    pub mcp: Vec<McpInfo>,
    /// D-429: the named sessions under this base (this one marked), so `/sessions` can offer them.
    pub sessions: Vec<SessionInfo>,
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
    /// Topology edge-list scroll offset (§9: the topology panel is an edge list).
    pub topo_scroll: usize,
    pub confirm: Option<Confirm>,
    /// Scroll offset in wrapped lines from the bottom (0 = following).
    pub chat_scroll: usize,
    pub watermark: i64,
    pub quit: bool,
    /// Total wrapped chat lines at the last render (scroll clamping).
    pub last_chat_lines: usize,
    /// §9: the transient text each instance is streaming, newest read only. Never an authoritative fact.
    pub previews: Vec<(String, String)>,
    /// Visible chat height at the last render (page scrolling).
    pub last_chat_height: usize,
}

impl V2App {
    pub fn new(session_id: &str) -> V2App {
        V2App {
            session_id: session_id.to_string(),
            view: View::Chat,
            instances: vec![],
            mcp: vec![],
            sessions: vec![],
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
            previews: vec![],
            last_chat_height: 1,
        }
    }

    pub fn active_instance(&self) -> Option<&InstanceInfo> {
        self.instances.get(self.active)
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

    /// A runtime note the user reads in the conversation (D-429 uses it for a session switch, which is a client
    /// event rather than a chat message).
    pub fn system_note(&mut self, text: impl Into<String>) {
        self.note(text);
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
        let previous = self.active_instance().map(|i| i.id.clone());
        self.mcp = snapshot["mcp"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| McpInfo {
                name: row["name"].as_str().unwrap_or("").to_string(),
                transport: row["transport"].as_str().unwrap_or("stdio").to_string(),
                required: row["required"].as_bool().unwrap_or(false),
                credential: row["credential"].as_str().map(str::to_string),
            })
            .collect();
        self.sessions = snapshot["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|row| SessionInfo {
                id: row["id"].as_str().unwrap_or("").to_string(),
                name: row["name"].as_str().unwrap_or("").to_string(),
                root: row["root"].as_str().unwrap_or("").to_string(),
                active: row["active"].as_bool().unwrap_or(false),
            })
            .collect();
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
                can_run_shell: i["can_run_shell"].as_bool().unwrap_or(true),
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
                deadline: g["deadline"].as_f64(),
                summary: g["summary"].as_str().filter(|text| !text.trim().is_empty()).map(|text| {
                    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    collapsed.chars().take(110).collect()
                }),
            })
        });
        if self.instance_sel >= self.instances.len() {
            self.instance_sel = self.instances.len().saturating_sub(1);
        }
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
                        text: truncate(content, PREVIEW_CHARS),
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
                    refresh.tasks = true;
                }
                "task_blocked" => {
                    refresh.tasks = true;
                    self.note(format!(
                        "task {} parked (BLOCKED): {}",
                        payload["task_id"].as_str().unwrap_or("?"),
                        payload["reason"].as_str().unwrap_or("")
                    ));
                }
                "task_cancelled" => {
                    refresh.tasks = true;
                    self.note(format!(
                        "task {} cancelled: {}",
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
                id: g["id"].as_str().unwrap_or("").to_string(),
                issuer: g["issuer"].as_str().unwrap_or("").to_string(),
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            self.quit = true;
            return Some(V2Effect::Quit);
        }
        // a pending destructive confirmation swallows keys until resolved
        if self.confirm.is_some() {
            return self.confirm_key(key);
        }
        // View switching is global and uses no function keys (many keyboards lack
        // them): Ctrl+N cycles chat -> instances -> tasks -> topology, Ctrl+A jumps
        // into the approvals box and Esc returns to the chat.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') => {
                    self.view = match self.view {
                        View::Chat => View::Instances,
                        View::Instances => View::Tasks,
                        View::Tasks => View::Topology,
                        View::Topology => View::Chat,
                    };
                    if self.view == View::Instances {
                        self.instance_sel = self.active;
                    }
                    if self.view == View::Chat {
                        self.chat_scroll = 0;
                    }
                    return None;
                }
                KeyCode::Char('a') => {
                    if self.view == View::Chat && !self.approvals.is_empty() {
                        self.focus = Focus::Approvals;
                    }
                    return None;
                }
                _ => {}
            }
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
            // D-363: interrupt the running turn; not destructive, so no confirmation
            KeyCode::Char('i') => {
                let instance = self.instances.get(self.instance_sel)?.id.clone();
                Some(V2Effect::Interrupt { instance })
            }
            // D-269: opening a goal is not destructive, so it needs no confirmation; it moves the instance's
            // goal pointer (D-266), which is exactly what makes later delegation charge to the new goal
            KeyCode::Char('g') => {
                let attach = self.instances.get(self.instance_sel)?.id.clone();
                Some(V2Effect::OpenGoal { attach })
            }
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

    /// Handle a `/command` typed in the composer (D-370). View switches and notes happen in place; the few that
    /// need the main loop return an effect. An unknown command is a note, never a silent drop into the Leader.
    fn slash_command(&mut self, text: &str) -> Option<V2Effect> {
        let command = text.split_whitespace().next().unwrap_or(text);
        match command {
            "/help" => {
                self.command_note(SLASH_HELP);
                None
            }
            "/quit" | "/exit" => {
                self.quit = true;
                None
            }
            "/chat" => {
                self.view = View::Chat;
                None
            }
            "/instances" => {
                self.view = View::Instances;
                self.instance_sel = self.active;
                None
            }
            "/tasks" => {
                self.view = View::Tasks;
                None
            }
            "/topology" => {
                self.view = View::Topology;
                None
            }
            "/approvals" => {
                self.view = View::Chat;
                self.focus = Focus::Approvals;
                None
            }
            "/status" => {
                let status = self.status_line();
                self.command_note(status);
                None
            }
            "/sessions" => {
                self.command_note(self.sessions_report());
                None
            }
            "/switch" => {
                // `/switch <id>` — the same effect `/sessions` tells the user about
                let id = text.split_whitespace().nth(1).unwrap_or("").to_string();
                match self.sessions.iter().find(|session| session.id == id) {
                    Some(session) if !session.active => Some(V2Effect::SwitchSession { id: session.id.clone() }),
                    Some(_) => {
                        self.command_note(format!("/switch {id}: this is the session already attached"));
                        None
                    }
                    None => {
                        self.command_note(format!("/switch {id}: no such session; /sessions lists them"));
                        None
                    }
                }
            }
            "/goals" => {
                let id = text.split_whitespace().nth(2).unwrap_or("").to_string();
                if let Some(verb) = text.split_whitespace().nth(1) {
                    if verb != "cancel" {
                        self.command_note(format!("/goals {verb}: the only verb is `cancel`; /goals lists them"));
                        return None;
                    }
                    if id.is_empty() {
                        self.command_note("/goals cancel needs a goal id; /goals lists them");
                        return None;
                    }
                    return Some(V2Effect::CancelGoal { id });
                }
                Some(V2Effect::RefreshGoals)
            }
            "/mcp" => {
                self.command_note(self.mcp_report());
                None
            }
            "/interrupt" => match self.active_instance() {
                Some(instance) => Some(V2Effect::Interrupt { instance: instance.id.clone() }),
                None => {
                    self.command_note("/interrupt: no instance is selected");
                    None
                }
            },
            _ => {
                self.command_note(format!("unknown command {command:?}; /help lists them"));
                None
            }
        }
    }

    /// What `/sessions` prints (D-429): the named sessions under this base with this one marked, and the command
    /// that switches. A TUI user otherwise has to know an id and pass it on the command line.
    pub fn sessions_report(&self) -> String {
        if self.sessions.is_empty() {
            return "no other sessions under this base: `teamagents sessions new --name NAME` creates one".to_string();
        }
        let mut out = String::from("sessions under this base:\n");
        for session in &self.sessions {
            let marker = if session.active { " (attached)" } else { "" };
            let name = if session.name.is_empty() { "-" } else { session.name.as_str() };
            out.push_str(&format!("  {}{marker}  name: {name}\n", session.id));
        }
        out.push_str("`/switch <id>` attaches this client to another session (its own daemon, started if needed)");
        out
    }

    /// What `/goals` prints (D-433): one line per goal the session carries, with the ones a user acts on named —
    /// the budget it has left, whether an instance is working on it, and what the model said when it settled
    /// (D-431's summary). The checkpoint reports a single goal (D-266), which is why this reads the list.
    pub fn goals_report(&self, rows: &Json) -> String {
        let Some(rows) = rows.as_array().filter(|rows| !rows.is_empty()) else {
            return "this session carries no goals: press `g` on an instance (or run `teamagents goals open \
                    --id NAME`) to open one"
                .to_string();
        };
        let mut out = String::from("goals in this session (newest the runtime is working on first):\n");
        for row in rows {
            let id = row["id"].as_str().unwrap_or("");
            let status = row["status"].as_str().unwrap_or("");
            let attached = row["attached_instances"]
                .as_array()
                .map(|list| list.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let usage = &row["known_usage"];
            let mut line = format!(
                "  {id}  {status}  usage {}{}",
                usage["total"].as_i64().unwrap_or(0),
                row["limits"]["max_total_tokens"].as_i64().map(|limit| format!("/{limit}")).unwrap_or_default()
            );
            if !attached.is_empty() {
                line.push_str(&format!("  attached: {attached}"));
            }
            out.push_str(&line);
            if let Some(summary) = row["summary"].as_str().filter(|text| !text.trim().is_empty()) {
                let collapsed = summary.split_whitespace().collect::<Vec<_>>().join(" ");
                out.push_str(&format!("\n      settled: {}", collapsed.chars().take(100).collect::<String>()));
            }
            out.push('\n');
        }
        out.push_str(
            "`/goals cancel <id>` closes a goal (`g` on an instance opens one); a settled goal's own summary is \
             shown when the model wrote none (D-431)",
        );
        out
    }

    /// What `/mcp` prints (D-413): one line per configured service. The two things a user acts on are here — the
    /// credential state (sign in with `teamagents mcp login --name X`) and the fact that a service bound at boot is
    /// what the session offers (so a credential stored while the session runs needs the instance's driver to be
    /// rebuilt: `instances pause` then `resume`, or `resume` when a required service parked it).
    pub fn mcp_report(&self) -> String {
        if self.mcp.is_empty() {
            return "no MCP services are configured for this session: bind one with \
                    `teamagents mcp add --name N --command CMD` (or `--transport http --url U`) and restart it"
                .to_string();
        }
        let mut out = String::from("MCP services in this session:\n");
        for service in &self.mcp {
            let required = if service.required { "required" } else { "optional" };
            match &service.credential {
                Some(state) => {
                    out.push_str(&format!("  {} ({}, {}, oauth: {state})\n", service.name, service.transport, required))
                }
                None => out.push_str(&format!("  {} ({}, {})\n", service.name, service.transport, required)),
            }
        }
        out.push_str(
            "a service is bound when its instance's driver boots; after `teamagents mcp login --name X`, \
             pause and resume that instance to pick the credential up (`mcp logout --name X` forgets it)",
        );
        out
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
            KeyCode::Tab => {
                if !self.instances.is_empty() {
                    self.active = (self.active + 1) % self.instances.len();
                    self.chat_scroll = 0;
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
            instance.map(|i| format!("{} [{}]", i.id, i.phase)).unwrap_or_else(|| "(no instance)".to_string());
        let goal_part = self
            .goal
            .as_ref()
            .map(|g| {
                let mut text = format!("goal {}", g.status);
                match g.limit_total {
                    Some(limit) => text.push_str(&format!(" · usage {}/{}", g.known_total, limit)),
                    None => text.push_str(&format!(" · usage {}", g.known_total)),
                }
                if g.unknown {
                    text.push_str(" (includes unknown)");
                }
                // D-431: the model's own account of the work, when it settled with a summary — which is the
                // only thing a user can read for a goal the model settled without writing prose.
                if let Some(summary) = &g.summary {
                    text.push_str(&format!(" · {summary}"));
                }
                if let Some(deadline) = g.deadline {
                    let left = deadline - crate::now_epoch();
                    text.push_str(&if left > 0.0 {
                        format!(" · ends in {}m", (left / 60.0) as i64)
                    } else {
                        " · past its deadline".to_string()
                    });
                }
                text
            })
            .unwrap_or_else(|| "no goal".to_string());
        let approvals_part = if self.approvals.is_empty() {
            String::new()
        } else {
            format!(" · {} pending approvals", self.approvals.len())
        };
        let link = if self.disconnected { " · disconnected, reconnecting..." } else { "" };
        let view = self.view.name();
        format!("session {} · view {view} · {instance_part} · {goal_part}{approvals_part}{link}", self.session_id)
    }

    pub fn footer_hint(&self) -> String {
        if self.confirm.is_some() {
            return "terminate this instance? y confirm / n cancel".to_string();
        }
        // a `/` draft is a client command (D-370): show what it may be instead of the send hint
        if self.view == View::Chat && self.focus == Focus::Composer && self.composer.text().starts_with('/') {
            return SLASH_HELP.to_string();
        }
        match self.view {
            View::Chat => match self.focus {
                Focus::Composer => {
                    let approvals = if self.approvals.is_empty() {
                        String::new()
                    } else {
                        format!(" · Ctrl+A approvals({})", self.approvals.len())
                    };
                    // Order is priority: a narrow terminal truncates the tail,
                    // so what a user needs while composing (send, newline, the
                    // pending approvals) comes first; at 80 columns the whole
                    // hint is visible.
                    format!("Enter send · ↑ history · Ctrl+J newline{approvals} · Ctrl+N panels · Tab target")
                }
                Focus::Approvals => "a approve once · d deny · up/down select · Esc back".to_string(),
            },
            View::Instances => {
                // A terminated instance takes no lifecycle command (the control plane
                // refuses them), so the hint stops advertising keys that would answer
                // with a refusal; reading its conversation stays available.
                let terminated =
                    self.instances.get(self.instance_sel).is_some_and(|info| info.lifecycle == "TERMINATED");
                if terminated {
                    "Enter set conversation target · up/down select · Ctrl+N next · Esc back".to_string()
                } else {
                    "Enter set conversation target · p pause · r resume · i interrupt · t terminate · g open the next goal · up/down select · Ctrl+N next · Esc back"
                        .to_string()
                }
            }
            View::Tasks => "c cancel task · up/down select · Ctrl+N next · Esc back".to_string(),
            View::Topology => "up/down scroll · Ctrl+N next · Esc back".to_string(),
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
    format!("{kept}...({count} chars total)")
}
