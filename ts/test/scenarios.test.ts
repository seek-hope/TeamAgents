/** T2–T5 scenario ports (tests/test_t2..t5_*.py). */
import { test } from "node:test";
import assert from "node:assert/strict";
import { CoreClient } from "../src/core-client.ts";
import { SessionRuntime } from "../src/runtime.ts";
import { ScriptedMember, Barrier, type Step } from "../src/scripted.ts";

const coreBin = new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname;

function member(id: string, role = "worker") {
  return { id, name: id, role, runtime_kind: "deepagents", model_profile: "m" };
}
const msg = (s: string, t: string[]) => ({ source: s, targets: t, mode: "message" });
const task = (s: string, t: string[]) => ({ source: s, targets: t, mode: "task" });

async function setup(sessionId: string, spec: any, members: Record<string, ScriptedMember>) {
  const core = new CoreClient(coreBin);
  await core.createSession(sessionId, "/tmp");
  await core.call("save_spec", { session_id: sessionId, spec });
  const rt = new SessionRuntime(core, sessionId, { runners: members });
  await rt.start();
  return { core, rt };
}

test("t2: parallel members, barrier-proven; supplement handled mid-run", async () => {
  const spec = {
    leader_id: "leader",
    agents: [member("leader", "leader"), member("b"), member("c")],
    channels: [task("leader", ["b", "c"]), msg("b", ["leader"]), msg("c", ["leader"])],
  };
  const barriers: Record<string, Barrier> = {};
  const members = {
    leader: new ScriptedMember("leader", [
      ["call", "assign_task", { assignee: "b", description: "slow job B" }],
      ["call", "assign_task", { assignee: "c", description: "slow job C" }],
      ["call", "wait_for_tasks", { task_ids: ["$r0.result.task_id", "$r1.result.task_id"] }],
      ["wait"],
      ["inbox"],
      ["call", "wait_for_tasks", { task_ids: ["$r0.result.task_id", "$r1.result.task_id"] }],
      ["wait"],
      ["inbox"],
      ["call", "signal_done", { summary: "both done" }],
      ["end"],
    ], barriers),
    b: new ScriptedMember("b", [
      ["barrier", "both-started"],
      ["sleep", 0.6],
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id", result_refs: ["b.out"] }],
      ["end"],
    ], barriers),
    c: new ScriptedMember("c", [
      ["barrier", "both-started"],
      ["sleep", 0.6],
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id", result_refs: ["c.out"] }],
      ["end"],
    ], barriers),
  };
  const { core, rt } = await setup("t2", spec, members);
  try {
    await rt.userMessage("run two jobs in parallel");
    // while B/C work, a supplement must reach the Leader without waiting for them
    await new Promise((r) => setTimeout(r, 200));
    await rt.userMessage("by the way, also check the logs", { supplement: true });
    const deadline = Date.now() + 5000;
    while (!members.leader.observedInbox.some((i) => i.kind === "user_message")) {
      assert.ok(Date.now() < deadline, "leader never processed the supplement");
      await new Promise((r) => setTimeout(r, 20));
    }
    const st = await rt.state();
    const byAgent = Object.fromEntries(st.runs.map((r) => [r.agent_id, r]));
    assert.equal(byAgent.b.status, "RUNNING");
    assert.equal(byAgent.c.status, "RUNNING");

    assert.ok(await rt.settle(10));
    const final = await rt.state();
    const tasks = Object.fromEntries(final.tasks.map((t) => [t.assignee, t]));
    assert.equal(tasks.b.status, "SUCCEEDED");
    assert.equal(tasks.c.status, "SUCCEEDED");
    assert.equal(final.session?.goal_state, "done");
  } finally {
    await rt.close();
    core.close();
  }
});

test("t3: channel enforcement and exactly-once delivery", async () => {
  const spec = {
    leader_id: "leader",
    agents: [member("leader", "leader"), member("b"), member("c"), member("d")],
    channels: [task("leader", ["b", "c"]), msg("b", ["c"]), msg("c", ["b"]), msg("b", ["leader"]), msg("c", ["leader"])],
  };
  const members = {
    leader: new ScriptedMember("leader", [
      ["call", "assign_task", { assignee: "b", description: "discuss" }],
      ["call", "wait_for_tasks", { task_ids: ["$r0.result.task_id"] }],
      ["wait"],
      ["call", "signal_done", {}],
      ["end"],
    ]),
    b: new ScriptedMember("b", [
      ["call", "send_message", { target: "c", text: "ping" }],
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id" }],
      ["end"],
      ["inbox"],
      ["end"],
    ]),
    c: new ScriptedMember("c", [
      ["inbox"],
      ["call", "send_message", { target: "b", text: "pong" }],
      ["call", "send_message", { target: "d", text: "leak" }],
      ["end"],
    ]),
    d: new ScriptedMember("d", []),
  };
  const { core, rt } = await setup("t3", spec, members);
  try {
    await rt.userMessage("let b and c discuss");
    assert.ok(await rt.settle(10));
    const cMsgs = members.c.observedInbox.filter((i) => i.kind === "message");
    assert.deepEqual(cMsgs.map((m) => m.payload.text), ["ping"]);
    const bMsgs = members.b.observedInbox.filter((i) => i.kind === "message");
    assert.deepEqual(bMsgs.map((m) => m.payload.text), ["pong"]);
    const leak = members.c.results.filter((r) => !r.ok && (r.error ?? "").includes("not allowed to message"));
    assert.ok(leak.length > 0);
  } finally {
    await rt.close();
    core.close();
  }
});

