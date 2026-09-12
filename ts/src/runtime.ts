/**
 * Session execution loop, ported from runtime.py (asyncio → node promises).
 * All authoritative state changes go through the Rust core over CoreClient.
 */
import type { CoreClient, TeamAction, Receipt } from "./core-client.ts";
import { ApprovalGate, PermissionPolicy, ToolGateway } from "./gateway.ts";
import type { AgentViewT, SessionState, TurnOutcome, TurnRun, TurnStatus, WakeInfo } from "./types.ts";

export interface AgentRunner {
  startOrResume(run: TurnRun, view: AgentViewT, gateway: ToolGateway, wake: WakeInfo | null): Promise<TurnOutcome>;
  requestInterrupt(runId: string): Promise<TurnStatus>;
  queryState(runId: string): TurnStatus | null;
  deliverMidTurn(runId: string, items: any[]): void;
  /** Optional restart convergence (RT-04). */
  reconcile?(run: TurnRun): Promise<TurnStatus | null>;
  /** Optional live-status hook (external backends). */
  statusHook?: (runId: string, status: TurnStatus) => void;
  progressHook?: (runId: string, text: string) => void;
  streamHook?: (runId: string, agentId: string, text: string) => void;
  resolveApproval?: (approvalId: string, decision: string) => void;
}

const TERMINAL: TurnStatus[] = ["COMPLETED", "FAILED", "CANCELLED", "OUTCOME_UNKNOWN"];

export class SessionRuntime {
  readonly approvals: ApprovalGate;
  private runners = new Map<string, AgentRunner>();
  private inflight = new Map<string, Promise<void>>();
  private cancelTasks = new Map<string, Promise<void>>();
  private cancelledRuns = new Set<string>();
  private offered = new Map<string, Set<number>>();
  private stepCounts = new Map<string, number>();
  private runnerRevisions = new Map<string, number>();
  private closed = false;
  private wake: (() => void) | null = null;
  private loopPromise: Promise<void> | null = null;
  streamSink: ((runId: string, agentId: string, text: string) => void) | null = null;
  private latestLimits: { turn_active_timeout_s?: number; max_model_steps_per_turn?: number } | null = null;

  private core: CoreClient;
  readonly sessionId: string;
  constructor(
    core: CoreClient,
    sessionId: string,
    opts: {
      runners?: Record<string, AgentRunner>;
      approvals?: ApprovalGate;
      toolExecutor?: (tool: string, args: Record<string, unknown>) => Promise<unknown>;
      runnerFactory?: (agentId: string) => AgentRunner;
      limits?: { turnActiveTimeoutS?: number; cancelConfirmTimeoutS?: number; maxParallelWorkers?: number; maxModelStepsPerTurn?: number };
    } = {},
  ) {
    this.core = core;
    this.sessionId = sessionId;
    this.approvals = opts.approvals ?? new ApprovalGate(core, sessionId, new PermissionPolicy());
    this.opts = opts;
    for (const [id, r] of Object.entries(opts.runners ?? {})) this.runners.set(id, r);
  }
  private opts: Exclude<ConstructorParameters<typeof SessionRuntime>[2], undefined>;

  addRunner(agentId: string, runner: AgentRunner) {
    this.runners.set(agentId, runner);
  }

  private signal() {
    this.wake?.();
  }

  private async sleepOrWake(ms: number) {
    await new Promise<void>((resolve) => {
      const t = setTimeout(resolve, ms);
      this.wake = () => {
        clearTimeout(t);
        resolve();
      };
    });
    this.wake = null;
  }

  // -- ingress ---------------------------------------------------------------

  async submit(action: TeamAction): Promise<Receipt> {
    const receipt = await this.core.submit(action);
    if (action.kind === "approval_decision" && receipt.ok) {
      await this.deliverApprovalDecision(String(action.payload?.approval_id), String(action.payload?.decision));
    }
    await this.drainMidTurn();
    this.signal();
    return receipt;
  }

  userMessage(text: string, opts: { actionId?: string; supplement?: boolean } = {}) {
    return this.submit({
      action_id: opts.actionId ?? `user_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`,
      session_id: this.sessionId,
      actor_id: "user",
      kind: opts.supplement ? "user_supplement" : "user_message",
      payload: { text },
    });
  }

