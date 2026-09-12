#!/usr/bin/env node
/**
 * Headless session worker for the Rust ratatui TUI.
 * One JSON request per line on stdin:  {"id": N, "method": "...", "params": {...}}
 * One JSON response per line:          {"id": N, "result": ...} | {"id": N, "error": "..."}
 * Async pushes (no id):                {"push": "delta", "run_id", "agent_id", "text"}
 *
 * The worker owns the SessionRuntime (execution engine); the Rust side is a
 * pure UI client. `call` passes through to teamagents-core with session_id
 * injected. ponytail: reuses the tested TS runtime instead of re-porting it.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import readline from "node:readline";
import { openSession, type OpenedSession } from "./session.ts";
import { loadUserConfig, sessionsDir, userConfigPath, xdgStateHome } from "./config.ts";
import { archiveSession, deleteSession, listSessions, newSessionId } from "./sessions.ts";
import { ScriptedMember, type Step } from "./scripted.ts";

let opened: OpenedSession | null = null;
let openOpts: { cwd?: string; fullAuto?: boolean; team?: string; coreBin?: string } = {};

const out = (msg: Record<string, unknown>) => process.stdout.write(JSON.stringify(msg) + "\n");
const pushDelta = (run_id: string, agent_id: string, text: string) =>
  out({ push: "delta", run_id, agent_id, text });

async function open(params: any): Promise<Record<string, unknown>> {
  await closeCurrent();
  openOpts = { cwd: params.cwd, fullAuto: params.fullAuto, team: params.team, coreBin: params.coreBin };
  const scripts = params.scripts as Record<string, Step[]> | undefined;
  opened = await openSession({
    cwd: params.cwd,
    sessionId: params.resume,
    fullAuto: params.fullAuto,
    initialSpec: params.team ? JSON.parse(readFileSync(params.team, "utf8")) : undefined,
    coreBin: params.coreBin,
    runnerFactory: scripts
      ? (agent: any) => new ScriptedMember(agent.id, scripts[agent.id] ?? [["end"]])
      : undefined,
  });
  await opened.runtime.start();
  opened.runtime.streamSink = pushDelta;
  return {
    session_id: opened.sessionId,
    state_dir: join(xdgStateHome(), "teamagents"),
    sessions_dir: sessionsDir(),
    user_config_path: userConfigPath(),
    catalog: loadUserConfig(),
  };
}

async function closeCurrent(): Promise<void> {
  const cur = opened;
  opened = null;
  if (cur) await cur.close();
}

function needSession(): OpenedSession {
  if (!opened) throw new Error("no open session");
  return opened;
}

const handlers: Record<string, (p: any) => Promise<unknown>> = {
  ping: async () => ({ worker: "0.1.0" }),
  open,
  call: async (p) => {
    const cur = needSession();
    return cur.core.call(p.method, { session_id: cur.sessionId, ...(p.params ?? {}) });
  },
  submit: async (p) => {
    const cur = needSession();
    return cur.runtime.submit({ actor_id: "user", ...p.action, session_id: cur.sessionId });
  },
  user_message: async (p) => {
    const cur = needSession();
    return cur.runtime.userMessage(String(p.text ?? ""), { supplement: Boolean(p.supplement) });
  },
  list_sessions: async (p) => ({ sessions: listSessions({ cwd: p.cwd ?? openOpts.cwd, includeArchived: true }) }),
  switch_session: async (p) => open({ ...openOpts, resume: p.session_id }),
  new_session: async () => open({ ...openOpts, resume: newSessionId(openOpts.cwd ?? process.cwd()) }),
  archive_session: async (p) => {
    const wasCurrent = opened?.sessionId === p.session_id;
    if (wasCurrent) await closeCurrent();
    const target = archiveSession(p.session_id);
    return { was_current: wasCurrent, target };
  },
  delete_session: async (p) => {
    const wasCurrent = opened?.sessionId === p.session_id;
    if (wasCurrent) await closeCurrent();
    deleteSession(p.session_id);
    return { was_current: wasCurrent };
  },
  close: async () => {
    // respond first, then exit after the reply line is flushed
    setImmediate(() => void closeCurrent().finally(() => process.exit(0)));
    return { ok: true };
  },
};

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  if (!line.trim()) return;
  let req: any;
  try {
    req = JSON.parse(line);
  } catch {
    return;
  }
  const { id, method, params } = req;
  const handler = handlers[method];
  if (!handler) {
    out({ id, error: `unknown method ${method}` });
    return;
  }
  handler(params ?? {})
    .then((result) => out({ id, result }))
    .catch((e) => out({ id, error: String(e?.message ?? e) }));
});
rl.on("close", () => {
  void closeCurrent().finally(() => process.exit(0));
});
process.on("unhandledRejection", (e) => console.error("[worker] unhandled:", e));
