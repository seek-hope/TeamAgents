"""Probe: what happens to a live Codex turn when the app-server child dies?

- the reader loop ends on stdout EOF, but only pending *calls* are failed;
  the in-flight turn's `_turn_done` future is never resolved.
Output: whether the member hangs until the runtime's turn_active_timeout (900s).
"""
from __future__ import annotations
import asyncio, os, shutil, sys
from pathlib import Path
sys.path.insert(0, "src"); sys.path.insert(0, "tests")
ROOT = Path("review/tmp/scratch/codexdeath").resolve()
shutil.rmtree(ROOT, ignore_errors=True); ROOT.mkdir(parents=True)
FAKE = Path("tests/fake_codex_app_server.py").resolve()
W = ROOT / "fake-codex"
W.write_text(f'#!/bin/sh\nexec "{sys.executable}" "{FAKE}" "$@"\n'); W.chmod(0o755)

from teamagents.codex import CodexError, CodexRunner
from teamagents.models import AgentSpec, AgentView, RuntimeKind, TurnRun
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.storage import Store

async def main():
    store = Store(ROOT / "s.db")
    store.create_session("s1", str(ROOT), "approved_scope")
    agent = AgentSpec(id="cx", name="CX", role="worker", runtime_kind=RuntimeKind.CODEX,
                      instructions="probe", model_profile="test", tool_bindings=[])
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    cx = CodexRunner(agent=agent, session_id="s1", workdir=ROOT, approvals=approvals,
                     store=store, codex_bin=str(W), env={"FAKE_CODEX_MODE": "slow"})
    run = TurnRun(run_id="r1", session_id="s1", agent_id="cx", config_revision=1, topology_revision=1)
    task = asyncio.create_task(cx.start_or_resume(run, AgentView(agent_id="cx"), None, None))
    for _ in range(200):
        if cx._current_turn.get("r1"):
            break
        await asyncio.sleep(0.05)
    print("turn started:", cx._current_turn.get("r1"), "state:", cx._states.get("r1"), flush=True)
    # simulate the app-server crashing (child dies, stdout EOF)
    os.kill(cx.server.proc.pid, 9)
    await asyncio.sleep(0.5)
    print("after child SIGKILL:",
          "task.done =", task.done(),
          "| _turn_done done =", cx._turn_done["r1"].done(),
          "| _states =", cx._states.get("r1"),
          "| proc.returncode =", cx.server.proc.returncode, flush=True)
    try:
        await asyncio.wait_for(asyncio.shield(task), timeout=5)
        print("turn ended on its own within 5s:", task.result().status, flush=True)
    except asyncio.TimeoutError:
        print("turn STILL HANGS 5s after the backend died -> only runtime "
              "turn_active_timeout_s (default 900s) can stop it", flush=True)
    task.cancel()
    with __import__("contextlib").suppress(Exception):
        await task
    await cx.aclose()
    # handshake failure path for comparison: a binary that exits immediately
    quick = ROOT / "dead-codex"
    quick.write_text("#!/bin/sh\nexit 1\n"); quick.chmod(0o755)
    cx2 = CodexRunner(agent=agent, session_id="s1", workdir=ROOT, approvals=approvals,
                      store=store, codex_bin=str(quick))
    try:
        await cx2.start_or_resume(TurnRun(run_id="r2", session_id="s1", agent_id="cx",
                                          config_revision=1, topology_revision=1),
                                  AgentView(agent_id="cx"), None, None)
        print("dead binary: unexpectedly OK", flush=True)
    except Exception as e:
        print("dead binary raises:", type(e).__name__, str(e)[:70], flush=True)
    store.close()

asyncio.run(main())
