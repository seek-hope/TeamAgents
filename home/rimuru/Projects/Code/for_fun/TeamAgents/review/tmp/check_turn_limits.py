#!/usr/bin/env python3
"""Read-only: show the turn limits *stored in each session DB* (host side).

Why: a session keeps its own TeamSpec snapshot (team_specs table). The runtime
and the TUI settings panel both read `limits` from that snapshot -- changing the
code defaults or user config does NOT rewrite existing sessions. This diagnostic
lists every session DB it can find, its stored limits, and flags values that
differ from the current code defaults, so we can tell whether an update landed
in the DB the running TUI actually uses.

Usage:
  .venv/bin/python review/tmp/check_turn_limits.py                 # default dirs
  .venv/bin/python review/tmp/check_turn_limits.py DIR [DIR ...]   # extra dirs
  .venv/bin/python review/tmp/check_turn_limits.py --goal goal_xxx # filter
  .venv/bin/python review/tmp/check_turn_limits.py --scan-root ~   # rglob team.db

Nothing is written; safe while the TUI is running.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))


def default_dirs() -> list[Path]:
    dirs: list[Path] = []
    try:
        from teamagents.config import sessions_dir

        dirs.append(Path(sessions_dir()))
    except Exception:
        pass
    dirs.append(Path.home() / ".local" / "state" / "teamagents" / "sessions")
    dirs.append(REPO / ".local" / "state" / "teamagents" / "sessions")
    return dirs


def db_paths(dirs: list[Path], scan_root: Path | None) -> list[Path]:
    out: list[Path] = []
    seen: set[str] = set()

    def add(p: Path) -> None:
        key = str(p.resolve())
        if key not in seen and p.is_file():
            seen.add(key)
            out.append(p)

    for base in dirs:
        if base.is_file() and base.name == "team.db":
            add(base)
            continue
        if not base.is_dir():
            continue
        add(base / "team.db")  # the dir itself may be a session dir
        for sub in sorted(base.iterdir()):
            if sub.is_dir():
                add(sub / "team.db")
    if scan_root is not None:
        for p in sorted(Path(scan_root).expanduser().rglob("team.db")):
            if ".venv" not in p.parts:
                add(p)
    return out


def read_db(db: Path) -> list[dict]:
    rows: list[dict] = []
    try:
        conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=5)
    except sqlite3.Error as exc:
        return [{"error": str(exc)}]
    conn.row_factory = sqlite3.Row
    try:
        sessions = conn.execute(
            "SELECT session_id, goal_id, goal_state FROM sessions"
        ).fetchall()
        for s in sessions:
            sid = s["session_id"]
            spec = conn.execute(
                "SELECT revision, spec_json FROM team_specs WHERE session_id=?"
                " ORDER BY revision DESC LIMIT 1",
                (sid,),
            ).fetchone()
            limits: dict = {}
            revision = None
            if spec is not None:
                revision = spec["revision"]
                try:
                    limits = (json.loads(spec["spec_json"]) or {}).get("limits") or {}
                except json.JSONDecodeError:
                    limits = {}
            agents: list[str] = []
            try:
                for a in conn.execute(
                    "SELECT agent_id, config_revision FROM agent_runtime"
                    " WHERE session_id=? ORDER BY agent_id",
                    (sid,),
                ):
                    agents.append(f"{a['agent_id']}@{a['config_revision']}")
            except sqlite3.Error:
                pass
            rows.append({
                "session_id": sid,
                "goal_id": s["goal_id"],
                "goal_state": s["goal_state"],
                "spec_revision": revision,
                "limits": limits,
                "agents": agents,
            })
    except sqlite3.Error as exc:
        rows.append({"error": str(exc)})
    finally:
        conn.close()
    return rows


def main() -> int:
    ap = argparse.ArgumentParser(description="列出各会话 DB 中存储的回合限额（只读）")
    ap.add_argument("extra_dirs", nargs="*", type=Path,
                    help="额外的会话目录（含 team.db）")
    ap.add_argument("--goal", default=None, help="只显示该 goal 的会话")
    ap.add_argument("--scan-root", type=Path, default=None,
                    help="在该目录下递归找 team.db（可能较慢）")
    args = ap.parse_args()

    from teamagents.models import Limits

    defaults = Limits().model_dump()
    dirs = default_dirs() + list(args.extra_dirs)
    seen_dirs: list[Path] = []
    for d in dirs:
        if d not in seen_dirs:
            seen_dirs.append(d)
    dirs = seen_dirs

    found = db_paths(dirs, args.scan_root)
    print(f"扫描目录：{', '.join(str(d) for d in dirs)}")
    if args.scan_root is not None:
        print(f"递归扫描：{args.scan_root}")
    print(f"找到 {len(found)} 个 team.db\n")

    by_session: dict[str, list[str]] = {}
    total = 0
    legacy = 0
    for db in found:
        for row in read_db(db):
            if "error" in row:
                print(f"[!] {db} 读取出错：{row['error']}\n")
                continue
            if args.goal and row["goal_id"] != args.goal:
                continue
            total += 1
            by_session.setdefault(row["session_id"], []).append(str(db))
            limits = row["limits"]
            steps = limits.get("max_model_steps_per_turn")
            timeout = limits.get("turn_active_timeout_s")
            flag = []
            if steps != defaults["max_model_steps_per_turn"]:
                flag.append(f"步骤 {steps} ≠ 默认 {defaults['max_model_steps_per_turn']}")
            if timeout != defaults["turn_active_timeout_s"]:
                flag.append(f"超时 {timeout} ≠ 默认 {defaults['turn_active_timeout_s']}")
            if flag:
                legacy += 1
            print(f"会话 {row['session_id']}")
            print(f"  目录：{db.parent}")
            print(f"  goal={row['goal_id']}  state={row['goal_state']}"
                  f"  spec_revision={row['spec_revision']}")
            print(f"  limits: {limits}")
            if row["agents"]:
                print(f"  成员 config_revision: {', '.join(row['agents'])}")
            print(f"  -> {'；'.join(flag) if flag else '与当前默认一致'}\n")

    dup = {sid: paths for sid, paths in by_session.items() if len(paths) > 1}
    if dup:
        print("[!] 同一会话出现在多个位置（可能有副本）：")
        for sid, paths in dup.items():
            print(f"  {sid}: {paths}")
        print()
    print(f"合计 {total} 个会话；其中 {legacy} 个存储限额与当前默认不同。")
    if legacy:
        print("升级某个会话：review/tmp/raise_turn_limits.py --session-dir <目录> --apply --yes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
