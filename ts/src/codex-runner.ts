/**
 * Codex execution member backend, ported from codex.py:
 * one app-server JSON-RPC process + one thread per member.
 */
import { spawn, type ChildProcess } from "node:child_process";
import readline from "node:readline";
import type { AgentRunner } from "./runtime.ts";
import { operationHash, type ApprovalGate } from "./gateway.ts";
import type { CoreClient } from "./core-client.ts";
import { renderView } from "./chat-runner.ts";
import type { AgentViewT, TurnOutcome, TurnRun, TurnStatus, WakeInfo } from "./types.ts";

export class CodexError extends Error {}

/** Minimal JSON-RPC client for one app-server process (codex.py::CodexAppServer). */
export class CodexAppServer {
  proc: ChildProcess | null = null;
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
  notify: ((message: any) => void) | null = null;
  onRequest: ((message: any) => Promise<any> | any) | null = null;
  stderrLines: string[] = [];

  private opts: {
    codexBin?: string;
    cwd: string;
    configOverrides?: Record<string, any>;
    env?: Record<string, string>;
    codexHome?: string;
  };
  constructor(opts: {
    codexBin?: string;
    cwd: string;
    configOverrides?: Record<string, any>;
    env?: Record<string, string>;
    codexHome?: string;
  }) {
    this.opts = opts;
  }

  async start() {
    const args = ["app-server"];
    for (const [key, value] of Object.entries(this.opts.configOverrides ?? {}))
      args.push("-c", typeof value === "string" ? `${key}=${value}` : `${key}=${JSON.stringify(value)}`);
    const env = { ...process.env, ...(this.opts.codexHome ? { CODEX_HOME: this.opts.codexHome } : {}), ...(this.opts.env ?? {}) };
    this.proc = spawn(this.opts.codexBin ?? "codex", args, {
      cwd: this.opts.cwd,
      env,
      stdio: ["pipe", "pipe", "pipe"],
      detached: true,
    });
    const rl = readline.createInterface({ input: this.proc.stdout! });
    rl.on("line", (line) => void this.onLine(line));
    this.proc.stderr!.on("data", (d) => {
      for (const line of String(d).split("\n").filter(Boolean))
        this.stderrLines = [...this.stderrLines, line].slice(-50);
    });
    await this.call("initialize", { clientInfo: { name: "teamagents", title: "TeamAgents", version: "0.1.0" } });
  }

  private async onLine(line: string) {
    let message: any;
    try {
      message = JSON.parse(line);
    } catch {
      return;
    }
    if ("id" in message && ("result" in message || "error" in message)) {
      const slot = this.pending.get(Number(message.id));
      if (slot) {
        this.pending.delete(Number(message.id));
        if ("error" in message) slot.reject(new CodexError(String(message.error)));
        else slot.resolve(message.result);
      }
    } else if ("id" in message && "method" in message) {
      try {
        const result = (await this.onRequest?.(message)) ?? {};
        await this.respond(message.id, result);
      } catch {
        await this.respond(message.id, { decision: "decline" });
      }
    } else if ("method" in message) {
      try {
        this.notify?.(message);
      } catch {}
    }
  }

  call(method: string, params: Record<string, any> = {}, timeoutMs = 60_000): Promise<any> {
    if (!this.proc?.stdin) return Promise.reject(new CodexError("app-server is not running"));
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.proc!.stdin!.write(JSON.stringify({ id, method, params }) + "\n");
      const t = setTimeout(() => {
        if (this.pending.delete(id)) reject(new CodexError(`${method} timed out`));
      }, timeoutMs);
      t.unref();
    });
  }

  async respond(requestId: any, result: any) {
    this.proc?.stdin?.write(JSON.stringify({ id: requestId, result }) + "\n");
  }

  async close() {
    if (this.proc && this.proc.exitCode == null) {
      try {
        process.kill(-this.proc.pid!, "SIGTERM");
      } catch {}
      await new Promise((r) => setTimeout(r, 500));
      if (this.proc.exitCode == null) {
        try {
          process.kill(-this.proc.pid!, "SIGKILL");
        } catch {}
      }
    }
    this.proc = null;
  }
}

