"""Repro: a mid-turn message delivered to a *running* Codex member is acked as
applied at turn end, but only injected into a later turn (in-memory queue)."""
from __future__ import annotations
import asyncio, json, os, shutil, sys
from pathlib import Path
sys.path.insert(0, "src"); sys.path.insert(0, "tests")
ROOT = Path("review/tmp/scratch/codexmid").resolve()
shutil.rmtree(ROOT, ignore_errors=True); ROOT.mkdir(parents=True)

from conftest import leader, member, msg_channel, spec_of, task_channel
from scripted_model import ScriptedChatModel, ai_text, ai_tool, last_tool_result
from teamagents.codex import CodexRunner
from teamagents.models import (AgentSpec, ModelProfile, RuntimeKind, TeamAction, UserConfig)
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store

FAKE = Path("tests/fake_codex_app_server.py").resolve()
def wrapper(rt: Path) -> Path:
    w = rt / "fake-codex"
    w.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n'); w.chmod(0o755)
    return w

def build_runtime(mode: str):
    work = ROOT / "work"; work.mkdir(parents=True, exist_ok=True)
    store = Store(ROOT / "s1.db")
    spec = spec_of(leader(), member("cx", runtime=RuntimeKind.CODEX),
                   channels=[task_channel("leader", ["cx"]),
                             msg_channel("cx", ["leader"]),
                             msg_channel("leader", ["cx"])])
    if store.get_session("s1") is None:
        store.create_session("s1", str(work), "approved_scope")
        store.save_team_spec("s1", spec)
        for a in spec.agents:
            store.ensure_agent("s1", a.id)
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    cx = CodexRunner(agent=spec.agent("cx"), session_id="s1", workdir=work,
                     approvals=approvals, store=store, codex_bin=str(wrapper(ROOT)),
                     env={"FAKE_CODEX_MODE": mode})
    runners = {"cx": cx}
    def leader_wait(messages):
        task_id = last_tool_result(messages).get("result", {}).get("task_id")
        return ai_tool("wait_for_tasks", {"task_ids": [task_id]})
    leader_model = ScriptedChatModel(script=[ai_tool("assign_task", {"assignee": "cx",
                                                                    "description": "probe"})])
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    from langgraph.checkpoint.memory import InMemorySaver
    runners["leader"] = DeepAgentsRunner(agent=spec.agent("leader"), catalog=catalog,
        session_id="s1", workdir=work, artifacts_dir=ROOT / "artifacts",
        checkpointer=InMemorySaver(), approvals=approvals, model_override=leader_model)
    rt = SessionRuntime(store, "s1", catalog, runners=runners, approvals=approvals)
    for r in runners.values():
        if hasattr(r, "status_hook"):
            r.status_hook = rt.note_external_status
        if hasattr(r, "progress_hook"):
            r.progress_hook = rt.note_external_progress
        if hasattr(r, "stream_hook"):
            r.stream_hook = rt.note_stream_chunk
    return rt, cx, store

async def wait_for(pred, timeout=20, what="cond"):
    deadline = asyncio.get_event_loop().time() + timeout
    while not pred():
        assert asyncio.get_event_loop().time() < deadline, f"timeout: {what}"
        await asyncio.sleep(0.02)

async def main():
    rt, cx, store = build_runtime("slow")
    await rt.start()
    try:
        rt.user_message("delegate")
        runs = lambda st: [r for r in store.runs_for_session("s1", [st]) if r.agent_id == "cx"]
        await wait_for(lambda: runs("RUNNING"), what="cx running")
        receipt = rt.submit(TeamAction(action_id="m1", session_id="s1", actor_id="leader",
                                       kind="send_message",
                                       payload={"target": "cx", "text": "IMPORTANT UPDATE"}))
        print("send_message receipt.ok:", receipt.ok, receipt.error)
        print("cx._queued_input (in-memory, deferred):", cx._queued_input)
        rows = {r["delivery_id"]: r for r in store.conn.execute("SELECT delivery_id, agent_id, status, batch_no FROM deliveries WHERE agent_id='cx'").fetchall()}
        print("deliveries for cx right after push:", [(k, v["status"]) for k, v in rows.items()])
        # user cancels the task: the member's turn ends without another turn
        task = store.tasks_for_session("s1")[0]
        rt.submit(TeamAction(action_id="c1", session_id="s1", actor_id="user",
                             kind="cancel_task", payload={"task_id": task.task_id}))
        await wait_for(lambda: not rt._inflight, timeout=30, what="cx turn to end")
        await asyncio.sleep(0.3)
        rows = {r["delivery_id"]: r for r in store.conn.execute("SELECT delivery_id, agent_id, status, batch_no FROM deliveries WHERE agent_id='cx'").fetchall()}
        print("deliveries for cx after the turn ended:", [(k, v["status"]) for k, v in rows.items()])
        print("pending_deliveries(cx) now:", store.pending_deliveries("s1", "cx"))
        print("cx run status:", [r.status for r in store.runs_for_session("s1") if r.agent_id == "cx"])
        print("queued input still in memory:", cx._queued_input)
        print("=> fresh runner (restart) would lose it:",
              CodexRunner(agent=cx.agent, session_id="s1", workdir=cx.workdir,
                          approvals=cx.approvals, store=store,
                          codex_bin=cx.codex_bin)._queued_input)
    finally:
        await rt.close(); store.close()

asyncio.run(main())
