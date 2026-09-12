/** Shared wire types matching the Rust core (models.rs). */
export type TurnStatus =
  | "QUEUED" | "RUNNING" | "WAITING_TASK" | "WAITING_APPROVAL"
  | "COMPLETED" | "FAILED" | "CANCELLED" | "OUTCOME_UNKNOWN";

export type AgentStatusT = "IDLE" | "BUSY" | "WAITING" | "DRAINING" | "REMOVED";

export interface TurnRun {
  run_id: string;
  session_id: string;
  task_id: string | null;
  goal_id: string | null;
  agent_id: string;
  config_revision: number;
  topology_revision: number;
  status: TurnStatus;
  input_delivery_ids: number[];
  context_ref: string | null;
  external_turn_id: string | null;
  cancel_requested: boolean;
  waiting_on: string[];
  created_at: number;
  updated_at: number;
}

export interface TaskT {
  task_id: string;
  parent_task_id: string | null;
  goal_id: string | null;
  requester: string;
  assignee: string;
  description: string;
  acceptance: string;
  dependencies: string[];
  status: "PENDING" | "RUNNING" | "SUCCEEDED" | "FAILED" | "CANCELLED" | "BLOCKED";
  result_refs: string[];
  created_at: number;
  updated_at: number;
}

export interface ApprovalRequestT {
  approval_id: string;
  session_id: string;
  agent_id: string;
  run_id: string;
  tool_call_id: string;
  operation_hash: string;
  requested_scope: Record<string, unknown>;
  policy_revision: number;
  status: "PENDING" | "APPROVED_ONCE" | "APPROVED_SESSION" | "DENIED" | "EXPIRED";
  created_at: number;
  decided_at: number | null;
}

export interface SessionState {
  session: { session_id: string; status: string; cwd: string; permissions_mode: string; goal_id: string | null; goal_state: string } | null;
  revision: number;
  agents: { id: string; status: AgentStatusT | null }[];
  runs: TurnRun[];
  tasks: TaskT[];
  pending_approvals: ApprovalRequestT[];
  events: Record<string, any>[];
}

export interface WakeInfo {
  reason: "approval" | "user_input" | "task_results" | "new_input";
  payload: Record<string, any>;
}

export interface TurnOutcome {
  status: TurnStatus;
  error?: string;
  note?: string;
  reply_text?: string;
}

export interface AgentViewT {
  agent_id: string;
  assignment: TaskT[];
  inbox_delta: { event_id: string; kind: string; from: string; task_id: string | null; payload: any }[];
  permitted_shared_delta: Record<string, any>[];
  relevant_topology: Record<string, any>;
  capabilities: string[];
  delivery_ids: number[];
  batch_no: number;
}
