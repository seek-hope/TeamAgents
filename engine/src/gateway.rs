//! Tool permissions and the single tool-call path for members
//! (permissions.py + agents.py::ToolGateway).

use crate::core_client::CoreClient;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use teamagents_core::models::{ActionKind, ApprovalRequest, ApprovalStatus, Receipt};

pub const TEAM_TOOLS: &[&str] = &[
    "send_message",
    "assign_task",
    "complete_task",
    "wait_for_tasks",
    "publish_shared",
    "read_shared",
    "list_shared",
    "request_help",
    "propose_team_change",
    "apply_topology_patch",
    "signal_done",
    "cancel_task",
    "cancel_run",
];

fn team_action_kind(tool: &str) -> Option<ActionKind> {
    Some(match tool {
        "send_message" => ActionKind::SendMessage,
        "assign_task" => ActionKind::AssignTask,
        "complete_task" => ActionKind::CompleteTask,
        "wait_for_tasks" => ActionKind::WaitForTasks,
        "publish_shared" => ActionKind::PublishShared,
        "read_shared" => ActionKind::ReadShared,
        "list_shared" => ActionKind::ListShared,
        "request_help" => ActionKind::RequestHelp,
        "propose_team_change" => ActionKind::ProposeTeamChange,
        "apply_topology_patch" => ActionKind::ApplyTopologyPatch,
        "signal_done" => ActionKind::SignalDone,
        "cancel_task" => ActionKind::CancelTask,
        "cancel_run" => ActionKind::CancelRun,
        _ => return None,
    })
}

/// Byte-compatible with Python `json.dumps(..., sort_keys=True, ensure_ascii=False)`:
/// `", "` / `": "` separators and raw UTF-8 (permissions.py::operation_hash).
/// The hash must match across versions because approvals share one DB (D-15).
fn canonical_json(v: &Json) -> String {
    match v {
        Json::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}: {}", serde_json::to_string(k).unwrap(), canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        Json::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(", ")),
        other => other.to_string(),
    }
}

/// permissions.py::operation_hash — approval is bound to the operation and its
/// parameters (plan §12.2).
pub fn operation_hash(tool: &str, args: &Json) -> String {
    let canonical = canonical_json(&json!({"tool": tool, "args": args}));
    let hex = format!("{:x}", Sha256::digest(canonical.as_bytes()));
    hex[..32].to_string()
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub allow: bool,
    pub scope: Option<Json>,
    pub reason: Option<String>,
}

impl Decision {
    fn allow() -> Self {
        Self { allow: true, scope: None, reason: None }
    }
    fn deny(scope: Json, reason: impl Into<String>) -> Self {
        Self { allow: false, scope: Some(scope), reason: Some(reason.into()) }
    }
}

pub struct PermissionPolicy {
    pub mode: String, // "approved_scope" | "full_auto"
    pub pre_authorized: HashSet<String>,
    pub require_approval: HashSet<String>,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            mode: "approved_scope".into(),
            pre_authorized: ["files", "shell"].iter().map(|s| s.to_string()).collect(),
            require_approval: HashSet::new(),
        }
    }
}

impl PermissionPolicy {
    pub fn evaluate(&self, tool: &str, args: &Json) -> Decision {
        if self.mode == "full_auto" {
            return Decision::allow();
        }
        let shell_network = args.get("network").and_then(|v| v.as_bool()).unwrap_or(false);
        if tool == "shell" && shell_network {
            return Decision::deny(
                json!({"tool": tool, "args": args, "reason": "shell network access is off by default"}),
                "shell network access requires approval",
            );
        }
        if self.require_approval.contains(tool) {
            return Decision::deny(
                json!({"tool": tool, "args": args, "reason": format!("{tool} needs approval")}),
                "outside pre-authorized scope",
            );
        }
        if self.pre_authorized.contains(tool) || bound_tool(tool) {
            return Decision::allow();
        }
        if tool.starts_with("mcp_") || tool.starts_with("web_") {
            return Decision::allow();
        }
        Decision::deny(
            json!({"tool": tool, "args": args, "reason": format!("{tool} is not pre-authorized")}),
            "tool not in approved scope",
        )
    }
}

/// Runtime-bound execution tools (file tools stay inside the sandboxed backend).
fn bound_tool(tool: &str) -> bool {
    matches!(
        tool,
        "ls" | "read_file" | "write_file" | "edit_file" | "delete" | "glob" | "grep" | "read_artifact" | "skill"
    )
}

