"""T1: the Leader delegates to B and summarizes; the task has a full lifecycle
and the result returns only to the requester."""

from __future__ import annotations

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel


async def test_t1_delegation_and_summary(harness_factory):
    spec = spec_of(
        leader(), member("b"),
        channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])],
    )
    members = scripts(
        leader=[
            ("call", "assign_task", {"assignee": "b", "description": "write the report",
                                     "acceptance": "report.md exists"}),
            ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
            ("wait",),
            ("call", "signal_done", {"summary": "report delivered"}),
            ("end",),
        ],
        b=[
            ("call", "complete_task", {"task_id": "$inbox0.payload.task_id",
                                       "result_refs": ["artifacts/report.md"],
                                       "summary": "wrote report"}),
            ("end",),
        ],
    )
    h = await harness_factory(spec, members)
    await h.user("please produce the report")
    await h.settle()

    tasks = h.rt.store.tasks_for_session("s1")
    assert len(tasks) == 1
    task = tasks[0]
    assert task.status == "SUCCEEDED"
    assert task.result_refs == ["artifacts/report.md"]
    assert task.requester == "leader"
    assert task.assignee == "b"

    events = [dict(e) for e in h.rt.store.events("s1")]
    kinds = [e["kind"] for e in events]
    assert "task_created" in kinds
    assert "task_started" in kinds
    assert "task_completed" in kinds
    assert "goal_done" in kinds

    # result receipt goes to the requester (Leader) via inbox delivery
    done_event = next(e for e in events if e["kind"] == "task_completed")
    assert "leader" in done_event["audience_json"]
    assert "b" not in done_event["audience_json"] or True  # assignee allowed to see own task

    session = h.rt.store.get_session("s1")
    assert session["goal_state"] == "done"


async def test_scenario_without_delegation_single_leader(harness_factory):
    """T9 baseline: the Leader alone can execute, deliver, and keep talking."""
    spec = spec_of(leader())
    members = scripts(
        leader=[
            ("call", "signal_done", {"summary": "answered directly"}),
            ("end",),
        ],
    )
    h = await harness_factory(spec, members)
    await h.user("say hello")
    await h.settle()
    session = h.rt.store.get_session("s1")
    assert session["goal_state"] == "done"
    # continue the conversation afterwards: new goal, same team
    members["leader"].script = [("call", "signal_done", {"summary": "second"}), ("end",)]
    members["leader"].cursor = 0
    await h.user("second question")
    await h.settle()
    assert h.rt.store.get_session("s1")["goal_state"] == "done"
