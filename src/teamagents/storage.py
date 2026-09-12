"""SQLite business database: the authority for team facts and action receipts.

One writer per session (the session runtime serializes via an asyncio lock),
transactions + constraints, WAL, busy timeout (plan section 9.1).
"""

from __future__ import annotations

import json
import sqlite3
import threading
import time
from pathlib import Path
from typing import Any, Iterable

from .models import (
    AgentStatus,
    ApprovalRequest,
    ApprovalStatus,
    Limits,
    PatchStatus,
    Receipt,
    SessionStatus,
    SharedEntry,
    Task,
    TaskStatus,
    TeamEvent,
    TeamSpec,
    TopologyPatch,
    TurnRun,
    TurnStatus,
)

DB_SCHEMA_VERSION = 1

SCHEMA = """
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS sessions(
  session_id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  cwd TEXT NOT NULL,
  permissions_mode TEXT NOT NULL DEFAULT 'approved_scope',
  goal_id TEXT,
  goal_state TEXT NOT NULL DEFAULT 'idle',
  created_at REAL NOT NULL,
  updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS team_specs(
  session_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  spec_json TEXT NOT NULL,
  created_at REAL NOT NULL,
  PRIMARY KEY(session_id, revision)
);

CREATE TABLE IF NOT EXISTS tasks(
  task_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  parent_task_id TEXT,
  goal_id TEXT,
  requester TEXT NOT NULL,
  assignee TEXT NOT NULL,
  description TEXT NOT NULL,
  acceptance TEXT NOT NULL DEFAULT '',
  dependencies TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL,
  result_refs TEXT NOT NULL DEFAULT '[]',
  cancel_requested INTEGER NOT NULL DEFAULT 0,
  created_at REAL NOT NULL,
  updated_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_session ON tasks(session_id, status);

CREATE TABLE IF NOT EXISTS turn_runs(
  run_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  task_id TEXT,
  goal_id TEXT,
  agent_id TEXT NOT NULL,
  config_revision INTEGER NOT NULL,
  topology_revision INTEGER NOT NULL,
  status TEXT NOT NULL,
  input_delivery_ids TEXT NOT NULL DEFAULT '[]',
  context_ref TEXT,
  external_turn_id TEXT,
  cancel_requested INTEGER NOT NULL DEFAULT 0,
  waiting_on TEXT NOT NULL DEFAULT '[]',
  created_at REAL NOT NULL,
  updated_at REAL NOT NULL
);
-- at most one active turn per member (plan section 2.2 rule 5)
CREATE UNIQUE INDEX IF NOT EXISTS uniq_active_run
  ON turn_runs(agent_id) WHERE status = 'RUNNING';
CREATE INDEX IF NOT EXISTS idx_runs_session ON turn_runs(session_id, status);

CREATE TABLE IF NOT EXISTS actions(
  action_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  run_id TEXT,
  kind TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  receipt_json TEXT NOT NULL,
  created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS events(
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  session_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  task_id TEXT,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  audience_json TEXT NOT NULL,
  topology_revision INTEGER NOT NULL,
  causation_id TEXT,
  created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, sequence);

CREATE TABLE IF NOT EXISTS deliveries(
  delivery_id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  batch_no INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  payload_override TEXT,
  created_at REAL NOT NULL,
  applied_at REAL
);
CREATE INDEX IF NOT EXISTS idx_deliveries_agent ON deliveries(session_id, agent_id, status);

CREATE TABLE IF NOT EXISTS agent_runtime(
  session_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'IDLE',
  config_revision INTEGER NOT NULL DEFAULT 1,
  context_epoch INTEGER NOT NULL DEFAULT 1,
  last_applied_batch INTEGER NOT NULL DEFAULT 0,
  next_batch_no INTEGER NOT NULL DEFAULT 1,
  external_thread_id TEXT,
  updated_at REAL NOT NULL,
  PRIMARY KEY(session_id, agent_id)
);

CREATE TABLE IF NOT EXISTS approvals(
  approval_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  run_id TEXT NOT NULL,
  tool_call_id TEXT NOT NULL,
  operation_hash TEXT NOT NULL,
  requested_scope TEXT NOT NULL,
  policy_revision INTEGER NOT NULL,
  status TEXT NOT NULL,
  created_at REAL NOT NULL,
  decided_at REAL
);
CREATE INDEX IF NOT EXISTS idx_approvals_session ON approvals(session_id, status);

CREATE TABLE IF NOT EXISTS shared_entries(
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  entry_id TEXT NOT NULL UNIQUE,
  session_id TEXT NOT NULL,
  space_id TEXT NOT NULL,
  author TEXT NOT NULL,
  kind TEXT NOT NULL,
  content TEXT NOT NULL DEFAULT '',
  ref TEXT,
  supersedes TEXT,
  created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shared_space ON shared_entries(session_id, space_id, sequence);

CREATE TABLE IF NOT EXISTS topology_patches(
  patch_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  base_revision INTEGER NOT NULL,
  proposer TEXT NOT NULL,
  decided_by TEXT,
  operations TEXT NOT NULL,
  affected_agents TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at REAL NOT NULL,
  updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS completion_requests(
  run_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  result_refs TEXT NOT NULL DEFAULT '[]',
  summary TEXT NOT NULL DEFAULT '',
  created_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS shared_cursors(
  session_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  space_id TEXT NOT NULL,
  sequence INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(session_id, agent_id, space_id)
);

CREATE TABLE IF NOT EXISTS session_approval_cache(
  session_id TEXT NOT NULL,
  operation_hash TEXT NOT NULL,
  scope_json TEXT NOT NULL,
  created_at REAL NOT NULL,
  PRIMARY KEY(session_id, operation_hash)
);
"""


