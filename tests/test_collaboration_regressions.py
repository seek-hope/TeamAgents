"""Cooperation regressions: dispatch, safe topology boundaries and recovery."""
import asyncio

import pytest

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.agents import FakeMember, TurnOutcome
from teamagents.models import (ActionKind, AgentStatus, ModelProfile, TaskStatus,
                               TeamAction, TurnRun, TurnStatus, UserConfig, new_id)
from teamagents.runtime import fake_session


def action(rt, kind, payload, actor="leader", run_id=None):
    return rt.submit(TeamAction(action_id=new_id("test"), session_id="s1",
                               actor_id=actor, kind=kind, payload=payload, run_id=run_id))


@pytest.fixture
def rt(tmp_path):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"]),
                                                  msg_channel("b", ["leader"])])
    runtime = fake_session(tmp_path, spec, scripts(leader=[], b=[]),
                           catalog=UserConfig(models={"test": ModelProfile(
                               provider="openai", model="test")}))
    yield runtime
    runtime.store.close()


def live_run(rt, agent="b", task_id=None):
    run = TurnRun(run_id=new_id("run"), session_id="s1", agent_id=agent,
                  task_id=task_id, config_revision=1, topology_revision=1,
                  status=TurnStatus.RUNNING)
    rt.store.insert_run(run)
    rt.store.set_agent_status("s1", agent, AgentStatus.BUSY)
    return run


async def test_tool_dispatch_starts_worker_before_leader_finishes(tmp_path):
    started = asyncio.Event()

    class WaitingLeader(FakeMember):
        async def start_or_resume(self, run, view, gateway, wake):
            if run.run_id != leader_run.run_id:
                return TurnOutcome(TurnStatus.COMPLETED)
            await asyncio.sleep(0.05)
            assert gateway.call("assign_task", {"assignee": "b", "description": "start now"}, "assign").ok
            await asyncio.wait_for(started.wait(), 1)
            return TurnOutcome(TurnStatus.COMPLETED)

    class Worker(FakeMember):
        async def start_or_resume(self, run, view, gateway, wake):
            started.set()
            assert gateway.call("send_message", {"target": "leader", "text": "working"}, "progress").ok
            assert runtime.runners["leader"]._mid_turn.get(leader_run.run_id), "mid-turn message must be delivered now"
            assert gateway.call("complete_task", {"task_id": run.task_id}, "done").ok
            return TurnOutcome(TurnStatus.COMPLETED)

    runtime = fake_session(tmp_path, spec_of(leader(), member("b"), channels=[
        task_channel("leader", ["b"]), msg_channel("b", ["leader"])]),
        {"leader": WaitingLeader("leader"), "b": Worker("b")})
    runtime.user_message("go")
    leader_run = runtime.store.runs_for_session("s1")[0]
    await runtime.start()
    try:
        await asyncio.wait_for(started.wait(), 2)
        assert await runtime.settle(3)
        assert runtime.store.get_run(leader_run.run_id).status == TurnStatus.COMPLETED
        assert runtime.store.tasks_for_session("s1")[0].status == TaskStatus.SUCCEEDED
    finally:
        await runtime.close()
        runtime.store.close()


def test_member_proposal_uses_boundary_and_records_decider(rt):
    run = live_run(rt)
    proposal = action(rt, ActionKind.PROPOSE_TEAM_CHANGE, {"operations": [
        {"op": "update_agent", "agent_id": "b", "changes": {"instructions": "updated"}}]}, "b")
    result = action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"patch_id": proposal.result["patch_id"]})
    assert result.ok and result.result["status"] == "WAITING_BOUNDARY"
    assert rt.store.load_team_spec("s1").agent("b").instructions != "updated"
    rt._finalize(run, TurnOutcome(TurnStatus.COMPLETED))
    patch = rt.store.get_patch(proposal.result["patch_id"])
    assert patch.status == "APPLIED" and patch.decided_by == "leader"
    assert rt.store.load_team_spec("s1").agent("b").instructions == "updated"


def test_stale_proposal_is_rejected(rt):
    proposal = action(rt, ActionKind.PROPOSE_TEAM_CHANGE, {"operations": [
        {"op": "update_agent", "agent_id": "b", "changes": {"instructions": "old"}}]}, "b")
    assert action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": 1, "operations": [
        {"op": "update_agent", "agent_id": "b", "changes": {"instructions": "new"}}]}).ok
    result = action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"patch_id": proposal.result["patch_id"]})
    assert not result.ok and "stale" in result.error
    assert rt.store.load_team_spec("s1").agent("b").instructions == "new"


