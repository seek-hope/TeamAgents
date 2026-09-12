/** Session bootstrap, ported from session.py::open_session. */
import { mkdirSync } from "node:fs";
import { resolve } from "node:path";
import { CoreClient } from "./core-client.ts";
import { loadUserConfig, type UserConfigT } from "./config.ts";
import { SessionRuntime, type AgentRunner } from "./runtime.ts";
import { ChatRunner } from "./chat-runner.ts";
import { CodexRunner } from "./codex-runner.ts";
import { ApprovalGate, PermissionPolicy } from "./gateway.ts";
import { acquireSessionLock, newSessionId, sessionPaths } from "./sessions.ts";
import { workspaceExecutor } from "./tools.ts";

export const LEADER_INSTRUCTIONS = `You are the Leader of a team of agents. Understand the user's goal, decide
whether to work alone or build a team, delegate with assign_task, coordinate
with send_message, and report completion with signal_done. Keep task descriptions
specific, include acceptance criteria, and never bypass runtime permissions.
`;

export function defaultLeaderSpec(profile = "leader_main", tools = ["files", "shell", "web"]) {
  return {
    leader_id: "leader",
    agents: [
      {
        id: "leader",
        name: "Leader",
        role: "leader",
        runtime_kind: "deepagents",
        instructions: LEADER_INSTRUCTIONS,
        model_profile: profile,
        tool_bindings: tools,
      },
    ],
    shared_spaces: [{ id: "main", readers: ["leader"], writers: ["leader"] }],
  };
}

export interface OpenedSession {
  runtime: SessionRuntime;
  core: CoreClient;
  sessionId: string;
  close: () => Promise<void>;
}

export async function openSession(opts: {
  cwd?: string;
  sessionId?: string;
  fullAuto?: boolean;
  catalog?: UserConfigT;
  initialSpec?: Record<string, any>;
  coreBin?: string;
  runnerFactory?: (agent: any) => AgentRunner;
}): Promise<OpenedSession> {
  const cwd = resolve(opts.cwd ?? process.cwd());
  const catalog = opts.catalog ?? loadUserConfig();
  const sessionId = opts.sessionId ?? newSessionId(cwd);
  const paths = sessionPaths(sessionId);
  mkdirSync(paths.artifacts, { recursive: true });
  const releaseLock = acquireSessionLock(sessionId);

  const core = new CoreClient(opts.coreBin ?? new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname, paths.db);
  try {
    const probe = await core.call("state", { session_id: sessionId }).catch(() => null);
    if (!probe?.session) {
      await core.createSession(sessionId, cwd, opts.fullAuto ? "full_auto" : "approved_scope");
      await core.call("save_spec", { session_id: sessionId, spec: opts.initialSpec ?? defaultLeaderSpec() });
    } else if (opts.fullAuto) {
      await core.submit({
        action_id: `mode_${Date.now()}`,
        session_id: sessionId,
        actor_id: "user",
        kind: "set_permission_mode",
        payload: { mode: "full_auto" },
      });
    }
    const state = await core.call("state", { session_id: sessionId });
    const approvals = new ApprovalGate(core, sessionId, new PermissionPolicy());
    if (state.session?.permissions_mode === "full_auto") approvals.setMode("full_auto");
    const agents: any[] = state.spec?.agents ?? [];

    const memberWorkdir = (agent: any) => {
      const dir = resolve(paths.artifacts, "..", "workspaces", agent.id);
      mkdirSync(dir, { recursive: true });
      return dir;
    };
    const makeRunner = (agent: any): AgentRunner => {
      if (agent.runtime_kind === "codex") {
        const profile = catalog.models[agent.model_profile];
        const overrides: Record<string, any> = {};
        if (profile) {
          overrides.model = profile.model;
          if (profile.provider) overrides.model_provider = profile.provider;
          Object.assign(overrides, profile.generation_options ?? {});
        }
        return new CodexRunner({
          agent,
          sessionId,
          workdir: memberWorkdir(agent),
          approvals,
          core,
          sandbox: "workspace-write",
          approvalPolicy: "on-request",
          effort: "xhigh",
          configOverrides: Object.keys(overrides).length ? overrides : undefined,
        });
      }
      return new ChatRunner(agent, catalog, memberWorkdir(agent));
    };

    const runners = opts.runnerFactory
      ? Object.fromEntries(agents.map((a: any) => [a.id, opts.runnerFactory!(a)]))
      : Object.fromEntries(agents.map((a: any) => [a.id, makeRunner(a)]));

    const runtime = new SessionRuntime(core, sessionId, {
      runners,
      approvals,
      toolExecutor: workspaceExecutor(cwd),
      runnerFactory: opts.runnerFactory ?? makeRunner,
    });
    for (const r of Object.values(runners)) {
      r.statusHook = (runId, status) => void runtime.noteExternalStatus(runId, status);
      r.progressHook = (runId, text) => void runtime.noteExternalProgress(runId, text);
      r.streamHook = (runId, agentId, text) => runtime.noteStreamChunk(runId, agentId, text);
    }
    return {
      runtime,
      core,
      sessionId,
      close: async () => {
        await runtime.close();
        for (const r of Object.values(runners)) await (r as any).close?.();
        core.close();
        releaseLock();
      },
    };
  } catch (e) {
    core.close();
    releaseLock();
    throw e;
  }
}