class Store:
    """Thin synchronous SQLite wrapper. The caller serializes writes."""

    def __init__(self, path: str | Path):
        self.path = str(path)
        if self.path != ":memory:":
            Path(self.path).parent.mkdir(parents=True, exist_ok=True)
        # tool threads may reach the store (langchain runs sync tools in a pool),
        # so the connection is shared but every statement is serialized by a lock
        self._lock = threading.RLock()
        self.conn = _LockedConn(
            sqlite3.connect(self.path, isolation_level=None, check_same_thread=False),
            self._lock)
        self.conn.row_factory = sqlite3.Row
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("PRAGMA foreign_keys=ON")
        self.conn.execute("PRAGMA busy_timeout=5000")
        self.conn.executescript(SCHEMA)
        self._tx_depth = 0
        # spec revisions are append-only, so a validated spec never changes
        self._spec_cache: dict[tuple[str, int], TeamSpec] = {}
        self._check_schema_version()

    def _check_schema_version(self) -> None:
        row = self.conn.execute("SELECT value FROM meta WHERE key='db_schema_version'").fetchone()
        if row is None:
            self.conn.execute(
                "INSERT INTO meta(key, value) VALUES('db_schema_version', ?)",
                (str(DB_SCHEMA_VERSION),),
            )
        elif int(row["value"]) != DB_SCHEMA_VERSION:
            raise RuntimeError(
                f"database schema version {row['value']} != supported {DB_SCHEMA_VERSION}; "
                "back up and migrate before opening"
            )

    def close(self) -> None:
        self.conn.close()

    # -- transactions --------------------------------------------------------

    def tx(self):
        return _Tx(self)

    # -- sessions -------------------------------------------------------------

    def create_session(self, session_id: str, cwd: str, permissions_mode: str) -> None:
        t = time.time()
        with self.tx():
            self.conn.execute(
                "INSERT INTO sessions(session_id, status, cwd, permissions_mode, created_at, updated_at)"
                " VALUES(?,?,?,?,?,?)",
                (session_id, SessionStatus.ACTIVE, cwd, permissions_mode, t, t),
            )

    def get_session(self, session_id: str) -> sqlite3.Row | None:
        return self.conn.execute(
            "SELECT * FROM sessions WHERE session_id=?", (session_id,)
        ).fetchone()

    def set_session_status(self, session_id: str, status: str) -> None:
        with self.tx():
            self.conn.execute(
                "UPDATE sessions SET status=?, updated_at=? WHERE session_id=?",
                (status, time.time(), session_id),
            )

    def set_goal_state(self, session_id: str, goal_id: str, state: str) -> None:
        with self.tx():
            self.conn.execute(
                "UPDATE sessions SET goal_id=?, goal_state=?, updated_at=? WHERE session_id=?",
                (goal_id, state, time.time(), session_id),
            )

    def set_permission_mode(self, session_id: str, mode: str) -> None:
        with self.tx():
            self.conn.execute(
                "UPDATE sessions SET permissions_mode=?, updated_at=? WHERE session_id=?",
                (mode, time.time(), session_id),
            )

    # -- meta ----------------------------------------------------------------

    def get_meta(self, key: str) -> str | None:
        row = self.conn.execute("SELECT value FROM meta WHERE key=?", (key,)).fetchone()
        return row["value"] if row else None

    def set_meta(self, key: str, value: str) -> None:
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?,?)"
            " ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (key, value),
        )

    def del_meta(self, key: str) -> None:
        self.conn.execute("DELETE FROM meta WHERE key=?", (key,))

    # -- team spec -----------------------------------------------------------

    def save_team_spec(self, session_id: str, spec: TeamSpec) -> int:
        rev = self.current_revision(session_id) + 1
        with self.tx():
            self.conn.execute(
                "INSERT INTO team_specs(session_id, revision, spec_json, created_at) VALUES(?,?,?,?)",
                (session_id, rev, spec.model_dump_json(), time.time()),
            )
        return rev

    def current_revision(self, session_id: str) -> int:
        row = self.conn.execute(
            "SELECT MAX(revision) AS r FROM team_specs WHERE session_id=?", (session_id,)
        ).fetchone()
        return int(row["r"] or 0)

    def load_team_spec(self, session_id: str, revision: int | None = None) -> TeamSpec:
        if revision is None:
            revision = self.current_revision(session_id)
        cached = self._spec_cache.get((session_id, revision))
        if cached is not None:
            return cached
        row = self.conn.execute(
            "SELECT spec_json FROM team_specs WHERE session_id=? AND revision=?",
            (session_id, revision),
        ).fetchone()
        if row is None:
            raise KeyError(f"no team spec revision {revision} for session {session_id!r}")
        data = json.loads(row["spec_json"])
        # sessions written before D-10 still carry the removed limit keys; drop
        # them so old sessions keep loading (TeamSpec *files* stay strict)
        limits = data.get("limits")
        if isinstance(limits, dict):
            data["limits"] = {k: v for k, v in limits.items() if k in Limits.model_fields}
        spec = TeamSpec.model_validate(data)
        self._spec_cache[(session_id, revision)] = spec
        return spec

    # -- action dedup --------------------------------------------------------

    def get_action_receipt(self, action_id: str) -> Receipt | None:
        row = self.conn.execute(
            "SELECT receipt_json FROM actions WHERE action_id=?", (action_id,)
        ).fetchone()
        return Receipt.model_validate_json(row["receipt_json"]) if row else None

    def record_action(self, action_id: str, session_id: str, actor_id: str,
                      run_id: str | None, kind: str, payload_hash: str, receipt: Receipt) -> None:
        self.conn.execute(
            "INSERT INTO actions(action_id, session_id, actor_id, run_id, kind, payload_hash,"
            " receipt_json, created_at) VALUES(?,?,?,?,?,?,?,?)",
            (action_id, session_id, actor_id, run_id, kind, payload_hash,
             receipt.model_dump_json(), time.time()),
        )

    # -- events and deliveries ----------------------------------------------

    def append_event(self, event: TeamEvent) -> int:
        """Insert event, return its sequence. Caller holds the transaction."""
        cur = self.conn.execute(
            "INSERT INTO events(event_id, session_id, actor_id, task_id, kind, payload_json,"
            " audience_json, topology_revision, causation_id, created_at)"
            " VALUES(?,?,?,?,?,?,?,?,?,?)",
            (event.event_id, event.session_id, event.actor_id, event.task_id, event.kind,
             json.dumps(event.payload), json.dumps(event.audience),
             event.topology_revision, event.causation_id, event.created_at),
        )
        return int(cur.lastrowid)

    def events(self, session_id: str, after_sequence: int = 0, limit: int = 1000) -> list[sqlite3.Row]:
        return self.conn.execute(
            "SELECT * FROM events WHERE session_id=? AND sequence>? ORDER BY sequence LIMIT ?",
            (session_id, after_sequence, limit),
        ).fetchall()

    def create_delivery(self, session_id: str, agent_id: str, event_id: str, batch_no: int,
                        payload_override: dict | None = None) -> int:
        cur = self.conn.execute(
            "INSERT INTO deliveries(session_id, agent_id, event_id, batch_no, status,"
            " payload_override, created_at) VALUES(?,?,?,?, 'pending', ?, ?)",
            (session_id, agent_id, event_id, batch_no,
             json.dumps(payload_override) if payload_override is not None else None,
             time.time()),
        )
        return int(cur.lastrowid)

    def next_batch_no(self, session_id: str, agent_id: str) -> int:
        """Caller holds the transaction. Monotonic per member."""
        row = self.conn.execute(
            "SELECT next_batch_no FROM agent_runtime WHERE session_id=? AND agent_id=?",
            (session_id, agent_id),
        ).fetchone()
        batch = int(row["next_batch_no"]) if row else 1
        self.conn.execute(
            "INSERT INTO agent_runtime(session_id, agent_id, next_batch_no, updated_at)"
            " VALUES(?,?,?,?)"
            " ON CONFLICT(session_id, agent_id) DO UPDATE SET next_batch_no=excluded.next_batch_no,"
            " updated_at=excluded.updated_at",
            (session_id, agent_id, batch + 1, time.time()),
        )
        return batch

    def pending_deliveries(self, session_id: str, agent_id: str) -> list[sqlite3.Row]:
        return self.conn.execute(
            "SELECT d.*, e.kind AS event_kind, e.payload_json, e.actor_id AS event_actor,"
            " e.task_id AS event_task_id, e.sequence AS event_sequence, e.audience_json"
            " FROM deliveries d JOIN events e ON e.event_id = d.event_id"
            " WHERE d.session_id=? AND d.agent_id=? AND d.status='pending'"
            " ORDER BY d.batch_no, e.sequence",
            (session_id, agent_id),
        ).fetchall()

    def delivery_event_kinds(self, delivery_ids: list[int]) -> list[str]:
        if not delivery_ids:
            return []
        marks = ",".join("?" * len(delivery_ids))
        rows = self.conn.execute(
            f"SELECT e.kind FROM deliveries d JOIN events e ON e.event_id=d.event_id"
            f" WHERE d.delivery_id IN ({marks}) ORDER BY e.sequence",
            tuple(delivery_ids),
        ).fetchall()
        return [r["kind"] for r in rows]

    def applied_batch(self, session_id: str, agent_id: str) -> int:
        row = self.conn.execute(
            "SELECT last_applied_batch FROM agent_runtime WHERE session_id=? AND agent_id=?",
            (session_id, agent_id),
        ).fetchone()
        return int(row["last_applied_batch"]) if row else 0

    def ack_deliveries(self, session_id: str, agent_id: str, batch_no: int) -> int:
        """Record that a member applied a delivery batch; advance consume cursor."""
        with self.tx():
            cur = self.conn.execute(
                "UPDATE deliveries SET status='applied', applied_at=?"
                " WHERE session_id=? AND agent_id=? AND batch_no<=? AND status='pending'",
                (time.time(), session_id, agent_id, batch_no),
            )
            self.conn.execute(
                "INSERT INTO agent_runtime(session_id, agent_id, last_applied_batch, updated_at)"
                " VALUES(?,?,?,?)"
                " ON CONFLICT(session_id, agent_id) DO UPDATE SET"
                " last_applied_batch=MAX(last_applied_batch, excluded.last_applied_batch),"
                " updated_at=excluded.updated_at",
                (session_id, agent_id, batch_no, time.time()),
            )
            return cur.rowcount

    def ack_deliveries_exact(self, session_id: str, agent_id: str,
                             delivery_ids: Iterable[int]) -> int:
        """Acknowledge exactly these deliveries (never a batch range).

        A run may only ack the deliveries it actually injected; a `MAX(batch_no)`
        range silently swallows deliveries the member never saw (RT-05). Returns
        how many of them were still pending.
        """
        ids = [int(i) for i in delivery_ids]
        if not ids:
            return 0
        marks = ",".join("?" * len(ids))
        with self.tx():
            cur = self.conn.execute(
                f"UPDATE deliveries SET status='applied', applied_at=?"
                f" WHERE delivery_id IN ({marks}) AND status='pending'",
                (time.time(), *ids),
            )
            row = self.conn.execute(
                f"SELECT MAX(batch_no) AS b FROM deliveries WHERE delivery_id IN ({marks})",
                tuple(ids),
            ).fetchone()
            if row and row["b"] is not None:
                self.conn.execute(
                    "INSERT INTO agent_runtime(session_id, agent_id, last_applied_batch, updated_at)"
                    " VALUES(?,?,?,?)"
                    " ON CONFLICT(session_id, agent_id) DO UPDATE SET"
                    " last_applied_batch=MAX(last_applied_batch, excluded.last_applied_batch),"
                    " updated_at=excluded.updated_at",
                    (session_id, agent_id, int(row["b"]), time.time()),
                )
        return cur.rowcount

    def ack_run_deliveries(self, run: TurnRun) -> None:
        """Acknowledge exactly the deliveries this run was given (no batch range)."""
        self.ack_deliveries_exact(run.session_id, run.agent_id, run.input_delivery_ids)

    def waiters_for_task(self, session_id: str, task_id: str) -> list[str]:
        """Members parked in WAITING_TASK on this task (to wake with the result)."""
        out = []
        for run in self.runs_for_session(session_id, [TurnStatus.WAITING_TASK]):
            if task_id in run.waiting_on:
                out.append(run.agent_id)
        return out

    def decided_approvals_for_run(self, run_id: str) -> list[ApprovalRequest]:
        rows = self.conn.execute(
            "SELECT approval_id FROM approvals WHERE run_id=? AND status!=?",
            (run_id, ApprovalStatus.PENDING),
        ).fetchall()
        return [a for r in rows if (a := self.get_approval(r["approval_id"])) is not None]

    # -- agent runtime -------------------------------------------------------

    def ensure_agent(self, session_id: str, agent_id: str) -> None:
        with self.tx():
            self.conn.execute(
                "INSERT OR IGNORE INTO agent_runtime(session_id, agent_id, updated_at) VALUES(?,?,?)",
                (session_id, agent_id, time.time()),
            )

    def set_agent_status(self, session_id: str, agent_id: str, status: AgentStatus) -> None:
        with self.tx():
            self.conn.execute(
                "UPDATE agent_runtime SET status=?, updated_at=? WHERE session_id=? AND agent_id=?",
                (status, time.time(), session_id, agent_id),
            )

    def agent_status(self, session_id: str, agent_id: str) -> AgentStatus:
        row = self.conn.execute(
            "SELECT status FROM agent_runtime WHERE session_id=? AND agent_id=?",
            (session_id, agent_id),
        ).fetchone()
        return AgentStatus(row["status"]) if row else AgentStatus.IDLE

    def bump_config_revision(self, session_id: str, agent_id: str) -> int:
        with self.tx():
            self.conn.execute(
                "UPDATE agent_runtime SET config_revision=config_revision+1, updated_at=?"
                " WHERE session_id=? AND agent_id=?",
                (time.time(), session_id, agent_id),
            )
            row = self.conn.execute(
                "SELECT config_revision FROM agent_runtime WHERE session_id=? AND agent_id=?",
                (session_id, agent_id),
            ).fetchone()
            return int(row["config_revision"])

    def agent_config_revision(self, session_id: str, agent_id: str) -> int:
        row = self.conn.execute(
            "SELECT config_revision FROM agent_runtime WHERE session_id=? AND agent_id=?",
            (session_id, agent_id),
        ).fetchone()
        return int(row["config_revision"]) if row else 1

    # -- tasks ---------------------------------------------------------------

    def insert_task(self, session_id: str, task: Task) -> None:
        self.conn.execute(
            "INSERT INTO tasks(task_id, session_id, parent_task_id, goal_id, requester, assignee,"
            " description, acceptance, dependencies, status, result_refs, created_at, updated_at)"
            " VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (task.task_id, session_id, task.parent_task_id, task.goal_id, task.requester,
             task.assignee, task.description, task.acceptance, json.dumps(task.dependencies),
             task.status, json.dumps(task.result_refs), task.created_at, task.updated_at),
        )

    def get_task(self, task_id: str) -> Task | None:
        row = self.conn.execute("SELECT * FROM tasks WHERE task_id=?", (task_id,)).fetchone()
        return self._row_to_task(row) if row else None

    def tasks_for_session(self, session_id: str, statuses: Iterable[str] | None = None) -> list[Task]:
        if statuses:
            marks = ",".join("?" * len(statuses))
            rows = self.conn.execute(
                f"SELECT * FROM tasks WHERE session_id=? AND status IN ({marks})",
                (session_id, *statuses),
            ).fetchall()
        else:
            rows = self.conn.execute(
                "SELECT * FROM tasks WHERE session_id=?", (session_id,)
            ).fetchall()
        return [self._row_to_task(r) for r in rows]

    def compare_and_set_task(self, task_id: str, expected: str, new: str,
                             result_refs: list[str] | None = None) -> bool:
        """Compare-and-swap on task status; returns False on version mismatch."""
        refs = json.dumps(result_refs) if result_refs is not None else None
        if refs is None:
            cur = self.conn.execute(
                "UPDATE tasks SET status=?, updated_at=? WHERE task_id=? AND status=?",
                (new, time.time(), task_id, expected),
            )
        else:
            cur = self.conn.execute(
                "UPDATE tasks SET status=?, result_refs=?, updated_at=?"
                " WHERE task_id=? AND status=?",
                (new, refs, time.time(), task_id, expected),
            )
        return cur.rowcount == 1

    def set_task_cancel_requested(self, task_id: str) -> None:
        self.conn.execute(
            "UPDATE tasks SET cancel_requested=1, updated_at=? WHERE task_id=?",
            (time.time(), task_id),
        )

    def reassign_tasks(self, assignee: str, new_assignee: str, statuses: Iterable[str]) -> list[str]:
        marks = ",".join("?" * len(statuses))
        rows = self.conn.execute(
            f"SELECT task_id FROM tasks WHERE assignee=? AND status IN ({marks})",
            (assignee, *statuses),
        ).fetchall()
        ids = [r["task_id"] for r in rows]
        with self.tx():
            for tid in ids:
                self.conn.execute(
                    "UPDATE tasks SET assignee=?, updated_at=? WHERE task_id=?",
                    (new_assignee, time.time(), tid),
                )
        return ids

    def drop_pending_deliveries(self, session_id: str, agent_id: str,
                                reason: str) -> int:
        """Undelivered messages keep a failure reason instead of vanishing."""
        cur = self.conn.execute(
            "UPDATE deliveries SET status=?, payload_override=? "
            "WHERE session_id=? AND agent_id=? AND status='pending'",
            ("dropped", json.dumps({"dropped_reason": reason}), session_id, agent_id),
        )
        return cur.rowcount

    @staticmethod
    def _row_to_task(row: sqlite3.Row) -> Task:
        return Task(
            task_id=row["task_id"], parent_task_id=row["parent_task_id"], goal_id=row["goal_id"],
            requester=row["requester"], assignee=row["assignee"], description=row["description"],
            acceptance=row["acceptance"], dependencies=json.loads(row["dependencies"]),
            status=TaskStatus(row["status"]), result_refs=json.loads(row["result_refs"]),
            created_at=row["created_at"], updated_at=row["updated_at"],
        )

    # -- turn runs -----------------------------------------------------------

    def insert_run(self, run: TurnRun) -> None:
        self.conn.execute(
            "INSERT INTO turn_runs(run_id, session_id, task_id, goal_id, agent_id, config_revision,"
            " topology_revision, status, input_delivery_ids, context_ref, external_turn_id,"
            " cancel_requested, waiting_on, created_at, updated_at)"
            " VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (run.run_id, run.session_id, run.task_id, run.goal_id, run.agent_id,
             run.config_revision, run.topology_revision, run.status,
             json.dumps(run.input_delivery_ids), run.context_ref, run.external_turn_id,
             int(run.cancel_requested), json.dumps(run.waiting_on),
             run.created_at, run.updated_at),
        )

    def set_run_status(self, run_id: str, status: TurnStatus,
                       external_turn_id: str | None = None) -> None:
        if external_turn_id is None:
            self.conn.execute(
                "UPDATE turn_runs SET status=?, updated_at=? WHERE run_id=?",
                (status, time.time(), run_id),
            )
        else:
            self.conn.execute(
                "UPDATE turn_runs SET status=?, external_turn_id=?, updated_at=? WHERE run_id=?",
                (status, external_turn_id, time.time(), run_id),
            )

    def update_run_status_where(self, run_id: str, expected: TurnStatus, new: TurnStatus) -> bool:
        cur = self.conn.execute(
            "UPDATE turn_runs SET status=?, updated_at=? WHERE run_id=? AND status=?",
            (new, time.time(), run_id, expected),
        )
        return cur.rowcount == 1

    def set_run_cancel_requested(self, run_id: str) -> None:
        self.conn.execute(
            "UPDATE turn_runs SET cancel_requested=1, updated_at=? WHERE run_id=?",
            (time.time(), run_id),
        )

    def set_run_external_turn(self, run_id: str, external_turn_id: str) -> None:
        """Record the backend turn id without touching the run's status."""
        self.conn.execute(
            "UPDATE turn_runs SET external_turn_id=?, updated_at=? WHERE run_id=?",
            (external_turn_id, time.time(), run_id),
        )

    def append_run_inputs(self, run_id: str, delivery_ids: list[int]) -> None:
        row = self.conn.execute(
            "SELECT input_delivery_ids FROM turn_runs WHERE run_id=?", (run_id,)
        ).fetchone()
        if row is None:
            return
        ids = json.loads(row["input_delivery_ids"])
        ids.extend(i for i in delivery_ids if i not in ids)
        self.conn.execute(
            "UPDATE turn_runs SET input_delivery_ids=?, updated_at=? WHERE run_id=?",
            (json.dumps(ids), time.time(), run_id),
        )

    def deliver_wait_registration(self, run_id: str, task_ids: list[str]) -> None:
        """Persist what a waiting run waits on (wake conditions)."""
        self.conn.execute(
            "UPDATE turn_runs SET waiting_on=?, updated_at=? WHERE run_id=?",
            (json.dumps(task_ids), time.time(), run_id),
        )

    def count_goal_runs(self, session_id: str, goal_id: str) -> int:
        row = self.conn.execute(
            "SELECT COUNT(*) AS n FROM turn_runs WHERE session_id=? AND goal_id=?",
            (session_id, goal_id),
        ).fetchone()
        return int(row["n"])

    def agent_context_epoch(self, session_id: str, agent_id: str) -> int:
        row = self.conn.execute(
            "SELECT context_epoch FROM agent_runtime WHERE session_id=? AND agent_id=?",
            (session_id, agent_id),
        ).fetchone()
        return int(row["context_epoch"]) if row else 1

    def set_codex_thread(self, session_id: str, agent_id: str, thread_id: str) -> None:
        with self.tx():
            self.conn.execute(
                "INSERT INTO agent_runtime(session_id, agent_id, external_thread_id, updated_at)"
                " VALUES(?,?,?,?) ON CONFLICT(session_id, agent_id) DO UPDATE SET"
                " external_thread_id=excluded.external_thread_id, updated_at=excluded.updated_at",
                (session_id, agent_id, thread_id, time.time()),
            )

    def get_codex_thread(self, session_id: str, agent_id: str) -> str | None:
        row = self.conn.execute(
            "SELECT external_thread_id FROM agent_runtime WHERE session_id=? AND agent_id=?",
            (session_id, agent_id),
        ).fetchone()
        return row["external_thread_id"] if row else None

    def bump_context_epoch(self, session_id: str, agent_id: str) -> int:
        with self.tx():
            self.conn.execute(
                "UPDATE agent_runtime SET context_epoch=context_epoch+1, updated_at=?"
                " WHERE session_id=? AND agent_id=?",
                (time.time(), session_id, agent_id),
            )
            return self.agent_context_epoch(session_id, agent_id)

    def get_run(self, run_id: str) -> TurnRun | None:
        row = self.conn.execute("SELECT * FROM turn_runs WHERE run_id=?", (run_id,)).fetchone()
        return self._row_to_run(row) if row else None

    def runs_for_session(self, session_id: str, statuses: Iterable[str] | None = None) -> list[TurnRun]:
        if statuses:
            marks = ",".join("?" * len(statuses))
            rows = self.conn.execute(
                f"SELECT * FROM turn_runs WHERE session_id=? AND status IN ({marks})",
                (session_id, *statuses),
            ).fetchall()
        else:
            rows = self.conn.execute(
                "SELECT * FROM turn_runs WHERE session_id=?", (session_id,)
            ).fetchall()
        return [self._row_to_run(r) for r in rows]

    def active_run_for_agent(self, session_id: str, agent_id: str) -> TurnRun | None:
        row = self.conn.execute(
            "SELECT * FROM turn_runs WHERE session_id=? AND agent_id=? AND status='RUNNING'",
            (session_id, agent_id),
        ).fetchone()
        return self._row_to_run(row) if row else None

    @staticmethod
    def _row_to_run(row: sqlite3.Row) -> TurnRun:
        return TurnRun(
            run_id=row["run_id"], session_id=row["session_id"], task_id=row["task_id"],
            goal_id=row["goal_id"],
            agent_id=row["agent_id"], config_revision=row["config_revision"],
            topology_revision=row["topology_revision"], status=TurnStatus(row["status"]),
            input_delivery_ids=json.loads(row["input_delivery_ids"]),
            context_ref=row["context_ref"], external_turn_id=row["external_turn_id"],
            cancel_requested=bool(row["cancel_requested"]),
            waiting_on=json.loads(row["waiting_on"]),
            created_at=row["created_at"], updated_at=row["updated_at"],
        )

    def run_cancel_requested(self, run_id: str) -> bool:
        row = self.conn.execute(
            "SELECT cancel_requested FROM turn_runs WHERE run_id=?", (run_id,)
        ).fetchone()
        return bool(row["cancel_requested"]) if row else False

    def task_cancel_requested(self, task_id: str) -> bool:
        row = self.conn.execute(
            "SELECT cancel_requested FROM tasks WHERE task_id=?", (task_id,)
        ).fetchone()
        return bool(row["cancel_requested"]) if row else False

    # -- completion requests -------------------------------------------------

    def record_completion_request(self, run_id: str, task_id: str,
                                  result_refs: list[str], summary: str) -> None:
        self.conn.execute(
            "INSERT INTO completion_requests(run_id, task_id, result_refs, summary, created_at)"
            " VALUES(?,?,?,?,?) ON CONFLICT(run_id) DO UPDATE SET"
            " task_id=excluded.task_id, result_refs=excluded.result_refs, summary=excluded.summary",
            (run_id, task_id, json.dumps(result_refs), summary, time.time()),
        )

    def completion_request(self, run_id: str) -> sqlite3.Row | None:
        return self.conn.execute(
            "SELECT * FROM completion_requests WHERE run_id=?", (run_id,)
        ).fetchone()

    # -- shared space --------------------------------------------------------

    def add_shared_entry(self, entry: SharedEntry, session_id: str) -> int:
        cur = self.conn.execute(
            "INSERT INTO shared_entries(entry_id, session_id, space_id, author, kind, content,"
            " ref, supersedes, created_at) VALUES(?,?,?,?,?,?,?,?,?)",
            (entry.entry_id, session_id, entry.space_id, entry.author, entry.kind, entry.content,
             entry.ref, entry.supersedes, entry.created_at),
        )
        return int(cur.lastrowid)

    def shared_entries(self, session_id: str, space_ids: Iterable[str],
                       after_sequence: int = 0, limit: int = 100) -> list[SharedEntry]:
        ids = list(space_ids)
        if not ids:
            return []
        marks = ",".join("?" * len(ids))
        rows = self.conn.execute(
            f"SELECT * FROM shared_entries WHERE session_id=? AND space_id IN ({marks})"
            " AND sequence>? ORDER BY sequence LIMIT ?",
            (session_id, *ids, after_sequence, limit),
        ).fetchall()
        return [self._row_to_shared(r) for r in rows]

    def shared_cursor(self, session_id: str, agent_id: str, space_id: str) -> int:
        row = self.conn.execute(
            "SELECT sequence FROM shared_cursors WHERE session_id=? AND agent_id=? AND space_id=?",
            (session_id, agent_id, space_id),
        ).fetchone()
        return int(row["sequence"]) if row else 0

    def advance_shared_cursor(self, session_id: str, agent_id: str, space_id: str,
                              sequence: int) -> None:
        self.conn.execute(
            "INSERT INTO shared_cursors(session_id, agent_id, space_id, sequence) VALUES(?,?,?,?)"
            " ON CONFLICT(session_id, agent_id, space_id) DO UPDATE SET"
            " sequence=MAX(sequence, excluded.sequence)",
            (session_id, agent_id, space_id, sequence),
        )

    @staticmethod
    def _row_to_shared(row: sqlite3.Row) -> SharedEntry:
        return SharedEntry(
            entry_id=row["entry_id"], space_id=row["space_id"], author=row["author"],
            kind=row["kind"], content=row["content"], ref=row["ref"],
            supersedes=row["supersedes"], sequence=row["sequence"], created_at=row["created_at"],
        )

    # -- approvals -----------------------------------------------------------

    def insert_approval(self, req: ApprovalRequest) -> None:
        self.conn.execute(
            "INSERT INTO approvals(approval_id, session_id, agent_id, run_id, tool_call_id,"
            " operation_hash, requested_scope, policy_revision, status, created_at)"
            " VALUES(?,?,?,?,?,?,?,?,?,?)",
            (req.approval_id, req.session_id, req.agent_id, req.run_id, req.tool_call_id,
             req.operation_hash, json.dumps(req.requested_scope), req.policy_revision,
             req.status, req.created_at),
        )

    def decide_approval(self, approval_id: str, status: ApprovalStatus) -> bool:
        cur = self.conn.execute(
            "UPDATE approvals SET status=?, decided_at=? WHERE approval_id=? AND status='PENDING'",
            (status, time.time(), approval_id),
        )
        return cur.rowcount == 1

    def get_approval(self, approval_id: str) -> ApprovalRequest | None:
        row = self.conn.execute(
            "SELECT * FROM approvals WHERE approval_id=?", (approval_id,)
        ).fetchone()
        if not row:
            return None
        return ApprovalRequest(
            approval_id=row["approval_id"], session_id=row["session_id"],
            agent_id=row["agent_id"], run_id=row["run_id"], tool_call_id=row["tool_call_id"],
            operation_hash=row["operation_hash"],
            requested_scope=json.loads(row["requested_scope"]),
            policy_revision=row["policy_revision"], status=ApprovalStatus(row["status"]),
            created_at=row["created_at"], decided_at=row["decided_at"],
        )

    def pending_approvals(self, session_id: str) -> list[ApprovalRequest]:
        rows = self.conn.execute(
            "SELECT approval_id FROM approvals WHERE session_id=? AND status='PENDING'",
            (session_id,),
        ).fetchall()
        return [self.get_approval(r["approval_id"]) for r in rows]

    def cache_session_approval(self, session_id: str, operation_hash: str,
                               scope: dict) -> None:
        self.conn.execute(
            "INSERT INTO session_approval_cache(session_id, operation_hash, scope_json, created_at)"
            " VALUES(?,?,?,?) ON CONFLICT(session_id, operation_hash) DO UPDATE SET"
            " scope_json=excluded.scope_json",
            (session_id, operation_hash, json.dumps(scope), time.time()),
        )

    def find_session_approval(self, session_id: str, operation_hash: str) -> dict | None:
        row = self.conn.execute(
            "SELECT scope_json FROM session_approval_cache"
            " WHERE session_id=? AND operation_hash=?",
            (session_id, operation_hash),
        ).fetchone()
        return json.loads(row["scope_json"]) if row else None

    def approval_for_call(self, run_id: str, tool_call_id: str,
                          operation_hash: str) -> ApprovalRequest | None:
        """The decision (if any) already recorded for this exact call."""
        row = self.conn.execute(
            "SELECT approval_id FROM approvals WHERE run_id=? AND tool_call_id=?"
            " AND operation_hash=? ORDER BY created_at DESC LIMIT 1",
            (run_id, tool_call_id, operation_hash),
        ).fetchone()
        return self.get_approval(row["approval_id"]) if row else None

    def expire_approval(self, approval_id: str) -> None:
        """A once-approval is single use (consumed after the operation runs); a
        pending approval is void once its turn can no longer use it."""
        self.conn.execute(
            "UPDATE approvals SET status=?, decided_at=? WHERE approval_id=? AND status IN (?,?)",
            (ApprovalStatus.EXPIRED, time.time(), approval_id,
             ApprovalStatus.APPROVED_ONCE, ApprovalStatus.PENDING),
        )

    def expire_run_approvals(self, run_id: str) -> list[ApprovalRequest]:
        """Void every still-PENDING approval of a run (terminal/cancelled turns).

        Returns the expired requests so the caller can emit an audit event.
        """
        with self.tx():
            rows = self.conn.execute(
                "SELECT approval_id FROM approvals WHERE run_id=? AND status='PENDING'",
                (run_id,),
            ).fetchall()
            expired = [req for r in rows if (req := self.get_approval(r["approval_id"]))]
            for req in expired:
                self.expire_approval(req.approval_id)
            return expired

    # -- topology patches ----------------------------------------------------

    def insert_patch(self, patch: TopologyPatch, session_id: str) -> None:
        self.conn.execute(
            "INSERT INTO topology_patches(patch_id, session_id, base_revision, proposer, decided_by,"
            " operations, affected_agents, status, created_at, updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)"
            " ON CONFLICT(patch_id) DO UPDATE SET decided_by=excluded.decided_by,"
            " operations=excluded.operations, affected_agents=excluded.affected_agents,"
            " status=excluded.status, updated_at=excluded.updated_at",
            (patch.patch_id, session_id, patch.base_revision, patch.proposer, patch.decided_by,
             json.dumps(patch.operations), json.dumps(patch.affected_agents), patch.status,
             patch.created_at, patch.updated_at),
        )

    def set_patch_status(self, patch_id: str, status: PatchStatus) -> None:
        self.conn.execute(
            "UPDATE topology_patches SET status=?, updated_at=? WHERE patch_id=?",
            (status, time.time(), patch_id),
        )

    def get_patch(self, patch_id: str) -> TopologyPatch | None:
        row = self.conn.execute(
            "SELECT * FROM topology_patches WHERE patch_id=?", (patch_id,)
        ).fetchone()
        if not row:
            return None
        return TopologyPatch(
            patch_id=row["patch_id"], base_revision=row["base_revision"],
            proposer=row["proposer"], decided_by=row["decided_by"],
            operations=json.loads(row["operations"]),
            affected_agents=json.loads(row["affected_agents"]),
            status=PatchStatus(row["status"]), created_at=row["created_at"],
            updated_at=row["updated_at"],
        )

    def patches_in_status(self, session_id: str, status: PatchStatus) -> list[TopologyPatch]:
        rows = self.conn.execute(
            "SELECT patch_id FROM topology_patches WHERE session_id=? AND status=?",
            (session_id, status),
        ).fetchall()
        return [self.get_patch(r["patch_id"]) for r in rows]


