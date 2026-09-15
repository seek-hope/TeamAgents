//! Core data types (DP-1: spec as data).
//! Serde defaults and enum strings are the stable wire format.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const SCHEMA_VERSION: i64 = 1;

pub fn new_id(prefix: &str) -> String {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &hex[..16])
}

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident, $case:literal, $($v:ident),+) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = $case)]
        pub enum $name { $($v),+ }
    };
}

str_enum!(RuntimeKind, "snake_case", Deepagents, Codex);
str_enum!(ChannelMode, "snake_case", Message, Task, Broadcast);
str_enum!(WorkspacePolicy, "snake_case", Shared, Isolated, GitWorktree);
str_enum!(TaskStatus, "SCREAMING_SNAKE_CASE", Pending, Running, Succeeded, Failed, Cancelled, Blocked);
str_enum!(TurnStatus, "SCREAMING_SNAKE_CASE", Queued, Running, WaitingTask, WaitingApproval,
          Completed, Failed, Cancelled, OutcomeUnknown);
str_enum!(AgentStatus, "SCREAMING_SNAKE_CASE", Idle, Busy, Waiting, Draining, Removed);
str_enum!(PatchStatus, "SCREAMING_SNAKE_CASE", Proposed, Accepted, WaitingBoundary, Applied, Rejected, Failed);
str_enum!(ApprovalStatus, "SCREAMING_SNAKE_CASE", Pending, ApprovedOnce, ApprovedSession, Denied, Expired);
str_enum!(PermissionMode, "snake_case", ApprovedScope, FullAuto);
str_enum!(SessionStatus, "SCREAMING_SNAKE_CASE", Active, Paused, Idle, Closed);
str_enum!(ActionKind, "snake_case", SendMessage, AssignTask, CompleteTask, WaitForTasks,
          PublishShared, ReadShared, ListShared, RequestHelp, ProposeTeamChange,
          ApplyTopologyPatch, SignalDone, UserMessage, UserSupplement, CancelTask,
          CancelRun, ApprovalDecision, SetPermissionMode, PauseSession, MemberCompletionRequest);
str_enum!(EventKind, "snake_case", UserMessage, LeaderReply, Message, TaskCreated, TaskReady,
          TaskStarted, TaskCompleted, TaskFailed, TaskCancelled, TaskBlocked, RunStarted,
          RunCompleted, RunFailed, RunCancelled, RunWaiting, RunProgress, SharedPublished,
          TopologyProposed, TopologyApplied, TopologyRejected, ApprovalRequested,
          ApprovalDecided, GoalDone, LimitReached, MemberAdded, MemberRemoved, MemberStatus,
          SessionStatus);

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

impl TurnStatus {
    pub fn is_active(self) -> bool { matches!(self, Self::Queued | Self::Running) }
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled | Self::OutcomeUnknown)
    }
}

pub type Json = serde_json::Value;
pub fn obj() -> serde_json::Map<String, Json> { serde_json::Map::new() }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub role: String,
    pub runtime_kind: RuntimeKind,
    #[serde(default)]
    pub instructions: String,
    pub model_profile: String,
    #[serde(default)]
    pub tool_bindings: Vec<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "default_workspace_policy")]
    pub workspace_policy: WorkspacePolicy,
}
fn default_workspace_policy() -> WorkspacePolicy { WorkspacePolicy::Shared }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSpec {
    pub source: String,
    pub targets: Vec<String>,
    pub mode: ChannelMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObserverSpec {
    pub agent_id: String,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(default)]
    pub event_types: Vec<String>,
    #[serde(default = "default_payload_scope")]
    pub payload_scope: String, // "status" | "public_message" | "result"
    #[serde(default = "default_wake_policy")]
    pub wake_policy: String, // "none" | "on_event"
    #[serde(default)]
    pub capabilities: Vec<String>,
}
fn default_payload_scope() -> String { "status".into() }
fn default_wake_policy() -> String { "none".into() }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSpaceSpec {
    pub id: String,
    #[serde(default)]
    pub readers: Vec<String>,
    #[serde(default)]
    pub writers: Vec<String>,
}

