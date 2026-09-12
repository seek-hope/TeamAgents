"""P4/T10-T14: natural-language team building effects — proposals, drain
boundaries, member removal hand-over, model change, version conflicts,
mid-execution supplements."""

from __future__ import annotations

import asyncio
import json
import time

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel
from scripted_model import (ScriptedChatModel, ai_text, ai_tool, find_task_id,
                            last_tool_result)
from teamagents.models import TeamAction
from teamagents.runners import DeepAgentsRunner
from teamagents.runtime import SessionRuntime
from teamagents.models import ModelProfile, UserConfig
from teamagents.permissions import ApprovalGate, PermissionPolicy
from teamagents.storage import Store
from langgraph.checkpoint.memory import InMemorySaver

CATALOG = UserConfig(models={"test": ModelProfile(provider="openai", model="test"),
                             "other": ModelProfile(provider="openai", model="other")})


def act(kind: str, actor: str, payload: dict, action_id: str | None = None):
    act.n += 1
    return TeamAction(action_id=action_id or f"p4-{kind}-{act.n}", session_id="s1",
                      actor_id=actor, kind=kind, payload=payload)


act.n = 0


def add_worker_op(agent_id: str = "worker", channels: list[dict] | None = None) -> dict:
    return {"op": "add_agent",
            "agent": {"id": agent_id, "name": agent_id.title(), "role": "worker",
                      "runtime_kind": "deepagents", "instructions": "work",
                      "model_profile": "test", "tool_bindings": ["files"],
                      "skills": [], "workspace_policy": "shared"},
            "channels": channels if channels is not None else [
                {"source": "leader", "targets": [agent_id], "mode": "task"}]}


async def test_member_proposal_is_leader_decision(harness_factory):
    """T11: a member can only propose; the Leader applies; it takes effect."""
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("inbox",), ("end",)],
        b=[("call", "propose_team_change", {
            "operations": [add_worker_op()], "rationale": "need a helper"}),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
    )
    h = await harness_factory(spec, members, catalog=CATALOG)
    await h.user("please start")
    receipt = h.rt.submit(act("assign_task", "leader",
                              {"assignee": "b", "description": "propose a helper"}))
    assert receipt.ok
    await h.settle()
    patches = [e for e in h.rt.store.events("s1")
               if e["kind"] == "topology_proposed"]
    assert patches, "member proposal must be recorded"
    proposal = h.rt.store.patches_in_status("s1", "PROPOSED")[0]
    applied = h.rt.submit(act("apply_topology_patch", "leader",
                              {"patch_id": proposal.patch_id}))
    assert applied.ok and applied.result["status"] == "APPLIED", applied.result
    assert json.loads(patches[0]["payload_json"])["proposer"] == "b", \
        "the proposal keeps its author for audit"
    spec_after = h.rt.store.load_team_spec("s1")
    assert any(a.id == "worker" for a in spec_after.agents), \
        "Leader application must create the proposed member"
    assert h.rt.store.current_revision("s1") >= 2


async def test_structural_change_waits_for_boundary(harness_factory):
    """T11: affected members drain first; the patch applies after their turn ends."""
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "slow"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("sleep", 1.0),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
    )
    h = await harness_factory(spec, members, catalog=CATALOG)
    await h.user("start the slow job")
    deadline = time.time() + 5
    while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
               if r.agent_id == "b"]:
        assert time.time() < deadline
        await asyncio.sleep(0.01)

    patch = h.rt.submit(act("apply_topology_patch", "leader",
                            {"operations": [
                                {"op": "update_agent", "agent_id": "b",
                                 "changes": {"instructions": "be terse"}}],
                             "base_revision": 1}))
    assert patch.ok and patch.result["status"] == "WAITING_BOUNDARY", patch.result
    assert h.rt.store.agent_status("s1", "b").value == "DRAINING"
    assert h.rt.store.current_revision("s1") == 1, "no partial application"
    assert h.rt.store.agent_config_revision("s1", "b") == 1

    await h.settle(10)
    assert h.rt.store.current_revision("s1") == 2, "patch applies after the boundary"
    assert h.rt.store.agent_config_revision("s1", "b") == 2
    assert h.rt.store.agent_status("s1", "b").value == "IDLE"
    assert "topology_applied" in [e["kind"] for e in h.rt.store.events("s1")]


