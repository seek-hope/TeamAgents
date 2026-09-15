//! The single serialized entry point for team transactions per session:
//! submit → validate → reduce → persist → schedule, one SQLite transaction per action.

use crate::models::*;
use crate::storage::Store;
use crate::views;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashSet;

pub const BUILTIN_TOOL_BINDINGS: &[&str] = &["files", "shell", "web", "skills"];

#[derive(Debug, Clone)]
pub struct EventDraft {
    pub kind: EventKind,
    pub payload: Json,
    pub task_id: Option<String>,
    pub targets: Option<Vec<String>>,
    pub actor_id: Option<String>,
    pub audience: Option<Vec<String>>,
    pub push: Option<Vec<String>>,
}

impl EventDraft {
    pub fn new(kind: EventKind, payload: Json) -> Self {
        Self { kind, payload, task_id: None, targets: None, actor_id: None, audience: None, push: None }
    }
}

pub struct Reduction {
    pub events: Vec<EventDraft>,
    pub receipt: Receipt,
}

pub struct Control {
    pub store: Store,
    pub session_id: String,
    pub catalog: UserConfig,
    /// (run_id, inbox items) recorded when a running member receives mid-turn input
    pub mid_turn_pushes: Vec<(String, Vec<Json>)>,
}

// -- payload helpers ----------------------------------------------------------

fn pget<'a>(p: &'a Json, key: &str) -> Option<&'a Json> {
    p.get(key).filter(|v| !v.is_null())
}

fn pstr(p: &Json, key: &str) -> String {
    match p.get(key) {
        Some(Json::String(s)) => s.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => String::new(),
    }
}

fn plist(p: &Json, key: &str) -> Vec<String> {
    p.get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn pbool(p: &Json, key: &str) -> bool {
    p.get(key).map(|v| v.as_bool().unwrap_or(!v.is_null() && *v != Json::from(0))).unwrap_or(false)
}

/// Wire-JSON truthiness for `payload.get(k) or payload.get(j)` style checks.
fn truthy(v: Option<&Json>) -> bool {
    match v {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(Json::String(s)) => !s.is_empty(),
        Some(Json::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Json::Array(a)) => !a.is_empty(),
        Some(Json::Object(o)) => !o.is_empty(),
    }
}

/// Strict integer coercion over wire JSON: numbers truncate, bools are 0/1,
/// integer strings parse; anything else is None (callers turn that into an error).
fn py_int(v: &Json) -> Option<i64> {
    match v {
        Json::Bool(b) => Some(*b as i64),
        Json::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)),
        Json::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Deterministic per action id.
pub fn derived_task_id(action: &TeamAction) -> String {
    if let Some(explicit) = action.payload.get("task_id").and_then(|v| v.as_str()) {
        return explicit.to_string();
    }
    format!("task_{}", hex_prefix(&Sha256::digest(action.action_id.as_bytes()), 12))
}

/// sha256 of the canonical (sorted) payload json.
pub fn payload_hash(action: &TeamAction) -> String {
    hex_prefix(&Sha256::digest(canonical_json(&action.payload).as_bytes()), 32)
}

/// Canonical payload JSON: sorted keys, `", "` / `": "` separators.
fn canonical_json(v: &Json) -> String {
    match v {
        Json::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}: {}", serde_json::to_string(k).unwrap(), canonical_json(&m[k])))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Json::Array(a) => {
            let inner: Vec<String> = a.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(", "))
        }
        other => other.to_string(),
    }
}

fn hex_prefix(bytes: &[u8], n: usize) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()[..n].to_string()
}

impl Control {
    pub fn new(store: Store, session_id: impl Into<String>) -> Self {
        Self { store, session_id: session_id.into(), catalog: UserConfig::default(), mid_turn_pushes: vec![] }
    }

