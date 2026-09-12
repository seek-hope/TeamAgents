"""Adversarial probes for the verify-domain review (read-only).

Run from the repo root:  .venv/bin/python review/tmp/probe_verify.py
Writes nothing outside review/tmp. Imports the repo's own test harness.
"""
from __future__ import annotations

import asyncio
import json
import sys
import tempfile
from pathlib import Path

CWD = Path.cwd()
assert (CWD / "pyproject.toml").exists(), "run from the repository root"
sys.path.insert(0, str(CWD / "src"))
sys.path.insert(0, str(CWD / "tests"))

from conftest import Harness, leader, member, msg_channel, scripts, spec_of, task_channel  # noqa: E402
from teamagents.models import TeamAction  # noqa: E402
from teamagents.runtime import fake_session  # noqa: E402
from teamagents.models import ModelProfile, UserConfig  # noqa: E402

RESULTS: list[dict] = []


def report(pid: str, claim: str, verdict: str, detail: str) -> None:
    RESULTS.append({"probe": pid, "claim": claim, "verdict": verdict, "detail": detail})
    print(f"[{pid}] {verdict} :: {claim} :: {detail}", flush=True)


def tmpdir() -> Path:
    return Path(tempfile.mkdtemp(prefix="probe-verify-"))


async def pr1_duplicate_action_returns_same_receipt() -> None:
    """Duplicate TeamAction (same action_id) must return the original receipt."""
    tmp = tmpdir()
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    rt = fake_session(tmp, spec, scripts(leader=[("end",)]), session_id="s1")
    try:
        act = TeamAction(action_id="dup-1", session_id="s1", actor_id="leader",
                         kind="assign_task",
                         payload={"assignee": "b", "description": "job"})
        r1 = rt.submit(act)
        r2 = rt.submit(act)
        tasks = rt.store.tasks_for_session("s1")
        created = [e for e in rt.store.events("s1") if e["kind"] == "task_created"]
        ok = r1.ok and r1 == r2 and len(tasks) == 1 and len(created) == 1
        report("PR-1", "重复动作返回原回执且只生效一次",
               "证实" if ok else "证伪",
               f"r1==r2:{r1 == r2} tasks:{len(tasks)} task_created:{len(created)} "
               f"r1.ok:{r1.ok} kind:{r1.kind}")
    finally:
        rt.store.close()


async def pr2_crash_replay_dedups() -> None:
    """Crash after the action committed; the replayed turn must not double-apply."""
    tmp = tmpdir()
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "one task"}),
                ("barrier", "hold"), ("call", "signal_done", {}), ("end",)],
    )
    h = await Harness(fake_session(tmp, spec, members, session_id="s1"), members).start()
    await h.user("go")
    deadline = asyncio.get_event_loop().time() + 5
    while not h.rt.store.tasks_for_session("s1"):
        assert asyncio.get_event_loop().time() < deadline
        await asyncio.sleep(0.01)
    original = members["leader"].results[0]
    await h.rt.close()  # hard process loss mid-turn

    fresh = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "one task"}),
                ("call", "signal_done", {}), ("end",)],
        b=[("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)],
    )
    h2 = Harness(fake_session(tmp, spec, fresh, session_id="s1"), fresh)
    await h2.start()
    try:
        ok = await h2.rt.settle(8)
        tasks = h2.rt.store.tasks_for_session("s1")
        created = [e for e in h2.rt.store.events("s1") if e["kind"] == "task_created"]
        replayed = fresh["leader"].results[0]
        dedup = len(tasks) == 1 and len(created) == 1
        report("PR-2", "崩溃后同 run 重放同一动作不重复建任务、返回原回执",
               "证实" if (ok and dedup) else "证伪",
               f"settle:{ok} tasks:{len(tasks)} task_created:{len(created)} "
               f"replay_receipt_ok:{replayed.ok} same_ok_kind:{replayed.kind == original.kind} "
               f"replay_task_id:{replayed.result.get('task_id')}")
    finally:
        await h2.aclose()


async def pr3_deliveries_not_reinjected() -> None:
    """Acked deliveries must not be re-injected by re-scheduling or a restart."""
    tmp = tmpdir()
    spec = spec_of(leader(), member("b"),
                   channels=[task_channel("leader", ["b"]), msg_channel("b", ["leader"])])
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "job"}),
                ("call", "wait_for_tasks", {"task_ids": ["$r0.result.task_id"]}),
                ("wait",), ("call", "signal_done", {}), ("end",)],
        b=[("inbox",),
           ("call", "complete_task", {"task_id": "$inbox0.payload.task_id"}), ("end",)],
    )
    h = await Harness(fake_session(tmp, spec, members, session_id="s1"), members).start()
    await h.user("run the job")
    await h.settle()
    b_before = len(members["b"].observed_inbox)
    for _ in range(3):
        h.rt.control.schedule()
    pending = h.rt.store.pending_deliveries("s1", "b")
    b_after = len(members["b"].observed_inbox)

    fresh = scripts(leader=[], b=[])
    h2 = Harness(fake_session(tmp, spec, fresh, session_id="s1"), fresh)
    await h2.start()
    try:
        await asyncio.sleep(0.15)
        reinjected = fresh["b"].observed_inbox
        ok = pending == [] and b_before == b_after and reinjected == []
        report("PR-3", "已确认投递在重新调度/重启后不重复注入",
               "证实" if ok else "证伪",
               f"pending_after_reschedule:{pending} inbox_before:{b_before} "
               f"inbox_after_reschedule:{b_after} restart_reinjected:{len(reinjected)}")
    finally:
        await h2.aclose()
        await h.aclose()


