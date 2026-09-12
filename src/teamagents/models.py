"""Core data types: TeamSpec, tasks, turns, actions, events, approvals, patches.

Strict validation everywhere: unknown fields, unknown references and unsupported
enum values are rejected with readable errors (plan section 5.2).
"""

from __future__ import annotations

import time
import uuid
from enum import StrEnum
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

SCHEMA_VERSION = 1


def new_id(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex[:16]}"


def now() -> float:
    return time.time()


class RuntimeKind(StrEnum):
    DEEPAGENTS = "deepagents"
    CODEX = "codex"


class ChannelMode(StrEnum):
    MESSAGE = "message"
    TASK = "task"
    BROADCAST = "broadcast"


class WorkspacePolicy(StrEnum):
    SHARED = "shared"
    ISOLATED = "isolated"
    GIT_WORKTREE = "git_worktree"


class TaskStatus(StrEnum):
    PENDING = "PENDING"
    RUNNING = "RUNNING"
    SUCCEEDED = "SUCCEEDED"
    FAILED = "FAILED"
    CANCELLED = "CANCELLED"
    BLOCKED = "BLOCKED"


TERMINAL_TASK_STATUSES = {TaskStatus.SUCCEEDED, TaskStatus.FAILED, TaskStatus.CANCELLED}


class TurnStatus(StrEnum):
    QUEUED = "QUEUED"
    RUNNING = "RUNNING"
    WAITING_TASK = "WAITING_TASK"
    WAITING_APPROVAL = "WAITING_APPROVAL"
    COMPLETED = "COMPLETED"
    FAILED = "FAILED"
    CANCELLED = "CANCELLED"
    OUTCOME_UNKNOWN = "OUTCOME_UNKNOWN"


TURN_ACTIVE_STATUSES = {TurnStatus.QUEUED, TurnStatus.RUNNING}
TURN_WAITING_STATUSES = {TurnStatus.WAITING_TASK, TurnStatus.WAITING_APPROVAL}
TURN_TERMINAL_STATUSES = {
    TurnStatus.COMPLETED,
    TurnStatus.FAILED,
    TurnStatus.CANCELLED,
    TurnStatus.OUTCOME_UNKNOWN,
}


class AgentStatus(StrEnum):
    IDLE = "IDLE"
    BUSY = "BUSY"
    WAITING = "WAITING"
    DRAINING = "DRAINING"
    REMOVED = "REMOVED"


class PatchStatus(StrEnum):
    PROPOSED = "PROPOSED"
    ACCEPTED = "ACCEPTED"
    WAITING_BOUNDARY = "WAITING_BOUNDARY"
    APPLIED = "APPLIED"
    REJECTED = "REJECTED"
    FAILED = "FAILED"


class ApprovalStatus(StrEnum):
    PENDING = "PENDING"
    APPROVED_ONCE = "APPROVED_ONCE"
    APPROVED_SESSION = "APPROVED_SESSION"
    DENIED = "DENIED"
    EXPIRED = "EXPIRED"


class PermissionMode(StrEnum):
    APPROVED_SCOPE = "approved_scope"
    FULL_AUTO = "full_auto"


class SessionStatus(StrEnum):
    ACTIVE = "ACTIVE"
    PAUSED = "PAUSED"
    IDLE = "IDLE"
    CLOSED = "CLOSED"


class ActionKind(StrEnum):
    # member -> team actions (plan section 7)
    SEND_MESSAGE = "send_message"
    ASSIGN_TASK = "assign_task"
    COMPLETE_TASK = "complete_task"
    WAIT_FOR_TASKS = "wait_for_tasks"
    PUBLISH_SHARED = "publish_shared"
    READ_SHARED = "read_shared"
    LIST_SHARED = "list_shared"
    REQUEST_HELP = "request_help"
    PROPOSE_TEAM_CHANGE = "propose_team_change"
    APPLY_TOPOLOGY_PATCH = "apply_topology_patch"
    SIGNAL_DONE = "signal_done"
    # runtime-injected actions (user / system / approval)
    USER_MESSAGE = "user_message"
    USER_SUPPLEMENT = "user_supplement"
    CANCEL_TASK = "cancel_task"
    CANCEL_RUN = "cancel_run"
    APPROVAL_DECISION = "approval_decision"
    SET_PERMISSION_MODE = "set_permission_mode"
    PAUSE_SESSION = "pause_session"
    MEMBER_COMPLETION_REQUEST = "member_completion_request"