fn default_max_parallel_workers() -> i64 { 8 }
fn default_max_members() -> i64 { 20 }
fn default_max_turns() -> i64 { 1000 }
fn default_max_steps() -> i64 { 200 }
fn default_turn_timeout() -> i64 { 1200 }
fn default_cancel_timeout() -> i64 { 60 }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_max_parallel_workers")]
    pub max_parallel_workers: i64,
    #[serde(default = "default_max_members")]
    pub max_members: i64,
    #[serde(default = "default_max_turns")]
    pub max_turns_per_goal: i64,
    #[serde(default = "default_max_steps")]
    pub max_model_steps_per_turn: i64,
    #[serde(default = "default_turn_timeout")]
    pub turn_active_timeout_s: i64,
    #[serde(default = "default_cancel_timeout")]
    pub cancel_confirm_timeout_s: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_parallel_workers: 8,
            max_members: 20,
            max_turns_per_goal: 1000,
            max_model_steps_per_turn: 200,
            turn_active_timeout_s: 1200,
            cancel_confirm_timeout_s: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSpec {
    #[serde(default = "default_schema_version")]
    pub schema_version: i64,
    pub leader_id: String,
    pub agents: Vec<AgentSpec>,
    #[serde(default)]
    pub channels: Vec<ChannelSpec>,
    #[serde(default)]
    pub observers: Vec<ObserverSpec>,
    #[serde(default)]
    pub shared_spaces: Vec<SharedSpaceSpec>,
    #[serde(default)]
    pub limits: Limits,
}
fn default_schema_version() -> i64 { SCHEMA_VERSION }

impl TeamSpec {
    /// Reference validation (plan §5.2).
    pub fn validate(&self) -> Result<(), String> {
        let ids: Vec<&str> = self.agents.iter().map(|a| a.id.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        if ids.iter().any(|id| !seen.insert(id)) {
            return Err("duplicate agent ids in team spec".into());
        }
        for a in &self.agents {
            if a.id.is_empty() || !a.id.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                return Err(format!("invalid agent id {:?}: use letters, digits, '-', '_'", a.id));
            }
        }
        if !ids.contains(&self.leader_id.as_str()) {
            return Err(format!("leader_id {:?} is not a member", self.leader_id));
        }
        let leaders: Vec<_> = self.agents.iter().filter(|a| a.id == self.leader_id && a.role == "leader").collect();
        if leaders.len() != 1 {
            return Err("exactly one member with role 'leader' must exist".into());
        }
        if self.agents.len() as i64 > self.limits.max_members {
            return Err(format!("team has {} members, limit is {}", self.agents.len(), self.limits.max_members));
        }
        for (k, v) in [
            ("max_parallel_workers", self.limits.max_parallel_workers),
            ("max_members", self.limits.max_members),
            ("max_turns_per_goal", self.limits.max_turns_per_goal),
            ("max_model_steps_per_turn", self.limits.max_model_steps_per_turn),
            ("turn_active_timeout_s", self.limits.turn_active_timeout_s),
            ("cancel_confirm_timeout_s", self.limits.cancel_confirm_timeout_s),
        ] {
            if v <= 0 {
                return Err(format!("limits.{k} must be a positive integer"));
            }
        }
        let known: Vec<&str> = ids.clone();
        for ch in &self.channels {
            if !known.contains(&ch.source.as_str()) {
                return Err(format!("channel source {:?} is not a member", ch.source));
            }
            for t in &ch.targets {
                if !known.contains(&t.as_str()) {
                    return Err(format!("channel target {:?} is not a member", t));
                }
            }
        }
        for ob in &self.observers {
            if !known.contains(&ob.agent_id.as_str()) {
                return Err(format!("observer {:?} is not a member", ob.agent_id));
            }
            for s in &ob.subjects {
                if !known.contains(&s.as_str()) {
                    return Err(format!("observer subject {:?} is not a member", s));
                }
            }
            if !matches!(ob.payload_scope.as_str(), "status" | "public_message" | "result") {
                return Err(format!("observer {:?} has unknown payload_scope {:?}", ob.agent_id, ob.payload_scope));
            }
            if !matches!(ob.wake_policy.as_str(), "none" | "on_event") {
                return Err(format!("observer {:?} has unknown wake_policy {:?}", ob.agent_id, ob.wake_policy));
            }
        }
        let mut space_ids = std::collections::HashSet::new();
        for sp in &self.shared_spaces {
            if !space_ids.insert(sp.id.as_str()) {
                return Err("duplicate shared space ids".into());
            }
            for who in sp.readers.iter().chain(sp.writers.iter()) {
                if !known.contains(&who.as_str()) {
                    return Err(format!("shared space {:?} references unknown member {:?}", sp.id, who));
                }
            }
        }
        Ok(())
    }

    pub fn agent(&self, id: &str) -> Option<&AgentSpec> { self.agents.iter().find(|a| a.id == id) }

