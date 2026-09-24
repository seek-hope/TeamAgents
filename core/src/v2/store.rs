//! R2 v2 per-session single SQLite store (plan §4). WAL + explicit
//! synchronous=FULL on verified local filesystems; one writer per database
//! (the Control short transaction); reads use bounded snapshots. Old v1
//! sessions are never migrated; v2's own later upgrades require an explicit
//! migrate-or-refuse decision (A34).

use super::models::{V2_FORMAT_ID, V2_SCHEMA_VERSION};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE IF NOT EXISTS commands (
    command_id TEXT PRIMARY KEY,
    payload_hash TEXT NOT NULL,
    result_json TEXT NOT NULL,
    created REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    session_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    kind TEXT NOT NULL,
    scope TEXT NOT NULL DEFAULT '',
    payload_json TEXT,
    payload_ref TEXT,
    created REAL NOT NULL,
    PRIMARY KEY (session_id, sequence)
);

CREATE TABLE IF NOT EXISTS instances (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    profile_revision INTEGER NOT NULL,
    workspace_ref TEXT NOT NULL DEFAULT '',
    context_epoch INTEGER NOT NULL DEFAULT 0,
    lifecycle TEXT NOT NULL,
    phase TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0,
    active_goal_id TEXT,
    active_request_id TEXT,
    context_head INTEGER NOT NULL DEFAULT 0,
    profile_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS goals (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    original_request_ref TEXT NOT NULL,
    requirement_revision INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL,
    deadline REAL,
    limits_json TEXT NOT NULL,
    known_usage_json TEXT NOT NULL,
    reservations_json TEXT NOT NULL,
    unknown_usage INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    goal_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    requester TEXT NOT NULL,
    assignee TEXT NOT NULL,
    dependencies_json TEXT NOT NULL,
    acceptance_refs_json TEXT NOT NULL,
    status TEXT NOT NULL,
    result_refs_json TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS grants (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    issuer TEXT NOT NULL,
    subject TEXT NOT NULL,
    action TEXT NOT NULL,
    resource_scope TEXT NOT NULL,
    parent_grant_id TEXT,
    revision INTEGER NOT NULL DEFAULT 0,
    revoked_at REAL
);

CREATE TABLE IF NOT EXISTS envelopes (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    recipient TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    kind TEXT NOT NULL,
    correlation_id TEXT,
    payload_json TEXT,
    payload_ref TEXT,
    sequence INTEGER NOT NULL,
    state TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_envelopes_recipient ON envelopes(recipient, state);

CREATE TABLE IF NOT EXISTS context_entries (
    instance_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    idx INTEGER NOT NULL,
    id TEXT NOT NULL,
    kind TEXT NOT NULL,
    message_json TEXT,
    payload_ref TEXT,
    envelope_id TEXT,
    refs_json TEXT NOT NULL DEFAULT '[]',
    created REAL NOT NULL,
    -- R22/A20: when a compression summary covers this entry this points at
    -- the summary entry id. Covered entries are view-hidden, never deleted:
    -- the original text stays reachable through read_history/readback.
    compressed_by TEXT,
    PRIMARY KEY (instance_id, epoch, idx)
);
-- apply dedup: one context change per (instance, epoch, envelope)
CREATE UNIQUE INDEX IF NOT EXISTS idx_context_apply_dedup
    ON context_entries(instance_id, epoch, envelope_id) WHERE envelope_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS model_requests (
    request_id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    goal_id TEXT,
    request_ref TEXT NOT NULL,
    selected_attempt_id TEXT,
    status TEXT NOT NULL,
    est_prompt_tokens INTEGER,
    -- 'turn' | 'compression' (R22/A20): both are billed to the goal and
    -- archived as attempts; only a turn request moves the instance phase.
    kind TEXT NOT NULL DEFAULT 'turn'
);

CREATE TABLE IF NOT EXISTS attempts (
    attempt_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,
    status TEXT NOT NULL,
    error_class TEXT,
    response_ref TEXT,
    usage_json TEXT,
    elapsed_ms INTEGER,
    created REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_attempts_request ON attempts(request_id);

CREATE TABLE IF NOT EXISTS decisions (
    decision_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL UNIQUE,
    completion_json TEXT
);

CREATE TABLE IF NOT EXISTS operations (
    operation_id TEXT PRIMARY KEY,
    decision_id TEXT NOT NULL,
    tool_index INTEGER NOT NULL,
    goal_id TEXT,
    epoch INTEGER NOT NULL,
    intent_json TEXT NOT NULL DEFAULT '',
    args_hash TEXT NOT NULL,
    grant_revision INTEGER NOT NULL DEFAULT 0,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    receipt_json TEXT,
    UNIQUE (decision_id, tool_index)
);
CREATE INDEX IF NOT EXISTS idx_operations_status ON operations(status);

CREATE TABLE IF NOT EXISTS approvals (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    args_hash TEXT NOT NULL,
    grant_revision INTEGER NOT NULL,
    expires_at REAL,
    status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS waits (
    id TEXT PRIMARY KEY,
    instance_id TEXT NOT NULL,
    epoch INTEGER NOT NULL,
    mode TEXT NOT NULL,
    conditions_json TEXT NOT NULL,
    timer_at REAL,
    status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    digest TEXT NOT NULL,
    size INTEGER NOT NULL,
    kind TEXT NOT NULL,
    owner_scope TEXT NOT NULL,
    storage_ref TEXT NOT NULL,
    completeness TEXT NOT NULL,
    owner_ref TEXT,
    created REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_artifacts_state ON artifacts(completeness);
"#;

/// Open (or create) a v2 session database. `create` stamps format/version;
/// opening an existing database verifies them and refuses anything else —
/// never reinterpret incompatible state (A34).
pub fn open(path: &Path, create: bool) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let conn = Connection::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| format!("journal_mode: {e}"))?;
    conn.pragma_update(None, "synchronous", "FULL").map_err(|e| format!("synchronous: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).map_err(|e| format!("busy_timeout: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON").map_err(|e| format!("foreign_keys: {e}"))?;
    // the stamp table must exist before the stamp can be read; everything
    // else is created only after the format check accepts the database
    conn.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .map_err(|e| format!("meta table: {e}"))?;
    let format: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'format_id'", [], |row| row.get(0))
        .optional()
        .map_err(|e| format!("read meta: {e}"))?;
    match format {
        None if create => {
            conn.execute_batch(SCHEMA).map_err(|e| format!("create schema: {e}"))?;
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('format_id', ?1), ('schema_version', ?2)",
                rusqlite::params![V2_FORMAT_ID, V2_SCHEMA_VERSION.to_string()],
            )
            .map_err(|e| format!("stamp meta: {e}"))?;
        }
        None => return Err(format!("{} is not a v2 session database (no format stamp)", path.display())),
        Some(found) => {
            if found != V2_FORMAT_ID {
                return Err(format!(
                    "{} has format {found:?}, expected {V2_FORMAT_ID:?}; refusing to reinterpret state",
                    path.display()
                ));
            }
            let version: String = conn
                .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
                .map_err(|e| format!("read schema_version: {e}"))?;
            let parsed = version.parse::<i64>().unwrap_or(-1);
            if parsed != V2_SCHEMA_VERSION {
                // v2's own upgrades migrate explicitly, in one transaction, or
                // refuse: a half-migrated or unknown version is never
                // reinterpreted as current state (§4.4, A34).
                migrate(&conn, parsed)?;
            }
            conn.execute_batch(SCHEMA).map_err(|e| format!("verify schema: {e}"))?;
        }
    }
    Ok(conn)
}

/// One-step-v1 chains are migrated in a single transaction; anything else is
/// refused. Each step must be self-contained (DDL + stamp together), so a
/// crash can only leave the database at its previous version.
fn migrate(conn: &Connection, from: i64) -> Result<(), String> {
    if from != V2_SCHEMA_VERSION - 1 {
        return Err(format!(
            "v2 schema version {from} != supported {V2_SCHEMA_VERSION}; explicit migration or refusal required"
        ));
    }
    let tx = conn.unchecked_transaction().map_err(|e| format!("migrate tx: {e}"))?;
    // 1 → 2 (R22/A20): compression provenance needs no table of its own.
    tx.execute_batch(
        "ALTER TABLE context_entries ADD COLUMN compressed_by TEXT;
         ALTER TABLE model_requests ADD COLUMN kind TEXT NOT NULL DEFAULT 'turn';",
    )
    .map_err(|e| format!("migrate 1 -> 2: {e}"))?;
    tx.execute("UPDATE meta SET value = ?1 WHERE key = 'schema_version'", [V2_SCHEMA_VERSION.to_string()])
        .map_err(|e| format!("migrate stamp: {e}"))?;
    tx.commit().map_err(|e| format!("migrate commit: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("teamagents-v2-store-{tag}-{}.db", uuid::Uuid::new_v4()))
    }

    fn cleanup(p: &std::path::Path) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(format!("{}-wal", p.display()));
        let _ = std::fs::remove_file(format!("{}-shm", p.display()));
    }

    #[test]
    fn open_creates_stamps_and_reopens() {
        let p = path("create");
        {
            let conn = open(&p, true).expect("create");
            let format: String =
                conn.query_row("SELECT value FROM meta WHERE key = 'format_id'", [], |row| row.get(0)).unwrap();
            assert_eq!(format, V2_FORMAT_ID);
            let journal: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0)).unwrap();
            assert_eq!(journal, "wal");
        }
        // reopen verifies the stamp instead of re-stamping
        let conn = open(&p, false).expect("reopen");
        let version: String =
            conn.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0)).unwrap();
        assert_eq!(version, V2_SCHEMA_VERSION.to_string());
        drop(conn);
        cleanup(&p);
    }

    #[test]
    fn open_refuses_unstamped_foreign_and_wrong_version() {
        // a database without a stamp is not a v2 session (A34)
        let plain = path("plain");
        {
            let conn = Connection::open(&plain).unwrap();
            conn.execute_batch("CREATE TABLE t (x TEXT);").unwrap();
        }
        let err = open(&plain, false).unwrap_err();
        assert!(err.contains("no format stamp"), "{err}");
        cleanup(&plain);

        // a foreign format id is never reinterpreted
        let foreign = path("foreign");
        {
            let conn = Connection::open(&foreign).unwrap();
            conn.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
            conn.execute("INSERT INTO meta VALUES ('format_id', 'other-store'), ('schema_version', '1')", []).unwrap();
        }
        let err = open(&foreign, false).unwrap_err();
        assert!(err.contains("refusing to reinterpret"), "{err}");
        cleanup(&foreign);

        // a newer/older schema version requires an explicit migration decision
        let versioned = path("version");
        {
            let conn = open(&versioned, true).unwrap();
            conn.execute("UPDATE meta SET value = '99' WHERE key = 'schema_version'", []).unwrap();
        }
        let err = open(&versioned, false).unwrap_err();
        assert!(err.contains("schema version 99"), "{err}");
        cleanup(&versioned);
    }

    #[test]
    fn open_migrates_the_previous_schema_version() {
        let p = path("migrate");
        {
            let conn = open(&p, true).unwrap();
            // the previous (v1) shape: no compression columns, older stamp
            conn.execute_batch(
                "ALTER TABLE context_entries DROP COLUMN compressed_by;
                 ALTER TABLE model_requests DROP COLUMN kind;
                 UPDATE meta SET value = '1' WHERE key = 'schema_version';",
            )
            .unwrap();
        }
        let conn = open(&p, false).expect("migrate");
        let version: String =
            conn.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0)).unwrap();
        assert_eq!(version, V2_SCHEMA_VERSION.to_string());
        // the migrated columns exist and carry their defaults
        conn.execute(
            "INSERT INTO model_requests (request_id, instance_id, epoch, request_ref, status)
             VALUES ('r1', 'i1', 0, '', 'PENDING')",
            [],
        )
        .unwrap();
        let kind: String =
            conn.query_row("SELECT kind FROM model_requests WHERE request_id = 'r1'", [], |row| row.get(0)).unwrap();
        assert_eq!(kind, "turn");
        conn.execute(
            "INSERT INTO context_entries (instance_id, epoch, idx, id, kind, message_json, created)
             VALUES ('i1', 0, 1, 'i1:0:1', 'user', '{}', 0)",
            [],
        )
        .unwrap();
        let covered: Option<String> = conn
            .query_row("SELECT compressed_by FROM context_entries WHERE id = 'i1:0:1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(covered, None);
        drop(conn);
        cleanup(&p);
    }
}