    /// One re-entrant transaction: BEGIN IMMEDIATE at depth 0,
    /// COMMIT on success, ROLLBACK on error. Store failures are returned, never
    /// swallowed.
    fn in_tx<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        // pushes queued by earlier committed txs may still be undrained (drain is a
        // separate RPC); a rollback here must only drop what this tx added
        let mark = self.mid_turn_pushes.len();
        self.store.begin().map_err(|e| e.to_string())?;
        match f(self) {
            Ok(v) => match self.store.commit() {
                Ok(()) => Ok(v),
                Err(e) => {
                    let _ = self.store.rollback();
                    self.mid_turn_pushes.truncate(mark);
                    Err(e.to_string())
                }
            },
            Err(e) => {
                let _ = self.store.rollback();
                self.mid_turn_pushes.truncate(mark);
                Err(e)
            }
        }
    }

    // ------------------------------------------------------------------ submit

    /// A failed attempt rolls back every write it
    /// made; the failure receipt then commits in a clean transaction.
    /// Err means even that receipt could not be recorded (store still busy).
    pub fn submit(&mut self, action: &TeamAction) -> Result<Receipt, String> {
        match self.submit_in_tx(action) {
            Ok(r) => Ok(r),
            Err(e) => {
                let _ = self.store.rollback();
                let receipt = Receipt::failure(action, e);
                let recorded = self.in_tx(|ctl| {
                    if let Some(prior) = ctl
                        .store
                        .get_action_receipt(&action.action_id)
                        .map_err(|e| e.to_string())?
                    {
                        return Ok(Some(prior));
                    }
                    ctl.store
                        .record_action(
                            &action.action_id, &ctl.session_id, &action.actor_id,
                            action.run_id.as_deref(), action.kind, &payload_hash(action), &receipt,
                        )
                        .map_err(|e| e.to_string())?;
                    Ok(None)
                });
                match recorded {
                    Ok(Some(prior)) => Ok(prior),
                    Ok(None) => Ok(receipt),
                    Err(e) => Err(e),
                }
            }
        }
    }

    fn submit_in_tx(&mut self, action: &TeamAction) -> Result<Receipt, String> {
        self.in_tx(|ctl| {
            if let Some(prior) = ctl.store.get_action_receipt(&action.action_id).map_err(|e| e.to_string())? {
                return Ok(prior);
            }
            let spec = ctl.store.load_team_spec(&ctl.session_id.clone(), None).map_err(|e| e.to_string())?;
            if let Some(error) = ctl.validate(action, &spec) {
                let receipt = Receipt::failure(action, error);
                ctl.store.record_action(
                    &action.action_id, &ctl.session_id, &action.actor_id,
                    action.run_id.as_deref(), action.kind, &payload_hash(action), &receipt,
                ).map_err(|e| e.to_string())?;
                return Ok(receipt);
            }
            let reduction = ctl.reduce(action, &spec)?;
            ctl.persist_events(action, &spec, &reduction.events)?;
            ctl.schedule_inner(&spec)?;
            ctl.store.record_action(
                &action.action_id, &ctl.session_id, &action.actor_id,
                action.run_id.as_deref(), action.kind, &payload_hash(action), &reduction.receipt,
            ).map_err(|e| e.to_string())?;
            Ok(reduction.receipt)
        })
    }

    /// Re-run scheduling against committed state.
    pub fn schedule(&mut self) -> Result<(), String> {
        self.in_tx(|ctl| {
            let spec = ctl.store.load_team_spec(&ctl.session_id.clone(), None).map_err(|e| e.to_string())?;
            ctl.schedule_inner(&spec)
        })
    }

    /// Persist runtime-originated events and schedule.
    pub fn emit(&mut self, drafts: Vec<EventDraft>, actor_id: &str) -> Result<(), String> {
        self.in_tx(|ctl| {
            let spec = ctl.store.load_team_spec(&ctl.session_id.clone(), None).map_err(|e| e.to_string())?;
            let action = TeamAction {
                action_id: new_id("sys"),
                session_id: ctl.session_id.clone(),
                actor_id: actor_id.to_string(),
                run_id: None,
                kind: ActionKind::SendMessage,
                payload: json!({}),
            };
            ctl.persist_events(&action, &spec, &drafts)?;
            ctl.schedule_inner(&spec)
        })
    }

    // -------------------------------------------------------------- validation

    /// Space ids this member can reach: named in errors so a wrong id is
    /// corrected in one round instead of guessed again. Only spaces the actor
    /// may actually use, so the hint cannot leak other teams' space names.
    fn reachable_space_ids<'a>(spec: &'a TeamSpec, actor: &str) -> Vec<&'a str> {
        spec.shared_spaces
            .iter()
            .filter(|s| s.readers.iter().any(|r| r == actor) || s.writers.iter().any(|w| w == actor))
            .map(|s| s.id.as_str())
            .collect()
    }

    /// Returns Some(error) on refusal.
    pub fn validate(&mut self, action: &TeamAction, spec: &TeamSpec) -> Option<String> {
        let kind = action.kind;
        let member_ids: HashSet<&str> = spec.agents.iter().map(|a| a.id.as_str()).collect();
        let actor = action.actor_id.as_str();
        let p = &action.payload;

        if matches!(kind, ActionKind::UserMessage | ActionKind::UserSupplement) {
            if actor != "user" {
                return Some("only the local user can submit user input".into());
            }
            if pstr(p, "text").trim().is_empty() {
                return Some("user input text must not be empty".into());
            }
            return None;
        }

        let user_kinds = [
            ActionKind::CancelTask,
            ActionKind::CancelRun,
            ActionKind::ApprovalDecision,
            ActionKind::SetPermissionMode,
            ActionKind::PauseSession,
        ];
        if actor == "user" {
            if !user_kinds.contains(&kind) {
                return Some(format!("the local user cannot submit {}", enum_name(kind)));
            }
        } else if !member_ids.contains(actor) {
            return Some(format!("actor {actor:?} is not a team member"));
        }
        if member_ids.contains(actor) && action.run_id.is_some() {
            let run_id = action.run_id.as_deref().unwrap();
            match self.store.get_run(run_id).ok().flatten() {
                None => return Some(format!("unknown run {run_id:?}")),
                Some(run) if run.agent_id != actor => {
                    return Some("run does not belong to the acting member".into())
                }
                _ => {}
            }
        }

        match kind {
            ActionKind::SendMessage => {
                let target = p.get("target").and_then(|v| v.as_str());
                if target == Some("*") {
                    let has_broadcast = spec.channels.iter().any(|ch| ch.source == actor && ch.mode == ChannelMode::Broadcast);
                    return if has_broadcast { None } else { Some(format!("{actor:?} has no broadcast channel")) };
                }
                match target {
                    Some(t) if member_ids.contains(t) => {
                        if spec.can_send(actor, t) {
                            None
                        } else {
                            // self-healing: name the reachable members instead of
                            // letting the model guess targets (or invent channels)
                            let mut reachable: Vec<&str> = member_ids.iter().copied().filter(|m| spec.can_send(actor, m)).collect();
                            reachable.sort();
                            Some(format!(
                                "{actor:?} is not allowed to message {t:?}: no channel covers this direction; reachable now: {reachable:?}"
                            ))
                        }
                    }
                    other => Some(format!("unknown message target {other:?}")),
                }
            }
            ActionKind::AssignTask => {
                let assignee = p.get("assignee").and_then(|v| v.as_str()).unwrap_or("");
                if !member_ids.contains(assignee) {
                    return Some(format!("unknown assignee {assignee:?}"));
                }
                if !spec.can_delegate(actor, assignee) {
                    return Some(format!("{actor:?} is not allowed to assign tasks to {assignee:?}"));
                }
                if spec.agent(assignee).map(|a| a.runtime_kind) == Some(RuntimeKind::Codex) && actor != spec.leader_id {
                    return Some("codex members can only be delegated to by the Leader".into());
                }
                if pstr(p, "description").trim().is_empty() {
                    return Some("task description must not be empty".into());
                }
                let deps = plist(p, "dependencies");
                for dep in &deps {
                    if self.store.get_task(dep).ok().flatten().is_none() {
                        return Some(format!("unknown dependency task {dep:?}"));
                    }
                }
                if self.dependency_cycle(&derived_task_id(action), &deps) {
                    return Some("task dependencies would form a cycle".into());
                }
                None
            }
            ActionKind::CompleteTask => {
                if action.run_id.is_none() {
                    return Some("complete_task requires an active run".into());
                }
                let tid = p.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
                match self.store.get_task(tid).ok().flatten() {
                    None => Some(format!("unknown task {tid:?}")),
                    Some(task) if task.assignee != actor => {
                        Some("only the current assignee can complete a task".into())
                    }
                    Some(task) if !matches!(task.status, TaskStatus::Pending | TaskStatus::Running) => {
                        // self-healing: a BLOCKED task (usually an interrupted turn) has
                        // exactly one way out, and the assignee is not the one who takes it
                        Some(match task.status {
                            TaskStatus::Blocked => format!(
                                "task is BLOCKED (its turn was interrupted): ask the Leader to cancel_task {:?} and assign the work again",
                                task.task_id
                            ),
                            other => format!("task is {}, cannot complete", enum_name(other)),
                        })
                    }
                    _ => None,
                }
            }
            ActionKind::WaitForTasks => {
                if action.run_id.is_none() {
                    return Some("wait_for_tasks requires an active run".into());
                }
                for tid in plist(p, "task_ids") {
                    if self.store.get_task(&tid).ok().flatten().is_none() {
                        return Some(format!("unknown task {tid:?}"));
                    }
                }
                None
            }
            ActionKind::PublishShared => {
                let space_id = p.get("space_id").and_then(|v| v.as_str()).unwrap_or("");
                let Some(space) = spec.space(space_id) else {
                    return Some(format!("unknown shared space {space_id:?}; available: {:?}", Self::reachable_space_ids(spec, actor)));
                };
                if !space.writers.iter().any(|w| w == actor) {
                    return Some(format!("{actor:?} has no write access to shared space {space_id:?}"));
                }
                if !truthy(pget(p, "content")) && !truthy(pget(p, "ref")) {
                    return Some("shared entry needs content or a ref".into());
                }
                None
            }
            ActionKind::ReadShared | ActionKind::ListShared => {
                if let Some(sid) = p.get("space_id").and_then(|v| v.as_str()) {
                    let Some(space) = spec.space(sid) else {
                        return Some(format!("unknown shared space {sid:?}; available: {:?}", Self::reachable_space_ids(spec, actor)));
                    };
                    if !space.readers.iter().any(|r| r == actor) && !space.writers.iter().any(|w| w == actor) {
                        return Some(format!("{actor:?} has no read access to shared space {sid:?}"));
                    }
                }
                None
            }
            ActionKind::RequestHelp => {
                if pstr(p, "message").trim().is_empty() {
                    return Some("help request must include a message".into());
                }
                None
            }
            ActionKind::ProposeTeamChange => {
                let ops = p.get("operations").and_then(|v| v.as_array());
                match ops {
                    Some(ops) if !ops.is_empty() => None,
                    _ => Some("proposal needs a non-empty operations list".into()),
                }
            }
            ActionKind::ApplyTopologyPatch => {
                if actor != spec.leader_id {
                    return Some("only the Leader can apply topology patches".into());
                }
                if let Some(patch_id) = p.get("patch_id").and_then(|v| v.as_str()) {
                    let Some(patch) = self.store.get_patch(patch_id).ok().flatten() else {
                        return Some(format!("unknown patch {patch_id:?}"));
                    };
                    // a WAITING_BOUNDARY patch may still be rejected; accepting it is
                    // pointless (the boundary applies it once members go idle)
                    let decidable = matches!(patch.status, PatchStatus::Proposed | PatchStatus::Accepted)
                        || (pbool(p, "reject") && patch.status == PatchStatus::WaitingBoundary);
                    if !decidable {
                        return Some(format!("patch is {}, cannot decide", enum_name(patch.status)));
                    }
                    if !pbool(p, "reject")
                        && patch.base_revision != self.store.current_revision(&self.session_id).unwrap_or(0)
                    {
                        return Some(format!(
                            "patch base_revision {} is stale; propose again against the current revision",
                            patch.base_revision
                        ));
                    }
                    return None;
                }
                let ops = p.get("operations").and_then(|v| v.as_array());
                if ops.map(|o| o.is_empty()).unwrap_or(true) {
                    return Some("patch needs a non-empty operations list".into());
                }
                let base = p.get("base_revision").and_then(|v| v.as_i64());
                let current = self.store.current_revision(&self.session_id).unwrap_or(0);
                if base != Some(current) {
                    return Some(match base {
                        // an absent base_revision is a retryable authoring mistake,
                        // not a stale patch: tell the caller the exact value to send
                        None => format!("patch needs base_revision {current} (from <team revision=...>); resend with it"),
                        Some(stale) => format!("patch base_revision {stale} is stale; current is {current}"),
                    });
                }
                None
            }
            ActionKind::SignalDone => {
                if actor != spec.leader_id {
                    return Some("only the Leader can signal goal completion".into());
                }
                if action.run_id.is_none() {
                    return Some("signal_done requires an active run".into());
                }
                None
            }
            ActionKind::CancelTask => {
                if actor != spec.leader_id && actor != "user" {
                    return Some("only the Leader or the user can cancel tasks".into());
                }
                let tid = p.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
                if self.store.get_task(tid).ok().flatten().is_none() {
                    return Some(format!("unknown task {tid:?}"));
                }
                None
            }
            ActionKind::CancelRun => {
                if actor != spec.leader_id && actor != "user" {
                    return Some("only the Leader or the user can cancel runs".into());
                }
                let rid = p.get("run_id").and_then(|v| v.as_str()).unwrap_or("");
                let Some(run) = self.store.get_run(rid).ok().flatten() else {
                    return Some(format!("unknown run {rid:?}"));
                };
                // An OUTCOME_UNKNOWN run is a turn that was interrupted mid-command:
                // nothing is running any more, and a human decides whether its side
                // effects are acceptable. Cancelling it is that decision.
                if matches!(run.status, TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled) {
                    return Some(format!("run {rid:?} already ended as {}", enum_name(run.status)));
                }
                None
            }
            ActionKind::ApprovalDecision => {
                if actor != "user" {
                    return Some("only the local user can decide approvals".into());
                }
                let aid = p.get("approval_id").and_then(|v| v.as_str()).unwrap_or("");
                let Some(req) = self.store.get_approval(aid).ok().flatten() else {
                    return Some(format!("unknown approval {aid:?}"));
                };
                if req.status != ApprovalStatus::Pending {
                    return Some(format!("approval is already {}", enum_name(req.status)));
                }
                let decision = p.get("decision").and_then(|v| v.as_str());
                if !matches!(decision, Some("once") | Some("session") | Some("deny")) {
                    return Some("decision must be one of: once, session, deny".into());
                }
                None
            }
            ActionKind::SetPermissionMode => {
                if actor != "user" {
                    return Some("only the local user can change the permission mode".into());
                }
                let mode = p.get("mode").and_then(|v| v.as_str());
                if !matches!(mode, Some("approved_scope") | Some("full_auto")) {
                    return Some("mode must be approved_scope or full_auto".into());
                }
                None
            }
            ActionKind::PauseSession => {
                if actor != "user" {
                    return Some("only the local user can pause the session".into());
                }
                None
            }
            other => Some(format!("unsupported action kind {}", enum_name(other))),
        }
    }

    fn dependency_cycle(&self, new_task_id: &str, deps: &[String]) -> bool {
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = deps.to_vec();
        while let Some(tid) = stack.pop() {
            if tid == new_task_id {
                return true;
            }
            if !seen.insert(tid.clone()) {
                continue;
            }
            if let Ok(Some(t)) = self.store.get_task(&tid) {
                stack.extend(t.dependencies);
            }
        }
        false
    }

    // ------------------------------------------------------------------ reduce

    fn reduce(&mut self, action: &TeamAction, spec: &TeamSpec) -> Result<Reduction, String> {
        let kind = action.kind;
        let actor = action.actor_id.clone();
        let p = &action.payload;
        let ok = |result: Json| Receipt::success(action, result);

        match kind {
            ActionKind::UserMessage | ActionKind::UserSupplement => {
                // user input resumes a paused session (plan §9.3)
                if let Some(session) = self.store.get_session(&self.session_id).map_err(|e| e.to_string())? {
                    if session.get("status").and_then(|v| v.as_str()) == Some("PAUSED") {
                        self.store.set_session_status(&self.session_id, SessionStatus::Active).map_err(|e| e.to_string())?;
                    }
                }
                let goal_id = self.ensure_goal(spec)?;
                let text = pstr(p, "text");
                Ok(Reduction {
                    events: vec![EventDraft {
                        kind: EventKind::UserMessage,
                        payload: json!({"text": text, "goal_id": goal_id,
                                        "supplement": kind == ActionKind::UserSupplement,
                                        "to": spec.leader_id}),
                        actor_id: Some("user".into()),
                        ..EventDraft::new(EventKind::UserMessage, json!({}))
                    }],
                    receipt: ok(json!({"received": true, "goal_id": goal_id})),
                })
            }

            ActionKind::SendMessage => {
                let target = p.get("target").and_then(|v| v.as_str());
                let targets: Vec<String> = if target == Some("*") {
                    spec.agents.iter().map(|a| a.id.clone()).filter(|id| id != &actor).collect()
                } else {
                    target.map(|t| vec![t.to_string()]).unwrap_or_default()
                };
                let targets: Vec<String> = targets
                    .into_iter()
                    .filter(|t| t != &actor && spec.can_send(&actor, t))
                    .collect();
                let text = pstr(p, "text");
                Ok(Reduction {
                    events: targets
                        .iter()
                        .map(|t| EventDraft {
                            kind: EventKind::Message,
                            payload: json!({"text": text, "from": actor, "target": t}),
                            targets: Some(vec![t.clone()]),
                            ..EventDraft::new(EventKind::Message, json!({}))
                        })
                        .collect(),
                    receipt: ok(json!({"delivered_to": targets})),
                })
            }

            ActionKind::AssignTask => {
                let deps = plist(p, "dependencies");
                let task = Task {
                    task_id: derived_task_id(action),
                    parent_task_id: p.get("parent_task_id").and_then(|v| v.as_str()).map(str::to_string),
                    goal_id: self.current_goal_id()?,
                    requester: actor.clone(),
                    assignee: pstr(p, "assignee"),
                    description: pstr(p, "description"),
                    acceptance: pstr(p, "acceptance"),
                    dependencies: deps.clone(),
                    status: TaskStatus::Pending,
                    result_refs: vec![],
                    created_at: now(),
                    updated_at: now(),
                };
                self.store.insert_task(&self.session_id, &task).map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![EventDraft {
                        kind: EventKind::TaskCreated,
                        payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                        "requester": task.requester, "description": task.description,
                                        "acceptance": task.acceptance, "dependencies": deps,
                                        "status": "PENDING"}),
                        task_id: Some(task.task_id.clone()),
                        ..EventDraft::new(EventKind::TaskCreated, json!({}))
                    }],
                    receipt: ok(json!({"task_id": task.task_id})),
                })
            }

            ActionKind::CompleteTask => {
                let task_id = pstr(p, "task_id");
                let result_refs = plist(p, "result_refs");
                let summary = pstr(p, "summary");
                self.store
                    .record_completion_request(action.run_id.as_deref().unwrap_or(""), &task_id, &result_refs, &summary)
                    .map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![],
                    receipt: ok(json!({"recorded": true, "task_id": task_id, "applies_at": "turn_end"})),
                })
            }

            ActionKind::WaitForTasks => {
                let task_ids = plist(p, "task_ids");
                let pending: Vec<String> = task_ids
                    .iter()
                    .filter(|t| {
                        matches!(
                            self.store.get_task(t).ok().flatten().map(|t| t.status),
                            Some(TaskStatus::Pending | TaskStatus::Running)
                        )
                    })
                    .cloned()
                    .collect();
                if !pending.is_empty() {
                    if let Some(run_id) = action.run_id.as_deref() {
                        if let Some(run) = self.store.get_run(run_id).map_err(|e| e.to_string())? {
                            self.store
                                .update_run_status_where(&run.run_id, TurnStatus::Running, TurnStatus::WaitingTask)
                                .map_err(|e| e.to_string())?;
                            self.store.deliver_wait_registration(&run.run_id, &pending).map_err(|e| e.to_string())?;
                        }
                    }
                    return Ok(Reduction {
                        events: vec![EventDraft::new(
                            EventKind::RunWaiting,
                            json!({"run_id": action.run_id, "agent_id": actor, "waiting_on": pending}),
                        )],
                        receipt: ok(json!({"waiting": true, "task_ids": pending})),
                    });
                }
                // results keyed by task id
                let results: serde_json::Map<String, Json> =
                    task_ids.iter().map(|t| (t.clone(), self.task_result(t))).collect();
                Ok(Reduction {
                    events: vec![],
                    receipt: ok(json!({"waiting": false, "results": results})),
                })
            }

            ActionKind::PublishShared => {
                let entry = SharedEntry {
                    entry_id: new_id("share"),
                    space_id: pstr(p, "space_id"),
                    author: actor.clone(),
                    kind: if pstr(p, "kind").is_empty() { "note".into() } else { pstr(p, "kind") },
                    content: pstr(p, "content"),
                    r#ref: p.get("ref").and_then(|v| v.as_str()).map(str::to_string),
                    supersedes: p.get("supersedes").and_then(|v| v.as_str()).map(str::to_string),
                    sequence: 0,
                    created_at: now(),
                };
                let seq = self.store.add_shared_entry(&entry, &self.session_id).map_err(|e| e.to_string())?;
                let summary: String = entry.content.chars().take(200).collect();
                Ok(Reduction {
                    events: vec![EventDraft::new(
                        EventKind::SharedPublished,
                        json!({"entry_id": entry.entry_id, "space_id": entry.space_id,
                               "author": actor, "kind": entry.kind,
                               "summary": if summary.is_empty() { entry.r#ref.clone().unwrap_or_default() } else { summary },
                               "sequence": seq}),
                    )],
                    receipt: ok(json!({"entry_id": entry.entry_id, "sequence": seq})),
                })
            }

            ActionKind::ReadShared => self.read_shared(action, spec, true),

            ActionKind::ListShared => {
                let spaces: Vec<&SharedSpaceSpec> = spec
                    .shared_spaces
                    .iter()
                    .filter(|s| s.readers.contains(&actor) || s.writers.contains(&actor))
                    .collect();
                let mut infos = vec![];
                for s in spaces {
                    let entries = self.store.shared_entries(&self.session_id, &[s.id.clone()], 0, 1000).map_err(|e| e.to_string())?;
                    infos.push(json!({
                        "space_id": s.id,
                        "entries": entries.len(),
                        "last_sequence": entries.last().map(|e| e.sequence).unwrap_or(0),
                        "readable": true,
                        "writable": s.writers.contains(&actor),
                    }));
                }
                Ok(Reduction { events: vec![], receipt: ok(json!({"spaces": infos})) })
            }

            ActionKind::RequestHelp => {
                let task_id = p.get("task_id").and_then(|v| v.as_str()).map(str::to_string);
                Ok(Reduction {
                    events: vec![EventDraft {
                        kind: EventKind::Message,
                        payload: json!({"text": pstr(p, "message"), "from": actor,
                                        "target": spec.leader_id, "help": true, "task_id": task_id}),
                        targets: Some(vec![spec.leader_id.clone()]),
                        task_id,
                        ..EventDraft::new(EventKind::Message, json!({}))
                    }],
                    receipt: ok(json!({"sent": true, "to": spec.leader_id})),
                })
            }

            ActionKind::ProposeTeamChange => {
                let patch = TopologyPatch {
                    patch_id: new_id("patch"),
                    base_revision: self.store.current_revision(&self.session_id).map_err(|e| e.to_string())?,
                    proposer: actor.clone(),
                    decided_by: None,
                    operations: p.get("operations").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                    affected_agents: vec![],
                    status: PatchStatus::Proposed,
                    created_at: now(),
                    updated_at: now(),
                };
                self.store.insert_patch(&patch, &self.session_id).map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![EventDraft::new(
                        EventKind::TopologyProposed,
                        json!({"patch_id": patch.patch_id, "proposer": actor,
                               "base_revision": patch.base_revision, "operations": patch.operations,
                               "rationale": pstr(p, "rationale")}),
                    )],
                    receipt: ok(json!({"patch_id": patch.patch_id, "status": "PROPOSED"})),
                })
            }

            ActionKind::ApplyTopologyPatch => self.apply_patch_action(action, spec),

            ActionKind::SignalDone => {
                let blockers = self.completion_blockers(spec, action.run_id.as_deref())?;
                if !blockers.is_empty() {
                    return Ok(Reduction {
                        events: vec![],
                        receipt: Receipt {
                            action_id: action.action_id.clone(),
                            ok: false,
                            kind,
                            result: json!({"blockers": blockers}),
                            error: Some("goal not yet complete".into()),
                        },
                    });
                }
                self.store
                    .record_completion_request(action.run_id.as_deref().unwrap_or(""), "", &[], &pstr(p, "summary"))
                    .map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![],
                    receipt: ok(json!({"accepted": true,
                                       "note": "completion commits when the turn ends",
                                       "summary": pstr(p, "summary")})),
                })
            }

            ActionKind::CancelTask => self.cancel_task(action, spec),

            ActionKind::CancelRun => {
                let run_id = pstr(p, "run_id");
                let run = self
                    .store
                    .get_run(&run_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown run {run_id:?}"))?;
                if run.status == TurnStatus::OutcomeUnknown {
                    // the acknowledgement itself is the point: no executor exists to
                    // stop, and leaving it unknown would block signal_done forever
                    self.store.set_run_status(&run.run_id, TurnStatus::Cancelled).map_err(|e| e.to_string())?;
                    return Ok(Reduction {
                        events: vec![EventDraft::new(
                            EventKind::RunCancelled,
                            json!({"run_id": run.run_id, "agent_id": run.agent_id, "status": "CANCELLED",
                                   "acknowledged_outcome_unknown": true}),
                        )],
                        receipt: ok(json!({"run_id": run.run_id, "status": "acknowledged", "was": "OUTCOME_UNKNOWN"})),
                    });
                }
                self.store.set_run_cancel_requested(&run.run_id).map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![EventDraft::new(
                        EventKind::RunCancelled,
                        json!({"run_id": run.run_id, "agent_id": run.agent_id, "status": "CANCEL_REQUESTED"}),
                    )],
                    receipt: ok(json!({"run_id": run.run_id, "status": "cancel_requested"})),
                })
            }

            ActionKind::ApprovalDecision => {
                let aid = pstr(p, "approval_id");
                let req = self
                    .store
                    .get_approval(&aid)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| format!("unknown approval {aid:?}"))?;
                let status = match pstr(p, "decision").as_str() {
                    "once" => ApprovalStatus::ApprovedOnce,
                    "session" => ApprovalStatus::ApprovedSession,
                    _ => ApprovalStatus::Denied,
                };
                self.store.decide_approval(&req.approval_id, status).map_err(|e| e.to_string())?;
                if status == ApprovalStatus::ApprovedSession {
                    self.store.cache_session_approval(&self.session_id, &req.operation_hash, &req.requested_scope).map_err(|e| e.to_string())?;
                }
                self.wake_approval_run(&req.run_id)?;
                Ok(Reduction {
                    events: vec![EventDraft::new(
                        EventKind::ApprovalDecided,
                        json!({"approval_id": req.approval_id, "agent_id": req.agent_id,
                               "status": enum_name(status)}),
                    )],
                    receipt: ok(json!({"approval_id": req.approval_id, "status": enum_name(status)})),
                })
            }

            ActionKind::SetPermissionMode => {
                let mode = pstr(p, "mode");
                let pm: PermissionMode = serde_json::from_value(Json::String(mode.clone())).map_err(|e| e.to_string())?;
                self.store.set_permission_mode(&self.session_id, pm).map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![EventDraft::new(EventKind::SessionStatus, json!({"permission_mode": mode}))],
                    receipt: ok(json!({"mode": mode})),
                })
            }

            ActionKind::PauseSession => {
                self.store.set_session_status(&self.session_id, SessionStatus::Paused).map_err(|e| e.to_string())?;
                Ok(Reduction {
                    events: vec![EventDraft::new(EventKind::SessionStatus, json!({"status": "PAUSED"}))],
                    receipt: ok(json!({"status": "PAUSED"})),
                })
            }

            other => Ok(Reduction {
                events: vec![],
                receipt: Receipt::failure(action, format!("unsupported action kind {}", enum_name(other))),
            }),
        }
    }

    // ------------------------------------------------------------- reductions

    fn read_shared(&mut self, action: &TeamAction, spec: &TeamSpec, advance: bool) -> Result<Reduction, String> {
        let actor = &action.actor_id;
        let p = &action.payload;
        let spaces: Vec<String> = match p.get("space_id").and_then(|v| v.as_str()) {
            None => spec
                .shared_spaces
                .iter()
                .filter(|s| s.readers.contains(actor) || s.writers.contains(actor))
                .map(|s| s.id.clone())
                .collect(),
            Some(sid) => vec![sid.to_string()],
        };
        let after: i64 = match p.get("after_sequence") {
            None | Some(Json::Null) => spaces
                .iter()
                .map(|s| self.store.shared_cursor(&self.session_id, actor, s).unwrap_or(0))
                .min()
                .unwrap_or(0),
            // a malformed cursor becomes a failed receipt — it must never
            // read from 0 silently.
            Some(v) => py_int(v).ok_or_else(|| format!("bad after_sequence {v}"))?,
        };
        let limit: i64 = match p.get("limit") {
            None => 50,
            Some(v) => py_int(v).ok_or_else(|| format!("bad limit {v}"))?,
        };
        let entries = self.store.shared_entries(&self.session_id, &spaces, after, limit).map_err(|e| e.to_string())?;
        if advance && !entries.is_empty() {
            for s in &spaces {
                let seq = entries.iter().filter(|e| &e.space_id == s).map(|e| e.sequence).max().unwrap_or(0);
                if seq > 0 {
                    self.store.advance_shared_cursor(&self.session_id, actor, s, seq).map_err(|e| e.to_string())?;
                }
            }
        }
        let next_seq = entries.last().map(|e| e.sequence).unwrap_or(after);
        Ok(Reduction {
            events: vec![],
            receipt: Receipt::success(
                action,
                json!({"entries": serde_json::to_value(&entries).unwrap_or(json!([])),
                       "next_sequence": next_seq}),
            ),
        })
    }

    fn apply_patch_action(&mut self, action: &TeamAction, spec: &TeamSpec) -> Result<Reduction, String> {
        let p = &action.payload;
        let patch_id = p.get("patch_id").and_then(|v| v.as_str()).map(str::to_string);
        let mut patch = if let Some(pid) = &patch_id {
            let patch = self
                .store
                .get_patch(pid)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("unknown patch {pid:?}"))?;
            if pbool(p, "reject") {
                self.store.set_patch_status(&patch.patch_id, PatchStatus::Rejected).map_err(|e| e.to_string())?;
                // a WAITING_BOUNDARY patch parked its affected members in Draining; release them
                for agent_id in &patch.affected_agents {
                    if matches!(self.store.agent_status(&self.session_id, agent_id), Ok(Some(AgentStatus::Draining))) {
                        self.store.set_agent_status(&self.session_id, agent_id, AgentStatus::Idle).map_err(|e| e.to_string())?;
                    }
                }
                return Ok(Reduction {
                    events: vec![EventDraft::new(
                        EventKind::TopologyRejected,
                        json!({"patch_id": patch.patch_id, "proposer": patch.proposer}),
                    )],
                    receipt: Receipt::success(action, json!({"patch_id": patch.patch_id, "status": "REJECTED"})),
                });
            }
            patch
        } else {
            TopologyPatch {
                patch_id: new_id("patch"),
                base_revision: p.get("base_revision").and_then(|v| v.as_i64()).unwrap_or(0),
                proposer: action.actor_id.clone(),
                decided_by: None,
                operations: p.get("operations").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                affected_agents: vec![],
                status: PatchStatus::Proposed,
                created_at: now(),
                updated_at: now(),
            }
        };
        let operations = if patch_id.is_some() {
            // `p.get("operations") or patch.operations` — an explicit
            // empty list falls back to the stored patch's operations.
            match p.get("operations").and_then(|v| v.as_array()) {
                Some(ops) if !ops.is_empty() => ops.clone(),
                _ => patch.operations.clone(),
            }
        } else {
            patch.operations.clone()
        };
        let (new_spec, error) = self.apply_operations(spec, &operations);
        let Some(new_spec) = new_spec else {
            return Ok(Reduction { events: vec![], receipt: Receipt::failure(action, error.unwrap_or_default()) });
        };
        let affected = self.affected_agents(spec, &new_spec, &operations);
        patch.decided_by = Some(action.actor_id.clone());
        patch.operations = operations.clone();
        patch.affected_agents = affected.clone();
        let waiting: Vec<String> = affected.iter().filter(|a| self.agent_has_live_run(a)).cloned().collect();
        if !waiting.is_empty() {
            patch.status = PatchStatus::WaitingBoundary;
            self.store.insert_patch(&patch, &self.session_id).map_err(|e| e.to_string())?;
            for a in &affected {
                if self.agent_has_live_run(a) {
                    self.store.set_agent_status(&self.session_id, a, AgentStatus::Draining).map_err(|e| e.to_string())?;
                }
            }
            return Ok(Reduction {
                events: vec![EventDraft::new(
                    EventKind::TopologyProposed,
                    json!({"patch_id": patch.patch_id, "status": "WAITING_BOUNDARY",
                           "affected_agents": affected}),
                )],
                receipt: Receipt::success(action, json!({"patch_id": patch.patch_id,
                                                         "status": "WAITING_BOUNDARY",
                                                         "affected_agents": affected})),
            });
        }
        let revision = self.store.save_team_spec(&self.session_id, &new_spec).map_err(|e| e.to_string())?;
        patch.status = PatchStatus::Applied;
        self.store.insert_patch(&patch, &self.session_id).map_err(|e| e.to_string())?;
        self.sync_members(&new_spec, spec)?;
        Ok(Reduction {
            events: vec![EventDraft::new(
                EventKind::TopologyApplied,
                json!({"patch_id": patch.patch_id, "revision": revision,
                       "decided_by": action.actor_id, "operations": operations}),
            )],
            receipt: Receipt::success(action, json!({"patch_id": patch.patch_id, "revision": revision,
                                                     "status": "APPLIED", "affected_agents": affected})),
        })
    }

    /// Returns (None, error) on failure.
    fn apply_operations(&mut self, spec: &TeamSpec, operations: &[Json]) -> (Option<TeamSpec>, Option<String>) {
        let mut data = serde_json::to_value(spec).expect("spec serializes");
        let cfg_tools: HashSet<&String> = self.catalog.tools.keys().collect();
        let leader_id = spec.leader_id.clone();
        // D-33: members coordinate through shared spaces, never by direct member→member
        // channels — the Leader (and the audit log) must be able to see that traffic.
        let member_to_member = |channel: &Json| -> Option<String> {
            let source = channel.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if source == leader_id {
                return None;
            }
            channel
                .get("targets")
                .and_then(|v| v.as_array())
                .and_then(|targets| targets.iter().filter_map(|t| t.as_str()).find(|t| *t != leader_id))
                .map(|target| format!("channel {source:?} -> {target:?} is member-to-member; use a shared space instead"))
        };
        let cfg_models: HashSet<&String> = self.catalog.models.keys().collect();
        let err = |msg: String| (None, Some(msg));

        for op in operations {
            let Some(op) = op.as_object() else {
                return err(format!("operation must be a mapping, got {}", op_type(op)));
            };
            let what = op.get("op").and_then(|v| v.as_str()).unwrap_or("");
            match what {
                "add_agent" => {
                    let agent = op.get("agent").cloned().unwrap_or(json!({}));
                    let aid = agent.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    if data["agents"].as_array().map(|a| a.iter().any(|x| x["id"] == aid)).unwrap_or(false) {
                        return err(format!("member {aid:?} already exists"));
                    }
                    if spec.agents.iter().any(|a| a.id == aid)
                        || matches!(self.store.agent_status(&self.session_id, aid), Ok(Some(AgentStatus::Removed)))
                    {
                        return err("removed member id cannot be reused; choose a new id".into());
                    }
                    let mp = agent.get("model_profile").and_then(|v| v.as_str()).unwrap_or("");
                    if !cfg_models.contains(&mp.to_string()) {
                        return err(format!("unknown model profile {mp:?}; configure it in user config first"));
                    }
                    for tb in agent.get("tool_bindings").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        let tb = tb.as_str().unwrap_or("");
                        if !cfg_tools.contains(&tb.to_string()) && !BUILTIN_TOOL_BINDINGS.contains(&tb) {
                            return err(format!("unknown tool binding {tb:?}"));
                        }
                    }
                    data["agents"].as_array_mut().unwrap().push(agent);
                    for ch in op.get("channels").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        if let Some(e) = member_to_member(&ch) {
                            return err(e);
                        }
                        data["channels"].as_array_mut().unwrap().push(ch);
                    }
                    for sp in op.get("shared_spaces").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        let sid = sp.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let existing = data["shared_spaces"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|s| s["id"] == sid);
                        if let Some(existing) = existing {
                            for key in ["readers", "writers"] {
                                for who in sp.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                                    if !existing[key].as_array().map(|a| a.contains(&who)).unwrap_or(false) {
                                        existing[key].as_array_mut().unwrap().push(who);
                                    }
                                }
                            }
                        } else {
                            data["shared_spaces"].as_array_mut().unwrap().push(json!({
                                "id": sid,
                                "readers": sp.get("readers").cloned().unwrap_or(json!([])),
                                "writers": sp.get("writers").cloned().unwrap_or(json!([])),
                            }));
                        }
                    }
                }
                "remove_agent" => {
                    let aid = op.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                    if aid == spec.leader_id {
                        return err("the Leader cannot be removed".into());
                    }
                    if !data["agents"].as_array().map(|a| a.iter().any(|x| x["id"] == aid)).unwrap_or(false) {
                        return err(format!("unknown member {aid:?}"));
                    }
                    let agents = data["agents"].as_array().cloned().unwrap_or_default();
                    data["agents"] = json!(agents.into_iter().filter(|a| a["id"] != aid).collect::<Vec<_>>());
                    for ch in data["channels"].as_array_mut().unwrap().iter_mut() {
                        let ts = ch["targets"].as_array().cloned().unwrap_or_default();
                        ch["targets"] = json!(ts.into_iter().filter(|t| t != aid).collect::<Vec<_>>());
                    }
                    let channels = data["channels"].as_array().cloned().unwrap_or_default();
                    data["channels"] = json!(channels
                        .into_iter()
                        .filter(|c| c["source"] != aid && !c["targets"].as_array().map(|t| t.is_empty()).unwrap_or(true))
                        .collect::<Vec<_>>());
                    let observers = data["observers"].as_array().cloned().unwrap_or_default();
                    data["observers"] = json!(observers.into_iter().filter(|o| o["agent_id"] != aid).collect::<Vec<_>>());
                    for ob in data["observers"].as_array_mut().unwrap().iter_mut() {
                        let subs = ob["subjects"].as_array().cloned().unwrap_or_default();
                        ob["subjects"] = json!(subs.into_iter().filter(|s| s != aid).collect::<Vec<_>>());
                    }
                    for sp in data["shared_spaces"].as_array_mut().unwrap().iter_mut() {
                        for key in ["readers", "writers"] {
                            let xs = sp[key].as_array().cloned().unwrap_or_default();
                            sp[key] = json!(xs.into_iter().filter(|x| x != aid).collect::<Vec<_>>());
                        }
                    }
                }
                "update_agent" => {
                    let aid = op.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
                    let agents = data["agents"].as_array_mut().unwrap();
                    let Some(target) = agents.iter_mut().find(|a| a["id"] == aid) else {
                        return err(format!("unknown member {aid:?}"));
                    };
                    let changes = op.get("changes").cloned().unwrap_or(json!({}));
                    if let Some(new_id) = changes.get("id").and_then(|v| v.as_str()) {
                        if new_id != aid {
                            return err("member id is immutable; add a new member instead".into());
                        }
                    }
                    if let Some(mp) = changes.get("model_profile").and_then(|v| v.as_str()) {
                        if !cfg_models.contains(&mp.to_string()) {
                            return err(format!("unknown model profile {mp:?}"));
                        }
                    }
                    for tb in changes.get("tool_bindings").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        let tb = tb.as_str().unwrap_or("");
                        if !cfg_tools.contains(&tb.to_string()) && !BUILTIN_TOOL_BINDINGS.contains(&tb) {
                            return err(format!("unknown tool binding {tb:?}"));
                        }
                    }
                    if let Some(obj) = changes.as_object() {
                        for (k, v) in obj {
                            target[k] = v.clone();
                        }
                    }
                }
                "add_channel" => {
                    let channel = op.get("channel").cloned().unwrap_or(json!({}));
                    if let Some(e) = member_to_member(&channel) {
                        return err(e);
                    }
                    data["channels"].as_array_mut().unwrap().push(channel);
                }
                "remove_channel" => {
                    let src = op.get("source").and_then(|v| v.as_str()).unwrap_or("");
                    let tgts = op.get("targets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                    for t in &tgts {
                        for c in data["channels"].as_array_mut().unwrap().iter_mut() {
                            if c["source"] == src && c["targets"].as_array().map(|a| a.contains(t)).unwrap_or(false) {
                                let ts = c["targets"].as_array().cloned().unwrap_or_default();
                                c["targets"] = json!(ts.into_iter().filter(|x| x != t).collect::<Vec<_>>());
                            }
                        }
                        let channels = data["channels"].as_array().cloned().unwrap_or_default();
                        data["channels"] = json!(channels
                            .into_iter()
                            .filter(|c| !(c["source"] == src && c["targets"].as_array().map(|t| t.is_empty()).unwrap_or(true)))
                            .collect::<Vec<_>>());
                    }
                }
                "set_observer" => {
                    let ob = op.get("observer").cloned().unwrap_or(json!({}));
                    let oid = ob.get("agent_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let observers = data["observers"].as_array().cloned().unwrap_or_default();
                    data["observers"] = json!(observers.into_iter().filter(|o| o["agent_id"] != oid).collect::<Vec<_>>());
                    if !pbool(&Json::Object(op.clone()), "remove") {
                        data["observers"].as_array_mut().unwrap().push(ob);
                    }
                }
                "set_space_acl" => {
                    let sid = op.get("space_id").and_then(|v| v.as_str()).unwrap_or("");
                    let spaces = data["shared_spaces"].as_array_mut().unwrap();
                    let Some(target) = spaces.iter_mut().find(|s| s["id"] == sid) else {
                        return err(format!("unknown shared space {sid:?}"));
                    };
                    for key in ["readers", "writers"] {
                        if let Some(v) = op.get(key) {
                            target[key] = v.clone();
                        }
                    }
                }
                other => return err(format!("unsupported operation {other:?}")),
            }
        }
        match serde_json::from_value::<TeamSpec>(data) {
            Ok(new_spec) => match new_spec.validate() {
                Ok(()) => (Some(new_spec), None),
                Err(e) => err(format!("patch produced an invalid team spec: {e}")),
            },
            Err(e) => err(format!("patch produced an invalid team spec: {e}")),
        }
    }

    fn affected_agents(&self, old: &TeamSpec, new: &TeamSpec, operations: &[Json]) -> Vec<String> {
        let mut affected: HashSet<String> = HashSet::new();
        macro_rules! add {
            ($v:expr) => {
                if let Some(Json::String(s)) = $v {
                    affected.insert(s.clone());
                }
            };
        }
        for op in operations {
            match op.get("op").and_then(|v| v.as_str()).unwrap_or("") {
                "remove_agent" | "update_agent" => add!(op.get("agent_id")),
                "add_channel" => {
                    let ch = op.get("channel").cloned().unwrap_or(json!({}));
                    add!(ch.get("source"));
                    for t in ch.get("targets").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        add!(Some(&t));
                    }
                }
                "remove_channel" => {
                    add!(op.get("source"));
                    for t in op.get("targets").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        add!(Some(&t));
                    }
                }
                "set_space_acl" => {
                    let sid = op.get("space_id").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(old_space) = old.shared_spaces.iter().find(|s| s.id == sid) {
                        affected.extend(old_space.readers.iter().cloned());
                        affected.extend(old_space.writers.iter().cloned());
                    }
                    for key in ["readers", "writers"] {
                        for who in op.get(key).and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                            add!(Some(&who));
                        }
                    }
                }
                "set_observer" => {
                    let ob = op.get("observer").cloned().unwrap_or(json!({}));
                    add!(ob.get("agent_id"));
                    for s in ob.get("subjects").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                        add!(Some(&s));
                    }
                }
                _ => {}
            }
        }
        let mut result = vec![];
        for aid in {
            let mut v: Vec<String> = affected.into_iter().filter(|a| !a.is_empty()).collect();
            v.sort();
            v
        } {
            let old_a = old.agent(&aid);
            let new_a = new.agent(&aid);
            if old_a.is_none() || new_a.is_none() || old_a != new_a {
                result.push(aid);
            } else if self.permissions_changed(old, new, &aid)
                && operations.iter().any(|op| !matches!(op.get("op").and_then(|v| v.as_str()), Some("add_agent") | Some("add_channel")))
            {
                result.push(aid);
            }
        }
        result
    }

    fn permissions_changed(&self, old: &TeamSpec, new: &TeamSpec, agent_id: &str) -> bool {
        let perms = |spec: &TeamSpec| {
            let sends: Vec<String> = {
                let mut v: Vec<String> = spec.agents.iter().filter(|a| spec.can_send(agent_id, &a.id)).map(|a| a.id.clone()).collect();
                v.sort();
                v
            };
            let delegates: Vec<String> = {
                let mut v: Vec<String> = spec.agents.iter().filter(|a| spec.can_delegate(agent_id, &a.id)).map(|a| a.id.clone()).collect();
                v.sort();
                v
            };
            let spaces: Vec<(String, bool, bool)> = {
                let mut v: Vec<(String, bool, bool)> = spec
                    .shared_spaces
                    .iter()
                    .map(|s| (s.id.clone(), s.readers.iter().any(|r| r == agent_id), s.writers.iter().any(|w| w == agent_id)))
                    .collect();
                v.sort();
                v
            };
            let observers: Vec<(String, Vec<String>, Vec<String>)> = {
                let mut v: Vec<(String, Vec<String>, Vec<String>)> = spec
                    .observers
                    .iter()
                    .map(|o| {
                        let mut et = o.event_types.clone();
                        et.sort();
                        (o.agent_id.clone(), o.subjects.clone(), et)
                    })
                    .collect();
                v.sort();
                v
            };
            (sends, delegates, spaces, observers)
        };
        perms(old) != perms(new)
    }

    fn sync_members(&mut self, new: &TeamSpec, old: &TeamSpec) -> Result<(), String> {
        for a in &new.agents {
            self.store.ensure_agent(&self.session_id, &a.id).map_err(|e| e.to_string())?;
        }
        for a in &new.agents {
            let old_a = old.agent(&a.id);
            if old_a != Some(a) {
                self.store.bump_config_revision(&self.session_id, &a.id).map_err(|e| e.to_string())?;
            }
            // Message/shared ACLs are read from the current topology. A grant
            // must not rebuild a parked member's graph and lose its resume state.
            if matches!(self.store.agent_status(&self.session_id, &a.id), Ok(Some(AgentStatus::Draining))) {
                self.store.set_agent_status(&self.session_id, &a.id, AgentStatus::Idle).map_err(|e| e.to_string())?;
            }
        }
        for a in &old.agents {
            if new.agent(&a.id).is_none() {
                self.store.set_agent_status(&self.session_id, &a.id, AgentStatus::Removed).map_err(|e| e.to_string())?;
                self.hand_over_removed_member(&a.id, new)?;
            }
        }
        Ok(())
    }

    /// A removed member's unfinished work goes back to the Leader (plan §8).
    fn hand_over_removed_member(&mut self, agent_id: &str, spec: &TeamSpec) -> Result<(), String> {
        let handed = self
            .store
            .reassign_tasks(agent_id, &spec.leader_id, &[TaskStatus::Pending, TaskStatus::Blocked])
            .map_err(|e| e.to_string())?;
        for task_id in &handed {
            // a handed-over task must be announced to its new assignee
            self.store.del_meta(&format!("task_ready_announced:{task_id}")).map_err(|e| e.to_string())?;
        }
        let dropped = self
            .store
            .drop_pending_deliveries(&self.session_id, agent_id, "member removed before delivery")
            .map_err(|e| e.to_string())?;
        let action = TeamAction {
            action_id: format!("member-removed:{agent_id}"),
            session_id: self.session_id.clone(),
            actor_id: "system".into(),
            run_id: None,
            kind: ActionKind::ApplyTopologyPatch,
            payload: json!({}),
        };
        self.persist_events(
            &action,
            spec,
            &[EventDraft::new(
                EventKind::MemberRemoved,
                json!({"agent_id": agent_id, "hands_over_to": spec.leader_id,
                       "handed_over_tasks": handed, "dropped_deliveries": dropped,
                       "note": "results and work directories are kept for review"}),
            )],
        )
    }

    fn cancel_task(&mut self, action: &TeamAction, _spec: &TeamSpec) -> Result<Reduction, String> {
        let task = self
            .store
            .get_task(&pstr(&action.payload, "task_id"))
            .map_err(|e| e.to_string())?
            .ok_or("unknown task")?;
        if matches!(task.status, TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled) {
            return Ok(Reduction {
                events: vec![],
                receipt: Receipt::success(action, json!({"task_id": task.task_id, "status": enum_name(task.status)})),
            });
        }
        // QUEUED runs have no executor to interrupt: drop them to CANCELLED
        // outright so the engine never begins a run for an already-cancelled task.
        for run in self.store.runs_for_session(&self.session_id, &[TurnStatus::Queued]).map_err(|e| e.to_string())? {
            if run.task_id.as_deref() == Some(task.task_id.as_str()) {
                self.store
                    .update_run_status_where(&run.run_id, TurnStatus::Queued, TurnStatus::Cancelled)
                    .map_err(|e| e.to_string())?;
            }
        }
        let mut active = self.store.active_run_for_agent(&self.session_id, &task.assignee).map_err(|e| e.to_string())?;
        if active.is_none() {
            // a parked turn (approval or task wait) has no executor to interrupt:
            // requesting its cancel lets schedule converge it (run -> CANCELLED)
            if let Some(parked) = self.waiting_run(&task.assignee)? {
                // task_id.is_none(): taskless parked runs (e.g. a chat turn parked by
                // wait_for_tasks) are cancelled alongside, like the WAITING_APPROVAL
                // case; the member starts a fresh run when its awaited
                // tasks finish.
                if matches!(parked.status, TurnStatus::WaitingApproval | TurnStatus::WaitingTask)
                    && (parked.task_id.as_deref() == Some(task.task_id.as_str()) || parked.task_id.is_none())
                {
                    active = Some(parked);
                }
            }
        }
        let events = vec![EventDraft {
            kind: EventKind::TaskCancelled,
            payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                            "requester": task.requester, "status": "CANCEL_REQUESTED"}),
            task_id: Some(task.task_id.clone()),
            ..EventDraft::new(EventKind::TaskCancelled, json!({}))
        }];
        let status;
        if let Some(active) = &active {
            if active.task_id.as_deref() == Some(task.task_id.as_str()) || active.task_id.is_none() {
                self.store.set_run_cancel_requested(&active.run_id).map_err(|e| e.to_string())?;
                self.store.set_task_cancel_requested(&task.task_id).map_err(|e| e.to_string())?;
                status = "CANCEL_REQUESTED";
            } else {
                self.store.compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Cancelled, None).map_err(|e| e.to_string())?;
                status = "CANCELLED";
            }
        } else {
            self.store.compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Cancelled, None).map_err(|e| e.to_string())?;
            status = "CANCELLED";
        }
        Ok(Reduction {
            events,
            receipt: Receipt::success(action, json!({"task_id": task.task_id, "status": status})),
        })
    }

    fn completion_blockers(&mut self, spec: &TeamSpec, current_run: Option<&str>) -> Result<Vec<String>, String> {
        let mut blockers = vec![];
        let live = self
            .store
            .runs_for_session(
                &self.session_id,
                &[TurnStatus::Queued, TurnStatus::Running, TurnStatus::WaitingTask, TurnStatus::WaitingApproval],
            )
            .map_err(|e| format!("read active runs: {e}"))?;
        let live: Vec<_> = live.into_iter().filter(|r| Some(r.run_id.as_str()) != current_run).collect();
        if !live.is_empty() {
            blockers.push(format!(
                "active turns: {}",
                live.iter().map(|r| format!("{}:{}", r.agent_id, enum_name(r.status))).collect::<Vec<_>>().join(", ")
            ));
        }
        let unknown = self.store.runs_for_session(&self.session_id, &[TurnStatus::OutcomeUnknown]).map_err(|e| format!("read unknown runs: {e}"))?;
        if !unknown.is_empty() {
            // the run id must stand alone: gluing `agent:run` together made models
            // copy the whole token into cancel_run (observed in a real run)
            blockers.push(format!(
                "outcome-unknown operations: {} (acknowledge each with cancel_run <run_id> once you accept its side effects)",
                unknown.iter().map(|r| format!("{} of {}", r.run_id, r.agent_id)).collect::<Vec<_>>().join(", ")
            ));
        }
        let un = self
            .store
            .tasks_for_session(&self.session_id, &["PENDING", "RUNNING", "BLOCKED"])
            .map_err(|e| format!("read unfinished tasks: {e}"))?;
        if !un.is_empty() {
            blockers.push(format!(
                "unfinished tasks: {}",
                un.iter().map(|t| format!("{}:{}", t.task_id, enum_name(t.status))).collect::<Vec<_>>().join(", ")
            ));
        }
        let pend = self.store.pending_approvals(&self.session_id).map_err(|e| format!("read pending approvals: {e}"))?;
        if !pend.is_empty() {
            blockers.push(format!("pending approvals: {}", pend.iter().map(|a| a.approval_id.clone()).collect::<Vec<_>>().join(", ")));
        }
        let _ = spec;
        Ok(blockers)
    }

    /// Void a run's PENDING approvals and return the audit events (RT-06).
    pub fn expire_run_approvals(&mut self, run_id: &str) -> Vec<EventDraft> {
        self.store
            .expire_run_approvals(run_id)
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                EventDraft::new(
                    EventKind::ApprovalDecided,
                    json!({"approval_id": a.approval_id, "agent_id": a.agent_id,
                           "run_id": a.run_id, "status": "EXPIRED",
                           "note": "turn ended before a decision"}),
                )
            })
            .collect()
    }

    // ----------------------------------------------------------------- persist

    fn persist_events(&mut self, action: &TeamAction, spec: &TeamSpec, drafts: &[EventDraft]) -> Result<(), String> {
        let mut batch_by_agent: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        for draft in drafts {
            let actor = draft.actor_id.clone().unwrap_or_else(|| action.actor_id.clone());
            let audience = draft.audience.clone().unwrap_or_else(|| {
                views::event_audience(spec, draft.kind, &actor, &draft.payload, draft.targets.as_deref())
            });
            let mut push = views::event_push(spec, draft.kind, &actor, &draft.payload, draft.targets.as_deref());
            if let Some(explicit) = &draft.push {
                let mut set: HashSet<String> = push.into_iter().collect();
                set.extend(explicit.iter().cloned());
                push = {
                    let mut v: Vec<String> = set.into_iter().collect();
                    v.sort();
                    v
                };
            }
            let revision = self.store.current_revision(&self.session_id).map_err(|e| e.to_string())?;
            let event = TeamEvent {
                event_id: new_id("evt"),
                session_id: self.session_id.clone(),
                sequence: 0,
                actor_id: actor.clone(),
                task_id: draft.task_id.clone(),
                kind: draft.kind,
                payload: draft.payload.clone(),
                audience,
                topology_revision: revision,
                causation_id: Some(action.action_id.clone()),
                created_at: now(),
            };
            self.store.append_event(&event).map_err(|e| e.to_string())?;
            for recipient in push {
                let batch = match batch_by_agent.get(&recipient) {
                    Some(b) => *b,
                    None => {
                        let b = self.store.next_batch_no(&self.session_id, &recipient).map_err(|e| e.to_string())?;
                        batch_by_agent.insert(recipient.clone(), b);
                        b
                    }
                };
                let scope = views::observer_scope_for(spec, &recipient, draft.kind, &actor, &draft.payload);
                let override_json = scope.map(|s| serde_json::to_string(&views::scope_payload(&s, draft.kind, &draft.payload)).unwrap());
                self.store
                    .create_delivery(&self.session_id, &recipient, &event.event_id, batch, override_json.as_deref())
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------- schedule

    /// Create/wake runs for pending deliveries
    /// and ready tasks.
    fn schedule_inner(&mut self, spec: &TeamSpec) -> Result<(), String> {
        let session = self.session_id.clone();
        self.apply_boundary_patches(spec)?;
        let spec = self.store.load_team_spec(&session, None).map_err(|e| e.to_string())?;
        self.block_unrunnable_tasks(&spec)?;
        self.announce_ready_tasks(&spec)?;
        let draining: HashSet<String> = self
            .store
            .patches_in_status(&session, PatchStatus::WaitingBoundary)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|p| p.affected_agents)
            .collect();
        for agent in &spec.agents {
            if draining.contains(&agent.id) {
                self.store.set_agent_status(&session, &agent.id, AgentStatus::Draining).map_err(|e| e.to_string())?;
            }
            let parked = self.waiting_run(&agent.id)?;
            if let Some(parked) = &parked {
                if matches!(parked.status, TurnStatus::WaitingApproval | TurnStatus::WaitingTask)
                    && parked.cancel_requested
                    && parked.external_turn_id.is_none()
                {
                    // a parked in-process turn has no executor to finalize it
                    self.converge_cancelled_waiting_run(parked, &spec)?;
                }
            }
            let status = self.store.agent_status(&session, &agent.id).map_err(|e| e.to_string())?;
            if matches!(status, Some(AgentStatus::Removed) | Some(AgentStatus::Draining)) {
                continue;
            }
            let pending = self.store.pending_deliveries_joined(&session, &agent.id).map_err(|e| e.to_string())?;
            let waiting = self.waiting_run(&agent.id)?;
            let active = self.active_runs(&agent.id)?;
            if pending.is_empty() && active.is_empty() && waiting.is_none() {
                // Liveness: a ready task whose wake notification was consumed
                // mid-turn by an earlier run would otherwise never be dispatched.
                let Some(task) = self.next_ready_task(&agent.id)? else { continue };
                if !self.turn_budget_ok()? {
                    self.emit_limit_reached()?;
                    continue;
                }
                self.store
                    .insert_run(&TurnRun {
                        run_id: new_id("run"),
                        session_id: session.clone(),
                        task_id: Some(task.task_id.clone()),
                        goal_id: self.current_goal_id()?,
                        agent_id: agent.id.clone(),
                        config_revision: self.store.agent_config_revision(&session, &agent.id).map_err(|e| e.to_string())?,
                        topology_revision: self.store.current_revision(&session).map_err(|e| e.to_string())?,
                        status: TurnStatus::Queued,
                        input_delivery_ids: vec![],
                        context_ref: Some(self.context_ref(&agent.id)?),
                        external_turn_id: None,
                        cancel_requested: false,
                        waiting_on: vec![],
                        created_at: now(),
                        updated_at: now(),
                    })
                    .map_err(|e| e.to_string())?;
                continue;
            }
            if pending.is_empty() {
                continue;
            }
            if !active.is_empty() {
                let run = &active[0];
                let fresh: Vec<&Json> = pending
                    .iter()
                    .filter(|d| !run.input_delivery_ids.contains(&d["delivery_id"].as_i64().unwrap_or(-1)))
                    .collect();
                let ids: Vec<i64> = fresh.iter().filter_map(|d| d["delivery_id"].as_i64()).collect();
                if ids.is_empty() {
                    continue;
                }
                self.store.append_run_inputs(&run.run_id, &ids).map_err(|e| e.to_string())?;
                if run.status == TurnStatus::Running {
                    self.mid_turn_pushes.push((
                        run.run_id.clone(),
                        fresh
                            .iter()
                            .map(|d| {
                                // same trim as views::build_agent_view: the delivery's scoped
                                // override wins, raw event payload is only the fallback
                                let payload = d["payload_override"]
                                    .as_str()
                                    .and_then(|o| serde_json::from_str(o).ok())
                                    .or_else(|| serde_json::from_str(d["payload_json"].as_str().unwrap_or("null")).ok())
                                    .unwrap_or(Json::Null);
                                json!({"kind": d["event_kind"], "from": d["event_actor"],
                                       "event_id": d["event_id"], "delivery_id": d["delivery_id"],
                                       "task_id": d["event_task_id"],
                                       "payload": payload})
                            })
                            .collect(),
                    ));
                }
                continue;
            }
            if let Some(waiting) = waiting {
                if waiting.status == TurnStatus::WaitingApproval {
                    continue;
                }
                // ponytail: a waiting turn resumes when ALL waited tasks are
                // terminal, or on user input / cancel.
                let pending_wait: Vec<String> = waiting
                    .waiting_on
                    .iter()
                    .filter(|t| {
                        matches!(
                            self.store.get_task(t).ok().flatten().map(|t| t.status),
                            Some(TaskStatus::Pending | TaskStatus::Running)
                        )
                    })
                    .cloned()
                    .collect();
                let seen: HashSet<i64> = waiting.input_delivery_ids.iter().copied().collect();
                let user_input = pending.iter().any(|d| {
                    d["event_kind"].as_str() == Some("user_message")
                        && !seen.contains(&d["delivery_id"].as_i64().unwrap_or(-1))
                });
                if !pending_wait.is_empty() && !user_input && !waiting.cancel_requested {
                    continue;
                }
                if self
                    .store
                    .update_run_status_where(&waiting.run_id, TurnStatus::WaitingTask, TurnStatus::Running)
                    .map_err(|e| e.to_string())?
                {
                    let ids: Vec<i64> = pending.iter().filter_map(|d| d["delivery_id"].as_i64()).collect();
                    self.store.append_run_inputs(&waiting.run_id, &ids).map_err(|e| e.to_string())?;
                }
                continue;
            }
            if !self.turn_budget_ok()? {
                self.emit_limit_reached()?;
                continue;
            }
            let task = self.next_ready_task(&agent.id)?;
            let run = TurnRun {
                run_id: new_id("run"),
                session_id: session.clone(),
                task_id: task.map(|t| t.task_id),
                goal_id: self.current_goal_id()?,
                agent_id: agent.id.clone(),
                config_revision: self.store.agent_config_revision(&session, &agent.id).map_err(|e| e.to_string())?,
                topology_revision: self.store.current_revision(&session).map_err(|e| e.to_string())?,
                status: TurnStatus::Queued,
                input_delivery_ids: pending.iter().filter_map(|d| d["delivery_id"].as_i64()).collect(),
                context_ref: Some(self.context_ref(&agent.id)?),
                external_turn_id: None,
                cancel_requested: false,
                waiting_on: vec![],
                created_at: now(),
                updated_at: now(),
            };
            self.store.insert_run(&run).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Finalize a run cancelled while parked on an approval (RT-06) or a task wait.
    fn converge_cancelled_waiting_run(&mut self, run: &TurnRun, spec: &TeamSpec) -> Result<(), String> {
        if !self
            .store
            .update_run_status_where(&run.run_id, run.status, TurnStatus::Cancelled)
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        let mut events = self.expire_run_approvals(&run.run_id);
        if let Some(task_id) = &run.task_id {
            if let Ok(Some(task)) = self.store.get_task(task_id) {
                if matches!(task.status, TaskStatus::Pending | TaskStatus::Running)
                    && self
                        .store
                        .compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Cancelled, None)
                        .map_err(|e| e.to_string())?
                {
                    events.push(EventDraft {
                        kind: EventKind::TaskCancelled,
                        payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                        "requester": task.requester, "status": "CANCELLED"}),
                        task_id: Some(task.task_id.clone()),
                        ..EventDraft::new(EventKind::TaskCancelled, json!({}))
                    });
                }
            }
        }
        self.store.set_agent_status(&self.session_id, &run.agent_id, AgentStatus::Idle).map_err(|e| e.to_string())?;
        self.store.ack_run_deliveries(run).map_err(|e| e.to_string())?;
        events.push(EventDraft::new(
            EventKind::RunCancelled,
            json!({"run_id": run.run_id, "agent_id": run.agent_id, "status": "CANCELLED",
                   "error": "cancelled while waiting"}),
        ));
        let action = TeamAction {
            action_id: format!("cancel-parked:{}", run.run_id),
            session_id: self.session_id.clone(),
            actor_id: "system".into(),
            run_id: None,
            kind: ActionKind::CancelRun,
            payload: json!({"run_id": run.run_id}),
        };
        self.persist_events(&action, spec, &events)
    }

    /// Dependencies that failed/cancelled make dependents BLOCKED.
    fn block_unrunnable_tasks(&mut self, spec: &TeamSpec) -> Result<(), String> {
        for task in self.store.tasks_for_session(&self.session_id, &["PENDING"]).map_err(|e| e.to_string())? {
            let broken: Vec<Task> = task
                .dependencies
                .iter()
                .filter_map(|d| self.store.get_task(d).ok().flatten())
                .filter(|d| matches!(d.status, TaskStatus::Failed | TaskStatus::Cancelled))
                .collect();
            if broken.is_empty() {
                continue;
            }
            if self
                .store
                .compare_and_set_task(&task.task_id, "PENDING", TaskStatus::Blocked, None)
                .map_err(|e| e.to_string())?
            {
                let draft = EventDraft {
                    kind: EventKind::TaskBlocked,
                    payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                    "requester": task.requester,
                                    "reason": format!("dependency {}", broken.iter().map(|d| format!("{}:{}", d.task_id, enum_name(d.status))).collect::<Vec<_>>().join(", "))}),
                    task_id: Some(task.task_id.clone()),
                    ..EventDraft::new(EventKind::TaskBlocked, json!({}))
                };
                let action = TeamAction {
                    action_id: format!("blocked:{}", task.task_id),
                    session_id: self.session_id.clone(),
                    actor_id: "system".into(),
                    run_id: None,
                    kind: ActionKind::CancelTask,
                    payload: json!({"task_id": task.task_id}),
                };
                self.persist_events(&action, spec, &[draft])?;
            }
        }
        Ok(())
    }

    /// One TASK_READY ping per task, once its dependencies are satisfied.
    fn announce_ready_tasks(&mut self, spec: &TeamSpec) -> Result<(), String> {
        let mut pending = self.store.tasks_for_session(&self.session_id, &["PENDING"]).map_err(|e| e.to_string())?;
        pending.sort_by(|a, b| a.created_at.partial_cmp(&b.created_at).unwrap_or(std::cmp::Ordering::Equal));
        for task in pending {
            let ready = task
                .dependencies
                .iter()
                .all(|d| matches!(self.store.get_task(d).ok().flatten().map(|t| t.status), Some(TaskStatus::Succeeded)));
            if !ready {
                continue;
            }
            let key = format!("task_ready_announced:{}", task.task_id);
            if self.store.get_meta(&key).map_err(|e| e.to_string())?.is_some() {
                continue;
            }
            let draft = EventDraft {
                kind: EventKind::TaskReady,
                payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                "requester": task.requester, "description": task.description,
                                "acceptance": task.acceptance}),
                task_id: Some(task.task_id.clone()),
                ..EventDraft::new(EventKind::TaskReady, json!({}))
            };
            let action = TeamAction {
                action_id: format!("ready:{}", task.task_id),
                session_id: self.session_id.clone(),
                actor_id: "system".into(),
                run_id: None,
                kind: ActionKind::CancelTask,
                payload: json!({}),
            };
            self.persist_events(&action, spec, &[draft])?;
            self.store.set_meta(&key, "1").map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn apply_boundary_patches(&mut self, _spec: &TeamSpec) -> Result<(), String> {
        for patch in self.store.patches_in_status(&self.session_id, PatchStatus::WaitingBoundary).map_err(|e| e.to_string())? {
            if patch.affected_agents.iter().any(|a| self.agent_has_live_run(a)) {
                continue;
            }
            let current = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
            let (new_spec, error) = if patch.base_revision != self.store.current_revision(&self.session_id).map_err(|e| e.to_string())? {
                (None, Some("patch base_revision is stale; Leader must decide again".to_string()))
            } else {
                self.apply_operations(&current, &patch.operations)
            };
            let Some(new_spec) = new_spec else {
                self.store.set_patch_status(&patch.patch_id, PatchStatus::Failed).map_err(|e| e.to_string())?;
                for agent_id in &patch.affected_agents {
                    if matches!(self.store.agent_status(&self.session_id, agent_id), Ok(Some(AgentStatus::Draining))) {
                        self.store.set_agent_status(&self.session_id, agent_id, AgentStatus::Idle).map_err(|e| e.to_string())?;
                    }
                }
                let action = TeamAction {
                    action_id: format!("patch-failed:{}", patch.patch_id),
                    session_id: self.session_id.clone(),
                    actor_id: "system".into(),
                    run_id: None,
                    kind: ActionKind::ApplyTopologyPatch,
                    payload: json!({}),
                };
                self.persist_events(
                    &action,
                    &current,
                    &[EventDraft::new(
                        EventKind::TopologyRejected,
                        json!({"patch_id": patch.patch_id, "proposer": patch.proposer, "error": error}),
                    )],
                )?;
                continue;
            };
            let revision = self.store.save_team_spec(&self.session_id, &new_spec).map_err(|e| e.to_string())?;
            self.store.set_patch_status(&patch.patch_id, PatchStatus::Applied).map_err(|e| e.to_string())?;
            self.sync_members(&new_spec, &current)?;
            self.persist_patch_event(&patch, revision)?;
        }
        Ok(())
    }

    fn persist_patch_event(&mut self, patch: &TopologyPatch, revision: i64) -> Result<(), String> {
        let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
        let action = TeamAction {
            action_id: format!("patch-apply:{}", patch.patch_id),
            session_id: self.session_id.clone(),
            actor_id: patch.decided_by.clone().unwrap_or_else(|| patch.proposer.clone()),
            run_id: None,
            kind: ActionKind::ApplyTopologyPatch,
            payload: json!({}),
        };
        self.persist_events(
            &action,
            &spec,
            &[EventDraft::new(
                EventKind::TopologyApplied,
                json!({"patch_id": patch.patch_id, "revision": revision,
                       "decided_by": patch.decided_by, "operations": patch.operations}),
            )],
        )
    }

    fn emit_limit_reached(&mut self) -> Result<(), String> {
        let key = format!("limit_notified:{}", self.current_goal_id()?.unwrap_or_default());
        if self.store.get_meta(&key).map_err(|e| e.to_string())?.is_some() {
            return Ok(());
        }
        self.store.set_meta(&key, "1").map_err(|e| e.to_string())?;
        let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
        let action = TeamAction {
            action_id: new_id("sys"),
            session_id: self.session_id.clone(),
            actor_id: "system".into(),
            run_id: None,
            kind: ActionKind::PauseSession,
            payload: json!({}),
        };
        self.persist_events(
            &action,
            &spec,
            &[EventDraft::new(
                EventKind::LimitReached,
                json!({"kind": "max_turns_per_goal", "limit": spec.limits.max_turns_per_goal,
                       "goal_id": self.current_goal_id()?}),
            )],
        )
    }

    // ------------------------------------------------------------- small helpers

    fn agent_has_live_run(&self, agent_id: &str) -> bool {
        self.store
            .runs_for_session(
                &self.session_id,
                &[TurnStatus::Queued, TurnStatus::Running, TurnStatus::WaitingTask, TurnStatus::WaitingApproval],
            )
            .unwrap_or_default()
            .iter()
            // ponytail: in-process runs parked on an approval or task wait have no
            // live thread (the executor exited at TurnPaused); counting them
            // deadlocks boundary patches behind an undecided approval or an
            // unfinished awaited task. External (codex) runs still have a live
            // waiter while external_turn_id is set, so they stay "live".
            .any(|r| {
                r.agent_id == agent_id
                    && !(matches!(r.status, TurnStatus::WaitingApproval | TurnStatus::WaitingTask)
                        && r.external_turn_id.is_none())
            })
    }

    fn active_runs(&self, agent_id: &str) -> Result<Vec<TurnRun>, String> {
        Ok(self
            .store
            .runs_for_session(&self.session_id, &[TurnStatus::Queued, TurnStatus::Running])
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|r| r.agent_id == agent_id)
            .collect())
    }

    fn waiting_run(&self, agent_id: &str) -> Result<Option<TurnRun>, String> {
        Ok(self
            .store
            .runs_for_session(&self.session_id, &[TurnStatus::WaitingTask, TurnStatus::WaitingApproval])
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|r| r.agent_id == agent_id))
    }

    fn next_ready_task(&self, agent_id: &str) -> Result<Option<Task>, String> {
        let mut pending = self.store.tasks_for_session(&self.session_id, &["PENDING"]).map_err(|e| e.to_string())?;
        pending.sort_by(|a, b| a.created_at.partial_cmp(&b.created_at).unwrap_or(std::cmp::Ordering::Equal));
        for task in pending {
            if task.assignee != agent_id {
                continue;
            }
            let ready = task
                .dependencies
                .iter()
                .all(|d| matches!(self.store.get_task(d).ok().flatten().map(|t| t.status), Some(TaskStatus::Succeeded)));
            if ready {
                return Ok(Some(task));
            }
        }
        Ok(None)
    }

    fn turn_budget_ok(&self) -> Result<bool, String> {
        let Some(goal_id) = self.current_goal_id()? else { return Ok(true) };
        let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
        let used = self.store.count_goal_runs(&self.session_id, &goal_id).map_err(|e| e.to_string())?;
        Ok(used < spec.limits.max_turns_per_goal)
    }

    fn task_result(&self, task_id: &str) -> Json {
        let task = self.store.get_task(task_id).ok().flatten();
        json!({"task_id": task_id,
               "status": task.as_ref().map(|t| enum_name(t.status)).unwrap_or_else(|| "UNKNOWN".into()),
               "result_refs": task.map(|t| t.result_refs).unwrap_or_default()})
    }

    fn context_ref(&self, agent_id: &str) -> Result<String, String> {
        let epoch = self.store.agent_context_epoch(&self.session_id, agent_id).map_err(|e| e.to_string())?;
        Ok(format!("ctx:{agent_id}:{epoch}"))
    }

    fn ensure_goal(&self, spec: &TeamSpec) -> Result<String, String> {
        let session = self.store.get_session(&self.session_id).map_err(|e| e.to_string())?;
        let goal_id = session.as_ref().and_then(|s| s.get("goal_id").and_then(|v| v.as_str()).map(str::to_string));
        let state = session.as_ref().and_then(|s| s.get("goal_state").and_then(|v| v.as_str())).unwrap_or("idle");
        if let Some(gid) = &goal_id {
            if state == "active" {
                return Ok(gid.clone());
            }
            if state == "done" {
                let gid = new_id("goal");
                self.store.set_goal_state(&self.session_id, &gid, "active").map_err(|e| e.to_string())?;
                return Ok(gid);
            }
        }
        let _ = spec;
        let gid = goal_id.unwrap_or_else(|| new_id("goal"));
        self.store.set_goal_state(&self.session_id, &gid, "active").map_err(|e| e.to_string())?;
        Ok(gid)
    }

    fn current_goal_id(&self) -> Result<Option<String>, String> {
        let session = self.store.get_session(&self.session_id).map_err(|e| e.to_string())?;
        Ok(session.and_then(|s| s.get("goal_id").and_then(|v| v.as_str()).map(str::to_string)))
    }

    fn wake_approval_run(&self, run_id: &str) -> Result<(), String> {
        if let Ok(Some(run)) = self.store.get_run(run_id) {
            if run.status == TurnStatus::WaitingApproval {
                self.store
                    .update_run_status_where(run_id, TurnStatus::WaitingApproval, TurnStatus::Running)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

/// Outcome of a finished turn segment.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnOutcome {
    pub status: TurnStatus,
    pub error: Option<String>,
    pub note: Option<String>,
    pub reply_text: Option<String>,
}

impl Control {
    /// Turn start semantics: mark RUNNING, start the
    /// attached task, emit lifecycle events. Returns the fresh run row.
    pub fn begin_run(&mut self, run_id: &str) -> Result<TurnRun, String> {
        self.in_tx(|ctl| ctl.begin_run_inner(run_id))
    }

    fn begin_run_inner(&mut self, run_id: &str) -> Result<TurnRun, String> {
        let mut run = self.store.get_run(run_id).map_err(|e| e.to_string())?.ok_or("unknown run")?;
        if run.status == TurnStatus::Queued {
            self.store.set_run_status(run.run_id.as_str(), TurnStatus::Running).map_err(|e| e.to_string())?;
            run.status = TurnStatus::Running;
        }
        self.store.set_agent_status(&self.session_id, &run.agent_id, AgentStatus::Busy).map_err(|e| e.to_string())?;
        if let Some(task_id) = &run.task_id {
            if let Ok(Some(task)) = self.store.get_task(task_id) {
                if self
                    .store
                    .compare_and_set_task(&task.task_id, "PENDING", TaskStatus::Running, None)
                    .map_err(|e| e.to_string())?
                {
                    let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
                    let action = self.sys_action(ActionKind::SendMessage, &run.agent_id);
                    self.persist_events(
                        &action,
                        &spec,
                        &[EventDraft {
                            kind: EventKind::TaskStarted,
                            payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                            "requester": task.requester, "status": "RUNNING"}),
                            task_id: Some(task.task_id.clone()),
                            actor_id: Some(run.agent_id.clone()),
                            ..EventDraft::new(EventKind::TaskStarted, json!({}))
                        }],
                    )?;
                }
            }
        }
        // Every turn announces its start; the wake
        // reason tells the member why this segment is running.
        let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
        let wake = self.wake_info(&run)["reason"].clone();
        let action = self.sys_action(ActionKind::SendMessage, &run.agent_id);
        self.persist_events(
            &action,
            &spec,
            &[EventDraft {
                kind: EventKind::RunStarted,
                payload: json!({"run_id": run.run_id, "agent_id": run.agent_id,
                                "status": "RUNNING", "wake": wake}),
                actor_id: Some(run.agent_id.clone()),
                ..EventDraft::new(EventKind::RunStarted, json!({}))
            }],
        )?;
        Ok(run)
    }

    /// Stop-request timeout path: mark OUTCOME_UNKNOWN and expire
    /// the run's pending approvals with audit events.
    pub fn stop_timeout(&mut self, run_id: &str) -> Result<(), String> {
        self.in_tx(|ctl| {
            let run = ctl.store.get_run(run_id).map_err(|e| e.to_string())?.ok_or("unknown run")?;
            let changed = ctl
                .store
                .update_run_status_where(run_id, TurnStatus::Running, TurnStatus::OutcomeUnknown)
                .map_err(|e| e.to_string())?;
            if !changed {
                return Ok(());
            }
            let events = ctl.expire_run_approvals(run_id);
            if !events.is_empty() {
                let spec = ctl.store.load_team_spec(&ctl.session_id.clone(), None).map_err(|e| e.to_string())?;
                let action = ctl.sys_action(ActionKind::CancelRun, &run.agent_id);
                ctl.persist_events(&action, &spec, &events)?;
            }
            Ok(())
        })
    }

    /// Drain mid-turn pushes recorded by schedule.
    pub fn drain_mid_turn_pushes(&mut self) -> Vec<(String, Vec<Json>)> {
        std::mem::take(&mut self.mid_turn_pushes)
    }

    /// Why this turn is waking.
    pub fn wake_info(&self, run: &TurnRun) -> Json {
        let decisions = self.store.decided_approvals_for_run(&run.run_id).unwrap_or_default();
        if !decisions.is_empty() {
            let denied = decisions.iter().any(|d| d.status == ApprovalStatus::Denied);
            return json!({"reason": "approval",
                          "payload": {"decisions": decisions.iter().map(|d| json!({
                              "approval_id": d.approval_id, "status": enum_name(d.status)})).collect::<Vec<_>>(),
                              "denied": denied}});
        }
        let kinds = self.store.delivery_event_kinds(&run.input_delivery_ids).unwrap_or_default();
        if kinds.last().map(|k| k.as_str()) == Some("user_message") {
            return json!({"reason": "user_input", "payload": {"kinds": kinds}});
        }
        if !run.waiting_on.is_empty() {
            // results are keyed by task id
            let results: serde_json::Map<String, Json> = run.waiting_on.iter().map(|tid| (tid.clone(), {
                match self.store.get_task(tid).ok().flatten() {
                    Some(t) => json!({"task_id": t.task_id, "status": enum_name(t.status), "result_refs": t.result_refs}),
                    None => json!({"task_id": tid, "status": "UNKNOWN"}),
                }
            })).collect();
            return json!({"reason": "task_results", "payload": {"task_ids": run.waiting_on, "results": results}});
        }
        json!({"reason": "new_input", "payload": {}})
    }

    fn sys_action(&self, kind: ActionKind, actor: &str) -> TeamAction {
        TeamAction {
            action_id: new_id("sys"),
            session_id: self.session_id.clone(),
            actor_id: actor.to_string(),
            run_id: None,
            kind,
            payload: json!({}),
        }
    }

    /// Apply the turn result: completion requests,
    /// task semantics, run status, member state, delivery ack, one transaction.
    /// `ack_ids` = the deliveries actually handed to the runner (RT-05 ledger).
    pub fn finalize_run(&mut self, run_id: &str, outcome: &TurnOutcome, ack_ids: &[i64]) -> Result<(), String> {
        self.in_tx(|ctl| ctl.finalize_run_inner(run_id, outcome, ack_ids))
    }

    fn finalize_run_inner(&mut self, run_id: &str, outcome: &TurnOutcome, ack_ids: &[i64]) -> Result<(), String> {
        let run = self.store.get_run(run_id).map_err(|e| e.to_string())?.ok_or("unknown run")?;
        let req = self.store.completion_request(&run.run_id).map_err(|e| e.to_string())?;
        let terminal = matches!(
            outcome.status,
            TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Cancelled | TurnStatus::OutcomeUnknown
        );
        let mut events: Vec<EventDraft> = vec![];
        if terminal {
            // a turn that ended can never use a pending decision (RT-06)
            events.extend(self.expire_run_approvals(&run.run_id));
        }
        if outcome.status == TurnStatus::Completed {
            if let Some(req) = &req {
                let req_task = req["task_id"].as_str().unwrap_or("");
                let refs: Vec<String> = req["result_refs"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                if !req_task.is_empty() {
                    if let Ok(Some(task)) = self.store.get_task(req_task) {
                        if self
                            .store
                            .compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Succeeded, Some(&refs))
                            .map_err(|e| e.to_string())?
                        {
                            let mut waiters = self.store.waiters_for_task(&self.session_id, &task.task_id).map_err(|e| e.to_string())?;
                            waiters.push(task.requester.clone());
                            waiters.sort();
                            waiters.dedup();
                            events.push(EventDraft {
                                kind: EventKind::TaskCompleted,
                                payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                                "requester": task.requester, "status": "SUCCEEDED",
                                                "result_refs": refs, "summary": req["summary"]}),
                                task_id: Some(task.task_id.clone()),
                                push: Some(waiters),
                                ..EventDraft::new(EventKind::TaskCompleted, json!({}))
                            });
                        }
                    }
                } else {
                    self.store
                        .set_goal_state(&self.session_id, run.goal_id.as_deref().unwrap_or(""), "done")
                        .map_err(|e| e.to_string())?;
                    events.push(EventDraft {
                        kind: EventKind::GoalDone,
                        payload: json!({"goal_id": run.goal_id, "agent_id": run.agent_id, "summary": req["summary"]}),
                        push: Some(vec![]),
                        ..EventDraft::new(EventKind::GoalDone, json!({}))
                    });
                }
            }
        }
        // Outcome COMPLETED with an attached task and (req is None or req
        // completes a *different* task) — its own task was left unfinished.
        let req_other = match &req {
            None => true, // no completion request at all
            Some(r) => {
                let t = r["task_id"].as_str().unwrap_or("");
                !t.is_empty() && run.task_id.as_deref() != Some(t)
            }
        };
        if outcome.status == TurnStatus::Completed && run.task_id.is_some() && req_other {
            // its own task was left unfinished: block it for intervention
            if let Some(tid) = &run.task_id {
                if let Ok(Some(task)) = self.store.get_task(tid) {
                    if matches!(task.status, TaskStatus::Pending | TaskStatus::Running)
                        && self
                            .store
                            .compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Blocked, None)
                            .map_err(|e| e.to_string())?
                    {
                        events.push(EventDraft {
                            kind: EventKind::TaskBlocked,
                            payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                            "requester": task.requester,
                                            "reason": "turn ended without complete_task or wait_for_tasks"}),
                            task_id: Some(task.task_id.clone()),
                            ..EventDraft::new(EventKind::TaskBlocked, json!({}))
                        });
                    }
                }
            }
        } else if outcome.status == TurnStatus::Failed && run.task_id.is_some() {
            if let Some(tid) = &run.task_id {
                if let Ok(Some(task)) = self.store.get_task(tid) {
                    if matches!(task.status, TaskStatus::Pending | TaskStatus::Running)
                        && self
                            .store
                            .compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Failed, None)
                            .map_err(|e| e.to_string())?
                    {
                        events.push(EventDraft {
                            kind: EventKind::TaskFailed,
                            payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                            "requester": task.requester, "error": outcome.error}),
                            task_id: Some(task.task_id.clone()),
                            ..EventDraft::new(EventKind::TaskFailed, json!({}))
                        });
                    }
                }
            }
        } else if outcome.status == TurnStatus::OutcomeUnknown && run.task_id.is_some() {
            if let Some(tid) = &run.task_id {
                if let Ok(Some(task)) = self.store.get_task(tid) {
                    if matches!(task.status, TaskStatus::Pending | TaskStatus::Running) {
                        self.store
                            .compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Blocked, None)
                            .map_err(|e| e.to_string())?;
                        events.push(EventDraft {
                            kind: EventKind::TaskBlocked,
                            payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                            "requester": task.requester,
                                            "reason": outcome.error.clone().unwrap_or_else(|| "external turn outcome could not be confirmed".into())}),
                            task_id: Some(task.task_id.clone()),
                            ..EventDraft::new(EventKind::TaskBlocked, json!({}))
                        });
                    }
                }
            }
        } else if outcome.status == TurnStatus::Cancelled && run.task_id.is_some() {
            if let Some(tid) = &run.task_id {
                if let Ok(Some(task)) = self.store.get_task(tid) {
                    if matches!(task.status, TaskStatus::Pending | TaskStatus::Running)
                        && self
                            .store
                            .compare_and_set_task(&task.task_id, &enum_name(task.status), TaskStatus::Cancelled, None)
                            .map_err(|e| e.to_string())?
                    {
                        events.push(EventDraft {
                            kind: EventKind::TaskCancelled,
                            payload: json!({"task_id": task.task_id, "assignee": task.assignee,
                                            "requester": task.requester, "status": "CANCELLED"}),
                            task_id: Some(task.task_id.clone()),
                            ..EventDraft::new(EventKind::TaskCancelled, json!({}))
                        });
                    }
                }
            }
        }

        // the terminal status, the member state and the delivery acknowledgement
        // are one write: a crash can never leave "run ended + input un-acked" (F-C3)
        self.store.set_run_status(&run.run_id, outcome.status).map_err(|e| e.to_string())?;
        if matches!(outcome.status, TurnStatus::WaitingTask | TurnStatus::WaitingApproval) {
            self.store.set_agent_status(&self.session_id, &run.agent_id, AgentStatus::Waiting).map_err(|e| e.to_string())?;
        } else {
            self.store.set_agent_status(&self.session_id, &run.agent_id, AgentStatus::Idle).map_err(|e| e.to_string())?;
        }
        if terminal && !ack_ids.is_empty() {
            // ack exactly the deliveries handed to the runner (RT-05)
            for id in ack_ids {
                self.store.ack_delivery_by_id(*id).map_err(|e| e.to_string())?;
            }
        }

        if outcome.status == TurnStatus::WaitingApproval {
            let pending: Vec<_> = self
                .store
                .pending_approvals_for_run(&run.run_id)
                .map_err(|e| e.to_string())?;
            if pending.is_empty() {
                // 决定先到、停下后到：这个回合已经等不到人来决定了（PENDING 行不存在），
                // 停在 WAITING_APPROVAL 会永远醒不过来 —— 直接回到 RUNNING 让它继续跑，
                // 工具调用会在网关里读到那条已决定的行（允许或拒绝）。
                // 场景：用户/自动化在请求刚出现时就拍板（CI 上偶发，见 chat_e2e 审批用例）。
                self.store.set_run_status(&run.run_id, TurnStatus::Running).map_err(|e| e.to_string())?;
            }
            for a in pending {
                events.push(EventDraft::new(
                    EventKind::ApprovalRequested,
                    json!({"approval_id": a.approval_id, "agent_id": a.agent_id,
                           "run_id": a.run_id, "scope": a.requested_scope}),
                ));
            }
        }

        if terminal {
            if outcome.note.as_deref() == Some("turn_limit") {
                events.push(EventDraft::new(
                    EventKind::LimitReached,
                    json!({"kind": "max_model_steps_per_turn", "run_id": run.run_id,
                           "agent_id": run.agent_id, "detail": outcome.error}),
                ));
            }
            let leader = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?.leader_id;
            if outcome.status == TurnStatus::Completed && outcome.reply_text.is_some() && run.agent_id == leader {
                events.push(EventDraft::new(
                    EventKind::LeaderReply,
                    json!({"text": outcome.reply_text, "run_id": run.run_id}),
                ));
            } else if outcome.status == TurnStatus::Completed && outcome.reply_text.is_some() {
                let text: String = outcome.reply_text.clone().unwrap_or_default().chars().take(2000).collect();
                events.push(EventDraft::new(
                    EventKind::RunProgress,
                    json!({"run_id": run.run_id, "agent_id": run.agent_id,
                           "text": text, "final": true, "task_id": run.task_id}),
                ));
            }
            let kind = match outcome.status {
                TurnStatus::Completed => EventKind::RunCompleted,
                TurnStatus::Failed | TurnStatus::OutcomeUnknown => EventKind::RunFailed,
                TurnStatus::Cancelled => EventKind::RunCancelled,
                _ => EventKind::RunFailed,
            };
            events.push(EventDraft::new(
                kind,
                json!({"run_id": run.run_id, "agent_id": run.agent_id,
                       "status": enum_name(outcome.status), "error": outcome.error}),
            ));
        }
        if !events.is_empty() {
            let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
            let action = self.sys_action(ActionKind::SendMessage, &run.agent_id);
            self.persist_events(&action, &spec, &events)?;
        }
        let spec = self.store.load_team_spec(&self.session_id, None).map_err(|e| e.to_string())?;
        self.schedule_inner(&spec)
    }
}

/// Enum wire string (the stable `.value` form).
pub fn enum_name<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn op_type(v: &Json) -> &'static str {
    match v {
        Json::Null => "null",
        Json::Bool(_) => "bool",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

fn new_id(prefix: &str) -> String {
    crate::models::new_id(prefix)
}

fn now() -> f64 {
    crate::models::now()
}
