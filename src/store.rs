use crate::{
    model::{ModelOutput, ModelSettings, UsageStats},
    protocol::TurnKind,
    sandbox::SandboxMode,
    Result,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path, time::Duration};
use uuid::Uuid;

mod tree;

fn collapse_ascii_ws(s: &str) -> String {
    s.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalized_action_key(action: &Value) -> Option<(String, Value)> {
    let op = action.get("op")?.as_str()?.to_owned();
    let key = match op.as_str() {
        "shell" => {
            let cmd = action.get("command").and_then(Value::as_str).unwrap_or("");
            json!({"op": "shell", "command": collapse_ascii_ws(cmd)})
        }
        "read_file" => json!({
            "op": "read_file",
            "path": &action["path"],
            "offset": &action["offset"],
            "limit": &action["limit"],
        }),
        _ => action.clone(),
    };
    Some((op, key))
}

fn is_readonly_shell_probe(cmd: &str) -> bool {
    let first = cmd.trim_start().split(|c: char| c.is_ascii_whitespace() || c == ';').next().unwrap_or("");
    matches!(
        first,
        "pwd"
            | "ls"
            | "sha256sum"
            | "md5sum"
            | "grep"
            | "rg"
            | "cat"
            | "wc"
            | "head"
            | "tail"
            | "sed"
            | "stat"
            | "find"
            | "file"
            | "which"
            | "command"
    )
}

/// Tool output is model context, not an archival channel. Keep one result below the
/// worker request limit even when there are too few messages for auto-compaction.
const TOOL_RESULT_LIMIT: usize = 1024 * 1024;
const BATCH_RESULTS_LIMIT: usize = 2 * 1024 * 1024;

fn bound_tool_result(text: &str) -> String {
    if text.len() <= TOOL_RESULT_LIMIT {
        return text.to_owned();
    }
    let marker = format!("\n\n[tool result truncated at {TOOL_RESULT_LIMIT} bytes]");
    let end = text.floor_char_boundary(TOOL_RESULT_LIMIT - marker.len());
    format!("{}{marker}", &text[..end])
}

// Bytes inside a compact serde_json string, excluding its two enclosing quotes.
fn escaped_byte_len(b: u8) -> usize {
    match b {
        b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 8 | 12 => 2,
        0..=31 => 6,
        _ => 1,
    }
}

fn escaped_len(text: &str) -> usize {
    text.bytes().map(escaped_byte_len).sum()
}

// Keep whole entries in seq order. A storage-truncated receipt can be shortened again
// for replay, so include the entries omitted by the first pass in the final count.
fn truncate_wb_read(receipt: &mut Value, limit: usize, size: fn(&str) -> usize) {
    let entries = std::mem::take(receipt["entries"].as_array_mut().expect("whiteboard read entries"));
    let omitted = receipt["omitted_entries"].as_u64().unwrap_or(0) + entries.len() as u64;
    receipt["truncated"] = json!(true);
    receipt["omitted_entries"] = json!(omitted);
    let mut used = size(&receipt.to_string());
    let mut kept = Vec::new();
    for entry in entries {
        used += size(&entry.to_string()) + usize::from(!kept.is_empty());
        if used > limit {
            break;
        }
        kept.push(entry);
    }
    receipt["omitted_entries"] = json!(omitted - kept.len() as u64);
    receipt["entries"] = Value::Array(kept);
}

fn truncate_wb_write(receipt: &mut Value) {
    for field in ["current", "entry"] {
        if let Some(entry) = receipt[field].as_object_mut() {
            // Keep identity, sequence and CAS metadata, even with oversized legacy author names.
            entry.retain(|key, _| !matches!(key.as_str(), "content" | "author_agent" | "author_task"));
        }
    }
    receipt["truncated"] = json!(true);
}

fn repetition_nudge(messages: &[Value]) -> Option<&'static str> {
    let mut actions: Vec<(String, Value, Option<String>)> = Vec::new();
    for msg in messages.iter().rev() {
        let Some(role) = msg["role"].as_str() else { continue };
        let Some(content) = msg["content"].as_str() else { continue };
        if role == "user" {
            if let Ok(v) = serde_json::from_str::<Value>(content) {
                if v.get("compacted_history").and_then(Value::as_bool) == Some(true) {
                    break;
                }
            }
            continue;
        }
        if role != "assistant" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(content) else { continue };
        if v.get("batch").is_some() {
            continue;
        }
        let Some((op, key)) = normalized_action_key(&v) else { continue };
        if matches!(op.as_str(), "edit_file" | "apply_patch" | "wb_write" | "handoff" | "finish" | "update_plan") {
            break;
        }
        let cmd = (op == "shell").then(|| v["command"].as_str().unwrap_or("").to_owned());
        actions.push((op, key, cmd));
        if actions.len() >= 10 {
            break;
        }
    }
    if actions.is_empty() {
        return None;
    }
    let window = &actions[..actions.len().min(6)];
    let duplicate = window[1..].iter().any(|(_, key, _)| *key == window[0].1);
    let shell_count = window.iter().filter(|(op, _, _)| op == "shell").count();
    let probe_count = window
        .iter()
        .filter(|(op, _, cmd)| op == "shell" && cmd.as_deref().is_some_and(is_readonly_shell_probe))
        .count();
    if duplicate || (shell_count >= 4 && probe_count >= 3) {
        Some(
            "You have repeated similar read-only inspection commands across several turns. Do not re-verify unchanged workspace state; proceed directly to making the next edit (prefer edit_file), handing off subtasks, or calling finish with your verified results.",
        )
    } else {
        None
    }
}

/// A session store is designed for one daemon process. Tests may open two handles to
/// exercise SQLite serialization, but production multi-daemon sharing is unsupported.
pub struct Store {
    db: Connection,
}

#[derive(Debug)]
pub struct Turn {
    pub kind: TurnKind,
    pub root: String,
    pub task: String,
    pub agent: String,
    pub messages: Vec<Value>,
    /// Computed before adding transient instructions to the outbound request.
    pub can_compact: bool,
    /// Pre-snapshot roots retain the provider's legacy environment-based selection.
    pub settings: Option<ModelSettings>,
    pub last_input_tokens: u64,
    pub last_history_bytes: usize,
}

#[derive(Debug)]
pub struct ToolJob {
    pub op_id: String,
    pub task: String,
    pub action: Value,
    pub sandbox: SandboxMode,
}

#[derive(Debug)]
pub struct CompactionJob {
    pub task: String,
    pub agent: String,
    pub head_keep: usize,
    pub tail_keep: usize,
    pub middle: Vec<Value>,
    pub settings: Option<ModelSettings>,
}

// These pragmas and the schema are committed together, before any work is admitted.
const APP_ID: i64 = 0x5357524d; // SWRM
const MAX_CHECK_ATTEMPTS: u32 = 10;
const SCHEMA_V1: &str = "
CREATE TABLE settings (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    max_agents INTEGER NOT NULL CHECK(max_agents>0),
    turn_budget INTEGER NOT NULL CHECK(turn_budget>0),
    workspace BLOB
);
CREATE TABLE agents (name TEXT PRIMARY KEY NOT NULL);
CREATE TABLE tasks (
    id TEXT PRIMARY KEY NOT NULL,
    root TEXT NOT NULL REFERENCES tasks(id), parent TEXT REFERENCES tasks(id),
    agent TEXT NOT NULL REFERENCES agents(name), prompt TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('queued','running','waiting','approval','done','failed','cancelled','unknown')),
    turns_used INTEGER NOT NULL DEFAULT 0,
    introduced INTEGER NOT NULL DEFAULT 0,
    model_reserved INTEGER NOT NULL DEFAULT 0,
    check_command TEXT, proposal TEXT, result TEXT
);
CREATE INDEX runnable ON tasks(status);
CREATE UNIQUE INDEX one_model_per_agent ON tasks(agent) WHERE model_reserved=1;
CREATE TABLE messages (
    agent TEXT NOT NULL REFERENCES agents(name),
    role TEXT NOT NULL CHECK(role IN ('assistant','user')), content TEXT NOT NULL
);
CREATE INDEX private_chat ON messages(agent);
CREATE TABLE operations (
    id TEXT PRIMARY KEY NOT NULL, task TEXT NOT NULL REFERENCES tasks(id), action TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('approval','ready','started','done','denied','cancelled','unknown')),
    reserved INTEGER NOT NULL DEFAULT 0, ok INTEGER, result TEXT
);
CREATE INDEX operation_history ON operations(task);
CREATE UNIQUE INDEX one_pending ON operations(task) WHERE state IN ('approval','ready','started');
CREATE INDEX live_tools ON operations(task) WHERE reserved=1;
CREATE VIEW occupied_agents AS
    SELECT agent FROM tasks WHERE model_reserved=1
    UNION ALL SELECT t.agent FROM tasks t JOIN operations o ON t.id=o.task WHERE o.reserved=1;
";

// Only roots own a policy. All descendants consult that same row at admission.
const MIGRATE_V2: &str = "
CREATE TABLE root_policies (
    root TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id),
    policy TEXT NOT NULL CHECK(policy IN ('auto','review'))
);
INSERT INTO root_policies(root,policy) SELECT id,'review' FROM tasks WHERE root=id;
";

const MIGRATE_V4: &str = "
CREATE TABLE model_overrides (
    provider TEXT PRIMARY KEY NOT NULL,
    model TEXT,
    effort TEXT
);
CREATE TABLE root_model_settings (
    root TEXT PRIMARY KEY NOT NULL REFERENCES tree_runs(root),
    model TEXT,
    effort TEXT
);
";

const MIGRATE_V6: &str = "
DROP INDEX IF EXISTS one_model_per_agent;
";

const MIGRATE_V8: &str = "
ALTER TABLE node_messages ADD COLUMN meta TEXT;
ALTER TABLE messages ADD COLUMN meta TEXT;
ALTER TABLE tasks ADD COLUMN last_input_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN last_history_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN total_input_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN total_cached_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN total_output_tokens INTEGER NOT NULL DEFAULT 0;
";

const MIGRATE_V9: &str = "
ALTER TABLE settings ADD COLUMN active_provider TEXT;
ALTER TABLE root_model_settings ADD COLUMN provider TEXT;
";

const MIGRATE_V10: &str = "
CREATE TABLE root_policies_v10 (
    root TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id),
    policy TEXT NOT NULL CHECK(policy IN ('auto','review')),
    sandbox TEXT NOT NULL DEFAULT 'direct' CHECK(sandbox IN ('direct','staged','read_only'))
);
INSERT INTO root_policies_v10(root,policy,sandbox) SELECT root,policy,sandbox FROM root_policies;
DROP TABLE root_policies;
ALTER TABLE root_policies_v10 RENAME TO root_policies;
DROP INDEX IF EXISTS one_pending;
ALTER TABLE operations ADD COLUMN batch_id TEXT;
ALTER TABLE operations ADD COLUMN seq_idx INTEGER NOT NULL DEFAULT 0;
ALTER TABLE operations ADD COLUMN call_id TEXT;
CREATE INDEX operation_batch ON operations(task, batch_id, seq_idx);
ALTER TABLE tree_runs ADD COLUMN plan_json TEXT;
CREATE TABLE steerings (
    id TEXT PRIMARY KEY NOT NULL,
    root TEXT NOT NULL REFERENCES tree_runs(root),
    task TEXT,
    text TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX steering_queue ON steerings(root);
";

fn unix_secs_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn model_settings_row(row: &Row<'_>) -> rusqlite::Result<ModelSettings> {
    Ok(ModelSettings { provider: None, model: row.get(0)?, effort: row.get(1)? })
}

fn root_model_settings_row(row: &Row<'_>) -> rusqlite::Result<ModelSettings> {
    Ok(ModelSettings { provider: row.get(0)?, model: row.get(1)?, effort: row.get(2)? })
}

fn root_model_settings(db: &Connection, root: &str) -> Result<Option<ModelSettings>> {
    db.query_row("SELECT provider,model,effort FROM root_model_settings WHERE root=?1", [root], root_model_settings_row)
        .optional()
        .db()
}

fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// Translate database errors at the boundary; application rejections remain ordinary strings.
trait Db<T> {
    fn db(self) -> Result<T>;
}
impl<T> Db<T> for rusqlite::Result<T> {
    fn db(self) -> Result<T> {
        self.map_err(error)
    }
}

fn id() -> String {
    Uuid::new_v4().to_string()
}

fn field<'a>(action: &'a Value, key: &str) -> Result<&'a str> {
    action[key].as_str().ok_or_else(|| format!("expected string field {key}"))
}