fn approval_from(value: Option<&Json>) -> Option<ApprovalRequest> {
    value
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub struct ApprovalGate {
    pub policy_revision: Mutex<i64>,
    policy: Mutex<PermissionPolicy>,
    core: Arc<CoreClient>,
    /// (run_id, operation_hash) → the approval row this gate parked the call
    /// on. Python's graph replay re-uses the original tool_call_id; the plain
    /// loop gets a fresh one, so the row is remembered instead of re-matched
    /// by call id.
    parked: Mutex<std::collections::HashMap<(String, String), String>>,
}

impl ApprovalGate {
    pub fn new(core: Arc<CoreClient>, policy: PermissionPolicy) -> Arc<Self> {
        let mode_full_auto = policy.mode == "full_auto";
        let gate = Arc::new(Self {
            policy_revision: Mutex::new(1),
            policy: Mutex::new(policy),
            core,
            parked: Mutex::new(std::collections::HashMap::new()),
        });
        if mode_full_auto {
            // nothing extra: the policy already allows everything
        }
        gate
    }

    pub fn mode(&self) -> String {
        self.policy.lock().map(|p| p.mode.clone()).unwrap_or_else(|_| "approved_scope".into())
    }

    pub fn set_mode(&self, mode: &str) {
        if let Ok(mut policy) = self.policy.lock() {
            policy.mode = mode.to_string();
        }
        if let Ok(mut revision) = self.policy_revision.lock() {
            *revision += 1;
        }
    }

    /// The session row is authoritative: a user toggling full-auto in the TUI
    /// (set_permission_mode) must change this gate without a restart.
    fn refresh_mode(&self) {
        let Ok(reply) = self.core.call_in_session("session_mode", json!({})) else { return };
        let Some(mode) = reply.get("mode").and_then(|v| v.as_str()) else { return };
        let changed = self.policy.lock().map(|policy| policy.mode != mode).unwrap_or(false);
        if changed {
            self.set_mode(mode);
        }
    }

    pub fn revision(&self) -> i64 {
        self.policy_revision.lock().map(|r| *r).unwrap_or(1)
    }

    /// Returns (decision, approval). A PENDING approval means: pause.
    pub fn check(
        &self,
        agent_id: &str,
        run_id: &str,
        tool: &str,
        args: &Json,
        tool_call_id: &str,
    ) -> Result<(Decision, Option<ApprovalRequest>), String> {
        self.refresh_mode();
        let decision = self
            .policy
            .lock()
            .map_err(|_| "policy lock poisoned".to_string())?
            .evaluate(tool, args);
        if decision.allow {
            return Ok((decision, None));
        }
        let op_hash = operation_hash(tool, args);
        let cached = self
            .core
            .call_in_session("approval_find_session", json!({"operation_hash": op_hash}))?;
        if !cached.get("scope").map(|v| v.is_null()).unwrap_or(true) {
            return Ok((Decision::allow(), None));
        }
        if let Some(existing) = self.decided_for_run(run_id, tool_call_id, &op_hash) {
            match existing.status {
                ApprovalStatus::Pending => {
                    self.remember(run_id, &op_hash, &existing.approval_id);
                    return Ok((decision, Some(existing)));
                }
                ApprovalStatus::Denied => {
                    let denied = Decision { allow: false, scope: decision.scope.clone(), reason: Some("denied by the user".into()) };
                    return Ok((denied, Some(existing)));
                }
                // only an unconsumed once/session approval with the same policy
                // revision authorizes the call; EXPIRED asks again (permissions.py)
                ApprovalStatus::ApprovedOnce | ApprovalStatus::ApprovedSession
                    if existing.policy_revision == self.revision() =>
                {
                    return Ok((Decision::allow(), Some(existing)))
                }
                _ => {}
            }
        }
        let request = ApprovalRequest {
            approval_id: teamagents_core::models::new_id("appr"),
            session_id: self.core.session_id.clone(),
            agent_id: agent_id.to_string(),
            run_id: run_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            operation_hash: op_hash.clone(),
            requested_scope: decision
                .scope
                .clone()
                .unwrap_or_else(|| json!({"tool": tool, "args": args})),
            policy_revision: self.revision(),
            status: ApprovalStatus::Pending,
            created_at: teamagents_core::models::now(),
            decided_at: None,
        };
        self.core.call_in_session("insert_approval", json!({"approval": request}))?;
        self.remember(run_id, &op_hash, &request.approval_id);
        Ok((decision, Some(request)))
    }

    fn remember(&self, run_id: &str, op_hash: &str, approval_id: &str) {
        self.parked
            .lock()
            .unwrap()
            .insert((run_id.to_string(), op_hash.to_string()), approval_id.to_string());
    }

    /// The decision recorded for this run+operation, whatever tool_call_id the
    /// model re-sent it under. Sources, in order: the row this gate parked on
    /// (any status — the only way a DENIED/EXPIRED answer survives a fresh call
    /// id), then the core's newest still-usable row for the run, then the
    /// exact-call lookup for a core without the `approval_find_run` contract.
    fn decided_for_run(&self, run_id: &str, tool_call_id: &str, op_hash: &str) -> Option<ApprovalRequest> {
        let parked = self
            .parked
            .lock()
            .unwrap()
            .get(&(run_id.to_string(), op_hash.to_string()))
            .cloned();
        if let Some(approval_id) = parked {
            if let Ok(reply) = self.core.call_in_session("get_approval", json!({"approval_id": approval_id})) {
                if let Some(row) = approval_from(reply.get("approval")) {
                    // EXPIRED is the absence of a decision: ask again
                    if row.status != ApprovalStatus::Expired {
                        return Some(row);
                    }
                }
            }
        }
        if let Ok(reply) = self
            .core
            .call_in_session("approval_find_run", json!({"run_id": run_id, "operation_hash": op_hash}))
        {
            return approval_from(reply.get("approval"));
        }
        self.core
            .call("approval_for_call", json!({
                "session_id": self.core.session_id,
                "run_id": run_id,
                "tool_call_id": tool_call_id,
                "operation_hash": op_hash,
            }))
            .ok()
            .and_then(|v| approval_from(v.get("approval")))
    }

    /// permissions.py::consume_once — a once-approval is single use, consumed
    /// after the operation ran (success or failure).
    pub fn consume_once(&self, approval_id: &str) {
        if let Err(e) = self.core.call_in_session("expire_approval", json!({"approval_id": approval_id})) {
            eprintln!("teamagents: approval {approval_id} could not be consumed: {e}");
        }
        self.parked.lock().unwrap().retain(|_, id| id != approval_id);
    }

    /// Void a pending approval whose waiter is gone (a declined codex approval
    /// or an abandoned turn) so the core never shows a dead PENDING row.
    pub fn expire(&self, approval_id: &str) {
        self.consume_once(approval_id);
    }
}

type Executor = Arc<dyn Fn(&str, &Json) -> Result<Json, String> + Send + Sync>;

/// Revocable ownership for a turn segment. The mutex drains tools and private
/// checkpoint writes before the session releases its execution lock.
#[derive(Default)]
pub struct TurnControl {
    cancelled: AtomicBool,
    active: Mutex<()>,
}

impl TurnControl {
    pub fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) { Err("turn interrupted".into()) } else { Ok(()) }
    }

    pub fn enter(&self) -> Result<MutexGuard<'_, ()>, String> {
        let guard = self.active.lock().map_err(|_| "turn execution lock poisoned")?;
        self.check()?;
        Ok(guard)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.active.try_lock() {
                Ok(_) | Err(std::sync::TryLockError::Poisoned(_)) => return true,
                Err(std::sync::TryLockError::WouldBlock) => {}
            }
            if Instant::now() >= deadline { return false; }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The single path for a member's tool calls, team actions and approvals.