def test_removed_identity_cannot_be_reused(rt):
    old = rt.store.load_team_spec("s1").agent("b").model_dump(mode="json")
    assert action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": 1, "operations": [
        {"op": "remove_agent", "agent_id": "b"}]}).ok
    result = action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": 2, "operations": [
        {"op": "add_agent", "agent": old}]})
    assert not result.ok and "id" in result.error
    old["id"] = "b-new"
    assert action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": 2, "operations": [
        {"op": "add_agent", "agent": old}]}).ok
    assert rt.store.agent_status("s1", "b-new") == AgentStatus.IDLE


async def test_recovery_applies_completed_task_and_wakes_requester(rt):
    task = action(rt, ActionKind.ASSIGN_TASK, {"assignee": "b", "description": "recover"})
    run = next(r for r in rt.store.runs_for_session("s1") if r.agent_id == "b")
    rt.store.set_run_status(run.run_id, TurnStatus.RUNNING)
    rt.store.compare_and_set_task(run.task_id, TaskStatus.PENDING, TaskStatus.RUNNING)
    rt.store.set_agent_status("s1", "b", AgentStatus.BUSY)
    assert action(rt, ActionKind.COMPLETE_TASK, {"task_id": task.result["task_id"]}, "b", run.run_id).ok
    rt.runners["b"].state[run.run_id] = TurnStatus.COMPLETED
    await rt.reconcile()
    assert rt.store.get_task(run.task_id).status == TaskStatus.SUCCEEDED
    assert rt.store.agent_status("s1", "b") == AgentStatus.IDLE
    assert any(e["kind"] == "task_completed" for e in rt.store.events("s1"))


def test_blocked_task_wakes_waiter_for_intervention(rt):
    task = action(rt, ActionKind.ASSIGN_TASK, {"assignee": "b", "description": "cannot finish"})
    run = live_run(rt, "leader")
    assert action(rt, ActionKind.WAIT_FOR_TASKS, {"task_ids": [task.result["task_id"]]}, "leader", run.run_id).ok
    worker = next(r for r in rt.store.runs_for_session("s1") if r.agent_id == "b")
    rt._finalize(worker, TurnOutcome(TurnStatus.COMPLETED))
    assert rt.store.get_task(worker.task_id).status == TaskStatus.BLOCKED
    assert rt.store.get_run(run.run_id).status == TurnStatus.RUNNING


async def test_native_cancel_waits_for_tool_and_prevents_next_side_effect(tmp_path):
    from langchain_core.tools import tool
    from scripted_model import ScriptedChatModel, ai_text, ai_tool
    from test_p3_deepagents_runner import build_runtime

    entered, release = asyncio.Event(), asyncio.Event()

    @tool
    async def slow_tool() -> str:
        """Wait for a controlled operation to finish."""
        entered.set()
        await release.wait()
        return "finished"

    model = ScriptedChatModel(script=[ai_tool("slow_tool", {}),
        ai_tool("write_file", {"file_path": "/must-not-exist", "content": "bad"}), ai_text("done")])
    runtime = build_runtime(tmp_path, spec_of(leader()), {"leader": model})
    runtime.runners["leader"].extra_tools.append(slow_tool)
    await runtime.start()
    try:
        runtime.user_message("go")
        await asyncio.wait_for(entered.wait(), 3)
        run = runtime.store.runs_for_session("s1", ["RUNNING"])[0]
        assert action(runtime, ActionKind.CANCEL_RUN, {"run_id": run.run_id}, "user").ok
        await asyncio.sleep(0.05)
        assert runtime.store.get_run(run.run_id).status == TurnStatus.RUNNING
        release.set()
        assert await runtime.settle(3)
        assert runtime.store.get_run(run.run_id).status == TurnStatus.CANCELLED
        assert not (tmp_path / "work" / "must-not-exist").exists()
        assert len(model.calls) == 1
    finally:
        release.set()
        await runtime.close()
        runtime.store.close()


def test_leader_can_create_delegate_and_cancel_without_waiting_for_itself(rt):
    from teamagents.agents import ToolGateway
    run = live_run(rt, "leader")
    gateway = ToolGateway(rt.control, "s1", "leader", run.run_id, rt.approvals, submit=rt.submit)
    result = gateway.call("apply_topology_patch", {"base_revision": 1, "operations": [{
        "op": "add_agent", "agent": member("c").model_dump(mode="json"),
        "channels": [task_channel("leader", ["c"]).model_dump(mode="json")]}]}, "add")
    assert result.ok and result.result["status"] == "APPLIED"
    assigned = gateway.call("assign_task", {"assignee": "c", "description": "work"}, "assign")
    assert assigned.ok
    task_id = assigned.result["task_id"]
    rt.store.compare_and_set_task(task_id, TaskStatus.PENDING, TaskStatus.BLOCKED)
    for queued in rt.store.runs_for_session("s1", ["QUEUED"]):
        rt.store.set_run_status(queued.run_id, TurnStatus.COMPLETED)
    cancelled = gateway.call("cancel_task", {"task_id": task_id}, "cancel")
    assert cancelled.ok and rt.store.get_task(task_id).status == TaskStatus.CANCELLED
    other = ToolGateway(rt.control, "s1", "b", live_run(rt).run_id, rt.approvals)
    assert not other.call("cancel_task", {"task_id": task_id}, "deny").ok


