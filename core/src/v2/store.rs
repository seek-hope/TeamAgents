//! R2 v2 per-session single SQLite store (plan §4). WAL + explicit
//! synchronous=FULL on verified local filesystems; one writer per database
//! (the Control short transaction); reads use bounded snapshots. Old v1
//! sessions are never migrated; v2's own later upgrades require an explicit
//! migrate-or-refuse decision (A34).

use super::models::{V2_FORMAT_ID, V2_SCHEMA_VERSION};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

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

-- D-165: `checkpoint` reads the last `instance_lifecycle` per instance for the row's `reason`, and every client
-- polls the checkpoint (`exec` and the TUI at ~4 Hz). Measured 2026-09-27 over a 200k-event log: the
-- kind-filtered read took 44.6 ms per call without this index (a full b-tree walk, 20 matching rows) and 0.02 ms
-- with it; the write side pays 1.71x on a bulk insert (100k events: 228 ms -> 392 ms, ~1.6 us per row). The
-- index is additive and applied to an existing database by the `verify schema` pass (`IF NOT EXISTS`, no format
-- or schema-version change), so a v3 state root opens unchanged.
CREATE INDEX IF NOT EXISTS idx_events_kind_sequence ON events(kind, sequence);

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
    kind TEXT NOT NULL DEFAULT 'turn',
    -- D-349 (the decision D-341 made for D-143): what the request was *offered* — the tool names, as a JSON
    -- array, never a schema or a payload (`docs/TOOLS.md` holds those) — and whether the surface check
    -- authorized that set against the grants at assembly time. Written once, at registration: a later grant
    -- changes the *next* request's record and never rewrites this one.
    offered_tools TEXT,
    surface_authorized INTEGER NOT NULL DEFAULT 0
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
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("{}: busy_timeout: {e}", path.display()))?;
    // Identity first, and read-only: the stamp is never created, and no file is
    // asked to switch journal mode, until the format check has accepted it. A
    // database that turns out to belong to someone else is therefore not
    // written to at all — not one table, not one header byte (A34).
    let has_meta = conn
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| format!("inspect {}: {e}", path.display()))?
        > 0;
    let format: Option<String> = if has_meta {
        conn.query_row("SELECT value FROM meta WHERE key = 'format_id'", [], |row| row.get(0))
            .optional()
            .map_err(|e| format!("read meta: {e}"))?
    } else {
        None
    };
    match format {
        None if create => {
            // An existing unstamped file may be someone else's database: never
            // adopt it. The v2 schema's own tables are allowed through, so a
            // database interrupted between its schema and its stamp (a crash in
            // this very function) is still completed instead of refused.
            let foreign = foreign_tables(&conn)?;
            if !foreign.is_empty() {
                return Err(format!(
                    "{} is not a v2 session database (no format stamp) and holds tables this session does \
                     not own ({}); move it aside or use another state root",
                    path.display(),
                    foreign.join(", ")
                ));
            }
            apply_store_pragmas(&conn, path)?;
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
            apply_store_pragmas(&conn, path)?;
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

/// Open a v2 session database **read-only** (D-253): the same identity check — a foreign, unstamped or
/// wrong-format file is refused — but nothing is written. Not the stamp, not the pragmas (WAL, `synchronous`,
/// `foreign_keys` are a *writer's* settings, A34), no migration and no schema verification.
///
/// It exists because a reader must work where this process cannot write: an `EVIDENCE`-marked root on read-only
/// media, another user's root, or — the case that produced it — a state root whose schema predates this build,
/// where `open` would migrate (a write) as a side effect of *reading*. `teamagents artifacts [list]` is the
/// first caller. The version is not migrated here; a reader that needs the current schema must say so, and a
/// writer (`gc`) uses `open`, which migrates the way a session's own boot does.
pub fn open_read_only(path: &Path) -> Result<Connection, String> {
    // A write-ahead-log database cannot be read read-only without its shared-memory file: SQLite has to be able
    // to open (or create) `<db>-shm`. Probing it here is what turns an unhelpful `attempt to write a readonly
    // database` into a named refusal — measured on a WAL-mode root whose directory was not writable and whose shm
    // was gone, which is exactly the read-only-media case this open exists for, and (the other direction) on a
    // root where a leftover shm made the reader answer **stale** — zero artifacts and no error at all. A silent
    // under-report is the one answer this must never give.
    let shm = PathBuf::from(format!("{}-shm", path.display()));
    let probed = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&shm).is_ok();
    if !probed {
        return Err(format!(
            "{}: this root is in write-ahead-log mode and its shared-memory file ({}) is missing and cannot be \
             created here, so a read-only open cannot attach the log; open the root once with a session (which \
             creates it), or copy it somewhere writable, and read again",
            path.display(),
            shm.display()
        ));
    }
    let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| format!("{}: busy_timeout: {e}", path.display()))?;
    let has_meta = conn
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| format!("inspect {}: {e}", path.display()))?
        > 0;
    if !has_meta {
        return Err(format!("{} is not a v2 session database (no format stamp)", path.display()));
    }
    let format: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'format_id'", [], |row| row.get(0))
        .optional()
        .map_err(|e| format!("read meta: {e}"))?;
    match format.as_deref() {
        Some(found) if found == V2_FORMAT_ID => Ok(conn),
        Some(found) => Err(format!(
            "{} has format {found:?}, expected {V2_FORMAT_ID:?}; refusing to reinterpret state",
            path.display()
        )),
        None => Err(format!("{} is not a v2 session database (no format stamp)", path.display())),
    }
}

