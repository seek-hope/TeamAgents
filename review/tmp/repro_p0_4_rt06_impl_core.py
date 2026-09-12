"""Repro for P0-4 (_reduce half-apply) and RT-06 (cancelled run leaves PENDING approval).

Run: .venv/bin/python review/tmp/repro_p0_4_rt06_impl_core.py
"""
from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))
sys.path.insert(0, str(REPO / "tests"))

from conftest import leader, scripts, spec_of  # noqa: E402
from teamagents.models import ActionKind, TeamAction, TurnStatus  # noqa: E402
from teamagents.control import Control  # noqa: E402
from teamagents.runtime import fake_session  # noqa: E402
from teamagents.storage import Store  # noqa: E402

OUT: list[str] = []


def say(*parts) -> None:
    line = " ".join(str(p) for p in parts)
    OUT.append(line)
    print(line, flush=True)


def p0_4() -> None:
    say("== P0-4: _reduce raises after a write inside the same tx ==")
    tmp = Path(tempfile.mkdtemp(prefix="p04-"))
    store = Store(tmp / "s1.db")
    spec = spec_of(leader(), shared_spaces=[{"id": "main", "readers": ["leader"],
                                             "writers": ["leader"]}])
    store.create_session("s1", str(tmp), "approved_scope")
    store.save_team_spec("s1", spec)
    store.ensure_agent("s1", "leader")
    control = Control(store, "s1")
    original = Store.add_shared_entry

    def boom(self, entry, session_id):
        original(self, entry, session_id)  # the write happens ...
        raise RuntimeError("simulated failure after write")  # ... then it fails

    Store.add_shared_entry = boom
    try:
        receipt = control.submit(TeamAction(
            action_id="P-publish", session_id="s1", actor_id="leader",
            kind=ActionKind.PUBLISH_SHARED,
            payload={"space_id": "main", "content": "partial-write"}))
    finally:
        Store.add_shared_entry = original
    entries = store.shared_entries("s1", ["main"])
    say("P0-4 receipt ok:", receipt.ok, "| error:", receipt.error)
    say("P0-4 committed entries:", [e.content for e in entries])
    retry = control.submit(TeamAction(
        action_id="P-publish", session_id="s1", actor_id="leader",
        kind=ActionKind.PUBLISH_SHARED,
        payload={"space_id": "main", "content": "partial-write"}))
    say("P0-4 same-action retry ok:", retry.ok, "| entry count:", len(store.shared_entries("s1", ["main"])))
    say("P0-4 VERDICT:",
        "partial write committed + action_id stuck on failure"
        if (not receipt.ok and entries) else "rolled back cleanly")
    store.close()


async def rt06() -> None:
    say("\n== RT-06: cancel a WAITING_APPROVAL run -> approval stays PENDING ==")
    tmp = Path(tempfile.mkdtemp(prefix="rt06-"))
    spec = spec_of(leader(), channels=[])
    members = scripts(leader=[("call", "shell", {"command": "echo x"}),
                              ("call", "signal_done", {}), ("end",)])
    rt = fake_session(tmp, spec, members, require_approval={"shell"})
    await rt.start()
    try:
        rt.user_message("go")
        deadline = asyncio.get_event_loop().time() + 5
        while True:
            pending = rt.store.pending_approvals("s1")
            if pending and rt.store.get_run(pending[0].run_id).status == TurnStatus.WAITING_APPROVAL:
                break
            assert asyncio.get_event_loop().time() < deadline, "no parked approval"
            await asyncio.sleep(0.02)
        approval = pending[0]
        run = rt.store.get_run(approval.run_id)
        say("RT-06 before: run =", run.status, "| approval =", approval.status)
        say("RT-06 before blockers:", rt.control._completion_blockers(spec))
        receipt = rt.submit(TeamAction(action_id="c1", session_id="s1", actor_id="user",
                                       kind=ActionKind.CANCEL_RUN,
                                       payload={"run_id": run.run_id}))
        say("RT-06 cancel receipt:", receipt.ok, receipt.result)
        run2 = rt.store.get_run(run.run_id)
        approval2 = rt.store.get_approval(approval.approval_id)
        blockers = rt.control._completion_blockers(spec)
        say("RT-06 after: run =", run2.status, "| approval =", approval2.status)
        say("RT-06 after blockers:", blockers)
        say("RT-06 VERDICT:",
            "deadlock: cancelled run still WAITING_APPROVAL with PENDING approval"
            if (approval2.status == "PENDING" and run2.status == TurnStatus.WAITING_APPROVAL)
            else "resolved")
    finally:
        await rt.close()
        rt.store.close()


async def main() -> None:
    p0_4()
    await rt06()
    (REPO / "review/tmp/repro-p0_4-rt06-impl_core.txt").write_text(
        "\n".join(OUT) + "\n", encoding="utf-8")


asyncio.run(main())
