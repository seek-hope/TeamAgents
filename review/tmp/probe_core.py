"""Core-domain review probes (read-only wrt the repo; writes only temp DBs).

A) DRAINING leak: a patch that changes only *permissions* marks an idle-at-boundary
   member DRAINING and never clears it -> the member is skipped forever.
B) Exception inside Control.submit's tx: a failure receipt is committed while a
   write already performed inside _reduce stays committed ("nothing half-applies").
D) Runtime.reconcile applies a terminal run status without _finalize: task stays
   RUNNING, agent stays BUSY, no events, completion request never applied.
C) TurnRun status is committed before deliveries are acked: a crash in between
   leaves a terminal run with pending deliveries -> next scheduling pass creates a
   new run that re-injects the same input into the member's context.
"""

from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

REPO = Path("/home/rimuru/Projects/Code/for_fun/TeamAgents")
sys.path.insert(0, str(REPO / "src"))
sys.path.insert(0, str(REPO / "tests"))

from teamagents.control import Control  # noqa: E402
from teamagents.models import (  # noqa: E402
    ActionKind, AgentSpec, AgentStatus, ChannelMode, ChannelSpec, RuntimeKind,
    Task, TaskStatus, TeamAction, TeamSpec, TurnRun, TurnStatus, UserConfig,
)
from teamagents.storage import Store  # noqa: E402

OUT: list[str] = []


def say(*parts) -> None:
    line = " ".join(str(p) for p in parts)
    OUT.append(line)
    print(line, flush=True)


def agent(aid: str, role: str = "worker") -> AgentSpec:
    return AgentSpec(id=aid, name=aid.title(), role=role, runtime_kind=RuntimeKind.DEEPAGENTS,
                     instructions="x", model_profile="test", tool_bindings=["files"])


def build_spec() -> TeamSpec:
    return TeamSpec(
        leader_id="leader",
        agents=[agent("leader", "leader"), agent("b"), agent("c")],
        channels=[
            ChannelSpec(source="leader", targets=["b", "c"], mode=ChannelMode.TASK),
            ChannelSpec(source="b", targets=["c"], mode=ChannelMode.MESSAGE),
            ChannelSpec(source="c", targets=["b"], mode=ChannelMode.MESSAGE),
        ],
        shared_spaces=[{"id": "main", "readers": ["leader", "b"], "writers": ["leader", "b"]}],
    )


def fresh_store(tag: str) -> Store:
    tmp = Path(tempfile.mkdtemp(prefix=f"probe-{tag}-"))
    store = Store(tmp / "team.db")
    store.create_session("s1", str(tmp), "approved_scope")
    spec = build_spec()
    store.save_team_spec("s1", spec)
    for a in spec.agents:
        store.ensure_agent("s1", a.id)
    return store


def act(kind, actor, payload, action_id):
    return TeamAction(action_id=action_id, session_id="s1", actor_id=actor, kind=kind,
                      payload=payload)


# --------------------------------------------------------------------------- A
def probe_a() -> None:
    say("\n== A) DRAINING leak (permission-only affected member) ==")
    store = fresh_store("a")
    control = Control(store, "s1")
    run = TurnRun(run_id="run_c", session_id="s1", agent_id="c", config_revision=1,
                  topology_revision=1, status=TurnStatus.RUNNING)
    store.insert_run(run)
    store.set_agent_status("s1", "c", AgentStatus.BUSY)
    receipt = control.submit(act(ActionKind.APPLY_TOPOLOGY_PATCH, "leader",
                                 {"base_revision": 1,
                                  "operations": [{"op": "add_channel",
                                                  "channel": {"source": "c",
                                                              "targets": ["b", "leader"],
                                                              "mode": "message"}}]},
                                 "A-patch"))
    say("A1 receipt:", receipt.ok, receipt.result)
    say("A2 affected_agents:", receipt.result.get("affected_agents"),
        "| c status:", store.agent_status("s1", "c"))
    store.update_run_status_where("run_c", TurnStatus.RUNNING, TurnStatus.COMPLETED)
    control.schedule()
    say("A3 after boundary: revision =", store.current_revision("s1"),
        "| c status =", store.agent_status("s1", "c"),
        "| c config_revision =", store.agent_config_revision("s1", "c"))
    # b sends c a message: c has a pending delivery but is skipped by _schedule
    msg = control.submit(act(ActionKind.SEND_MESSAGE, "b", {"target": "c", "text": "hi"}, "A-msg"))
    control.schedule()
    runs_c = [r for r in store.runs_for_session("s1", ["QUEUED", "RUNNING"])
              if r.agent_id == "c"]
    say("A4 message delivered_to:", msg.result, "| pending deliveries for c:",
        len(store.pending_deliveries("s1", "c")), "| new runs for c:", len(runs_c),
        "| c status:", store.agent_status("s1", "c"))
    say("A5 VERDICT: DRAINING leak" if store.agent_status("s1", "c") is AgentStatus.DRAINING
        and not runs_c and store.pending_deliveries("s1", "c") else "A5 VERDICT: no leak")
    store.close()


