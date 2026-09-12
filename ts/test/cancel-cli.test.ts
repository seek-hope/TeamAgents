/** Cancel/pause paths (test_p2_* port, scoped) + CLI smoke. */
import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { CoreClient } from "../src/core-client.ts";
import { SessionRuntime } from "../src/runtime.ts";
import { ScriptedMember } from "../src/scripted.ts";

const coreBin = new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname;

test("p2: cancel_run stops a slow member turn", async () => {
  const core = new CoreClient(coreBin);
  await core.createSession("p2", "/tmp");
  await core.call("save_spec", {
    session_id: "p2",
    spec: { leader_id: "leader", agents: [{ id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" }] },
  });
  const leader = new ScriptedMember("leader", [["sleep", 30], ["end"]]);
  const rt = new SessionRuntime(core, "p2", { runners: { leader } });
  await rt.start();
  await rt.userMessage("long work");
  await new Promise((r) => setTimeout(r, 300));
  let st = await rt.state();
  const run = st.runs.find((r) => r.agent_id === "leader");
  assert.equal(run?.status, "RUNNING");
  const receipt = await rt.submit({ action_id: "cx1", session_id: "p2", actor_id: "user", kind: "cancel_run", payload: { run_id: run!.run_id } });
  assert.ok(receipt.ok);
  assert.ok(await rt.settle(10));
  st = await rt.state();
  assert.equal(st.runs.find((r) => r.run_id === run!.run_id)?.status, "CANCELLED");
  await rt.close();
  core.close();
});

test("p2: pause then resume by user input", async () => {
  const core = new CoreClient(coreBin);
  await core.createSession("p2b", "/tmp");
  await core.call("save_spec", {
    session_id: "p2b",
    spec: { leader_id: "leader", agents: [{ id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" }] },
  });
  const leader = new ScriptedMember("leader", [["call", "signal_done", { summary: "s" }], ["end"]]);
  const rt = new SessionRuntime(core, "p2b", { runners: { leader } });
  await rt.start();
  const r = await rt.submit({ action_id: "ps1", session_id: "p2b", actor_id: "user", kind: "pause_session", payload: {} });
  assert.ok(r.ok);
  assert.equal((await rt.state()).session?.status, "PAUSED");
  await rt.userMessage("resume please");
  assert.ok(await rt.settle(10));
  const st = await rt.state();
  assert.equal(st.session?.status, "ACTIVE");
  assert.equal(st.session?.goal_state, "done");
  await rt.close();
  core.close();
});

test("cli: version/validate/sessions", () => {
  const out = execFileSync("node", ["src/cli.ts", "version"], { encoding: "utf8" });
  assert.ok(out.includes("0.1.0-ts"));
  const spec = JSON.stringify({ leader_id: "leader", agents: [{ id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "leader_main" }] });
  execFileSync("node", ["-e", `require("fs").writeFileSync("/tmp/ta-spec.json", ${JSON.stringify(spec)})`]);
  const ok = execFileSync("node", ["src/cli.ts", "validate", "/tmp/ta-spec.json"], { encoding: "utf8" });
  assert.ok(ok.includes("ok:"));
  const sess = execFileSync("node", ["src/cli.ts", "sessions"], { encoding: "utf8" });
  assert.ok(sess.includes("会话"));
});