def test_waiting_patch_conflict_cannot_overwrite_new_decision(rt):
    run = live_run(rt)
    first = action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": 1, "operations": [
        {"op": "update_agent", "agent_id": "b", "changes": {"instructions": "stale"}}]})
    assert first.result["status"] == "WAITING_BOUNDARY"
    assert action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": 1, "operations": [
        {"op": "update_agent", "agent_id": "leader", "changes": {"instructions": "new"}}]}).ok
    rt._finalize(run, TurnOutcome(TurnStatus.COMPLETED))
    assert rt.store.get_patch(first.result["patch_id"]).status == "FAILED"
    assert rt.store.load_team_spec("s1").agent("b").instructions != "stale"
    assert rt.store.agent_status("s1", "b") == AgentStatus.IDLE
    assert any(e["kind"] == "topology_rejected" for e in rt.store.events("s1"))


def test_final_event_failure_rolls_back_task_run_and_ack(rt, monkeypatch):
    assigned = action(rt, ActionKind.ASSIGN_TASK, {"assignee": "b", "description": "atomic"})
    run = next(r for r in rt.store.runs_for_session("s1") if r.agent_id == "b")
    rt.store.set_run_status(run.run_id, TurnStatus.RUNNING)
    assert action(rt, ActionKind.COMPLETE_TASK, {"task_id": assigned.result["task_id"]}, "b", run.run_id).ok
    rt._offered[run.run_id] = set(run.input_delivery_ids)
    append = rt.store.append_event

    def fail(event):
        if event.kind == EventKind.TASK_COMPLETED:
            raise RuntimeError("simulated crash at result event")
        return append(event)

    from teamagents.models import EventKind
    monkeypatch.setattr(rt.store, "append_event", fail)
    with pytest.raises(RuntimeError, match="simulated crash"):
        rt._finalize(run, TurnOutcome(TurnStatus.COMPLETED))
    assert rt.store.get_task(run.task_id).status == TaskStatus.PENDING
    assert rt.store.get_run(run.run_id).status == TurnStatus.RUNNING
    assert rt.store.pending_deliveries("s1", "b")
    assert rt._offered[run.run_id]
    monkeypatch.setattr(rt.store, "append_event", append)
    rt._finalize(run, TurnOutcome(TurnStatus.COMPLETED))
    assert rt.store.get_task(run.task_id).status == TaskStatus.SUCCEEDED


def test_remove_member_keeps_other_channel_targets_and_observer_scope(rt):
    from teamagents.models import ObserverSpec
    spec = rt.store.load_team_spec("s1")
    spec.agents.append(member("c"))
    spec.channels[0].targets.append("c")
    spec.observers = [ObserverSpec(agent_id="leader", subjects=["b", "c"],
                                   event_types=["task_completed"], payload_scope="status")]
    revision = rt.store.save_team_spec("s1", spec)
    rt.store.ensure_agent("s1", "c")
    result = action(rt, ActionKind.APPLY_TOPOLOGY_PATCH, {"base_revision": revision,
        "operations": [{"op": "remove_agent", "agent_id": "b"}]})
    assert result.ok, result.error
    after = rt.store.load_team_spec("s1")
    assert after.can_delegate("leader", "c")
    assert after.observers[0].subjects == ["c"]


@pytest.mark.parametrize("operations", [
    [{"op": "remove_agent", "agent_id": "b"},
     {"op": "add_agent", "agent": member("b").model_dump(mode="json")}],
    [{"op": "update_agent", "agent_id": "b", "changes": {"id": "renamed"}}],
])
def test_identity_cannot_change_inside_patch(rt, operations):
    result = action(rt, ActionKind.APPLY_TOPOLOGY_PATCH,
                    {"base_revision": 1, "operations": operations})
    assert not result.ok and "id" in result.error
    assert rt.store.load_team_spec("s1").agent("b")
    assert rt.store.current_revision("s1") == 1


async def test_recovery_keeps_confirmed_running_member_busy(rt):
    run = live_run(rt)
    rt.runners["b"].state[run.run_id] = TurnStatus.RUNNING
    await rt.reconcile()
    assert rt.store.get_run(run.run_id).status == TurnStatus.RUNNING
    assert rt.store.agent_status("s1", "b") == AgentStatus.BUSY
