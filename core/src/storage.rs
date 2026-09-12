//! Authoritative state: SQLite (WAL), ported from src/teamagents/storage.py.
//! Thin synchronous wrapper; the caller (Control) serializes writes.

use crate::models::*;
use rusqlite::{params, Connection, OptionalExtension, Row};

pub const DB_SCHEMA_VERSION: i64 = 1;

pub const SCHEMA: &str = r#"
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
"#;

fn j<T: serde::Serialize>(v: &T) -> String { serde_json::to_string(v).expect("json encode") }

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).expect("enum").as_str().expect("str enum").to_string()
}

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000_i64)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        let store = Self { conn };
        store.check_schema_version()?;
        Ok(store)
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        let store = Self { conn };
        store.check_schema_version()?;
        Ok(store)
    }

    fn check_schema_version(&self) -> rusqlite::Result<()> {
        let row: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='db_schema_version'", [], |r| r.get(0))
            .optional()?;
        match row {
            None => {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES('db_schema_version', ?1)",
                    params![DB_SCHEMA_VERSION.to_string()],
                )?;
            }
            Some(v) if v.parse::<i64>().ok() == Some(DB_SCHEMA_VERSION) => {}
            Some(v) => panic!(
                "database schema version {v} != supported {DB_SCHEMA_VERSION}; back up and migrate before opening"
            ),
        }
        Ok(())
    }

    // -- sessions / meta -----------------------------------------------------

    pub fn create_session(&self, session_id: &str, cwd: &str, permissions_mode: &str) -> rusqlite::Result<()> {
        let t = now();
        self.conn.execute(
            "INSERT INTO sessions(session_id, status, cwd, permissions_mode, created_at, updated_at)
             VALUES(?1, 'ACTIVE', ?2, ?3, ?4, ?4)",
            params![session_id, cwd, permissions_mode, t],
        )?;
        Ok(())
    }

    pub fn get_session(&self, session_id: &str) -> rusqlite::Result<Option<Json>> {
        self.conn
            .query_row(
                "SELECT session_id, status, cwd, permissions_mode, goal_id, goal_state FROM sessions WHERE session_id=?1",
                params![session_id],
                |r| {
                    Ok(serde_json::json!({
                        "session_id": r.get::<_, String>(0)?,
                        "status": r.get::<_, String>(1)?,
                        "cwd": r.get::<_, String>(2)?,
                        "permissions_mode": r.get::<_, String>(3)?,
                        "goal_id": r.get::<_, Option<String>>(4)?,
                        "goal_state": r.get::<_, String>(5)?,
                    }))
                },
            )
            .optional()
    }

    pub fn set_session_status(&self, session_id: &str, status: SessionStatus) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sessions SET status=?1, updated_at=?2 WHERE session_id=?3",
            params![enum_str(&status), now(), session_id],
        )?;
        Ok(())
    }

    pub fn set_goal_state(&self, session_id: &str, goal_id: &str, state: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sessions SET goal_id=?1, goal_state=?2, updated_at=?3 WHERE session_id=?4",
            params![goal_id, state, now(), session_id],
        )?;
        Ok(())
    }

    pub fn set_permission_mode(&self, session_id: &str, mode: PermissionMode) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sessions SET permissions_mode=?1, updated_at=?2 WHERE session_id=?3",
            params![enum_str(&mode), now(), session_id],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| r.get(0)).optional()
    }

    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // -- team specs ------------------------------------------------------------

    pub fn save_team_spec(&self, session_id: &str, spec: &TeamSpec) -> rusqlite::Result<i64> {
        let revision = self.current_revision(session_id)? + 1;
        self.conn.execute(
            "INSERT INTO team_specs(session_id, revision, spec_json, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![session_id, revision, j(spec), now()],
        )?;
        Ok(revision)
    }

    pub fn current_revision(&self, session_id: &str) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(revision), 0) FROM team_specs WHERE session_id=?1",
                params![session_id],
                |r| r.get(0),
            )
            .unwrap_or(0))
    }

    pub fn load_team_spec(&self, session_id: &str, revision: Option<i64>) -> rusqlite::Result<TeamSpec> {
        let blob: Option<String> = match revision {
            Some(r) => self
                .conn
                .query_row(
                    "SELECT spec_json FROM team_specs WHERE session_id=?1 AND revision=?2",
                    params![session_id, r],
                    |row| row.get(0),
                )
                .optional()?,
            None => self
                .conn
                .query_row(
                    "SELECT spec_json FROM team_specs WHERE session_id=?1 ORDER BY revision DESC LIMIT 1",
                    params![session_id],
                    |row| row.get(0),
                )
                .optional()?,
        };
        match blob {
            Some(b) => Ok(serde_json::from_str(&b).expect("stored spec is valid")),
            None => panic!("no team spec for session {session_id}"),
        }
    }

    // -- actions / receipts ----------------------------------------------------

    pub fn get_action_receipt(&self, action_id: &str) -> rusqlite::Result<Option<Receipt>> {
        self.conn
            .query_row(
                "SELECT receipt_json FROM actions WHERE action_id=?1",
                params![action_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map(|opt| opt.map(|blob| serde_json::from_str(&blob).expect("stored receipt is valid")))
    }

    pub fn record_action(
        &self,
        action_id: &str,
        session_id: &str,
        actor_id: &str,
        run_id: Option<&str>,
        kind: ActionKind,
        payload_hash: &str,
        receipt: &Receipt,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO actions(action_id, session_id, actor_id, run_id, kind, payload_hash, receipt_json, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![action_id, session_id, actor_id, run_id, enum_str(&kind), payload_hash, j(receipt), now()],
        )?;
        Ok(())
    }

    // -- events / deliveries ---------------------------------------------------

    pub fn append_event(&self, event: &TeamEvent) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO events(event_id, session_id, actor_id, task_id, kind, payload_json, audience_json, topology_revision, causation_id, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.event_id, event.session_id, event.actor_id, event.task_id,
                enum_str(&event.kind), j(&event.payload), j(&event.audience),
                event.topology_revision, event.causation_id, event.created_at
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn events(&self, session_id: &str, after_sequence: i64, limit: i64) -> rusqlite::Result<Vec<Json>> {
        let mut stmt = self.conn.prepare(
            "SELECT sequence, event_id, actor_id, task_id, kind, payload_json, audience_json, topology_revision, causation_id, created_at
             FROM events WHERE session_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![session_id, after_sequence, limit], event_row)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn create_delivery(
        &self,
        session_id: &str,
        agent_id: &str,
        event_id: &str,
        batch_no: i64,
        payload_override: Option<&str>,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO deliveries(session_id, agent_id, event_id, batch_no, payload_override, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            params![session_id, agent_id, event_id, batch_no, payload_override, now()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn next_batch_no(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        let n: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(batch_no), 0) + 1 FROM deliveries WHERE session_id=?1 AND agent_id=?2",
            params![session_id, agent_id],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    pub fn pending_deliveries(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<Vec<Json>> {
        let mut stmt = self.conn.prepare(
            "SELECT delivery_id, event_id, batch_no, payload_override, created_at
             FROM deliveries WHERE session_id=?1 AND agent_id=?2 AND status='pending' ORDER BY delivery_id",
        )?;
        let rows = stmt.query_map(params![session_id, agent_id], |r| {
            Ok(serde_json::json!({
                "delivery_id": r.get::<_, i64>(0)?,
                "event_id": r.get::<_, String>(1)?,
                "batch_no": r.get::<_, i64>(2)?,
                "payload_override": r.get::<_, Option<String>>(3)?,
                "created_at": r.get::<_, f64>(4)?,
            }))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn ack_deliveries_exact(&self, session_id: &str, agent_id: &str, batch_no: i64, delivery_ids: &[i64]) -> rusqlite::Result<i64> {
        // storage.py::ack_deliveries_exact — mark the exact ids of one batch applied.
        let mut n = 0;
        for id in delivery_ids {
            n += self.conn.execute(
                "UPDATE deliveries SET status='applied', applied_at=?1
                 WHERE delivery_id=?2 AND session_id=?3 AND agent_id=?4 AND batch_no=?5 AND status='pending'",
                params![now(), id, session_id, agent_id, batch_no],
            )?;
        }
        Ok(n as i64)
    }

    // -- tasks -----------------------------------------------------------------

    pub fn insert_task(&self, session_id: &str, task: &Task) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO tasks(task_id, session_id, parent_task_id, goal_id, requester, assignee, description, acceptance, dependencies, status, result_refs, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                task.task_id, session_id, task.parent_task_id, task.goal_id, task.requester,
                task.assignee, task.description, task.acceptance, j(&task.dependencies),
                enum_str(&task.status), j(&task.result_refs), task.created_at, task.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_task(&self, task_id: &str) -> rusqlite::Result<Option<Task>> {
        self.conn
            .query_row(&format!("SELECT {TASK_COLS} FROM tasks WHERE task_id=?1"), params![task_id], row_to_task)
            .optional()
    }

    pub fn tasks_for_session(&self, session_id: &str, statuses: &[&str]) -> rusqlite::Result<Vec<Task>> {
        let sql = if statuses.is_empty() {
            format!("SELECT {TASK_COLS} FROM tasks WHERE session_id=?1 ORDER BY created_at")
        } else {
            // ponytail: statuses come from our own enums, not user input
            let placeholders = statuses.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",");
            format!("SELECT {TASK_COLS} FROM tasks WHERE session_id=?1 AND status IN ({placeholders}) ORDER BY created_at")
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![session_id], row_to_task)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// storage.py::compare_and_set_task — optimistic status transition.
    pub fn compare_and_set_task(&self, task_id: &str, expected: &str, new: TaskStatus, result_refs: Option<&[String]>) -> rusqlite::Result<bool> {
        let refs = result_refs.map(|r| serde_json::to_string(r).unwrap());
        let n = match refs {
            Some(r) => self.conn.execute(
                "UPDATE tasks SET status=?1, result_refs=?2, updated_at=?3 WHERE task_id=?4 AND status=?5",
                params![enum_str(&new), r, now(), task_id, expected],
            )?,
            None => self.conn.execute(
                "UPDATE tasks SET status=?1, updated_at=?2 WHERE task_id=?3 AND status=?4",
                params![enum_str(&new), now(), task_id, expected],
            )?,
        };
        Ok(n == 1)
    }

    // -- turn runs ---------------------------------------------------------------

    pub fn insert_run(&self, run: &TurnRun) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO turn_runs(run_id, session_id, task_id, goal_id, agent_id, config_revision, topology_revision, status, input_delivery_ids, context_ref, external_turn_id, cancel_requested, waiting_on, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                run.run_id, run.session_id, run.task_id, run.goal_id, run.agent_id,
                run.config_revision, run.topology_revision, enum_str(&run.status),
                j(&run.input_delivery_ids), run.context_ref, run.external_turn_id,
                run.cancel_requested as i64, j(&run.waiting_on), run.created_at, run.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn get_run(&self, run_id: &str) -> rusqlite::Result<Option<TurnRun>> {
        self.conn
            .query_row(&format!("SELECT {RUN_COLS} FROM turn_runs WHERE run_id=?1"), params![run_id], row_to_run)
            .optional()
    }

    pub fn set_run_status(&self, run_id: &str, status: TurnStatus) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE turn_runs SET status=?1, updated_at=?2 WHERE run_id=?3",
            params![enum_str(&status), now(), run_id],
        )?;
        Ok(())
    }

    pub fn update_run_status_where(&self, run_id: &str, expected: TurnStatus, new: TurnStatus) -> rusqlite::Result<bool> {
        let n = self.conn.execute(
            "UPDATE turn_runs SET status=?1, updated_at=?2 WHERE run_id=?3 AND status=?4",
            params![enum_str(&new), now(), run_id, enum_str(&expected)],
        )?;
        Ok(n == 1)
    }

    pub fn active_run_for_agent(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<Option<TurnRun>> {
        self.conn
            .query_row(
                &format!("SELECT {RUN_COLS} FROM turn_runs WHERE session_id=?1 AND agent_id=?2 AND status IN ('QUEUED','RUNNING') ORDER BY created_at LIMIT 1"),
                params![session_id, agent_id],
                row_to_run,
            )
            .optional()
    }

    // -- agent runtime -------------------------------------------------------------

    pub fn ensure_agent(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO agent_runtime(session_id, agent_id, updated_at) VALUES(?1, ?2, ?3)",
            params![session_id, agent_id, now()],
        )?;
        Ok(())
    }

    pub fn set_agent_status(&self, session_id: &str, agent_id: &str, status: AgentStatus) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE agent_runtime SET status=?1, updated_at=?2 WHERE session_id=?3 AND agent_id=?4",
            params![enum_str(&status), now(), session_id, agent_id],
        )?;
        Ok(())
    }

    pub fn agent_status(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<Option<AgentStatus>> {
        let s: Option<String> = self
            .conn
            .query_row(
                "SELECT status FROM agent_runtime WHERE session_id=?1 AND agent_id=?2",
                params![session_id, agent_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(s.map(|v| serde_json::from_value(Json::String(v)).expect("stored agent status")))
    }

    // -- approvals / completion requests -------------------------------------------

    pub fn insert_approval(&self, a: &ApprovalRequest) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO approvals(approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                a.approval_id, a.session_id, a.agent_id, a.run_id, a.tool_call_id,
                a.operation_hash, j(&a.requested_scope), a.policy_revision,
                enum_str(&a.status), a.created_at, a.decided_at
            ],
        )?;
        Ok(())
    }

    pub fn decide_approval(&self, approval_id: &str, status: ApprovalStatus) -> rusqlite::Result<bool> {
        let n = self.conn.execute(
            "UPDATE approvals SET status=?1, decided_at=?2 WHERE approval_id=?3 AND status='PENDING'",
            params![enum_str(&status), now(), approval_id],
        )?;
        Ok(n == 1)
    }

    pub fn record_completion_request(&self, run_id: &str, task_id: &str, result_refs: &[String], summary: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO completion_requests(run_id, task_id, result_refs, summary, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![run_id, task_id, j(&result_refs), summary, now()],
        )?;
        Ok(())
    }
}

const TASK_COLS: &str = "task_id, parent_task_id, goal_id, requester, assignee, description, acceptance, dependencies, status, result_refs, created_at, updated_at";
const RUN_COLS: &str = "run_id, session_id, task_id, goal_id, agent_id, config_revision, topology_revision, status, input_delivery_ids, context_ref, external_turn_id, cancel_requested, waiting_on, created_at, updated_at";

fn row_to_task(row: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        task_id: row.get(0)?,
        parent_task_id: row.get(1)?,
        goal_id: row.get(2)?,
        requester: row.get(3)?,
        assignee: row.get(4)?,
        description: row.get(5)?,
        acceptance: row.get(6)?,
        dependencies: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
        status: serde_json::from_value(Json::String(row.get::<_, String>(9)?)).expect("task status"),
        result_refs: serde_json::from_str(&row.get::<_, String>(10)?).unwrap_or_default(),
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn row_to_run(row: &Row) -> rusqlite::Result<TurnRun> {
    Ok(TurnRun {
        run_id: row.get(0)?,
        session_id: row.get(1)?,
        task_id: row.get(2)?,
        goal_id: row.get(3)?,
        agent_id: row.get(4)?,
        config_revision: row.get(5)?,
        topology_revision: row.get(6)?,
        status: serde_json::from_value(Json::String(row.get::<_, String>(7)?)).expect("turn status"),
        input_delivery_ids: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
        context_ref: row.get(9)?,
        external_turn_id: row.get(10)?,
        cancel_requested: row.get::<_, i64>(11)? != 0,
        waiting_on: serde_json::from_str(&row.get::<_, String>(12)?).unwrap_or_default(),
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn event_row(row: &Row) -> rusqlite::Result<Json> {
    Ok(serde_json::json!({
        "sequence": row.get::<_, i64>(0)?,
        "event_id": row.get::<_, String>(1)?,
        "actor_id": row.get::<_, String>(2)?,
        "task_id": row.get::<_, Option<String>>(3)?,
        "kind": row.get::<_, String>(4)?,
        "payload": serde_json::from_str::<Json>(&row.get::<_, String>(5)?).unwrap_or(Json::Null),
        "audience": serde_json::from_str::<Json>(&row.get::<_, String>(6)?).unwrap_or(Json::Null),
        "topology_revision": row.get::<_, i64>(7)?,
        "causation_id": row.get::<_, Option<String>>(8)?,
        "created_at": row.get::<_, f64>(9)?,
    }))
}