/// Identity is injected here, never trusted from model fields (§5.2).
pub struct ToolGateway {
    core: Arc<CoreClient>,
    pub agent_id: String,
    pub run_id: String,
    approvals: Arc<ApprovalGate>,
    executor: Option<Executor>,
    pub pending_approval_id: Mutex<Option<String>>,
    pub control: Arc<TurnControl>,
    /// D-30: run before an apply_topology_patch submit; may rewrite the payload
    /// (auto-created member profiles) or veto it with Err.
    topology_prepare: Option<Arc<dyn Fn(&mut Json) -> Result<(), String> + Send + Sync>>,
}

impl ToolGateway {
    pub fn new(
        core: Arc<CoreClient>,
        agent_id: &str,
        run_id: &str,
        approvals: Arc<ApprovalGate>,
        executor: Option<Executor>,
    ) -> Arc<Self> {
        Self::with_control(core, agent_id, run_id, approvals, executor, Arc::new(TurnControl::default()), None)
    }

    pub(crate) fn with_control(
        core: Arc<CoreClient>, agent_id: &str, run_id: &str,
        approvals: Arc<ApprovalGate>, executor: Option<Executor>, control: Arc<TurnControl>,
        topology_prepare: Option<Arc<dyn Fn(&mut Json) -> Result<(), String> + Send + Sync>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            agent_id: agent_id.to_string(),
            run_id: run_id.to_string(),
            approvals,
            executor,
            pending_approval_id: Mutex::new(None),
            control,
            topology_prepare,
        })
    }

    fn receipt_placeholder(&self, call_id: &str, ok: bool, result: Json, error: Option<String>) -> Receipt {
        Receipt {
            action_id: call_id.to_string(),
            ok,
            kind: ActionKind::CompleteTask,
            result,
            error,
        }
    }

    pub fn call(&self, tool: &str, args: &Json, tool_call_id: &str) -> Receipt {
        let call_id = format!("{}:{}", self.run_id, tool_call_id);
        let _execution = match self.control.enter() {
            Ok(guard) => guard,
            Err(e) => return self.receipt_placeholder(&call_id, false, json!({}), Some(e)),
        };
        if let Some(kind) = team_action_kind(tool) {
            let mut payload = args.clone();
            if tool == "apply_topology_patch" && payload.get("reject").and_then(|v| v.as_bool()) != Some(true) {
                if let Some(prepare) = &self.topology_prepare {
                    if let Err(e) = prepare(&mut payload) {
                        return self.receipt_placeholder(&call_id, false, json!({}), Some(e));
                    }
                }
            }
            let action = teamagents_core::models::TeamAction {
                action_id: call_id.clone(),
                session_id: self.core.session_id.clone(),
                actor_id: self.agent_id.clone(),
                run_id: Some(self.run_id.clone()),
                kind,
                payload,
            };
            return match self.core.submit(&action) {
                Ok(receipt) => receipt,
                Err(e) => self.receipt_placeholder(&call_id, false, json!({}), Some(e)),
            };
        }
        let (decision, approval) = match self.approvals.check(&self.agent_id, &self.run_id, tool, args, &call_id) {
            Ok(pair) => pair,
            Err(e) => return self.receipt_placeholder(&call_id, false, json!({}), Some(e)),
        };
        let mut consume_once: Option<String> = None;
        if let Some(approval) = &approval {
            match approval.status {
                ApprovalStatus::Pending if !decision.allow => {
                    if let Ok(mut pending) = self.pending_approval_id.lock() {
                        *pending = Some(approval.approval_id.clone());
                    }
                    return self.receipt_placeholder(
                        &call_id,
                        false,
                        json!({"approval_id": approval.approval_id, "scope": approval.requested_scope}),
                        Some("approval_required".into()),
                    );
                }
                ApprovalStatus::Denied => {
                    return self.receipt_placeholder(
                        &call_id,
                        false,
                        json!({}),
                        Some("The user denied this operation. Do not retry it; choose another approach or ask the user.".into()),
                    );
                }
                // consumed after the operation runs, even when it fails
                // (runners.py: handler then consume_once)
                ApprovalStatus::ApprovedOnce => consume_once = Some(approval.approval_id.clone()),
                _ => {}
            }
        }
        if !decision.allow {
            return self.receipt_placeholder(
                &call_id,
                false,
                json!({}),
                Some(decision.reason.unwrap_or_else(|| "operation not permitted".into())),
            );
        }
        let Some(executor) = &self.executor else {
            return self.receipt_placeholder(&call_id, false, json!({}), Some(format!("no executor for tool {tool}")));
        };
        let receipt = match executor(tool, args) {
            Ok(output) => self.receipt_placeholder(&call_id, true, json!({"output": output}), None),
            Err(e) => self.receipt_placeholder(&call_id, false, json!({}), Some(e)),
        };
        if let Some(approval_id) = consume_once {
            self.approvals.consume_once(&approval_id);
        }
        receipt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_canonical_and_stable() {
        let a = operation_hash("shell", &json!({"command": "ls", "network": false}));
        let b = operation_hash("shell", &json!({"network": false, "command": "ls"}));
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert_ne!(a, operation_hash("shell", &json!({"command": "rm -rf /"})));
    }

    #[test]
    fn operation_hash_matches_python_json_dumps() {
        // Vectors from .venv/bin/python:
        //   hashlib.sha256(json.dumps({"tool": t, "args": a}, sort_keys=True,
        //                              ensure_ascii=False).encode()).hexdigest()[:32]
        assert_eq!(
            operation_hash("shell", &json!({"command": "ls", "network": false})),
            "3b029cd4d67fd563bc49494e01f94404"
        );
        assert_eq!(
            operation_hash("shell", &json!({"command": "echo 你好"})),
            "7e78ae54357806e4c55d0a7b8cad4a8d"
        );
        assert_eq!(
            operation_hash("web_fetch", &json!({"url": "https://example.com/a?b=1", "n": null})),
            "c8ce429c3fc12927f239b7e72c84c5a3"
        );
    }

    #[test]
    fn policy_defaults_match_python() {
        let policy = PermissionPolicy::default();
        assert!(policy.evaluate("shell", &json!({"command": "ls"})).allow);
        assert!(!policy.evaluate("shell", &json!({"command": "curl x", "network": true})).allow);
        assert!(policy.evaluate("read_file", &json!({"path": "a"})).allow);
        assert!(!policy.evaluate("something_else", &json!({})).allow);
        let full_auto = PermissionPolicy { mode: "full_auto".into(), ..Default::default() };
        assert!(full_auto.evaluate("something_else", &json!({})).allow);
    }
}
