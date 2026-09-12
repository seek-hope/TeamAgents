"""P5/T17: Codex execution members — lifecycle, progress, approvals, cancel
and recovery mapping, driven through the production runtime."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path

import pytest

from conftest import Harness, leader, member, msg_channel, spec_of, task_channel
from scripted_model import ScriptedChatModel, ai_text, ai_tool, last_tool_result
from teamagents.codex import CodexRunner
from teamagents.models import (
    ModelProfile,
    RuntimeKind,
    TeamAction,
    UserConfig,
)
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store

FAKE = Path(__file__).parent / "fake_codex_app_server.py"


def make_cli_wrapper(tmp_path: Path, mode: str) -> Path:
    wrapper = tmp_path / "fake-codex"
    wrapper.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n')
    wrapper.chmod(0o755)
    return wrapper


def build_runtime(tmp_path, mode: str, session_id: str = "s1"):
    work = tmp_path / "work"
    work.mkdir(parents=True, exist_ok=True)
    store = Store(tmp_path / f"{session_id}.db")
    spec = spec_of(leader(), member("cx", runtime=RuntimeKind.CODEX),
                   channels=[task_channel("leader", ["cx"]),
                             msg_channel("cx", ["leader"])])
    if store.get_session(session_id) is None:
        store.create_session(session_id, str(work), "approved_scope")
        store.save_team_spec(session_id, spec)
        for agent in spec.agents:
            store.ensure_agent(session_id, agent.id)
    approvals = ApprovalGate(store, session_id, PermissionPolicy())
    runners = {}
    cx = CodexRunner(agent=spec.agent("cx"), session_id=session_id, workdir=work,
                     approvals=approvals, store=store,
                     codex_bin=str(make_cli_wrapper(tmp_path, mode)),
                     env={"FAKE_CODEX_MODE": mode})
    runners["cx"] = cx

    def leader_wait(messages):
        task_id = last_tool_result(messages).get("result", {}).get("task_id")
        return ai_tool("wait_for_tasks", {"task_ids": [task_id]})

    leader_model = ScriptedChatModel(script=[
        ai_tool("assign_task", {"assignee": "cx", "description": "run the probe task"}),
        leader_wait,
        ai_tool("signal_done", {"summary": "probe done"}),
        ai_text("all done"),
    ])
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    from langgraph.checkpoint.memory import InMemorySaver
    runners["leader"] = DeepAgentsRunner(
        agent=spec.agent("leader"), catalog=catalog, session_id=session_id,
        workdir=work, artifacts_dir=tmp_path / "artifacts", checkpointer=InMemorySaver(),
        approvals=approvals, model_override=leader_model)
    rt = SessionRuntime(store, session_id, catalog, runners=runners, approvals=approvals)
    for runner in runners.values():
        if hasattr(runner, "status_hook"):
            runner.status_hook = rt.note_external_status
        if hasattr(runner, "progress_hook"):
            runner.progress_hook = rt.note_external_progress
    return rt, cx, leader_model


async def _wait(predicate, timeout=20.0, what="condition"):
    deadline = asyncio.get_event_loop().time() + timeout
    while not predicate():
        assert asyncio.get_event_loop().time() < deadline, f"timeout waiting for {what}"
        await asyncio.sleep(0.02)


async def test_codex_member_completes_task_and_reports_progress(tmp_path):
    rt, cx, _ = build_runtime(tmp_path, "simple")
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("delegate the probe")
        assert await rt.settle(30)
        tasks = rt.store.tasks_for_session("s1")
        assert tasks and tasks[0].status == "SUCCEEDED"
        runs = {r.agent_id: r for r in rt.store.runs_for_session("s1")}
        assert runs["cx"].status == "COMPLETED"
        assert runs["cx"].external_turn_id, "turn id must be recorded"
        assert rt.store.get_codex_thread("s1", "cx"), "thread id persisted before submission"
        progress = [e for e in rt.store.events("s1") if e["kind"] == "run_progress"]
        assert progress and "fake reply" in progress[0]["payload_json"]

        # recovery mapping: a fresh runner reads its own thread history
        fresh = CodexRunner(agent=rt.runners["cx"].agent, session_id="s1",
                            workdir=cx.workdir, approvals=cx.approvals, store=rt.store,
                            codex_bin=cx.codex_bin, env={"FAKE_CODEX_MODE": "simple"})
        state = await fresh.reconcile(runs["cx"])
        assert state in ("COMPLETED", None), f"unexpected reconcile state {state}"
        await fresh.aclose()
    finally:
        await rt.close()
        rt.store.close()


async def test_codex_approval_accept_flows_through_team_queue(tmp_path):
    rt, cx, _ = build_runtime(tmp_path, "approval")
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("delegate the probe")
        def parked():
            pending = rt.store.pending_approvals("s1")
            return bool(pending) and rt.store.get_run(pending[0].run_id).status == \
                "WAITING_APPROVAL"
        await _wait(parked, what="parked approval request")
        approval = rt.store.pending_approvals("s1")[0]
        assert approval.agent_id == "cx"
        run = rt.store.get_run(approval.run_id)
        assert run.status == "WAITING_APPROVAL", \
            "a Codex member waiting on approval is parked, not running"
        decide = rt.submit(TeamAction(action_id="d1", session_id="s1", actor_id="user",
                                      kind="approval_decision",
                                      payload={"approval_id": approval.approval_id,
                                               "decision": "once"}))
        assert decide.ok
        assert await rt.settle(30)
        assert rt.store.get_run(approval.run_id).status == "COMPLETED"
        progress = [e["payload_json"] for e in rt.store.events("s1")
                    if e["kind"] == "run_progress"]
        assert any("approval=accept" in p for p in progress)
        assert rt.store.tasks_for_session("s1")[0].status == "SUCCEEDED"
    finally:
        await rt.close()
        rt.store.close()


async def test_codex_approval_deny_is_recorded_and_reported(tmp_path):
    rt, cx, _ = build_runtime(tmp_path, "approval")
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("delegate the probe")
        def parked():
            pending = rt.store.pending_approvals("s1")
            return bool(pending) and rt.store.get_run(pending[0].run_id).status == \
                "WAITING_APPROVAL"
        await _wait(parked, what="parked approval request")
        approval = rt.store.pending_approvals("s1")[0]
        rt.submit(TeamAction(action_id="d2", session_id="s1", actor_id="user",
                             kind="approval_decision",
                             payload={"approval_id": approval.approval_id,
                                      "decision": "deny"}))
        assert await rt.settle(30)
        progress = [e["payload_json"] for e in rt.store.events("s1")
                    if e["kind"] == "run_progress"]
        assert any("approval=decline" in p for p in progress)
        assert rt.store.get_approval(approval.approval_id).status == "DENIED"
    finally:
        await rt.close()
        rt.store.close()


async def test_codex_cancel_waits_for_confirmed_stop(tmp_path):
    rt, cx, _ = build_runtime(tmp_path, "slow")
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("delegate the probe")
        await _wait(lambda: [r for r in rt.store.runs_for_session("s1", ["RUNNING"])
                             if r.agent_id == "cx"], what="cx turn running")
        task = rt.store.tasks_for_session("s1")[0]
        receipt = rt.submit(TeamAction(action_id="c1", session_id="s1", actor_id="user",
                                       kind="cancel_task",
                                       payload={"task_id": task.task_id}))
        assert receipt.ok
        assert await rt.settle(40)
        run = next(r for r in rt.store.runs_for_session("s1") if r.agent_id == "cx")
        assert run.status == "CANCELLED"
        assert rt.store.get_task(task.task_id).status == "CANCELLED"
    finally:
        await rt.close()
        rt.store.close()


async def test_codex_member_input_has_no_team_management_tools(tmp_path):
    rt, cx, _ = build_runtime(tmp_path, "simple")
    h = Harness(rt, {})
    await h.start()
    try:
        rt.user_message("delegate the probe")
        assert await rt.settle(30)
        rendered = cx._render_input.__wrapped__ if hasattr(cx._render_input, "__wrapped__") else None
        text = json.dumps(cx._queued_input) + str(rendered or "")
        # the adapter never hands team tools to Codex; the prompt says so explicitly
        from teamagents.models import AgentView
        view = AgentView(agent_id="cx")
        prompt = cx._render_input(view, None)
        assert "Do not attempt team-management actions" in prompt
        assert "assign_task" not in prompt and "signal_done" not in prompt
        assert not hasattr(cx, "bound_tool_names")
        _ = text
    finally:
        await rt.close()
        rt.store.close()
