/** CodexRunner adapter tests against a fake app-server (tests/test_p5_codex_adapter.py port). */
import { test } from "node:test";
import assert from "node:assert/strict";
import { CoreClient } from "../src/core-client.ts";
import { CodexRunner } from "../src/codex-runner.ts";
import { ApprovalGate, PermissionPolicy } from "../src/gateway.ts";

const coreBin = new URL("../../core/target/debug/teamagents-core", import.meta.url).pathname;
const fakeBin = new URL("./fake-codex.mjs", import.meta.url).pathname;

async function setup(sessionId: string, mode: string) {
  const core = new CoreClient(coreBin);
  await core.createSession(sessionId, "/tmp");
  await core.call("save_spec", {
    session_id: sessionId,
    spec: { leader_id: "leader", agents: [
      { id: "leader", name: "L", role: "leader", runtime_kind: "deepagents", model_profile: "m" },
      { id: "cx", name: "C", role: "worker", runtime_kind: "codex", model_profile: "m" },
    ] },
  });
  const approvals = new ApprovalGate(core, sessionId, new PermissionPolicy());
  const runner = new CodexRunner({
    agent: { id: "cx", name: "C" },
    sessionId,
    workdir: "/tmp",
    approvals,
    core,
    codexBin: fakeBin,
    env: { FAKE_CODEX_MODE: mode },
    effort: null,
  });
  return { core, runner };
}

test("codex simple turn completes and persists the thread", async () => {
  const { core, runner } = await setup("cx1", "simple");
  try {
    const run = {
      run_id: "run_cx1", session_id: "cx1", task_id: null, goal_id: null, agent_id: "cx",
      config_revision: 1, topology_revision: 1, status: "QUEUED", input_delivery_ids: [],
      context_ref: null, external_turn_id: null, cancel_requested: false, waiting_on: [],
      created_at: 0, updated_at: 0,
    } as any;
    const view = { agent_id: "cx", assignment: [], inbox_delta: [], permitted_shared_delta: [],
                   relevant_topology: { revision: 1 }, capabilities: [], delivery_ids: [], batch_no: 0 } as any;
    const outcome = await runner.startOrResume(run, view, null, null);
    assert.equal(outcome.status, "COMPLETED");
    assert.ok(outcome.reply_text?.includes("fake work done"));
    const { thread_id } = await core.call("get_codex_thread", { session_id: "cx1", agent_id: "cx" });
    assert.ok(thread_id, "thread id persisted before the turn");
    assert.equal(runner.queryState("run_cx1"), "COMPLETED");
  } finally {
    await runner.close();
    core.close();
  }
});

test("codex approval flow: parks, user decides, turn resumes", async () => {
  const { core, runner } = await setup("cx2", "approval");
  try {
    const run = {
      run_id: "run_cx2", session_id: "cx2", task_id: null, goal_id: null, agent_id: "cx",
      config_revision: 1, topology_revision: 1, status: "QUEUED", input_delivery_ids: [],
      context_ref: null, external_turn_id: null, cancel_requested: false, waiting_on: [],
      created_at: 0, updated_at: 0,
    } as any;
    const view = { agent_id: "cx", assignment: [], inbox_delta: [], permitted_shared_delta: [],
                   relevant_topology: { revision: 1 }, capabilities: [], delivery_ids: [], batch_no: 0 } as any;
    const started = runner.startOrResume(run, view, null, null);
    // wait for the approval request to land in the core
    const deadline = Date.now() + 5000;
    let pending: any[] = [];
    while (Date.now() < deadline) {
      const st = await core.call("state", { session_id: "cx2" });
      pending = st.pending_approvals;
      if (pending.length) break;
      await new Promise((r) => setTimeout(r, 20));
    }
    assert.equal(pending.length, 1, "approval request recorded");
    // user approves once → runner resolves → fake server completes the turn
    assert.ok(runner.resolveApproval(pending[0].approval_id, "once"));
    const outcome = await started;
    assert.equal(outcome.status, "COMPLETED");
    assert.ok(outcome.reply_text?.includes("approval=accept"));
  } finally {
    await runner.close();
    core.close();
  }
});

test("codex interrupt: slow turn stops with confirmed status", async () => {
  const { core, runner } = await setup("cx3", "slow");
  try {
    const run = {
      run_id: "run_cx3", session_id: "cx3", task_id: null, goal_id: null, agent_id: "cx",
      config_revision: 1, topology_revision: 1, status: "QUEUED", input_delivery_ids: [],
      context_ref: null, external_turn_id: null, cancel_requested: false, waiting_on: [],
      created_at: 0, updated_at: 0,
    } as any;
    const view = { agent_id: "cx", assignment: [], inbox_delta: [], permitted_shared_delta: [],
                   relevant_topology: { revision: 1 }, capabilities: [], delivery_ids: [], batch_no: 0 } as any;
    const started = runner.startOrResume(run, view, null, null);
    await new Promise((r) => setTimeout(r, 200));
    const status = await runner.requestInterrupt("run_cx3");
    assert.equal(status, "CANCELLED");
    const outcome = await started;
    assert.equal(outcome.status, "CANCELLED");
  } finally {
    await runner.close();
    core.close();
  }
});
