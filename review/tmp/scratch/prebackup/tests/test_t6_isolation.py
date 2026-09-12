"""T6: directed messages stay out of third-party views; no private context
reaches the Leader automatically; new sessions do not inherit anything."""

from __future__ import annotations

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel
from teamagents.runtime import fake_session


async def test_t6_private_message_not_in_third_party_view(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("c"),
        channels=[task_channel("leader", ["b", "c"]),
                  msg_channel("b", ["c"]), msg_channel("c", ["b"])],
    )
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "work"}),
                ("inbox",),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "send_message", {"target": "c", "text": "for your eyes only"}),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
        c=[("inbox",), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("b and c coordinate")
    await h.settle()

    c_texts = [i["payload"]["text"] for i in members["c"].observed_inbox
               if i.get("kind") == "message"]
    assert c_texts == ["for your eyes only"]
    # the Leader was never delivered the private exchange
    leader_texts = [i["payload"].get("text") for i in members["leader"].observed_inbox
                    if i.get("kind") == "message"]
    assert "for your eyes only" not in leader_texts


async def test_t6_new_session_does_not_inherit(harness_factory, tmp_path):
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "one-off"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("session one work")
    await h.settle()
    assert h.rt.store.tasks_for_session("s1")

    rt2 = fake_session(tmp_path, spec, {"leader": members["leader"], "b": members["b"]},
                       session_id="s2")
    h2 = Harness(rt2, members)
    await h2.start()
    try:
        assert rt2.store.tasks_for_session("s2") == []
        assert rt2.store.events("s2") == []
        assert rt2.store.pending_deliveries("s2", "leader") == []
    finally:
        await h2.aclose()