class _Tx:
    """Re-entrant transaction context manager (BEGIN IMMEDIATE at depth 0).

    Re-entrancy lets control-layer methods compose store operations into one
    business transaction without nesting BEGIN statements.
    """

    def __init__(self, store: "Store"):
        self.store = store
        self.conn = store.conn

    def __enter__(self):
        self.store._lock.acquire()
        if self.store._tx_depth == 0:
            self.conn.execute("BEGIN IMMEDIATE")
        self.store._tx_depth += 1
        return self.conn

    def __exit__(self, exc_type, exc, tb):
        self.store._tx_depth -= 1
        if self.store._tx_depth > 0:
            self.store._lock.release()
            return False
        if exc_type is None:
            self.conn.execute("COMMIT")
        else:
            self.conn.execute("ROLLBACK")
        self.store._lock.release()
        return False


class _LockedConn:
    """sqlite3 connection whose statements are serialized across threads."""

    def __init__(self, conn: sqlite3.Connection, lock: threading.RLock):
        object.__setattr__(self, "_conn", conn)
        object.__setattr__(self, "_lock", lock)

    def execute(self, *args):
        with self._lock:
            return self._conn.execute(*args)

    def executemany(self, *args):
        with self._lock:
            return self._conn.executemany(*args)

    def executescript(self, *args):
        with self._lock:
            return self._conn.executescript(*args)

    def close(self) -> None:
        with self._lock:
            self._conn.close()

    def __getattr__(self, name):
        return getattr(self._conn, name)

    def __setattr__(self, name, value) -> None:
        setattr(self._conn, name, value)