const TURN_STATUS_MAP: Record<string, TurnStatus> = {
  completed: "COMPLETED",
  interrupted: "CANCELLED",
  failed: "FAILED",
  inProgress: "RUNNING",
};

export class CodexRunner implements AgentRunner {
  server: CodexAppServer | null = null;
  threadId: string | null = null;
  private states = new Map<string, TurnStatus>();
  private currentTurn = new Map<string, string>();
  private turnDone = new Map<string, { resolve: () => void; promise: Promise<void> }>();
  private progress = new Map<string, string[]>();
  private reported = new Map<string, number>();
  private approvalWaits = new Map<string, (decision: string) => void>();
  private approvalIds = new Map<string, string>();
  private queuedInput = new Map<string, string[]>();
  private bufferedNotes = new Map<string, any[]>();
  private effortFallbackUsed = false;
  statusHook?: (runId: string, status: TurnStatus) => void;
  progressHook?: (runId: string, text: string) => void;
  streamHook?: (runId: string, agentId: string, text: string) => void;

  private opts: {
      agent: { id: string; name: string; instructions?: string };
      sessionId: string;
      workdir: string;
      approvals: ApprovalGate;
      core: CoreClient;
      sandbox?: string;
      approvalPolicy?: string;
      effort?: string | null;
      model?: string;
      codexBin?: string;
      codexHome?: string;
      env?: Record<string, string>;
      configOverrides?: Record<string, any>;
      serverFactory?: (o: any) => CodexAppServer;
  };
  constructor(opts: CodexRunner["opts"]) {
    this.opts = opts;
  }

  queryState(runId: string): TurnStatus | null {
    return this.states.get(runId) ?? null;
  }

  deliverMidTurn(runId: string, items: any[]) {
    const texts = items.map((i) => `<inbox from="${i.from}" kind="${i.kind}">${JSON.stringify(i.payload)}</inbox>`);
    const agentId = this.opts.agent.id;
    this.queuedInput.set(agentId, [...(this.queuedInput.get(agentId) ?? []), ...texts]);
    // a live turn gets nudged via turn/steer when possible
    const turnId = this.currentTurn.get(runId);
    if (turnId && this.server && this.threadId) {
      void this.server.call("turn/steer", { threadId: this.threadId, turnId, input: texts.map((t) => ({ type: "text", text: t })) }).catch(() => {});
    }
  }

  private async ensureServer(): Promise<CodexAppServer> {
    if (!this.server) {
      const overrides = { ...(this.opts.configOverrides ?? {}) };
      if (this.opts.model) overrides.model = this.opts.model;
      const factory = this.opts.serverFactory ?? ((o: any) => new CodexAppServer(o));
      this.server = factory({
        cwd: this.opts.workdir,
        codexHome: this.opts.codexHome,
        configOverrides: overrides,
        env: this.opts.env,
        codexBin: this.opts.codexBin,
      });
      await this.server.start();
      const { thread_id } = await this.opts.core.call("get_codex_thread", {
        session_id: this.opts.sessionId,
        agent_id: this.opts.agent.id,
      });
      this.threadId = thread_id;
    }
    return this.server;
  }

  private async ensureThread(server: CodexAppServer): Promise<string> {
    if (this.threadId) return this.threadId;
    const params: Record<string, any> = {
      cwd: this.opts.workdir,
      sandbox: this.opts.sandbox ?? "workspace-write",
      approvalPolicy: this.opts.approvalPolicy ?? "on-request",
      approvalsReviewer: "user",
      threadSource: "appServer",
    };
    if (this.opts.model) params.model = this.opts.model;
    const result = await server.call("thread/start", params);
    this.threadId = result.thread.id;
    // persist the thread before submitting a turn (§10.2)
    await this.opts.core.call("set_codex_thread", {
      session_id: this.opts.sessionId,
      agent_id: this.opts.agent.id,
      thread_id: this.threadId,
    });
    return this.threadId!;
  }