/// The store's own durability settings. Applied only to a database this session
/// owns: they are the settings a *write* needs, and switching a foreign file to
/// WAL is a write (A34).
fn apply_store_pragmas(conn: &Connection, path: &Path) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL").map_err(|e| format!("{}: journal_mode: {e}", path.display()))?;
    conn.pragma_update(None, "synchronous", "FULL").map_err(|e| format!("{}: synchronous: {e}", path.display()))?;
    conn.pragma_update(None, "foreign_keys", "ON").map_err(|e| format!("{}: foreign_keys: {e}", path.display()))?;
    Ok(())
}

/// The first SQLite release carrying the official WAL-reset fix (DESIGN §4.4): 3.51.3, and the specific
/// backports of it.
pub const MIN_SQLITE_VERSION: i32 = 3_051_003;

/// Whether the SQLite this binary **actually links** carries that fix.
///
/// A runtime answer, never the crate's declared version (DESIGN §4.4): `rusqlite::version_number()` reports the
/// library the build linked, so a dependency change, a switch to a system SQLite or a vendored patch is seen
/// here. One predicate, three readers — `doctor`'s state-root row, the store's own test in `make check`, and
/// the probe envelope (`engine/examples/probe/suite.rs`) that used to carry this comparison inline.
///
/// Ceiling: a backport keeps its old version number, so a patched 3.49.x is reported as too old. That is the
/// conservative direction — the check refuses until someone states the backport's fix in a decision.
pub fn linked_sqlite_carries_the_wal_reset_fix() -> bool {
    rusqlite::version_number() >= MIN_SQLITE_VERSION
}

/// Table names `SCHEMA` itself creates, read from the schema text so the list
/// cannot drift from it (an index or trigger does not count as a table).
fn schema_tables() -> std::collections::HashSet<String> {
    SCHEMA
        .lines()
        .filter_map(|line| line.trim().strip_prefix("CREATE TABLE IF NOT EXISTS "))
        .filter_map(|rest| rest.split([' ', '(']).next().filter(|name| !name.is_empty()).map(str::to_string))
        .collect()
}

/// Tables in an existing file that the v2 schema did not create. An unstamped
/// database containing any of them belongs to someone else (§4.4, A34); the
/// `meta` stamp table is created before this check and is never foreign.
fn foreign_tables(conn: &Connection) -> Result<Vec<String>, String> {
    let own = schema_tables();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|e| format!("inspect tables: {e}"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| format!("inspect tables: {e}"))?;
    let mut foreign = vec![];
    for name in names {
        let name = name.map_err(|e| format!("inspect tables: {e}"))?;
        if name != "meta" && !own.contains(&name) {
            foreign.push(name);
        }
    }
    foreign.sort();
    Ok(foreign)
}

