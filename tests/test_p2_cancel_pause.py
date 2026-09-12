"""P2: pause stops new dispatch; cancel waits for a confirmed stop and never
pretends that cancellation rolled back side effects."""

from __future__ import annotations

import asyncio

from conftest import leader, member, scripts, spec_of, task_channel
from teamagents.models import TeamAction


async def test_pause_stops_dispatch_and_user_input_resumes(harness_factory):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("start")
    await h.settle()
    assert h.rt.store.get_session("s1")["goal_state"] == "done"

    pause = h.rt.submit(TeamAction(action_id="pause-1", session_id="s1", actor_id="user",
                                   kind="pause_session", payload={}))
    assert pause.ok
    assert h.rt.store.get_session("s1")["status"] == "PAUSED"
    members["leader"].script = [("call", "signal_done", {"summary": "second goal"}),
                                ("end",)]
    members["leader"].cursor = 0
    h.rt.user_message("another goal while paused")
    await asyncio.sleep(0.2)
    assert h.rt.store.get_session("s1")["status"] == "ACTIVE", "user input resumes"
    await h.settle()
    assert h.rt.store.get_session("s1")["goal_state"] == "done"


async def test_cancel_waits_for_confirmed_stop(harness_factory):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "long job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("end",)],
        b=[("sleep", 5), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("start the long job")
    deadline = asyncio.get_event_loop().time() + 5
    while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
               if r.agent_id == "b"]:
        assert asyncio.get_event_loop().time() < deadline
        await asyncio.sleep(0.01)
    task = h.rt.store.tasks_for_session("s1")[0]

    receipt = h.rt.submit(TeamAction(action_id="cancel-1", session_id="s1",
                                     actor_id="user", kind="cancel_task",
                                     payload={"task_id": task.task_id}))
    assert receipt.ok and receipt.result["status"] == "CANCEL_REQUESTED"
    await h.settle(8)
    task = h.rt.store.get_task(task.task_id)
    assert task.status == "CANCELLED"
    run = [r for r in h.rt.store.runs_for_session("s1") if r.agent_id == "b"][0]
    assert run.status == "CANCELLED"
    kinds = [e["kind"] for e in h.rt.store.events("s1")]
    assert "task_cancelled" in kinds


async def test_cancel_is_not_rollback(harness_factory):
    """Cancel keeps completed work and records it; it never claims a rollback."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])],
                   shared_spaces=[{"id": "main", "readers": ["leader", "b"],
                                   "writers": ["leader", "b"]}])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("end",)],
        b=[("sleep", 5), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("work then get cut off")
    deadline = asyncio.get_event_loop().time() + 5
    while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
               if r.agent_id == "b"]:
        assert asyncio.get_event_loop().time() < deadline
        await asyncio.sleep(0.01)
    # b already published an artifact-like shared entry before being cancelled
    h.rt.submit(TeamAction(action_id="share-1", session_id="s1", actor_id="leader",
                           kind="publish_shared",
                           payload={"space_id": "main", "content": "partial work"}))
    task = h.rt.store.tasks_for_session("s1")[0]
    h.rt.submit(TeamAction(action_id="cancel-2", session_id="s1", actor_id="user",
                           kind="cancel_task", payload={"task_id": task.task_id}))
    await h.settle(8)
    entries = h.rt.store.shared_entries("s1", ["main"])
    assert entries and entries[0].content == "partial work", \
        "cancellation must not delete already produced work"