# --------------------------------------------------------------------------- B
def probe_b() -> None:
    say("\n== B) _reduce exception commits an already-performed write ==")
    store = fresh_store("b")
    control = Control(store, "s1")
    original = Store.add_shared_entry

    def boom(self, entry, session_id):
        original(self, entry, session_id)      # the write happens...
        raise RuntimeError("simulated failure after write")   # ...then the step fails

    Store.add_shared_entry = boom
    try:
        receipt = control.submit(act(ActionKind.PUBLISH_SHARED, "b",
                                    {"space_id": "main", "content": "partial-write"},
                                    "B-publish"))
    finally:
        Store.add_shared_entry = original
    rows = store.shared_entries("s1", ["main"])
    say("B1 receipt:", receipt.ok, repr(receipt.error))
    say("B2 committed entries in space 'main':", [e.content for e in rows])
    say("B3 failure receipt persisted (action cannot be retried):",
        store.get_action_receipt("B-publish") is not None)
    say("B4 VERDICT: partial write committed with failure receipt"
        if (not receipt.ok and rows) else "B4 VERDICT: clean rollback")
    store.close()


# --------------------------------------------------------------------------- D
async def probe_d() -> None:
    say("\n== D) runtime.reconcile sets a terminal run status without _finalize ==")
    store = fresh_store("d")
    spec = build_spec()
    store.insert_task("s1", Task(task_id="t1", requester="leader", assignee="b",
                                 description="job", status=TaskStatus.RUNNING))
    store.insert_run(TurnRun(run_id="run_b", session_id="s1", agent_id="b", task_id="t1",
                             config_revision=1, topology_revision=1, status=TurnStatus.RUNNING))
    store.set_agent_status("s1", "b", AgentStatus.BUSY)
    store.record_completion_request("run_b", "t1", ["ref:one"], "finished before crash")

    class StubRunner:  # the AgentRunner surface reconcile() uses
        def query_state(self, run_id):
            return None

        async def reconcile(self, run):        # a checkpoint that looks finished
            return TurnStatus.COMPLETED

        async def request_interrupt(self, run_id):
            return TurnStatus.CANCELLED

    from teamagents.runtime import SessionRuntime
    runtime = SessionRuntime(store, "s1", UserConfig(), runners={"b": StubRunner()})
    await runtime.reconcile()
    control = Control(store, "s1")
    say("D1 run status:", store.get_run("run_b").status,
        "| task status:", store.get_task("t1").status,
        "| agent status:", store.agent_status("s1", "b"))
    say("D2 events:", [e["kind"] for e in store.events("s1")])
    say("D3 completion request still unapplied:",
        store.completion_request("run_b") is not None)
    say("D4 signal_done blockers:", control._completion_blockers(spec, current_run=None))
    leak = (store.get_run("run_b").status is TurnStatus.COMPLETED
            and store.get_task("t1").status is TaskStatus.RUNNING
            and store.completion_request("run_b") is not None)
    say("D5 VERDICT: recovered result silently dropped" if leak
        else "D5 VERDICT: clean recovery")
    store.close()


# --------------------------------------------------------------------------- C
async def probe_c() -> None:
    say("\n== C) run status committed before delivery ack -> duplicate injection ==")
    from conftest import Harness, leader, member, scripts, spec_of, task_channel
    from teamagents.runtime import fake_session

    tmp = Path(tempfile.mkdtemp(prefix="probe-c-"))
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])

    original_ack = Store.ack_run_deliveries
    Store.ack_run_deliveries = lambda self, run: None     # simulate "crash before ack"
    try:
        h = Harness(fake_session(tmp, spec, scripts(leader=[("end",)], b=[("end",)]),
                                 session_id="s1"),
                    scripts(leader=[("end",)], b=[("end",)]))
        await h.start()
        h.rt.user_message("hello leader")
        await h.settle()
        runs = h.rt.store.runs_for_session("s1")
        pending = h.rt.store.pending_deliveries("s1", "leader")
        say("C1 first pass runs:", [(r.run_id, r.status, r.input_delivery_ids) for r in runs])
        say("C2 pending deliveries for leader after a terminal run:",
            [(d["delivery_id"], d["batch_no"], d["status"]) for d in pending])
        await h.rt.close()
        h.rt.store.close()
    finally:
        Store.ack_run_deliveries = original_ack

    # restart over the same database: reconcile ignores the terminal run, the next
    # scheduling pass re-queues the very same deliveries into a new run
    members2 = scripts(leader=[("end",)], b=[("end",)])
    h2 = Harness(fake_session(tmp, spec, members2, session_id="s1"), members2)
    await h2.start()
    try:
        h2.rt.control.schedule()
        runs2 = h2.rt.store.runs_for_session("s1")
        new_runs = [r for r in runs2 if r.status in (TurnStatus.QUEUED, TurnStatus.RUNNING)]
        say("C3 runs after restart+schedule:",
            [(r.run_id, r.status, r.input_delivery_ids) for r in runs2])
        from teamagents.views import build_agent_view
        dup = []
        for r in new_runs:
            view = build_agent_view(h2.rt.store, spec, "s1", r.agent_id, r)
            dup.append((r.run_id, [i["kind"] for i in view.inbox_delta]))
        say("C4 new run would inject:", dup)
        say("C5 VERDICT: same input re-injected after restart"
            if new_runs and any(v for _, v in dup) else "C5 VERDICT: no duplicate")
    finally:
        await h2.rt.close()
        h2.rt.store.close()


async def main() -> None:
    probe_a()
    probe_b()
    await probe_d()
    await probe_c()
    Path(REPO / "home/rimuru/Projects/Code/for_fun/TeamAgents/review/tmp/probe-core-output.txt") \
        .write_text("\n".join(OUT) + "\n", encoding="utf-8")


asyncio.run(main())