async def pr4_leader_only_actions() -> None:
    """Non-leader members must not signal_done or apply_topology_patch."""
    tmp = tmpdir()
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    rt = fake_session(tmp, spec, scripts(leader=[("end",)], b=[("end",)]), session_id="s1")
    try:
        done = rt.submit(TeamAction(action_id="sd-b", session_id="s1", actor_id="b",
                                    kind="signal_done", payload={}))
        patch = rt.submit(TeamAction(
            action_id="tp-b", session_id="s1", actor_id="b", kind="apply_topology_patch",
            payload={"operations": [{"op": "remove_agent", "agent_id": "leader"}],
                     "base_revision": 1}))
        kinds = [e["kind"] for e in rt.store.events("s1")]
        ok = (not done.ok and "only the Leader" in (done.error or "")
              and not patch.ok and "only the Leader" in (patch.error or "")
              and "goal_done" not in kinds)
        report("PR-4", "非 Leader 不能 signal_done / apply_topology_patch",
               "证实" if ok else "证伪",
               f"signal_done.ok:{done.ok} err:{done.error!r}; "
               f"patch.ok:{patch.ok} err:{patch.error!r}; goal_done:{'goal_done' in kinds}")
    finally:
        rt.store.close()


async def pr5_full_auto_user_only() -> None:
    """Only the local user may switch the session into full_auto."""
    tmp = tmpdir()
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    rt = fake_session(tmp, spec, scripts(leader=[("end",)], b=[("end",)]), session_id="s1")
    try:
        member_try = rt.submit(TeamAction(action_id="pm-b", session_id="s1", actor_id="b",
                                          kind="set_permission_mode",
                                          payload={"mode": "full_auto"}))
        leader_try = rt.submit(TeamAction(action_id="pm-leader", session_id="s1",
                                          actor_id="leader", kind="set_permission_mode",
                                          payload={"mode": "full_auto"}))
        user_ok = rt.submit(TeamAction(action_id="pm-user", session_id="s1", actor_id="user",
                                       kind="set_permission_mode",
                                       payload={"mode": "full_auto"}))
        session = rt.store.get_session("s1")
        mode = session["permissions_mode"]
        ok = (not member_try.ok and not leader_try.ok and user_ok.ok and mode == "full_auto")
        report("PR-5", "全自动模式只能由 user 开启",
               "证实" if ok else "证伪",
               f"member:{member_try.ok}/{member_try.error!r} "
               f"leader:{leader_try.ok}/{leader_try.error!r} user:{user_ok.ok} mode:{mode}")
    finally:
        rt.store.close()