    /// Channel with mode message/broadcast.
    pub fn can_send(&self, source: &str, target: &str) -> bool {
        self.channels.iter().any(|c| {
            c.source == source
                && match c.mode {
                    ChannelMode::Task => false,
                    ChannelMode::Broadcast => true,
                    ChannelMode::Message => c.targets.iter().any(|t| t == target),
                }
        })
    }

    /// Task channel, or the leader.
    pub fn can_delegate(&self, source: &str, target: &str) -> bool {
        if source == self.leader_id {
            return true;
        }
        self.channels.iter().any(|c| {
            c.source == source && c.mode == ChannelMode::Task && c.targets.iter().any(|t| t == target)
        })
    }

    pub fn space(&self, space_id: &str) -> Option<&SharedSpaceSpec> {
        self.shared_spaces.iter().find(|s| s.id == space_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub provider: String,
    #[serde(default = "default_protocol")]
    pub protocol: String, // "openai" (chat completions) | "responses" | "anthropic" | "deepseek"
    pub model: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout: i64,
    #[serde(default = "default_retries")]
    pub max_retries: i64,
    #[serde(default)]
    pub generation_options: HashMap<String, Json>,
    /// Model context window in tokens (drives the /status remaining-context
    /// column; None = unknown, shown as "not configured").
    #[serde(default)]
    pub context_window: Option<u64>,
    /// Codex members only: layer `$CODEX_HOME/<name>.config.toml` by running
    /// `codex --profile <name> app-server`. The Codex profile then owns the
    /// provider, model and credentials (e.g. a `deepseek` profile instead of the
    /// official subscription).
    #[serde(default)]
    pub codex_profile: Option<String>,
}
fn default_protocol() -> String { "openai".into() }
fn default_timeout() -> i64 { 120 }
fn default_retries() -> i64 { 5 }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ToolBinding {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub mcp_server: Option<String>,
    #[serde(default)]
    pub mcp_transport: Option<String>,
    /// Local MCP execution boundary: workspace (default) or explicit host.
    #[serde(default)]
    pub mcp_execution: Option<String>,
    /// Network access for workspace-sandboxed MCP processes.
    #[serde(default)]
    pub mcp_network: bool,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// Bearer token for the http transport: names the environment variable the
    /// secret is read from — the token itself never lands in this file.
    #[serde(default)]
    pub bearer_token_env_var: Option<String>,
    /// initialize/tools/list timeout in seconds (default 60).
    #[serde(default)]
    pub startup_timeout_s: Option<u64>,
    /// tools/call timeout in seconds (default 120).
    #[serde(default)]
    pub tool_timeout_s: Option<u64>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub tool_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default)]
    pub models: HashMap<String, ModelProfile>,
    #[serde(default)]
    pub tools: HashMap<String, ToolBinding>,
    #[serde(default)]
    pub skills_paths: Vec<String>,
    #[serde(default)]
    pub instruction_files: Vec<String>,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub hooks: Hooks,
}

/// Engine event hooks: a user-authored command, never a model-chosen one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hooks {
    /// argv of the command to run (event name is appended as the last argument,
    /// the event JSON arrives on stdin). Empty = no hooks.
    #[serde(default)]
    pub notify: Vec<String>,
}

/// Session housekeeping policy. Nothing is deleted unless a [retention] block
/// asks for it: archiving is the user's own "done with this" marker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Retention {
    /// Delete archived sessions untouched for this many days when a session is
    /// opened (also available as `teamagents sessions prune`). 0 disables it.
    #[serde(default)]
    pub archived_days: u64,
}