fn nonempty<'a>(text: &'a str, key: &str) -> Result<&'a str> {
    if text.trim().is_empty() {
        Err(format!("empty {key}"))
    } else {
        Ok(text)
    }
}

fn check_member(db: &Connection, agent: &str) -> Result<()> {
    let found: bool =
        db.query_row("SELECT EXISTS(SELECT 1 FROM agents WHERE name=?1 AND internal=0)", [agent], |r| r.get(0)).db()?;
    if found {
        Ok(())
    } else {
        Err(format!("unknown agent: {agent}"))
    }
}

fn members(db: &Connection) -> Result<Value> {
    let mut q = db.prepare("SELECT name FROM agents WHERE internal=0 ORDER BY rowid").db()?;
    let names = q.query_map([], |r| Ok(json!({"name":r.get::<_, String>(0)?}))).db()?;
    Ok(Value::Array(names.collect::<rusqlite::Result<_>>().db()?))
}

fn message_with_meta(db: &Connection, agent: &str, role: &str, content: &str, meta: Option<&Value>) -> Result<()> {
    let meta_json = meta.map(Value::to_string);
    db.execute(
        "INSERT INTO messages(agent,role,content,meta) VALUES (?1,?2,?3,?4)",
        params![agent, role, content, meta_json],
    )
    .db()?;
    Ok(())
}

fn message(db: &Connection, agent: &str, role: &str, content: &str) -> Result<()> {
    message_with_meta(db, agent, role, content, None)
}

fn node_message_with_meta(db: &Connection, task: &str, role: &str, content: &str, meta: Option<&Value>) -> Result<()> {
    let meta_json = meta.map(Value::to_string);
    db.execute(
        "INSERT INTO node_messages(task,role,content,meta) VALUES (?1,?2,?3,?4)",
        params![task, role, content, meta_json],
    )
    .db()?;
    Ok(())
}

fn node_message(db: &Connection, task: &str, role: &str, content: &str) -> Result<()> {
    node_message_with_meta(db, task, role, content, None)
}

fn history_msg(r: &Row<'_>) -> rusqlite::Result<Value> {
    let role: String = r.get(0)?;
    let content: String = r.get(1)?;
    let meta: Option<String> = r.get(2)?;
    let mut msg = json!({"role": role, "content": content});
    if let Some(raw) = meta {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            msg["meta"] = v;
        }
    }
    Ok(msg)
}

fn report(db: &Connection, task: &str, body: Value) -> Result<()> {
    if tree::work_root(db, task)?.is_some() {
        node_message(db, task, "user", &body.to_string())
    } else {
        db.execute(
            "INSERT INTO messages(agent,role,content) SELECT agent,'user',?2 FROM tasks WHERE id=?1",
            params![task, body.to_string()],
        )
        .db()?;
        Ok(())
    }
}

fn take_model(db: &Connection, task: &str) -> Result<String> {
    db.query_row(
        "UPDATE tasks SET model_reserved=0 WHERE id=?1 AND status='running' AND model_reserved=1 RETURNING agent",
        [task],
        |r| r.get(0),
    )
    .optional()
    .db()?
    .ok_or_else(|| "task does not have a running model turn".into())
}

fn queue(db: &Connection, task: &str, feedback: Value) -> Result<()> {
    report(db, task, feedback)?;
    db.execute("UPDATE tasks SET status='queued' WHERE id=?1", [task]).db()?;
    Ok(())
}

fn done(db: &Connection, task: &str, text: &str) -> Result<()> {
    let parent: Option<String> = db
        .query_row("UPDATE tasks SET status='done',result=?2 WHERE id=?1 RETURNING parent", params![task, text], |r| {
            r.get(0)
        })
        .db()?;
    if let Some(parent) = parent {
        queue(db, &parent, json!({"child":task,"status":"done","result":text}))?;
    }
    Ok(())
}

fn finish_check(db: &Connection, task: &str, ok: bool, text: &str) -> Result<()> {
    let attempt: u32 = db
        .query_row(
            "SELECT count(*) FROM operations WHERE task=?1 AND state='done' AND json_extract(action,'$.op')='verify'",
            [task],
            |r| r.get(0),
        )
        .db()?;
    let mut feedback =
        json!({"task":task,"tool":"verify","ok":ok,"result":text,"attempt":attempt,"max_attempts":MAX_CHECK_ATTEMPTS});
    if ok {
        report(db, task, feedback)?;
        let proposal: String = db.query_row("SELECT proposal FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
        return done(db, task, &proposal);
    }

    db.execute("UPDATE tasks SET proposal=NULL WHERE id=?1", [task]).db()?;
    if attempt < MAX_CHECK_ATTEMPTS {
        feedback["next"] = json!("Repair the failure and propose completion again to rerun the submitted check.");
        return queue(db, task, feedback);
    }
    report(db, task, feedback)?;
    stop_chain(db, task, "failed", &format!("verification failed (check attempt limit reached): {text}"))
}

// A failed, cancelled or uncertain prerequisite can never produce a successful ancestor.
fn stop_chain(db: &Connection, task: &str, mut status: &str, reason: &str) -> Result<()> {
    let mut current = Some(task.to_owned());
    let mut result = reason.to_owned();
    // Only the originating task receives private diagnostics. Parents get dependency state,
    // never raw child tool output, provider errors or other private conversation content.
    let summary = format!("Descendant task {task} ended with status {status}");
    while let Some(id) = current {
        current = db
            .query_row(
                "UPDATE tasks SET status=?2,result=?3 WHERE id=?1 RETURNING parent",
                params![id, status, result],
                |r| r.get(0),
            )
            .db()?;
        report(db, &id, json!({"task":id,"status":status,"result":result}))?;
        if current.is_some() {
            result.clone_from(&summary);
            if status != "unknown" {
                status = "failed";
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn operation_with_meta(
    db: &Connection,
    task: &str,
    action: &Value,
    state: &str,
    round: i64,
    batch_id: Option<&str>,
    seq_idx: usize,
    call_id: Option<&str>,
) -> Result<()> {
    db.execute(
        "INSERT INTO operations(id,task,action,state,round,batch_id,seq_idx,call_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![id(), task, action.to_string(), state, round, batch_id, seq_idx as i64, call_id],
    )
    .db()?;
    if batch_id.is_none() {
        let status = if state == "approval" { "approval" } else { "waiting" };
        db.execute("UPDATE tasks SET status=?2 WHERE id=?1", params![task, status]).db()?;
    }
    Ok(())
}

fn operation(db: &Connection, task: &str, action: &Value, state: &str, round: i64) -> Result<()> {
    operation_with_meta(db, task, action, state, round, None, 0, None)
}

fn schema_version(db: &Connection) -> Result<i64> {
    let app: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0)).db()?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0)).db()?;
    if app == APP_ID && matches!(version, 1..=10) {
        return Ok(version);
    }
    let tables: i64 = db.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0)).db()?;
    if app == 0 && version == 0 && tables == 0 {
        Ok(0)
    } else {
        Err("not a supported swarm database (foreign application or schema version)".into())
    }
}

