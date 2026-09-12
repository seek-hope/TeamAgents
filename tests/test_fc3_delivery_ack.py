"""F-C3/RT-05: terminal run status + delivery acknowledgement are one transaction,
and only deliveries actually handed to a turn are acknowledged.

The old shape wrote the terminal status and then acknowledged a `MAX(batch_no)`
range: a crash in between left "terminal run + un-acked delivery" (the same input
was injected again by a fresh run), and the range swallowed deliveries the member
never saw. These tests pin the new semantics:

* a failure inside the finalize transaction rolls back status, member state and
  acknowledgements together - the window cannot be produced by `_finalize`;
* a turn that never reached a runner acknowledges nothing; the deliveries stay
  pending and are handed to exactly one later run (not lost, not duplicated);
* the storage-level acknowledgement never touches deliveries outside the given
  id set (no batch range).
"""

from __future__ import annotations

import pytest

from conftest import leader, member, scripts, spec_of, task_channel
from teamagents.agents import FakeMember, TurnOutcome
from teamagents.models import (
    ActionKind,
    AgentStatus,
    EventKind,
    TeamAction,
    TeamEvent,
    TurnStatus,
)
from teamagents.runtime import fake_session
from teamagents.storage import Store


def _action(action_id: str, actor: str, kind: ActionKind, payload: dict) -> TeamAction:
    return TeamAction(action_id=action_id, session_id="s1", actor_id=actor,
                      kind=kind, payload=payload)


def _runs(rt, agent_id: str) -> list:
    return [r for r in rt.store.runs_for_session("s1") if r.agent_id == agent_id]


def _delivery_status(rt, agent_id: str) -> list[str]:
    rows = rt.store.conn.execute(
        "SELECT status FROM deliveries WHERE agent_id=? ORDER BY delivery_id",
        (agent_id,)).fetchall()
    return [r["status"] for r in rows]


def _user_message(rt, action_id: str, text: str) -> None:
    receipt = rt.control.submit(TeamAction(
        action_id=action_id, session_id=rt.session_id, actor_id="user",
        kind=ActionKind.USER_MESSAGE, payload={"text": text}))
    assert receipt.ok, receipt.error


def _deliveries(store) -> list[tuple[int, int, str]]:
    rows = store.conn.execute(
        "SELECT delivery_id, batch_no, status FROM deliveries ORDER BY delivery_id").fetchall()
    return [(r["delivery_id"], r["batch_no"], r["status"]) for r in rows]


async def test_ack_failure_rolls_back_terminal_state_then_converges_once(tmp_path,
                                                                         monkeypatch):
    """A failing ack must not leave a terminal run behind (F-C3 atomicity)."""
    spec = spec_of(leader())
    members = scripts(leader=[("end",)])
    rt = fake_session(tmp_path, spec, members, session_id="s1")
    try:
        _user_message(rt, "m1", "hello")
        run = rt.store.runs_for_session("s1", [TurnStatus.QUEUED])[0]

        def _raiser(self, *args, **kwargs):
            raise RuntimeError("ack write failed")

        monkeypatch.setattr(Store, "ack_deliveries_exact", _raiser)
        with pytest.raises(RuntimeError):
            await rt._execute(run)

        # the whole finalize transaction rolled back: no "terminal + un-acked" window
        assert rt.store.get_run(run.run_id).status is TurnStatus.RUNNING
        assert rt.store.agent_status("s1", "leader") is AgentStatus.BUSY
        assert _deliveries(rt.store) == [(1, 1, "pending")]
        # and the still-running turn keeps its own input, so no duplicate run appears
        rt.control.schedule()
        assert len(rt.store.runs_for_session("s1")) == 1

        # the executor retries the run once the acknowledgement works again
        monkeypatch.undo()
        rt.runners["leader"] = members["leader"]
        await rt._execute(rt.store.get_run(run.run_id))

        assert rt.store.get_run(run.run_id).status is TurnStatus.COMPLETED
        assert rt.store.agent_status("s1", "leader") is AgentStatus.IDLE
        assert _deliveries(rt.store) == [(1, 1, "applied")]
        rt.control.schedule()
        runs = rt.store.runs_for_session("s1")
        assert len(runs) == 1, f"input re-injected by a fresh run: {runs}"
        assert rt.store.pending_deliveries("s1", "leader") == []
    finally:
        rt.store.close()


async def test_uninjected_deliveries_stay_pending_and_are_delivered_once(tmp_path):
    """A turn that never reached a runner acknowledges nothing (RT-05 exactness)."""
    spec = spec_of(leader())
    rt = fake_session(tmp_path, spec, {}, session_id="s1")  # no runner for leader
    try:
        _user_message(rt, "m1", "hello")
        run1 = rt.store.runs_for_session("s1", [TurnStatus.QUEUED])[0]
        _user_message(rt, "m2", "second")  # queues onto the same (not yet started) run
        run1 = rt.store.get_run(run1.run_id)
        assert run1.input_delivery_ids == [1, 2]

        await rt._execute(run1)  # fails before a runner segment: nothing was injected
        assert rt.store.get_run(run1.run_id).status is TurnStatus.FAILED
        assert _deliveries(rt.store) == [(1, 1, "pending"), (2, 2, "pending")]

        # the inputs are handed to exactly one fresh run, not swallowed by an ack range
        rt.runners["leader"] = FakeMember("leader", [("inbox",), ("end",)])
        run2 = [r for r in rt.store.runs_for_session("s1", [TurnStatus.QUEUED])
                if r.run_id != run1.run_id]
        assert len(run2) == 1, f"expected exactly one follow-up run: {run2}"
        await rt._execute(run2[0])

        assert _deliveries(rt.store) == [(1, 1, "applied"), (2, 2, "applied")]
        observed = [(i.get("payload") or {}).get("text")
                    for i in rt.runners["leader"].observed_inbox]
        assert observed == ["hello", "second"], observed
        rt.control.schedule()
        assert len(rt.store.runs_for_session("s1")) == 2
    finally:
        rt.store.close()


