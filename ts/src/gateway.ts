/**
 * Tool permissions and the single tool-call path for members,
 * ported from permissions.py + agents.py::ToolGateway.
 */
import { createHash } from "node:crypto";
import type { CoreClient, Receipt, TeamAction } from "./core-client.ts";

export interface Decision {
  allow: boolean;
  scope?: Record<string, unknown>;
  reason?: string;
}

/** Approval is bound to the operation and its parameters (§12.2). */
export function operationHash(toolName: string, args: Record<string, unknown>): string {
  const blob = JSON.stringify({ tool: toolName, args }, Object.keys({ args, tool: "" }).sort());
  // JSON.stringify with a replacer array drops nested keys; do canonical manually:
  return createHash("sha256").update(canonical({ tool: toolName, args })).digest("hex").slice(0, 32);
  function canonical(v: any): string {
    if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
    if (v && typeof v === "object")
      return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`).join(",")}}`;
    return JSON.stringify(v);
  }
  void blob;
}

export class PermissionPolicy {
  mode: "approved_scope" | "full_auto" = "approved_scope";
  preAuthorized: Set<string>;
  requireApproval: Set<string>;

  constructor(opts: { preAuthorized?: Set<string>; requireApproval?: Set<string> } = {}) {
    this.preAuthorized = opts.preAuthorized ?? new Set(["files", "shell"]);
    this.requireApproval = opts.requireApproval ?? new Set();
  }

  evaluate(toolName: string, args: Record<string, unknown>): Decision {
    if (this.mode === "full_auto") return { allow: true };
    if (toolName === "shell" && args.network)
      return { allow: false, scope: { tool: toolName, args, reason: "shell network access is off by default" },
               reason: "shell network access requires approval" };
    if (this.requireApproval.has(toolName))
      return { allow: false, scope: { tool: toolName, args, reason: `${toolName} needs approval` },
               reason: "outside pre-authorized scope" };
    if (this.preAuthorized.has(toolName) || PermissionPolicy.boundTool(toolName)) return { allow: true };
    if (toolName.startsWith("mcp_") || toolName.startsWith("web_")) return { allow: true };
    return { allow: false, scope: { tool: toolName, args, reason: `${toolName} is not pre-authorized` },
             reason: "tool not in approved scope" };
  }

  /** Runtime-bound execution tools (file tools stay inside the sandboxed backend). */
  private static boundTool(toolName: string): boolean {
    return new Set(["ls", "read_file", "write_file", "edit_file", "delete", "glob", "grep", "read_artifact"]).has(toolName);
  }
}

export class ApprovalGate {
  policyRevision = 1;
  private core: CoreClient;
  private sessionId: string;
  policy: PermissionPolicy;
  constructor(core: CoreClient, sessionId: string, policy: PermissionPolicy) {
    this.core = core;
    this.sessionId = sessionId;
    this.policy = policy;
  }

  setMode(mode: "approved_scope" | "full_auto") {
    this.policy.mode = mode;
    this.policyRevision += 1;
  }

  /** Returns [decision, approval]. An open request means: pause. */
  async check(
    agentId: string,
    runId: string,
    toolName: string,
    args: Record<string, unknown>,
    toolCallId: string,
  ): Promise<[Decision, any | null]> {
    const decision = this.policy.evaluate(toolName, args);
    if (decision.allow) return [decision, null];
    const opHash = operationHash(toolName, args);
    const cached = await this.core.call("approval_find_session", { session_id: this.sessionId, operation_hash: opHash });
    if (cached.scope != null) return [{ allow: true }, null];
    const { approval: existing } = await this.core.call("approval_for_call", {
      run_id: runId,
      tool_call_id: toolCallId,
      operation_hash: opHash,
    });
    if (existing) {
      if (existing.status === "PENDING") return [decision, existing];
      if (existing.status === "DENIED") return [{ allow: false, reason: "denied by the user" }, existing];
      if (existing.policy_revision === this.policyRevision) return [{ allow: true }, existing];
    }
    const req = {
      approval_id: `appr_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`,
      session_id: this.sessionId,
      agent_id: agentId,
      run_id: runId,
      tool_call_id: toolCallId,
      operation_hash: opHash,
      requested_scope: decision.scope ?? { tool: toolName, args },
      policy_revision: this.policyRevision,
      status: "PENDING",
      created_at: Date.now() / 1000,
      decided_at: null,
    };
    await this.core.call("insert_approval", { session_id: this.sessionId, approval: req });
    return [decision, req];
  }
}

const TEAM_TOOLS: Record<string, string> = {
  send_message: "send_message",
  assign_task: "assign_task",
  complete_task: "complete_task",
  wait_for_tasks: "wait_for_tasks",
  publish_shared: "publish_shared",
  read_shared: "read_shared",
  list_shared: "list_shared",
  request_help: "request_help",
  propose_team_change: "propose_team_change",
  apply_topology_patch: "apply_topology_patch",
  signal_done: "signal_done",
  cancel_task: "cancel_task",
  cancel_run: "cancel_run",
};

/**
 * The single path for a member's tool calls, team actions and approvals.
 * Identity is injected here, never trusted from model fields (§5.2).
 */
export class ToolGateway {
  pendingApprovalId: string | null = null;
  private core: CoreClient;
  private sessionId: string;
  agentId: string;
  runId: string;
  private approvals: ApprovalGate;
  private executor?: (tool: string, args: Record<string, unknown>) => Promise<unknown>;
  constructor(
    core: CoreClient,
    sessionId: string,
    agentId: string,
    runId: string,
    approvals: ApprovalGate,
    executor?: (tool: string, args: Record<string, unknown>) => Promise<unknown>,
  ) {
    this.core = core;
    this.sessionId = sessionId;
    this.agentId = agentId;
    this.runId = runId;
    this.approvals = approvals;
    this.executor = executor;
  }

  async call(toolName: string, args: Record<string, unknown>, toolCallId: string): Promise<Receipt> {
    const callId = `${this.runId}:${toolCallId}`;
    const teamKind = TEAM_TOOLS[toolName];
    if (teamKind) {
      const action: TeamAction = {
        action_id: callId,
        session_id: this.sessionId,
        actor_id: this.agentId,
        run_id: this.runId,
        kind: teamKind,
        payload: args,
      };
      return this.core.submit(action);
    }
    const [decision, approval] = await this.approvals.check(this.agentId, this.runId, toolName, args, callId);
    if (approval && !decision.allow && approval.status === "PENDING") {
      this.pendingApprovalId = approval.approval_id;
      return { action_id: callId, ok: false, kind: "complete_task",
               error: "approval_required",
               result: { approval_id: approval.approval_id, scope: approval.requested_scope } };
    }
    if (!decision.allow)
      return { action_id: callId, ok: false, kind: "complete_task", error: decision.reason ?? "operation not permitted", result: {} };
    if (!this.executor)
      return { action_id: callId, ok: false, kind: "complete_task", error: `no executor for tool ${toolName}`, result: {} };
    try {
      const output = await this.executor(toolName, args);
      return { action_id: callId, ok: true, kind: "complete_task", result: { output } };
    } catch (e) {
      return { action_id: callId, ok: false, kind: "complete_task", error: `${(e as Error).name}: ${(e as Error).message}`, result: {} };
    }
  }
}
