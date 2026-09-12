#!/usr/bin/env python3
"""Host-side recovery for the stuck review session (run where the TUI runs).

Clears the two leftovers the running UI cannot clear today:
  * task_528b1b39c8c1       (BLOCKED; no tool/agent may complete it - finding A-03)
  * appr_cfcb4a86bc4048d0   (PENDING; its run is long gone - finding RT-06)

It submits the sanctioned user control actions through the product's own
control plane (Control.submit -> validate -> reduce -> persist, with events and
idempotent receipts), i.e. the same transaction path a TUI button would use.

Usage:
  .venv/bin/python review/tmp/unblock_session.py                # dry-run: locate + show
  .venv/bin/python review/tmp/unblock_session.py --apply        # confirm, then submit
  .venv/bin/python review/tmp/unblock_session.py --apply --yes  # no prompt

Options may repeat: --task ID --task ID ... / --approval ID --approval ID ...
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))

DEFAULT_TASK = "task_528b1b39c8c1"
DEFAULT_APPROVAL = "appr_cfcb4a86bc4048d0"


def candidate_dirs(explicit: Path | None):
    """Session dirs to scan; explicit wins, else XDG state + HOME/repo fallbacks."""
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


def probe(session_dir: Path, task_ids: list[str], approval_ids: list[str]) -> str | None:
    """Return the session_id if this DB holds one of the target rows (read-only first)."""
    db = session_dir / "team.db"
    for mode in ("ro", "rw"):
        try:
            if mode == "ro":
                conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5)
            else:
                conn = sqlite3.connect(db, timeout=5)
        except sqlite3.Error:
            continue
        try:
            for tid in task_ids:
                row = conn.execute(
                    "SELECT session_id FROM tasks WHERE task_id=?", (tid,)
                ).fetchone()
                if row:
                    return row[0]
            for aid in approval_ids:
                row = conn.execute(
                    "SELECT session_id FROM approvals WHERE approval_id=?", (aid,)
                ).fetchone()
                if row:
                    return row[0]
            return None
        except sqlite3.Error:
            continue
        finally:
            conn.close()
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description="清除会话里 TUI 无法处理的 BLOCKED 任务/残留批准")
    ap.add_argument("--session-dir", type=Path, default=None,
                    help="会话目录（含 team.db）；默认自动扫描 XDG/HOME 下的会话目录")
    ap.add_argument("--task", action="append", dest="tasks", default=None, help="任务 id，可重复")
    ap.add_argument("--approval", action="append", dest="approvals", default=None,
                    help="批准 id，可重复")
    ap.add_argument("--apply", action="store_true", help="真正提交（默认 dry-run）")
    ap.add_argument("--yes", action="store_true", help="跳过确认")
    args = ap.parse_args()

    tasks = args.tasks or [DEFAULT_TASK]
    approvals = args.approvals or [DEFAULT_APPROVAL]

    hits: list[tuple[Path, str]] = []
    for d in candidate_dirs(args.session_dir):
        sid = probe(d, tasks, approvals)
        if sid:
            hits.append((d, sid))
    if not hits:
        print("没有找到包含目标任务/批准的会话 DB。")
        print("可先用 `uv run teamagents sessions` 查看会话目录，再用 --session-dir 指定；")
        print("若已经是清理过的状态，则无需操作。")
        return 1
    if len(hits) > 1:
        print("找到多个候选，请用 --session-dir 指定其一：")
        for d, sid in hits:
            print(f"  {sid}  {d}")
        return 1

    session_dir, session_id = hits[0]
    print(f"会话：{session_id}")
    print(f"目录：{session_dir}")

    from teamagents.control import Control
    from teamagents.models import ActionKind, ApprovalStatus, TaskStatus, TeamAction
    from teamagents.storage import Store

    store = Store(session_dir / "team.db")
    control = Control(store, session_id)

    def show_state() -> None:
        for tid in tasks:
            t = store.get_task(tid)
            print(f"  {tid}: {t.status if t else '缺失'}")
        for aid in approvals:
            a = store.get_approval(aid)
            print(f"  {aid}: {a.status if a else '缺失'}")

    def report_rest() -> None:
        unfinished = store.tasks_for_session(
            session_id, [TaskStatus.PENDING, TaskStatus.RUNNING, TaskStatus.BLOCKED])
        pending = store.pending_approvals(session_id)
        print("本会话剩余未完成任务：",
              [f"{t.task_id}:{t.status}" for t in unfinished] or "无")
        print("本会话剩余待批准：", [a.approval_id for a in pending] or "无")
        try:
            spec = store.load_team_spec(session_id)
            blockers = control._completion_blockers(spec)
            print("完成阻塞项：", blockers or "无")
            if any(str(b).startswith("active turns:") for b in blockers):
                print("  （若只剩 Leader 的活跃回合：那是本次取消推送的通知回合，走完即消失）")
        except Exception as exc:  # best-effort internal check
            print("（阻塞项检查跳过）", exc)

    print("当前：")
    show_state()

    todo: list[tuple[str, ActionKind, dict]] = []
    for tid in tasks:
        t = store.get_task(tid)
        if t is not None and t.status not in (
                TaskStatus.SUCCEEDED, TaskStatus.FAILED, TaskStatus.CANCELLED):
            todo.append((f"cancel-task-{tid}", ActionKind.CANCEL_TASK, {"task_id": tid}))
    for aid in approvals:
        a = store.get_approval(aid)
        if a is not None and a.status is ApprovalStatus.PENDING:
            todo.append((f"deny-approval-{aid}", ActionKind.APPROVAL_DECISION,
                         {"approval_id": aid, "decision": "deny"}))

    if not todo:
        print("无需提交：目标项都已在终态。")
        report_rest()
        return 0

    print("将提交（user 身份，走产品控制面）：")
    for _, kind, payload in todo:
        print(f"  {kind} {payload}")
    if not args.apply:
        print("（dry-run，未提交；加 --apply 执行）")
        return 0
    if not args.yes:
        try:
            answer = input("执行？[y/N] ").strip().lower()
        except EOFError:
            answer = ""
        if answer != "y":
            print("已取消（未确认；可加 --yes 免交互）。")
            return 0

    for tag, kind, payload in todo:
        receipt = control.submit(TeamAction(
            action_id=f"manual-{tag}", session_id=session_id, actor_id="user",
            kind=kind, payload=payload))
        print(f"{kind}: ok={receipt.ok} result={receipt.result} error={receipt.error}")

    print("现在：")
    show_state()
    report_rest()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