async def pr6_limit_reached_does_not_fake_done() -> None:
    """Hitting LIMIT_REACHED must not mark the goal done or emit goal_done."""
    tmp = tmpdir()
    spec = spec_of(
        leader(), member("b"), member("c"),
        channels=[task_channel("leader", ["b"]), msg_channel("b", ["c"]),
                  msg_channel("c", ["b"])],
        limits={"max_parallel_workers": 2, "max_members": 8, "max_turns_per_goal": 3,
                "max_model_steps_per_turn": 10, "turn_active_timeout_s": 30},
    )
    bounce = [("call", "send_message", {"target": "c", "text": "tick"}), ("end",)] * 5
    reply = [("call", "send_message", {"target": "b", "text": "tick"}), ("end",)] * 5
    members = scripts(
        leader=[("call", "assign_task", {"assignee": "b", "description": "spam"}),
                ("wait",), ("end",)],
        b=bounce, c=reply,
    )
    h = await Harness(fake_session(tmp, spec, members, session_id="s1"), members).start()
    try:
        await h.user("go")
        await h.settle(15)
        kinds = [e["kind"] for e in h.rt.store.events("s1")]
        session = h.rt.store.get_session("s1")
        ok = ("limit_reached" in kinds and "goal_done" not in kinds
              and session["goal_state"] != "done")
        report("PR-6", "LIMIT_REACHED 不冒充完成（不改 goal_state、不发 goal_done）",
               "证实" if ok else "证伪",
               f"limit_reached:{'limit_reached' in kinds} "
               f"goal_done:{'goal_done' in kinds} goal_state:{session['goal_state']}")
    finally:
        await h.aclose()


async def pr7_same_name_identity_isolation() -> None:
    """T24 claim: a re-added member with the same id must not inherit the old
    identity/context. Adversarial check: is context_epoch ever bumped?"""
    tmp = tmpdir()
    spec = spec_of(leader(), member("b"), channels=[task_channel("leader", ["b"])])
    catalog = UserConfig(models={"test": ModelProfile(provider="openai", model="test")})
    rt = fake_session(tmp, spec, scripts(leader=[("end",)], b=[("end",)]),
                      catalog=catalog, session_id="s1")
    try:
        before = rt.store.agent_context_epoch("s1", "b")
        remove = rt.submit(TeamAction(
            action_id="patch-1", session_id="s1", actor_id="leader",
            kind="apply_topology_patch",
            payload={"operations": [{"op": "remove_agent", "agent_id": "b"}],
                     "base_revision": 1}))
        after_removal = rt.store.agent_context_epoch("s1", "b")
        revision = rt.store.current_revision("s1")
        readd = rt.submit(TeamAction(
            action_id="patch-2", session_id="s1", actor_id="leader",
            kind="apply_topology_patch",
            payload={"operations": [{"op": "add_agent", "agent": {"id": "b", "name": "B-new",
             "role": "worker", "runtime_kind": "deepagents", "instructions": "act as a new b",
             "model_profile": "test", "tool_bindings": ["files"], "skills": [],
             "workspace_policy": "shared"},
             "channels": [{"source": "leader", "targets": ["b"], "mode": "task"}]}],
             "base_revision": revision}))
        after_readd = rt.store.agent_context_epoch("s1", "b")
        run_context_refs = [r.context_ref for r in rt.store.runs_for_session("s1")]
        ok = after_readd > before and remove.ok and readd.ok
        report("PR-7", "同名成员移除→重建后 context_epoch 递增（不继承旧身份）",
               "证实" if ok else "证伪",
               f"epoch before:{before} after_removal:{after_removal} after_readd:{after_readd} "
               f"remove.ok:{remove.ok}/{remove.error!r} readd.ok:{readd.ok}/{readd.error!r} "
               f"ctx_refs:{run_context_refs}")
    finally:
        rt.store.close()


async def main() -> int:
    probes = [("PR-1", pr1_duplicate_action_returns_same_receipt),
              ("PR-2", pr2_crash_replay_dedups),
              ("PR-3", pr3_deliveries_not_reinjected),
              ("PR-4", pr4_leader_only_actions),
              ("PR-5", pr5_full_auto_user_only),
              ("PR-6", pr6_limit_reached_does_not_fake_done),
              ("PR-7", pr7_same_name_identity_isolation)]
    wanted = [a.upper() for a in sys.argv[1:] if a.upper().startswith("PR")]
    for pid, fn in probes:
        if wanted and pid not in wanted:
            continue
        try:
            await asyncio.wait_for(fn(), timeout=90)
        except Exception as exc:  # a probe never takes the whole run down
            report(pid, "probe execution", "无法验证", f"{type(exc).__name__}: {exc}")
    print("\nJSON:", json.dumps(RESULTS, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
