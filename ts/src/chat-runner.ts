/**
 * LLM chat runner: the deepagents-kind member backend for the TS rewrite.
 * A plain tool-calling loop over an OpenAI-compatible endpoint — the graph
 * framework (deepagents/LangGraph) is replaced by this loop; team semantics
 * stay in the Rust core.
 */
import type { AgentRunner } from "./runtime.ts";
import type { ToolGateway } from "./gateway.ts";
import type { AgentViewT, TurnOutcome, TurnRun, TurnStatus, WakeInfo } from "./types.ts";
import type { ModelProfileT, UserConfigT } from "./config.ts";

/** runners.py::render_view — compact text view for the next model call. */
export function renderView(view: AgentViewT, wake: WakeInfo | null, workdir?: string): string {
  const parts: string[] = [];
  if (wake && wake.reason !== "new_input") parts.push(`<wake reason="${wake.reason}">${JSON.stringify(wake.payload)}</wake>`);
  if (view.assignment.length)
    parts.push(
      "<your_tasks>" +
        JSON.stringify(
          view.assignment.map((t) => ({
            task_id: t.task_id,
            description: t.description,
            acceptance: t.acceptance,
            status: t.status,
            requester: t.requester,
          })),
        ) +
        "</your_tasks>",
    );
  for (const item of view.inbox_delta)
    parts.push(`<inbox from="${item.from}" kind="${item.kind}">${JSON.stringify(item.payload)}</inbox>`);
  if (view.permitted_shared_delta.length)
    parts.push(
      "<shared_space_updates>" +
        JSON.stringify(
          view.permitted_shared_delta.map((e: any) => ({
            space: e.space_id,
            author: e.author,
            kind: e.kind,
            content: String(e.content ?? "").slice(0, 500),
            ref: e.ref,
          })),
        ) +
        "</shared_space_updates>",
    );
  const topo = view.relevant_topology as any;
  parts.push(
    `<team revision="${topo.revision}">` +
      JSON.stringify({
        members: topo.members,
        you_can_message: topo.can_send_to,
        you_can_delegate_to: topo.can_delegate_to,
        shared_spaces: topo.shared_spaces,
      }) +
      "</team>",
  );
  if (workdir) parts.push(`<your_workspace>${workdir}</your_workspace>`);
  return parts.join("\n");
}

export const TEAM_TOOL_DOCS: Record<string, string> = {
  send_message: "Send a message to a teammate you are allowed to reach. target='*' broadcasts where a broadcast channel exists.",
  assign_task: "Assign a task to a teammate; returns a task_id immediately and never waits for completion. Include acceptance criteria.",
  complete_task: "Report the current task finished with result refs and a short summary; the task becomes SUCCEEDED when your turn ends cleanly.",
  wait_for_tasks: "Park this turn until the given tasks finish (or the user sends new input). Releases your execution slot.",
  publish_shared: "Append a structured entry (finding/decision/artifact ref) to a shared space you can write to.",
  read_shared: "Read shared-space entries after a sequence cursor.",
  list_shared: "List shared spaces you can read and their entry counts.",
  request_help: "Ask the Leader for help with your current task.",
  propose_team_change: "Propose a team/topology change to the Leader; only the Leader can apply it.",
  apply_topology_patch: "Leader only: apply (or reject) a topology patch from a base revision.",
  cancel_task: "Leader only: cancel an unfinished or blocked task; running work stops first.",
  cancel_run: "Leader only: request a turn to stop; side effects are not rolled back.",
  signal_done: "Leader only: declare the current user goal complete; the runtime verifies no work, approvals or unknown outcomes are outstanding.",
};