impl Store {
    #[cfg(test)]
    pub(crate) fn conn(&self) -> &Connection {
        &self.db
    }
    pub fn open(path: &Path) -> Result<Self> {
        let mut db = Connection::open(path).db()?;
        db.busy_timeout(Duration::from_secs(5)).db()?;
        schema_version(&db)?; // Do not switch a foreign database into WAL mode.
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0)).db()?;
        if mode != "wal" {
            return Err(format!("WAL journal mode unavailable: {mode}"));
        }
        db.execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF;").db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate).db()?;
        let version = schema_version(&tx)?;
        if version == 0 {
            tx.execute_batch(SCHEMA_V1).db()?;
            tx.execute("INSERT INTO settings(singleton,max_agents,turn_budget) VALUES (1,1,1)", []).db()?;
            tx.execute("INSERT INTO agents(name) VALUES ('default')", []).db()?;
            tx.pragma_update(None, "application_id", APP_ID).db()?;
        }
        if version < 2 {
            tx.execute_batch(MIGRATE_V2).db()?;
            tx.pragma_update(None, "user_version", 2).db()?;
        }
        if version < 3 {
            tx.execute_batch(tree::MIGRATE_V3).db()?;
            tx.pragma_update(None, "user_version", 3).db()?;
        }
        if version < 4 {
            tx.execute_batch(MIGRATE_V4).db()?;
            tx.pragma_update(None, "user_version", 4).db()?;
        }
        if version < 5 {
            tx.execute_batch(tree::MIGRATE_V5).db()?;
            tx.pragma_update(None, "user_version", 5).db()?;
        }
        if version < 6 {
            tx.execute_batch(MIGRATE_V6).db()?;
            tx.pragma_update(None, "user_version", 6).db()?;
        }
        if version < 7 {
            tx.execute_batch(tree::MIGRATE_V7).db()?;
            tx.pragma_update(None, "user_version", 7).db()?;
        }
        if version < 8 {
            tx.execute_batch(MIGRATE_V8).db()?;
            tx.pragma_update(None, "user_version", 8).db()?;
        }
        if version < 9 {
            tx.execute_batch(MIGRATE_V9).db()?;
            tx.pragma_update(None, "user_version", 9).db()?;
        }
        if version < 10 {
            tx.execute_batch(MIGRATE_V10).db()?;
            tx.pragma_update(None, "user_version", 10).db()?;
        }
        tx.query_row("SELECT 1 FROM settings,agents WHERE singleton=1 AND name='default'", [], |_| Ok(())).db()?;
        // Reject missing schema in the same transaction as the migration.
        tx.prepare(&format!("{TASK_VIEW} LIMIT 0")).db()?;
        tx.prepare("SELECT active_provider FROM settings LIMIT 0").db()?;
        tx.prepare("SELECT role,content,meta FROM messages LIMIT 0").db()?;
        tx.prepare("SELECT role,content,meta FROM node_messages LIMIT 0").db()?;
        tx.prepare(
            "SELECT last_input_tokens,last_history_bytes,total_input_tokens,total_cached_tokens,total_output_tokens FROM tasks LIMIT 0",
        )
        .db()?;
        tx.prepare("SELECT provider,model,effort FROM model_overrides LIMIT 0").db()?;
        tx.prepare("SELECT root,provider,model,effort FROM root_model_settings LIMIT 0").db()?;
        tx.prepare("SELECT root,round,subtree_root FROM tree_rounds LIMIT 0").db()?;
        tx.prepare(
            "SELECT root,key,kind,content,author_agent,round,version,seq,status,updated_at FROM whiteboard_entries LIMIT 0",
        )
        .db()?;
        tx.prepare("SELECT batch_id,seq_idx,call_id FROM operations LIMIT 0").db()?;
        tx.prepare("SELECT plan_json FROM tree_runs LIMIT 0").db()?;
        tx.prepare("SELECT root,task,text FROM steerings LIMIT 0").db()?;
        tx.commit().db()?;
        Ok(Self { db })
    }

    fn transaction<T>(&mut self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate).db()?;
        let result = f(&tx)?;
        tx.commit().db()?;
        Ok(result)
    }

    pub fn bind_workspace(&mut self, workspace: &Path) -> Result<()> {
        let workspace = workspace.canonicalize().map_err(error)?;
        if !workspace.is_dir() {
            return Err("workspace is not a directory".into());
        }
        let bytes = workspace.as_os_str().as_encoded_bytes();
        self.transaction(|db| {
            let updated =
                db.execute("UPDATE settings SET workspace=?1 WHERE workspace IS NULL OR workspace=?1", [bytes]).db()?;
            if updated == 1 {
                Ok(())
            } else {
                Err("session belongs to another workspace".into())
            }
        })
    }

    pub fn active_provider(&self) -> Result<Option<String>> {
        self.db.query_row("SELECT active_provider FROM settings WHERE singleton=1", [], |r| r.get(0)).db()
    }

    pub fn set_active_provider(&mut self, provider: Option<&str>) -> Result<()> {
        self.transaction(|db| {
            db.execute("UPDATE settings SET active_provider=?1 WHERE singleton=1", [provider]).db()?;
            Ok(())
        })
    }

    /// With no daemon default this returns the provider's raw session overrides.
    pub fn model_settings(&self, provider: &str, default_model: Option<&str>) -> Result<ModelSettings> {
        let mut settings = self
            .db
            .query_row("SELECT model,effort FROM model_overrides WHERE provider=?1", [provider], model_settings_row)
            .optional()
            .db()?
            .unwrap_or_default();
        settings.provider = Some(provider.to_owned());
        if settings.model.is_none() {
            settings.model = default_model.map(str::to_owned);
        }
        Ok(settings)
    }

    pub fn set_model_settings(&mut self, provider: &str, settings: &ModelSettings) -> Result<()> {
        self.db
            .execute(
                "INSERT INTO model_overrides(provider,model,effort) VALUES (?1,?2,?3)
             ON CONFLICT(provider) DO UPDATE SET model=excluded.model,effort=excluded.effort",
                params![provider, settings.model, settings.effort],
            )
            .db()?;
        Ok(())
    }

    // Convenience API retained for callers that use the default admission policy.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn submit(&mut self, agent: &str, prompt: &str) -> Result<String> {
        self.submit_checked(agent, prompt, None)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn submit_checked(&mut self, agent: &str, prompt: &str, check: Option<&str>) -> Result<String> {
        self.submit_with_policy(agent, prompt, check, false)
    }

    pub fn submit_with_policy(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
    ) -> Result<String> {
        self.submit_with_model(agent, prompt, check, review, &ModelSettings::default())
    }

    pub fn submit_with_model(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        settings: &ModelSettings,
    ) -> Result<String> {
        self.submit_with_options(agent, prompt, check, review, false, settings)
    }

    pub fn submit_with_options(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        staged: bool,
        settings: &ModelSettings,
    ) -> Result<String> {
        let sandbox = if staged { SandboxMode::Staged } else { SandboxMode::Direct };
        self.submit_with_full_options(agent, prompt, check, review, sandbox, settings, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_with_full_options(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        sandbox: SandboxMode,
        settings: &ModelSettings,
        context_prefix: Option<&str>,
    ) -> Result<String> {
        self.create_root(agent, prompt, check, review, sandbox, Some(settings), context_prefix)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn continue_tree(
        &mut self,
        target_id: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        settings: &ModelSettings,
    ) -> Result<(String, String)> {
        self.continue_tree_with_options(target_id, prompt, check, review, false, settings)
    }

    pub fn continue_tree_with_options(
        &mut self,
        target_id: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        staged: bool,
        settings: &ModelSettings,
    ) -> Result<(String, String)> {
        let sandbox = if staged { SandboxMode::Staged } else { SandboxMode::Direct };
        self.continue_tree_with_full_options(target_id, prompt, check, review, sandbox, settings, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn continue_tree_with_full_options(
        &mut self,
        target_id: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        sandbox: SandboxMode,
        settings: &ModelSettings,
        context_prefix: Option<&str>,
    ) -> Result<(String, String)> {
        nonempty(prompt, "prompt")?;
        if let Some(c) = check {
            nonempty(c, "check command")?;
        }
        self.transaction(|db| {
            let root: String = db
                .query_row("SELECT root FROM tasks WHERE id=?1", [target_id], |r| r.get(0))
                .optional()
                .db()?
                .ok_or_else(|| format!("unknown task: {target_id}"))?;
            let row: Option<(String, i64, Option<String>)> = db
                .query_row("SELECT phase,round,continuation_node FROM tree_runs WHERE root=?1", [&root], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()
                .db()?;
            let Some((phase, round, continuation_node)) = row else {
                return Err("legacy task cannot be continued as a tree".into());
            };
            if phase != "complete" {
                return Err("session has active work; wait for it to settle or start a new session".into());
            }
            let Some(node_id) = continuation_node else {
                return Err("session has no single leaf or summarizer node to continue".into());
            };
            if target_id != root && target_id != node_id {
                return Err("only the sole leaf or summarizer node can be continued".into());
            }
            let blocked: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM tasks WHERE parent=?1)
                        OR EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND model_reserved=1)
                        OR EXISTS(SELECT 1 FROM operations WHERE task=?1 AND reserved=1)",
                    [&node_id],
                    |r| r.get(0),
                )
                .db()?;
            if blocked {
                return Err("selected continuation node cannot be expanded".into());
            }
            let tail_op: Option<(String, String)> = db
                .query_row(
                    "SELECT role,content FROM node_messages WHERE task=?1 ORDER BY rowid DESC LIMIT 1",
                    [&node_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .db()?;
            if let Some((role, content)) = tail_op {
                if role == "assistant"
                    && serde_json::from_str::<Value>(&content)
                        .ok()
                        .and_then(|v| v["op"].as_str().map(str::to_owned))
                        .as_deref()
                        == Some("finish")
                {
                    node_message(db, &node_id, "user", r#"{"finished":true}"#)?;
                }
            }
            let next_round = round + 1;
            db.execute(
                "UPDATE tree_runs SET round=?2,active_root=?3,continuation_node=NULL,reporter=?4,
                 phase='work',outcome=NULL,directory_used=0,result=NULL,question=NULL,round_turns_used=0
                 WHERE root=?1",
                params![root, next_round, node_id, id()],
            )
            .db()?;
            db.execute(
                "UPDATE root_policies SET policy=?2,sandbox=?3 WHERE root=?1",
                params![root, if review { "review" } else { "auto" }, sandbox.as_str()],
            )
            .db()?;
            db.execute(
                "INSERT INTO root_model_settings(root,provider,model,effort) VALUES (?1,?2,?3,?4)
                 ON CONFLICT(root) DO UPDATE SET provider=excluded.provider,model=excluded.model,effort=excluded.effort",
                params![root, settings.provider, settings.model, settings.effort],
            )
            .db()?;
            db.execute(
                "UPDATE tasks SET round=?2,prompt=?3,check_command=?4,handoff_context=NULL,
                 status='queued',introduced=0,proposal=NULL,result=NULL,wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1",
                params![node_id, next_round, prompt, check],
            )
            .db()?;
            if let Some(cp) = context_prefix {
                let has_msgs: bool = db
                    .query_row("SELECT EXISTS(SELECT 1 FROM node_messages WHERE task=?1)", [&node_id], |r| r.get(0))
                    .db()?;
                if !has_msgs {
                    node_message(db, &node_id, "user", cp)?;
                }
            }
            db.execute(
                "INSERT INTO tree_rounds(root,round,subtree_root,prompt,check_command) VALUES (?1,?2,?3,?4,?5)",
                params![root, next_round, node_id, prompt, check],
            )
            .db()?;
            Ok((root, node_id))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn create_root(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        sandbox: SandboxMode,
        tree_settings: Option<&ModelSettings>,
        context_prefix: Option<&str>,
    ) -> Result<String> {
        nonempty(prompt, "prompt")?;
        if let Some(c) = check {
            nonempty(c, "check command")?;
        }
        self.transaction(|db| {
            check_member(db, agent)?;
            let task = id();
            db.execute(
                "INSERT INTO tasks(id,root,agent,prompt,handoff_context,status,check_command) VALUES (?1,?1,?2,?3,?4,'queued',?5)",
                params![task, agent, prompt, context_prefix, check],
            )
            .db()?;
            db.execute(
                "INSERT INTO root_policies(root,policy,sandbox) VALUES (?1,?2,?3)",
                params![task, if review { "review" } else { "auto" }, sandbox.as_str()],
            )
            .db()?;
            if let Some(settings) = tree_settings {
                db.execute(
                    "INSERT INTO tree_runs(root,reporter,round,active_root,round_turns_used) VALUES (?1,?2,1,?1,0)",
                    params![task, id()],
                )
                .db()?;
                db.execute(
                    "INSERT INTO tree_rounds(root,round,subtree_root,prompt,check_command) VALUES (?1,1,?1,?2,?3)",
                    params![task, prompt, check],
                )
                .db()?;
                db.execute(
                    "INSERT INTO root_model_settings(root,provider,model,effort) VALUES (?1,?2,?3,?4)",
                    params![task, settings.provider, settings.model, settings.effort],
                )
                .db()?;
            }
            Ok(task)
        })
    }

    pub fn agents(&mut self) -> Result<Value> {
        members(&self.db)
    }

    pub fn tasks(&mut self) -> Result<Value> {
        let mut q = self.db.prepare(&format!("{TASK_VIEW} ORDER BY t.rowid")).db()?;
        let rows = q.query_map([], task_view).db()?;
        Ok(Value::Array(rows.collect::<rusqlite::Result<_>>().db()?))
    }

    pub fn task(&mut self, id: &str) -> Result<Value> {
        let tx = self.db.transaction().db()?;
        let mut task = tx
            .query_row(&format!("{TASK_VIEW} WHERE t.id=?1"), [id], task_view)
            .optional()
            .db()?
            .ok_or_else(|| format!("unknown task: {id}"))?;
        let verify_task = task["active_root"].as_str().unwrap_or(id).to_owned();
        let verify_round = task["round"].as_i64().unwrap_or(1);
        let mut q = tx
            .prepare(
                "SELECT action,ok,result FROM operations
                 WHERE state='done' AND (task=?1 OR (task=?2 AND round=?3 AND json_extract(action,'$.op')='verify'))
                 ORDER BY rowid",
            )
            .db()?;
        let evidence = q
            .query_map(params![id, verify_task, verify_round], |r| {
                Ok(json!({"action":action_at(r,0)?,"ok":r.get::<_,bool>(1)?,"result":r.get::<_,String>(2)?}))
            })
            .db()?;
        let evidence = evidence.collect::<rusqlite::Result<Vec<_>>>().db()?;
        drop(q);
        let mut round_checks = Vec::new();
        let mut cq_v = tx
            .prepare(
                "SELECT ok FROM operations
                 WHERE task=?1 AND round=?2 AND state='done' AND json_extract(action,'$.op')='verify'
                 ORDER BY rowid",
            )
            .db()?;
        let rows_v = cq_v.query_map(params![verify_task, verify_round], |r| r.get::<_, bool>(0)).db()?;
        for row in rows_v {
            round_checks.push(row.db()?);
        }
        drop(cq_v);
        let interrupted = task["tool_state"] == "unknown" && task["action"]["op"] == "verify";
        let attempted = interrupted || !round_checks.is_empty();
        let passed = round_checks.into_iter().any(|ok| ok);
        let terminal = matches!(task["status"].as_str(), Some("failed" | "cancelled" | "unknown"));
        let verification = if passed {
            "passed"
        } else if task["check"].is_null() {
            "not_requested"
        } else if interrupted {
            "unknown"
        } else if terminal && !attempted {
            "not_run"
        } else if attempted && terminal {
            "failed"
        } else {
            "pending"
        };
        task["verification"] = json!(verification);
        if !passed {
            task["verification_note"] = json!(if interrupted {
                "No independent verification of completion: the submitted check started, but its outcome is unknown."
            } else if terminal && !attempted && !task["check"].is_null() {
                "No independent verification of completion: the task ended before the submitted check ran."
            } else {
                "No independent verification of completion."
            });
        }
        task["evidence"] = Value::Array(evidence);
        if task["root"] == id {
            if task["kind"] == "root" {
                let outputs = tree::outputs(&tx, id)?;
                task["tree_nodes"] = outputs.nodes;
                task["leaf_results"] = outputs.leaves;
                let mut cq = tx
                    .prepare(
                        "SELECT c.question,c.answer FROM clarifications c JOIN tree_runs g ON g.root=c.root
                         WHERE c.root=?1 AND c.round=g.round ORDER BY c.rowid",
                    )
                    .db()?;
                let clarifications = cq
                    .query_map([id], |r| Ok(json!({"question":r.get::<_,String>(0)?,"answer":r.get::<_,String>(1)?})))
                    .db()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .db()?;
                task["clarifications"] = Value::Array(clarifications);
                let mut rq = tx
                    .prepare(
                        "SELECT round,subtree_root,prompt,check_command,outcome,verification,result,continuation_node
                         FROM tree_rounds WHERE root=?1 ORDER BY round",
                    )
                    .db()?;
                let rounds = rq
                    .query_map([id], |r| {
                        Ok(json!({
                            "round": r.get::<_, u32>(0)?,
                            "subtree_root": r.get::<_, String>(1)?,
                            "prompt": r.get::<_, String>(2)?,
                            "check": r.get::<_, Option<String>>(3)?,
                            "outcome": r.get::<_, Option<String>>(4)?,
                            "verification": r.get::<_, Option<String>>(5)?,
                            "result": r.get::<_, Option<String>>(6)?,
                            "continuation_node": r.get::<_, Option<String>>(7)?
                        }))
                    })
                    .db()?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .db()?;
                task["rounds"] = Value::Array(rounds);
                let (wb_seq, wb_entries) = tree::whiteboard_entries(&tx, id, None, None, None)?;
                task["whiteboard_seq"] = json!(wb_seq);
                task["whiteboard"] = Value::Array(wb_entries);
            }
            if task["kind"] != "legacy" {
                if let Some(root_id) = task["root"].as_str().map(str::to_owned) {
                    let plan_raw: Option<String> = tx
                        .query_row("SELECT plan_json FROM tree_runs WHERE root=?1", [&root_id], |r| r.get(0))
                        .optional()
                        .db()?
                        .flatten();
                    if let Some(raw) = plan_raw {
                        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                            task["plan"] = v;
                        }
                    }
                    let steer_cnt: i64 =
                        tx.query_row("SELECT count(*) FROM steerings WHERE root=?1", [&root_id], |r| r.get(0)).db()?;
                    if steer_cnt > 0 {
                        task["pending_steerings"] = json!(steer_cnt);
                    }
                }
            }
            // User status only. Omit tool results and edit bodies, and bound even a single-line command.
            let mut q = tx
                .prepare(
                    "SELECT t.id,t.agent,json_extract(o.action,'$.op'),
                COALESCE(substr(CASE json_extract(o.action,'$.op')
                    WHEN 'shell' THEN json_extract(o.action,'$.command')
                    WHEN 'verify' THEN json_extract(o.action,'$.command')
                    WHEN 'apply_patch' THEN 'patch'
                    WHEN 'grep_files' THEN json_extract(o.action,'$.pattern')
                    WHEN 'list_dir' THEN COALESCE(json_extract(o.action,'$.path'), '.')
                    WHEN 'read_skill' THEN json_extract(o.action,'$.name')
                    WHEN 'mcp_list' THEN COALESCE(json_extract(o.action,'$.server'), 'all')
                    WHEN 'mcp_call' THEN COALESCE(json_extract(o.action,'$.server'),'') || ':' || COALESCE(json_extract(o.action,'$.tool'),'')
                    WHEN 'update_plan' THEN 'plan'
                    ELSE json_extract(o.action,'$.path') END, 1, 120), ''),o.ok
                FROM tasks t JOIN operations o ON o.task=t.id WHERE t.root=?1 AND o.state='done' ORDER BY o.rowid",
                )
                .db()?;
            let receipts = q
                .query_map([id], |r| {
                    Ok(json!({"task":r.get::<_,String>(0)?,"agent":r.get::<_,String>(1)?,
                "op":r.get::<_,String>(2)?,"label":r.get::<_,String>(3)?,"ok":r.get::<_,bool>(4)?}))
                })
                .db()?;
            task["subtree_receipts"] = Value::Array(receipts.collect::<rusqlite::Result<_>>().db()?);
        }
        let is_tree = task["kind"] != "legacy";
        let is_root = task["root"].as_str() == Some(id);
        let (inp, cached, out, last_in): (u64, u64, u64, u64) = if is_tree && is_root {
            let (inp, cached, out) = tx
                .query_row(
                    "SELECT COALESCE(SUM(total_input_tokens),0), COALESCE(SUM(total_cached_tokens),0), COALESCE(SUM(total_output_tokens),0)
                     FROM tasks WHERE root=?1",
                    [id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)),
                )
                .db()?;
            let last_in: i64 =
                tx.query_row("SELECT last_input_tokens FROM tasks WHERE id=?1", [id], |r| r.get(0)).db()?;
            (inp.max(0) as u64, cached.max(0) as u64, out.max(0) as u64, last_in.max(0) as u64)
        } else {
            let (inp, cached, out, last_in) = tx
                .query_row(
                    "SELECT total_input_tokens, total_cached_tokens, total_output_tokens, last_input_tokens
                     FROM tasks WHERE id=?1",
                    [id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)),
                )
                .db()?;
            (inp.max(0) as u64, cached.max(0) as u64, out.max(0) as u64, last_in.max(0) as u64)
        };
        let hit_rate = if inp > 0 { cached as f64 / inp as f64 } else { 0.0 };
        task["usage"] = json!({
            "input_tokens": inp,
            "cached_tokens": cached,
            "output_tokens": out,
            "cache_hit_rate": hit_rate,
            "last_input_tokens": last_in
        });
        tx.commit().db()?;
        Ok(task)
    }

    pub fn claim(&mut self) -> Result<Option<Turn>> {
        self.transaction(|db| {
            if let Some(reporter) = tree::claim_report(db)? {
                return Ok(Some(reporter));
            }
            let next = db
                .query_row(
                    "SELECT id,root,agent,prompt,introduced,round,last_input_tokens,last_history_bytes,
                     EXISTS(SELECT 1 FROM tree_runs WHERE tree_runs.root=tasks.root)
                     FROM tasks WHERE status='queued' AND model_reserved=0 ORDER BY rowid LIMIT 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, bool>(4)?,
                            r.get::<_, i64>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, i64>(7)?,
                            r.get::<_, bool>(8)?,
                        ))
                    },
                )
                .optional()
                .db()?;
            let Some((task, root, agent, prompt, introduced, round, last_in, last_bytes, is_tree)) = next else {
                return Ok(None);
            };
            let active_root = if is_tree {
                let active_root: String = db
                    .query_row("SELECT COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [&root], |r| r.get(0))
                    .db()?;
                db.execute("UPDATE tree_runs SET round_turns_used=round_turns_used+1 WHERE root=?1", [&root]).db()?;
                db.execute(
                    "UPDATE tasks SET turns_used=(SELECT round_turns_used FROM tree_runs WHERE root=?1) WHERE id=?1",
                    [&root],
                )
                .db()?;
                active_root
            } else {
                db.execute("UPDATE tasks SET turns_used=turns_used+1 WHERE id=?1", [&root]).db()?;
                root.clone()
            };
            let _ = round;
            db.execute("UPDATE tasks SET status='running',introduced=1,model_reserved=1 WHERE id=?1", [&task]).db()?;
            if !introduced {
                let context: Option<String> =
                    db.query_row("SELECT handoff_context FROM tasks WHERE id=?1", [&task], |r| r.get(0)).db()?;
                if is_tree {
                    let has_prefix: bool = db
                        .query_row("SELECT EXISTS(SELECT 1 FROM node_messages WHERE task=?1)", [&task], |r| r.get(0))
                        .db()?;
                    if has_prefix {
                        let is_forked_child: bool = db
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM tasks t JOIN tasks p ON p.id=t.parent WHERE t.id=?1 AND t.round=p.round)",
                                [&task],
                                |r| r.get(0),
                            )
                            .db()?;
                        let intro = if is_forked_child {
                            format!(
                                "Agent: {agent}\nTask: {task}\n\nYou are child stage @{agent} spawned by the handoff above. Do not repeat completed parent work; focus strictly on your assigned scope (ask is unavailable on child stages).\n\n{prompt}"
                            )
                        } else {
                            format!("Agent: {agent}\nTask: {task}\n\n{prompt}")
                        };
                        node_message(db, &task, "user", &intro)?;
                    } else {
                        if let Some(text) = context {
                            node_message(db, &task, "user", &text)?;
                        }
                        node_message(db, &task, "user", &format!("Agent: {agent}\nTask: {task}\n\n{prompt}"))?;
                    }
                } else {
                    if let Some(text) = context {
                        message(db, &agent, "user", &text)?;
                    }
                    message(db, &agent, "user", &prompt)?;
                }
            }
            if is_tree {
                let queued_steers: Vec<String> = {
                    let mut sq = db
                        .prepare(
                            "SELECT text FROM steerings WHERE root=?1 AND (task IS NULL OR task=?2) ORDER BY rowid",
                        )
                        .db()?;
                    let rows = sq.query_map(params![root, task], |r| r.get::<_, String>(0)).db()?;
                    rows.collect::<rusqlite::Result<_>>().db()?
                };
                if !queued_steers.is_empty() {
                    db.execute("DELETE FROM steerings WHERE root=?1 AND (task IS NULL OR task=?2)", params![root, task])
                        .db()?;
                    node_message(db, &task, "user", &json!({"steering":true,"messages":queued_steers}).to_string())?;
                }
            }
            let mut messages: Vec<Value> = if is_tree {
                let mut q = db.prepare("SELECT role,content,meta FROM node_messages WHERE task=?1 ORDER BY rowid").db()?;
                let rows = q.query_map([&task], history_msg).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            } else {
                let mut q = db.prepare("SELECT role,content,meta FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
                let rows = q.query_map([&agent], history_msg).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            };
            let can_compact = compute_compaction_slice(&messages).is_some();
            if introduced || is_tree {
                let mut parts = Vec::new();
                if introduced {
                    let is_protocol_retry = messages.last().and_then(|m| m["content"].as_str()).is_some_and(|s| {
                        serde_json::from_str::<Value>(s).ok().is_some_and(|v| {
                            v["task"].as_str() == Some(task.as_str()) && v["protocol_error"].is_string()
                        })
                    });
                    if !is_tree {
                        let focus = if is_protocol_retry {
                            format!(
                                "Continue task {task}. Original request: {prompt}\nYour previous turn was rejected (see protocol_error above). Invoke one allowed tool with valid arguments (or, if tool calling is unavailable, respond with exactly one valid raw JSON action object and stop at '}}')."
                            )
                        } else {
                            format!("Continue task {task}. Original request: {prompt}")
                        };
                        parts.push(focus);
                    } else if is_protocol_retry {
                        parts.push(format!(
                            "Continue task {task}. Your previous turn was rejected (see protocol_error above). Invoke one allowed tool with valid arguments (or, if tool calling is unavailable, respond with exactly one valid raw JSON action object and stop at '}}')."
                        ));
                    } else if let Some(nudge) = repetition_nudge(&messages) {
                        parts.push(nudge.to_owned());
                    }
                }
                if is_tree {
                    let seen_seq: i64 = db
                        .query_row("SELECT wb_seen_seq FROM tasks WHERE id=?1", [&task], |r| r.get(0))
                        .db()?;
                    if let Some((_wb_seq, digest)) = tree::whiteboard_digest(db, &root, seen_seq)? {
                        parts.push(digest);
                    }
                }
                if !parts.is_empty() {
                    // Inject transient protocol retry hint, repetition nudge, or delta whiteboard digest without mutating persisted history.
                    messages.push(json!({"role":"user","content":parts.join("\n\n")}));
                }
            }
            let kind = if !is_tree {
                TurnKind::Legacy
            } else if task == active_root {
                TurnKind::Root
            } else {
                TurnKind::Node
            };
            let settings = root_model_settings(db, &root)?;
            Ok(Some(Turn {
                kind,
                root,
                task,
                agent,
                messages,
                can_compact,
                settings,
                last_input_tokens: last_in.max(0) as u64,
                last_history_bytes: last_bytes.max(0) as usize,
            }))
        })
    }

    pub fn record_usage(&mut self, task: &str, usage: &UsageStats, history_bytes: usize) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                db.execute(
                    "UPDATE tasks SET total_input_tokens=total_input_tokens+?2,
                     total_cached_tokens=total_cached_tokens+?3,
                     total_output_tokens=total_output_tokens+?4 WHERE id=?1",
                    params![root, usage.input_tokens as i64, usage.cached_tokens as i64, usage.output_tokens as i64],
                )
                .db()?;
            } else {
                db.execute(
                    "UPDATE tasks SET last_input_tokens=?2, last_history_bytes=?3,
                     total_input_tokens=total_input_tokens+?2,
                     total_cached_tokens=total_cached_tokens+?4,
                     total_output_tokens=total_output_tokens+?5 WHERE id=?1",
                    params![
                        task,
                        usage.input_tokens as i64,
                        history_bytes as i64,
                        usage.cached_tokens as i64,
                        usage.output_tokens as i64
                    ],
                )
                .db()?;
            }
            Ok(())
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn apply(&mut self, task: &str, action: &Value) -> Result<()> {
        self.apply_with_meta(task, action, None)
    }

    pub fn apply_with_meta(&mut self, task: &str, action: &Value, meta: Option<&Value>) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                return tree::finish_report(db, &root, action, meta);
            }
            let agent = take_model(db, task)?;
            if let Some(root) = tree::work_root(db, task)? {
                return tree::apply(db, &root, task, action, meta);
            }
            message_with_meta(db, &agent, "assistant", &action.to_string(), meta)?;
            match field(action, "op")? {
                "finish" => {
                    let text = nonempty(field(action, "text")?, "finish text")?;
                    let check: Option<String> =
                        db.query_row("SELECT check_command FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
                    if let Some(command) = check {
                        db.execute("UPDATE tasks SET proposal=?2 WHERE id=?1", params![task, text]).db()?;
                        operation(db, task, &json!({"op":"verify","command":command}), "ready", 1)
                    } else {
                        done(db, task, text)
                    }
                }
                "members" => queue(db,task,json!({"members":members(db)?})),
                "spawn" => {
                    let name = nonempty(field(action,"agent")?,"agent name")?;
                    db.execute("INSERT OR IGNORE INTO agents(name) VALUES (?1)", [name]).db()?;
                    check_member(db, name)?;
                    queue(db,task,json!({"spawned":name}))
                }
                "delegate" => {
                    let target = field(action,"agent")?;
                    let text = nonempty(field(action,"text")?,"delegation text")?;
                    check_member(db,target)?;
                    let cycle: bool = db.query_row("WITH RECURSIVE ancestors(id,parent,agent) AS (
                        SELECT id,parent,agent FROM tasks WHERE id=?1 UNION ALL
                        SELECT t.id,t.parent,t.agent FROM tasks t JOIN ancestors a ON t.id=a.parent)
                        SELECT EXISTS (SELECT 1 FROM ancestors WHERE agent=?2)",params![task,target],|r|r.get(0)).db()?;
                    if cycle { return Err("delegation would wait on an ancestor agent".into()); }
                    db.execute("INSERT INTO tasks(id,root,parent,agent,prompt,status) SELECT ?2,root,id,?3,?4,'queued' FROM tasks WHERE id=?1",
                        params![task,id(),target,text]).db()?;
                    db.execute("UPDATE tasks SET status='waiting' WHERE id=?1",[task]).db()?;
                    Ok(())
                }
                "send" => {
                    let target = field(action,"agent")?;
                    let text = nonempty(field(action,"text")?,"message text")?;
                    check_member(db,target)?;
                    message(db,target,"user",&json!({"from":agent,"message":text}).to_string())?;
                    queue(db,task,json!({"delivered_to":target}))
                }
                other => Err(format!("unknown model action: {other}")),
            }
        })
    }

    pub fn fail(&mut self, task: &str, error: &str) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                return tree::fallback(db, &root, "reporter could not complete");
            }
            take_model(db, task)?;
            stop_work(db, task, "failed", error)
        })
    }

    pub fn fail_quota(&mut self, task: &str, error: &str) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                return tree::fallback(
                    db,
                    &root,
                    "provider quota limit reached; pause requests and resume with continue",
                );
            }
            take_model(db, task)?;
            stop_work(db, task, "failed", error)
        })
    }

    pub fn reject_action(&mut self, task: &str, reason: &str) -> Result<()> {
        const MAX_ACTION_REJECTIONS: u32 = 5;
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                return tree::fallback(db, &root, "reporter could not complete");
            }
            let agent = take_model(db, task)?;
            let is_tree = tree::work_root(db, task)?.is_some();
            let (cur_round, prior): (i64, u32) = {
                let cur_round = if is_tree {
                    db.query_row(
                        "SELECT g.round FROM tasks t JOIN tree_runs g ON g.root=t.root WHERE t.id=?1",
                        [task],
                        |r| r.get(0),
                    )
                    .db()?
                } else {
                    1
                };
                let (sql, param1): (&str, &str) = if is_tree {
                    ("SELECT role,content FROM node_messages WHERE task=?1 ORDER BY rowid DESC", task)
                } else {
                    ("SELECT role,content FROM messages WHERE agent=?1 ORDER BY rowid DESC", &agent)
                };
                let mut q = db.prepare(sql).db()?;
                let mut rows = q.query([param1]).db()?;
                let mut streak = 0u32;
                while let Some(row) = rows.next().db()? {
                    let role: String = row.get(0).db()?;
                    let content: String = row.get(1).db()?;
                    if role != "user" {
                        break;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(&content) else {
                        break;
                    };
                    let round_ok = !is_tree || v["round"].as_i64().unwrap_or(1) == cur_round;
                    if v["task"].as_str() == Some(task) && v["protocol_error"].is_string() && round_ok {
                        streak += 1;
                    } else {
                        break;
                    }
                }
                (cur_round, streak)
            };
            if prior < MAX_ACTION_REJECTIONS {
                return queue(
                    db,
                    task,
                    json!({
                        "task": task,
                        "round": cur_round,
                        "protocol_error": reason,
                        "attempt": prior + 1,
                        "max_corrections": MAX_ACTION_REJECTIONS,
                        "next": "Invoke exactly one allowed tool with valid arguments (or return one raw JSON object with an allowed op and required fields without extra text)."
                    }),
                );
            }
            stop_work(db, task, "failed", reason)
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn request_tool(&mut self, task: &str, action: &Value) -> Result<()> {
        self.request_tool_with_meta(task, action, None)
    }

    pub fn request_tool_with_meta(&mut self, task: &str, action: &Value, meta: Option<&Value>) -> Result<()> {
        let output = ModelOutput::single(action.clone(), meta.cloned(), None);
        self.apply_model_output(task, &output)
    }

    pub fn apply_model_output(&mut self, task: &str, output: &ModelOutput) -> Result<()> {
        if output.actions.is_empty() {
            return Err("expected at least one model action".into());
        }
        if output.actions.len() == 1 {
            let ma = &output.actions[0];
            let op = field(&ma.action, "op")?;
            if op == "update_plan" {
                let plan_val = validate_update_plan(&ma.action)?;
                let plan_str = plan_val.to_string();
                return self.transaction(|db| {
                    let agent = take_model(db, task)?;
                    if let Some(root) = tree::work_root(db, task)? {
                        db.execute("UPDATE tree_runs SET plan_json=?2 WHERE root=?1", params![root, plan_str]).db()?;
                        node_message_with_meta(db, task, "assistant", &ma.action.to_string(), output.meta.as_ref())?;
                    } else {
                        message_with_meta(db, &agent, "assistant", &ma.action.to_string(), output.meta.as_ref())?;
                    }
                    queue(db, task, json!({"task":task,"tool":"update_plan","ok":true,"result":"Plan updated"}))
                });
            }
            if is_local_tool_op(op) {
                validate_local_tool(&ma.action)?;
                let ro = is_read_only_local_tool(op);
                return self.transaction(|db| {
                    let agent = take_model(db, task)?;
                    let (auto, round): (bool, i64) = db
                        .query_row(
                            "SELECT p.policy='auto', t.round FROM tasks t JOIN root_policies p ON p.root=t.root WHERE t.id=?1",
                            [task],
                            |r| Ok((r.get(0)?, r.get(1)?)),
                        )
                        .db()?;
                    if tree::work_root(db, task)?.is_some() {
                        node_message_with_meta(db, task, "assistant", &ma.action.to_string(), output.meta.as_ref())?;
                    } else {
                        message_with_meta(db, &agent, "assistant", &ma.action.to_string(), output.meta.as_ref())?;
                    }
                    let state = if auto || ro { "ready" } else { "approval" };
                    operation_with_meta(db, task, &ma.action, state, round, None, 0, ma.call_id.as_deref())
                });
            }
            return self.apply_with_meta(task, &ma.action, output.meta.as_ref());
        }

        // Batch of multiple parallel tool calls (`TurnKind::Root | TurnKind::Node`).
        let mut validated_plans: Vec<Option<String>> = Vec::with_capacity(output.actions.len());
        let mut raw_actions = Vec::with_capacity(output.actions.len());
        for ma in &output.actions {
            let op = field(&ma.action, "op")?;
            if op == "update_plan" {
                validated_plans.push(Some(validate_update_plan(&ma.action)?.to_string()));
            } else if is_batch_tool_op(op) {
                if is_local_tool_op(op) {
                    validate_local_tool(&ma.action)?;
                }
                validated_plans.push(None);
            } else {
                return Err(format!("action {op} cannot be batched with other tools"));
            }
            raw_actions.push(ma.action.clone());
        }
        let assistant_batch = json!({"batch": raw_actions}).to_string();
        self.transaction(|db| {
            let _agent = take_model(db, task)?;
            let Some(root) = tree::work_root(db, task)? else {
                return Err("legacy tasks do not support parallel tool batches".into());
            };
            let (auto, round): (bool, i64) = db
                .query_row(
                    "SELECT p.policy='auto', t.round FROM tasks t JOIN root_policies p ON p.root=t.root WHERE t.id=?1",
                    [task],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .db()?;
            node_message_with_meta(db, task, "assistant", &assistant_batch, output.meta.as_ref())?;
            let batch_id = id();
            for (idx, (ma, plan_opt)) in output.actions.iter().zip(validated_plans.iter()).enumerate() {
                if let Some(plan_str) = plan_opt {
                    db.execute("UPDATE tree_runs SET plan_json=?2 WHERE root=?1", params![root, plan_str]).db()?;
                    db.execute(
                        "INSERT INTO operations(id,task,action,state,reserved,ok,result,round,batch_id,seq_idx,call_id)
                         VALUES (?1,?2,?3,'done',0,1,'Plan updated',?4,?5,?6,?7)",
                        params![id(), task, ma.action.to_string(), round, batch_id, idx as i64, ma.call_id.as_deref()],
                    )
                    .db()?;
                } else {
                    let op = ma.action["op"].as_str().unwrap_or("");
                    let state =
                        if auto || is_batch_read_only_op(op) || op == "wb_write" { "ready" } else { "approval" };
                    operation_with_meta(
                        db,
                        task,
                        &ma.action,
                        state,
                        round,
                        Some(&batch_id),
                        idx,
                        ma.call_id.as_deref(),
                    )?;
                }
            }
            refresh_batch_or_queue(db, task, &batch_id)
        })
    }

    pub fn approve(&mut self, approval_id: &str, allow: bool) -> Result<()> {
        self.transaction(|db| {
            let state = if allow { "ready" } else { "denied" };
            let (task, batch_id): (String, Option<String>) = db
                .query_row(
                    "UPDATE operations SET state=?2 WHERE id=?1 AND state='approval' RETURNING task,batch_id",
                    params![approval_id, state],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .db()?
                .ok_or("no such pending approval")?;
            if allow {
                let more: bool = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM operations WHERE task=?1 AND state='approval')",
                        [&task],
                        |r| r.get(0),
                    )
                    .db()?;
                let next_status = if more { "approval" } else { "waiting" };
                db.execute("UPDATE tasks SET status=?2 WHERE id=?1", params![task, next_status]).db()?;
                Ok(())
            } else if let Some(bid) = batch_id {
                db.execute(
                    "UPDATE operations SET state='denied', result=COALESCE(result,'tool denied by user')
                     WHERE task=?1 AND batch_id=?2 AND state IN ('approval','ready')",
                    params![task, bid],
                )
                .db()?;
                refresh_batch_or_queue(db, &task, &bid)
            } else {
                queue(db, &task, json!({"tool_denied":true}))
            }
        })
    }

    pub fn claim_tool_job(&mut self) -> Result<Option<ToolJob>> {
        self.transaction(|db| {
            let claimed = db
                .query_row(
                    "UPDATE operations SET state='started',reserved=1 WHERE rowid=(
                         SELECT o.rowid FROM operations o
                         WHERE o.state='ready' AND o.reserved=0
                           AND NOT EXISTS (
                               SELECT 1 FROM operations p
                               WHERE p.task=o.task AND o.batch_id IS NOT NULL AND p.batch_id=o.batch_id
                                 AND p.seq_idx < o.seq_idx AND p.state='approval'
                           )
                           AND NOT EXISTS (
                               SELECT 1 FROM operations s
                               WHERE s.task=o.task AND s.state='started'
                                 AND COALESCE(json_extract(s.action,'$.op'),'') NOT IN ('read_file','list_dir','grep_files','read_skill','mcp_list','wb_read')
                           )
                           AND (
                               COALESCE(json_extract(o.action,'$.op'),'') IN ('read_file','list_dir','grep_files','read_skill','mcp_list','wb_read')
                               OR (
                                   NOT EXISTS (SELECT 1 FROM operations s WHERE s.task=o.task AND s.state='started')
                                   AND NOT EXISTS (
                                       SELECT 1 FROM operations p
                                       WHERE p.task=o.task AND o.batch_id IS NOT NULL AND p.batch_id=o.batch_id
                                         AND p.seq_idx < o.seq_idx AND p.state IN ('approval','ready','started')
                                   )
                               )
                           )
                         ORDER BY o.rowid LIMIT 1
                     ) RETURNING id,task,action",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, action_at(r, 2)?)),
                )
                .optional()
                .db()?;
            let Some((op_id, task, action)) = claimed else {
                return Ok(None);
            };
            let sb_str: String = db
                .query_row(
                    "SELECT COALESCE((SELECT p.sandbox FROM root_policies p JOIN tasks t ON t.root=p.root WHERE t.id=?1), 'direct')",
                    [&task],
                    |r| r.get(0),
                )
                .db()?;
            let sandbox = SandboxMode::parse(&sb_str).unwrap_or(SandboxMode::Direct);
            Ok(Some(ToolJob { op_id, task, action, sandbox }))
        })
    }

    /// Execute a batched whiteboard operation in the store's single-writer transaction.
    /// The daemon calls this instead of dispatching wb_read/wb_write to the filesystem tool worker.
    pub fn execute_whiteboard_op(&mut self, task: &str, action: &Value) -> Result<String> {
        self.transaction(|db| {
            let root = tree::work_root(db, task)?.ok_or("whiteboard operations require a tree stage")?;
            let round: i64 = db.query_row("SELECT round FROM tree_runs WHERE root=?1", [&root], |r| r.get(0)).db()?;
            match field(action, "op")? {
                "wb_read" => {
                    let key = action.get("key").and_then(Value::as_str);
                    let prefix = action.get("prefix").and_then(Value::as_str);
                    let since_seq = action.get("since_seq").and_then(Value::as_i64);
                    let (seq, entries) = tree::whiteboard_entries(db, &root, key, prefix, since_seq)?;
                    let mut receipt = json!({"whiteboard":true,"op":"wb_read","seq":seq,"entries":entries});
                    let text = receipt.to_string();
                    if text.len() <= TOOL_RESULT_LIMIT {
                        return Ok(text);
                    }
                    truncate_wb_read(&mut receipt, TOOL_RESULT_LIMIT, str::len);
                    Ok(receipt.to_string())
                }
                "wb_write" => {
                    let key = field(action, "key")?;
                    let mode = action.get("mode").and_then(Value::as_str).unwrap_or("set");
                    let kind = action.get("kind").and_then(Value::as_str);
                    let content = action.get("content").and_then(Value::as_str);
                    let expected_version = action.get("expected_version").and_then(Value::as_i64);
                    let author_agent: String =
                        db.query_row("SELECT agent FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
                    let mut receipt = tree::wb_write_entry(
                        db,
                        &root,
                        round,
                        Some(task),
                        &author_agent,
                        key,
                        mode,
                        kind,
                        content,
                        expected_version,
                    )?;
                    let mut text = receipt.to_string();
                    if text.len() > TOOL_RESULT_LIMIT {
                        truncate_wb_write(&mut receipt);
                        text = receipt.to_string();
                    }
                    if receipt["ok"] == false {
                        return Err(text);
                    }
                    Ok(text)
                }
                op => Err(format!("action {op} is not a whiteboard operation")),
            }
        })
    }

    #[cfg(test)]
    pub fn claim_tool(&mut self) -> Result<Option<(String, Value)>> {
        Ok(self.claim_tool_job()?.map(|j| (j.task, j.action)))
    }

    #[cfg(test)]
    pub fn tool_result(&mut self, task: &str, result: std::result::Result<&str, &str>) -> Result<()> {
        let op_id: String = self
            .db
            .query_row(
                "SELECT id FROM operations WHERE task=?1 AND state='started' ORDER BY rowid LIMIT 1",
                [task],
                |r| r.get(0),
            )
            .optional()
            .db()?
            .ok_or("task has no started tool")?;
        self.tool_result_op(&op_id, result)
    }

    pub fn tool_result_op(&mut self, op_id: &str, result: std::result::Result<&str, &str>) -> Result<()> {
        self.transaction(|db| {
            let (ok, text) = match result {
                Ok(text) => (true, bound_tool_result(text)),
                Err(error) => (false, bound_tool_result(error)),
            };
            let (task, action, batch_id): (String, Value, Option<String>) = db
                .query_row(
                    "UPDATE operations SET state='done',reserved=0,ok=?2,result=?3 WHERE id=?1 AND state='started' RETURNING task,action,batch_id",
                    params![op_id, ok, &text],
                    |r| Ok((r.get(0)?, action_at(r, 1)?, r.get(2)?)),
                )
                .optional()
                .db()?
                .ok_or("operation is not active")?;
            if action["op"] == "verify" {
                if let Some(root) = tree::work_root(db, &task)? {
                    tree::finish_check(db, &root, ok, &text)
                } else {
                    finish_check(db, &task, ok, &text)
                }
            } else if let Some(bid) = batch_id {
                refresh_batch_or_queue(db, &task, &bid)
            } else {
                queue(db, &task, json!({"task":task,"tool":action["op"],"ok":ok,"result":text}))
            }
        })
    }

    pub fn discard_late_tool_op(&mut self, op_id: &str) -> Result<bool> {
        self.transaction(|db| {
            let stale: bool = db
                .query_row(
                    "SELECT CASE WHEN EXISTS(SELECT 1 FROM operations WHERE id=?1) THEN
                        EXISTS(SELECT 1 FROM operations o JOIN tasks t ON t.id=o.task
                               WHERE o.id=?1 AND (o.state<>'started' OR t.status IN ('cancelled','failed','unknown')))
                     ELSE 1 END",
                    [op_id],
                    |r| r.get(0),
                )
                .db()?;
            if stale {
                db.execute("UPDATE operations SET reserved=0 WHERE id=?1 AND state='unknown'", [op_id]).db()?;
            }
            Ok(stale)
        })
    }

    pub fn cancel(&mut self, task: &str) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::work_root(db,task)? {
                return tree::cancel(db,&root,task);
            }
            let descendants = {
                let mut q = db.prepare("WITH RECURSIVE subtree(id) AS (SELECT id FROM tasks WHERE id=?1
                    UNION ALL SELECT t.id FROM tasks t JOIN subtree s ON t.parent=s.id)
                    SELECT id FROM subtree WHERE id IN (SELECT id FROM tasks WHERE status IN ('queued','running','waiting','approval'))").db()?;
                let ids = q.query_map([task],|r|r.get::<_,String>(0)).db()?;
                ids.collect::<rusqlite::Result<Vec<_>>>().db()?
            };
            if descendants.is_empty() { return Err("task does not exist or is already terminal".into()); }
            let mut uncertain = false;
            for child in descendants {
                let started = db.execute("UPDATE operations SET state='unknown' WHERE task=?1 AND state='started'",[&child]).db()? > 0;
                uncertain |= started;
                db.execute("UPDATE operations SET state='cancelled' WHERE task=?1 AND state IN ('approval','ready')",[&child]).db()?;
                let (status,reason) = if started { ("unknown","cancelled after tool start; outcome unknown") } else { ("cancelled","cancelled by user") };
                // Do not release an in-flight model here. Its stale completion cannot authorize work.
                db.execute("UPDATE tasks SET status=?2,result=?3 WHERE id=?1",params![child,status,reason]).db()?;
            }
            let (status,reason) = if uncertain { ("unknown","cancelled after tool start; outcome unknown") } else { ("cancelled","cancelled by user") };
            stop_chain(db,task,status,reason)
        })
    }

    pub fn delete(&mut self, task_id: &str) -> Result<String> {
        self.transaction(|db| {
            let root: String = db
                .query_row("SELECT root FROM tasks WHERE id=?1", [task_id], |r| r.get(0))
                .optional()
                .db()?
                .ok_or_else(|| format!("unknown task: {task_id}"))?;
            let in_flight: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM tasks WHERE root=?1 AND model_reserved=1)
                        OR EXISTS(SELECT 1 FROM operations o JOIN tasks t ON t.id=o.task WHERE t.root=?1 AND o.reserved=1)
                        OR EXISTS(SELECT 1 FROM tasks WHERE root=?1 AND status IN ('queued','running','waiting','approval'))
                        OR EXISTS(SELECT 1 FROM tree_runs WHERE root=?1 AND phase<>'complete')",
                    [&root],
                    |r| r.get(0),
                )
                .db()?;
            if in_flight {
                return Err(
                    "cannot delete session while work is in flight; cancel it first and wait for the current operation to settle"
                        .into(),
                );
            }
            let internal_agents: Vec<String> = {
                let mut q = db
                    .prepare(
                        "SELECT DISTINCT t.agent FROM tasks t JOIN agents a ON a.name=t.agent WHERE t.root=?1 AND a.internal=1",
                    )
                    .db()?;
                let rows = q.query_map([&root], |r| r.get::<_, String>(0)).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            };
            db.execute("DELETE FROM clarifications WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM steerings WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM whiteboard_entries WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM tree_rounds WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM root_model_settings WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM tree_runs WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM root_policies WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM node_messages WHERE task IN (SELECT id FROM tasks WHERE root=?1)", [&root]).db()?;
            db.execute("DELETE FROM operations WHERE task IN (SELECT id FROM tasks WHERE root=?1)", [&root]).db()?;
            db.execute("UPDATE tasks SET parent=NULL WHERE root=?1", [&root]).db()?;
            db.execute("DELETE FROM tasks WHERE root=?1 AND id<>?1", [&root]).db()?;
            db.execute("DELETE FROM tasks WHERE id=?1", [&root]).db()?;
            for agent in internal_agents {
                db.execute("DELETE FROM messages WHERE agent=?1", [&agent]).db()?;
                db.execute("DELETE FROM agents WHERE name=?1 AND internal=1", [&agent]).db()?;
            }
            Ok(root)
        })
    }

    #[cfg(test)]
    pub fn release_cancelled(&mut self, task: &str) -> Result<()> {
        self.transaction(|db| release_reservations(db, task))
    }

    pub fn whiteboard(&mut self, task_or_root: &str) -> Result<Value> {
        self.transaction(|db| {
            let root: String = db
                .query_row("SELECT root FROM tasks WHERE id=?1", [task_or_root], |r| r.get(0))
                .optional()
                .db()?
                .ok_or_else(|| format!("unknown task: {task_or_root}"))?;
            let exists: bool =
                db.query_row("SELECT EXISTS(SELECT 1 FROM tree_runs WHERE root=?1)", [&root], |r| r.get(0)).db()?;
            if !exists {
                return Err("legacy tasks do not have a collaborative whiteboard".into());
            }
            let (seq, entries) = tree::whiteboard_entries(db, &root, None, None, None)?;
            Ok(json!({"root": root, "seq": seq, "entries": entries}))
        })
    }

    pub fn user_wb_write(
        &mut self,
        task_or_root: &str,
        key: &str,
        mode: &str,
        kind: Option<&str>,
        content: Option<&str>,
    ) -> Result<Value> {
        self.transaction(|db| {
            let root: String = db
                .query_row("SELECT root FROM tasks WHERE id=?1", [task_or_root], |r| r.get(0))
                .optional()
                .db()?
                .ok_or_else(|| format!("unknown task: {task_or_root}"))?;
            let round: Option<i64> =
                db.query_row("SELECT round FROM tree_runs WHERE root=?1", [&root], |r| r.get(0)).optional().db()?;
            let Some(round) = round else {
                return Err("legacy tasks do not have a collaborative whiteboard".into());
            };
            tree::wb_write_entry(db, &root, round, None, "user", key, mode, kind, content, None)
        })
    }

    pub fn steer(&mut self, task_or_root: &str, text: &str) -> Result<Value> {
        nonempty(text, "steering text")?;
        self.transaction(|db| {
            let root: String = db
                .query_row("SELECT root FROM tasks WHERE id=?1", [task_or_root], |r| r.get(0))
                .optional()
                .db()?
                .ok_or_else(|| format!("unknown task: {task_or_root}"))?;
            let row: Option<(String, Option<String>)> = db
                .query_row("SELECT phase,question FROM tree_runs WHERE root=?1", [&root], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()
                .db()?;
            let Some((phase, question)) = row else {
                return Err("legacy tasks do not support mid-turn steering".into());
            };
            if phase == "complete" {
                return Err("session has already settled; send follow-up input to continue instead".into());
            }
            if question.is_some() {
                return Err("session is waiting for clarification; use answer instead".into());
            }
            let target_task = (task_or_root != root).then_some(task_or_root);
            db.execute(
                "INSERT INTO steerings(id,root,task,text,created_at) VALUES (?1,?2,?3,?4,?5)",
                params![id(), root, target_task, text.trim(), unix_secs_now()],
            )
            .db()?;
            Ok(json!({"ok":true,"root":root,"task":task_or_root}))
        })
    }

    pub fn prepare_compaction(&mut self, task: &str) -> Result<Option<CompactionJob>> {
        self.transaction(|db| {
            if tree::report_root(db, task)?.is_some() {
                return Ok(None);
            }
            let row: Option<(String, String, bool)> = db
                .query_row(
                    "SELECT root,agent,EXISTS(SELECT 1 FROM tree_runs WHERE tree_runs.root=tasks.root)
                     FROM tasks WHERE id=?1 AND status='running' AND model_reserved=1",
                    [task],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()
                .db()?;
            let Some((root, agent, is_tree)) = row else {
                return Ok(None);
            };
            let history = load_compaction_history(db, task, &agent, is_tree)?;
            let Some((head_keep, tail_keep, mid_end)) = compute_compaction_slice(&history) else {
                return Ok(None);
            };
            let settings = root_model_settings(db, &root)?;
            Ok(Some(CompactionJob {
                task: task.to_owned(),
                agent,
                head_keep,
                tail_keep,
                middle: history[head_keep..mid_end].to_vec(),
                settings,
            }))
        })
    }
}

pub(crate) fn compute_compaction_slice(history: &[Value]) -> Option<(usize, usize, usize)> {
    let len = history.len();
    if len < 4 {
        return None;
    }
    let mut head_keep = 2usize.min(len.saturating_sub(2));
    if head_keep > 1 && history[head_keep - 1]["role"] == "assistant" {
        head_keep -= 1;
    }
    let mut tail_keep = if len >= 10 { 6 } else { 2.min(len.saturating_sub(head_keep + 2)) };
    let mut mid_end = len.saturating_sub(tail_keep);
    if tail_keep > 0 && mid_end > 0 && history[mid_end - 1]["role"] == "assistant" && history[mid_end]["role"] == "user"
    {
        tail_keep -= 1;
        mid_end = len.saturating_sub(tail_keep);
    }
    if mid_end <= head_keep + 1 {
        return None;
    }
    Some((head_keep, tail_keep, mid_end))
}

fn load_compaction_history(db: &Connection, task: &str, agent: &str, is_tree: bool) -> Result<Vec<Value>> {
    if is_tree {
        let mut q = db.prepare("SELECT role,content FROM node_messages WHERE task=?1 ORDER BY rowid").db()?;
        let rows =
            q.query_map([task], |r| Ok(json!({"role":r.get::<_,String>(0)?,"content":r.get::<_,String>(1)?}))).db()?;
        rows.collect::<rusqlite::Result<_>>().db()
    } else {
        let mut q = db.prepare("SELECT role,content FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
        let rows =
            q.query_map([agent], |r| Ok(json!({"role":r.get::<_,String>(0)?,"content":r.get::<_,String>(1)?}))).db()?;
        rows.collect::<rusqlite::Result<_>>().db()
    }
}

fn rewrite_compacted_history(
    db: &Connection,
    task: &str,
    agent: &str,
    head_keep: usize,
    tail_keep: usize,
    summary: &str,
) -> Result<()> {
    let is_tree = tree::work_root(db, task)?.is_some();
    let history: Vec<(String, String, Option<String>)> = if is_tree {
        let mut q = db.prepare("SELECT role,content,meta FROM node_messages WHERE task=?1 ORDER BY rowid").db()?;
        let rows = q.query_map([task], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).db()?;
        rows.collect::<rusqlite::Result<_>>().db()?
    } else {
        let mut q = db.prepare("SELECT role,content,meta FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
        let rows = q.query_map([agent], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).db()?;
        rows.collect::<rusqlite::Result<_>>().db()?
    };
    let len = history.len();
    let tail_start = len.saturating_sub(tail_keep).max(head_keep);
    let dropped = tail_start.saturating_sub(head_keep);
    let checkpoint = json!({
        "compacted_history": true,
        "dropped_messages": dropped,
        "summary": summary
    })
    .to_string();
    if is_tree {
        db.execute("DELETE FROM node_messages WHERE task=?1", [task]).db()?;
        for (role, content, meta) in &history[..head_keep.min(len)] {
            db.execute(
                "INSERT INTO node_messages(task,role,content,meta) VALUES (?1,?2,?3,?4)",
                params![task, role, content, meta],
            )
            .db()?;
        }
        node_message(db, task, "user", &checkpoint)?;
        for (role, content, meta) in &history[tail_start..] {
            db.execute(
                "INSERT INTO node_messages(task,role,content,meta) VALUES (?1,?2,?3,?4)",
                params![task, role, content, meta],
            )
            .db()?;
        }
    } else {
        db.execute("DELETE FROM messages WHERE agent=?1", [agent]).db()?;
        for (role, content, meta) in &history[..head_keep.min(len)] {
            db.execute(
                "INSERT INTO messages(agent,role,content,meta) VALUES (?1,?2,?3,?4)",
                params![agent, role, content, meta],
            )
            .db()?;
        }
        message(db, agent, "user", &checkpoint)?;
        for (role, content, meta) in &history[tail_start..] {
            db.execute(
                "INSERT INTO messages(agent,role,content,meta) VALUES (?1,?2,?3,?4)",
                params![agent, role, content, meta],
            )
            .db()?;
        }
    }
    Ok(())
}

impl Store {
    pub fn prepare_manual_compaction(&mut self, task_or_root: &str) -> Result<CompactionJob> {
        self.transaction(|db| {
            let root: String = db
                .query_row("SELECT root FROM tasks WHERE id=?1", [task_or_root], |r| r.get(0))
                .optional()
                .db()?
                .ok_or_else(|| format!("unknown task: {task_or_root}"))?;
            let tree_info: Option<(String, Option<String>)> = db
                .query_row("SELECT phase,continuation_node FROM tree_runs WHERE root=?1", [&root], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()
                .db()?;
            let target = match &tree_info {
                Some((phase, cont)) => {
                    if phase == "reporting" {
                        return Err("cannot compact while reporter is running".into());
                    }
                    if task_or_root == root && phase == "complete" {
                        cont.clone().unwrap_or_else(|| task_or_root.to_owned())
                    } else {
                        task_or_root.to_owned()
                    }
                }
                None => task_or_root.to_owned(),
            };
            let (agent, model_busy): (String, bool) = db
                .query_row("SELECT agent,model_reserved FROM tasks WHERE id=?1", [&target], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .db()?;
            let op_busy: bool = db
                .query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE task=?1 AND reserved=1)", [&target], |r| {
                    r.get(0)
                })
                .db()?;
            if model_busy || op_busy {
                return Err("cannot compact while a turn or tool operation is in flight".into());
            }
            let is_tree = tree_info.is_some();
            let history = load_compaction_history(db, &target, &agent, is_tree)?;
            let (head_keep, tail_keep, mid_end) =
                compute_compaction_slice(&history).ok_or("conversation history is too short to compact yet")?;
            db.execute("UPDATE tasks SET model_reserved=1 WHERE id=?1", [&target]).db()?;
            let settings = root_model_settings(db, &root)?;
            Ok(CompactionJob {
                task: target,
                agent,
                head_keep,
                tail_keep,
                middle: history[head_keep..mid_end].to_vec(),
                settings,
            })
        })
    }

    pub fn apply_compaction(&mut self, task: &str, head_keep: usize, tail_keep: usize, summary: &str) -> Result<()> {
        self.transaction(|db| {
            let agent = take_model(db, task)?;
            rewrite_compacted_history(db, task, &agent, head_keep, tail_keep, summary)?;
            db.execute(
                "UPDATE tasks SET status='queued', last_input_tokens=0, last_history_bytes=0 WHERE id=?1",
                [task],
            )
            .db()?;
            Ok(())
        })
    }

    pub fn apply_manual_compaction(
        &mut self,
        task: &str,
        head_keep: usize,
        tail_keep: usize,
        summary: &str,
    ) -> Result<()> {
        self.transaction(|db| {
            let agent: String = db
                .query_row(
                    "UPDATE tasks SET model_reserved=0 WHERE id=?1 AND model_reserved=1 RETURNING agent",
                    [task],
                    |r| r.get(0),
                )
                .optional()
                .db()?
                .ok_or("manual compaction reservation was lost")?;
            rewrite_compacted_history(db, task, &agent, head_keep, tail_keep, summary)?;
            db.execute("UPDATE tasks SET last_input_tokens=0, last_history_bytes=0 WHERE id=?1", [task]).db()?;
            Ok(())
        })
    }

    pub fn recover(&mut self) -> Result<()> {
        self.transaction(|db| {
            // Reserved turns are already charged. A repeated model request gets a new charge.
            db.execute("UPDATE tasks SET status='queued',wb_wait_key=NULL,wb_wait_seq=NULL WHERE status='running'", [])
                .db()?;
            db.execute("UPDATE tasks SET model_reserved=0 WHERE model_reserved=1", []).db()?;
            db.execute("UPDATE operations SET reserved=0 WHERE reserved=1", []).db()?;
            let lost = {
                let mut q =
                    db.prepare("UPDATE operations SET state='unknown' WHERE state='started' RETURNING task").db()?;
                let rows = q.query_map([], |r| r.get::<_, String>(0)).db()?;
                rows.collect::<rusqlite::Result<Vec<_>>>().db()?
            };
            // Once a stage has an unknown outcome, neither an unstarted job nor an approval may resume it.
            db.execute(
                "UPDATE operations SET state='cancelled' WHERE state IN ('ready','approval')
                 AND (task,round) IN (SELECT task,round FROM operations WHERE state='unknown')",
                [],
            )
            .db()?;
            let mut seen_lost = HashSet::new();
            for task in lost {
                if !seen_lost.insert(task.clone()) {
                    continue;
                }
                if let Some(root) = tree::work_root(db, &task)? {
                    tree::lost_operation(db, &root, &task)?;
                } else {
                    stop_chain(db, &task, "unknown", "tool was started without a durable result; outcome unknown")?;
                }
            }
            let active_roots: Vec<String> = {
                let mut q = db.prepare("SELECT root FROM tree_runs WHERE phase='work'").db()?;
                let rows = q.query_map([], |r| r.get::<_, String>(0)).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            };
            for root in active_roots {
                tree::wake_wb_waiters(db, &root)?;
            }
            tree::recover_reports(db)
        })
    }
}

fn is_read_only_local_tool(op: &str) -> bool {
    matches!(op, "read_file" | "list_dir" | "grep_files" | "read_skill" | "mcp_list")
}

fn is_local_tool_op(op: &str) -> bool {
    is_read_only_local_tool(op) || matches!(op, "edit_file" | "apply_patch" | "shell" | "mcp_call")
}

fn is_batch_tool_op(op: &str) -> bool {
    is_local_tool_op(op) || matches!(op, "wb_read" | "wb_write")
}

fn is_batch_read_only_op(op: &str) -> bool {
    is_read_only_local_tool(op) || op == "wb_read"
}

fn validate_local_tool(action: &Value) -> Result<()> {
    match field(action, "op")? {
        "read_file" => {
            nonempty(field(action, "path")?, "path")?;
        }
        "list_dir" => {
            if let Some(p) = action.get("path").filter(|v| !v.is_null()) {
                nonempty(p.as_str().ok_or("expected string field path")?, "path")?;
            }
        }
        "grep_files" => {
            nonempty(field(action, "pattern")?, "pattern")?;
        }
        "edit_file" => {
            nonempty(field(action, "path")?, "path")?;
            field(action, "old")?;
            field(action, "new")?;
        }
        "apply_patch" => {
            nonempty(field(action, "patch")?, "patch")?;
        }
        "shell" => {
            nonempty(field(action, "command")?, "command")?;
        }
        "read_skill" => {
            nonempty(field(action, "name")?, "name")?;
        }
        "mcp_list" => {
            if let Some(s) = action.get("server").filter(|v| !v.is_null()) {
                nonempty(s.as_str().ok_or("expected string field server")?, "server")?;
            }
        }
        "mcp_call" => {
            nonempty(field(action, "server")?, "server")?;
            nonempty(field(action, "tool")?, "tool")?;
        }
        _ => return Err("not a model-requestable local tool".into()),
    }
    Ok(())
}

fn validate_update_plan(action: &Value) -> Result<Value> {
    let items = action.get("plan").and_then(Value::as_array).ok_or("update_plan requires plan array")?;
    if items.is_empty() || items.len() > 32 {
        return Err("update_plan requires 1 to 32 steps".into());
    }
    let mut in_progress = 0usize;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let step = nonempty(item.get("step").and_then(Value::as_str).ok_or("plan step must be string")?, "plan step")?;
        let status = item.get("status").and_then(Value::as_str).ok_or("plan status must be string")?;
        if !matches!(status, "pending" | "in_progress" | "completed") {
            return Err("plan step status must be pending, in_progress, or completed".into());
        }
        if status == "in_progress" {
            in_progress += 1;
            if in_progress > 1 {
                return Err("update_plan allows at most one in_progress step".into());
            }
        }
        out.push(json!({"step": step, "status": status}));
    }
    Ok(Value::Array(out))
}

fn truncate_batch_result(op: &Value, text: &str, limit: usize) -> String {
    if matches!(op.as_str(), Some("wb_read" | "wb_write")) {
        if let Ok(mut receipt) = serde_json::from_str::<Value>(text) {
            if receipt["op"] == *op && receipt["whiteboard"] == true {
                if op == "wb_read" && receipt["entries"].is_array() {
                    truncate_wb_read(&mut receipt, limit, escaped_len);
                    return receipt.to_string();
                }
                if op == "wb_write" {
                    truncate_wb_write(&mut receipt);
                    return receipt.to_string();
                }
            }
        }
    }
    let marker = "\n\n[batch tool result truncated]";
    let mut remaining = limit - escaped_len(marker);
    let end = text.bytes().position(|b| {
        let cost = escaped_byte_len(b);
        if cost > remaining {
            return true;
        }
        remaining -= cost;
        false
    });
    let end = text.floor_char_boundary(end.unwrap_or(text.len()));
    format!("{}{marker}", &text[..end])
}

fn refresh_batch_or_queue(db: &Connection, task: &str, batch_id: &str) -> Result<()> {
    let (approval_cnt, active_cnt): (i64, i64) = db
        .query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN state='approval' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN state IN ('approval','ready','started') THEN 1 ELSE 0 END), 0)
             FROM operations WHERE task=?1 AND batch_id=?2",
            params![task, batch_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .db()?;
    if approval_cnt > 0 {
        db.execute("UPDATE tasks SET status='approval' WHERE id=?1", [task]).db()?;
        return Ok(());
    }
    if active_cnt > 0 {
        db.execute("UPDATE tasks SET status='waiting' WHERE id=?1", [task]).db()?;
        return Ok(());
    }
    let mut q = db
        .prepare("SELECT action,state,ok,result FROM operations WHERE task=?1 AND batch_id=?2 ORDER BY seq_idx,rowid")
        .db()?;
    let rows = q
        .query_map(params![task, batch_id], |r| {
            Ok((action_at(r, 0)?, r.get::<_, String>(1)?, r.get::<_, Option<bool>>(2)?, r.get::<_, Option<String>>(3)?))
        })
        .db()?;
    let mut results = Vec::new();
    let mut reads = Vec::new();
    let mut overhead = r#"{"batch_results":[]}"#.len();
    let mut payload_size = 0;
    for row in rows {
        let (action, state, ok, res) = row.db()?;
        let ok = state == "done" && ok == Some(true);
        if ok && action["op"] == "wb_read" && !action["key"].is_string() && !action["prefix"].is_string() {
            reads.push((results.len(), action["since_seq"].as_i64().unwrap_or(0)));
        }
        let text = res.unwrap_or_else(|| if state == "denied" { "tool denied by user".into() } else { String::new() });
        let mut item = json!({"task":task,"tool":action["op"],"ok":ok,"result":""});
        overhead += item.to_string().len() + usize::from(!results.is_empty());
        payload_size += escaped_len(&text);
        item["result"] = Value::String(text);
        results.push(item);
    }
    // UUID task IDs, fixed tool names and <=16 calls leave ample room for a marker per item.
    if overhead + payload_size > BATCH_RESULTS_LIMIT {
        let per_item = (BATCH_RESULTS_LIMIT - overhead) / results.len();
        for item in &mut results {
            let text = item["result"].as_str().unwrap();
            if escaped_len(text) > per_item {
                item["result"] = Value::String(truncate_batch_result(&item["tool"], text, per_item));
            }
        }
    }
    // Acknowledge only complete snapshots in the exact feedback being queued. Interleaved
    // writes (including after the snapshot) must not let a read skip or regress the watermark.
    for (idx, since) in reads {
        let text = results[idx]["result"].as_str().unwrap();
        if let Ok(receipt) = serde_json::from_str::<Value>(text) {
            if receipt["op"] == "wb_read" && receipt["entries"].is_array() && receipt["truncated"] != true {
                if let Some(seq) = receipt["seq"].as_i64() {
                    db.execute(
                        "UPDATE tasks SET wb_seen_seq=?2 WHERE id=?1 AND wb_seen_seq BETWEEN ?3 AND ?2",
                        params![task, seq, since],
                    )
                    .db()?;
                }
            }
        }
    }
    queue(db, task, json!({"batch_results": results}))
}

