"""P0-4: an exception inside `Control._reduce` must not half-apply its writes.

The action's transaction rolls back; the failure receipt is recorded afterwards in
a clean transaction, so the same action id replays the refusal while a new action
id may retry against the clean state.
"""

from __future__ import annotations

import pytest

from conftest import leader, member, msg_channel, spec_of, task_channel
from teamagents.control import Control
from teamagents.models import ActionKind, TeamAction
from teamagents.storage import Store


def build_control(tmp_path):
    store = Store(tmp_path / "s1.db")
    store.create_session("s1", str(tmp_path), "approved_scope")
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])],
                   shared_spaces=[{"id": "main", "readers": ["leader", "b"],
                                   "writers": ["leader", "b"]}])
    store.save_team_spec("s1", spec)
    for agent in spec.agents:
        store.ensure_agent("s1", agent.id)
    return store, Control(store, "s1")


def test_reduce_failure_rolls_back_shared_entry_and_receipt_semantics(tmp_path, monkeypatch):
    store, control = build_control(tmp_path)
    original = Store.add_shared_entry

    def boom(self, entry, session_id):
        original(self, entry, session_id)              # the write happens ...
        raise RuntimeError("simulated failure after write")  # ... then the step fails

    publish = TeamAction(action_id="publish-1", session_id="s1", actor_id="b",
                         kind=ActionKind.PUBLISH_SHARED,
                         payload={"space_id": "main", "content": "partial-write"})
    with monkeypatch.context() as patch:
        patch.setattr(Store, "add_shared_entry", boom)
        receipt = control.submit(publish)

    assert not receipt.ok
    assert "simulated failure after write" in (receipt.error or "")
    assert store.shared_entries("s1", ["main"]) == [], \
        "the entry written before the exception must roll back with the attempt"
    assert store.events("s1") == [], "no event may leak from the failed attempt"

    # same action id: the recorded refusal replays, nothing runs twice
    replay = control.submit(publish)
    assert not replay.ok and replay.error == receipt.error
    assert store.shared_entries("s1", ["main"]) == []

    # new action id: retry succeeds on the clean state, exactly one entry
    retry = control.submit(TeamAction(action_id="publish-2", session_id="s1", actor_id="b",
                                      kind=ActionKind.PUBLISH_SHARED,
                                      payload={"space_id": "main", "content": "partial-write"}))
    assert retry.ok, retry.error
    entries = store.shared_entries("s1", ["main"])
    assert [e.content for e in entries] == ["partial-write"]
    store.close()


def test_reduce_failure_leaves_task_state_untouched(tmp_path, monkeypatch):
    store, control = build_control(tmp_path)
    original = Store.insert_task

    def boom(self, session_id, task):
        original(self, session_id, task)
        raise RuntimeError("boom after task insert")

    assign = TeamAction(action_id="assign-1", session_id="s1", actor_id="leader",
                        kind=ActionKind.ASSIGN_TASK,
                        payload={"assignee": "b", "description": "job"})
    with monkeypatch.context() as patch:
        patch.setattr(Store, "insert_task", boom)
        receipt = control.submit(assign)

    assert not receipt.ok and "boom after task insert" in (receipt.error or "")
    assert store.tasks_for_session("s1") == [], "the task row must roll back"
    assert not [e for e in store.events("s1") if e["kind"] == "task_created"]

    ok = control.submit(TeamAction(action_id="assign-2", session_id="s1", actor_id="leader",
                                   kind=ActionKind.ASSIGN_TASK,
                                   payload={"assignee": "b", "description": "job"}))
    assert ok.ok, ok.error
    tasks = store.tasks_for_session("s1")
    assert [t.task_id for t in tasks] == [ok.result["task_id"]]
    store.close()


@pytest.mark.parametrize("kind,payload", [
    ("send_message", {"target": "nobody", "text": "hi"}),
    ("cancel_run", {"run_id": "missing-run"}),
    ("approval_decision", {"approval_id": "missing-approval", "decision": "once"}),
])
def test_validation_refusals_still_get_a_readable_receipt(tmp_path, kind, payload):
    store, control = build_control(tmp_path)
    actor = "user" if kind in ("cancel_run", "approval_decision") else "b"
    action = TeamAction(action_id=f"x-{kind}", session_id="s1", actor_id=actor, kind=kind,
                        payload=payload)
    receipt = control.submit(action)
    assert receipt.ok is False and receipt.error
    assert store.get_action_receipt(action.action_id) is not None, "refusal is replayable"
    store.close()
