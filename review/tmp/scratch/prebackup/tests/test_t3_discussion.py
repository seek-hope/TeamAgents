"""T3: discussion rides allowed channels; deliveries are exactly-once injected
even if actions are retried; blocks targets outside the channel."""

from __future__ import annotations

from conftest import leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.models import TeamAction


async def test_t3_discussion_once_and_channel_enforcement(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("c"), member("d"),
        channels=[task_channel("leader", ["b", "c"]),
                  msg_channel("b", ["c"]), msg_channel("c", ["b"]),
                  msg_channel("b", ["leader"]), msg_channel("c", ["leader"])],
    )
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "discuss"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "send_message", {"target": "c", "text": "ping"}),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",),
           ("inbox",), ("end",)],
        c=[("inbox",),
           ("call", "send_message", {"target": "b", "text": "pong"}),
           ("call", "send_message", {"target": "d", "text": "leak"}),
           ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("let b and c discuss")
    await h.settle()

    c_messages = [x for x in members["c"].observed_inbox if x.get("kind") == "message"]
    assert [m["payload"]["text"] for m in c_messages] == ["ping"], "delivered once"
    b_messages = [x for x in members["b"].observed_inbox if x.get("kind") == "message"]
    assert [m["payload"]["text"] for m in b_messages] == ["pong"]

    # c cannot message d: rejected, and rejected calls never reach d
    c_receipts = [r for r in members["c"].results]
    leak = [r for r in c_receipts if not r.ok and "not allowed to message" in (r.error or "")]
    assert leak, "message outside the channel must be rejected"
    d_deliveries = h.rt.store.pending_deliveries("s1", "d")
    assert d_deliveries == []

    # retrying the same action id must not deliver a second time
    b_send = members["b"].results[0]
    again = h.rt.submit(TeamAction(
        action_id=b_send.action_id.replace(":step0", ":step0"), session_id="s1",
        actor_id="b", run_id=None, kind="send_message",
        payload={"target": "c", "text": "ping"}))
    assert again == h.rt.control.store.get_action_receipt(b_send.action_id)
    assert len([e for e in h.rt.store.events("s1") if e["kind"] == "message"]) == 2