  async startOrResume(run: TurnRun, view: AgentViewT, gateway: any, wake: WakeInfo | null): Promise<TurnOutcome> {
    const server = await this.ensureServer();
    const threadId = await this.ensureThread(server);
    const runId = run.run_id;
    this.states.set(runId, "RUNNING");
    let resolveDone!: () => void;
    const done = new Promise<void>((r) => (resolveDone = r));
    this.turnDone.set(runId, { resolve: resolveDone, promise: done });
    this.progress.set(runId, []);
    const text = this.renderInput(view, wake);
    const params: Record<string, any> = {
      threadId,
      input: [{ type: "text", text }],
      approvalPolicy: this.opts.approvalPolicy ?? "on-request",
      approvalsReviewer: "user",
    };
    let effort = this.opts.effort ?? "xhigh";
    if (effort) params.effort = effort;
    // handlers live before the turn starts
    server.notify = (m) => this.onNotification(runId, m);
    server.onRequest = (m) => this.onRequest(runId, m);
    let result;
    try {
      result = await server.call("turn/start", params);
    } catch (e) {
      if (effort && !this.effortFallbackUsed && String(e).toLowerCase().includes("effort")) {
        this.effortFallbackUsed = true;
        effort = "max";
        params.effort = effort;
        result = await server.call("turn/start", params);
      } else {
        this.states.set(runId, "FAILED");
        return { status: "FAILED", error: `CodexError: ${(e as Error).message}` };
      }
    }
    const turnId = result.turn.id;
    this.currentTurn.set(runId, turnId);
    for (const m of this.bufferedNotes.get(runId) ?? []) this.applyNotification(runId, turnId, m);
    this.bufferedNotes.delete(runId);
    await this.opts.core.call("set_run_external_turn", { session_id: this.opts.sessionId, run_id: runId, external_turn_id: turnId });

    await done;
    const status = this.states.get(runId) ?? "OUTCOME_UNKNOWN";
    if (status === "WAITING_APPROVAL") return { status, note: this.approvalIds.get(runId) };
    if (status === "COMPLETED" && run.task_id) {
      const summary = (this.progress.get(runId) ?? []).join(" ").slice(-2000);
      const receipt = await gateway.call(
        "complete_task",
        { task_id: run.task_id, summary, result_refs: [] },
        `${turnId}:complete`,
      );
      if (!receipt.ok) console.warn(`completion request rejected for ${run.task_id}: ${receipt.error}`);
    }
    const pieces = this.progress.get(runId) ?? [];
    return {
      status,
      reply_text: pieces.join(" ").slice(-4000) || undefined,
      error: status === "FAILED" ? pieces[pieces.length - 1] : undefined,
    };
  }

  private renderInput(view: AgentViewT, wake: WakeInfo | null): string {
    let text = renderView(view, wake, this.opts.workdir);
    const queued = this.queuedInput.get(view.agent_id) ?? [];
    if (queued.length) {
      this.queuedInput.delete(view.agent_id);
      text += "\n<queued_updates>" + queued.join("\n") + "</queued_updates>";
    }
    text +=
      "\n<codex_member>\nYou are an execution member. Work the assigned task, report progress through your own outputs; the Leader coordinates the team. Do not attempt team-management actions.\n</codex_member>";
    return text;
  }

  private onNotification(runId: string, message: any) {
    const expected = this.currentTurn.get(runId);
    if (expected == null) {
      // buffer events that arrive before turn/start returns
      this.bufferedNotes.set(runId, [...(this.bufferedNotes.get(runId) ?? []), message]);
      return;
    }
    this.applyNotification(runId, expected, message);
  }