async def test_removed_member_hands_tasks_to_leader_and_keeps_audit(harness_factory):
    """T13: stop first, then remove; pending tasks go to the Leader; results kept."""
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])],
                   shared_spaces=[{"id": "main", "readers": ["leader", "b"],
                                   "writers": ["leader", "b"]}])
    members = scripts(
        leader=[("end",)],
        b=[("call", "publish_shared", {"space_id": "main", "content": "partial result"}),
           ("sleep", 0.6),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",), ("end",)],
    )
    h = await harness_factory(spec, members, catalog=CATALOG)
    # (1) a first task keeps b busy; (2) a second one is queued behind it
    first = h.rt.submit(act("assign_task", "leader",
                            {"assignee": "b", "description": "publish partial work"}))
    assert first.ok, first.error
    deadline = time.time() + 5
    while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
               if r.agent_id == "b"]:
        assert time.time() < deadline
        await asyncio.sleep(0.01)
    second = h.rt.submit(act("assign_task", "leader",
                             {"assignee": "b", "description": "not started yet"}))
    assert second.ok and second.result["task_id"] != first.result["task_id"]
    receipt = h.rt.submit(act("apply_topology_patch", "leader",
                              {"operations": [{"op": "remove_agent", "agent_id": "b"}],
                               "base_revision": 1}))
    assert receipt.ok and receipt.result["status"] == "WAITING_BOUNDARY"
    await h.settle(15)

    spec_after = h.rt.store.load_team_spec("s1")
    assert all(a.id != "b" for a in spec_after.agents)
    assert h.rt.store.agent_status("s1", "b").value == "REMOVED"
    tasks = h.rt.store.tasks_for_session("s1")
    pending = [t for t in tasks if t.status in ("PENDING", "BLOCKED")]
    assert pending and all(t.assignee == "leader" for t in pending), \
        "unfinished work is handed back to the Leader"
    entries = h.rt.store.shared_entries("s1", ["main"])
    assert entries and entries[0].content == "partial result", \
        "results produced before removal are kept"
    removed_events = [e for e in h.rt.store.events("s1") if e["kind"] == "member_removed"]
    assert removed_events, "removal is audited"
    assert not h.rt.store.pending_deliveries("s1", "b"), \
        "undelivered messages are dropped with a recorded reason"


