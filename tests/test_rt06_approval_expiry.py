"""RT-06: a turn that ends (or is cancelled) never keeps a PENDING approval.

Otherwise `pending_approvals` / `WAITING_APPROVAL` runs block `_completion_blockers`
forever and `signal_done` can never succeed (reproduced on the live session).
"""

from __future__ import annotations

import asyncio

import pytest

from conftest import leader, member, scripts, spec_of, task_channel
from teamagents.agents import FakeMember, TurnOutcome
from teamagents.models import (
    ActionKind,
    AgentStatus,
    ApprovalRequest,
    ApprovalStatus,
    TeamAction,
    TurnRun,
    TurnStatus,
    UserConfig,
    new_id,
)
from teamagents.runtime import SessionRuntime, fake_session
from teamagents.storage import Store


def build_store(tmp_path, spec):
    store = Store(tmp_path / "s1.db")
    store.create_session("s1", str(tmp_path), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)
    return store


async def wait_for(pred, timeout: float = 5.0, what: str = "condition"):
    deadline = asyncio.get_event_loop().time() + timeout
    while not pred():
        assert asyncio.get_event_loop().time() < deadline, f"timeout waiting for {what}"
        await asyncio.sleep(0.01)


def insert_pending_approval(store, run_id: str, agent_id: str = "leader") -> ApprovalRequest:
    req = ApprovalRequest(approval_id=new_id("appr"), session_id="s1", agent_id=agent_id,
                          run_id=run_id, tool_call_id=f"{run_id}:call-1",
                          operation_hash="op-hash", requested_scope={"tool": "shell"},
                          policy_revision=1)
    store.insert_approval(req)
    return req


# --------------------------------------------------------------------- runtime


@pytest.mark.parametrize("status", [TurnStatus.COMPLETED, TurnStatus.FAILED,
                                    TurnStatus.CANCELLED, TurnStatus.OUTCOME_UNKNOWN])
def test_finalize_terminal_expires_the_runs_pending_approval(tmp_path, status):
    spec = spec_of(leader())
    store = build_store(tmp_path, spec)
    run = TurnRun(run_id=new_id("run"), session_id="s1", agent_id="leader",
                  config_revision=1, topology_revision=1, status=TurnStatus.RUNNING)
    store.insert_run(run)
    req = insert_pending_approval(store, run.run_id)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"leader": FakeMember("leader")})

    rt._finalize(store.get_run(run.run_id), TurnOutcome(status=status))

    assert store.get_run(run.run_id).status is status
    assert store.get_approval(req.approval_id).status is ApprovalStatus.EXPIRED
    assert store.pending_approvals("s1") == []
    decided = [e for e in store.events("s1")
               if e["kind"] == "approval_decided" and "EXPIRED" in e["payload_json"]]
    assert decided, "expiry is auditable"
    store.close()


def test_finalize_waiting_approval_keeps_the_request_open(tmp_path):
    spec = spec_of(leader())
    store = build_store(tmp_path, spec)
    run = TurnRun(run_id=new_id("run"), session_id="s1", agent_id="leader",
                  config_revision=1, topology_revision=1, status=TurnStatus.RUNNING)
    store.insert_run(run)
    req = insert_pending_approval(store, run.run_id)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"leader": FakeMember("leader")})

    rt._finalize(store.get_run(run.run_id), TurnOutcome(status=TurnStatus.WAITING_APPROVAL))

    assert store.get_approval(req.approval_id).status is ApprovalStatus.PENDING
    store.close()