  private applyNotification(runId: string, expectedTurn: string, message: any) {
    const method = message.method ?? "";
    const params = message.params ?? {};
    const turn = params.turn ?? {};
    const turnId = turn.id ?? params.turnId;
    if (turnId && turnId !== expectedTurn) return;
    if (method === "item/agentMessage/delta") {
      const delta = params.delta ?? "";
      this.progress.set(runId, [...(this.progress.get(runId) ?? []), delta]);
      if (delta) this.streamHook?.(runId, this.opts.agent.id, delta);
    } else if (method === "item/completed") {
      const item = params.item ?? {};
      if (item.type === "agentMessage" && item.text) {
        this.progress.set(runId, [...(this.progress.get(runId) ?? []), item.text]);
        this.progressHook?.(runId, item.text);
      }
    } else if (method === "turn/completed") {
      const status = TURN_STATUS_MAP[turn.status ?? ""] ?? "FAILED";
      const pieces = this.progress.get(runId) ?? [];
      const reported = this.reported.get(runId) ?? 0;
      const newText = pieces.slice(reported).join(" ").trim();
      if (newText && this.progressHook) {
        this.reported.set(runId, pieces.length);
        this.progressHook(runId, newText);
      }
      this.states.set(runId, status);
      this.turnDone.get(runId)?.resolve();
    } else if (method === "error") {
      this.progress.set(runId, [...(this.progress.get(runId) ?? []), String(params.message ?? "")]);
    }
  }

  private async onRequest(runId: string, message: any): Promise<any> {
    const method: string = message.method ?? "";
    const params = message.params ?? {};
    if (!method.endsWith("requestApproval")) {
      if (method === "item/tool/requestUserInput") return { answers: [] };
      return {};
    }
    const scope = { kind: method.includes("/") ? method.split("/")[1] : method, request: params };
    const opHash = operationHash(String(scope.kind ?? "external"), (scope as any).request ?? {});
    const req = {
      approval_id: `appr_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`,
      session_id: this.opts.sessionId,
      agent_id: this.opts.agent.id,
      run_id: runId,
      tool_call_id: String(params.itemId ?? params.approvalId ?? `cx_${Date.now()}`),
      operation_hash: opHash,
      requested_scope: scope,
      policy_revision: this.opts.approvals.policyRevision,
      status: "PENDING",
      created_at: Date.now() / 1000,
      decided_at: null,
    };
    await this.opts.core.call("insert_approval", { session_id: this.opts.sessionId, approval: req });
    this.approvalIds.set(runId, req.approval_id);
    this.states.set(runId, "WAITING_APPROVAL");
    this.statusHook?.(runId, "WAITING_APPROVAL");
    const decision = await new Promise<string>((resolve) => this.approvalWaits.set(req.approval_id, resolve));
    this.states.set(runId, "RUNNING");
    this.statusHook?.(runId, "RUNNING");
    return { decision };
  }

  /** Called by the runtime when the user decides (once/session/deny). */
  resolveApproval(approvalId: string, decision: string): boolean {
    const wait = this.approvalWaits.get(approvalId);
    if (!wait) return false;
    this.approvalWaits.delete(approvalId);
    wait({ once: "accept", session: "acceptForSession", deny: "decline" }[decision] ?? "decline");
    return true;
  }

  async requestInterrupt(runId: string): Promise<TurnStatus> {
    const turnId = this.currentTurn.get(runId);
    if (!this.server || !turnId || !this.threadId) {
      this.states.set(runId, "CANCELLED");
      return "CANCELLED";
    }
    try {
      await this.server.call("turn/interrupt", { threadId: this.threadId, turnId }, 30_000);
    } catch {}
    // cancellation is confirmed only when the turn reports a terminal state
    try {
      await Promise.race([
        this.turnDone.get(runId)?.promise ?? Promise.resolve(),
        new Promise((_, rej) => {
          const t = setTimeout(() => rej(new Error("timeout")), 30_000);
          t.unref();
        }),
      ]);
    } catch {
      this.states.set(runId, "OUTCOME_UNKNOWN");
    }
    const status = this.states.get(runId) ?? "OUTCOME_UNKNOWN";
    return ["CANCELLED", "COMPLETED", "FAILED"].includes(status) ? status : "OUTCOME_UNKNOWN";
  }

  /** A codex turn survives our restart; query the live thread state. */
  async reconcile(run: TurnRun): Promise<TurnStatus | null> {
    if (!this.server || !this.threadId || !run.external_turn_id) return null;
    try {
      const result = await this.server.call("thread/status", { threadId: this.threadId });
      const active = result?.activeTurnId ?? null;
      return active ? "RUNNING" : null;
    } catch {
      return null;
    }
  }

  async close() {
    await this.server?.close();
    this.server = null;
  }
}