async def test_model_change_takes_effect_on_next_turn(tmp_path):
    """T11/T24: changing a member's model rebuilds its backend at the boundary;
    the next turn runs on the new model, identity and ACLs unchanged."""
    from conftest import msg_channel as mc

    work = tmp_path / "work"
    work.mkdir()
    store = Store(tmp_path / "s1.db")
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), mc("b", ["leader"])])
    store.create_session("s1", str(work), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test"),
                                 "other": ModelProfile(provider="openai", model="other")})
    approvals = ApprovalGate(store, "s1", PermissionPolicy())
    def complete_from_view(label):
        def step(messages):
            text = json.dumps([str(getattr(m, "content", "")) for m in messages])
            return ai_tool("complete_task", {"task_id": find_task_id(text),
                                             "summary": label})
        return step

    old_model = ScriptedChatModel(script=[complete_from_view("old"), ai_text("old done")])
    new_model = ScriptedChatModel(script=[complete_from_view("new"), ai_text("new done")])
    created: list[ScriptedChatModel] = []

    def factory(agent):
        model = old_model if not created else new_model
        created.append(model)
        return DeepAgentsRunner(agent=agent, catalog=catalog, session_id="s1",
                                workdir=work, artifacts_dir=tmp_path / "art",
                                checkpointer=InMemorySaver(), approvals=approvals,
                                model_override=model)

    leader_model = ScriptedChatModel(script=[
        ai_tool("assign_task", {"assignee": "b", "description": "first"}),
        lambda m: ai_tool("wait_for_tasks",
                          {"task_ids": [last_tool_result(m)["result"]["task_id"]]}),
        ai_tool("assign_task", {"assignee": "b", "description": "second"}),
        lambda m: ai_tool("wait_for_tasks",
                          {"task_ids": [last_tool_result(m)["result"]["task_id"]]}),
        ai_text("all done"),
    ])
    runners = {
        "leader": DeepAgentsRunner(agent=spec.agent("leader"), catalog=catalog,
                                   session_id="s1", workdir=work,
                                   artifacts_dir=tmp_path / "art",
                                   checkpointer=InMemorySaver(), approvals=approvals,
                                   model_override=leader_model),
    }
    rt = SessionRuntime(store, "s1", catalog, runners=runners, approvals=approvals,
                        runner_factory=factory)
    for runner in list(runners.values()):
        if hasattr(runner, "bound_tool_names"):
            pass
    await rt.start()
    try:
        rt.user_message("run the first job")
        deadline = time.time() + 5
        while not [r for r in rt.store.runs_for_session("s1", ["RUNNING"])
                   if r.agent_id == "b"]:
            assert time.time() < deadline
            await asyncio.sleep(0.01)
        assert created and created[0] is old_model, "member backend built lazily"
        # the change lands at the boundary of the live turn, before the next one
        patch = rt.submit(act("apply_topology_patch", "leader",
                              {"operations": [{"op": "update_agent", "agent_id": "b",
                                               "changes": {"model_profile": "other"}}],
                               "base_revision": 1}))
        assert patch.ok and patch.result["status"] == "WAITING_BOUNDARY", patch.result
        await rt.settle(15)
        assert created[-1] is new_model, "config change rebuilds the member backend"
        assert old_model.calls and new_model.calls, "both models served turns"
        # identity and ACLs unchanged: same member id, same channel rights
        assert rt.store.load_team_spec("s1").agent("b").model_profile == "other"
        assert rt.store.load_team_spec("s1").can_delegate("leader", "b")
    finally:
        await rt.close()
        store.close()


async def test_two_conflicting_patches_never_partially_apply(harness_factory):
    """T12: a patch based on an old revision is rejected wholesale."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    h = await harness_factory(spec, scripts(leader=[("end",)], b=[("end",)]), catalog=CATALOG)
    first = h.rt.submit(act("apply_topology_patch", "leader",
                            {"operations": [add_worker_op("worker1")],
                             "base_revision": 1}))
    assert first.ok
    conflicting = h.rt.submit(act("apply_topology_patch", "leader",
                                  {"operations": [add_worker_op("worker2"),
                                                  {"op": "set_space_acl",
                                                   "space_id": "main",
                                                   "readers": ["worker2"]}],
                                   "base_revision": 1}))
    assert not conflicting.ok and "stale" in conflicting.error
    spec_after = h.rt.store.load_team_spec("s1")
    assert [a.id for a in spec_after.agents] == ["leader", "b", "worker1"], \
        "no partial application from the rejected patch"
    assert h.rt.store.current_revision("s1") == 2


async def test_mid_execution_supplement_reaches_running_leader(harness_factory):
    """T14: while B keeps running, the Leader takes in a supplement at its next
    model call and only the affected member adjusts."""
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "long job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",),
                ("inbox",),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",),
                ("call", "signal_done", {"summary": "adjusted to the supplement"}),
                ("end",)],
        b=[("sleep", 1.0),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
    )
    h = await harness_factory(spec, members, catalog=CATALOG)
    await h.user("start the long job")
    deadline = time.time() + 5
    while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
               if r.agent_id == "b"]:
        assert time.time() < deadline
        await asyncio.sleep(0.01)
    h.rt.user_message("补充：结果要包含一句话总结", supplement=True)
    await h.settle(15)
    observed = [i for i in members["leader"].observed_inbox
                if i.get("kind") == "user_message"]
    assert observed and observed[-1]["payload"].get("supplement") is True, \
        "the Leader must receive the supplement at its next model call"
    b_receipts = members["b"].results
    assert all("总结" not in str(r.result) for r in b_receipts), \
        "the unaffected member was not interrupted with the supplement"
    assert h.rt.store.get_session("s1")["goal_state"] == "done"