async def test_manual_terminal_unacked_input_is_injected_exactly_once(tmp_path):
    """Reconstruct the old crash window by hand (probe C-b): the input must reach
    the member exactly once - never lost, and never re-injected twice."""
    spec = spec_of(leader())
    rt = fake_session(tmp_path, spec, {"leader": FakeMember("leader", [("inbox",), ("end",)])},
                      session_id="s1")
    try:
        _user_message(rt, "m1", "hello")
        run1 = rt.store.runs_for_session("s1", [TurnStatus.QUEUED])[0]
        # the old ordering committed the terminal status first; the ack never landed
        rt.store.set_run_status(run1.run_id, TurnStatus.COMPLETED)
        assert _deliveries(rt.store) == [(1, 1, "pending")]

        rt.control.schedule()
        runs = rt.store.runs_for_session("s1")
        assert len(runs) == 2, "the un-acked input was silently dropped"
        run2 = [r for r in runs if r.run_id != run1.run_id][0]
        assert run2.input_delivery_ids == [1]

        await rt._execute(run2)
        assert _deliveries(rt.store) == [(1, 1, "applied")]
        observed = [(i.get("payload") or {}).get("text")
                    for i in rt.runners["leader"].observed_inbox]
        assert observed == ["hello"], observed
        rt.control.schedule()
        assert len(rt.store.runs_for_session("s1")) == 2
    finally:
        rt.store.close()


def test_ack_deliveries_exact_never_swallows_unlisted_pending(tmp_path):
    """Storage contract: acknowledging ids never touches other pending rows."""
    store = Store(tmp_path / "s1.db")
    try:
        with store.tx():
            for i in (1, 2, 3):
                store.append_event(TeamEvent(
                    event_id=f"evt{i}", session_id="s1", actor_id="user",
                    kind=EventKind.USER_MESSAGE, payload={"text": str(i)}))
                store.create_delivery("s1", "leader", f"evt{i}", batch_no=i)

        assert store.ack_deliveries_exact("s1", "leader", [1]) == 1
        assert _deliveries(store) == [(1, 1, "applied"), (2, 2, "pending"),
                                      (3, 3, "pending")]
        assert store.applied_batch("s1", "leader") == 1

        # a later acknowledgement advances the cursor only by its own batches
        assert store.ack_deliveries_exact("s1", "leader", [3]) == 1
        assert _deliveries(store) == [(1, 1, "applied"), (2, 2, "pending"),
                                      (3, 3, "applied")]
        assert store.applied_batch("s1", "leader") == 3

        # re-acknowledging an already applied id is a no-op
        assert store.ack_deliveries_exact("s1", "leader", [1, 3]) == 0
    finally:
        store.close()


async def test_parked_run_not_woken_by_its_own_input_but_by_new_supplement(tmp_path):
    """A parked run must not treat its own input as new user input.

    Regression guard for `seen = waiting.input_delivery_ids`: the user message
    that the parked run already carries must not resume it, while a fresh
    supplement (a new delivery) still wakes it.
    """
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[
            ("call", "assign_task", {"assignee": "b", "description": "job"}),
            ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
            ("wait",),
            ("inbox",),
            ("end",),
        ],
        b=[("end",)],
    )
    rt = fake_session(tmp_path, spec, members, session_id="s1")
    try:
        assert rt.control.submit(_action(
            "m1", "user", ActionKind.USER_MESSAGE, {"text": "start"})).ok
        run = _runs(rt, "leader")[0]
        await rt._execute(run)

        run = rt.store.get_run(run.run_id)
        assert run.status is TurnStatus.WAITING_TASK
        assert run.waiting_on, "the run must be parked on the pending task"
        assert _delivery_status(rt, "leader") == ["pending"]

        # its own input is not news: the run stays parked
        rt.control.schedule()
        assert rt.store.get_run(run.run_id).status is TurnStatus.WAITING_TASK, (
            "a parked run was woken by the input it already carries")
        assert len(_runs(rt, "leader")) == 1

        # a fresh supplement is new input: it resumes the parked run
        assert rt.control.submit(_action(
            "m2", "user", ActionKind.USER_SUPPLEMENT, {"text": "one more thing"})).ok
        assert rt.store.get_run(run.run_id).status is TurnStatus.RUNNING

        await rt._execute(rt.store.get_run(run.run_id))
        assert rt.store.get_run(run.run_id).status is TurnStatus.COMPLETED
        observed = [(i.get("payload") or {}).get("text")
                    for i in members["leader"].observed_inbox]
        assert observed == ["start", "one more thing"], observed
        # both inputs were injected into the resumed segment: both are acknowledged
        assert _delivery_status(rt, "leader") == ["applied", "applied"]
        rt.control.schedule()
        assert len(_runs(rt, "leader")) == 1
    finally:
        rt.store.close()