fn release_reservations(db: &Connection, task: &str) -> Result<()> {
    db.execute("UPDATE tasks SET model_reserved=0 WHERE id=?1 AND status IN ('failed','cancelled','unknown')", [task])
        .db()?;
    db.execute("UPDATE operations SET reserved=0 WHERE task=?1 AND state='unknown'", [task]).db()?;
    Ok(())
}

fn stop_work(db: &Connection, task: &str, status: &str, reason: &str) -> Result<()> {
    if let Some(root) = tree::work_root(db, task)? {
        db.execute(
            "UPDATE tasks SET status=?2,result=?3,wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1",
            params![task, status, reason],
        )
        .db()?;
        tree::advance(db, &root)
    } else {
        stop_chain(db, task, status, reason)
    }
}

const TASK_VIEW: &str = "SELECT t.id,t.root,t.parent,t.agent,
    CASE WHEN t.id<>g.root OR g.root IS NULL THEN t.status WHEN g.phase='complete' THEN g.outcome
         WHEN g.question IS NOT NULL THEN 'input' WHEN COALESCE(g.active_root,g.root)=g.root AND at.status IN ('queued','running','approval','waiting') THEN at.status ELSE 'waiting' END,
    CASE WHEN t.id=g.root THEN g.result ELSE t.result END,
    CASE WHEN t.id=g.root THEN COALESCE((SELECT tr.prompt FROM tree_rounds tr WHERE tr.root=t.root AND tr.round=1), t.prompt) ELSE t.prompt END,
    CASE WHEN g.root IS NOT NULL AND t.id=g.root THEN at.check_command ELSE t.check_command END,
    CASE WHEN g.root IS NOT NULL AND t.id=g.root THEN at.proposal ELSE t.proposal END,
    o.action,CASE WHEN o.state='approval' THEN o.id END,o.state,
    CASE WHEN g.root IS NOT NULL THEN g.round_turns_used ELSE r.turns_used END,
    NULL,p.policy,
    CASE WHEN g.root IS NULL THEN 'legacy' WHEN t.kind='summarizer' THEN 'summarizer' WHEN t.id=g.root THEN 'root' WHEN a.internal=1 THEN 'repair' ELSE 'node' END,
    CASE WHEN t.id=g.root AND g.question IS NOT NULL THEN 'input' WHEN g.root IS NOT NULL AND t.id=g.root THEN at.status ELSE t.status END,
    CASE WHEN t.id=g.root THEN g.question END,
    CASE WHEN t.id=g.root AND at.status='failed' THEN at.result END,
    CASE WHEN g.root IS NOT NULL THEN g.round ELSE 1 END,
    CASE WHEN t.id=g.root THEN g.active_root END,
    CASE WHEN t.id=g.root THEN g.continuation_node END,
    CASE WHEN t.id=g.root AND cn.id IS NOT NULL THEN CASE WHEN cn.kind='summarizer' THEN 'summarizer' ELSE cn.agent END END,
    CASE WHEN t.id=g.root AND cn.id IS NOT NULL THEN CASE WHEN cn.kind='summarizer' THEN 'summarizer' ELSE 'leaf' END END,
    p.sandbox
    FROM tasks t JOIN tasks r ON r.id=t.root JOIN agents a ON a.name=t.agent JOIN root_policies p ON p.root=r.id
    LEFT JOIN tree_runs g ON g.root=t.root
    LEFT JOIN tasks at ON at.id=g.active_root
    LEFT JOIN tasks cn ON cn.id=g.continuation_node
    LEFT JOIN operations o ON o.id = (
        SELECT o2.id FROM operations o2
        WHERE (
            (o2.task=t.id AND o2.round=t.round)
            OR (g.root IS NOT NULL AND t.id=g.root AND COALESCE(g.active_root,g.root)<>g.root
                AND o2.task=g.active_root AND o2.round=g.round AND json_extract(o2.action,'$.op')='verify')
        ) AND o2.state IN ('approval','ready','started','unknown')
        ORDER BY CASE o2.state WHEN 'approval' THEN 0 WHEN 'unknown' THEN 1 WHEN 'started' THEN 2 ELSE 3 END, o2.seq_idx, o2.rowid
        LIMIT 1
    )";

fn action_at(row: &Row<'_>, col: usize) -> rusqlite::Result<Value> {
    let serialized: Option<String> = row.get(col)?;
    serialized
        .map(|s| {
            serde_json::from_str(&s)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(e)))
        })
        .unwrap_or(Ok(Value::Null))
}

fn task_view(r: &Row<'_>) -> rusqlite::Result<Value> {
    let text = |n| r.get::<_, Option<String>>(n);
    Ok(json!({"id":text(0)?,"root":text(1)?,"parent":text(2)?,"agent":text(3)?,"status":text(4)?,
        "result":text(5)?,"prompt":text(6)?,"check":text(7)?,"proposal":text(8)?,"action":action_at(r,9)?,
        "approval_id":text(10)?,"tool_state":text(11)?,"turns_used":r.get::<_,u32>(12)?,"turn_budget":r.get::<_,Option<u32>>(13)?,
        "policy":text(14)?,"kind":text(15)?,"stage_status":text(16)?,"question":text(17)?,"stage_error":text(18)?,
        "round":r.get::<_,u32>(19)?,"active_root":text(20)?,"continuation_node":text(21)?,
        "continuation_agent":text(22)?,"continuation_kind":text(23)?,"sandbox":text(24)?}))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
