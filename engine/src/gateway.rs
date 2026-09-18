//! Tool permissions and the single tool-call path for members.

use crate::core_client::CoreClient;
use serde_json::{json, Value as Json};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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

/// Canonical JSON for hashing: sorted keys, `", "` / `": "` separators and raw
/// UTF-8 (unescaped non-ASCII). The hash must stay stable across releases
/// because approvals share one DB (D-15).
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

/// Approval is bound to the operation and its parameters (plan §12.2).
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
        "ls" | "read_file"
            | "write_file"
            | "edit_file"
            | "edit_files"
            | "delete"
            | "glob"
            | "grep"
            | "read_artifact"
            | "view_image"
            | "skill"
    )
}

fn approval_from(value: Option<&Json>) -> Option<ApprovalRequest> {
    value.filter(|v| !v.is_null()).and_then(|v| serde_json::from_value(v.clone()).ok())
}

pub struct ApprovalGate {
    pub policy_revision: Mutex<i64>,
    policy: Mutex<PermissionPolicy>,
    core: Arc<CoreClient>,
    /// (run_id, operation_hash) → the approval row this gate parked the call
    /// on. A resumed call gets a fresh tool_call_id, so the row is remembered
    /// instead of re-matched by call id.
    parked: Mutex<std::collections::HashMap<(String, String), String>>,
    /// op_hashes with a session approval the gate itself observed at the
    /// current policy revision. The core's session_approval_cache rows carry
    /// no revision, so they authorize a call only while a same-revision grant
    /// is on record here; set_mode clears the record (review 2026-09-15: a
    /// stale row must not bypass the policy-revision guard).
    session_grants: Mutex<HashSet<String>>,
}