class EventKind(StrEnum):
    USER_MESSAGE = "user_message"
    LEADER_REPLY = "leader_reply"
    MESSAGE = "message"
    TASK_CREATED = "task_created"
    TASK_READY = "task_ready"
    TASK_STARTED = "task_started"
    TASK_COMPLETED = "task_completed"
    TASK_FAILED = "task_failed"
    TASK_CANCELLED = "task_cancelled"
    TASK_BLOCKED = "task_blocked"
    RUN_STARTED = "run_started"
    RUN_COMPLETED = "run_completed"
    RUN_FAILED = "run_failed"
    RUN_CANCELLED = "run_cancelled"
    RUN_WAITING = "run_waiting"
    RUN_PROGRESS = "run_progress"
    SHARED_PUBLISHED = "shared_published"
    TOPOLOGY_PROPOSED = "topology_proposed"
    TOPOLOGY_APPLIED = "topology_applied"
    TOPOLOGY_REJECTED = "topology_rejected"
    APPROVAL_REQUESTED = "approval_requested"
    APPROVAL_DECIDED = "approval_decided"
    GOAL_DONE = "goal_done"
    LIMIT_REACHED = "limit_reached"
    MEMBER_ADDED = "member_added"
    MEMBER_REMOVED = "member_removed"
    MEMBER_STATUS = "member_status"
    SESSION_STATUS = "session_status"


# ---------------------------------------------------------------------------
# Team spec
# ---------------------------------------------------------------------------


class AgentSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: str
    name: str
    role: str
    runtime_kind: RuntimeKind
    instructions: str = ""
    model_profile: str
    tool_bindings: list[str] = Field(default_factory=list)
    skills: list[str] = Field(default_factory=list)
    workspace_policy: WorkspacePolicy = WorkspacePolicy.SHARED

    @field_validator("id")
    @classmethod
    def _id_shape(cls, v: str) -> str:
        if not v or not all(c.isalnum() or c in "-_" for c in v):
            raise ValueError(f"invalid agent id {v!r}: use letters, digits, '-', '_'")
        return v


class ChannelSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    source: str
    targets: list[str]
    mode: ChannelMode


class ObserverSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    agent_id: str
    subjects: list[str] = Field(default_factory=list)
    event_types: list[str] = Field(default_factory=list)
    payload_scope: Literal["status", "public_message", "result"] = "status"
    wake_policy: Literal["none", "on_event"] = "none"
    capabilities: list[str] = Field(default_factory=list)


class SharedSpaceSpec(BaseModel):
    model_config = ConfigDict(extra="forbid")

    id: str
    readers: list[str] = Field(default_factory=list)
    writers: list[str] = Field(default_factory=list)


class Limits(BaseModel):
    model_config = ConfigDict(extra="forbid")

    max_parallel_workers: int = 8
    max_members: int = 20
    max_turns_per_goal: int = 1000
    max_model_steps_per_turn: int = 200
    turn_active_timeout_s: int = 1200
    cancel_confirm_timeout_s: int = 60

    @field_validator("*")
    @classmethod
    def _positive(cls, v: int) -> int:
        if v <= 0:
            raise ValueError("limits must be positive integers")
        return v