const TEAM_TOOL_SCHEMAS: Record<string, any> = {
  send_message: { type: "object", properties: { target: { type: "string" }, text: { type: "string" } }, required: ["target", "text"] },
  assign_task: { type: "object", properties: { assignee: { type: "string" }, description: { type: "string" }, acceptance: { type: "string" }, dependencies: { type: "array", items: { type: "string" } } }, required: ["assignee", "description"] },
  complete_task: { type: "object", properties: { task_id: { type: "string" }, result_refs: { type: "array", items: { type: "string" } }, summary: { type: "string" } }, required: ["task_id"] },
  wait_for_tasks: { type: "object", properties: { task_ids: { type: "array", items: { type: "string" } } }, required: ["task_ids"] },
  publish_shared: { type: "object", properties: { space_id: { type: "string" }, content: { type: "string" }, kind: { type: "string" }, ref: { type: "string" } }, required: ["space_id"] },
  read_shared: { type: "object", properties: { space_id: { type: "string" }, after_sequence: { type: "integer" }, limit: { type: "integer" } } },
  list_shared: { type: "object", properties: {} },
  request_help: { type: "object", properties: { message: { type: "string" }, task_id: { type: "string" } }, required: ["message"] },
  propose_team_change: { type: "object", properties: { operations: { type: "array", items: { type: "object" } }, rationale: { type: "string" } }, required: ["operations"] },
  apply_topology_patch: { type: "object", properties: { operations: { type: "array", items: { type: "object" } }, patch_id: { type: "string" }, base_revision: { type: "integer" }, reject: { type: "boolean" } } },
  cancel_task: { type: "object", properties: { task_id: { type: "string" } }, required: ["task_id"] },
  cancel_run: { type: "object", properties: { run_id: { type: "string" } }, required: ["run_id"] },
  signal_done: { type: "object", properties: { summary: { type: "string" } } },
};

interface ChatMessage {
  role: "system" | "user" | "assistant" | "tool";
  content: string | null;
  tool_calls?: { id: string; type: "function"; function: { name: string; arguments: string } }[];
  tool_call_id?: string;
}

export class ChatRunner implements AgentRunner {
  private profile: ModelProfileT;
  private messages = new Map<string, ChatMessage[]>(); // per context thread
  private states = new Map<string, TurnStatus>();
  private pausedKind = new Map<string, "approval" | "waiting">();
  private midTurn = new Map<string, any[]>();
  private interrupted = new Set<string>();
  streamHook?: (runId: string, agentId: string, text: string) => void;

  private agent: { id: string; name: string; role: string; instructions?: string; model_profile: string; tool_bindings?: string[] };
  private catalog: UserConfigT;
  private workdir?: string;
  private modelOverride?: ModelProfileT;
  constructor(
    agent: { id: string; name: string; role: string; instructions?: string; model_profile: string; tool_bindings?: string[] },
    catalog: UserConfigT,
    workdir?: string,
    modelOverride?: ModelProfileT,
  ) {
    this.agent = agent;
    this.catalog = catalog;
    this.workdir = workdir;
    this.modelOverride = modelOverride;
    const profile = modelOverride ?? catalog.models[agent.model_profile];
    if (!profile) throw new Error(`unknown model profile ${agent.model_profile}`);
    this.profile = profile;
  }

  queryState(runId: string): TurnStatus | null {
    return this.states.get(runId) ?? null;
  }

  deliverMidTurn(runId: string, items: any[]) {
    this.midTurn.set(runId, [...(this.midTurn.get(runId) ?? []), ...items]);
  }

  async requestInterrupt(runId: string): Promise<TurnStatus> {
    this.interrupted.add(runId);
    this.states.set(runId, "CANCELLED");
    return "CANCELLED";
  }

  /** A restart loses in-memory messages: the segment must not replay blindly. */
  async reconcile(_run: TurnRun): Promise<TurnStatus | null> {
    return null;
  }

  async startOrResume(run: TurnRun, view: AgentViewT, gateway: ToolGateway, wake: WakeInfo | null): Promise<TurnOutcome> {
    this.states.set(run.run_id, "RUNNING");
    this.interrupted.delete(run.run_id);
    const thread = run.context_ref ?? run.run_id;
    const history = this.messages.get(thread) ?? [];
    if (history.length === 0 && this.agent.instructions) {
      history.push({ role: "system", content: this.systemPrompt() });
    }
    // the rendered view is authoritative for what this segment saw
    history.push({ role: "user", content: renderView(view, wake, this.workdir) });
    this.messages.set(thread, history);

    const tools = Object.entries(TEAM_TOOL_SCHEMAS).map(([name, schema]) => ({
      type: "function",
      function: { name, description: TEAM_TOOL_DOCS[name] ?? name, parameters: schema },
    }));

    try {
      const reply = await this.loop(run, history, gateway, tools);
      this.states.set(run.run_id, "COMPLETED");
      return { status: "COMPLETED", reply_text: reply };
    } catch (e) {
      const err = e as Error;
      if (err.name === "TurnInterrupted") {
        this.states.set(run.run_id, "CANCELLED");
        return { status: "CANCELLED" };
      }
      if (err.name === "TurnPaused") {
        const kind = this.pausedKind.get(run.run_id);
        const status: TurnStatus = kind === "approval" ? "WAITING_APPROVAL" : "WAITING_TASK";
        this.states.set(run.run_id, status);
        return { status, note: err.message || undefined };
      }
      this.states.set(run.run_id, "FAILED");
      return { status: "FAILED", error: `${err.name}: ${err.message}` };
    }
  }

