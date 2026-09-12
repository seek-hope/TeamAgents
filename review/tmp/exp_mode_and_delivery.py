"""Review experiment C (runtime domain).

C1: `set_permission_mode` (user action, T16) only writes the DB + an event; the
    live ApprovalGate keeps its old mode until the process restarts.
C2: a delivery that arrives while the member is running is acked at turn end
    even when the graph never made another model call, and it is never retried.

Read-only w.r.t. the repo; everything under a temp dir.
Run:  .venv/bin/python review/tmp/exp_mode_and_delivery.py
"""
from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

import teamagents

REPO = Path(teamagents.__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "tests"))

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel  # noqa: E402
from teamagents.models import TeamAction  # noqa: E402
from teamagents.runtime import fake_session  # noqa: E402


async def c1_mode(tmp: Path) -> None:
    spec = spec_of(leader(), channels=[])
    rt = fake_session(tmp / "c1", spec, scripts(leader=[("end",)]), session_id="c1")
    await rt.start()
    try:
        gate = rt.approvals
        print("C1 before: gate.mode =", gate.policy.mode.value,
              "| policy_revision =", gate.policy_revision)
        rt.submit(TeamAction(action_id="m1", session_id="c1", actor_id="user",
                             kind="set_permission_mode", payload={"mode": "full_auto"}))
        print("C1 after : gate.mode =", gate.policy.mode.value,
              "| db mode =", rt.store.get_session("c1")["permissions_mode"],
              "| policy_revision =", gate.policy_revision)
        # ... and the reverse direction
        rt.submit(TeamAction(action_id="m2", session_id="c1", actor_id="user",
                             kind="set_permission_mode", payload={"mode": "approved_scope"}))
        print("C1 back  : gate.mode =", gate.policy.mode.value,
              "| db mode =", rt.store.get_session("c1")["permissions_mode"])
        print("C1 set_mode callers in src:",
              "(none -- grep)")  # see report; ApprovalGate.set_mode is only reachable from tests
    finally:
        await rt.close()
        rt.store.close()


async def c2_delivery(tmp: Path) -> None:
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("sleep", 0.6), ("end",)],
        b=[("call", "send_message", {"target": "leader", "text": "MIDTURN-LOST"}),
           ("end",)],
    )
    rt = fake_session(tmp / "c2", spec, members, session_id="c2")
    await rt.start()
    try:
        rt.user_message("go")
        await rt.settle(20)
        leader_runs = [r for r in rt.store.runs_for_session("c2") if r.agent_id == "leader"]
        print("C2 leader runs:", len(leader_runs),
              "| statuses:", [r.status for r in leader_runs])
        print("C2 pending deliveries for leader after settle:",
              rt.store.pending_deliveries("c2", "leader"))
        print("C2 leader observed inbox items:",
              [i.get("payload", {}).get("text") for i in members["leader"].observed_inbox])
        print("C2 message delivered_to:",
              [e["payload_json"] for e in rt.store.events("c2") if e["kind"] == "message"])
        print("C2 acked (applied) deliveries:",
              rt.store.conn.execute(
                  "SELECT status, COUNT(*) c FROM deliveries WHERE agent_id='leader'"
                  " GROUP BY status").fetchall().__len__())
    finally:
        await rt.close()
        rt.store.close()


async def main() -> None:
    base = Path(tempfile.mkdtemp(prefix="ta-mode-"))
    await c1_mode(base)
    await c2_delivery(base)


if __name__ == "__main__":
    asyncio.run(main())
