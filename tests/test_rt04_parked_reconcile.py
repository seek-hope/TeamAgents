"""RT-04/AD-7 + F-C4: what happens to turns that were in flight when the process died.

A parked turn (WAITING_TASK / WAITING_APPROVAL) is only resumable if the runner's own
checkpoint still backs that pause; otherwise it converges through `_finalize` (no
whole-input replay as "new message", no blind retry of external side effects) and its
deliveries are acknowledged exactly once (F-C3 hand-off ledger).
"""

from __future__ import annotations

import pytest

from conftest import leader, member, spec_of, task_channel
from teamagents.agents import TurnOutcome
from teamagents.control import Control, EventDraft
from teamagents.models import (
    ActionKind,
    AgentStatus,
    ApprovalRequest,
    ApprovalStatus,
    EventKind,
    Task,
    TaskStatus,
    TeamAction,
    TeamEvent,
    TurnRun,
    TurnStatus,
    UserConfig,
    new_id,
)
from teamagents.runtime import SessionRuntime
from teamagents.storage import Store


def build_store(tmp_path, spec):
    store = Store(tmp_path / "s1.db")
    store.create_session("s1", str(tmp_path), "approved_scope")
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)
    return store


def insert_pending_approval(store, run_id: str, agent_id: str) -> ApprovalRequest:
    req = ApprovalRequest(approval_id=new_id("appr"), session_id="s1", agent_id=agent_id,
                          run_id=run_id, tool_call_id=f"{run_id}:call-1",
                          operation_hash="op-hash", requested_scope={"tool": "shell"},
                          policy_revision=1)
    store.insert_approval(req)
    return req


def insert_input_delivery(store, agent_id: str) -> int:
    with store.tx():
        store.append_event(TeamEvent(event_id=new_id("evt"), session_id="s1", actor_id="user",
                                     kind=EventKind.USER_MESSAGE, payload={"text": "hello"}))
    event = store.events("s1")[-1]
    return store.create_delivery("s1", agent_id, event["event_id"], batch_no=1)


def delivery_status(store, agent_id: str) -> list[str]:
    rows = store.conn.execute(
        "SELECT status FROM deliveries WHERE agent_id=? ORDER BY delivery_id",
        (agent_id,)).fetchall()
    return [r["status"] for r in rows]


class ParkedRunner:
    """Internal-backend stand-in: `reconcile` probes a checkpoint and restores the
    pause marker, exactly like DeepAgentsRunner does from a LangGraph checkpoint."""

    def __init__(self, checkpoint_state: TurnStatus | None):
        self.checkpoint_state = checkpoint_state
        self.paused: dict[str, str] = {}
        self.state: dict[str, TurnStatus] = {}
        self.calls: list[tuple[str, str | None]] = []

    def query_state(self, run_id: str) -> TurnStatus | None:
        return self.state.get(run_id)

    async def reconcile(self, run: TurnRun) -> TurnStatus | None:
        if self.checkpoint_state in (TurnStatus.WAITING_APPROVAL, TurnStatus.WAITING_TASK):
            self.paused[run.run_id] = ("approval"
                                       if self.checkpoint_state is TurnStatus.WAITING_APPROVAL
                                       else "waiting")
        return self.checkpoint_state

    async def start_or_resume(self, run, view, gateway, wake):
        # the real runner can only resume (instead of replaying the rendered view as
        # a brand-new message) when the pause marker survived the restart
        mode = "resume" if run.run_id in self.paused else "new_message"
        self.calls.append((mode, wake.reason if wake else None))
        self.state[run.run_id] = TurnStatus.COMPLETED
        return TurnOutcome(status=TurnStatus.COMPLETED)

    async def request_interrupt(self, run_id: str) -> TurnStatus:
        return TurnStatus.CANCELLED

    def deliver_mid_turn(self, run_id: str, items: list[dict]) -> None:
        pass


# ----------------------------------------------------------- F-C4: RUNNING rows