  private async deliverApprovalDecision(approvalId: string, decision: string) {
    const { approval } = await this.core.call("get_approval", { session_id: this.sessionId, approval_id: approvalId });
    if (!approval) return;
    const state = await this.state();
    const run = state.runs.find((r) => r.run_id === approval.run_id);
    const runner = run && this.runners.get(run.agent_id);
    runner?.resolveApproval?.(approvalId, decision);
  }

  private async drainMidTurn() {
    const { pushes } = await this.core.call("drain_mid_turn", { session_id: this.sessionId });
    for (const { run_id, items } of pushes as { run_id: string; items: any[] }[]) {
      const state = await this.state();
      const run = state.runs.find((r) => r.run_id === run_id);
      const runner = run && this.runners.get(run.agent_id);
      if (runner && run) {
        runner.deliverMidTurn(run_id, items);
        const offered = this.offered.get(run_id) ?? new Set<number>();
        for (const id of run.input_delivery_ids) offered.add(id);
        this.offered.set(run_id, offered);
      }
    }
  }

  // -- lifecycle ---------------------------------------------------------------

  async start() {
    this.closed = false;
    await this.reconcile();
    this.loopPromise = this.loop();
  }

  async close() {
    this.closed = true;
    this.signal();
    await Promise.allSettled([...this.inflight.values()]);
    if (this.loopPromise) await this.loopPromise;
  }

  async state(): Promise<SessionState> {
    return this.core.call("state", { session_id: this.sessionId });
  }

  /** After a restart: re-check in-flight runs, never blind-retry side effects. */
  async reconcile() {
    const state = await this.state();
    const parked = state.runs.filter((r) => ["RUNNING", "WAITING_TASK", "WAITING_APPROVAL"].includes(r.status));
    for (const run of parked) {
      const runner = this.runners.get(run.agent_id);
      let st = runner?.queryState(run.run_id) ?? null;
      if (st == null && runner?.reconcile) st = await runner.reconcile(run);
      if (run.status !== "RUNNING") {
        if (st === run.status) continue;
        if (st == null || TERMINAL.includes(st)) {
          await this.converge(run, {
            status: st ?? "OUTCOME_UNKNOWN",
            error: st == null ? "suspended turn could not be restored after restart" : undefined,
          });
        } else {
          await this.finalize(run, { status: st, note: "pause restored" });
        }
        continue;
      }
      if (st != null) {
        if (TERMINAL.includes(st) || st === "WAITING_TASK" || st === "WAITING_APPROVAL") {
          await this.converge(run, { status: st });
        }
        // live RUNNING: nothing to do, the runner is still executing
        continue;
      }
      if (run.external_turn_id) {
        await this.converge(run, { status: "OUTCOME_UNKNOWN", error: "external turn outcome could not be confirmed" });
      } else {
        // in-process runner only: safe to re-run the segment
        await this.core.call("requeue_run", { session_id: this.sessionId, run_id: run.run_id });
      }
    }
    await this.core.call("schedule", { session_id: this.sessionId });
    this.signal();
  }

  /** Finalize a run found at restart; its input counts as injected (F-C3/RT-05). */
  private async converge(run: TurnRun, outcome: TurnOutcome) {
    this.offered.set(run.run_id, new Set(run.input_delivery_ids));
    await this.finalize(run, outcome);
  }

  // -- executor ---------------------------------------------------------------

  private async loop() {
    while (!this.closed) {
      await this.startReadyRuns();
      await this.sleepOrWake(50);
    }
  }

