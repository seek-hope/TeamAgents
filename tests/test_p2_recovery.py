"""P2: persistence, restart recovery and the four crash windows.

- queued execution intents survive a restart and still run
- a RUNNING run without an external turn is re-queued; with one it becomes
  OUTCOME_UNKNOWN instead of being blindly retried
- an action committed before a crash is not applied twice after the retry
- acknowledged deliveries are never injected twice
"""

from __future__ import annotations

import asyncio

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.runtime import fake_session


async def test_crash_after_action_commit_applies_once(harness_factory, tmp_path):
    """Crash window 3: the team action committed but the turn never finished.
    Re-running the turn replays the same action id -> exactly one task."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])

    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "one task"}),
                ("barrier", "hold"), ("call", "signal_done", {}), ("end",)],
    )
    h = await harness_factory(spec, members, barriers={"hold": asyncio.Barrier(2)})
    await h.user("go")
    # wait until assign_task committed, then simulate a hard process loss
    deadline = asyncio.get_event_loop().time() + 5
    while not h.rt.store.tasks_for_session("s1"):
        assert asyncio.get_event_loop().time() < deadline
        await asyncio.sleep(0.01)
    await h.rt.close()  # process loss: in-flight task cancelled mid-turn

    # restart in a fresh runtime over the same database
    fresh = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "one task"}),
                ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)],
    )
    h2 = Harness(fake_session(tmp_path, spec, fresh, session_id="s1"), fresh)
    await h2.start()
    try:
        await h2.settle(8)
        tasks = h2.rt.store.tasks_for_session("s1")
        assert len(tasks) == 1, "replayed action must dedup, not create a second task"
        created = [e for e in h2.rt.store.events("s1") if e["kind"] == "task_created"]
        assert len(created) == 1
    finally:
        await h2.aclose()


async def test_running_run_with_external_turn_becomes_outcome_unknown(tmp_path):
    spec = spec_of(leader(), channels=[])
    members = scripts(leader=[("sleep", 1), ("end",)])
    rt = fake_session(tmp_path, spec, members, session_id="s1")
    await rt.start()
    try:
        rt.user_message("go")
        deadline = asyncio.get_event_loop().time() + 5
        while not rt.store.runs_for_session("s1", ["RUNNING"]):
            assert asyncio.get_event_loop().time() < deadline
            await asyncio.sleep(0.01)
        run = rt.store.runs_for_session("s1", ["RUNNING"])[0]
        # an external backend turn was submitted before the crash
        rt.store.set_run_status(run.run_id, run.status, external_turn_id="codex-turn-1")
    finally:
        await rt.close()
        rt.store.close()

    rt2 = fake_session(tmp_path, spec, scripts(leader=[("end",)]), session_id="s1")
    await rt2.start()
    try:
        await asyncio.sleep(0.1)
        run = rt2.store.get_run(run.run_id)
        assert run.status == "OUTCOME_UNKNOWN"
        assert not [r for r in rt2.store.runs_for_session("s1", ["RUNNING"])], \
            "unknown outcomes are never auto-retried"
    finally:
        await rt2.close()
        rt2.store.close()


async def test_acked_deliveries_are_not_injected_twice(harness_factory, tmp_path):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("run the job")
    await h.settle()
    assert h.rt.store.pending_deliveries("s1", "b") == []

    # restart: nothing pending, nothing re-injected
    fresh = scripts(leader=[], b=[])
    h2 = Harness(fake_session(tmp_path, spec, fresh, session_id="s1"), fresh)
    await h2.start()
    try:
        await asyncio.sleep(0.1)
        assert h2.rt.store.pending_deliveries("s1", "b") == []
        assert fresh["b"].observed_inbox == []
    finally:
        await h2.aclose()


async def test_task_ready_announced_once_across_restart(harness_factory, tmp_path):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(leader=[("call", "assign_task",
                               {"assignee": "b", "description": "job"}), ("end",)],
                      b=[("end",)])
    h = await harness_factory(spec, members)
    await h.user("assign only")
    await h.settle(5)
    before = len([e for e in h.rt.store.events("s1") if e["kind"] == "task_ready"])

    fresh = scripts(leader=[], b=[])
    h2 = Harness(fake_session(tmp_path, spec, fresh, session_id="s1"), fresh)
    await h2.start()
    try:
        await asyncio.sleep(0.1)
        after = len([e for e in h2.rt.store.events("s1") if e["kind"] == "task_ready"])
        assert after == before == 1, "TASK_READY must not be re-announced after restart"
    finally:
        await h2.aclose()
