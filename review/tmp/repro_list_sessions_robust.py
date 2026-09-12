"""Probe: list_sessions vs a broken symlink / unreadable file inside a session dir."""
from __future__ import annotations
import os, shutil, sqlite3, sys
from pathlib import Path
sys.path.insert(0, "src")
ROOT = Path("review/tmp/scratch/listsessions").resolve()
shutil.rmtree(ROOT, ignore_errors=True)
os.environ["XDG_STATE_HOME"] = str(ROOT / "state")
from teamagents.config import sessions_dir
from teamagents.sessions import list_sessions
s = sessions_dir() / "s1"; s.mkdir(parents=True)
conn = sqlite3.connect(s / "team.db")
conn.executescript("CREATE TABLE sessions(session_id TEXT, cwd TEXT, status TEXT, goal_state TEXT,"
                   " permissions_mode TEXT, updated_at REAL); CREATE TABLE events(event_id TEXT);"
                   " CREATE TABLE tasks(task_id TEXT);")
conn.execute("INSERT INTO sessions VALUES('s1', ?, 'ACTIVE', '?', '?', 1.0)", (str(ROOT),))
conn.commit(); conn.close()
print("listing with a healthy dir:", [(i.session_id, i.size_mb) for i in list_sessions()], flush=True)
os.symlink(s / "does-not-exist", s / "broken-link")   # shell tools create these easily
try:
    print("listing with a broken symlink:", [(i.session_id, i.size_mb) for i in list_sessions()], flush=True)
except Exception as e:
    print(f"listing RAISED {type(e).__name__}: {e}", flush=True)