// -- runtime objects ---------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub task_id: String,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    #[serde(default)]
    pub goal_id: Option<String>,
    pub requester: String,
    pub assignee: String,
    pub description: String,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default = "default_task_status")]
    pub status: TaskStatus,
    #[serde(default)]
    pub result_refs: Vec<String>,
    #[serde(default = "now")]
    pub created_at: f64,
    #[serde(default = "now")]
    pub updated_at: f64,
}
fn default_task_status() -> TaskStatus { TaskStatus::Pending }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRun {
    pub run_id: String,
    pub session_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub goal_id: Option<String>,
    pub agent_id: String,
    pub config_revision: i64,
    pub topology_revision: i64,
    #[serde(default = "default_turn_status")]
    pub status: TurnStatus,
    #[serde(default)]
    pub input_delivery_ids: Vec<i64>,
    #[serde(default)]
    pub context_ref: Option<String>,
    #[serde(default)]
    pub external_turn_id: Option<String>,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub waiting_on: Vec<String>,
    #[serde(default = "now")]
    pub created_at: f64,
    #[serde(default = "now")]
    pub updated_at: f64,
}
fn default_turn_status() -> TurnStatus { TurnStatus::Queued }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamAction {
    pub action_id: String,
    pub session_id: String,
    pub actor_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    pub kind: ActionKind,
    #[serde(default)]
    pub payload: Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamEvent {
    pub event_id: String,
    pub session_id: String,
    #[serde(default)]
    pub sequence: i64,
    pub actor_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub kind: EventKind,
    #[serde(default)]
    pub payload: Json,
    #[serde(default)]
    pub audience: Vec<String>,
    #[serde(default)]
    pub topology_revision: i64,
    #[serde(default)]
    pub causation_id: Option<String>,
    #[serde(default = "now")]
    pub created_at: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TopologyPatch {
    pub patch_id: String,
    pub base_revision: i64,
    pub proposer: String,
    #[serde(default)]
    pub decided_by: Option<String>,
    pub operations: Vec<Json>,
    #[serde(default)]
    pub affected_agents: Vec<String>,
    #[serde(default = "default_patch_status")]
    pub status: PatchStatus,
    #[serde(default = "now")]
    pub created_at: f64,
    #[serde(default = "now")]
    pub updated_at: f64,
}
fn default_patch_status() -> PatchStatus { PatchStatus::Proposed }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub operation_hash: String,
    pub requested_scope: Json,
    pub policy_revision: i64,
    #[serde(default = "default_approval_status")]
    pub status: ApprovalStatus,
    #[serde(default = "now")]
    pub created_at: f64,
    #[serde(default)]
    pub decided_at: Option<f64>,
}
fn default_approval_status() -> ApprovalStatus { ApprovalStatus::Pending }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedEntry {
    pub entry_id: String,
    pub space_id: String,
    pub author: String,
    #[serde(default = "default_entry_kind")]
    pub kind: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub sequence: i64,
    #[serde(default = "now")]
    pub created_at: f64,
}
fn default_entry_kind() -> String { "note".into() }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub action_id: String,
    pub ok: bool,
    pub kind: ActionKind,
    #[serde(default)]
    pub result: Json,
    #[serde(default)]
    pub error: Option<String>,
}

impl Receipt {
    pub fn failure(action: &TeamAction, error: impl Into<String>) -> Self {
        Self {
            action_id: action.action_id.clone(),
            ok: false,
            kind: action.kind,
            result: Json::Object(obj()),
            error: Some(error.into()),
        }
    }
    pub fn success(action: &TeamAction, result: Json) -> Self {
        Self {
            action_id: action.action_id.clone(),
            ok: true,
            kind: action.kind,
            result,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json() -> Json {
        serde_json::json!({
            "leader_id": "lead",
            "agents": [
                {"id": "lead", "name": "Lead", "role": "leader",
                 "runtime_kind": "deepagents", "model_profile": "default"},
                {"id": "w1", "name": "W1", "role": "worker",
                 "runtime_kind": "codex", "model_profile": "default"}
            ],
            "channels": [{"source": "lead", "targets": ["w1"], "mode": "message"}]
        })
    }

    #[test]
    fn spec_roundtrip_and_refs() {
        let spec: TeamSpec = serde_json::from_value(spec_json()).unwrap();
        spec.validate().unwrap();
        assert!(spec.can_send("lead", "w1"));
        assert!(!spec.can_send("w1", "lead"));
        assert_eq!(spec.limits.max_members, 20);
    }

    #[test]
    fn spec_rejects_unknown_fields_and_bad_refs() {
        let mut bad = spec_json();
        bad["agents"][0]["bogus"] = Json::from(1);
        assert!(serde_json::from_value::<TeamSpec>(bad).is_err());

        let mut bad = spec_json();
        bad["leader_id"] = Json::from("ghost");
        let spec: TeamSpec = serde_json::from_value(bad).unwrap();
        assert!(spec.validate().is_err());

        let mut bad = spec_json();
        bad["channels"][0]["targets"] = serde_json::json!(["ghost"]);
        let spec: TeamSpec = serde_json::from_value(bad).unwrap();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn enum_wire_strings_are_stable() {
        assert_eq!(serde_json::to_string(&TaskStatus::Pending).unwrap(), "\"PENDING\"");
        assert_eq!(serde_json::to_string(&TurnStatus::WaitingApproval).unwrap(), "\"WAITING_APPROVAL\"");
        assert_eq!(serde_json::to_string(&ActionKind::MemberCompletionRequest).unwrap(), "\"member_completion_request\"");
        assert_eq!(serde_json::to_string(&WorkspacePolicy::GitWorktree).unwrap(), "\"git_worktree\"");
    }
}
