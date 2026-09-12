"""P1 guards: ACL/validation violations are rejected with readable errors,
identity is runtime-injected, version conflicts never half-apply, and the
turn budget stops runaway loops with LIMIT_REACHED."""

from __future__ import annotations

import asyncio
import json

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.models import RuntimeKind, TeamAction


def act(kind: str, actor: str, payload: dict, action_id: str = None, run_id: str = None):
    act.n += 1
    return TeamAction(action_id=action_id or f"t-{kind}-{actor}-{act.n}", session_id="s1",
                      actor_id=actor, run_id=run_id, kind=kind, payload=payload)
act.n = 0


async def test_violations_are_rejected(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("c"),
        channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])],
    )
    h = await harness_factory(spec, scripts(leader=[("end",)], b=[("end",)]))

    cases = [
        (act("assign_task", "b", {"assignee": "ghost", "description": "x"}),
         "unknown assignee"),
        (act("assign_task", "b", {"assignee": "leader", "description": "x"}),
         "not allowed to assign"),
        (act("send_message", "b", {"target": "leader", "text": "hi"}), None),  # allowed
        (act("send_message", "leader", {"target": "b", "text": "hi"}),
         "not allowed to message"),
        (act("signal_done", "b", {}), "only the Leader"),
        (act("apply_topology_patch", "b", {"operations": [{"op": "remove_agent",
                                                           "agent_id": "leader"}],
                                           "base_revision": 1}), "only the Leader"),
        (act("complete_task", "b", {"task_id": "nope"}), "unknown task"),
        (act("publish_shared", "b", {"space_id": "ghost", "content": "x"}), "unknown shared space"),
        (act("approval_decision", "b", {"approval_id": "x", "decision": "once"}),
         "only the local user"),
        (act("set_permission_mode", "leader", {"mode": "full_auto"}),
         "only the local user"),
    ]
    for i, (action, expect_error) in enumerate(cases):
        unique = action.model_copy(update={"action_id": f"{action.action_id}-{i}"})
        receipt = h.rt.submit(unique)
        if expect_error is None:
            assert receipt.ok, f"{action.kind} should be allowed"
        else:
            assert not receipt.ok and expect_error in (receipt.error or ""), \
                f"{action.kind}: expected {expect_error!r}, got {receipt.error!r}"

    # identity is injected: a member cannot act as someone else via payload fields
    forged = h.rt.submit(act("send_message", "b",
                             {"target": "leader", "text": "x", "actor_id": "leader"}))
    assert forged.ok  # payload field ignored, actor is b
    events = [e for e in h.rt.store.events("s1") if e["kind"] == "message"]
    assert all(e["actor_id"] == "b" for e in events)


async def test_codex_members_only_delegated_by_leader(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("cx", runtime=RuntimeKind.CODEX),
        channels=[task_channel("leader", ["b", "cx"]), task_channel("b", ["cx"])],
    )
    h = await harness_factory(spec, scripts(leader=[("end",)], b=[("end",)]))
    receipt = h.rt.submit(act("assign_task", "b",
                              {"assignee": "cx", "description": "delegate"}))
    assert not receipt.ok and "Leader" in receipt.error
    receipt = h.rt.submit(act("assign_task", "leader",
                              {"assignee": "cx", "description": "delegate"}))
    assert receipt.ok, receipt.error


async def test_dependency_cycle_and_unknown_dependency_rejected(harness_factory):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    h = await harness_factory(spec, scripts(leader=[("end",)], b=[("end",)]))
    r1 = h.rt.submit(act("assign_task", "leader",
                         {"assignee": "b", "description": "a", "task_id": "task_a"}))
    assert r1.ok
    r2 = h.rt.submit(act("assign_task", "leader",
                         {"assignee": "b", "description": "b", "task_id": "task_b",
                          "dependencies": ["task_a"]}))
    assert r2.ok
    r3 = h.rt.submit(act("assign_task", "leader",
                         {"assignee": "b", "description": "cycle", "task_id": "task_a",
                          "dependencies": ["task_b"]}))
    assert not r3.ok and "cycle" in r3.error
    r4 = h.rt.submit(act("assign_task", "leader",
                         {"assignee": "b", "description": "x", "dependencies": ["ghost"]}))
    assert not r4.ok and "unknown dependency" in r4.error


async def test_stale_patch_base_revision_conflicts(harness_factory):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    h = await harness_factory(spec, scripts(leader=[("end",)], b=[("end",)]))
    operations = [{"op": "add_channel",
                   "channel": {"source": "b", "targets": ["leader"], "mode": "message"}}]
    stale = h.rt.submit(act("apply_topology_patch", "leader",
                            {"operations": operations, "base_revision": 0}))
    assert not stale.ok and "stale" in stale.error
    ok = h.rt.submit(act("apply_topology_patch", "leader",
                         {"operations": operations, "base_revision": 1}))
    assert ok.ok and ok.result["status"] == "APPLIED"


async def test_turn_budget_stops_runaway_with_limit_reached(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("c"),
        channels=[task_channel("leader", ["b"]), msg_channel("b", ["c"]),
                  msg_channel("c", ["b"])],
        limits={"max_parallel_workers": 2, "max_members": 8, "max_turns_per_goal": 3,
                "max_model_steps_per_turn": 10, "turn_active_timeout_s": 30},
    )
    # b <-> c ping-pong forever; the turn budget must stop the cascade
    bounce = [("call", "send_message", {"target": "c", "text": "tick"}), ("end",)] * 5
    reply = [("call", "send_message", {"target": "b", "text": "tick"}), ("end",)] * 5
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "spam"}),
                ("wait",), ("end",)],
        b=bounce,
        c=reply,
    )
    h = await harness_factory(spec, members)
    await h.user("go")
    await h.settle()
    kinds = [e["kind"] for e in h.rt.store.events("s1")]
    assert "limit_reached" in kinds, "runaway scheduling must report LIMIT_REACHED"
    runs = h.rt.store.runs_for_session("s1")
    assert len(runs) <= 3, f"turn budget must cap started runs, got {len(runs)}"


async def test_failed_dependency_blocks_dependents_and_notifies_leader(harness_factory):
    """T22: no silent stalls — a failed dependency turns dependents BLOCKED and
    the Leader is told, so it can re-plan instead of idling."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "will fail",
                                        "task_id": "task_a"}),
                ("end",),
                ("inbox",), ("end",),
                ("inbox",), ("end",)],
        b=[("fail", "member crashed")],
    )
    h = await harness_factory(spec, members)
    await h.user("start")
    await h.settle()
    first = h.rt.submit(act("assign_task", "leader",
                            {"assignee": "b", "description": "depends on a",
                             "task_id": "task_b", "dependencies": ["task_a"]}))
    assert first.ok
    await asyncio.sleep(0.2)
    await h.settle()
    kinds = [(e["kind"], json.loads(e["payload_json"])) for e in h.rt.store.events("s1")]
    blocked = [p for k, p in kinds if k == "task_blocked" and p.get("task_id") == "task_b"]
    assert blocked, "dependency failure must block the dependent task"
    assert "task_a" in blocked[0]["reason"]
    assert h.rt.store.get_task("task_b").status == "BLOCKED"
    # the Leader is notified (its next turn saw the blocked notice), so it can re-decide
    seen = [item for item in members["leader"].observed_inbox
            if item.get("kind") == "task_blocked"]
    assert seen and seen[0]["payload"]["task_id"] == "task_b", \
        "Leader must receive the blocked notice"
