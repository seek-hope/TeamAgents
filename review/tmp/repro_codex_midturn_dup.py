"""Probe: mid-turn message to a *running* Codex member.

Hypothesis A: the text is queued only in memory (`_queued_input`) and injected on
the next turn.
Hypothesis B: the next turn ALSO receives the same message through the persisted
delivery (view.inbox_delta) -> the member sees it twice.
Hypothesis C: the delivery is marked applied by the interrupted turn's ack while
Codex never saw it -> silent loss when the process exits before the next turn.
"""
from __future__ import annotations
import asyncio, os, shutil, sys
from pathlib import Path
sys.path.insert(0, "src"); sys.path.insert(0, "tests")
ROOT = Path("review/tmp/scratch/codexmid2").resolve()
shutil.rmtree(ROOT, ignore_errors=True); ROOT.mkdir(parents=True)

from conftest import leader, member, msg_channel, spec_of, task_channel
from scripted_model import ScriptedChatModel, ai_tool
from teamagents.codex import CodexRunner
from teamagents.models import ModelProfile, RuntimeKind, TeamAction, UserConfig
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store

FAKE = Path("tests/fake_codex_app_server.py").resolve()
W = ROOT / "fake-codex"
W.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n'); W.chmod(0o755)
PROMPTS: list[str] = []
RUNS: list[str] = []

async def main():
    work = ROOT / "work"; work.mkdir(parents=True, exist_ok=True)
    store = Store(ROOT / "s1.db")
    spec = spec_of(leader(), member("cx", runtime=RuntimeKind.CODEX),
                   channels=[task_channel("leader", ["cx"]),
                             msg_channel("leader", ["cx"]), msg_channel("cx", ["leader"])])
    store.create_session("s1", str(work), "approved_scope")
    store.save_team_spec("s1", spec)
    for a in spec.agents:
        store.ensure_agent("s1", a.id)
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    cx = CodexRunner(agent=spec.agent("cx"), session_id="s1", workdir=work,
                     approvals=approvals, store=store, codex_bin=str(W),
                     env={"FAKE_CODEX_MODE": "slow"})
    orig = cx._render_input
    def capture(view, wake):
        text = orig(view, wake)
        PROMPTS.append(text)
        return text
    cx._render_input = capture
    from langgraph.checkpoint.memory import InMemorySaver
    leader_model = ScriptedChatModel(script=[ai_tool("assign_task", {"assignee": "cx",
                                                                    "description": "probe"})])
    runners = {"cx": cx, "leader": DeepAgentsRunner(
        agent=spec.agent("leader"), catalog=UserConfig(models={"test": ModelProfile(provider="openai", model="test")}),
        session_id="s1", workdir=work, artifacts_dir=ROOT / "art", checkpointer=InMemorySaver(),
        approvals=approvals, model_override=leader_model)}
    rt = SessionRuntime(store, "s1", UserConfig(), runners=runners, approvals=approvals)
    for r in runners.values():
        if hasattr(r, "status_hook"): r.status_hook = rt.note_external_status
        if hasattr(r, "progress_hook"): r.progress_hook = rt.note_external_progress
    await rt.start()
    try:
        rt.user_message("delegate")
        async def wait_for(pred, timeout=20, what=""):
            dl = asyncio.get_event_loop().time() + timeout
            while not pred():
                assert asyncio.get_event_loop().time() < dl, f"timeout {what}"
                await asyncio.sleep(0.02)
        await wait_for(lambda: [r for r in store.runs_for_session("s1", ["RUNNING"]) if r.agent_id == "cx"], what="cx running")
        await wait_for(lambda: len(PROMPTS) >= 1, what="cx turn 1 rendered")   # message arrives mid-turn, after the prompt
        rt.submit(TeamAction(action_id="m1", session_id="s1", actor_id="leader",
                             kind="send_message", payload={"target": "cx",
                                                           "text": "IMPORTANT UPDATE"}))
        q = list(cx._queued_input.get("cx", []))
        print("A) queued only in memory:", q, flush=True)
        task = store.tasks_for_session("s1")[0]
        run1 = [x for x in store.runs_for_session("s1") if x.agent_id == "cx"][0]
        r = rt.submit(TeamAction(action_id="c1", session_id="s1", actor_id="user",
                             kind="cancel_task", payload={"task_id": task.task_id}))
        print("cancel receipt:", r.ok, r.result, r.error, flush=True)
        await wait_for(lambda: store.get_run(run1.run_id).status == "CANCELLED", timeout=40, what="cx cancelled")
        print("run1 cancelled; runs:", [(r.run_id, r.status) for r in store.runs_for_session("s1")], flush=True)
        # state of the pushed deliveries right after the interrupted turn
        ids = [r["delivery_id"] for r in store.conn.execute(
            "SELECT delivery_id FROM deliveries WHERE agent_id='cx'").fetchall()]
        rows = store.conn.execute(
            "SELECT delivery_id, batch_no, status FROM deliveries WHERE agent_id='cx'").fetchall()
        print("B) deliveries:", [(r["delivery_id"], r["batch_no"], r["status"]) for r in rows], flush=True)
        print("   pending_deliveries(cx):", [d["delivery_id"] for d in store.pending_deliveries("s1", "cx")], flush=True)
        # let the follow-up turn start (slow mode: it starts and blocks)
        await wait_for(lambda: len(PROMPTS) >= 2, timeout=40, what=f"second cx turn (have {len(PROMPTS)})")
        text2 = PROMPTS[1]
        print("C) second turn count of 'IMPORTANT UPDATE':", text2.count("IMPORTANT UPDATE"), flush=True)
        print("C) has queued_updates block:", "<queued_updates>" in text2, flush=True)
        print("C) prompt1 queued block:", "<queued_updates>" in PROMPTS[0], flush=True)
        print("C) all prompts count:", [t.count("IMPORTANT UPDATE") for t in PROMPTS], flush=True)
        print("C) queue now:", cx._queued_input, flush=True)
        print("C) run agents:", [(x.run_id[-6:], x.agent_id, x.status) for x in store.runs_for_session("s1")], flush=True)
        print("C) fresh runner after restart has queued input:",
              CodexRunner(agent=cx.agent, session_id="s1", workdir=work, approvals=approvals,
                          store=store, codex_bin=str(W))._queued_input, flush=True)
    finally:
        await rt.close(); store.close()

asyncio.run(main())
