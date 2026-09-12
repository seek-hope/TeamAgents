"""Probe: a Codex approval that is still PENDING when the process dies.

After a restart the run is terminal (OUTCOME_UNKNOWN/FAILED) but the approval
stays PENDING, and `_completion_blockers` counts pending approvals as a blocker
-> signal_done can never succeed and the approval is delivered to the user for a
turn that no longer exists.
"""
from __future__ import annotations
import asyncio, shutil, sys
from pathlib import Path
sys.path.insert(0, "src"); sys.path.insert(0, "tests")
ROOT = Path("review/tmp/scratch/orphan_approval").resolve()
shutil.rmtree(ROOT, ignore_errors=True); ROOT.mkdir(parents=True)

from conftest import leader, member, msg_channel, spec_of, task_channel
from scripted_model import ScriptedChatModel, ai_text, ai_tool, last_tool_result
from teamagents.codex import CodexRunner
from teamagents.models import (ModelProfile, RuntimeKind, TeamAction, UserConfig)
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store

FAKE = Path("tests/fake_codex_app_server.py").resolve()
WORK = ROOT / "work"; WORK.mkdir(parents=True)

def wrapper(name: str) -> Path:
    w = ROOT / name
    w.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n'); w.chmod(0o755)
    return w

def build(store_path: Path, leader_script):
    work = WORK
    store = Store(store_path)
    spec = spec_of(leader(), member("cx", runtime=RuntimeKind.CODEX),
                   channels=[task_channel("leader", ["cx"]), msg_channel("cx", ["leader"])])
    if store.get_session("s1") is None:
        store.create_session("s1", str(work), "approved_scope")
        store.save_team_spec("s1", spec)
        for a in spec.agents:
            store.ensure_agent("s1", a.id)
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    cx = CodexRunner(agent=spec.agent("cx"), session_id="s1", workdir=work,
                     approvals=approvals, store=store, codex_bin=str(wrapper("fake-codex")),
                     env={"FAKE_CODEX_MODE": "approval"})
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    from langgraph.checkpoint.memory import InMemorySaver
    lm = ScriptedChatModel(script=leader_script)
    lr = DeepAgentsRunner(agent=spec.agent("leader"), catalog=catalog, session_id="s1",
        workdir=work, artifacts_dir=ROOT / "artifacts", checkpointer=InMemorySaver(),
        approvals=approvals, model_override=lm)
    rt = SessionRuntime(store, "s1", catalog, runners={"cx": cx, "leader": lr},
                        approvals=approvals)
    for r in (cx, lr):
        for attr, value in (("status_hook", rt.note_external_status),
                            ("progress_hook", rt.note_external_progress),
                            ("stream_hook", rt.note_stream_chunk)):
            if hasattr(r, attr):
                setattr(r, attr, value)
    return rt, store, lm

async def wait_for(pred, timeout=25, what="cond"):
    dl = asyncio.get_event_loop().time() + timeout
    while not pred():
        assert asyncio.get_event_loop().time() < dl, f"timeout {what}"
        await asyncio.sleep(0.02)

def leader_wait(messages):
    tid = last_tool_result(messages).get("result", {}).get("task_id")
    return ai_tool("wait_for_tasks", {"task_ids": [tid]})

async def main():
    rt, store, _ = build(ROOT / "s1.db", [ai_tool("assign_task",
                                                  {"assignee": "cx", "description": "probe"}), leader_wait])
    await rt.start()
    rt.user_message("delegate")
    await wait_for(lambda: store.pending_approvals("s1"), what="parked approval")
    ap = store.pending_approvals("s1")[0]
    print("parked approval:", ap.approval_id, "run:", store.get_run(ap.run_id).status, flush=True)
    # process loss with the approval still PENDING (plan T8 asks that approvals recover)
    await rt.close(); store.close()
    print("after restart, approval in DB:", flush=True)
    store2 = Store(ROOT / "s1.db")
    print("  pending approvals:", [a.approval_id for a in store2.pending_approvals("s1")],
          "| run status:", store2.get_run(ap.run_id).status, flush=True)
    rt2, store3, lm2 = build(ROOT / "s1.db", [])
    await rt2.start()
    try:
        # the user (somehow) decides the approval that belongs to the dead turn
        r = rt2.submit(TeamAction(action_id="d1", session_id="s1", actor_id="user",
                                  kind="approval_decision",
                                  payload={"approval_id": ap.approval_id, "decision": "once"}))
        print("approval_decision receipt:", r.ok, r.result, r.error, flush=True)
        await asyncio.sleep(2.0)
        runs = [(x.run_id[-6:], x.status, x.external_turn_id) for x in store3.runs_for_session("s1")]
        print("runs after the decision:", runs, flush=True)
        print("approval now:", store3.get_approval(ap.approval_id).status, flush=True)
        ev = [(e["kind"], e["payload_json"][:90]) for e in store3.events("s1")]
        print("events tail:", ev[-4:], flush=True)
    finally:
        await rt2.close(); store3.close()

asyncio.run(main())
