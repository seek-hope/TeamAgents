/**
 * T1 delegation end-to-end against the real Rust core over stdio,
 * mirrored from tests/test_t1_delegation.py.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { CoreClient } from "../src/core-client.ts";
import { SessionRuntime } from "../src/runtime.ts";
import { ScriptedMember } from "../src/scripted.ts";

const coreBin = process.env.TEAMAGENTS_CORE ?? "../core/target/debug/teamagents-core";

const SPEC = {
  leader_id: "leader",
  agents: [
    { id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" },
    { id: "b", name: "B", role: "worker", runtime_kind: "deepagents", model_profile: "m" },
  ],
  channels: [
    { source: "leader", targets: ["b"], mode: "task" },
    { source: "b", targets: ["leader"], mode: "message" },
  ],
};

test("t1: delegation and summary, full lifecycle", async () => {
  const core = new CoreClient(coreBin);
  try {
    await core.createSession("s1", "/tmp");
    await core.call("save_spec", { session_id: "s1", spec: SPEC });

    const leader = new ScriptedMember("leader", [
      ["call", "assign_task", { assignee: "b", description: "write the report", acceptance: "report.md exists" }],
      ["call", "wait_for_tasks", { task_ids: ["$r0.result.task_id"] }],
      ["wait"],
      ["call", "signal_done", { summary: "report delivered" }],
      ["end"],
    ]);
    const b = new ScriptedMember("b", [
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id", result_refs: ["artifacts/report.md"], summary: "wrote report" }],
      ["end"],
    ]);
    const rt = new SessionRuntime(core, "s1", { runners: { leader, b } });
    await rt.start();
    const receipt = await rt.userMessage("please produce the report");
    assert.ok(receipt.ok);
    assert.ok(await rt.settle(10), "runtime settles");

    const state = await rt.state();
    assert.equal(state.tasks.length, 1);
    const task = state.tasks[0];
    assert.equal(task.status, "SUCCEEDED");
    assert.deepEqual(task.result_refs, ["artifacts/report.md"]);
    assert.equal(task.requester, "leader");
    assert.equal(task.assignee, "b");

    const kinds = state.events.map((e) => e.kind);
    for (const k of ["task_created", "task_started", "task_completed", "goal_done"]) assert.ok(kinds.includes(k), k);

    // result receipt goes to the requester (Leader) via inbox delivery
    const done = state.events.find((e) => e.kind === "task_completed");
    assert.ok(JSON.stringify(done.audience).includes("leader"));

    assert.equal(state.session?.goal_state, "done");
    await rt.close();
  } finally {
    core.close();
  }
});

test("t9 baseline: leader alone executes and keeps talking", async () => {
  const core = new CoreClient(coreBin);
  try {
    await core.createSession("s2", "/tmp");
    await core.call("save_spec", {
      session_id: "s2",
      spec: { leader_id: "leader", agents: [{ id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" }] },
    });
    const leader = new ScriptedMember("leader", [["call", "signal_done", { summary: "answered directly" }], ["end"]]);
    const rt = new SessionRuntime(core, "s2", { runners: { leader } });
    await rt.start();
    await rt.userMessage("say hello");
    assert.ok(await rt.settle(10));
    assert.equal((await rt.state()).session?.goal_state, "done");

    // second conversation: new goal, same team
    leader.script = [["call", "signal_done", { summary: "second" }], ["end"]];
    leader.cursor = 0;
    await rt.userMessage("second question");
    assert.ok(await rt.settle(10));
    assert.equal((await rt.state()).session?.goal_state, "done");
    await rt.close();
  } finally {
    core.close();
  }
});
