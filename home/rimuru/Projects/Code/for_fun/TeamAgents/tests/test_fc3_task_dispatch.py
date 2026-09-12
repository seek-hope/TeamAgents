"""Dispatch liveness: a task assigned to a *busy* member must still get its own
turn after the member frees up.

The wake notification for a task is a delivery. When it arrives while the
member already runs a turn, it is pushed into that turn and acked when that
turn ends (F-C3/RT-05 semantics). Nothing then re-triggers a dispatch, so the
task could sit PENDING forever with the member IDLE (observed on the live
session: tasks assigned while a member was busy never started).

`_schedule` now dispatches a ready task directly to an idle member even with
no pending deliveries. This test pins both halves:

- while busy, the queued task's notification must NOT steal a second turn
  (one turn at a time per member);
- after the busy turn ends, the queued task runs and can be completed.
"""

from __future__ import annotations

from conftest import leader, member, scripts, spec_of, task_channel


async def test_task_assigned_while_busy_runs_after_current_turn(harness_factory):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[
            ("call", "assign_task", {"assignee": "b", "description": "t1: slow"}),
            ("call", "assign_task", {"assignee": "b",
                                     "description": "t2: queued while busy"}),
            ("end",),
        ],
        # turn 1: long enough that t2 arrives mid-turn, then ends
        # turn 2 (dispatched for t2): complete it
        b=[("sleep", 0.6), ("end",),
           ("call", "complete_task", {"task_id": "$run.task_id", "summary": "done",
                                      "result_refs": ["b.out"]}),
           ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("go")
    assert await h.rt.settle(15), "session did not settle"

    tasks = {t.description: t for t in h.rt.store.tasks_for_session("s1")}
    t2 = tasks["t2: queued while busy"]
    runs = [r for r in h.rt.store.runs_for_session("s1") if r.agent_id == "b"]

    assert len(runs) == 2, (
        f"the queued task must get its own turn after the busy one ends, "
        f"got b runs={[str(r.status) for r in runs]}")
    second = sorted(runs, key=lambda r: r.created_at)[1]
    assert second.task_id == t2.task_id, "the second turn must carry the queued task"
    assert t2.status == "SUCCEEDED", f"queued task not completed: {t2.status}"
    assert t2.result_refs == ["b.out"]


async def test_turn_completing_another_task_blocks_its_own_task(harness_factory):
    """A terminal run must not leave its own task RUNNING forever.

    Observed live: the turn carried t1, the member completed another task (t2)
    and ended; finalize only converged run.task_id when the turn ended with NO
    completion request at all, so t1 stayed RUNNING with a terminal run and no
    executor - a second flavour of "task frozen, member IDLE".
    """
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[
            ("call", "assign_task", {"assignee": "b", "description": "t1: left unfinished"}),
            ("call", "assign_task", {"assignee": "b", "description": "t2: completed instead"}),
            ("end",),
        ],
        # single turn: both pings arrive before the turn starts, member
        # completes t2 (the second one) instead of its own t1
        b=[("call", "complete_task",
            {"task_id": "$inbox1.payload.task_id", "summary": "did t2",
             "result_refs": ["b.out"]}),
           ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("go")
    assert await h.rt.settle(15), "session did not settle"

    tasks = {t.description: t for t in h.rt.store.tasks_for_session("s1")}
    t1 = tasks["t1: left unfinished"]
    t2 = tasks["t2: completed instead"]
    runs = [r for r in h.rt.store.runs_for_session("s1") if r.agent_id == "b"]
    assert len(runs) == 1, f"one turn expected, got {[str(r.status) for r in runs]}"
    assert runs[0].task_id == t1.task_id, "the turn must carry the older ready task"
    assert t2.status == "SUCCEEDED", f"t2 not completed: {t2.status}"
    assert t1.status == "BLOCKED", (
        f"a task whose run ended without completing it must be BLOCKED, "
        f"not stuck: {t1.status}")
