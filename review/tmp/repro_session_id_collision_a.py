"""Repro: archiving a session frees its id, so the *next* new session reuses it."""
from __future__ import annotations
import os, shutil, sys
from pathlib import Path
sys.path.insert(0, "src")
ROOT = Path("review/tmp/scratch/idcollide").resolve()
shutil.rmtree(ROOT, ignore_errors=True)
os.environ["XDG_STATE_HOME"] = str(ROOT / "state")
PROJECT = ROOT / "project"; PROJECT.mkdir(parents=True)

from teamagents.config import default_session_id, sessions_dir
from teamagents.sessions import archive_session, list_sessions, new_session_id

sessions_dir().mkdir(parents=True, exist_ok=True)
base = default_session_id(PROJECT)
print("default id for this dir:", base)
first = new_session_id(PROJECT)
print("new_session_id (nothing exists):", first)
# create the session directory the way open_session would
d = sessions_dir() / first; d.mkdir(parents=True)
import sqlite3
conn = sqlite3.connect(d / "team.db")
conn.execute("CREATE TABLE sessions(session_id TEXT, cwd TEXT, status TEXT, goal_state TEXT,"
             " permissions_mode TEXT, updated_at REAL)")
conn.execute("CREATE TABLE events(event_id TEXT)")
conn.execute("CREATE TABLE tasks(task_id TEXT)")
conn.execute("INSERT INTO sessions VALUES(?,?,?,?,?,?)", (first, str(PROJECT), "ACTIVE", "?", "?", 1.0))
conn.commit(); conn.close()

archive_session(first)
print("archived:", [p.name for p in (sessions_dir() / "archived").iterdir()])
second = new_session_id(PROJECT)
print("new_session_id after archiving:", second, "-> collides:", second == first)
(d2 := sessions_dir() / second).mkdir(parents=True, exist_ok=True)
conn = sqlite3.connect(d2 / "team.db")
conn.executescript("CREATE TABLE sessions(session_id TEXT, cwd TEXT, status TEXT, goal_state TEXT,"
                   " permissions_mode TEXT, updated_at REAL); CREATE TABLE events(event_id TEXT);"
                   " CREATE TABLE tasks(task_id TEXT);")
conn.execute("INSERT INTO sessions VALUES(?,?,?,?,?,?)", (second, str(PROJECT), "ACTIVE", "?", "?", 2.0))
conn.commit(); conn.close()
rows = list_sessions(cwd=PROJECT)
print("list_sessions ids+archived:", [(r.session_id, r.archived) for r in rows])
ids = [r.session_id for r in rows]
print("duplicate session ids visible to the UI:", len(ids) != len(set(ids)))

