"""F-C3/RT-05 regression: a delivery pushed into a RUNNING turn mid-flight is
injection too - it must be acknowledged when the run ends, exactly once.

With terminal-only acking of `view.delivery_ids`, a mid-turn push was never
acknowledged and the next scheduling pass created a second run that delivered
the same input again (duplicate processing).
"""

from __future__ import annotations

import asyncio

from conftest import leader, scripts, spec_of


async def test_mid_turn_push_is_acked_and_delivered_once(harness_factory):
    spec = spec_of(leader())
    members = scripts(leader=[("sleep", 0.6), ("inbox",), ("end",)])
    h = await harness_factory(spec, members)
    await h.user("go")

    deadline = asyncio.get_event_loop().time() + 5
    while not [r for r in h.rt.store.runs_for_session("s1", ["RUNNING"])
               if r.agent_id == "leader"]:
        assert asyncio.get_event_loop().time() < deadline
        await asyncio.sleep(0.01)
    await asyncio.sleep(0.1)
    receipt = h.rt.user_message("mid-turn supplement", supplement=True)
    assert receipt.ok
    assert await h.rt.settle(15)

    runs = h.rt.store.runs_for_session("s1")
    assert len(runs) == 1, f"duplicate run created for a mid-turn push: {len(runs)}"
    assert runs[0].status == "COMPLETED"

    deliveries = h.rt.store.conn.execute(
        "SELECT delivery_id, status FROM deliveries ORDER BY delivery_id").fetchall()
    assert [d["status"] for d in deliveries] == ["applied", "applied"]

    observed = [(i.get("payload") or {}).get("text")
                for i in members["leader"].observed_inbox]
    assert "mid-turn supplement" in observed, (
        "the pushed supplement must reach the member at its next model call")
