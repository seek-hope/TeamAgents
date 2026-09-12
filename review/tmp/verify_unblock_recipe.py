"""End-to-end check of the host recovery recipe in unblock_session.py.

Builds a scratch session that reproduces the live leftovers exactly
(task_528b1b39c8c1 BLOCKED, appr_cfcb4a86bc4048d0 PENDING), keeps a second
"live" connection open (as the running TUI does), runs the recovery script as
a subprocess, and verifies from the live connection that both leftovers are
cleared and the completion blockers are empty.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))

from teamagents.control import Control  # noqa: E402
from teamagents.models import (  # noqa: E402
    ActionKind,
    AgentSpec,
    ApprovalRequest,
    ApprovalStatus,
    ChannelMode,
    ChannelSpec,
    RuntimeKind,
    Task,
    TaskStatus,
    TeamSpec,
    TurnStatus,
)
from teamagents.storage import Store  # noqa: E402

TASK_ID = "task_528b1b39c8c1"
APPR_ID = "appr_cfcb4a86bc4048d0"


def agent(aid: str, role: str = "worker") -> AgentSpec:
    return AgentSpec(id=aid, name=aid.title(), role=role, runtime_kind=RuntimeKind.DEEPAGENTS,
                     instructions="x", model_profile="test", tool_bindings=["files"])


def build_spec() -> TeamSpec:
    return TeamSpec(
        leader_id="leader",
        agents=[agent("leader", "leader"), agent("b")],
        channels=[ChannelSpec(source="leader", targets=["b"], mode=ChannelMode.TASK)],
    )


def main() -> int:
    tmp = Path(tempfile.mkdtemp(prefix="unblock-sim-"))
    live = Store(tmp / "team.db")  # stands in for the running TUI's connection
    live.create_session("s1", str(tmp), "approved_scope")
    spec = build_spec()
    live.save_team_spec("s1", spec)
    for a in spec.agents:
        live.ensure_agent("s1", a.id)
    live.insert_task("s1", Task(task_id=TASK_ID, requester="leader", assignee="b",
                                description="review core domain", status=TaskStatus.BLOCKED))
    live.insert_approval(ApprovalRequest(
        approval_id=APPR_ID, session_id="s1", agent_id="b", run_id="run_gone",
        tool_call_id="call_1", operation_hash="h",
        requested_scope={"tool": "regex_placeholder", "args": {}}, policy_revision=0))

    live_control = Control(live, "s1")
    before = live_control._completion_blockers(spec)
    print("baseline blockers:", before)
    assert len(before) == 2, before

    print("\n== run the recipe as the user would ==")
    proc = subprocess.run(
        [str(REPO / ".venv/bin/python"), str(REPO / "review/tmp/unblock_session.py"),
         "--session-dir", str(tmp), "--apply", "--yes"],
        capture_output=True, text=True, cwd=str(REPO))
    print(proc.stdout)
    if proc.stderr:
        print("stderr:", proc.stderr)
    assert proc.returncode == 0, proc.returncode

    task = live.get_task(TASK_ID)
    appr = live.get_approval(APPR_ID)
    print("task:", task.status, "| approval:", appr.status)
    assert task.status is TaskStatus.CANCELLED
    assert appr.status is ApprovalStatus.DENIED

    # the cancel emits TASK_CANCELLED, which pushes a wake to the leader: the
    # runtime schedules a notice turn for it. Expected, not a leak.
    deliveries = live.conn.execute(
        "SELECT d.agent_id, e.kind FROM deliveries d JOIN events e ON e.event_id=d.event_id"
    ).fetchall()
    print("deliveries:", sorted((r["agent_id"], r["kind"]) for r in deliveries))
    wake_runs = live.runs_for_session("s1", [TurnStatus.QUEUED, TurnStatus.RUNNING])
    print("scheduled wake runs:", [(r.agent_id, r.status) for r in wake_runs])
    assert wake_runs and all(r.agent_id == "leader" for r in wake_runs)
    while_notice = live_control._completion_blockers(spec)
    print("blockers while notice turn pending:", while_notice)
    assert all(b.startswith("active turns: leader:QUEUED") for b in while_notice)
    # simulate the runtime draining that notice turn (start -> finish)
    for r in wake_runs:
        assert live.update_run_status_where(r.run_id, TurnStatus.QUEUED, TurnStatus.RUNNING)
        assert live.update_run_status_where(r.run_id, TurnStatus.RUNNING, TurnStatus.COMPLETED)
    final = live_control._completion_blockers(spec)
    print("blockers after the wake turn drains:", final)
    assert final == []

    kinds = [r[0] for r in live.conn.execute(
        "SELECT kind FROM events WHERE kind IN ('task_cancelled','approval_decided')"
    ).fetchall()]
    print("events recorded:", sorted(kinds))
    assert sorted(kinds) == ["approval_decided", "task_cancelled"]

    # idempotency: a second run must not duplicate side effects
    proc2 = subprocess.run(
        [str(REPO / ".venv/bin/python"), str(REPO / "review/tmp/unblock_session.py"),
         "--session-dir", str(tmp), "--apply", "--yes"],
        capture_output=True, text=True, cwd=str(REPO))
    assert proc2.returncode == 0, proc2.returncode
    n = live.conn.execute(
        "SELECT COUNT(*) FROM events WHERE kind IN ('task_cancelled','approval_decided')"
    ).fetchone()[0]
    assert n == 2, n
    print("second run: no new events (idempotent) -> PASS")
    print("\nPASS: recipe clears both leftovers on the live connection")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
