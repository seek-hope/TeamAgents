"""Compact core probes (C): un-acked delivery -> repeated runs; restart re-injection."""

from __future__ import annotations

import asyncio
import sys
import tempfile
from collections import Counter
from pathlib import Path

REPO = Path("/home/rimuru/Projects/Code/for_fun/TeamAgents")
sys.path.insert(0, str(REPO / "src"))
sys.path.insert(0, str(REPO / "tests"))

from teamagents.control import Control  # noqa: E402
from teamagents.models import ActionKind, TeamAction, TurnStatus  # noqa: E402
from teamagents.storage import Store  # noqa: E402

LINES: list[str] = []


def say(*p) -> None:
    line = " ".join(str(x) for x in p)
    LINES.append(line)
    print(line, flush=True)


async def _close(rt) -> None:
    for name in ("aclose", "close", "shutdown"):
        fn = getattr(rt, name, None)
        if fn is not None:
            res = fn()
            if asyncio.iscoroutine(res):
                await res
            return


async def part_a() -> None:
    from conftest import Harness, leader, member, scripts, spec_of, task_channel
    from teamagents.runtime import fake_session

    say("== C-a: delivery that never gets acked -> repeated turns ==")
    tmp = Path(tempfile.mkdtemp(prefix="c2a-"))
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(leader=[("end",)], b=[("end",)])
    original = Store.ack_run_deliveries
    Store.ack_run_deliveries = lambda self, run: None          # crash before ack
    try:
        h = Harness(fake_session(tmp, spec, members, session_id="s1"), members)
        await h.start()
        h.rt.user_message("hello leader")
        settled = await h.rt.settle(20)
        runs = h.rt.store.runs_for_session("s1")
        say("C-a1 settle():", settled)
        say("C-a2 total runs created:", len(runs))
        say("C-a3 status histogram:", dict(Counter(str(r.status) for r in runs)))
        say("C-a4 runs whose input_delivery_ids == [1]:",
            sum(1 for r in runs if r.input_delivery_ids == [1]))
        say("C-a5 pending deliveries:",
            [(d["delivery_id"], d["batch_no"], d["status"])
             for d in h.rt.store.pending_deliveries("s1", "leader")])
        say("C-a6 event kinds:",
            dict(Counter(e["kind"] for e in h.rt.store.events("s1"))))
        say("C-a7 queued/running runs left:",
            [(r.run_id, str(r.status)) for r in h.rt.store.runs_for_session(
                "s1", [TurnStatus.QUEUED, TurnStatus.RUNNING])])
        await _close(h.rt)
    except Exception as exc:  # noqa: BLE001
        say("C-a CRASHED:", type(exc).__name__, exc)
    finally:
        Store.ack_run_deliveries = original


async def part_b() -> None:
    from conftest import leader, member, spec_of, task_channel
    from teamagents.runtime import fake_session
    from teamagents.views import build_agent_view

    say("\n== C-b: terminal run + un-acked delivery -> same input re-queued ==")
    tmp = Path(tempfile.mkdtemp(prefix="c2b-"))
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    rt = fake_session(tmp, spec, {}, session_id="s1")
    rt.control.submit(TeamAction(action_id="m1", session_id="s1", actor_id="user",
                                 kind=ActionKind.USER_MESSAGE, payload={"text": "hello"}))
    run = rt.store.runs_for_session("s1", [TurnStatus.QUEUED])[0]
    say("C-b1 first run:", run.run_id, str(run.status), run.input_delivery_ids)
    # simulate the crash window: the run status commits, the ack never happens
    rt.store.set_run_status(run.run_id, TurnStatus.COMPLETED)
    say("C-b2 after 'crash': run =", rt.store.get_run(run.run_id).status,
        "| pending deliveries =",
        [(d["delivery_id"], d["status"]) for d in rt.store.pending_deliveries("s1", "leader")])
    rt.control.schedule()
    new = [r for r in rt.store.runs_for_session("s1") if r.run_id != run.run_id]
    say("C-b3 runs created by the next scheduling pass:",
        [(r.run_id, str(r.status), r.input_delivery_ids) for r in new])
    for r in new:
        view = build_agent_view(rt.store, spec, "s1", r.agent_id, r)
        say("C-b4 what the re-created run would inject:",
            [(i["kind"], i["payload"].get("text")) for i in view.inbox_delta])
    rt.store.close()


async def main() -> None:
    await part_a()
    await part_b()
    Path(REPO / "home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/probe-core-C.txt") \
        .write_text("\n".join(LINES) + "\n", encoding="utf-8")


asyncio.run(main())