/// A store older than the current version is walked up one step at a time in a
/// single transaction; a version from the future (or from before the first
/// step) is refused. Each step is self-contained, so a crash can only leave the
/// database at a version this chain knows how to continue from.
fn migrate(conn: &Connection, from: i64) -> Result<(), String> {
    if from > V2_SCHEMA_VERSION {
        return Err(format!(
            "v2 schema version {from} is newer than this build ({V2_SCHEMA_VERSION}); refusing to reinterpret state"
        ));
    }
    if from < 1 {
        return Err(format!("v2 schema version {from} is not a known version; explicit migration or refusal required"));
    }
    let tx = conn.unchecked_transaction().map_err(|e| format!("migrate tx: {e}"))?;
    let mut version = from;
    while version < V2_SCHEMA_VERSION {
        match version {
            // 1 → 2 (R22/A20): compression provenance needs no table of its own.
            1 => tx
                .execute_batch(
                    "ALTER TABLE context_entries ADD COLUMN compressed_by TEXT;
                     ALTER TABLE model_requests ADD COLUMN kind TEXT NOT NULL DEFAULT 'turn';",
                )
                .map_err(|e| format!("migrate 1 -> 2: {e}"))?,
            // 2 → 3 (D-71): the runtime's own closing notes were stored as the
            // member's assistant text — the shape a client reading "the last
            // assistant entry is the reply" reported as the model's answer. The
            // writer is fixed and so is the past: rewrite them where they are,
            // identified by the envelope the runtime itself generated (a model
            // entry carries its decision id there, never one of these).
            2 => rewrite_closing_notes(&tx)?,
            // 3 → 4 (D-349): the per-request surface record. Past requests keep NULL/0: the record did not
            // exist when they ran, and inventing one would be the inference this column exists to replace.
            3 => tx
                .execute_batch(
                    "ALTER TABLE model_requests ADD COLUMN offered_tools TEXT;
                     ALTER TABLE model_requests ADD COLUMN surface_authorized INTEGER NOT NULL DEFAULT 0;",
                )
                .map_err(|e| format!("migrate 3 -> 4: {e}"))?,
            other => return Err(format!("no migration step from v2 schema version {other}")),
        }
        version += 1;
        tx.execute("UPDATE meta SET value = ?1 WHERE key = 'schema_version'", [version.to_string()])
            .map_err(|e| format!("migrate stamp: {e}"))?;
    }
    tx.commit().map_err(|e| format!("migrate commit: {e}"))?;
    Ok(())
}

