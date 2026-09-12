"""Repro: Codex progress text is duplicated (streamed deltas + item/completed)."""
from __future__ import annotations
import asyncio, os, shutil, sys
from pathlib import Path
sys.path.insert(0, "src"); sys.path.insert(0, "tests")
ROOT = Path("review/tmp/scratch/codexdup").resolve()
shutil.rmtree(ROOT, ignore_errors=True); ROOT.mkdir(parents=True)
FAKE = Path("tests/fake_codex_app_server.py").resolve()
wrapper = ROOT / "fake-codex"
wrapper.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n'); wrapper.chmod(0o755)

from teamagents.codex import CodexRunner
from teamagents.models import AgentSpec, AgentView, ApprovalRequest, RuntimeKind, TurnRun, UserConfig
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.storage import Store

store = Store(ROOT / "s.db")
store.create_session("s1", str(ROOT), "approved_scope")
agent = AgentSpec(id="cx", name="CX", role="worker", runtime_kind=RuntimeKind.CODEX,
                  instructions="probe", model_profile="test", tool_bindings=[])
spec_ok = True
approvals = ApprovalGate(store, "s1", PermissionPolicy())
events = []
cx = CodexRunner(agent=agent, session_id="s1", workdir=ROOT, approvals=approvals, store=store,
                 codex_bin=str(wrapper), env={"FAKE_CODEX_MODE": "simple"},
                 progress_hook=lambda run_id, text: events.append(text))
run = TurnRun(run_id="r1", session_id="s1", agent_id="cx", config_revision=1, topology_revision=1)

async def main():
    outcome = await cx.start_or_resume(run, AgentView(agent_id="cx"), None, None)
    print("outcome.status:", outcome.status)
    print("reply_text:", repr(outcome.reply_text))
    print("progress events:", [repr(e) for e in events])
    await cx.aclose()
asyncio.run(main())