  private systemPrompt(): string {
    const toolList = Object.entries(TEAM_TOOL_DOCS).map(([n, d]) => `- ${n}: ${d}`).join("\n");
    return `${this.agent.instructions ?? `You are ${this.agent.name}, role ${this.agent.role}, in a team.`}

Team tools available:
${toolList}

Rules: use complete_task to finish your assigned task; use signal_done only when the whole user goal is complete (Leader only).`;
  }

  private async loop(run: TurnRun, history: ChatMessage[], gateway: ToolGateway, tools: any[]): Promise<string> {
    const maxSteps = 200; // core enforces the configured cap via the gateway executor
    for (let step = 0; step < maxSteps; step++) {
      if (this.interrupted.has(run.run_id)) throw Object.assign(new Error("interrupted"), { name: "TurnInterrupted" });
      // mid-turn inputs are injected at the next model call boundary
      const mid = this.midTurn.get(run.run_id) ?? [];
      if (mid.length) {
        this.midTurn.delete(run.run_id);
        history.push({ role: "user", content: mid.map((i) => `<inbox from="${i.from}" kind="${i.kind}">${JSON.stringify(i.payload)}</inbox>`).join("\n") });
      }
      const msg = await this.chat(history, tools);
      history.push(msg);
      if (msg.content) this.streamHook?.(run.run_id, this.agent.id, msg.content);
      if (!msg.tool_calls?.length) return msg.content ?? "";
      for (const call of msg.tool_calls) {
        if (this.interrupted.has(run.run_id)) throw Object.assign(new Error("interrupted"), { name: "TurnInterrupted" });
        let args: Record<string, unknown> = {};
        try {
          args = JSON.parse(call.function.arguments || "{}");
        } catch {
          history.push({ role: "tool", tool_call_id: call.id, content: "invalid JSON arguments" });
          continue;
        }
        const receipt = await gateway.call(call.function.name, args, call.id);
        if (receipt.error === "approval_required") {
          this.pausedKind.set(run.run_id, "approval");
          throw Object.assign(new Error(String(receipt.result?.approval_id ?? "")), { name: "TurnPaused" });
        }
        if (call.function.name === "wait_for_tasks" && (receipt.result as any)?.waiting === true) {
          this.pausedKind.set(run.run_id, "waiting");
          throw Object.assign(new Error("waiting"), { name: "TurnPaused" });
        }
        history.push({ role: "tool", tool_call_id: call.id, content: JSON.stringify(receipt.ok ? receipt.result : { error: receipt.error }) });
      }
    }
    throw Object.assign(new Error(`step limit ${maxSteps} reached`), { name: "TurnLimitExceeded" });
  }

  private async chat(messages: ChatMessage[], tools: any[]): Promise<ChatMessage> {
    const apiKey = this.profile.api_key_env ? process.env[this.profile.api_key_env] : undefined;
    if (!apiKey) throw new Error(`missing API key env ${this.profile.api_key_env}`);
    const base = (this.profile.base_url ?? "https://api.openai.com/v1").replace(/\/$/, "");
    const body: Record<string, any> = {
      model: this.profile.model,
      messages,
      tools,
      ...this.profile.generation_options,
    };
    let lastErr: Error | null = null;
    for (let attempt = 0; attempt <= this.profile.max_retries; attempt++) {
      try {
        const res = await fetch(`${base}/chat/completions`, {
          method: "POST",
          headers: { "content-type": "application/json", authorization: `Bearer ${apiKey}` },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(this.profile.timeout * 1000),
        });
        if (!res.ok) throw new Error(`chat API ${res.status}: ${(await res.text()).slice(0, 500)}`);
        const data = (await res.json()) as any;
        return data.choices[0].message as ChatMessage;
      } catch (e) {
        lastErr = e as Error;
        await new Promise((r) => setTimeout(r, Math.min(2 ** attempt * 500, 8000)));
      }
    }
    throw lastErr ?? new Error("chat call failed");
  }
}