async def test_reconcile_outcome_unknown_expires_the_pending_approval(tmp_path):
    spec = spec_of(leader())
    store = build_store(tmp_path, spec)
    run = TurnRun(run_id=new_id("run"), session_id="s1", agent_id="leader",
                  config_revision=1, topology_revision=1, status=TurnStatus.RUNNING,
                  external_turn_id="codex-turn-1")
    store.insert_run(run)
    req = insert_pending_approval(store, run.run_id)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"leader": FakeMember("leader")})

    await rt.reconcile()

    assert store.get_run(run.run_id).status is TurnStatus.OUTCOME_UNKNOWN
    assert store.get_approval(req.approval_id).status is ApprovalStatus.EXPIRED
    assert store.pending_approvals("s1") == []
    kinds = [e["kind"] for e in store.events("s1")]
    assert "run_failed" in kinds and "approval_decided" in kinds
    store.close()


# ------------------------------------------------------------------ cancel path


async def test_cancel_run_on_a_parked_turn_expires_the_approval(tmp_path):
    spec = spec_of(leader())
    members = scripts(leader=[("call", "shell", {"command": "echo x"}), ("end",)])
    rt = fake_session(tmp_path, spec, members, require_approval={"shell"},
                      tool_executor=lambda name, args: "ok")
    await rt.start()
    try:
        rt.user_message("go")
        await wait_for(lambda: rt.store.pending_approvals("s1"), what="parked approval")
        approval = rt.store.pending_approvals("s1")[0]
        run = rt.store.get_run(approval.run_id)
        assert run.status is TurnStatus.WAITING_APPROVAL
        assert any("pending approvals" in b
                   for b in rt.control._completion_blockers(spec))

        receipt = rt.submit(TeamAction(action_id="cancel-1", session_id="s1", actor_id="user",
                                       kind=ActionKind.CANCEL_RUN,
                                       payload={"run_id": run.run_id}))

        assert receipt.ok, receipt.error
        assert rt.store.get_approval(approval.approval_id).status is ApprovalStatus.EXPIRED
        assert rt.store.pending_approvals("s1") == []
        assert rt.store.get_run(run.run_id).status is TurnStatus.CANCELLED
        assert rt.store.agent_status("s1", "leader") is AgentStatus.IDLE
        blockers = rt.control._completion_blockers(spec)
        assert not any("pending approvals" in b for b in blockers)
        assert not any("active turns" in b for b in blockers), blockers

        # deciding an expired approval is a clean refusal, not a crash
        late = rt.submit(TeamAction(action_id="late-1", session_id="s1", actor_id="user",
                                    kind=ActionKind.APPROVAL_DECISION,
                                    payload={"approval_id": approval.approval_id,
                                             "decision": "once"}))
        assert not late.ok and "EXPIRED" in (late.error or "")
        assert rt.store.get_approval(approval.approval_id).status is ApprovalStatus.EXPIRED
    finally:
        await rt.close()
        rt.store.close()


async def test_cancel_task_on_a_parked_turn_converges_run_and_task(tmp_path):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("end",)],
        b=[("call", "shell", {"command": "echo x"}), ("end",)],
    )
    rt = fake_session(tmp_path, spec, members, require_approval={"shell"},
                      tool_executor=lambda name, args: "ok")
    await rt.start()
    try:
        rt.user_message("go")
        await wait_for(lambda: rt.store.pending_approvals("s1"), what="parked approval")
        approval = rt.store.pending_approvals("s1")[0]
        task = rt.store.tasks_for_session("s1")[0]
        assert rt.store.get_run(approval.run_id).status is TurnStatus.WAITING_APPROVAL

        receipt = rt.submit(TeamAction(action_id="cancel-2", session_id="s1", actor_id="user",
                                       kind=ActionKind.CANCEL_TASK,
                                       payload={"task_id": task.task_id}))

        assert receipt.ok, receipt.error
        assert rt.store.get_approval(approval.approval_id).status is ApprovalStatus.EXPIRED
        assert rt.store.pending_approvals("s1") == []
        assert rt.store.get_run(approval.run_id).status is TurnStatus.CANCELLED
        assert rt.store.get_task(task.task_id).status == "CANCELLED"
        blockers = rt.control._completion_blockers(spec)
        assert not any("pending approvals" in b for b in blockers), blockers
        assert not any(approval.run_id in b for b in blockers), blockers
    finally:
        await rt.close()
        rt.store.close()