class TeamSpec(BaseModel):
    """Team structure as validated data (DP-1). Never generated code."""

    model_config = ConfigDict(extra="forbid")

    schema_version: int = SCHEMA_VERSION
    leader_id: str
    agents: list[AgentSpec]
    channels: list[ChannelSpec] = Field(default_factory=list)
    observers: list[ObserverSpec] = Field(default_factory=list)
    shared_spaces: list[SharedSpaceSpec] = Field(default_factory=list)
    limits: Limits = Field(default_factory=Limits)

    # -- reference validation (plan section 5.2) ----------------------------

    @model_validator(mode="after")
    def _validate_refs(self) -> "TeamSpec":
        ids = [a.id for a in self.agents]
        if len(ids) != len(set(ids)):
            raise ValueError("duplicate agent ids in team spec")
        if self.leader_id not in ids:
            raise ValueError(f"leader_id {self.leader_id!r} is not a member")
        leaders = [a for a in self.agents if a.id == self.leader_id]
        if len(leaders) != 1 or leaders[0].role != "leader":
            raise ValueError("exactly one member with role 'leader' must exist")
        if len(self.agents) > self.limits.max_members:
            raise ValueError(
                f"team has {len(self.agents)} members, limit is {self.limits.max_members}"
            )
        known = set(ids)
        for ch in self.channels:
            if ch.source not in known:
                raise ValueError(f"channel source {ch.source!r} is not a member")
            for t in ch.targets:
                if t not in known:
                    raise ValueError(f"channel target {t!r} is not a member")
        for ob in self.observers:
            if ob.agent_id not in known:
                raise ValueError(f"observer {ob.agent_id!r} is not a member")
            for s in ob.subjects:
                if s not in known:
                    raise ValueError(f"observer subject {s!r} is not a member")
        spaces = [s.id for s in self.shared_spaces]
        if len(spaces) != len(set(spaces)):
            raise ValueError("duplicate shared space ids")
        for sp in self.shared_spaces:
            for who in sp.readers + sp.writers:
                if who not in known:
                    raise ValueError(f"shared space {sp.id!r} references unknown member {who!r}")
        return self

    # -- lookups -------------------------------------------------------------

    def agent(self, agent_id: str) -> AgentSpec:
        for a in self.agents:
            if a.id == agent_id:
                return a
        raise ValueError(f"unknown member {agent_id!r}")

    @property
    def leader(self) -> AgentSpec:
        return self.agent(self.leader_id)

    @property
    def agent_ids(self) -> list[str]:
        return [a.id for a in self.agents]

    def can_send(self, source: str, target: str) -> bool:
        """Directed send rights: channel entry with mode message/broadcast."""
        for ch in self.channels:
            if ch.source != source:
                continue
            if ch.mode is ChannelMode.TASK:
                continue
            if ch.mode is ChannelMode.BROADCAST or target in ch.targets:
                return True
        return False

    def can_delegate(self, source: str, target: str) -> bool:
        """Task delegation rights: channel entry with mode task, or leader."""
        if source == self.leader_id:
            return True
        for ch in self.channels:
            if ch.source == source and ch.mode is ChannelMode.TASK and target in ch.targets:
                return True
        return False

    def space(self, space_id: str) -> SharedSpaceSpec:
        for s in self.shared_spaces:
            if s.id == space_id:
                return s
        raise ValueError(f"unknown shared space {space_id!r}")


class ModelProfile(BaseModel):
    """Logical model config name (DP-7). Only names live in TeamSpec."""

    model_config = ConfigDict(extra="forbid")

    provider: str
    protocol: Literal["openai", "anthropic", "deepseek"] = "openai"
    model: str
    base_url: str | None = None
    api_key_env: str | None = None
    timeout: int = 120
    max_retries: int = 5
    generation_options: dict[str, Any] = Field(default_factory=dict)


class ToolBinding(BaseModel):
    """User-configured tool binding a member may reference."""

    model_config = ConfigDict(extra="forbid")

    kind: Literal["files", "shell", "web_search", "web_fetch", "mcp", "custom"]
    required: bool = False
    provider: str | None = None          # e.g. "anysearch" for web_search bindings
    api_key_env: str | None = None
    mcp_server: str | None = None
    mcp_transport: Literal["stdio", "http", "sse"] | None = None
    command: str | None = None
    args: list[str] = Field(default_factory=list)
    url: str | None = None
    env: dict[str, str] = Field(default_factory=dict)
    tool_names: list[str] = Field(default_factory=list)


class UserConfig(BaseModel):
    """User-level configuration: model profiles and tool catalog (plan section 14)."""

    model_config = ConfigDict(extra="forbid")

    models: dict[str, ModelProfile] = Field(default_factory=dict)
    tools: dict[str, ToolBinding] = Field(default_factory=dict)
    skills_paths: list[str] = Field(default_factory=list)
    instruction_files: list[str] = Field(default_factory=list)


