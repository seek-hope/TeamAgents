//! Authoritative state: SQLite (WAL).
//! Thin synchronous wrapper; the caller (Control) serializes writes.

use crate::models::*;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::json;

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

/// A stored enum string is data, not an invariant: a value this build does not
/// know is a readable error for the caller instead of a process abort.
fn stored_enum<T: serde::de::DeserializeOwned>(v: String, what: &str) -> rusqlite::Result<T> {
    serde_json::from_value(Json::String(v.clone())).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            usize::MAX,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(format!("bad {what} {v:?} stored in the database: {e}")),
        )
    })
}

/// `Limits` fields that still exist; anything else in a stored spec is a key
/// removed from the model since (D-10) and is dropped on load.
const LIMITS_KEYS: &[&str] = &[
    "max_parallel_workers",
    "max_members",
    "max_turns_per_goal",
    "max_model_steps_per_turn",
    "turn_active_timeout_s",
    "cancel_confirm_timeout_s",
];

fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).expect("enum").as_str().expect("str enum").to_string()
}

pub struct Store {
    pub conn: Connection,
    /// Re-entrant transaction depth (BEGIN IMMEDIATE at depth 0).
    tx_depth: std::cell::Cell<usize>,
}

impl Store {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000_i64)?;
        // no `synchronous` pragma: keep SQLite's FULL default
        conn.execute_batch(SCHEMA)?;
        let store = Self { conn, tx_depth: std::cell::Cell::new(0) };
        store.check_schema_version()?;
        Ok(store)
    }

    pub fn open_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        let store = Self { conn, tx_depth: std::cell::Cell::new(0) };
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
                Ok(())
            }
            Some(v) if v.parse::<i64>().ok() == Some(DB_SCHEMA_VERSION) => Ok(()),
            // a version mismatch is data, not an invariant (same principle as
            // stored_enum): core is linked in-process, a panic kills the host
            Some(v) => Err(rusqlite::Error::FromSqlConversionFailure(
                usize::MAX,
                rusqlite::types::Type::Text,
                Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "database schema version {v} != supported {DB_SCHEMA_VERSION}; back up and migrate before opening"
                )),
            )),
        }
    }

    // -- transactions ----------------------------------------------------------

    pub fn begin(&self) -> rusqlite::Result<()> {
        if self.tx_depth.get() == 0 {
            self.conn.execute_batch("BEGIN IMMEDIATE")?;
        }
        self.tx_depth.set(self.tx_depth.get() + 1);
        Ok(())
    }

    /// Ok(true)=commit, Err/Ok(false) semantics handled by caller via rollback.
    pub fn commit(&self) -> rusqlite::Result<()> {
        let d = self.tx_depth.get().saturating_sub(1);
        self.tx_depth.set(d);
        if d == 0 {
            self.conn.execute_batch("COMMIT")?;
        }
        Ok(())
    }

    pub fn rollback(&self) -> rusqlite::Result<()> {
        self.tx_depth.set(0);
        self.conn.execute_batch("ROLLBACK")?;
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

    /// Stored specs are read for their own errors
    /// (no panic), and limits keys removed since the row was written are dropped
    /// so old sessions keep loading (TeamSpec *files* stay strict).
    pub fn load_team_spec(&self, session_id: &str, revision: Option<i64>) -> Result<TeamSpec, String> {
        let revision = match revision {
            Some(r) => r,
            None => self.current_revision(session_id).map_err(|e| e.to_string())?,
        };
        let blob: Option<String> = self
            .conn
            .query_row(
                "SELECT spec_json FROM team_specs WHERE session_id=?1 AND revision=?2",
                params![session_id, revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(b) = blob else {
            // a missing revision is a readable refusal, not a crash
            return Err(format!("no team spec revision {revision} for session {session_id:?}"));
        };
        let mut data: Json = serde_json::from_str(&b).map_err(|e| format!("stored spec is invalid: {e}"))?;
        if let Some(Json::Object(limits)) = data.get_mut("limits") {
            limits.retain(|k, _| LIMITS_KEYS.contains(&k.as_str()));
        }
        serde_json::from_value(data).map_err(|e| format!("stored spec is invalid: {e}"))
    }

    // -- actions / receipts ----------------------------------------------------

    pub fn get_action_receipt(&self, action_id: &str) -> rusqlite::Result<Option<Receipt>> {
        let blob: Option<String> = self
            .conn
            .query_row(
                "SELECT receipt_json FROM actions WHERE action_id=?1",
                params![action_id],
                |r| r.get(0),
            )
            .optional()?;
        match blob {
            None => Ok(None),
            Some(b) => serde_json::from_str(&b).map(Some).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    usize::MAX,
                    rusqlite::types::Type::Text,
                    Box::<dyn std::error::Error + Send + Sync>::from(format!("stored receipt is invalid: {e}")),
                )
            }),
        }
    }

    pub fn get_action_metadata(&self, action_id: &str) -> rusqlite::Result<Option<(String, String, String, String)>> {
        self.conn
            .query_row(
                "SELECT session_id, actor_id, kind, payload_hash FROM actions WHERE action_id=?1",
                params![action_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()
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
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
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

    /// Hand out the member's next batch number and
    /// advance the ledger in the same transaction (caller holds the tx).
    pub fn next_batch_no(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        let batch: i64 = self
            .conn
            .query_row(
                "SELECT next_batch_no FROM agent_runtime WHERE session_id=?1 AND agent_id=?2",
                params![session_id, agent_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1);
        self.conn.execute(
            "INSERT INTO agent_runtime(session_id, agent_id, next_batch_no, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, agent_id) DO UPDATE SET next_batch_no=excluded.next_batch_no,
               updated_at=excluded.updated_at",
            params![session_id, agent_id, batch + 1, now()],
        )?;
        Ok(batch)
    }

    /// The member's consume cursor.
    pub fn applied_batch(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT last_applied_batch FROM agent_runtime WHERE session_id=?1 AND agent_id=?2",
                params![session_id, agent_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    /// The cursor half of the ack: advance the member's
    /// applied-batch ledger to at least `batch_no`.
    fn advance_applied_batch(&self, session_id: &str, agent_id: &str, batch_no: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO agent_runtime(session_id, agent_id, last_applied_batch, updated_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, agent_id) DO UPDATE SET
               last_applied_batch=MAX(last_applied_batch, excluded.last_applied_batch),
               updated_at=excluded.updated_at",
            params![session_id, agent_id, batch_no, now()],
        )?;
        Ok(())
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
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Mark exactly these ids applied (never a
    /// batch range) and advance the member's applied-batch cursor.
    /// `_batch_no` is kept for existing callers; matching is by id.
    pub fn ack_deliveries_exact(&self, session_id: &str, agent_id: &str, _batch_no: i64, delivery_ids: &[i64]) -> rusqlite::Result<i64> {
        if delivery_ids.is_empty() {
            return Ok(0);
        }
        let marks = delivery_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now())];
        for id in delivery_ids {
            p.push(Box::new(*id));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        let n = self.conn.execute(
            &format!("UPDATE deliveries SET status='applied', applied_at=?1 WHERE delivery_id IN ({marks}) AND status='pending'"),
            refs.as_slice(),
        )?;
        let max: Option<i64> = self.conn.query_row(
            &format!("SELECT MAX(batch_no) FROM deliveries WHERE delivery_id IN ({marks})"),
            &refs[1..],
            |r| r.get(0),
        )?;
        if let Some(b) = max {
            self.advance_applied_batch(session_id, agent_id, b)?;
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

    // ponytail: lookups by primary key assume one DB serves one session
    // (engine opens a per-session team.db today). If a single DB ever hosts
    // multiple sessions, get_task/get_run/decide_approval/reassign_tasks need
    // a session_id filter.
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
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Optimistic status transition.
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
                &format!("SELECT {RUN_COLS} FROM turn_runs WHERE session_id=?1 AND agent_id=?2 AND status='RUNNING'"),
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
        match s {
            None => Ok(None),
            Some(v) => Ok(Some(stored_enum(v, "agent status")?)),
        }
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
        status: stored_enum(row.get::<_, String>(8)?, "task status")?,
        result_refs: serde_json::from_str(&row.get::<_, String>(9)?).unwrap_or_default(),
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
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
        status: stored_enum(row.get::<_, String>(7)?, "turn status")?,
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

// ---------------------------------------------------------------------------
// Extended ops (control-layer needs)
// ---------------------------------------------------------------------------

impl Store {
    pub fn del_meta(&self, key: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM meta WHERE key=?1", params![key])?;
        Ok(())
    }

    pub fn set_task_cancel_requested(&self, task_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE tasks SET cancel_requested=1, updated_at=?1 WHERE task_id=?2",
            params![now(), task_id],
        )?;
        Ok(())
    }

    pub fn reassign_tasks(&self, assignee: &str, new_assignee: &str, statuses: &[TaskStatus]) -> rusqlite::Result<Vec<String>> {
        let marks = statuses.iter().map(|s| format!("'{}'", enum_str(s))).collect::<Vec<_>>().join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT task_id FROM tasks WHERE assignee=?1 AND status IN ({marks})"
        ))?;
        let ids: Vec<String> = stmt
            .query_map(params![assignee], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in &ids {
            self.conn.execute(
                "UPDATE tasks SET assignee=?1, updated_at=?2 WHERE task_id=?3",
                params![new_assignee, now(), id],
            )?;
        }
        Ok(ids)
    }

    /// Undelivered messages keep the reason.
    pub fn drop_pending_deliveries(&self, session_id: &str, agent_id: &str, reason: &str) -> rusqlite::Result<usize> {
        let n = self.conn.execute(
            "UPDATE deliveries SET status='dropped', payload_override=?1
             WHERE session_id=?2 AND agent_id=?3 AND status='pending'",
            params![json!({"dropped_reason": reason}).to_string(), session_id, agent_id],
        )?;
        Ok(n)
    }

    pub fn append_run_inputs(&self, run_id: &str, delivery_ids: &[i64]) -> rusqlite::Result<()> {
        let mut run = self.get_run(run_id)?.expect("run exists");
        let mut seen: std::collections::HashSet<i64> = run.input_delivery_ids.iter().copied().collect();
        for id in delivery_ids {
            if seen.insert(*id) {
                run.input_delivery_ids.push(*id);
            }
        }
        self.conn.execute(
            "UPDATE turn_runs SET input_delivery_ids=?1, updated_at=?2 WHERE run_id=?3",
            params![j(&run.input_delivery_ids), now(), run_id],
        )?;
        Ok(())
    }

    pub fn deliver_wait_registration(&self, run_id: &str, task_ids: &[String]) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE turn_runs SET waiting_on=?1, updated_at=?2 WHERE run_id=?3",
            params![j(&task_ids), now(), run_id],
        )?;
        Ok(())
    }

    pub fn runs_for_session(&self, session_id: &str, statuses: &[TurnStatus]) -> rusqlite::Result<Vec<TurnRun>> {
        let sql = if statuses.is_empty() {
            format!("SELECT {RUN_COLS} FROM turn_runs WHERE session_id=?1 ORDER BY created_at")
        } else {
            let marks = statuses.iter().map(|s| format!("'{}'", enum_str(s))).collect::<Vec<_>>().join(",");
            format!("SELECT {RUN_COLS} FROM turn_runs WHERE session_id=?1 AND status IN ({marks}) ORDER BY created_at")
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![session_id], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Runs for the `state` wire view: every non-terminal run (scheduler and
    /// cancel paths need all of them) plus the newest `terminal_limit` finished
    /// runs (display + TUI delta flushing). The 250ms status poll used to
    /// serialize the whole run history, growing without bound over a session.
    /// ponytail: fixed terminal cap; page or raise it if a UI ever needs deep history.
    pub fn runs_for_state(&self, session_id: &str, terminal_limit: i64) -> rusqlite::Result<Vec<TurnRun>> {
        let mut runs = self.runs_for_session(
            session_id,
            &[TurnStatus::Queued, TurnStatus::Running, TurnStatus::WaitingTask, TurnStatus::WaitingApproval],
        )?;
        let terminal = [TurnStatus::Completed, TurnStatus::Failed, TurnStatus::Cancelled, TurnStatus::OutcomeUnknown]
            .iter()
            .map(|s| format!("'{}'", enum_str(s)))
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM turn_runs WHERE session_id=?1 AND status IN ({terminal}) ORDER BY created_at DESC LIMIT ?2"
        ))?;
        let mut tail: Vec<TurnRun> = stmt
            .query_map(params![session_id, terminal_limit], row_to_run)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        tail.reverse();
        runs.extend(tail);
        Ok(runs)
    }

    pub fn count_goal_runs(&self, session_id: &str, goal_id: &str) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM turn_runs WHERE session_id=?1 AND goal_id=?2",
            params![session_id, goal_id],
            |r| r.get(0),
        )
    }

    pub fn agent_config_revision(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT config_revision FROM agent_runtime WHERE session_id=?1 AND agent_id=?2",
                params![session_id, agent_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1))
    }

    pub fn bump_config_revision(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        self.ensure_agent(session_id, agent_id)?;
        self.conn.execute(
            "UPDATE agent_runtime SET config_revision=config_revision+1, updated_at=?1
             WHERE session_id=?2 AND agent_id=?3",
            params![now(), session_id, agent_id],
        )?;
        self.agent_config_revision(session_id, agent_id)
    }

    pub fn agent_context_epoch(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT context_epoch FROM agent_runtime WHERE session_id=?1 AND agent_id=?2",
                params![session_id, agent_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1))
    }

    pub fn set_run_cancel_requested(&self, run_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE turn_runs SET cancel_requested=1, updated_at=?1 WHERE run_id=?2",
            params![now(), run_id],
        )?;
        Ok(())
    }

    pub fn ack_run_deliveries(&self, run: &TurnRun) -> rusqlite::Result<()> {
        if run.input_delivery_ids.is_empty() {
            return Ok(());
        }
        let marks = run.input_delivery_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now())];
        for id in &run.input_delivery_ids {
            p.push(Box::new(*id));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        self.conn.execute(
            &format!("UPDATE deliveries SET status='applied', applied_at=?1 WHERE delivery_id IN ({marks}) AND status='pending'"),
            refs.as_slice(),
        )?;
        Ok(())
    }

    // -- approvals (extended) --------------------------------------------------

    fn row_to_approval(row: &Row) -> rusqlite::Result<ApprovalRequest> {
        Ok(ApprovalRequest {
            approval_id: row.get(0)?,
            session_id: row.get(1)?,
            agent_id: row.get(2)?,
            run_id: row.get(3)?,
            tool_call_id: row.get(4)?,
            operation_hash: row.get(5)?,
            requested_scope: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or(Json::Null),
            policy_revision: row.get(7)?,
            status: stored_enum(row.get::<_, String>(8)?, "approval status")?,
            created_at: row.get(9)?,
            decided_at: row.get(10)?,
        })
    }

    pub fn get_approval(&self, approval_id: &str) -> rusqlite::Result<Option<ApprovalRequest>> {
        self.conn
            .query_row(
                "SELECT approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at
                 FROM approvals WHERE approval_id=?1",
                params![approval_id],
                Self::row_to_approval,
            )
            .optional()
    }

    pub fn get_approval_for_session(&self, session_id: &str, approval_id: &str) -> rusqlite::Result<Option<ApprovalRequest>> {
        self.conn
            .query_row(
                "SELECT approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at
                 FROM approvals WHERE session_id=?1 AND approval_id=?2",
                params![session_id, approval_id],
                Self::row_to_approval,
            )
            .optional()
    }

    pub fn pending_approvals(&self, session_id: &str) -> rusqlite::Result<Vec<ApprovalRequest>> {
        let mut stmt = self.conn.prepare(
            "SELECT approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at
             FROM approvals WHERE session_id=?1 AND status='PENDING' ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![session_id], Self::row_to_approval)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn expire_run_approvals(&self, run_id: &str) -> rusqlite::Result<Vec<ApprovalRequest>> {
        let pending = self.pending_approvals_for_run(run_id)?;
        for a in &pending {
            self.expire_approval(&a.approval_id)?;
        }
        Ok(pending)
    }

    /// A once-approval is single use; a pending
    /// approval is void once its turn can no longer use it. Other states stay
    /// untouched and report false.
    pub fn expire_approval(&self, approval_id: &str) -> rusqlite::Result<bool> {
        let n = self.conn.execute(
            "UPDATE approvals SET status='EXPIRED', decided_at=?1
             WHERE approval_id=?2 AND status IN ('PENDING', 'APPROVED_ONCE')",
            params![now(), approval_id],
        )?;
        Ok(n == 1)
    }

    pub fn expire_approval_for_session(&self, session_id: &str, approval_id: &str) -> rusqlite::Result<bool> {
        let n = self.conn.execute(
            "UPDATE approvals SET status='EXPIRED', decided_at=?1
             WHERE session_id=?2 AND approval_id=?3 AND status IN ('PENDING', 'APPROVED_ONCE')",
            params![now(), session_id, approval_id],
        )?;
        Ok(n == 1)
    }

    /// Newest still-usable approval for this run + operation: PENDING or
    /// APPROVED_ONCE (the engine replays a re-sent call under a once-approval).
    pub fn find_run_approval(&self, run_id: &str, operation_hash: &str) -> rusqlite::Result<Option<ApprovalRequest>> {
        self.conn
            .query_row(
                "SELECT approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at
                 FROM approvals WHERE run_id=?1 AND operation_hash=?2 AND status IN ('PENDING', 'APPROVED_ONCE')
                 ORDER BY created_at DESC LIMIT 1",
                params![run_id, operation_hash],
                Self::row_to_approval,
            )
            .optional()
    }

    pub fn pending_approvals_for_run(&self, run_id: &str) -> rusqlite::Result<Vec<ApprovalRequest>> {
        let mut stmt = self.conn.prepare(
            "SELECT approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at
             FROM approvals WHERE run_id=?1 AND status='PENDING' ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![run_id], Self::row_to_approval)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn cache_session_approval(&self, session_id: &str, operation_hash: &str, scope: &Json) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO session_approval_cache(session_id, operation_hash, scope_json, created_at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, operation_hash) DO UPDATE SET scope_json=excluded.scope_json",
            params![session_id, operation_hash, j(scope), now()],
        )?;
        Ok(())
    }

    pub fn find_session_approval(&self, session_id: &str, operation_hash: &str) -> rusqlite::Result<Option<Json>> {
        self.conn
            .query_row(
                "SELECT scope_json FROM session_approval_cache WHERE session_id=?1 AND operation_hash=?2",
                params![session_id, operation_hash],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map(|o| o.and_then(|s| serde_json::from_str(&s).ok()))
    }

    /// The decision (if any) already recorded for this exact call.
    pub fn approval_for_call(&self, run_id: &str, tool_call_id: &str, operation_hash: &str) -> rusqlite::Result<Option<ApprovalRequest>> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT approval_id FROM approvals WHERE run_id=?1 AND tool_call_id=?2 AND operation_hash=?3
                 ORDER BY created_at DESC LIMIT 1",
                params![run_id, tool_call_id, operation_hash],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(id) => self.get_approval(&id),
            None => Ok(None),
        }
    }

    // -- shared spaces -----------------------------------------------------------

    pub fn add_shared_entry(&self, entry: &SharedEntry, session_id: &str) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO shared_entries(entry_id, session_id, space_id, author, kind, content, ref, supersedes, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![entry.entry_id, session_id, entry.space_id, entry.author, entry.kind,
                    entry.content, entry.r#ref, entry.supersedes, entry.created_at],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn row_to_shared(row: &Row) -> rusqlite::Result<SharedEntry> {
        Ok(SharedEntry {
            sequence: row.get(0)?,
            entry_id: row.get(1)?,
            space_id: row.get(3)?,
            author: row.get(4)?,
            kind: row.get(5)?,
            content: row.get(6)?,
            r#ref: row.get(7)?,
            supersedes: row.get(8)?,
            created_at: row.get(9)?,
        })
    }

    pub fn shared_entries(&self, session_id: &str, space_ids: &[String], after_sequence: i64, limit: i64) -> rusqlite::Result<Vec<SharedEntry>> {
        if space_ids.is_empty() {
            return Ok(vec![]);
        }
        let marks = space_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT sequence, entry_id, session_id, space_id, author, kind, content, ref, supersedes, created_at
             FROM shared_entries WHERE session_id=? AND space_id IN ({marks}) AND sequence>? ORDER BY sequence LIMIT ?"
        );
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(session_id.to_string())];
        for s in space_ids {
            p.push(Box::new(s.clone()));
        }
        p.push(Box::new(after_sequence));
        p.push(Box::new(limit));
        let refs: Vec<&dyn rusqlite::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), Self::row_to_shared)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn shared_cursor(&self, session_id: &str, agent_id: &str, space_id: &str) -> rusqlite::Result<i64> {
        Ok(self
            .conn
            .query_row(
                "SELECT sequence FROM shared_cursors WHERE session_id=?1 AND agent_id=?2 AND space_id=?3",
                params![session_id, agent_id, space_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    pub fn advance_shared_cursor(&self, session_id: &str, agent_id: &str, space_id: &str, sequence: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO shared_cursors(session_id, agent_id, space_id, sequence) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(session_id, agent_id, space_id) DO UPDATE SET sequence=MAX(sequence, excluded.sequence)",
            params![session_id, agent_id, space_id, sequence],
        )?;
        Ok(())
    }

    // -- topology patches ----------------------------------------------------------

    pub fn insert_patch(&self, patch: &TopologyPatch, session_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO topology_patches(patch_id, session_id, base_revision, proposer, decided_by, operations, affected_agents, status, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(patch_id) DO UPDATE SET decided_by=excluded.decided_by,
               operations=excluded.operations, affected_agents=excluded.affected_agents,
               status=excluded.status, updated_at=excluded.updated_at",
            params![patch.patch_id, session_id, patch.base_revision, patch.proposer,
                    patch.decided_by, j(&patch.operations), j(&patch.affected_agents),
                    enum_str(&patch.status), patch.created_at, patch.updated_at],
        )?;
        Ok(())
    }

    pub fn set_patch_status(&self, patch_id: &str, status: PatchStatus) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE topology_patches SET status=?1, updated_at=?2 WHERE patch_id=?3",
            params![enum_str(&status), now(), patch_id],
        )?;
        Ok(())
    }

    fn row_to_patch(row: &Row) -> rusqlite::Result<TopologyPatch> {
        Ok(TopologyPatch {
            patch_id: row.get(0)?,
            base_revision: row.get(2)?,
            proposer: row.get(3)?,
            decided_by: row.get(4)?,
            operations: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
            affected_agents: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
            status: stored_enum(row.get::<_, String>(7)?, "patch status")?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        })
    }

    pub fn get_patch(&self, patch_id: &str) -> rusqlite::Result<Option<TopologyPatch>> {
        self.conn
            .query_row(
                "SELECT patch_id, session_id, base_revision, proposer, decided_by, operations, affected_agents, status, created_at, updated_at
                 FROM topology_patches WHERE patch_id=?1",
                params![patch_id],
                Self::row_to_patch,
            )
            .optional()
    }

    pub fn patches_in_status(&self, session_id: &str, status: PatchStatus) -> rusqlite::Result<Vec<TopologyPatch>> {
        let mut stmt = self.conn.prepare(
            "SELECT patch_id, session_id, base_revision, proposer, decided_by, operations, affected_agents, status, created_at, updated_at
             FROM topology_patches WHERE session_id=?1 AND status=?2 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![session_id, enum_str(&status)], Self::row_to_patch)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delivery_event_kinds(&self, delivery_ids: &[i64]) -> rusqlite::Result<Vec<String>> {
        if delivery_ids.is_empty() {
            return Ok(vec![]);
        }
        let marks = delivery_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let mut p: Vec<Box<dyn rusqlite::ToSql>> = vec![];
        for id in delivery_ids {
            p.push(Box::new(*id));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = p.iter().map(|b| b.as_ref()).collect();
        let mut stmt = self.conn.prepare(&format!(
            "SELECT e.kind FROM deliveries d JOIN events e ON e.event_id=d.event_id
             WHERE d.delivery_id IN ({marks}) ORDER BY e.sequence"
        ))?;
        let rows = stmt.query_map(refs.as_slice(), |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// RT-05: ack exactly one delivery (runtime ledger passes the offered ids).
    /// Retention for one session's high-volume bookkeeping: applied deliveries
    /// and events older than `days`. A delivery that is still pending (and every
    /// event it needs) is kept, so replay after a crash stays possible; the
    /// caller decides how much audit history to drop.
    /// Returns (deliveries, events, vacuumed).
    pub fn prune_history(&self, session_id: &str, days: u64, dry_run: bool) -> rusqlite::Result<(i64, i64, bool)> {
        let cutoff = now() - (days as f64) * 86_400.0;
        let count = |sql: &str| -> rusqlite::Result<i64> {
            self.conn.query_row(sql, params![session_id, cutoff], |row| row.get(0))
        };
        let deliveries = count("SELECT COUNT(*) FROM deliveries WHERE session_id=?1 AND status='applied' AND created_at < ?2")?;
        // an event is only droppable once no live delivery still points at it
        let events = count(
            "SELECT COUNT(*) FROM events WHERE session_id=?1 AND created_at < ?2
             AND event_id NOT IN (SELECT event_id FROM deliveries WHERE session_id=?1 AND status != 'applied')",
        )?;
        if dry_run || (deliveries == 0 && events == 0) {
            return Ok((deliveries, events, false));
        }
        self.conn.execute(
            "DELETE FROM deliveries WHERE session_id=?1 AND status='applied' AND created_at < ?2",
            params![session_id, cutoff],
        )?;
        self.conn.execute(
            "DELETE FROM events WHERE session_id=?1 AND created_at < ?2
             AND event_id NOT IN (SELECT event_id FROM deliveries WHERE session_id=?1 AND status != 'applied')",
            params![session_id, cutoff],
        )?;
        // VACUUM cannot run inside a transaction
        let vacuumed = self.conn.execute_batch("VACUUM").is_ok();
        Ok((deliveries, events, vacuumed))
    }

    pub fn ack_delivery_by_id(&self, delivery_id: i64) -> rusqlite::Result<()> {
        let n = self.conn.execute(
            "UPDATE deliveries SET status='applied', applied_at=?1 WHERE delivery_id=?2 AND status='pending'",
            params![now(), delivery_id],
        )?;
        if n == 0 {
            return Ok(());
        }
        // the consume cursor advances with the ack
        let row: Option<(String, String, i64)> = self
            .conn
            .query_row(
                "SELECT session_id, agent_id, batch_no FROM deliveries WHERE delivery_id=?1",
                params![delivery_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((session_id, agent_id, batch_no)) = row {
            self.advance_applied_batch(&session_id, &agent_id, batch_no)?;
        }
        Ok(())
    }

    /// pending_deliveries joined with event data.
    pub fn pending_deliveries_joined(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<Vec<Json>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.delivery_id, d.event_id, d.batch_no, d.payload_override, d.created_at,
                    e.kind AS event_kind, e.payload_json, e.actor_id AS event_actor, e.task_id AS event_task_id, e.sequence AS event_sequence
             FROM deliveries d JOIN events e ON e.event_id = d.event_id
             WHERE d.session_id=?1 AND d.agent_id=?2 AND d.status='pending'
             ORDER BY d.batch_no, e.sequence",
        )?;
        let rows = stmt.query_map(params![session_id, agent_id], |r| {
            Ok(serde_json::json!({
                "delivery_id": r.get::<_, i64>(0)?,
                "event_id": r.get::<_, String>(1)?,
                "batch_no": r.get::<_, i64>(2)?,
                "payload_override": r.get::<_, Option<String>>(3)?,
                "created_at": r.get::<_, f64>(4)?,
                "event_kind": r.get::<_, String>(5)?,
                "payload_json": r.get::<_, String>(6)?,
                "event_actor": r.get::<_, String>(7)?,
                "event_task_id": r.get::<_, Option<String>>(8)?,
                "event_sequence": r.get::<_, i64>(9)?,
            }))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

// -- runtime finalization support ------------------------------------------------

impl Store {
    /// Members parked in WAITING_TASK on this task (to wake with the result).
    pub fn waiters_for_task(&self, session_id: &str, task_id: &str) -> rusqlite::Result<Vec<String>> {
        Ok(self
            .runs_for_session(session_id, &[TurnStatus::WaitingTask])?
            .into_iter()
            .filter(|r| r.waiting_on.iter().any(|t| t == task_id))
            .map(|r| r.agent_id)
            .collect())
    }

    pub fn decided_approvals_for_run(&self, run_id: &str) -> rusqlite::Result<Vec<ApprovalRequest>> {
        let mut stmt = self.conn.prepare(
            "SELECT approval_id, session_id, agent_id, run_id, tool_call_id, operation_hash, requested_scope, policy_revision, status, created_at, decided_at
             FROM approvals WHERE run_id=?1 AND status!='PENDING' ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![run_id], Self::row_to_approval)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn run_cancel_requested(&self, run_id: &str) -> rusqlite::Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT cancel_requested FROM turn_runs WHERE run_id=?1", params![run_id], |r| r.get::<_, i64>(0))
            .optional()?
            .unwrap_or(0)
            != 0)
    }

    /// completion_requests row for a run (runtime._finalize).
    pub fn completion_request(&self, run_id: &str) -> rusqlite::Result<Option<Json>> {
        self.conn
            .query_row(
                "SELECT run_id, task_id, result_refs, summary FROM completion_requests WHERE run_id=?1",
                params![run_id],
                |r| {
                    Ok(serde_json::json!({
                        "run_id": r.get::<_, String>(0)?,
                        "task_id": r.get::<_, String>(1)?,
                        "result_refs": serde_json::from_str::<Json>(&r.get::<_, String>(2)?).unwrap_or(json!([])),
                        "summary": r.get::<_, String>(3)?,
                    }))
                },
            )
            .optional()
    }

    pub fn get_codex_thread(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT external_thread_id FROM agent_runtime WHERE session_id=?1 AND agent_id=?2",
                params![session_id, agent_id],
                |r| r.get(0),
            )
            .optional()
            .map(|o| o.flatten())
    }

    pub fn set_codex_thread(&self, session_id: &str, agent_id: &str, thread_id: &str) -> rusqlite::Result<()> {
        self.ensure_agent(session_id, agent_id)?;
        self.conn.execute(
            "UPDATE agent_runtime SET external_thread_id=?1, updated_at=?2 WHERE session_id=?3 AND agent_id=?4",
            params![thread_id, now(), session_id, agent_id],
        )?;
        Ok(())
    }

    pub fn bump_context_epoch(&self, session_id: &str, agent_id: &str) -> rusqlite::Result<i64> {
        self.ensure_agent(session_id, agent_id)?;
        self.conn.execute(
            "UPDATE agent_runtime SET context_epoch=context_epoch+1, updated_at=?1 WHERE session_id=?2 AND agent_id=?3",
            params![now(), session_id, agent_id],
        )?;
        self.agent_context_epoch(session_id, agent_id)
    }

    pub fn set_run_external_turn(&self, run_id: &str, external_turn_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE turn_runs SET external_turn_id=?1, updated_at=?2 WHERE run_id=?3",
            params![external_turn_id, now(), run_id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ApprovalStatus, EventKind};

    #[test]
    fn future_schema_version_is_an_error_not_a_panic() {
        let path = std::env::temp_dir().join(format!("teamagents-schema-version-{}.db", std::process::id()));
        {
            let store = Store::open(&path).unwrap();
            store
                .conn
                .execute("UPDATE meta SET value='9999' WHERE key='db_schema_version'", [])
                .unwrap();
        }
        let err = Store::open(&path).err().expect("future schema version must be an error");
        assert!(err.to_string().contains("schema version 9999"), "{err}");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_file_name(format!("{}-wal", path.file_name().unwrap().to_str().unwrap())));
        let _ = std::fs::remove_file(path.with_file_name(format!("{}-shm", path.file_name().unwrap().to_str().unwrap())));
    }

    #[test]
    fn history_pruning_keeps_pending_deliveries_and_their_events() {
        let store = store_with_spec();
        let old = now() - 40.0 * 86_400.0;
        let make_event = |id: &str, age: f64| TeamEvent {
            event_id: id.into(),
            sequence: 0,
            session_id: "s1".into(),
            actor_id: "lead".into(),
            task_id: None,
            kind: EventKind::UserMessage,
            payload: json!({"text": id}),
            audience: vec!["lead".into()],
            topology_revision: 1,
            causation_id: None,
            created_at: now() - age,
        };
        store.append_event(&make_event("evt-old-applied", 40.0)).unwrap();
        store.append_event(&make_event("evt-old-pending", 41.0)).unwrap();
        store.append_event(&make_event("evt-recent", 1.0)).unwrap();
        let applied = store.create_delivery("s1", "lead", "evt-old-applied", 1, None).unwrap();
        store.create_delivery("s1", "lead", "evt-old-pending", 1, None).unwrap();
        // append_event stamps now(): age the rows the way a long-lived session would
        store.conn.execute("UPDATE events SET created_at=?1 WHERE event_id IN ('evt-old-applied','evt-old-pending')", rusqlite::params![old]).unwrap();
        store.conn.execute("UPDATE deliveries SET created_at=?1 WHERE delivery_id=?2", rusqlite::params![old, applied]).unwrap();
        store.conn.execute("UPDATE deliveries SET status='applied' WHERE delivery_id=?1", rusqlite::params![applied]).unwrap();

        // a dry run reports without deleting
        assert_eq!(store.prune_history("s1", 30, true).unwrap(), (1, 1, false));
        assert_eq!(store.conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(), 3);

        let (deliveries, events, _) = store.prune_history("s1", 30, false).unwrap();
        assert_eq!((deliveries, events), (1, 1), "one applied delivery and its event go");
        let remaining: Vec<String> = store
            .conn
            .prepare("SELECT event_id FROM events ORDER BY sequence")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(remaining, vec!["evt-old-pending", "evt-recent"], "a pending delivery keeps its event");
        assert_eq!(store.pending_deliveries("s1", "lead").unwrap().len(), 1);
    }

    fn store_with_spec() -> Store {
        let store = Store::open_memory().unwrap();
        store.create_session("s1", "/tmp", "approved_scope").unwrap();
        let spec: TeamSpec = serde_json::from_value(json!({
            "leader_id": "lead",
            "agents": [{"id": "lead", "name": "L", "role": "leader",
                        "runtime_kind": "deepagents", "model_profile": "m"}]
        }))
        .unwrap();
        store.save_team_spec("s1", &spec).unwrap();
        store.ensure_agent("s1", "lead").unwrap();
        store
    }

    #[test]
    fn runs_for_state_keeps_every_active_run_and_caps_terminal_history() {
        let store = store_with_spec();
        let mk = |id: &str, status: TurnStatus, ts: f64| {
            let mut run: TurnRun = serde_json::from_value(json!({
                "run_id": id, "session_id": "s1", "agent_id": "lead",
                "config_revision": 0, "topology_revision": 0,
            }))
            .unwrap();
            run.status = status;
            run.created_at = ts;
            run
        };
        for i in 0..60 {
            store.insert_run(&mk(&format!("old-{i}"), TurnStatus::Completed, i as f64)).unwrap();
        }
        store.insert_run(&mk("queued", TurnStatus::Queued, 1000.0)).unwrap();
        store.insert_run(&mk("waiting", TurnStatus::WaitingApproval, 1001.0)).unwrap();
        let runs = store.runs_for_state("s1", 50).unwrap();
        let ids: Vec<&str> = runs.iter().map(|r| r.run_id.as_str()).collect();
        // active runs are never capped out (the scheduler needs all of them)
        assert!(ids.contains(&"queued") && ids.contains(&"waiting"));
        assert_eq!(runs.iter().filter(|r| r.status.is_terminal()).count(), 50);
        // newest terminal runs survive, oldest drop out of the window
        assert!(ids.contains(&"old-59"));
        assert!(!ids.contains(&"old-9"));
    }

    fn event(store: &Store, event_id: &str) -> String {
        store
            .append_event(&TeamEvent {
                event_id: event_id.into(),
                session_id: "s1".into(),
                sequence: 0,
                actor_id: "system".into(),
                task_id: None,
                kind: EventKind::SessionStatus,
                payload: json!({}),
                audience: vec![],
                topology_revision: 1,
                causation_id: None,
                created_at: now(),
            })
            .unwrap();
        event_id.into()
    }

    fn approval(id: &str, status: ApprovalStatus, created_at: f64, hash: &str) -> ApprovalRequest {
        ApprovalRequest {
            approval_id: id.into(),
            session_id: "s1".into(),
            agent_id: "lead".into(),
            run_id: "run_1".into(),
            tool_call_id: format!("call_{id}"),
            operation_hash: hash.into(),
            requested_scope: json!({}),
            policy_revision: 1,
            status,
            created_at,
            decided_at: None,
        }
    }

    #[test]
    fn legacy_limits_keys_are_dropped_on_load() {
        let store = store_with_spec();
        // a session written before D-10: removed limit keys plus one live override
        let mut data: Json = serde_json::to_value(store.load_team_spec("s1", None).unwrap()).unwrap();
        data["limits"]["leader_reserve"] = json!(2);
        data["limits"]["model_request_timeout_s"] = json!(30);
        data["limits"]["max_auto_retries"] = json!(3);
        data["limits"]["max_members"] = json!(5);
        store
            .conn
            .execute(
                "INSERT INTO team_specs(session_id, revision, spec_json, created_at) VALUES('s1', 2, ?1, ?2)",
                params![data.to_string(), now()],
            )
            .unwrap();

        let spec = store.load_team_spec("s1", None).unwrap();
        assert_eq!(spec.limits.max_members, 5, "live key preserved");
        assert_eq!(spec.limits.max_turns_per_goal, 1000, "unknown keys dropped, defaults apply");
    }

    #[test]
    fn missing_or_corrupt_spec_is_an_error_not_a_panic() {
        let store = Store::open_memory().unwrap();
        let err = store.load_team_spec("ghost", None).unwrap_err();
        assert!(err.contains("no team spec revision"), "{err}");

        store.create_session("s1", "/tmp", "approved_scope").unwrap();
        store
            .conn
            .execute(
                "INSERT INTO team_specs(session_id, revision, spec_json, created_at) VALUES('s1', 1, '{not json', 0.0)",
                [],
            )
            .unwrap();
        let err = store.load_team_spec("s1", None).unwrap_err();
        assert!(err.contains("stored spec is invalid"), "{err}");
    }

    #[test]
    fn unknown_stored_status_is_an_error_not_a_dropped_row() {
        let store = store_with_spec();
        store
            .conn
            .execute(
                "INSERT INTO tasks(task_id, session_id, requester, assignee, description, status, created_at, updated_at)
                 VALUES('t1', 's1', 'lead', 'lead', 'x', 'ANCIENT_STATUS', 0.0, 0.0)",
                [],
            )
            .unwrap();
        let err = store.tasks_for_session("s1", &[]).unwrap_err().to_string();
        assert!(err.contains("bad task status"), "{err}");
        assert!(store.get_task("t1").unwrap_err().to_string().contains("bad task status"));
    }

    #[test]
    fn next_batch_no_advances_the_runtime_ledger() {
        let store = store_with_spec();
        assert_eq!(store.next_batch_no("s1", "lead").unwrap(), 1);
        assert_eq!(store.next_batch_no("s1", "lead").unwrap(), 2);
        let next: i64 = store
            .conn
            .query_row("SELECT next_batch_no FROM agent_runtime WHERE session_id='s1' AND agent_id='lead'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(next, 3);
    }

    #[test]
    fn ack_advances_last_applied_batch() {
        let store = store_with_spec();
        event(&store, "evt_1");
        event(&store, "evt_2");
        let d1 = store.create_delivery("s1", "lead", "evt_1", 1, None).unwrap();
        let d2 = store.create_delivery("s1", "lead", "evt_2", 2, None).unwrap();

        // acking only the first batch must not claim the second
        assert_eq!(store.ack_deliveries_exact("s1", "lead", 1, &[d1]).unwrap(), 1);
        assert_eq!(store.applied_batch("s1", "lead").unwrap(), 1);

        // per-id ack (the runtime path) advances it too
        store.ack_delivery_by_id(d2).unwrap();
        assert_eq!(store.applied_batch("s1", "lead").unwrap(), 2);
        let applied: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM deliveries WHERE status='applied'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied, 2);
    }

    #[test]
    fn expire_approval_only_touches_pending_and_approved_once() {
        let store = store_with_spec();
        store.insert_approval(&approval("a_pending", ApprovalStatus::Pending, 1.0, "h1")).unwrap();
        store.insert_approval(&approval("a_once", ApprovalStatus::ApprovedOnce, 2.0, "h1")).unwrap();
        store.insert_approval(&approval("a_denied", ApprovalStatus::Denied, 3.0, "h1")).unwrap();
        store.insert_approval(&approval("a_session", ApprovalStatus::ApprovedSession, 4.0, "h1")).unwrap();

        assert!(store.expire_approval("a_pending").unwrap());
        assert!(store.expire_approval("a_once").unwrap());
        assert!(!store.expire_approval("a_denied").unwrap(), "other states stay untouched");
        assert!(!store.expire_approval("a_session").unwrap());
        assert!(!store.expire_approval("ghost").unwrap());
        assert!(!store.expire_approval_for_session("s2", "a_pending").unwrap());
        assert!(store.get_approval_for_session("s2", "a_pending").unwrap().is_none());
        assert_eq!(store.get_approval("a_pending").unwrap().unwrap().status, ApprovalStatus::Expired);
        assert_eq!(store.get_approval("a_denied").unwrap().unwrap().status, ApprovalStatus::Denied);
    }

    #[test]
    fn find_run_approval_returns_the_newest_usable_decision() {
        let store = store_with_spec();
        store.insert_approval(&approval("old_pending", ApprovalStatus::Pending, 1.0, "h1")).unwrap();
        store.insert_approval(&approval("new_denied", ApprovalStatus::Denied, 2.0, "h1")).unwrap();
        store.insert_approval(&approval("newer_once", ApprovalStatus::ApprovedOnce, 3.0, "h1")).unwrap();
        store.insert_approval(&approval("other_hash", ApprovalStatus::Pending, 4.0, "h2")).unwrap();

        let found = store.find_run_approval("run_1", "h1").unwrap().unwrap();
        assert_eq!(found.approval_id, "newer_once", "denied is not usable, newest usable wins");
        assert!(store.find_run_approval("run_2", "h1").unwrap().is_none());
        assert!(store.find_run_approval("run_1", "h3").unwrap().is_none());
    }

    #[test]
    fn drop_pending_deliveries_keeps_the_reason() {
        let store = store_with_spec();
        event(&store, "evt_1");
        let d1 = store.create_delivery("s1", "lead", "evt_1", 1, None).unwrap();

        assert_eq!(store.drop_pending_deliveries("s1", "lead", "member removed").unwrap(), 1);
        let (status, override_json): (String, String) = store
            .conn
            .query_row(
                "SELECT status, payload_override FROM deliveries WHERE delivery_id=?1",
                params![d1],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "dropped");
        assert_eq!(serde_json::from_str::<Json>(&override_json).unwrap()["dropped_reason"], json!("member removed"));
    }
}