test("t4: observer scoped events, no extra rights", async () => {
  const spec = {
    leader_id: "leader",
    agents: [member("leader", "leader"), member("b"), member("watch")],
    channels: [task("leader", ["b"]), msg("b", ["leader"])],
    observers: [
      { agent_id: "watch", subjects: ["b"], event_types: ["task_completed"], payload_scope: "status", wake_policy: "on_event", capabilities: [] },
    ],
  };
  const members = {
    leader: new ScriptedMember("leader", [
      ["call", "assign_task", { assignee: "b", description: "secret work" }],
      ["call", "wait_for_tasks", { task_ids: ["$r0.result.task_id"] }],
      ["wait"],
      ["call", "signal_done", {}],
      ["end"],
    ]),
    b: new ScriptedMember("b", [
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id", result_refs: ["private/out.txt"], summary: "PRIVATE DETAILS" }],
      ["end"],
    ]),
    watch: new ScriptedMember("watch", [["inbox"], ["end"]]),
  };
  const { core, rt } = await setup("t4", spec, members);
  try {
    await rt.userMessage("do the secret work");
    assert.ok(await rt.settle(10));
    const observed = members.watch.observedInbox;
    assert.ok(observed.length > 0, "on_event observer must receive matching events");
    for (const item of observed) assert.equal(item.kind, "task_completed");
    const payload = observed[0].payload;
    assert.equal(payload.status, "SUCCEEDED");
    assert.ok(!JSON.stringify(payload).includes("PRIVATE DETAILS"));
    assert.ok(!JSON.stringify(payload).includes("private/out.txt"));

    const send = await rt.submit({ action_id: "w1", session_id: "t4", actor_id: "watch", kind: "send_message", payload: { target: "b", text: "hi" } });
    assert.ok(!send.ok && send.error!.includes("not allowed to message"));
    const patch = await rt.submit({
      action_id: "w2",
      session_id: "t4",
      actor_id: "watch",
      kind: "apply_topology_patch",
      payload: { operations: [{ op: "remove_agent", agent_id: "b" }], base_revision: 1 },
    });
    assert.ok(!patch.ok && patch.error!.includes("Leader"));
  } finally {
    await rt.close();
    core.close();
  }
});

test("t5: shared space permissions and discovery", async () => {
  const spec = {
    leader_id: "leader",
    agents: [member("leader", "leader"), member("b"), member("c"), member("d")],
    channels: [task("leader", ["b", "c"])],
    shared_spaces: [{ id: "main", readers: ["leader", "b", "c"], writers: ["leader", "b"] }],
  };
  const members = {
    leader: new ScriptedMember("leader", [
      ["call", "assign_task", { assignee: "b", description: "publish findings" }],
      ["call", "assign_task", { assignee: "c", description: "use findings" }],
      ["call", "wait_for_tasks", { task_ids: ["$r0.result.task_id", "$r1.result.task_id"] }],
      ["wait"],
      ["call", "signal_done", {}],
      ["end"],
    ]),
    b: new ScriptedMember("b", [
      ["call", "publish_shared", { space_id: "main", kind: "finding", content: "the cache is cold", ref: "artifacts/trace-1.bin" }],
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id" }],
      ["end"],
    ]),
    c: new ScriptedMember("c", [
      ["inbox"],
      ["call", "read_shared", { space_id: "main" }],
      ["call", "list_shared", {}],
      ["call", "complete_task", { task_id: "$inbox0.payload.task_id" }],
      ["end"],
    ]),
    d: new ScriptedMember("d", []),
  };
  const { core, rt } = await setup("t5", spec, members);
  try {
    await rt.userMessage("share and reuse findings");
    assert.ok(await rt.settle(10));
    const read = members.c.results.find((r) => r.kind === "read_shared");
    assert.ok(read?.ok);
    assert.equal(read.result.entries[0].content, "the cache is cold");
    assert.equal(read.result.entries[0].ref, "artifacts/trace-1.bin");

    const deniedWrite = await rt.submit({ action_id: "d1", session_id: "t5", actor_id: "d", kind: "publish_shared", payload: { space_id: "main", content: "nope" } });
    assert.ok(!deniedWrite.ok && deniedWrite.error!.includes("write access"));
    const deniedRead = await rt.submit({ action_id: "d2", session_id: "t5", actor_id: "d", kind: "read_shared", payload: { space_id: "main" } });
    assert.ok(!deniedRead.ok && deniedRead.error!.includes("read access"));
    const listed = await rt.submit({ action_id: "d3", session_id: "t5", actor_id: "d", kind: "list_shared", payload: {} });
    assert.ok(listed.ok);
    assert.deepEqual(listed.result.spaces, []);
  } finally {
    await rt.close();
    core.close();
  }
});
