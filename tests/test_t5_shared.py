"""T5: shared space publish/discover with permissions; large outputs use refs."""

from __future__ import annotations

from conftest import leader, member, scripts, spec_of, task_channel
from teamagents.models import TeamAction


async def test_t5_shared_space_permissions_and_discovery(harness_factory):
    spec = spec_of(
        leader(), member("b"), member("c"), member("d"),
        channels=[task_channel("leader", ["b", "c"])],
        shared_spaces=[{"id": "main", "readers": ["leader", "b", "c"],
                        "writers": ["leader", "b"]}],
    )
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "publish findings"}),
                ("call", "assign_task", {"assignee": "c", "description": "use findings"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id",
                                                         "$r1.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("call", "publish_shared", {"space_id": "main", "kind": "finding",
                                       "content": "the cache is cold",
                                       "ref": "artifacts/trace-1.bin"}),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
        c=[("inbox",),
           ("call", "read_shared", {"space_id": "main"}),
           ("call", "list_shared", {}),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}),
           ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("share and reuse findings")
    await h.settle()

    # discoverable by c (read permission), with the ref for large output
    read = next(r for r in members["c"].results if r.kind == "read_shared")
    assert read.ok and read.result["entries"][0]["content"] == "the cache is cold"
    assert read.result["entries"][0]["ref"] == "artifacts/trace-1.bin"

    # d has neither read nor write access
    denied_write = h.rt.submit(TeamAction(action_id="d1", session_id="s1", actor_id="d",
                                          kind="publish_shared",
                                          payload={"space_id": "main", "content": "nope"}))
    assert not denied_write.ok and "write access" in denied_write.error
    denied_read = h.rt.submit(TeamAction(action_id="d2", session_id="s1", actor_id="d",
                                         kind="read_shared",
                                         payload={"space_id": "main"}))
    assert not denied_read.ok and "read access" in denied_read.error
    listed = h.rt.submit(TeamAction(action_id="d3", session_id="s1", actor_id="d",
                                    kind="list_shared", payload={}))
    assert listed.ok and listed.result["spaces"] == []


async def test_t5_pagination_and_supersede(harness_factory):
    spec = spec_of(
        leader(),
        channels=[],
        shared_spaces=[{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    )
    members = scripts(
        leader=[("call", "publish_shared", {"space_id": "main", "content": "v1",
                                            "kind": "decision"}),
                ("call", "publish_shared", {"space_id": "main", "content": "v2",
                                            "kind": "decision",
                                            "supersedes": "$r0.result.entry_id"}),
                ("call", "read_shared", {"space_id": "main", "after_sequence": "$r0.result.sequence",
                                         "limit": 10}),
                ("call", "signal_done", {}), ("end",)],
    )
    h = await harness_factory(spec, members)
    await h.user("record two decisions")
    await h.settle()
    read = next(r for r in members["leader"].results if r.kind == "read_shared")
    assert len(read.result["entries"]) == 1
    assert read.result["entries"][0]["content"] == "v2"
    assert read.result["entries"][0]["supersedes"], "supersede link must be preserved"
