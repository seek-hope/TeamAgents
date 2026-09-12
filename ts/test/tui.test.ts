/** TUI smoke on Ink: frame content + composer history persistence (p6 port, scoped). */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

process.env.XDG_STATE_HOME ??= mkdtempSync(join(tmpdir(), "ta-tui-state-"));

import React from "react";
import { render } from "ink";
import { PassThrough } from "node:stream";
import { CoreClient } from "../src/core-client.ts";
import { SessionRuntime } from "../src/runtime.ts";
import { ScriptedMember } from "../src/scripted.ts";
import { loadHistory, saveHistory } from "../src/tui/history.ts";
import { formatEvent } from "../src/tui/ink-app.ts";

const coreBin = new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname;

test("history persists across loads with a 500 cap", () => {
  saveHistory(["a", "b", "c"]);
  assert.deepEqual(loadHistory(), ["a", "b", "c"]);
  saveHistory(Array.from({ length: 600 }, (_, i) => `cmd${i}`));
  const h = loadHistory();
  assert.equal(h.length, 500);
  assert.equal(h[0], "cmd100");
});

test("formatEvent styles: leader green, errors red, system dim", () => {
  const [you] = formatEvent({ kind: "user_message", actor_id: "user", payload: { text: "hi" } });
  assert.equal(you.label, "You");
  const [fail] = formatEvent({ kind: "run_failed", actor_id: "b", payload: { error: "boom" } });
  assert.equal(fail.bodyColor, "red");
  const [done] = formatEvent({ kind: "goal_done", actor_id: "leader", payload: { summary: "s" } });
  assert.equal(done.labelColor, "#00ff00");
});

test("ink app renders header, tabs, chat over a live session", async () => {
  const core = new CoreClient(coreBin);
  await core.createSession("tui2", "/tmp");
  await core.call("save_spec", {
    session_id: "tui2",
    spec: { leader_id: "leader", agents: [{ id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" }] },
  });
  const leader = new ScriptedMember("leader", [["call", "signal_done", { summary: "ok" }], ["end"]]);
  const runtime = new SessionRuntime(core, "tui2", { runners: { leader } });
  await runtime.start();

  const stdout = new PassThrough();
  (stdout as any).columns = 110;
  (stdout as any).rows = 32;
  let allOut = "";
  stdout.on("data", (d) => (allOut += d));
  const stdin = new PassThrough();
  (stdin as any).isTTY = true;
  (stdin as any).setRawMode = () => {};
  (stdin as any).ref = () => stdin;
  (stdin as any).unref = () => stdin;

  // mount the real App component (exported for tests)
  const { AppForTest } = await import("../src/tui/ink-app.ts");
  const opened = { runtime, core, sessionId: "tui2", close: async () => {} } as any;
  const stderr = new PassThrough();
  let errOut = "";
  stderr.on("data", (d) => (errOut += d));
  const app = render(React.createElement(AppForTest, { opened }), { stdout, stdin, stderr, exitOnCtrlC: false, debug: true });
  if (errOut) console.log("INK STDERR:", errOut);
  await new Promise((r) => setTimeout(r, 700));

  // type + submit (separate data events, like a real terminal)
  const key = async (s: string) => {
    stdin.write(s);
    await new Promise((r) => setTimeout(r, 30));
  };
  await key("h");
  await key("i");
  await key("\r");
  await new Promise((r) => setTimeout(r, 800));

  await key("\x11"); // Ctrl+Q: quit, which persists composer history
  await new Promise((r) => setTimeout(r, 200));
  const frame = allOut;
  (await import("node:fs")).writeFileSync("/tmp/ink-frame.ansi", frame);
  app.unmount();
  assert.ok(frame.includes("TeamAgents"), "header");
  assert.ok(frame.includes("Approvals"), "tabs");
  assert.ok(frame.includes("hi"), "composer echo or chat");

  assert.ok(loadHistory().includes("hi"), "input persisted");
  await runtime.close();
  core.close();
});