# ------------------------------------------------------- normal approval flows


async def test_session_approval_survives_expiry_and_is_auto_allowed(tmp_path):
    """Expiry only voids PENDING/once approvals; APPROVED_SESSION stays usable."""
    spec = spec_of(leader())
    shell = {"command": "echo x"}
    members = scripts(leader=[("call", "shell", shell), ("call", "shell", shell), ("end",)])
    rt = fake_session(tmp_path, spec, members, require_approval={"shell"},
                      tool_executor=lambda name, args: "ok")
    await rt.start()
    try:
        rt.user_message("go")
        await wait_for(lambda: rt.store.pending_approvals("s1"), what="parked approval")
        approval = rt.store.pending_approvals("s1")[0]
        decided = rt.submit(TeamAction(action_id="decide-1", session_id="s1", actor_id="user",
                                       kind=ActionKind.APPROVAL_DECISION,
                                       payload={"approval_id": approval.approval_id,
                                                "decision": "session"}))
        assert decided.ok, decided.error
        await rt.settle(10)
        assert rt.store.get_approval(approval.approval_id).status is ApprovalStatus.APPROVED_SESSION
        assert rt.store.pending_approvals("s1") == []
        assert rt.store.get_session("s1")["goal_state"] is not None
    finally:
        await rt.close()
        rt.store.close()


# ------------------------------------------------- external (Codex-like) turns


def test_control_leaves_a_live_external_parked_turn_to_the_runtime(tmp_path):
    """A parked run with an external turn id is still owned by its backend: control
    must not fake-finalize it, the runtime's cancel path stops it."""
    spec = spec_of(leader())
    store = build_store(tmp_path, spec)
    run = TurnRun(run_id=new_id("run"), session_id="s1", agent_id="leader",
                  config_revision=1, topology_revision=1, status=TurnStatus.WAITING_APPROVAL,
                  external_turn_id="codex-turn-1", cancel_requested=True)
    store.insert_run(run)
    req = insert_pending_approval(store, run.run_id)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"leader": FakeMember("leader")})

    rt.control.schedule()

    assert store.get_run(run.run_id).status is TurnStatus.WAITING_APPROVAL
    assert store.get_approval(req.approval_id).status is ApprovalStatus.PENDING
    store.close()


async def test_cancel_confirm_timeout_expires_the_approval(tmp_path, monkeypatch):
    """When the backend never confirms the stop, the run goes OUTCOME_UNKNOWN and
    its parked approval must expire with it."""
    spec = spec_of(leader())
    store = build_store(tmp_path, spec)
    run = TurnRun(run_id=new_id("run"), session_id="s1", agent_id="leader",
                  config_revision=1, topology_revision=1, status=TurnStatus.RUNNING,
                  external_turn_id="codex-turn-1", cancel_requested=True)
    store.insert_run(run)
    req = insert_pending_approval(store, run.run_id)
    # Limits only accepts positive ints; zero makes the confirm timeout immediate
    fast = spec.model_copy(update={"limits": spec.limits.model_copy(
        update={"cancel_confirm_timeout_s": 0})})
    monkeypatch.setattr(Store, "load_team_spec",
                        lambda self, session_id, revision=None: fast)

    class HangingRunner(FakeMember):
        async def request_interrupt(self, run_id):
            await asyncio.sleep(30)
            return TurnStatus.CANCELLED

    rt = SessionRuntime(store, "s1", UserConfig(), runners={"leader": HangingRunner("leader")})
    try:
        await rt._request_stop(store.get_run(run.run_id), rt.runners["leader"])
        assert store.get_run(run.run_id).status is TurnStatus.OUTCOME_UNKNOWN
        assert store.get_approval(req.approval_id).status is ApprovalStatus.EXPIRED
        assert store.pending_approvals("s1") == []
    finally:
        store.close()