# ---------------------------------------------------------------------------
# Runtime objects
# ---------------------------------------------------------------------------


class Task(BaseModel):
    model_config = ConfigDict(extra="forbid")

    task_id: str
    parent_task_id: str | None = None
    goal_id: str | None = None
    requester: str
    assignee: str
    description: str
    acceptance: str = ""
    dependencies: list[str] = Field(default_factory=list)
    status: TaskStatus = TaskStatus.PENDING
    result_refs: list[str] = Field(default_factory=list)
    created_at: float = Field(default_factory=now)
    updated_at: float = Field(default_factory=now)


class TurnRun(BaseModel):
    model_config = ConfigDict(extra="forbid")

    run_id: str
    session_id: str
    task_id: str | None = None
    goal_id: str | None = None
    agent_id: str
    config_revision: int
    topology_revision: int
    status: TurnStatus = TurnStatus.QUEUED
    input_delivery_ids: list[int] = Field(default_factory=list)
    context_ref: str | None = None
    external_turn_id: str | None = None
    cancel_requested: bool = False
    waiting_on: list[str] = Field(default_factory=list)
    created_at: float = Field(default_factory=now)
    updated_at: float = Field(default_factory=now)


class TeamAction(BaseModel):
    model_config = ConfigDict(extra="forbid")

    action_id: str
    session_id: str
    actor_id: str
    run_id: str | None = None
    kind: ActionKind
    payload: dict[str, Any] = Field(default_factory=dict)


class TeamEvent(BaseModel):
    model_config = ConfigDict(extra="forbid")

    event_id: str
    session_id: str
    sequence: int = 0
    actor_id: str
    task_id: str | None = None
    kind: EventKind
    payload: dict[str, Any] = Field(default_factory=dict)
    audience: list[str] = Field(default_factory=list)
    topology_revision: int = 0
    causation_id: str | None = None
    created_at: float = Field(default_factory=now)


class TopologyPatch(BaseModel):
    model_config = ConfigDict(extra="forbid")

    patch_id: str
    base_revision: int
    proposer: str
    decided_by: str | None = None
    operations: list[dict[str, Any]]
    affected_agents: list[str] = Field(default_factory=list)
    status: PatchStatus = PatchStatus.PROPOSED
    created_at: float = Field(default_factory=now)
    updated_at: float = Field(default_factory=now)


class ApprovalRequest(BaseModel):
    model_config = ConfigDict(extra="forbid")

    approval_id: str
    session_id: str
    agent_id: str
    run_id: str
    tool_call_id: str
    operation_hash: str
    requested_scope: dict[str, Any]
    policy_revision: int
    status: ApprovalStatus = ApprovalStatus.PENDING
    created_at: float = Field(default_factory=now)
    decided_at: float | None = None


class SharedEntry(BaseModel):
    model_config = ConfigDict(extra="forbid")

    entry_id: str
    space_id: str
    author: str
    kind: str = "note"
    content: str = ""
    ref: str | None = None
    supersedes: str | None = None
    sequence: int = 0
    created_at: float = Field(default_factory=now)


class AgentView(BaseModel):
    """What one member may see at a delivery boundary (plan section 5.2)."""

    model_config = ConfigDict(extra="forbid")

    agent_id: str
    assignment: list[Task] = Field(default_factory=list)
    inbox_delta: list[dict[str, Any]] = Field(default_factory=list)
    permitted_shared_delta: list[SharedEntry] = Field(default_factory=list)
    relevant_topology: dict[str, Any] = Field(default_factory=dict)
    capabilities: list[str] = Field(default_factory=list)
    delivery_ids: list[int] = Field(default_factory=list)
    batch_no: int = 0


class Receipt(BaseModel):
    """Action receipt: success means the business transaction is committed."""

    model_config = ConfigDict(extra="forbid")

    action_id: str
    ok: bool
    kind: ActionKind
    result: dict[str, Any] = Field(default_factory=dict)
    error: str | None = None

    @classmethod
    def failure(cls, action: TeamAction, error: str) -> "Receipt":
        return cls(action_id=action.action_id, ok=False, kind=action.kind, error=error)
