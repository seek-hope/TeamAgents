/**
 * Deterministic scripted member, ported from agents.py::FakeMember.
 * Steps: ["call", tool, args] | ["barrier", name] | ["sleep", seconds]
 *        ["wait"] | ["end"] | ["fail", msg] | ["inbox"]
 */
import type { AgentRunner } from "./runtime.ts";
import type { ToolGateway } from "./gateway.ts";
import type { AgentViewT, TurnOutcome, TurnRun, TurnStatus, WakeInfo } from "./types.ts";

export type Step =
  | ["call", string, Record<string, unknown>]
  | ["barrier", string]
  | ["sleep", number]
  | ["wait"]
  | ["end"]
  | ["fail", string]
  | ["inbox"];

/** Templates in scripts: "$r0.result.task_id", "$inbox0.payload.task_id", "$run.task_id". */
function resolveRefs(item: any, ctx: Record<string, any>): any {
  if (typeof item === "string" && item.startsWith("$")) {
    const [head, ...rest] = item.slice(1).split(".");
    let value: any = ctx[head];
    for (const part of rest) {
      if (value == null) break;
      if (Array.isArray(value) && /^\d+$/.test(part)) value = value[Number(part)];
      else value = value[part];
    }
    return value;
  }
  if (Array.isArray(item)) return item.map((v) => resolveRefs(v, ctx));
  if (item && typeof item === "object")
    return Object.fromEntries(Object.entries(item).map(([k, v]) => [k, resolveRefs(v, ctx)]));
  return item;
}

export class Barrier {
  private waiters: (() => void)[] = [];
  private arrived = 0;
  private parties: number;
  constructor(parties: number) {
    this.parties = parties;
  }
  async wait(): Promise<void> {
    this.arrived++;
    if (this.arrived >= this.parties) {
      for (const w of this.waiters.splice(0)) w();
      return;
    }
    await new Promise<void>((resolve) => this.waiters.push(resolve));
  }
}

export class ScriptedMember implements AgentRunner {
  cursor = 0;
  results: any[] = [];
  observedInbox: any[] = [];
  state = new Map<string, TurnStatus>();
  private midTurn = new Map<string, any[]>();
  private cancelled = new Set<string>();
  lastView: AgentViewT | null = null;

  agentId: string;
  script: Step[];
  private barriers: Record<string, Barrier>;
  /** `barriers` must be SHARED across members to synchronize them (conftest parity). */
  constructor(agentId: string, script: Step[] = [], barriers: Record<string, Barrier> = {}) {
    this.agentId = agentId;
    this.script = script;
    this.barriers = barriers;
  }

  async startOrResume(run: TurnRun, view: AgentViewT, gateway: ToolGateway, wake: WakeInfo | null): Promise<TurnOutcome> {
    this.state.set(run.run_id, "RUNNING");
    this.lastView = view;
    const ctx: Record<string, any> = {
      run: { task_id: run.task_id, run_id: run.run_id, agent_id: run.agent_id, wake: wake?.reason ?? null },
    };
    view.inbox_delta.forEach((item, i) => (ctx[`inbox${i}`] = item));
    this.results.forEach((r, i) => (ctx[`r${i}`] = { ok: r.ok, error: r.error, result: r.result, kind: r.kind }));

    while (this.cursor < this.script.length) {
      const step = this.script[this.cursor];
      switch (step[0]) {
        case "call": {
          const [, tool, args] = step;
          const resolved = resolveRefs(structuredClone(args), ctx);
          const receipt = await gateway.call(tool, resolved, `step${this.cursor}`);
          this.results.push(receipt);
          ctx[`r${this.results.length - 1}`] = { ok: receipt.ok, error: receipt.error, result: receipt.result, kind: receipt.kind };
          this.cursor++;
          if (receipt.error === "approval_required") {
            this.state.set(run.run_id, "WAITING_APPROVAL");
            return { status: "WAITING_APPROVAL", note: String(receipt.result?.approval_id) };
          }
          if (!receipt.ok) continue; // tool error: member sees it and continues
          break;
        }
        case "barrier": {
          const b = (this.barriers[step[1]] ??= new Barrier(2));
          await this.awaitOrCancel(b.wait(), 15000, run.run_id);
          this.cursor++;
          break;
        }
        case "sleep": {
          await this.awaitOrCancel(new Promise((r) => setTimeout(r, step[1] * 1000)), step[1] * 1000 + 1000, run.run_id);
          this.cursor++;
          break;
        }
        case "inbox": {
          const items = [...view.inbox_delta, ...(this.midTurn.get(run.run_id) ?? [])];
          this.midTurn.delete(run.run_id);
          this.observedInbox.push(...items);
          this.results.push({ action_id: `inbox:${this.cursor}`, ok: true, kind: "send_message", result: { injected: items } });
          this.cursor++;
          break;
        }
        case "wait":
          this.cursor++;
          this.state.set(run.run_id, "WAITING_TASK");
          return { status: "WAITING_TASK" };
        case "end":
          this.cursor++;
          this.state.set(run.run_id, "COMPLETED");
          return { status: "COMPLETED" };
        case "fail":
          this.cursor++;
          this.state.set(run.run_id, "FAILED");
          return { status: "FAILED", error: String(step[1]) };
        default:
          this.cursor++;
      }
      if (this.cancelled.has(run.run_id)) {
        this.state.set(run.run_id, "CANCELLED");
        return { status: "CANCELLED", note: "interrupted" };
      }
    }
    this.state.set(run.run_id, "COMPLETED");
    return { status: "COMPLETED" };
  }

  private async awaitOrCancel(p: Promise<unknown>, timeoutMs: number, runId: string) {
    const deadline = Date.now() + timeoutMs;
    let done = false;
    p.then(() => (done = true)).catch(() => (done = true));
    while (!done) {
      if (this.cancelled.has(runId)) return;
      if (Date.now() > deadline) throw new Error(`step timed out after ${timeoutMs}ms`);
      await new Promise((r) => setTimeout(r, 20));
    }
  }

  async requestInterrupt(runId: string): Promise<TurnStatus> {
    this.cancelled.add(runId);
    this.state.set(runId, "CANCELLED");
    return "CANCELLED";
  }

  queryState(runId: string): TurnStatus | null {
    return this.state.get(runId) ?? null;
  }

  deliverMidTurn(runId: string, items: any[]) {
    this.midTurn.set(runId, [...(this.midTurn.get(runId) ?? []), ...items]);
  }

  remaining() {
    return Math.max(0, this.script.length - this.cursor);
  }
}
