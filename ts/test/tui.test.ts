/** TUI smoke: headless render + composer history persistence (p6 port, scoped). */
import { test } from "node:test";
import assert from "node:assert/strict";
import { PassThrough, Readable } from "node:stream";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

process.env.XDG_STATE_HOME ??= mkdtempSync(join(tmpdir(), "ta-tui-state-"));

import { CoreClient } from "../src/core-client.ts";
import { SessionRuntime } from "../src/runtime.ts";
import { ScriptedMember } from "../src/scripted.ts";
import { TuiApp, loadHistory, saveHistory } from "../src/tui/app.ts";

const coreBin = new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname;

test("history persists across loads with a 500 cap", () => {
  saveHistory(["a", "b", "c"]);
  assert.deepEqual(loadHistory(), ["a", "b", "c"]);
  saveHistory(Array.from({ length: 600 }, (_, i) => `cmd${i}`));
  const h = loadHistory();
  assert.equal(h.length, 500);
  assert.equal(h[0], "cmd100");
});

test("tui renders header, tabs, chat and submits input", async () => {
  const core = new CoreClient(coreBin);
  await core.createSession("tui1", "/tmp");
  await core.call("save_spec", {
    session_id: "tui1",
    spec: { leader_id: "leader", agents: [{ id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" }] },
  });
  const leader = new ScriptedMember("leader", [["call", "signal_done", { summary: "ok" }], ["end"]]);
  const runtime = new SessionRuntime(core, "tui1", { runners: { leader } });
  await runtime.start();

  const out = new PassThrough();
  (out as any).columns = 100;
  (out as any).rows = 30;
  let screen = "";
  out.on("data", (d) => (screen += d));
  const input = new PassThrough();

  const app = new TuiApp({ runtime, core, sessionId: "tui1", close: async () => {} } as any, out, input as any);
  const main = (app as any).main();
  await new Promise((r) => setTimeout(r, 400));

  // type and submit a message
  input.emit("keypress", "h", { name: "h" });
  input.emit("keypress", "i", { name: "i" });
  input.emit("keypress", undefined, { name: "return" });
  await new Promise((r) => setTimeout(r, 800));
  input.emit("keypress", undefined, { name: "q", ctrl: true });
  await main;

  assert.ok(screen.includes("TeamAgents"), "header rendered");
  assert.ok(screen.includes("team") && screen.includes("approvals"), "tabs rendered");
  assert.ok(screen.includes("you> hi"), "composer echo");
  assert.ok(screen.includes("[goal done]") || screen.includes("task"), "chat events rendered");

  // submitted input landed in persisted history
  const h = loadHistory();
  assert.ok(h.includes("hi"), "input persisted to history");
  await runtime.close();
  core.close();
});