  private async startReadyRuns() {
    const state = await this.state();
    if (!state.session || state.session.status === "PAUSED") return;
    const leaderId = (state as any).leader_id ?? "";
    const specLimits = (state as any).limits ?? {};
    const limitWorkers = specLimits.max_parallel_workers ?? this.opts.limits?.maxParallelWorkers ?? 8;
    let workersBusy = 0;
    for (const runId of this.inflight.keys()) {
      const run = state.runs.find((r) => r.run_id === runId);
      if (run && run.agent_id !== leaderId && ["QUEUED", "RUNNING"].includes(run.status)) workersBusy++;
    }
    for (const run of state.runs.filter((r) => ["QUEUED", "RUNNING"].includes(r.status))) {
      if (this.inflight.has(run.run_id)) continue;
      let runner = this.runners.get(run.agent_id);
      if (!runner && this.opts.runnerFactory) {
        try {
          runner = this.opts.runnerFactory(run.agent_id);
          this.runners.set(run.agent_id, runner);
        } catch (e) {
          await this.finalize(run, { status: "FAILED", error: `cannot start member ${run.agent_id}: ${e}` });
          continue;
        }
      }
      if (!runner) continue;
      if (run.agent_id !== leaderId && workersBusy >= limitWorkers) continue;
      const agentRow = state.agents.find((a) => a.id === run.agent_id);
      if (agentRow?.status === "REMOVED") continue;
      if (run.agent_id !== leaderId) workersBusy++;
      const p = this.execute(run).finally(() => {
        this.inflight.delete(run.run_id);
        this.cancelTasks.delete(run.run_id);
        this.signal();
      });
      this.inflight.set(run.run_id, p);
    }
    await this.watchCancellations(state);
  }

  private async watchCancellations(state: SessionState) {
    for (const runId of this.inflight.keys()) {
      if (this.cancelTasks.has(runId)) continue;
      const run = state.runs.find((r) => r.run_id === runId);
      if (!run?.cancel_requested) continue;
      const runner = this.runners.get(run.agent_id);
      if (!runner) continue;
      this.cancelTasks.set(runId, this.requestStop(run, runner));
    }
  }

  private async requestStop(run: TurnRun, runner: AgentRunner) {
    const timeout = (this.opts.limits?.cancelConfirmTimeoutS ?? 60) * 1000;
    try {
      await withTimeout(runner.requestInterrupt(run.run_id), timeout);
    } catch {
      // confirmation timeout: mark OUTCOME_UNKNOWN + expire approvals (RT-06)
      await this.core.call("stop_timeout", { session_id: this.sessionId, run_id: run.run_id });
    } finally {
      this.cancelTasks.delete(run.run_id);
      this.signal();
    }
  }

  private async execute(run: TurnRun) {
    try {
      await this.executeInner(run);
    } catch (e) {
      await this.finalize(run, { status: "FAILED", error: `${(e as Error).name}: ${(e as Error).message}` });
    } finally {
      this.offered.delete(run.run_id);
      this.stepCounts.delete(run.run_id);
      this.signal();
    }
  }

  private async executeInner(run: TurnRun) {
    const runner = this.runners.get(run.agent_id);
    if (!runner) {
      await this.finalize(run, { status: "FAILED", error: `no runner for member ${run.agent_id}` });
      return;
    }
    const { run: fresh, wake } = await this.core.call("begin_run", { session_id: this.sessionId, run_id: run.run_id });
    const view: AgentViewT = await this.core.call("agent_view", { session_id: this.sessionId, agent_id: fresh.agent_id });
    if (view.delivery_ids.length) {
      const offered = this.offered.get(run.run_id) ?? new Set<number>();
      for (const id of view.delivery_ids) offered.add(id);
      this.offered.set(run.run_id, offered);
    }
    const gateway = new ToolGateway(this.core, this.sessionId, fresh.agent_id, fresh.run_id, this.approvals, this.guardedExecutor(fresh));
    this.latestLimits = ((await this.state()) as any).limits ?? null;
    const timeout = ((this.latestLimits?.turn_active_timeout_s ?? this.opts.limits?.turnActiveTimeoutS) ?? 1200) * 1000;
    let outcome: TurnOutcome;
    try {
      outcome = await withTimeout(runner.startOrResume(fresh, view, gateway, wake), timeout);
    } catch (e) {
      if ((e as Error).name === "TurnTimeout")
        outcome = { status: "FAILED", error: `turn active-time limit ${timeout / 1000}s reached` };
      else throw e;
    }
    const state = await this.state();
    const current = state.runs.find((r) => r.run_id === run.run_id);
    if (current?.cancel_requested && ["COMPLETED", "WAITING_TASK", "WAITING_APPROVAL"].includes(outcome.status)) {
      let confirmed: TurnStatus;
      try {
        confirmed = await withTimeout(runner.requestInterrupt(run.run_id), (this.opts.limits?.cancelConfirmTimeoutS ?? 60) * 1000);
      } catch {
        confirmed = "OUTCOME_UNKNOWN";
      }
      outcome = { status: confirmed, note: "cancelled by request" };
    }
    await this.finalize(fresh, outcome);
  }

