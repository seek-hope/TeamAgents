#!/usr/bin/env node
/** teamagents CLI, ported from cli.py. */
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync, unlinkSync } from "node:fs";
import { resolve } from "node:path";
import readline from "node:readline";
import { CoreClient } from "./core-client.ts";
import { loadUserConfig, sessionsDir, userConfigPath } from "./config.ts";
import { listSessions } from "./sessions.ts";
import { openSession } from "./session.ts";
import { bwrapAvailable } from "./tools.ts";

const VERSION = "0.1.0-ts";
const coreBin = process.env.TEAMAGENTS_CORE ?? new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname;

function which(name: string): string | null {
  try {
    return execFileSync("which", [name], { encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

async function doctor(): Promise<number> {
  const results: [string, boolean, string][] = [];
  const check = (name: string, ok: boolean, detail = ""): [string, boolean, string] => {
    results.push([name, ok, detail]);
    return [name, ok, detail];
  };
  try {
    const core = new CoreClient(coreBin);
    const info = await core.call("ping");
    core.close();
    check("rust core", true, `teamagents-core ${info.core}`);
  } catch (e) {
    check("rust core", false, String(e));
  }
  try {
    const catalog = loadUserConfig();
    check("user config", true, `models=${JSON.stringify(Object.keys(catalog.models))} tools=${JSON.stringify(Object.keys(catalog.tools))}`);
    for (const [name, profile] of Object.entries(catalog.models)) {
      const env = profile.api_key_env;
      if (env) check(`model profile ${name}`, Boolean(process.env[env]), `${profile.provider}/${profile.model}${process.env[env] ? "" : ` (missing env ${env})`}`);
    }
  } catch (e) {
    check("user config", false, String(e));
  }
  check("bubblewrap isolation", await bwrapAvailable(), (await bwrapAvailable()) ? "bwrap present" : "bwrap not found: out-of-scope commands must ask for approval");
  const codex = which("codex");
  check("codex app-server", Boolean(codex), codex ? "codex CLI found" : "codex CLI not found");
  try {
    const dir = sessionsDir();
    mkdirSync(dir, { recursive: true });
    const probe = resolve(dir, ".doctor-probe");
    writeFileSync(probe, "ok");
    unlinkSync(probe);
    check("state directory", true, dir);
  } catch (e) {
    check("state directory", false, String(e));
  }
  console.log(`TeamAgents doctor (${VERSION})`);
  let failed = 0;
  for (const [name, ok, detail] of results) {
    if (!ok) failed++;
    console.log(`  [${ok ? "ok  " : "FAIL"}] ${name.padEnd(28)} ${detail}`);
  }
  return failed ? 1 : 0;
}

async function validateSpec(path: string): Promise<number> {
  const core = new CoreClient(coreBin);
  try {
    const spec = JSON.parse(readFileSync(path, "utf8"));
    const catalog = loadUserConfig();
    const result = await core
      .call("validate_spec", { spec, models: Object.keys(catalog.models), tools: Object.keys(catalog.tools) })
      .catch((e: Error) => ({ error: String(e) }));
    if ((result as any).error) {
      console.log(`invalid: ${(result as any).error}`);
      return 1;
    }
    console.log(`ok: ${path} — leader=${result.leader} members=${result.members} channels=${result.channels} spaces=${result.spaces}`);
    return 0;
  } catch (e) {
    console.log(`invalid: ${e}`);
    return 1;
  } finally {
    core.close();
  }
}

function listSessionsCmd(verbose: boolean): number {
  const rows = listSessions({ includeArchived: true });
  if (!rows.length) {
    console.log(`没有会话记录（${sessionsDir()}）`);
    return 0;
  }
  console.log(`会话记录目录：${sessionsDir()}`);
  for (const r of rows) {
    const updated = r.updatedAt ? new Date(r.updatedAt * 1000).toISOString().slice(5, 16).replace("T", " ") : "?";
    const flags = [r.archived && "已归档", r.locked && "运行中", r.error && `读取异常:${r.error.slice(0, 40)}`].filter(Boolean).join(" ");
    console.log(
      `  ${r.sessionId.padEnd(24)} ${r.status.padEnd(7)} 目标 ${r.goalState.padEnd(7)} 事件 ${String(r.events).padEnd(5)} 任务 ${String(r.tasks).padEnd(3)} ${r.sizeMb.toFixed(1).padStart(6)}MB  ${updated}  ${r.cwd}  ${flags}`,
    );
    if (verbose) console.log(`      ${r.path}`);
  }
  console.log("\n在 TUI 的“会话”面板可切换/新建/归档/删除；命令行恢复：teamagents --resume <会话 id>");
  return 0;
}

function printEvent(event: any) {
  const { kind, actor_id: actor } = event;
  const payload = event.payload ?? {};
  if (kind === "user_message") return;
  if (["task_completed", "task_failed", "task_blocked", "task_created", "goal_done", "limit_reached"].includes(kind))
    console.log(`  [${kind}] ${JSON.stringify(payload).slice(0, 200)}`);
  else if (kind === "message") console.log(`  [message] ${actor} -> ${payload.target}: ${String(payload.text ?? "").slice(0, 160)}`);
  else if (kind === "approval_requested") console.log(`  [approval] ${JSON.stringify(payload).slice(0, 200)}`);
  else if (kind === "leader_reply") console.log(`  [Leader] ${String(payload.text ?? "").slice(0, 2000)}`);
  else if (kind === "run_failed") console.log(`  [运行失败] ${actor}: ${String(payload.error ?? "未知错误").slice(0, 400)}`);
}

async function repl(args: Record<string, any>): Promise<number> {
  const { runtime, core, close, sessionId } = await openSession({
    cwd: args.cwd,
    sessionId: args.resume,
    fullAuto: args.fullAuto,
    initialSpec: args.team ? JSON.parse(readFileSync(args.team, "utf8")) : undefined,
    coreBin,
  });
  console.log(`session: ${sessionId} (Ctrl-D to exit)`);
  await runtime.start();
  let cursor = 0;
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  try {
    for await (const line of rl) {
      if (!line.trim()) continue;
      const receipt = await runtime.userMessage(line);
      console.log(`  [input received: ${(receipt.result as any)?.goal_id ?? ""}]`);
      await runtime.settle(600);
      const state = await core.call("state", { session_id: sessionId, after_sequence: cursor });
      for (const event of state.events) {
        cursor = event.sequence;
        printEvent(event);
      }
    }
  } finally {
    rl.close();
    await close();
  }
  return 0;
}

async function runTui(args: Record<string, any>): Promise<number> {
  const { runTuiApp } = await import("./tui/app.ts");
  await runTuiApp({
    cwd: args.cwd,
    resume: args.resume,
    fullAuto: args.fullAuto,
    team: args.team,
    coreBin,
  });
  return 0;
}

async function main(argv: string[]): Promise<number> {
  const args: Record<string, any> = {};
  let command: string | null = null;
  let positional: string | null = null;
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--cwd") args.cwd = argv[++i];
    else if (a === "--resume") args.resume = argv[++i];
    else if (a === "--full-auto") args.fullAuto = true;
    else if (a === "--team") args.team = argv[++i];
    else if (a === "--plain") args.plain = true;
    else if (["doctor", "validate", "sessions", "version"].includes(a)) command = a;
    else if (a === "-v" || a === "--verbose") args.verbose = true;
    else if (!a.startsWith("-")) positional = a;
  }
  switch (command) {
    case "doctor":
      return doctor();
    case "validate":
      return validateSpec(positional!);
    case "sessions":
      return listSessionsCmd(Boolean(args.verbose));
    case "version":
      console.log(JSON.stringify({ version: VERSION, core: "teamagents-core", config: userConfigPath() }, null, 2));
      return 0;
    default:
      return args.plain ? repl(args) : runTui(args);
  }
}

main(process.argv.slice(2))
  .then((code) => process.exit(code))
  .catch((e) => {
    console.error(e);
    process.exit(1);
  });
