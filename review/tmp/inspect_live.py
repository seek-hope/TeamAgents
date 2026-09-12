#!/usr/bin/env python3
"""Host-side read-only inspector: why are tasks stuck while members look IDLE?

Prints the authoritative state for the session matching --goal (or --session-dir):
tasks, the latest turn runs per agent, member statuses, pending deliveries,
pending approvals, and per-task meta flags. Ends with a short diagnosis that
names the known stall patterns (dispatch liveness, zombie runs, blocked tasks).

Usage:
  .venv/bin/python review/tmp/inspect_live.py                     # auto-locate by goal
  .venv/bin/python review/tmp/inspect_live.py --session-dir DIR
Read-only: opens team.db with mode=ro; never writes.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))

DEFAULT_GOAL = "goal_b3de92c12a7d45a3"


def candidate_dirs(explicit: Path | None):
    if explicit is not None:
        yield explicit
        return
    bases: list[Path] = []
    try:
        from teamagents.config import sessions_dir

        bases.append(sessions_dir())
    except Exception:
        pass
    bases.append(Path.home() / ".local" / "state" / "teamagents" / "sessions")
    bases.append(REPO / ".local" / "state" / "teamagents" / "sessions")
    seen: set[str] = set()
    for base in bases:
        if not base.is_dir():
            continue
        for d in sorted(base.iterdir()):
            db = d / "team.db"
            if db.is_file() and str(db) not in seen:
                seen.add(str(db))
                yield d


def open_ro(db: Path) -> sqlite3.Connection | None:
    try:
        conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5)
        conn.row_factory = sqlite3.Row
        return conn
    except sqlite3.Error:
        return None


def locate(args) -> tuple[Path, str] | None:
    if args.session_dir is not None:
        conn = open_ro(args.session_dir / "team.db")
        if conn is None:
            print(f"无法打开 {args.session_dir/'team.db'}")
            return None
        row = conn.execute("SELECT session_id, goal_id FROM sessions").fetchone()
        conn.close()
        return (args.session_dir, row["session_id"]) if row else None
    for d in candidate_dirs(None):
        conn = open_ro(d / "team.db")
        if conn is None:
            continue
        try:
            rows = conn.execute("SELECT session_id, goal_id FROM sessions").fetchall()
        except sqlite3.Error:
            rows = []
        finally:
            conn.close()
        for r in rows:
            if r["goal_id"] == args.goal:
                return (d, r["session_id"])
    return None


def fmt_age(ts: float | None) -> str:
    if not ts:
        return "-"
    return f"{max(0.0, time.time() - ts):.0f}s前"


def main() -> int:
    ap = argparse.ArgumentParser(description="只读检查：卡住的任务/回合/投递")
    ap.add_argument("--session-dir", type=Path, default=None)
    ap.add_argument("--goal", default=DEFAULT_GOAL)
    ap.add_argument("--all", action="store_true", help="打印全部非终态任务与全部回合")
    args = ap.parse_args()

    hit = locate(args)
    if hit is None:
        print(f"未找到 goal={args.goal} 的会话；可用 --session-dir 指定。")
        return 1
    session_dir, session_id = hit
    conn = open_ro(session_dir / "team.db")
    if conn is None:
        print("无法打开数据库")
        return 1
    try:
        sess = conn.execute("SELECT * FROM sessions WHERE session_id=?",
                            (session_id,)).fetchone()
        print(f"会话 {session_id}  目录 {session_dir}")
        print(f"状态 {sess['status']}  目标 {sess['goal_id']} ({sess['goal_state']})  "
              f"权限模式 {sess['permissions_mode']}")

        print("\n== 成员（agent_runtime） ==")
        rus = conn.execute(
            "SELECT * FROM agent_runtime WHERE session_id=? ORDER BY agent_id",
            (session_id,)).fetchall()
        for r in rus:
            print(f"  {r['agent_id']:<18} {r['status']:<8} rev={r['config_revision']} "
                  f"updated={fmt_age(r['updated_at'])}")

        print("\n== 任务 ==")
        trows = conn.execute(
            "SELECT * FROM tasks WHERE session_id=? ORDER BY created_at",
            (session_id,)).fetchall()
        nonfinal = {"PENDING", "RUNNING", "BLOCKED"}
        shown = trows if args.all else [t for t in trows if t["status"] in nonfinal]
        if not shown:
            print("  （无未完成任务）")
        by_id = {t["task_id"]: t for t in trows}
        ready_ids: set[str] = set()
        for t in trows:
            if t["status"] != "PENDING":
                continue
            deps = json.loads(t["dependencies"] or "[]")
            dep_rows = [by_id.get(d) for d in deps]
            if all(d is not None and d["status"] == "SUCCEEDED" for d in dep_rows):
                ready_ids.add(t["task_id"])
        for t in shown:
            deps = json.loads(t["dependencies"] or "[]")
            mark = ""
            if t["status"] == "PENDING":
                mark = " [ready]" if t["task_id"] in ready_ids else " [dep-blocked]"
            print(f"  {t['task_id']} {t['status']:<9} assignee={t['assignee']:<18} "
                  f"age={fmt_age(t['created_at'])} deps={deps}{mark}")
            print(f"      {t['description'][:90]!r}")

        print("\n== 回合（最近 14 条） ==")
        rrows = conn.execute(
            "SELECT * FROM turn_runs WHERE session_id=? ORDER BY created_at DESC LIMIT 14",
            (session_id,)).fetchall()
        for r in rrows:
            print(f"  {r['run_id']} {r['agent_id']:<18} {r['status']:<16} "
                  f"task={r['task_id'] or '-'} waiting={r['waiting_on']} "
                  f"cancel={r['cancel_requested']} updated={fmt_age(r['updated_at'])}")

        print("\n== 未确认投递（pending deliveries） ==")
        drows = conn.execute(
            "SELECT agent_id, COUNT(*) AS n FROM deliveries WHERE session_id=? AND status='pending'"
            " GROUP BY agent_id", (session_id,)).fetchall()
        pending_del = {d["agent_id"]: d["n"] for d in drows}
        for d in drows:
            print(f"  {d['agent_id']}: {d['n']}")
        if not drows:
            print("  （无）")

        print("\n== 待批准 ==")
        arows = conn.execute(
            "SELECT approval_id, agent_id, status FROM approvals"
            " WHERE session_id=? AND status IN ('PENDING','APPROVED_ONCE')",
            (session_id,)).fetchall()
        for a in arows:
            print(f"  {a['approval_id']} {a['agent_id']} {a['status']}")
        if not arows:
            print("  （无）")

        print("\n== 诊断 ==")
        idle = {r["agent_id"] for r in rus if r["status"] == "IDLE"}
        active_agents = {r["agent_id"] for r in rrows
                         if r["status"] in ("QUEUED", "RUNNING", "WAITING_TASK",
                                            "WAITING_APPROVAL")}
        pending_by_agent: dict[str, list[str]] = {}
        dep_blocked_by_agent: dict[str, list[str]] = {}
        for t in trows:
            if t["status"] != "PENDING":
                continue
            bucket = (pending_by_agent if t["task_id"] in ready_ids
                      else dep_blocked_by_agent)
            bucket.setdefault(t["assignee"], []).append(t["task_id"])
        for agent, tids in dep_blocked_by_agent.items():
            print(f"  - {agent} 的 PENDING 任务 {tids} 依赖未满足：正常等待（依赖完成后自动派发）")
        problems = 0
        for agent, tids in pending_by_agent.items():
            if agent not in idle:
                continue  # busy elsewhere: the task waits for the member's turn
            if active_agents and agent in active_agents:
                print(f"  - {agent} 有未终态回合但成员显示 IDLE：restart（reconcile）会收敛")
                problems += 1
                continue
            if pending_del.get(agent, 0) > 0:
                print(f"  - {agent}: PENDING 任务 {tids}，仍有未确认投递 → "
                      f"下一次调度应派发（若有异常再查）")
                continue
            print(f"  - {agent} IDLE 且 PENDING 任务 {tids} 无未确认投递 → 唤醒通知可能已被"
                  f"忙回合吞掉（派发活性缺口）。已修复；restart 会话后自动派发，"
                  f"或临时用 re-ping 方案")
            problems += 1
        stale = [r for r in rrows
                 if r["status"] in ("RUNNING",) and (time.time() - (r["updated_at"] or 0)) > 180]
        for r in stale:
            print(f"  - 回合 {r['run_id']}（{r['agent_id']}）RUNNING 但 {fmt_age(r['updated_at'])} 未更新"
                  f" → 可能是僵尸回合；restart 时 reconcile 会收敛")
            problems += 1
        running_tasks = [t for t in trows if t["status"] == "RUNNING"]
        for t in running_tasks:
            if t["assignee"] not in active_agents:
                print(f"  - 任务 {t['task_id']} RUNNING（{t['assignee']}）但没有未终态回合"
                      f" → 陈旧状态（回合已终态、自身任务未收敛；已修复）。"
                      f"restart 后由 Leader/用户 c 取消再重派即可")
                problems += 1
        blocked = [t for t in trows if t["status"] == "BLOCKED"]
        for t in blocked:
            print(f"  - 任务 {t['task_id']} BLOCKED（{t['assignee']}）：等待人工/Leader 干预"
                  f"（TUI 任务页 c 取消，或重派）")
        if problems == 0:
            print("  （未发现明显异常；若仍怀疑，请带 --all 输出完整列表）")
        return 0
    finally:
        conn.close()


if __name__ == "__main__":
    raise SystemExit(main())
