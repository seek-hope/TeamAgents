"""Session records: list, archive, delete.

Complements `session.py` (which bootstraps a live runtime): this module is the
inventory view over `$XDG_STATE_HOME/teamagents/sessions`, used by the CLI and
the TUI. Deleting never loses member work silently (plan sections 9.1/12.3).
"""

from __future__ import annotations

import contextlib
import dataclasses
import fcntl
import os
import shutil
import sqlite3
import subprocess
import time
from pathlib import Path

from .config import default_session_id, sessions_dir


@dataclasses.dataclass
class SessionInfo:
    session_id: str
    path: Path
    cwd: str = ""
    status: str = "?"
    goal_state: str = "?"
    permissions_mode: str = "?"
    updated_at: float = 0.0
    events: int = 0
    tasks: int = 0
    size_mb: float = 0.0
    archived: bool = False
    locked: bool = False          # another process holds the execution lock
    error: str | None = None

    @property
    def running(self) -> bool:
        return self.locked


class SessionInUse(RuntimeError):
    """The session is currently running in another process."""


class SessionDeleteBlocked(RuntimeError):
    """Deletion would lose unmerged or uncommitted member work."""


def archived_dir() -> Path:
    return sessions_dir() / "archived"


def is_session_locked(session_id: str, base: Path | None = None) -> bool:
    """True when another process holds the session's execution lock."""
    root = base or sessions_dir()
    lock_path = root / session_id / "session.lock"
    if not lock_path.exists():
        return False
    handle = os.open(lock_path, os.O_RDWR)
    try:
        fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(handle, fcntl.LOCK_UN)
        return False
    except BlockingIOError:
        return True
    finally:
        os.close(handle)


def _read_meta(path: Path) -> dict:
    db = path / "team.db"
    if not db.exists():
        return {}
    try:
        conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        conn.row_factory = sqlite3.Row
        row = conn.execute(
            "SELECT status, cwd, permissions_mode, goal_state, updated_at"
            " FROM sessions ORDER BY updated_at DESC LIMIT 1").fetchone()
        meta = dict(row) if row else {}
        meta["events"] = conn.execute("SELECT COUNT(*) FROM events").fetchone()[0]
        meta["tasks"] = conn.execute("SELECT COUNT(*) FROM tasks").fetchone()[0]
        conn.close()
        return meta
    except Exception as e:      # a damaged session must not break the listing
        return {"error": str(e)}


def list_sessions(cwd: Path | None = None, include_archived: bool = True,
                  base: Path | None = None) -> list[SessionInfo]:
    """All session records; `cwd` restricts to one working directory."""
    root = base or sessions_dir()
    wanted = str(Path(cwd).resolve()) if cwd is not None else None
    groups = [(root, False)]
    if include_archived:
        groups.append((root / "archived", True))
    infos: list[SessionInfo] = []
    for group_root, archived in groups:
        if not group_root.is_dir():
            continue
        for path in sorted(group_root.iterdir()):
            if not path.is_dir() or not (path / "team.db").exists():
                continue
            meta = _read_meta(path)
            info = SessionInfo(
                session_id=path.name, path=path, archived=archived,
                cwd=str(meta.get("cwd") or ""), status=str(meta.get("status") or "?"),
                goal_state=str(meta.get("goal_state") or "?"),
                permissions_mode=str(meta.get("permissions_mode") or "?"),
                updated_at=float(meta.get("updated_at") or 0.0),
                events=int(meta.get("events") or 0), tasks=int(meta.get("tasks") or 0),
                locked=is_session_locked(path.name, group_root),
                error=meta.get("error"))
            info.size_mb = round(sum(f.stat().st_size for f in path.rglob("*")
                                     if f.is_file()) / 1e6, 1)
            if (not archived and wanted is not None
                    and str(Path(info.cwd).resolve() if info.cwd else "") != wanted):
                continue
            infos.append(info)
    infos.sort(key=lambda i: (i.archived, -i.updated_at))
    return infos


def new_session_id(cwd: Path) -> str:
    """A fresh session id for this directory (the default id stays stable)."""
    root = sessions_dir()
    existing = {p.name for p in root.iterdir()} if root.is_dir() else set()
    base = default_session_id(cwd)
    if base not in existing:
        return base
    index = 2
    while f"{base}_{index}" in existing:
        index += 1
    return f"{base}_{index}"


def _member_worktrees(path: Path) -> list[Path]:
    """Member work directories that are git worktrees (`.git` file, not dir)."""
    found: list[Path] = []
    members = path / "members"
    if members.is_dir():
        for member in sorted(members.iterdir()):
            work = member / "work"
            if (work / ".git").is_file():
                found.append(work)
    return found


def _worktree_branch(work: Path) -> str | None:
    with contextlib.suppress(Exception):
        out = subprocess.run(
            ["git", "-C", str(work), "rev-parse", "--abbrev-ref", "HEAD"],
            capture_output=True, text=True, timeout=30).stdout.strip()
        return out or None
    return None


def archive_session(session_id: str, *, base: Path | None = None) -> Path:
    """Move a session out of the active list; all records are preserved."""
    root = base or sessions_dir()
    source = root / session_id
    if not source.is_dir():
        raise FileNotFoundError(f"unknown session {session_id!r}")
    if is_session_locked(session_id, root):
        raise SessionInUse(f"session {session_id!r} is running in another process")
    with contextlib.suppress(Exception):
        conn = sqlite3.connect(source / "team.db")
        conn.execute("UPDATE sessions SET status='CLOSED', updated_at=?", (time.time(),))
        conn.commit()
        conn.close()
    target = root / "archived" / session_id
    if target.exists():
        target = root / "archived" / f"{session_id}_{int(time.time())}"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.move(str(source), str(target))
    return target


def delete_session(session_id: str, *, base: Path | None = None) -> None:
    """Remove a session's records.

    Refuses while it runs elsewhere, and refuses to delete member worktrees
    that still hold uncommitted or unmerged work.
    """
    root = base or sessions_dir()
    path = root / session_id
    if not path.is_dir():
        raise FileNotFoundError(f"unknown session {session_id!r}")
    if is_session_locked(session_id, root):
        raise SessionInUse(f"session {session_id!r} is running in another process")
    project_cwd = _read_meta(path).get("cwd")
    worktrees = _member_worktrees(path)
    if worktrees and not project_cwd:
        raise SessionDeleteBlocked(
            "session has member worktrees but its project directory is unknown; "
            "remove them manually first")
    from .models import WorkspacePolicy
    from .workspace import Workspace, cleanup

    for work in worktrees:
        workspace = Workspace(path=work, policy=WorkspacePolicy.GIT_WORKTREE,
                              branch=_worktree_branch(work))
        ok, reason = cleanup(workspace, Path(project_cwd or path))
        if not ok:
            raise SessionDeleteBlocked(
                f"member worktree {work} keeps unmerged or uncommitted work: {reason}")
    shutil.rmtree(path)
