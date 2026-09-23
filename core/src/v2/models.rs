//! R2 v2 data contract (rebuild plan §4.1): one authoritative representation
//! per fact class. Identity, ownership, revision and dedup constraints are
//! not optional; table names may change, these fields may not.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub const V2_FORMAT_ID: &str = "teamagents-v2";
pub const V2_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lifecycle {
    Active,
    Paused,
    Parked,
    Terminated,
}

impl Lifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Lifecycle::Active => "ACTIVE",
            Lifecycle::Paused => "PAUSED",
            Lifecycle::Parked => "PARKED",
            Lifecycle::Terminated => "TERMINATED",
        }
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "ACTIVE" => Ok(Lifecycle::Active),
            "PAUSED" => Ok(Lifecycle::Paused),
            "PARKED" => Ok(Lifecycle::Parked),
            "TERMINATED" => Ok(Lifecycle::Terminated),
            other => Err(format!("unknown lifecycle {other}")),
        }
    }
}

/// Execution position (§3): persisted per instance; runnable state is the
/// separate lifecycle field — one status never covers both facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Ready,
    ModelPending,
    ToolsPending,
    Waiting,
    CompletionPending,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Ready => "READY",
            Phase::ModelPending => "MODEL_PENDING",
            Phase::ToolsPending => "TOOLS_PENDING",
            Phase::Waiting => "WAITING",
            Phase::CompletionPending => "COMPLETION_PENDING",
        }
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "READY" => Ok(Phase::Ready),
            "MODEL_PENDING" => Ok(Phase::ModelPending),
            "TOOLS_PENDING" => Ok(Phase::ToolsPending),
            "WAITING" => Ok(Phase::Waiting),
            "COMPLETION_PENDING" => Ok(Phase::CompletionPending),
            other => Err(format!("unknown phase {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub session_id: String,
    pub profile_revision: i64,
    pub workspace_ref: String,
    pub context_epoch: i64,
    pub lifecycle: Lifecycle,
    pub phase: Phase,
    pub revision: i64,
    pub active_goal_id: Option<String>,
    pub active_request_id: Option<String>,
    pub context_head: i64,
    /// Kernel profile snapshot for this revision (instructions/tools/options).
    pub profile: Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    Active,
    Completed,
    Failed,
    Cancelled,
    Parked,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GoalStatus::Active => "ACTIVE",
            GoalStatus::Completed => "COMPLETED",
            GoalStatus::Failed => "FAILED",
            GoalStatus::Cancelled => "CANCELLED",
            GoalStatus::Parked => "PARKED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Goal {
    pub id: String,
    pub session_id: String,
    pub original_request_ref: String,
    pub requirement_revision: i64,
    pub status: GoalStatus,
    pub deadline: Option<f64>,
    pub limits: Json,
    /// Settled provider usage across every instance/attempt/retry (§8).
    pub known_usage: crate::kernel::Usage,
    /// Outstanding reservations: parallel holds never exceed what is left.
    pub reservations: Json,
    /// Count of attempts whose usage the provider never reported (§8).
    pub unknown_usage: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Pending,
    Running,
    Blocked,
    Succeeded,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "PENDING",
            TaskStatus::Running => "RUNNING",
            TaskStatus::Blocked => "BLOCKED",
            TaskStatus::Succeeded => "SUCCEEDED",
            TaskStatus::Failed => "FAILED",
            TaskStatus::Cancelled => "CANCELLED",
        }
    }
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub goal_id: String,
    pub session_id: String,
    pub requester: String,
    pub assignee: String,
    pub dependencies: Vec<String>,
    pub acceptance_refs: Vec<String>,
    pub status: TaskStatus,
    pub result_refs: Vec<String>,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    pub session_id: String,
    pub issuer: String,
    pub subject: String,
    pub action: String,
    pub resource_scope: String,
    pub parent_grant_id: Option<String>,
    pub revision: i64,
    pub revoked_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    pub session_id: String,
    pub sender: String,
    pub recipient: String,
    pub epoch: i64,
    pub kind: String,
    pub correlation_id: Option<String>,
    pub payload: Option<Json>,
    pub payload_ref: Option<String>,
    pub sequence: i64,
    /// ACCEPTED → APPLIED (or CANCELLED before application).
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRequestRecord {
    pub request_id: String,
    pub instance_id: String,
    pub epoch: i64,
    pub goal_id: Option<String>,
    /// Artifact reference of the fixed request body (§4.3).
    pub request_ref: String,
    pub selected_attempt_id: Option<String>,
    /// PENDING | COMPLETE | FAILED
    pub status: String,
    pub est_prompt_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub attempt_id: String,
    pub request_id: String,
    /// RUNNING | COMPLETE | TRANSIENT | PERMANENT | CONTEXT_OVERFLOW | INTERRUPTED
    pub status: String,
    pub response_ref: Option<String>,
    pub usage: Option<crate::kernel::Usage>,
    pub elapsed_ms: i64,
}

/// Terminal operation state is unique (§6.1); OUTCOME_UNKNOWN is honest, not
/// a guess. A cancelled-before-start operation permanently rejects late GO.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    Prepared,
    DispatchCommitted,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    CancelledBeforeStart,
    OutcomeUnknown,
}

impl OperationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            OperationStatus::Prepared => "PREPARED",
            OperationStatus::DispatchCommitted => "DISPATCH_COMMITTED",
            OperationStatus::Running => "RUNNING",
            OperationStatus::Succeeded => "SUCCEEDED",
            OperationStatus::Failed => "FAILED",
            OperationStatus::Cancelled => "CANCELLED",
            OperationStatus::CancelledBeforeStart => "CANCELLED_BEFORE_START",
            OperationStatus::OutcomeUnknown => "OUTCOME_UNKNOWN",
        }
    }
    pub fn is_terminal(self) -> bool {
        !matches!(self, OperationStatus::Prepared | OperationStatus::DispatchCommitted | OperationStatus::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub operation_id: String,
    pub decision_id: String,
    pub tool_index: i64,
    pub goal_id: Option<String>,
    pub epoch: i64,
    pub args_hash: String,
    pub grant_revision: i64,
    pub status: OperationStatus,
    pub receipt: Option<Json>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactState {
    Staging,
    Live,
    Deleting,
    Abandoned,
}

impl ArtifactState {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactState::Staging => "STAGING",
            ArtifactState::Live => "LIVE",
            ArtifactState::Deleting => "DELETING",
            ArtifactState::Abandoned => "ABANDONED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub session_id: String,
    pub digest: String,
    pub size: i64,
    pub kind: String,
    pub owner_scope: String,
    pub storage_ref: String,
    pub completeness: ArtifactState,
    /// Publishing owner (request/job) protecting STAGING from GC (§4.3).
    pub owner_ref: Option<String>,
}