async def test_reconcile_converges_finished_turn_and_applies_completion_request(tmp_path):
    """probe D: a terminal checkpoint must converge like any other turn end."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    store = build_store(tmp_path, spec)
    store.insert_task("s1", Task(task_id="t1", requester="leader", assignee="b",
                                 description="job", status=TaskStatus.RUNNING))
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b", task_id="t1",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.RUNNING))
    store.set_agent_status("s1", "b", AgentStatus.BUSY)
    store.record_completion_request("run_b", "t1", ["ref:one"], "finished before crash")
    rt = SessionRuntime(store, "s1", UserConfig(),
                        runners={"b": ParkedRunner(TurnStatus.COMPLETED)})

    await rt.reconcile()

    assert store.get_run("run_b").status is TurnStatus.COMPLETED
    task = store.get_task("t1")
    assert task.status is TaskStatus.SUCCEEDED and task.result_refs == ["ref:one"]
    assert store.agent_status("s1", "b") is AgentStatus.IDLE
    kinds = [e["kind"] for e in store.events("s1")]
    assert "task_completed" in kinds and "run_completed" in kinds
    assert store.completion_request("run_b") is not None  # consumed, kept for replay
    blockers = Control(store, "s1")._completion_blockers(spec, current_run=None)
    assert not any("unfinished tasks" in b for b in blockers), blockers
    store.close()


async def test_reconcile_keeps_confirmed_running_turn_running(tmp_path):
    spec = spec_of(leader())
    store = build_store(tmp_path, spec)
    store.insert_run(TurnRun(run_id="run_l", session_id="s1", agent_id="leader",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.RUNNING))
    store.set_agent_status("s1", "leader", AgentStatus.BUSY)
    runner = ParkedRunner(TurnStatus.RUNNING)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"leader": runner})

    await rt.reconcile()

    assert store.get_run("run_l").status is TurnStatus.RUNNING
    assert store.agent_status("s1", "leader") is AgentStatus.BUSY
    store.close()


# --------------------------------------------- RT-04/AD-7: parked rows, no checkpoint


async def test_restart_converges_unverifiable_parked_approval(tmp_path):
    """AD-7: an external turn parked on an approval cannot be verified after a
    restart - converge honestly, never replay the input as a new message."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    store = build_store(tmp_path, spec)
    delivery_id = insert_input_delivery(store, "b")
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.WAITING_APPROVAL,
                             input_delivery_ids=[delivery_id],
                             external_turn_id="codex-turn-1"))
    store.set_agent_status("s1", "b", AgentStatus.WAITING)
    req = insert_pending_approval(store, "run_b", "b")
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"b": ParkedRunner(None)})

    await rt.reconcile()

    assert store.get_run("run_b").status is TurnStatus.OUTCOME_UNKNOWN
    assert store.agent_status("s1", "b") is AgentStatus.IDLE
    assert store.get_approval(req.approval_id).status is ApprovalStatus.EXPIRED
    kinds = [e["kind"] for e in store.events("s1")]
    assert "run_failed" in kinds and "approval_decided" in kinds
    # the input is acknowledged, not re-delivered into a fresh turn
    assert delivery_status(store, "b") == ["applied"]
    assert len(store.runs_for_session("s1")) == 1
    # no stuck blocker: only the explicit unknown-outcome decision point remains
    blockers = Control(store, "s1")._completion_blockers(spec, current_run=None)
    assert not any("active turns" in b for b in blockers), blockers
    assert not any("pending approvals" in b for b in blockers), blockers
    assert any("outcome-unknown" in b for b in blockers), blockers
    # a late decision on the expired approval is a clean refusal, not a replay
    late = Control(store, "s1").submit(TeamAction(
        action_id="late-decision", session_id="s1", actor_id="user",
        kind=ActionKind.APPROVAL_DECISION,
        payload={"approval_id": req.approval_id, "decision": "once"}))
    assert not late.ok and "EXPIRED" in late.error
    store.close()

async def test_restart_converges_parked_run_without_a_runner(tmp_path):
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    store = build_store(tmp_path, spec)
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.WAITING_APPROVAL))
    store.set_agent_status("s1", "b", AgentStatus.WAITING)
    req = insert_pending_approval(store, "run_b", "b")
    rt = SessionRuntime(store, "s1", UserConfig(), runners={})

    await rt.reconcile()

    assert store.get_run("run_b").status is TurnStatus.OUTCOME_UNKNOWN
    assert store.get_approval(req.approval_id).status is ApprovalStatus.EXPIRED
    assert store.pending_approvals("s1") == []
    store.close()


# ------------------------------------------- RT-04: parked rows with a checkpoint


