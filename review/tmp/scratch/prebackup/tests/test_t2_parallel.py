"""T2: B and C run in parallel; a slow member does not block the Leader.

Parallelism is proven with a real synchronization barrier (both members must
arrive at once), never with timing assumptions.
"""

from __future__ import annotations

import asyncio
import time

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel


async def test_t2_parallel_with_barrier_and_supplement(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("c"),
        channels=[task_channel("leader", ["b", "c"]),
                  msg_channel("b", ["leader"]), msg_channel("c", ["leader"])],
    )
    members = scripts(
        leader=[
            ("call", "assign_task", {"assignee": "b", "description": "slow job B"}),
            ("call", "assign_task", {"assignee": "c", "description": "slow job C"}),
            ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id",
                                                     "$r1.result.task_id"]}),
            ("wait",),
            ("inbox",),                 # wake (user supplement): read the inbox
            ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id",
                                                     "$r1.result.task_id"]}),
            ("wait",),
            ("inbox",),                 # wake (task results)
            ("call", "signal_done", {"summary": "both done"}),
            ("end",),
        ],
        b=[("barrier", "both-started"), ("sleep", 0.6),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id",
                                      "result_refs": ["b.out"]}), ("end",)],
        c=[("barrier", "both-started"), ("sleep", 0.6),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id",
                                      "result_refs": ["c.out"]}), ("end",)],
    )
    h = await harness_factory(spec, members,
                              barriers={"both-started": asyncio.Barrier(2)})
    await h.user("run two jobs in parallel")

    # while B/C are working, a user supplement must be handled by the Leader
    await asyncio.sleep(0.2)
    h.rt.user_message("by the way, also check the logs", supplement=True)
    deadline = time.time() + 5
    while not any(item.get("kind") == "user_message" for item in members["leader"].observed_inbox):
        assert time.time() < deadline, "leader never processed the supplement"
        await asyncio.sleep(0.02)

    runs = {r.agent_id: r for r in h.rt.store.runs_for_session("s1")}
    assert runs["b"].status == "RUNNING" and runs["c"].status == "RUNNING", \
        "supplement handling must not wait for slow members"

    await h.settle()
    tasks = {t.assignee: t for t in h.rt.store.tasks_for_session("s1")}
    assert tasks["b"].status == "SUCCEEDED" and tasks["c"].status == "SUCCEEDED"
    assert h.rt.store.get_session("s1")["goal_state"] == "done"


async def test_t2_mid_turn_injection_to_running_member(harness_factory):
    """A message arriving while a member is RUNNING is injected at its next
    model-call boundary, not queued for a later turn."""
    spec = spec_of(
        leader(), member("b"),
        channels=[task_channel("leader", ["b"]), msg_channel("leader", ["b"])],
    )
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("sleep", 0.4),
           ("inbox",),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("start")
    deadline = time.time() + 5
    # wait until B is running, then inject a message while B still works
    while True:
        runs = [r for r in h.rt.store.runs_for_session("s1") if r.agent_id == "b"]
        if runs and runs[0].status == "RUNNING":
            break
        assert time.time() < deadline
        await asyncio.sleep(0.01)
    from teamagents.models import TeamAction
    leader_run = [r for r in h.rt.store.runs_for_session("s1")
                  if r.agent_id == "leader"][0]
    inject = h.rt.submit(TeamAction(
        action_id="inject-1", session_id="s1", actor_id="leader",
        run_id=leader_run.run_id, kind="send_message",
        payload={"target": "b", "text": "extra note mid-turn"}))
    assert inject.ok, inject.error
    await h.settle()
    observed = [x for x in members["b"].observed_inbox if x.get("kind") == "message"]
    assert observed and observed[0]["payload"]["text"] == "extra note mid-turn"