/// 2 → 3: `assistant` → `runtime` for the runtime's own three closing notes,
/// with their message role turned from `assistant` to `user` — they were never
/// the member speaking (§8, D-71).
fn rewrite_closing_notes(tx: &rusqlite::Transaction) -> Result<(), String> {
    let rows: Vec<(String, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT id, message_json FROM context_entries
                 WHERE kind = 'assistant'
                   AND (envelope_id LIKE 'goal-close-%' OR envelope_id LIKE 'goal-block-%'
                        OR envelope_id LIKE 'turn-close-%')",
            )
            .map_err(|e| format!("migrate 2 -> 3 scan: {e}"))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("migrate 2 -> 3 query: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("migrate 2 -> 3 collect: {e}"))?
    };
    for (id, raw) in rows {
        let mut message: Json = serde_json::from_str(&raw).map_err(|e| format!("migrate {id}: {e}"))?;
        if message["role"] == json!("assistant") {
            message["role"] = json!("user");
        }
        tx.execute(
            "UPDATE context_entries SET kind = 'runtime', message_json = ?1 WHERE id = ?2",
            rusqlite::params![message.to_string(), id],
        )
        .map_err(|e| format!("migrate {id}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D-253: a read-only open refuses what `open` refuses and **writes nothing** — a reader must not migrate a
    /// state root. The case that produced it: `artifacts list` against a root whose schema predates this build
    /// attempted the migration inside a *read* and failed wherever the process could not write, which is exactly the
    /// read-only (or read-only-media `EVIDENCE`) root a reader has to serve.
    #[test]
    fn a_read_only_open_reads_a_foreign_or_old_root_without_touching_it() {
        use super::{open, open_read_only};
        use crate::v2::models::V2_FORMAT_ID;
        // A unique directory, like the sibling helper: a pid-keyed one collided with a *previous* run's leftover
        // (a stamped store, a 0555 directory) and made the fixture itself flaky — measured while writing the
        // read-only test, and the reason this is a uuid rather than a pid.
        let dir = std::env::temp_dir().join(format!("ta-store-readonly-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session.sqlite");
        {
            let conn = open(&path, true).unwrap();
            // a stamp from an older build: a writable open would migrate it (that is the reader's hazard)
            conn.execute("UPDATE meta SET value = '1' WHERE key = 'schema_version'", []).unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        let reader = open_read_only(&path).unwrap();
        let seen: String =
            reader.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0)).unwrap();
        assert_eq!(seen, "1", "a read-only open must not migrate");
        // the identity check still runs, and a write on it is refused rather than silently allowed
        assert!(reader.execute("UPDATE meta SET value = '9' WHERE key = 'schema_version'", []).is_err());
        drop(reader);
        assert_eq!(std::fs::read(&path).unwrap(), before, "a read changed the store's bytes");
        // it never creates a database: a missing path stays missing
        let missing = dir.join("absent.sqlite");
        assert!(open_read_only(&missing).is_err());
        assert!(!missing.exists(), "a read-only open must not create anything");
        // a file that is not ours is refused, read-only included
        let foreign = dir.join("foreign.sqlite");
        {
            let conn = rusqlite::Connection::open(&foreign).unwrap();
            conn.execute_batch("CREATE TABLE someone_elses (id INTEGER)").unwrap();
        }
        assert!(open_read_only(&foreign).is_err(), "an unstamped file is not a v2 store");
        let wrong_format = dir.join("wrong.sqlite");
        {
            let conn = open(&wrong_format, true).unwrap();
            conn.execute("UPDATE meta SET value = 'not-ours' WHERE key = 'format_id'", []).unwrap();
        }
        let refused = open_read_only(&wrong_format).expect_err("a foreign format is refused");
        assert!(refused.contains("refusing to reinterpret state"), "{refused}");
        // a write-ahead log with commits and no shared-memory file: a read-only open would answer *stale* (the
        // measured shape: zero artifacts, no error), so it refuses and names the fix instead
        let stale = dir.join("stale.sqlite");
        {
            let conn = open(&stale, true).unwrap();
            conn.execute("UPDATE meta SET value = '1' WHERE key = 'schema_version'", []).unwrap();
        }
        let _ = std::fs::remove_file(format!("{}-shm", stale.display()));
        let unwritable = std::fs::metadata(&dir).unwrap().permissions();
        std::fs::set_permissions(&dir, std::os::unix::fs::PermissionsExt::from_mode(0o555)).unwrap();
        let refused = open_read_only(&stale).expect_err("no attachable shm in an unwritable root: refuse");
        assert!(refused.contains("shared-memory"), "{refused}");
        assert!(refused.contains("writable"), "the refusal names the fix: {refused}");
        std::fs::set_permissions(&dir, unwritable).unwrap();
        let _ = V2_FORMAT_ID;
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("teamagents-v2-store-{tag}-{}.db", uuid::Uuid::new_v4()))
    }

    fn cleanup(p: &std::path::Path) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(format!("{}-wal", p.display()));
        let _ = std::fs::remove_file(format!("{}-shm", p.display()));
    }

    /// DESIGN §4.4: the state store's durability rests on SQLite's WAL-reset fix, so the version this build
    /// links is a requirement, not a detail. `make check` runs this offline; `doctor` reports the same
    /// predicate to a user, and the probe envelope asserts it before it measures anything else.
    #[test]
    fn the_linked_sqlite_carries_the_wal_reset_fix() {
        let linked = rusqlite::version_number();
        assert!(
            super::linked_sqlite_carries_the_wal_reset_fix(),
            "the linked SQLite is {} ({}), older than the WAL-reset fix {} (DESIGN §4.4); bump libsqlite3-sys \
             or link a backport in a recorded decision — do not weaken the sync strategy instead",
            rusqlite::version(),
            linked,
            super::MIN_SQLITE_VERSION
        );
        assert!(rusqlite::version().starts_with("3."), "unexpected SQLite version string {}", rusqlite::version());
    }

    #[test]
    fn open_never_adopts_an_unstamped_file_that_holds_foreign_tables() {
        // create = true is what the daemon passes; it must not turn someone
        // else's database into a session store by adding tables to it (A34).
        let foreign = path("adopt");
        let before = {
            {
                let conn = Connection::open(&foreign).unwrap();
                conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);").unwrap();
                conn.execute("INSERT INTO users (name) VALUES ('alice')", []).unwrap();
            }
            std::fs::read(&foreign).unwrap()
        };
        let err = open(&foreign, true).unwrap_err();
        assert!(err.contains("not a v2 session database"), "{err}");
        assert!(err.contains("users"), "the refusal names the foreign tables: {err}");
        assert_eq!(std::fs::read(&foreign).unwrap(), before, "the refusal wrote nothing to the file");
        {
            let conn = Connection::open(&foreign).unwrap();
            let names: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<String>, _>>()
                .unwrap();
            assert_eq!(names, vec!["users"], "the foreign file is not written to at all, meta included");
            let rows: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0)).unwrap();
            assert_eq!(rows, 1, "and its rows are untouched");
        }
        cleanup(&foreign);

        // the same file opened for reads was already refused (the message is
        // identical), so the two entry points now agree
        let again = path("adopt-read");
        {
            let conn = Connection::open(&again).unwrap();
            conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY);").unwrap();
        }
        assert!(open(&again, false).unwrap_err().contains("not a v2 session database"));
        cleanup(&again);
    }

    #[test]
    fn open_completes_a_session_database_that_lost_its_stamp_to_a_crash() {
        // A crash between the schema batch and the stamp insert leaves v2's own
        // tables without a stamp; that file is ours to finish, and refusing it
        // would strand the session.
        let p = path("half");
        {
            let conn = Connection::open(&p).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
        }
        let conn = open(&p, true).expect("a half-initialized v2 database is completed");
        let format: String =
            conn.query_row("SELECT value FROM meta WHERE key = 'format_id'", [], |row| row.get(0)).unwrap();
        assert_eq!(format, V2_FORMAT_ID);
        drop(conn);
        cleanup(&p);
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
            // the previous (v1) shape: no compression columns, no per-request surface record, older stamp
            conn.execute_batch(
                "ALTER TABLE context_entries DROP COLUMN compressed_by;
                 ALTER TABLE model_requests DROP COLUMN kind;
                 ALTER TABLE model_requests DROP COLUMN offered_tools;
                 ALTER TABLE model_requests DROP COLUMN surface_authorized;
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

    /// D-71: a session written before the fix carries the runtime's own closing
    /// notes as the member's assistant text. The migration recognises them by the
    /// envelope the runtime generated (a model entry carries its decision id
    /// there) and moves them to the runtime kind in the user's voice, so no
    /// reader can take the runtime's word for the model's answer.
    #[test]
    fn migrate_rewrites_the_runtimes_closing_notes() {
        let p = path("notes");
        {
            let conn = open(&p, true).unwrap();
            conn.execute_batch(
                "INSERT INTO context_entries (instance_id, epoch, idx, id, kind, message_json, envelope_id, created)
                 VALUES ('i1', 0, 1, 'i1:0:1', 'assistant',
                         '{\"role\":\"assistant\",\"content\":\"runtime: goal g1 closed as SUCCEEDED\"}',
                         'goal-close-g1', 0),
                        ('i1', 0, 2, 'i1:0:2', 'assistant',
                         '{\"role\":\"assistant\",\"content\":\"the real answer\"}', 'dec-1', 0),
                        ('i1', 0, 3, 'i1:0:3', 'tool_result',
                         '{\"role\":\"tool\",\"tool_call_id\":\"c1\",\"content\":\"out\"}', 'op-1', 0);
                 ALTER TABLE model_requests DROP COLUMN offered_tools;
                 ALTER TABLE model_requests DROP COLUMN surface_authorized;
                 UPDATE meta SET value = '2' WHERE key = 'schema_version';",
            )
            .unwrap();
        }
        let conn = open(&p, false).expect("migrate");
        let version: String =
            conn.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0)).unwrap();
        assert_eq!(version, V2_SCHEMA_VERSION.to_string());
        let note: (String, String) = conn
            .query_row("SELECT kind, message_json FROM context_entries WHERE id = 'i1:0:1'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(note.0, "runtime");
        assert!(note.1.contains(r#""role":"user""#), "{}", note.1);
        // the member's own answer and the tool result are untouched
        let answer: (String, String) = conn
            .query_row("SELECT kind, message_json FROM context_entries WHERE id = 'i1:0:2'", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(answer.0, "assistant");
        assert!(answer.1.contains(r#""role":"assistant""#), "{}", answer.1);
        let receipt: String =
            conn.query_row("SELECT kind FROM context_entries WHERE id = 'i1:0:3'", [], |row| row.get(0)).unwrap();
        assert_eq!(receipt, "tool_result");
        drop(conn);
        cleanup(&p);
    }
}