async def test_restart_restores_parked_approval_and_resumes_on_decision(tmp_path):
    """Internal backend: the checkpoint still shows the interrupt, so the pause is
    restored and the approval decision resumes the turn (no new-message replay)."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    store = build_store(tmp_path, spec)
    delivery_id = insert_input_delivery(store, "b")
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.WAITING_APPROVAL,
                             input_delivery_ids=[delivery_id]))
    store.set_agent_status("s1", "b", AgentStatus.WAITING)
    req = insert_pending_approval(store, "run_b", "b")
    runner = ParkedRunner(TurnStatus.WAITING_APPROVAL)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"b": runner})

    await rt.reconcile()

    assert store.get_run("run_b").status is TurnStatus.WAITING_APPROVAL
    assert runner.paused["run_b"] == "approval"
    assert store.get_approval(req.approval_id).status is ApprovalStatus.PENDING

    receipt = rt.submit(TeamAction(action_id="decide-1", session_id="s1", actor_id="user",
                                   kind=ActionKind.APPROVAL_DECISION,
                                   payload={"approval_id": req.approval_id,
                                            "decision": "once"}))
    assert receipt.ok, receipt.error
    assert store.get_run("run_b").status is TurnStatus.RUNNING

    await rt._execute(store.get_run("run_b"))

    assert runner.calls == [("resume", "approval")], runner.calls
    assert store.get_run("run_b").status is TurnStatus.COMPLETED
    assert delivery_status(store, "b") == ["applied"]
    store.close()


async def test_restart_keeps_waiting_task_and_resumes_when_result_lands(tmp_path):
    """Internal backend: a wait is durable (waiting_on is in the DB); the run is
    resumable and wakes when the waited-on task finishes while the process is down."""
    spec = spec_of(leader(), member("b"), member("c"),
                   channels=[task_channel("leader", ["b", "c"])])
    store = build_store(tmp_path, spec)
    store.insert_task("s1", Task(task_id="t1", requester="leader", assignee="c",
                                 description="slow job", status=TaskStatus.PENDING))
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.WAITING_TASK, waiting_on=["t1"]))
    store.set_agent_status("s1", "b", AgentStatus.WAITING)
    runner = ParkedRunner(TurnStatus.WAITING_TASK)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"b": runner})

    await rt.reconcile()

    assert store.get_run("run_b").status is TurnStatus.WAITING_TASK
    assert runner.paused["run_b"] == "waiting"

    # the waited-on job finished while the process was down: the task completion
    # pushes a wake delivery to the waiter and scheduling resumes the parked run
    control = Control(store, "s1")
    with store.tx():
        assert store.compare_and_set_task("t1", TaskStatus.PENDING, TaskStatus.SUCCEEDED)
    control.emit([EventDraft(kind=EventKind.TASK_COMPLETED,
                             payload={"task_id": "t1", "status": "SUCCEEDED",
                                      "assignee": "c", "requester": "leader"},
                             task_id="t1", push=["b"])])

    assert store.get_run("run_b").status is TurnStatus.RUNNING, "wait must wake on result"
    await rt._execute(store.get_run("run_b"))

    assert runner.calls == [("resume", "task_results")], runner.calls
    assert store.get_run("run_b").status is TurnStatus.COMPLETED
    store.close()


async def test_restart_converges_waiting_task_whose_checkpoint_is_gone(tmp_path):
    spec = spec_of(leader(), member("b"), member("c"),
                   channels=[task_channel("leader", ["b", "c"])])
    store = build_store(tmp_path, spec)
    store.insert_task("s1", Task(task_id="t1", requester="leader", assignee="c",
                                 description="slow job", status=TaskStatus.PENDING))
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.WAITING_TASK, waiting_on=["t1"]))
    store.set_agent_status("s1", "b", AgentStatus.WAITING)
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"b": ParkedRunner(None)})

    await rt.reconcile()

    assert store.get_run("run_b").status is TurnStatus.OUTCOME_UNKNOWN
    assert store.agent_status("s1", "b") is AgentStatus.IDLE
    blockers = Control(store, "s1")._completion_blockers(spec, current_run=None)
    assert not any("WAITING" in b for b in blockers), blockers
    assert any("outcome-unknown operations: run_b" in b for b in blockers), blockers
    store.close()


@pytest.mark.parametrize("status", [TurnStatus.COMPLETED, TurnStatus.FAILED,
                                    TurnStatus.CANCELLED, TurnStatus.OUTCOME_UNKNOWN])
async def test_restart_converges_parked_run_by_checkpoint_verdict(tmp_path, status):
    """The checkpoint is authoritative: a parked row whose checkpoint says the turn
    actually ended converges with that verdict (and expires its approvals)."""
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    store = build_store(tmp_path, spec)
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b",
                             config_revision=1, topology_revision=1,
                             status=TurnStatus.WAITING_APPROVAL))
    store.set_agent_status("s1", "b", AgentStatus.WAITING)
    req = insert_pending_approval(store, "run_b", "b")
    rt = SessionRuntime(store, "s1", UserConfig(), runners={"b": ParkedRunner(status)})

    await rt.reconcile()

    assert store.get_run("run_b").status is status
    assert store.agent_status("s1", "b") is AgentStatus.IDLE
    assert store.get_approval(req.approval_id).status is ApprovalStatus.EXPIRED
    store.close()
