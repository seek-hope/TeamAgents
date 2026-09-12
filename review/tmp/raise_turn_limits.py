#!/usr/bin/env python3
"""Host-side: raise this session's stored turn limits to the current defaults.

Why: sessions created before D-10 store `limits.max_model_steps_per_turn: 50`
(the legacy default). The runner reads the *stored* TeamSpec, so such sessions
still die at 50 model steps per turn (observed 2026-09-12: four implementer
turns killed with "model-step limit 50 reached", while the code default is 200;
`Store.load_team_spec` only drops *removed* keys, it keeps stored values).

What it does, through the product's own storage API:
  1. load the latest TeamSpec of the session;
  2. set `limits` to the current code defaults (`Limits()`), or `--steps/--timeout`
     overrides on top;
  3. save it as a new revision;
  4. bump every agent's config revision so running backends rebuild their graphs
     on the next scheduling pass (no TUI restart needed).

Usage:
  .venv/bin/python review/tmp/raise_turn_limits.py                # dry-run: locate + show
  .venv/bin/python review/tmp/raise_turn_limits.py --apply        # confirm, then write
  .venv/bin/python review/tmp/raise_turn_limits.py --apply --yes  # no prompt
  .venv/bin/python review/tmp/raise_turn_limits.py --session-dir <dir> [--apply]
"""

from __future__ import annotations

import argparse
import sqlite3
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))

DEFAULT_GOAL = "goal_b3de92c12a7d45a3"


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


def read_sessions(session_dir: Path) -> list[tuple[str, str | None, str]]:
    """(session_id, goal_id, goal_state) read-only; [] when unreadable."""
    db = session_dir / "team.db"
    try:
        conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5)
    except sqlite3.Error:
        return []
    try:
        rows = conn.execute(
            "SELECT session_id, goal_id, goal_state FROM sessions"
        ).fetchall()
        return [(r[0], r[1], r[2]) for r in rows]
    except sqlite3.Error:
        return []
    finally:
        conn.close()


def main() -> int:
    ap = argparse.ArgumentParser(description="把会话 TeamSpec 的 turns 限额升到当前默认（D-10）")
    ap.add_argument("--session-dir", type=Path, default=None,
                    help="会话目录（含 team.db）；默认按 goal 自动定位")
    ap.add_argument("--goal", default=DEFAULT_GOAL, help="用 goal id 定位会话")
    ap.add_argument("--steps", type=int, default=None, help="覆盖 max_model_steps_per_turn")
    ap.add_argument("--timeout", type=int, default=None, help="覆盖 turn_active_timeout_s")
    ap.add_argument("--apply", action="store_true", help="真正写入（默认 dry-run）")
    ap.add_argument("--yes", action="store_true", help="跳过确认")
    ap.add_argument("--any", action="store_true",
                    help="忽略 goal 过滤（配合 --session-dir 使用）")
    args = ap.parse_args()

    hits: list[tuple[Path, str]] = []
    dirs = list(candidate_dirs(args.session_dir))
    for d in dirs:
        for sid, goal, _state in read_sessions(d):
            if args.any or goal == args.goal:
                hits.append((d, sid))
    if not hits and args.session_dir is not None:
        # explicit dir wins: a sole session there is the target even when its
        # goal column differs from the one we were told about
        sessions = read_sessions(args.session_dir)
        if len(sessions) == 1:
            hits.append((args.session_dir, sessions[0][0]))
            print(f"（--session-dir 指定且唯一会话：{sessions[0][0]}）")
    if not hits and args.session_dir is None:
        if len(dirs) == 1:
            sessions = read_sessions(dirs[0])
            if len(sessions) == 1:
                hits.append((dirs[0], sessions[0][0]))
                print(f"（未按 goal 匹配到，按唯一会话定位：{sessions[0][0]}）")
    if not hits:
        print(f"没有找到 goal={args.goal} 的会话。")
        print("现有会话：")
        for d in dirs:
            for sid, goal, state in read_sessions(d):
                print(f"  {sid}  goal={goal}  state={state}  {d}")
        print("可用 --session-dir 指定其一。")
        return 1
    if len(hits) > 1:
        print("找到多个候选，请用 --session-dir 指定其一：")
        for d, sid in hits:
            print(f"  {sid}  {d}")
        return 1

    session_dir, session_id = hits[0]
    print(f"会话：{session_id}")
    print(f"目录：{session_dir}")

    from teamagents.models import Limits
    from teamagents.storage import Store

    store = Store(session_dir / "team.db")
    spec = store.load_team_spec(session_id)
    before = spec.limits
    print("当前限额：", before.model_dump())

    desired = Limits().model_dump()
    if args.steps is not None:
        desired["max_model_steps_per_turn"] = args.steps
    if args.timeout is not None:
        desired["turn_active_timeout_s"] = args.timeout
    if desired == before.model_dump():
        print("无需修改：限额已与目标一致。")
        return 0

    print("目标限额：", desired)
    if not args.apply:
        print("（dry-run，未写入；加 --apply 执行）")
        return 0
    if not args.yes:
        try:
            answer = input("执行？[y/N] ").strip().lower()
        except EOFError:
            answer = ""
        if answer != "y":
            print("已取消（未确认；可加 --yes 免交互）。")
            return 0

    new_spec = spec.model_copy(update={"limits": Limits.model_validate(desired)})
    revision = store.save_team_spec(session_id, new_spec)
    print(f"已保存 TeamSpec revision={revision}")
    for agent in new_spec.agents:
        try:
            rev = store.bump_config_revision(session_id, agent.id)
            print(f"  成员 {agent.id}: config_revision -> {rev}")
        except Exception as exc:  # member without runtime row: fine
            print(f"  成员 {agent.id}: 跳过（{exc}）")

    check = store.load_team_spec(session_id)
    print("生效后限额：", check.limits.model_dump())
    print("即可继续；运行中的会话会在下一次调度时重建各成员后端。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
