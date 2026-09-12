#!/usr/bin/env python3
"""Host-side: re-arm wake notifications for stuck PENDING tasks (no restart).

Why: a task assigned while its member was mid-turn had its TASK_READY wake
notification consumed by that (busy) turn and acked with it. The product then
never re-dispatches the task: member IDLE, task PENDING forever (dispatch
liveness gap; fixed in control._schedule, but a running process keeps the old
code until restart).

What it does: for every PENDING task whose dependencies are satisfied, drop the
`task_ready_announced:<task_id>` meta marker so the product's own
announce/schedule path re-pings it on the next control activity. After running,
send any message in the TUI (that triggers the scheduler); the task will be
dispatched to its (idle) member.

Usage:
  .venv/bin/python review/tmp/reping_stuck_tasks.py                    # dry-run
  .venv/bin/python review/tmp/reping_stuck_tasks.py --apply --yes
  .venv/bin/python review/tmp/reping_stuck_tasks.py --session-dir DIR [--apply]
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
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


def locate(args) -> tuple[Path, str] | None:
    for d in candidate_dirs(args.session_dir):
        db = d / "team.db"
        try:
            conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5)
        except sqlite3.Error:
            continue
        try:
            rows = conn.execute("SELECT session_id, goal_id FROM sessions").fetchall()
        except sqlite3.Error:
            rows = []
        finally:
            conn.close()
        if args.session_dir is not None and rows:
            return d, rows[0][0]
        for sid, goal in rows:
            if goal == args.goal:
                return d, sid
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description="为滞留的 PENDING 任务重发唤醒通知（清 announced 标记）")
    ap.add_argument("--session-dir", type=Path, default=None)
    ap.add_argument("--goal", default=DEFAULT_GOAL)
    ap.add_argument("--apply", action="store_true")
    ap.add_argument("--yes", action="store_true")
    args = ap.parse_args()

    hit = locate(args)
    if hit is None:
        print(f"未找到 goal={args.goal} 的会话；可用 --session-dir 指定。")
        return 1
    session_dir, session_id = hit
    print(f"会话 {session_id}  目录 {session_dir}")

    conn = sqlite3.connect(session_dir / "team.db", timeout=5)
    conn.row_factory = sqlite3.Row
    try:
        tasks = conn.execute(
            "SELECT * FROM tasks WHERE session_id=? AND status='PENDING' ORDER BY created_at",
            (session_id,)).fetchall()
        by_id = {t["task_id"]: t for t in tasks}
        candidates: list[sqlite3.Row] = []
        for t in tasks:
            deps = json.loads(t["dependencies"] or "[]")
            dep_rows = [by_id.get(d) for d in deps]
            # deps may also be terminal (SUCCEEDED) already
            ok = True
            for d in deps:
                row = conn.execute("SELECT status FROM tasks WHERE task_id=?", (d,)).fetchone()
                if row is None or row["status"] != "SUCCEEDED":
                    ok = False
                    break
            if ok:
                candidates.append(t)

        if not candidates:
            print("没有「依赖已满足的 PENDING 任务」——无需处理。")
            return 0

        print("将处理（清 task_ready_announced 标记，让产品自己重发通知）：")
        for t in candidates:
            print(f"  {t['task_id']} assignee={t['assignee']}  {t['description'][:60]!r}")
        if not args.apply:
            print("（dry-run，未写入；加 --apply 执行）")
            return 0
        if not args.yes:
            try:
                answer = input("执行？[y/N] ").strip().lower()
            except EOFError:
                answer = ""
            if answer != "y":
                print("已取消。")
                return 0

        changed = 0
        with conn:  # transaction
            for t in candidates:
                key = f"task_ready_announced:{t['task_id']}"
                cur = conn.execute("DELETE FROM meta WHERE key=?", (key,))
                changed += cur.rowcount
                print(f"  已清除 {key}" if cur.rowcount else f"  （无标记 {key}，跳过）")
        print(f"完成：{changed} 个标记已清除。")
        print("下一步：在 TUI 里随便发一条消息（或等 Leader 活动）以触发调度；"
              "任务会派发给空闲成员。重开会话（restart）则无需此步。")
        return 0
    finally:
        conn.close()


if __name__ == "__main__":
    raise SystemExit(main())
