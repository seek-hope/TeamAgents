"""P3: real Deep Agents members — delegation, approval interrupt/resume, denial,
step limit and mid-turn injection, all through the production runtime."""

from __future__ import annotations

import asyncio
import json

import pytest

from conftest import Harness, leader, member, msg_channel, spec_of, task_channel
from scripted_model import ScriptedChatModel, ai_text, ai_tool, find_task_id, last_tool_result
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.models import PermissionMode, TeamAction
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store
from teamagents.models import UserConfig, ModelProfile


def build_runtime(tmp_path, spec, models: dict[str, ScriptedChatModel],
                  workdir=None, policy: PermissionPolicy | None = None,
                  session_id: str = "s1") -> SessionRuntime:
    workdir = workdir or (tmp_path / "work")
    workdir.mkdir(parents=True, exist_ok=True)
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir(exist_ok=True)
    store = Store(tmp_path / f"{session_id}.db")
    if store.get_session(session_id) is None:
        store.create_session(session_id, str(workdir), "approved_scope")
        store.save_team_spec(session_id, spec)
        for agent in spec.agents:
            store.ensure_agent(session_id, agent.id)
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    approvals = ApprovalGate(store, session_id, policy or PermissionPolicy())
    from langgraph.checkpoint.memory import InMemorySaver
    runners = {agent.id: DeepAgentsRunner(
        agent=agent, catalog=catalog, session_id=session_id, workdir=workdir,
        artifacts_dir=artifacts, checkpointer=InMemorySaver(), approvals=approvals,
        model_override=models[agent.id]) for agent in spec.agents}
    return SessionRuntime(store, session_id, catalog, runners=runners,
                          approvals=approvals)


async def test_deepagents_delegation_end_to_end(tmp_path):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])

    def leader_finish(messages):
        task_id = last_tool_result(messages).get("result", {}).get("task_id")
        return ai_tool("wait_for_tasks", {"task_ids": [task_id]})

    leader_model = ScriptedChatModel(script=[
        ai_tool("assign_task", {"assignee": "b", "description": "write report.md",
                                "acceptance": "file exists"}),
        leader_finish,
        ai_tool("signal_done", {"summary": "report delivered"}),
        ai_text("all done"),
    ])

    def b_complete(messages):
        text = json.dumps([m.content for m in messages if hasattr(m, "content")])
        return ai_tool("complete_task", {"task_id": find_task_id(text),
                                         "summary": "wrote it",
                                         "result_refs": ["report.md"]})

    b_model = ScriptedChatModel(script=[b_complete, ai_text("done")])
    rt = build_runtime(tmp_path, spec, {"leader": leader_model, "b": b_model})
    h = Harness(rt, {})
    await h.start()
    try:
        receipt = rt.user_message("please produce the report")
        assert receipt.ok
        assert await rt.settle(15), "session did not settle"
        tasks = rt.store.tasks_for_session("s1")
        assert len(tasks) == 1 and tasks[0].status == "SUCCEEDED"
        assert tasks[0].result_refs == ["report.md"]
        assert rt.store.get_session("s1")["goal_state"] == "done"
        # both members used the real graph: the leader's tools were bound
        assert "assign_task" in leader_model.tool_names
        assert "signal_done" in leader_model.tool_names
        assert "shell" in b_model.tool_names
    finally:
        await rt.close()
        rt.store.close()


async def test_deepagents_shell_runs_isolated_inside_workdir(tmp_path):
    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[
        ai_tool("shell", {"command": "echo hi > made.txt && cat made.txt"}),
        ai_tool("shell", {"command": "ls /home/rimuru 2>&1 | head -1"}),
        ai_tool("signal_done", {"summary": "ok"}),
        ai_text("done"),
    ])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("run commands")
        assert await rt.settle(15)
        assert (tmp_path / "work" / "made.txt").read_text().strip() == "hi"
        results = [json.loads(m.content) for m in model.calls[-1]
                   if type(m).__name__ == "ToolMessage" and m.content.startswith("{")]
        assert any(r.get("exit_code") == 0 and "hi" in r.get("output", "") for r in results)
        assert any("No such file" in r.get("output", "") for r in results), \
            "outside the workdir must stay unreachable"
    finally:
        await rt.close()
        rt.store.close()