impl ApprovalGate {
    pub fn new(core: Arc<CoreClient>, policy: PermissionPolicy) -> Arc<Self> {
        let mode_full_auto = policy.mode == "full_auto";
        let gate = Arc::new(Self {
            policy_revision: Mutex::new(1),
            policy: Mutex::new(policy),
            core,
            parked: Mutex::new(std::collections::HashMap::new()),
            session_grants: Mutex::new(HashSet::new()),
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
        // bump and clear under the grants lock: no reader can observe a new
        // revision with a stale grant (nesting order grants→revision, as in
        // note_session_grant); a new revision re-asks for everything the
        // session cache used to allow
        let mut grants = self.session_grants.lock().unwrap();
        if let Ok(mut revision) = self.policy_revision.lock() {
            *revision += 1;
        }
        grants.clear();
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

    /// A session grant authorizes this operation only while both the
    /// persisted cache row and the in-memory revision-scoped grant agree:
    /// set_mode clears the set, so a stale row alone cannot authorize (§12.2).
    pub fn session_grant_active(&self, op_hash: &str) -> bool {
        let cached = self.core.call_in_session("approval_find_session", json!({"operation_hash": op_hash}));
        matches!(cached, Ok(reply) if !reply.get("scope").map(|v| v.is_null()).unwrap_or(true))
            && self.session_grants.lock().unwrap().contains(op_hash)
    }

    /// Register a session grant decided outside this gate's own check path
    /// (the codex runner's approval waiter). Void when the policy revision
    /// moved since the request was parked — the same guard ToolGateway::check
    /// applies after inserting.
    pub fn note_session_grant(&self, op_hash: &str, request_revision: i64) {
        let mut grants = self.session_grants.lock().unwrap();
        grants.insert(op_hash.to_string());
        if request_revision != self.revision() {
            grants.remove(op_hash);
        }
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
        let decision = self.policy.lock().map_err(|_| "policy lock poisoned".to_string())?.evaluate(tool, args);
        if decision.allow {
            return Ok((decision, None));
        }
        let op_hash = operation_hash(tool, args);
        let cached = self.core.call_in_session("approval_find_session", json!({"operation_hash": op_hash}))?;
        if !cached.get("scope").map(|v| v.is_null()).unwrap_or(true)
            && self.session_grants.lock().unwrap().contains(&op_hash)
        {
            return Ok((Decision::allow(), None));
        }
        if let Some(existing) = self.decided_for_run(run_id, tool_call_id, &op_hash) {
            match existing.status {
                ApprovalStatus::Pending => {
                    self.remember(run_id, &op_hash, &existing.approval_id);
                    return Ok((decision, Some(existing)));
                }
                ApprovalStatus::Denied => {
                    let denied = Decision {
                        allow: false,
                        scope: decision.scope.clone(),
                        reason: Some("denied by the user".into()),
                    };
                    return Ok((denied, Some(existing)));
                }
                // only an unconsumed once/session approval with the same policy
                // revision authorizes the call; EXPIRED asks again
                ApprovalStatus::ApprovedOnce | ApprovalStatus::ApprovedSession
                    if existing.policy_revision == self.revision() =>
                {
                    if existing.status == ApprovalStatus::ApprovedSession {
                        let mut grants = self.session_grants.lock().unwrap();
                        grants.insert(op_hash.clone());
                        // recheck after inserting: set_mode may have bumped the
                        // revision and cleared the set between the guard above
                        // and this insert — this grant would then be stale for
                        // the new revision. If set_mode lands after this check,
                        // its clear() removes the grant, so either order is safe.
                        if existing.policy_revision != self.revision() {
                            grants.remove(&op_hash);
                        }
                    }
                    return Ok((Decision::allow(), Some(existing)));
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
            requested_scope: decision.scope.clone().unwrap_or_else(|| json!({"tool": tool, "args": args})),
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
        self.parked.lock().unwrap().insert((run_id.to_string(), op_hash.to_string()), approval_id.to_string());
    }

    /// The decision recorded for this run+operation, whatever tool_call_id the
    /// model re-sent it under. Sources, in order: the row this gate parked on
    /// (any status — the only way a DENIED/EXPIRED answer survives a fresh call
    /// id), then the core's newest still-usable row for the run, then the
    /// exact-call lookup for a core without the `approval_find_run` contract.
    fn decided_for_run(&self, run_id: &str, tool_call_id: &str, op_hash: &str) -> Option<ApprovalRequest> {
        let parked = self.parked.lock().unwrap().get(&(run_id.to_string(), op_hash.to_string())).cloned();
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
        if let Ok(reply) =
            self.core.call_in_session("approval_find_run", json!({"run_id": run_id, "operation_hash": op_hash}))
        {
            return approval_from(reply.get("approval"));
        }
        self.core
            .call(
                "approval_for_call",
                json!({
                    "session_id": self.core.session_id,
                    "run_id": run_id,
                    "tool_call_id": tool_call_id,
                    "operation_hash": op_hash,
                }),
            )
            .ok()
            .and_then(|v| approval_from(v.get("approval")))
    }

    /// A once-approval is single use, consumed
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
        if self.cancelled.load(Ordering::SeqCst) {
            Err("turn interrupted".into())
        } else {
            Ok(())
        }
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
            if Instant::now() >= deadline {
                return false;
            }
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
    /// User policy hook (`[hooks] pre_tool`): can veto a native tool call.
    hooks: Option<Arc<crate::hooks::Hooks>>,
}

impl ToolGateway {
    pub fn new(
        core: Arc<CoreClient>,
        agent_id: &str,
        run_id: &str,
        approvals: Arc<ApprovalGate>,
        executor: Option<Executor>,
    ) -> Arc<Self> {
        Self::with_control(core, agent_id, run_id, approvals, executor, Arc::new(TurnControl::default()), None, None)
    }

    pub(crate) fn with_control(
        core: Arc<CoreClient>,
        agent_id: &str,
        run_id: &str,
        approvals: Arc<ApprovalGate>,
        executor: Option<Executor>,
        control: Arc<TurnControl>,
        topology_prepare: Option<Arc<dyn Fn(&mut Json) -> Result<(), String> + Send + Sync>>,
        hooks: Option<Arc<crate::hooks::Hooks>>,
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
            hooks,
        })
    }

    fn receipt_placeholder(&self, call_id: &str, ok: bool, result: Json, error: Option<String>) -> Receipt {
        Receipt { action_id: call_id.to_string(), ok, kind: ActionKind::CompleteTask, result, error }
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
                        Some(
                            "The user denied this operation. Do not retry it; choose another approach or ask the user."
                                .into(),
                        ),
                    );
                }
                // consumed after the operation runs, even when it fails
                // handler, then consume_once
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
        // user policy before the sandbox: a pre_tool hook can veto this call
        if let Some(hooks) = self.hooks.as_ref().filter(|hooks| hooks.has_pre_tool()) {
            let payload = json!({"agent_id": self.agent_id, "run_id": self.run_id, "tool": tool, "arguments": args});
            if let Some(reason) = hooks.deny_reason(&payload) {
                return self.receipt_placeholder(
                    &call_id,
                    false,
                    json!({"denied_by": "pre_tool_hook"}),
                    Some(format!("denied by pre_tool hook: {reason}")),
                );
            }
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

    /// `[hooks] pre_tool` is a real policy gate: a denied call must not reach the
    /// executor at all, and the reason must reach the model.
    #[test]
    fn pre_tool_hook_denies_before_the_executor_runs() {
        let dir = std::env::temp_dir().join(format!("ta-gate-hook-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("deny.sh");
        std::fs::write(&script, "#!/bin/sh\ncat > /dev/null\necho 'write_file is banned by policy' >&2\nexit 2\n")
            .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut config = teamagents_core::models::UserConfig::default();
        config.hooks.pre_tool = vec![script.to_string_lossy().into_owned()];
        let hooks = crate::hooks::Hooks::from_config(&config, "s-hook").expect("configured hook");

        let core = crate::core_client::CoreClient::open(":memory:", "s-hook").unwrap();
        core.call("create_session", json!({"session_id": "s-hook", "cwd": dir.to_string_lossy()})).unwrap();
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = ran.clone();
        let executor: Executor = Arc::new(move |_tool: &str, _args: &Json| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"output": "ran"}))
        });
        let gateway = ToolGateway::with_control(
            core.clone(),
            "leader",
            "run_1",
            ApprovalGate::new(core.clone(), PermissionPolicy::default()),
            Some(executor),
            Arc::new(TurnControl::default()),
            None,
            Some(hooks),
        );
        let receipt = gateway.call("write_file", &json!({"path": "a.txt", "content": "x"}), "call_1");
        assert!(!receipt.ok, "{receipt:?}");
        let error = receipt.error.clone().unwrap_or_default();
        assert!(error.contains("denied by pre_tool hook") && error.contains("banned by policy"), "{error}");
        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0, "the executor never saw the call");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hash_is_canonical_and_stable() {
        let a = operation_hash("shell", &json!({"command": "ls", "network": false}));
        let b = operation_hash("shell", &json!({"network": false, "command": "ls"}));
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        assert_ne!(a, operation_hash("shell", &json!({"command": "rm -rf /"})));
    }

    #[test]
    fn operation_hash_matches_golden_wire_vectors() {
        // Golden wire vectors: sha256 of the canonical JSON (sorted keys,
        // ", " / ": " separators, raw UTF-8), hex, first 32 chars.
        assert_eq!(
            operation_hash("shell", &json!({"command": "ls", "network": false})),
            "3b029cd4d67fd563bc49494e01f94404"
        );
        assert_eq!(operation_hash("shell", &json!({"command": "echo 你好"})), "7e78ae54357806e4c55d0a7b8cad4a8d");
        assert_eq!(
            operation_hash("web_fetch", &json!({"url": "https://example.com/a?b=1", "n": null})),
            "c8ce429c3fc12927f239b7e72c84c5a3"
        );
    }

    /// Review 2026-09-15: a session-scoped approval must stop authorizing once
    /// the policy revision moves (probe /tmp/ta-probe-gate).
    #[test]
    fn session_approval_does_not_survive_a_policy_revision_change() {
        let session = "gate-rev-guard";
        let core = CoreClient::open(":memory:", session).expect("core");
        core.call("create_session", json!({"session_id": session, "cwd": "/tmp"})).expect("create");
        core.call(
            "set_catalog",
            json!({"session_id": session, "catalog": json!({
                "models": {"m": {"provider": "openai", "protocol": "openai", "model": "test"}},
                "tools": {}, "skills_paths": [], "instruction_files": [],
            })}),
        )
        .expect("catalog");
        core.call("save_spec", json!({"session_id": session, "spec": json!({
            "leader_id": "a",
            "agents": [{"id": "a", "name": "A", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"}],
        })})).expect("spec");
        let gate = ApprovalGate::new(core.clone(), PermissionPolicy::default());
        let args = json!({"command": "curl example.com", "network": true});
        let (d1, a1) = gate.check("a", "r1", "shell", &args, "tc-1").expect("check1");
        assert!(!d1.allow);
        let approval_id = a1.expect("pending").approval_id;
        let receipt = core
            .submit(&teamagents_core::models::TeamAction {
                action_id: teamagents_core::models::new_id("user"),
                session_id: session.into(),
                actor_id: "user".into(),
                run_id: None,
                kind: ActionKind::ApprovalDecision,
                payload: json!({"approval_id": approval_id, "decision": "session"}),
            })
            .expect("submit");
        assert!(receipt.ok);
        // the approved run resumes: allowed, and the grant goes on record
        let (d2, _) = gate.check("a", "r1", "shell", &args, "tc-1b").expect("check2");
        assert!(d2.allow, "the approved run resumes at the same revision");
        // another run is authorized by the session cache at the same revision
        let (d3, _) = gate.check("a", "r2", "shell", &args, "tc-2").expect("check3");
        assert!(d3.allow, "session cache authorizes at the same revision");
        gate.set_mode("full_auto");
        gate.set_mode("approved_scope");
        let (d5, a5) = gate.check("a", "r3", "shell", &args, "tc-3").expect("check5");
        assert!(!d5.allow, "the stale session approval must not authorize after the revision moved");
        assert_eq!(a5.map(|a| a.status), Some(ApprovalStatus::Pending), "the op asks again");
    }

    #[test]
    fn policy_defaults_are_approved_scope() {
        let policy = PermissionPolicy::default();
        assert!(policy.evaluate("shell", &json!({"command": "ls"})).allow);
        assert!(!policy.evaluate("shell", &json!({"command": "curl x", "network": true})).allow);
        assert!(policy.evaluate("read_file", &json!({"path": "a"})).allow);
        assert!(!policy.evaluate("something_else", &json!({})).allow);
        let full_auto = PermissionPolicy { mode: "full_auto".into(), ..Default::default() };
        assert!(full_auto.evaluate("something_else", &json!({})).allow);
    }
}
