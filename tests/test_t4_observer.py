"""T4: observers get exactly the authorized events and scoped payloads, and
observation grants no send/stop/modify rights."""

from __future__ import annotations

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.models import TeamAction


async def test_t4_observer_scoped_events_no_extra_rights(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("watch"),
        channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])],
        observers=[{"agent_id": "watch", "subjects": ["b"],
                    "event_types": ["task_completed"], "payload_scope": "status",
                    "wake_policy": "on_event", "capabilities": []}],
    )
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "secret work"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id",
                                      "result_refs": ["private/out.txt"],
                                      "summary": "PRIVATE DETAILS"}),
           ("end",)],
        watch=[("inbox",), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("do the secret work")
    await h.settle()

    observed = members["watch"].observed_inbox
    assert observed, "observer with wake_policy=on_event must receive matching events"
    assert all(item["kind"] == "task_completed" for item in observed)
    payload = observed[0]["payload"]
    assert payload.get("status") == "SUCCEEDED"
    assert "PRIVATE DETAILS" not in str(payload), "status scope must not leak content"
    assert "private/out.txt" not in str(payload)

    # observation grants no sending right and no team modification
    send = h.rt.submit(TeamAction(action_id="w1", session_id="s1", actor_id="watch",
                                  kind="send_message",
                                  payload={"target": "b", "text": "hi"}))
    assert not send.ok and "not allowed to message" in send.error
    patch = h.rt.submit(TeamAction(action_id="w2", session_id="s1", actor_id="watch",
                                   kind="apply_topology_patch",
                                   payload={"operations": [{"op": "remove_agent",
                                                            "agent_id": "b"}],
                                            "base_revision": 1}))
    assert not patch.ok and "Leader" in patch.error
