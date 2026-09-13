//! Tool permissions and the single tool-call path for members
//! (permissions.py + agents.py::ToolGateway).

use crate::core_client::CoreClient;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
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

fn canonical_json(v: &Json) -> String {
    match v {
        Json::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", serde_json::to_string(k).unwrap(), canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Json::Array(a) => format!("[{}]", a.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
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
        "ls" | "read_file" | "write_file" | "edit_file" | "delete" | "glob" | "grep" | "read_artifact"
    )
}

pub struct ApprovalGate {
    pub policy_revision: Mutex<i64>,
    policy: Mutex<PermissionPolicy>,
    core: Arc<CoreClient>,
}

impl ApprovalGate {
    pub fn new(core: Arc<CoreClient>, policy: PermissionPolicy) -> Arc<Self> {
        let mode_full_auto = policy.mode == "full_auto";
        let gate = Arc::new(Self { policy_revision: Mutex::new(1), policy: Mutex::new(policy), core });
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
        let existing: Option<ApprovalRequest> = self
            .core
            .call("approval_for_call", json!({
                "session_id": self.core.session_id,
                "run_id": run_id,
                "tool_call_id": tool_call_id,
                "operation_hash": op_hash,
            }))
            .ok()
            .and_then(|v| serde_json::from_value(v.get("approval").cloned().unwrap_or(Json::Null)).ok())
            .flatten();
        if let Some(existing) = &existing {
            match existing.status {
                ApprovalStatus::Pending => return Ok((decision, existing.clone().into())),
                ApprovalStatus::Denied => {
                    let denied = Decision { allow: false, scope: decision.scope.clone(), reason: Some("denied by the user".into()) };
                    return Ok((denied, Some(existing.clone())));
                }
                _ if existing.policy_revision == self.revision() => {
                    return Ok((Decision::allow(), Some(existing.clone())))
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
            operation_hash: op_hash,
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
        Ok((decision, Some(request)))
    }
}

type Executor = Arc<dyn Fn(&str, &Json) -> Result<Json, String> + Send + Sync>;

/// The single path for a member's tool calls, team actions and approvals.
/// Identity is injected here, never trusted from model fields (§5.2).
pub struct ToolGateway {
    core: Arc<CoreClient>,
    pub agent_id: String,
    pub run_id: String,
    approvals: Arc<ApprovalGate>,
    executor: Option<Executor>,
    pub pending_approval_id: Mutex<Option<String>>,
}

impl ToolGateway {
    pub fn new(
        core: Arc<CoreClient>,
        agent_id: &str,
        run_id: &str,
        approvals: Arc<ApprovalGate>,
        executor: Option<Executor>,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            agent_id: agent_id.to_string(),
            run_id: run_id.to_string(),
            approvals,
            executor,
            pending_approval_id: Mutex::new(None),
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
        if let Some(kind) = team_action_kind(tool) {
            let action = teamagents_core::models::TeamAction {
                action_id: call_id.clone(),
                session_id: self.core.session_id.clone(),
                actor_id: self.agent_id.clone(),
                run_id: Some(self.run_id.clone()),
                kind,
                payload: args.clone(),
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
        if let Some(approval) = &approval {
            if !decision.allow && approval.status == ApprovalStatus::Pending {
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
        match executor(tool, args) {
            Ok(output) => self.receipt_placeholder(&call_id, true, json!({"output": output}), None),
            Err(e) => self.receipt_placeholder(&call_id, false, json!({}), Some(e)),
        }
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