async def test_deepagents_approval_interrupt_and_resume(tmp_path):
    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[
        ai_tool("shell", {"command": "timeout 2 bash -c 'echo > /dev/tcp/1.1.1.1/80'",
                          "network": True}),
        ai_tool("signal_done", {"summary": "network attempt finished"}),
        ai_text("done"),
    ])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("fetch something")
        deadline = asyncio.get_event_loop().time() + 15
        while True:
            pending = rt.store.pending_approvals("s1")
            if pending and rt.store.get_run(pending[0].run_id).status == "WAITING_APPROVAL":
                break
            assert asyncio.get_event_loop().time() < deadline, "no parked approval"
            await asyncio.sleep(0.02)
        approval = rt.store.pending_approvals("s1")[0]
        assert approval.requested_scope["tool"] == "shell"
        run = rt.store.get_run(approval.run_id)
        assert run.status == "WAITING_APPROVAL"

        decide = rt.submit(TeamAction(action_id="a1", session_id="s1", actor_id="user",
                                      kind="approval_decision",
                                      payload={"approval_id": approval.approval_id,
                                               "decision": "once"}))
        assert decide.ok, decide.error
        assert await rt.settle(20)
        assert rt.store.get_approval(approval.approval_id).status == "EXPIRED", \
            "a once-approval is consumed by use"
        kinds = [e["kind"] for e in rt.store.events("s1")]
        assert "approval_requested" in kinds and "approval_decided" in kinds
        assert rt.store.get_session("s1")["goal_state"] == "done"
    finally:
        await rt.close()
        rt.store.close()


async def test_deepagents_denied_approval_blocks_operation_and_turn_continues(tmp_path):
    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[
        ai_tool("shell", {"command": "echo x > /tmp/should-not-exist",
                          "network": True}),
        ai_tool("signal_done", {"summary": "gave up on network"}),
        ai_text("done"),
    ])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("do a network thing")
        deadline = asyncio.get_event_loop().time() + 15
        while True:
            pending = rt.store.pending_approvals("s1")
            if pending and rt.store.get_run(pending[0].run_id).status == "WAITING_APPROVAL":
                break
            assert asyncio.get_event_loop().time() < deadline
            await asyncio.sleep(0.02)
        approval = rt.store.pending_approvals("s1")[0]
        rt.submit(TeamAction(action_id="a2", session_id="s1", actor_id="user",
                             kind="approval_decision",
                             payload={"approval_id": approval.approval_id,
                                      "decision": "deny"}))
        assert await rt.settle(20)
        denied = [m for m in model.calls[-1] if type(m).__name__ == "ToolMessage"]
        assert any("denied" in str(m.content).lower() for m in denied)
        assert not (tmp_path / "work" / "should-not-exist").exists()
    finally:
        await rt.close()
        rt.store.close()


async def test_deepagents_full_auto_skips_approval(tmp_path):
    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[
        ai_tool("shell", {"command": "echo ok", "network": True}),
        ai_tool("signal_done", {}),
        ai_text("done"),
    ])
    policy = PermissionPolicy(mode=PermissionMode.FULL_AUTO)
    rt = build_runtime(tmp_path, spec, {"leader": model}, policy=policy)
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("run it")
        assert await rt.settle(20)
        assert rt.store.pending_approvals("s1") == []
        assert not [e for e in rt.store.events("s1") if e["kind"] == "approval_requested"]
    finally:
        await rt.close()
        rt.store.close()


async def test_deepagents_mid_turn_injection_reaches_next_model_call(tmp_path):
    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[
        ai_tool("shell", {"command": "sleep 0.4"}),
        ai_tool("signal_done", {"summary": "after injection"}),
        ai_text("done"),
    ])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("start")
        deadline = asyncio.get_event_loop().time() + 10
        while not rt.store.runs_for_session("s1", ["RUNNING"]):
            assert asyncio.get_event_loop().time() < deadline
            await asyncio.sleep(0.01)
        run = rt.store.runs_for_session("s1", ["RUNNING"])[0]
        rt.submit(TeamAction(action_id="supp-1", session_id="s1", actor_id="user",
                             kind="user_supplement", payload={"text": "MIDTURN-MARKER"}))
        assert await rt.settle(20)
        seen = [str(m.content) for call in model.calls for m in call]
        assert any("MIDTURN-MARKER" in s for s in seen)
        run = rt.store.get_run(run.run_id)
        assert run.status == "COMPLETED"
    finally:
        await rt.close()
        rt.store.close()


async def test_deepagents_model_step_limit_reports_limit_reached(tmp_path):
    spec = spec_of(leader(), channels=[])
    model = ScriptedChatModel(script=[
        ai_tool("shell", {"command": "echo tick"}),
    ])
    rt = build_runtime(tmp_path, spec, {"leader": model})
    h = Harness(rt, {})
    await h.start()
    try:
        # shrink the budget so the loop hits it quickly
        spec_obj = rt.store.load_team_spec("s1")
        spec_obj.limits.max_model_steps_per_turn = 3
        rt.store.save_team_spec("s1", spec_obj)
        rt.user_message("spin forever")
        assert await rt.settle(30)
        kinds = [e["kind"] for e in rt.store.events("s1")]
        assert "limit_reached" in kinds
        run = rt.store.runs_for_session("s1")[0]
        assert run.status == "FAILED"
    finally:
        await rt.close()
        rt.store.close()
