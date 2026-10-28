//! R2-P4 R19-b② v2 conversation interface state and key handling (plan §9).
//! Pure logic like app.rs: side effects leave as `V2Effect`s executed by the
//! main loop through the daemon client or session CLI, so interaction logic
//! is testable without a terminal. Tools and models run in the engine;
//! history is the authoritative conversation, events are signals,
//! and system notes are UI-level annotations on top.

use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};

use crate::text::Composer;

/// The slash commands the composer understands (D-370), shown while a `/` draft is open and by `/help`.
const SLASH_HELP: &str = "commands: /help · /edit · /sessions · /queue · /interrupt · /model · /quit";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Composer,
    Approvals,
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
    /// The wire model the catalog entry resolves to, for display beside the key.
    pub model: String,
    pub provider: String,
    /// The ordered effort levels this model declares (D-476), or empty for the built-in list.
    pub efforts: Vec<String>,
}

/// The Ctrl+M menu: the Leader's model and reasoning effort, applied at the next turn.
#[derive(Clone, Debug)]
pub struct ProfileMenu {
    pub instance: String,
    pub models: Vec<ModelInfo>,
    pub efforts: Vec<String>,
    pub model_ix: usize,
    pub effort_ix: usize,
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

fn select_model(menu: &mut ProfileMenu, index: usize, fallback: &[String]) {
    let current = menu.efforts.get(menu.effort_ix).cloned().unwrap_or_default();
    menu.model_ix = index;
    menu.efforts = model_efforts(&menu.models[index], fallback);
    menu.effort_ix = effort_index(&menu.efforts, &current);
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
    Queue(crate::queue::Action),
    InputPaths {
        prefix: String,
    },
    /// Open the external editor for this draft (D-454).
    EditDraft {
        initial: String,
    },
    SubmitInput {
        instance: String,
        envelope: String,
        text: String,
        attachments: Vec<String>,
    },
    Decide {
        approval_id: String,
        decision: &'static str,
    },
    /// Interrupt the Leader at the runtime boundary; leaving the TUI is independent.
    Interrupt {
        instance: String,
    },
    /// D-473: change the Leader's model/effort; the control plane applies it at the next READY/WAITING boundary.
    SetProfile {
        instance: String,
        model: Option<String>,
        effort: Option<String>,
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

pub struct V2App {
    pub session_root: String,
    pub session_name: String,
    pub browser: Option<crate::sessions::SessionBrowser>,
    pub queue: Option<crate::queue::Queue>,
    /// D-473: the daemon's `[models]` catalog and the open profile menu.
    pub models: Vec<ModelInfo>,
    pub efforts: Vec<String>,
    pub profile: Option<ProfileMenu>,
    pub help_open: bool,
    pub help_scroll: usize,
    pub session_id: String,
    pub instances: Vec<InstanceInfo>,
    /// Read-only checkpoint observations, never instructions to the model.
    work_diagnostics: Vec<(String, Json)>,
    environment: Json,
    pending_input_receipts: HashSet<String>,
    pub entries: Vec<ChatEntry>,
    pub transcript: crate::transcript::Transcript,
    pub graphics: crate::graphics::Graphics,
    pub pictures: Vec<crate::graphics::Placement>,
    pub expanded_tools: bool,
    pub goal: Option<GoalInfo>,
    pub approvals: Vec<ApprovalInfo>,
    pub approval_sel: usize,
    pub focus: Focus,
    pub composer: Composer,
    pub completion: crate::completion::Completion,
    pub attachments: crate::attachments::Attachments,
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
            queue: None,
            models: vec![],
            efforts: default_efforts(),
            profile: None,
            help_open: false,
            help_scroll: 0,
            session_id: session_id.to_string(),
            instances: vec![],
            work_diagnostics: vec![],
            environment: Json::Null,
            pending_input_receipts: HashSet::new(),
            entries: vec![],
            transcript: crate::transcript::Transcript::default(),
            graphics: crate::graphics::Graphics::default(),
            pictures: vec![],
            expanded_tools: false,
            goal: None,
            approvals: vec![],
            approval_sel: 0,
            focus: Focus::Composer,
            composer: Composer::new(vec![]),
            completion: crate::completion::Completion::default(),
            attachments: crate::attachments::Attachments::default(),
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

    fn note_error(&mut self, text: impl Into<String>) {
        self.entries.push(ChatEntry {
            kind: ChatKind::Error,
            who: "error".into(),
            text: text.into(),
            tool: None,
            workspace: String::new(),
            media: vec![],
        });
    }

    // ---- daemon → app application points --------------------------------

    /// Checkpoint (connect/reconnect): snapshot state and the watermark the
    /// event stream resumes from — one consistent read (§9).
    pub fn apply_checkpoint(&mut self, snapshot: Json, watermark: i64) {
        self.environment = snapshot["environment"].clone();
        for row in snapshot["input_delivery"].as_array().into_iter().flatten() {
            let Some(envelope) = row["id"].as_str() else { continue };
            let state = match row["state"].as_str() {
                Some("APPLIED") => "received into the Leader's context (not a completion verdict)",
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
                matches!(row["state"].as_str(), Some("ACCEPTED" | "DEFERRED")),
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
                summary: g["summary"]
                    .as_str()
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| text.trim().to_string()),
            })
        });
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
                        model: row["model"].as_str().unwrap_or(&key).to_string(),
                        provider: row["provider"].as_str().unwrap_or("").to_string(),
                        key,
                        efforts,
                    })
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
            "i-leader",
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

    /// Settle an external-editor attempt. The `/edit` suffix path takes the
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

    /// D-479: retain the acknowledged envelope through refresh/reconnect. History or a checkpoint,
    /// never a timer, changes 'queued' into 'received'. Retry identity remains owned by Attachments.
    pub fn input_receipt(&mut self, envelope: &str, reply: &Json) {
        let state = if reply["deferred"] == true {
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
            self.command_note(text);
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
        self.note_error(format!("the approval decision failed: {error}"));
    }

    // ---- keys ------------------------------------------------------------

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::{KeyCode, KeyModifiers};
        if key.kind == crossterm::event::KeyEventKind::Release {
            return None;
        }
        if self.help_open {
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
            if key.modifiers.contains(KeyModifiers::CONTROL) {
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
                    KeyCode::Char('m') => {
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
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
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
        match self.focus {
            Focus::Approvals => self.approval_key(key),
            Focus::Composer => self.composer_key(key),
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

    /// D-473: the Leader's model/effort menu. The catalog comes from the checkpoint; a daemon that does not
    /// report one still offers the current model so the effort can be changed.
    fn open_profile(&mut self) -> Option<V2Effect> {
        let Some(leader) = self.active_instance().cloned() else {
            self.command_note("No Leader is connected, so there is no profile to change.");
            return None;
        };
        let mut models = self.models.clone();
        if models.is_empty() {
            models.push(ModelInfo {
                key: leader.model.clone(),
                model: leader.model.clone(),
                provider: String::new(),
                efforts: vec![],
            });
        }
        let model_ix =
            models.iter().position(|model| model.key == leader.model || model.model == leader.model).unwrap_or(0);
        let efforts = model_efforts(&models[model_ix], &self.efforts);
        let effort_ix = effort_index(&efforts, &leader.effort);
        self.profile = Some(ProfileMenu { instance: leader.id, models, efforts, model_ix, effort_ix });
        None
    }

    fn profile_key(&mut self, key: crossterm::event::KeyEvent) -> Option<V2Effect> {
        use crossterm::event::KeyCode;
        let fallback = self.efforts.clone();
        let menu = self.profile.as_mut()?;
        match key.code {
            KeyCode::Esc => {
                self.profile = None;
                None
            }
            KeyCode::Up => {
                let index = (menu.model_ix + menu.models.len() - 1) % menu.models.len();
                select_model(menu, index, &fallback);
                None
            }
            KeyCode::Down => {
                let index = (menu.model_ix + 1) % menu.models.len();
                select_model(menu, index, &fallback);
                None
            }
            KeyCode::Left => {
                menu.effort_ix = (menu.effort_ix + menu.efforts.len() - 1) % menu.efforts.len();
                None
            }
            KeyCode::Right => {
                menu.effort_ix = (menu.effort_ix + 1) % menu.efforts.len();
                None
            }
            KeyCode::Enter => {
                let menu = self.profile.take()?;
                let model = menu.models.get(menu.model_ix)?.key.clone();
                let effort = menu.efforts.get(menu.effort_ix)?.clone();
                Some(V2Effect::SetProfile { instance: menu.instance, model: Some(model), effort: Some(effort) })
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
        if let Some(path) = text.strip_prefix("/attach ") {
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
        if let Some(index) = text.strip_prefix("/detach ") {
            match index.trim().parse::<usize>().ok().filter(|i| *i > 0 && *i <= self.attachments.paths.len()) {
                Some(index) => {
                    let path = self.attachments.paths.remove(index - 1);
                    self.command_note(format!("Attachment removed from draft: {path:?}"));
                }
                None => self.command_note("Use /detach INDEX with a staged attachment's one-based index."),
            }
            return None;
        }
        match text.trim() {
            "/help" => self.command_note(format!(
                "Session {} · model {}
Enter sends to the Leader; Ctrl+J or Shift+Enter adds a newline.
Up/Down recalls input; PageUp/PageDown or the wheel reads history.
? on an empty draft opens shortcuts; Esc closes them.
Ctrl+Left/Right or Alt+Left/Right moves by word; Ctrl+B/F moves by character.
Ctrl+W or Alt+Backspace deletes a word; Ctrl+U/K deletes to line start/end; Ctrl+Y restores the last deletion.
Home/End moves to line start/end; Ctrl+E opens the external editor.
A slash prefix shows commands: Up/Down selects, Tab/Enter fills, Esc cancels; Enter submits the filled command.
Ctrl+C or /interrupt cancels the Leader's model request; running tools and delegated work continue.
Ctrl+A focuses pending approvals; a approves once, d denies, Esc returns.
Ctrl+Q or /quit quits the TUI; the session keeps running.
Cancelling a task does not stop its command. To stop executing work, use teamagents instances terminate --id INSTANCE --yes.
Keep the same --state-root/--session; SSH remote processes may need separate cleanup.
Ctrl+S or /sessions opens session history, switching and confirmed deletion; / searches stored messages.
Ctrl+P or /queue shows pending Leader inputs; Enter edits, Delete asks to cancel, Esc returns to the draft.
Ctrl+M or /model changes the Leader's model and reasoning effort at its next turn.
/attach PATH stages a workspace file; Tab lists paths; Enter fills, another Enter stages. /detach INDEX removes it.
Attachments: eight files, 1 MiB UTF-8 text or 5 MiB PNG/JPEG/GIF/WebP, 16 MiB total; symlinks are refused.
Ctrl+O expands or folds tool previews; /sessions shows full stored results.
Ctrl+G switches graphics previews and source; image files stay unchanged.
Developer tools: teamagents --help; teamagents status --json; teamagents history --id ID.
{}
{}",
                self.session_id,
                self.active_instance().map(|i| i.model.as_str()).unwrap_or("unavailable"),
                self.environment_details(),
                self.diagnostic_details()
            )),
            "/attach" => self.command_note(format!(
                "Staged attachments: {:?}. Use /attach PATH; Tab lists workspace paths.",
                self.attachments.paths
            )),
            "/edit" => self
                .command_note("Ctrl+E edits the current draft; type /edit after a draft to open it in VISUAL/EDITOR."),
            "/sessions" => return self.open_sessions(),
            "/queue" => return self.open_queue(),
            "/interrupt" => return self.interrupt(),
            "/model" => return self.open_profile(),
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
        if key.code == KeyCode::Enter && key.kind == crossterm::event::KeyEventKind::Repeat {
            return None;
        }
        if key.code == KeyCode::Char('?')
            && !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && self.composer.text().is_empty()
        {
            self.help_open = true;
            self.help_scroll = 0;
            return None;
        }
        if self.attachments.key(&mut self.composer, key) {
            return None;
        }
        if key.code == KeyCode::Tab && key.modifiers.is_empty() {
            if let Some(prefix) = self.composer.text().strip_prefix("/attach ") {
                return Some(V2Effect::InputPaths { prefix: prefix.to_string() });
            }
        }
        if self.completion.key(&mut self.composer, key) {
            return None;
        }
        if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(V2Effect::EditDraft { initial: self.composer.text() });
        }
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
                let raw = self.composer.text();
                if let Some(initial) = raw.strip_suffix("/edit") {
                    if !initial.trim().is_empty() {
                        self.composer.clear();
                        return Some(V2Effect::EditDraft { initial: initial.trim_end().to_string() });
                    }
                }
                if !self.composer.text().starts_with('/') && (self.disconnected || self.active_instance().is_none()) {
                    self.command_note("Message kept in the draft: waiting for the Leader connection.");
                    return None;
                }
                let text = {
                    let raw = self.composer.text();
                    if raw == "/attach"
                        || raw == "/detach"
                        || raw == "/edit"
                        || raw.starts_with("/attach ")
                        || raw.starts_with("/detach ")
                    {
                        // Client commands never enter the input history (same for attach and detach).
                        self.composer.clear();
                        raw
                    } else {
                        self.composer.submit()?
                    }
                };
                // D-370: a leading `/` is a client command, not an instruction to the Leader (pi's slash-command
                // shape). Handled here, before the active instance is required, so `/help` works with no instance.
                // Client commands are not sent messages, so they never enter the input history; D-77's
                // "the arrows walk what was sent" records ordinary submissions only, below.
                if text.starts_with('/') {
                    return self.slash_command(&text);
                }
                self.composer.record_submission(&text);
                let instance = self.active_instance()?.id.clone();
                let envelope = self.attachments.envelope(&text);
                Some(V2Effect::SubmitInput { instance, envelope, text, attachments: self.attachments.paths.clone() })
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

    fn environment_details(&self) -> String {
        if !self.environment.is_object() {
            return "Execution environment: not reported by this daemon; use teamagents doctor.".into();
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
            lines.push(
                "Configuration comparison unavailable; inspect teamagents status --json for affected paths.".into(),
            );
        }
        lines.push(
            "Boot-selected sources (contents read per turn; skills on demand, not proof of script availability):"
                .into(),
        );
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
                "Last Leader prompt assembly: epoch {}, observed at {} (not a provider receipt).",
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
            lines.push("No instruction load observed in the current Leader driver lifetime.".into());
        }
        lines.push("teamagents doctor shows current source readability/refusals; doctor environment explicitly probes host and configured tools, without repairs.".into());
        lines.join("\n")
    }

    fn diagnostic_details(&self) -> String {
        let mut lines =
            vec!["Snapshot diagnostics (read only; use the same --state-root/--session in CLI commands):".to_string()];
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
        lines.push("INSTANCE/TASK mean the row ids. Full diagnostics: teamagents instances; teamagents tasks; teamagents status --json. No automatic retry or cancellation.".into());
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
            return format!("{} · {} · teamagents instances", leader.lifecycle, leader.reason);
        }
        if let Some((id, diagnostic)) = self.work_diagnostics.iter().find(|(_, d)| d["attention"] == true) {
            return format!(
                "{id}: {} · /help",
                diagnostic["code"].as_str().unwrap_or("inspect status").replace('_', " ")
            );
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
        let wait = self
            .work_diagnostics
            .iter()
            .find(|(id, d)| id == "i-leader" && d["code"] == "waiting")
            .map(|(_, d)| format!(" · {} · /help", d["summary"].as_str().unwrap_or("")))
            .unwrap_or_default();
        format!("{state}{usage}{deadline}{wait}")
    }

    pub fn footer_hint(&self) -> String {
        if self.help_open {
            return "Esc close · Up/Down scroll · PgUp/PgDn page".into();
        }
        if self.profile.is_some() {
            return "Enter apply · Esc cancel · Up/Down model · Left/Right effort".into();
        }
        if self.focus == Focus::Approvals {
            return "a approve once · d deny · Up/Down select · Esc back".into();
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
        if self.active_instance().is_some_and(|i| i.lifecycle == "ACTIVE" && i.phase != "READY") {
            return "Ctrl+C interrupt · Enter send · Ctrl+P queue · Ctrl+Q quit".into();
        }
        if self.composer.text().is_empty() {
            "Enter send · ? shortcuts · Ctrl+J newline · Ctrl+M model · Ctrl+S sessions · Ctrl+Q quit".into()
        } else {
            "Enter send · Ctrl+J newline · Ctrl+E editor · Ctrl+Q quit · /help".into()
        }
    }
}