  private guardedExecutor(run: TurnRun) {
    return async (toolName: string, args: Record<string, unknown>) => {
      const used = (this.stepCounts.get(run.run_id) ?? 0) + 1;
      this.stepCounts.set(run.run_id, used);
      const max = this.latestLimits?.max_model_steps_per_turn ?? this.opts.limits?.maxModelStepsPerTurn ?? 200;
      if (used > max) throw new Error(`step limit ${max} reached for this turn`);
      if (!this.opts.toolExecutor) throw new Error(`no tool executor configured for ${toolName}`);
      return this.opts.toolExecutor(toolName, args);
    };
  }

  private async finalize(run: TurnRun, outcome: TurnOutcome) {
    const ack = [...(this.offered.get(run.run_id) ?? [])];
    await this.core.call("finalize_run", {
      session_id: this.sessionId,
      run_id: run.run_id,
      status: outcome.status,
      error: outcome.error ?? null,
      note: outcome.note ?? null,
      reply_text: outcome.reply_text ?? null,
      ack_ids: ack,
    });
    this.offered.delete(run.run_id);
    await this.drainMidTurn();
    this.signal();
  }

  // -- external backend hooks (codex.py runners report through these) ------------

  async noteExternalStatus(runId: string, status: TurnStatus) {
    const state = await this.state();
    const run = state.runs.find((r) => r.run_id === runId);
    if (!run || TERMINAL.includes(run.status)) return;
    await this.core.call("set_run_status", { session_id: this.sessionId, run_id: runId, status });
    if (status === "WAITING_APPROVAL" && state.pending_approvals.length) {
      const a = state.pending_approvals.filter((x) => x.run_id === runId).pop();
      if (a)
        await this.core.call("emit", {
          session_id: this.sessionId,
          actor_id: run.agent_id,
          events: [{ kind: "approval_requested",
                     payload: { approval_id: a.approval_id, agent_id: run.agent_id, run_id: runId, scope: a.requested_scope } }],
        });
    }
    this.signal();
  }

  async noteExternalProgress(runId: string, text: string) {
    if (!text) return;
    const state = await this.state();
    const run = state.runs.find((r) => r.run_id === runId);
    if (!run) return;
    const task = run.task_id ? state.tasks.find((t) => t.task_id === run.task_id) : null;
    await this.core.call("emit", {
      session_id: this.sessionId,
      actor_id: run.agent_id,
      events: [{ kind: "run_progress",
                 payload: { run_id: runId, agent_id: run.agent_id, text: text.slice(0, 2000),
                            requester: task?.requester ?? null, task_id: run.task_id } }],
    });
    this.signal();
  }

  noteStreamChunk(runId: string, agentId: string, text: string) {
    if (this.streamSink && text) this.streamSink(runId, agentId, text);
  }

  // -- test/CLI helper -----------------------------------------------------------

  /** Wait until no runs are executing or queued. */
  async settle(timeoutS = 10): Promise<boolean> {
    const deadline = Date.now() + timeoutS * 1000;
    while (Date.now() < deadline) {
      const state = await this.state();
      const busy = state.runs.some((r) => ["QUEUED", "RUNNING"].includes(r.status));
      if (!busy && this.inflight.size === 0) return true;
      await new Promise((r) => setTimeout(r, 20));
    }
    return false;
  }
}

class TimeoutError extends Error {
  constructor(msg: string) {
    super(msg);
    this.name = "TurnTimeout";
  }
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout>;
  const t = new Promise<T>((_, reject) => {
    timer = setTimeout(() => reject(new TimeoutError("timeout")), ms);
    timer.unref(); // a timeout must not hold the event loop open
  });
  return Promise.race([p, t]).finally(() => clearTimeout(timer));
}

