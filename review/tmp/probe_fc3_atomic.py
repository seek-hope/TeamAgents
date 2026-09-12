"""F-C3/RT-05 after-fix probe: the crash window cannot produce "terminal + un-acked",
and the retry acknowledges exactly the injected input once.

Run:  .venv/bin/python review/tmp/probe_fc3_atomic.py
"""
from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

REPO = Path("/home/rimuru/Projects/Code/for_fun/TeamAgents")
sys.path.insert(0, str(REPO / "src"))
sys.path.insert(0, str(REPO / "tests"))

from conftest import leader, scripts, spec_of  # noqa: E402
from teamagents.models import ActionKind, TeamAction, TurnStatus  # noqa: E402
from teamagents.runtime import fake_session  # noqa: E402
from teamagents.storage import Store  # noqa: E402

LINES: list[str] = []


def say(*parts) -> None:
    line = " ".join(str(p) for p in parts)
    LINES.append(line)
    print(line, flush=True)


def _deliveries(store):
    return [(r["delivery_id"], r["status"]) for r in store.conn.execute(
        "SELECT delivery_id, status FROM deliveries ORDER BY delivery_id")]


async def main() -> None:
    say("== F-C3 after: ack failure rolls the terminal transaction back ==")
    tmp = Path(tempfile.mkdtemp(prefix="fc3-"))
    rt = fake_session(tmp, spec_of(leader()), scripts(leader=[("end",)]), session_id="s1")
    original = Store.ack_deliveries_exact

    def raise_ack(self, *args, **kwargs):
        raise RuntimeError("ack write failed")

    rt.control.submit(TeamAction(action_id="m1", session_id="s1", actor_id="user",
                                 kind=ActionKind.USER_MESSAGE, payload={"text": "hello"}))
    run = rt.store.runs_for_session("s1", [TurnStatus.QUEUED])[0]
    Store.ack_deliveries_exact = raise_ack
    try:
        await rt._execute(run)
        say("UNEXPECTED: finalize did not fail")
    except RuntimeError as exc:
        say("finalize raised:", exc)
    finally:
        Store.ack_deliveries_exact = original
    say("run status after failure:", rt.store.get_run(run.run_id).status,
        "| agent:", rt.store.agent_status("s1", "leader"))
    say("deliveries after failure:", _deliveries(rt.store))
    rt.control.schedule()
    say("runs after the next scheduling pass:",
        len(rt.store.runs_for_session("s1")))

    say("\n-- the executor retries the same run once the ack works --")
    await rt._execute(rt.store.get_run(run.run_id))
    say("run status after retry:", rt.store.get_run(run.run_id).status,
        "| deliveries:", _deliveries(rt.store))
    rt.control.schedule()
    say("total runs:", len(rt.store.runs_for_session("s1")),
        "| pending deliveries:", rt.store.pending_deliveries("s1", "leader"))
    rt.store.close()

    out = REPO / "review" / "tmp" / "probe-fc3-after.txt"
    out.write_text("\n".join(LINES) + "\n", encoding="utf-8")


asyncio.run(main())
