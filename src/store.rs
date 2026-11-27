use crate::{model::ModelSettings, protocol::TurnKind, Result};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
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
        let Some((op, key)) = normalized_action_key(&v) else { continue };
        if matches!(op.as_str(), "edit_file" | "wb_write" | "handoff" | "finish") {
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

pub struct Store {
    db: Connection,
}

#[derive(Debug)]
pub struct Turn {
    pub kind: TurnKind,
    pub task: String,
    pub agent: String,
    pub messages: Vec<Value>,
    /// Pre-snapshot roots retain the provider's legacy environment-based selection.
    pub settings: Option<ModelSettings>,
}

#[derive(Debug)]
pub struct ToolJob {
    pub task: String,
    pub action: Value,
    pub staged: bool,
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

fn model_settings_row(row: &Row<'_>) -> rusqlite::Result<ModelSettings> {
    Ok(ModelSettings { model: row.get(0)?, effort: row.get(1)? })
}

fn root_model_settings(db: &Connection, root: &str) -> Result<Option<ModelSettings>> {
    db.query_row("SELECT model,effort FROM root_model_settings WHERE root=?1", [root], model_settings_row)
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

fn message(db: &Connection, agent: &str, role: &str, content: &str) -> Result<()> {
    db.execute("INSERT INTO messages(agent,role,content) VALUES (?1,?2,?3)", params![agent, role, content]).db()?;
    Ok(())
}

fn node_message(db: &Connection, task: &str, role: &str, content: &str) -> Result<()> {
    db.execute("INSERT INTO node_messages(task,role,content) VALUES (?1,?2,?3)", params![task, role, content]).db()?;
    Ok(())
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

fn operation(db: &Connection, task: &str, action: &Value, state: &str, round: i64) -> Result<()> {
    db.execute(
        "INSERT INTO operations(id,task,action,state,round) VALUES (?1,?2,?3,?4,?5)",
        params![id(), task, action.to_string(), state, round],
    )
    .db()?;
    let status = if state == "approval" { "approval" } else { "waiting" };
    db.execute("UPDATE tasks SET status=?2 WHERE id=?1", params![task, status]).db()?;
    Ok(())
}

fn schema_version(db: &Connection) -> Result<i64> {
    let app: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0)).db()?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0)).db()?;
    if app == APP_ID && matches!(version, 1..=7) {
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
        tx.query_row("SELECT 1 FROM settings,agents WHERE singleton=1 AND name='default'", [], |_| Ok(())).db()?;
        // Reject missing schema in the same transaction as the migration.
        tx.prepare(&format!("{TASK_VIEW} LIMIT 0")).db()?;
        tx.prepare("SELECT role,content FROM messages LIMIT 0").db()?;
        tx.prepare("SELECT role,content FROM node_messages LIMIT 0").db()?;
        tx.prepare("SELECT provider,model,effort FROM model_overrides LIMIT 0").db()?;
        tx.prepare("SELECT root,model,effort FROM root_model_settings LIMIT 0").db()?;
        tx.prepare("SELECT root,round,subtree_root FROM tree_rounds LIMIT 0").db()?;
        tx.prepare(
            "SELECT root,key,kind,content,author_agent,round,version,seq,status,updated_at FROM whiteboard_entries LIMIT 0",
        )
        .db()?;
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

    /// With no daemon default this returns the provider's raw session overrides.
    pub fn model_settings(&self, provider: &str, default_model: Option<&str>) -> Result<ModelSettings> {
        let mut settings = self
            .db
            .query_row("SELECT model,effort FROM model_overrides WHERE provider=?1", [provider], model_settings_row)
            .optional()
            .db()?
            .unwrap_or_default();
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
        self.create_root(agent, prompt, check, review, staged, Some(settings))
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
                params![root, if review { "review" } else { "auto" }, if staged { "staged" } else { "direct" }],
            )
            .db()?;
            db.execute(
                "INSERT INTO root_model_settings(root,model,effort) VALUES (?1,?2,?3)
                 ON CONFLICT(root) DO UPDATE SET model=excluded.model,effort=excluded.effort",
                params![root, settings.model, settings.effort],
            )
            .db()?;
            db.execute(
                "UPDATE tasks SET round=?2,prompt=?3,check_command=?4,handoff_context=NULL,
                 status='queued',introduced=0,proposal=NULL,result=NULL,wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1",
                params![node_id, next_round, prompt, check],
            )
            .db()?;
            db.execute(
                "INSERT INTO tree_rounds(root,round,subtree_root,prompt,check_command) VALUES (?1,?2,?3,?4,?5)",
                params![root, next_round, node_id, prompt, check],
            )
            .db()?;
            Ok((root, node_id))
        })
    }

    fn create_root(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        staged: bool,
        tree_settings: Option<&ModelSettings>,
    ) -> Result<String> {
        nonempty(prompt, "prompt")?;
        if let Some(c) = check {
            nonempty(c, "check command")?;
        }
        self.transaction(|db| {
            check_member(db, agent)?;
            let task = id();
            db.execute(
                "INSERT INTO tasks(id,root,agent,prompt,status,check_command) VALUES (?1,?1,?2,?3,'queued',?4)",
                params![task, agent, prompt, check],
            )
            .db()?;
            db.execute(
                "INSERT INTO root_policies(root,policy,sandbox) VALUES (?1,?2,?3)",
                params![task, if review { "review" } else { "auto" }, if staged { "staged" } else { "direct" }],
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
                    "INSERT INTO root_model_settings(root,model,effort) VALUES (?1,?2,?3)",
                    params![task, settings.model, settings.effort],
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
            // User status only. Omit tool results and edit bodies, and bound even a single-line command.
            let mut q = tx.prepare("SELECT t.id,t.agent,json_extract(o.action,'$.op'),
                substr(CASE json_extract(o.action,'$.op') WHEN 'shell' THEN json_extract(o.action,'$.command')
                    WHEN 'verify' THEN json_extract(o.action,'$.command') ELSE json_extract(o.action,'$.path') END,1,120),o.ok
                FROM tasks t JOIN operations o ON o.task=t.id WHERE t.root=?1 AND o.state='done' ORDER BY o.rowid").db()?;
            let receipts = q
                .query_map([id], |r| {
                    Ok(json!({"task":r.get::<_,String>(0)?,"agent":r.get::<_,String>(1)?,
                "op":r.get::<_,String>(2)?,"label":r.get::<_,String>(3)?,"ok":r.get::<_,bool>(4)?}))
                })
                .db()?;
            task["subtree_receipts"] = Value::Array(receipts.collect::<rusqlite::Result<_>>().db()?);
        }
        tx.commit().db()?;
        Ok(task)
    }

    pub fn claim(&mut self) -> Result<Option<Turn>> {
        self.transaction(|db| {
            if let Some(reporter) = tree::claim_report(db)? {
                return Ok(Some(reporter));
            }
            let next = db.query_row(
                "SELECT id,root,agent,prompt,introduced,round,EXISTS(SELECT 1 FROM tree_runs WHERE tree_runs.root=tasks.root) FROM tasks
                 WHERE status='queued' AND model_reserved=0 ORDER BY rowid LIMIT 1",
                [], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?,
                           r.get::<_,String>(3)?, r.get::<_,bool>(4)?, r.get::<_,i64>(5)?, r.get::<_,bool>(6)?)),
            ).optional().db()?;
            let Some((task, root, agent, prompt, introduced, round, is_tree)) = next else { return Ok(None); };
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
                    if let Some(text) = context {
                        node_message(db, &task, "user", &text)?;
                    }
                    node_message(db, &task, "user", &prompt)?;
                } else {
                    if let Some(text) = context {
                        message(db, &agent, "user", &text)?;
                    }
                    message(db, &agent, "user", &prompt)?;
                }
            }
            let mut messages: Vec<Value> = if is_tree {
                let mut q = db.prepare("SELECT role,content FROM node_messages WHERE task=?1 ORDER BY rowid").db()?;
                let rows = q.query_map([&task], |r| {
                    Ok(json!({"role":r.get::<_,String>(0)?, "content":r.get::<_,String>(1)?}))
                }).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            } else {
                let mut q = db.prepare("SELECT role,content FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
                let rows = q.query_map([&agent], |r| {
                    Ok(json!({"role":r.get::<_,String>(0)?, "content":r.get::<_,String>(1)?}))
                }).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            };
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
                    if let Some(digest) = tree::whiteboard_digest(db, &root, seen_seq)? {
                        parts.push(digest);
                    }
                }
                if !parts.is_empty() {
                    // Inject transient task focus, repetition nudge, or whiteboard digest without mutating persisted history.
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
            Ok(Some(Turn { kind, task, agent, messages, settings }))
        })
    }

    pub fn apply(&mut self, task: &str, action: &Value) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                return tree::finish_report(db, &root, action);
            }
            let agent = take_model(db, task)?;
            if let Some(root) = tree::work_root(db, task)? {
                return tree::apply(db, &root, task, action);
            }
            message(db, &agent, "assistant", &action.to_string())?;
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

    pub fn request_tool(&mut self, task: &str, action: &Value) -> Result<()> {
        let op = field(action, "op")?;
        match op {
            "read_file" => {
                nonempty(field(action, "path")?, "path")?;
            }
            "edit_file" => {
                nonempty(field(action, "path")?, "path")?;
                field(action, "old")?;
                field(action, "new")?;
            }
            "shell" => {
                nonempty(field(action, "command")?, "command")?;
            }
            _ => return Err("not a model-requestable local tool".into()),
        }
        self.transaction(|db| {
            let agent = take_model(db, task)?;
            let (auto, round): (bool, i64) = db
                .query_row(
                    "SELECT p.policy='auto', t.round FROM tasks t JOIN root_policies p ON p.root=t.root WHERE t.id=?1",
                    [task],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .db()?;
            if tree::work_root(db, task)?.is_some() {
                node_message(db, task, "assistant", &action.to_string())?;
            } else {
                message(db, &agent, "assistant", &action.to_string())?;
            }
            operation(db, task, action, if auto || op == "read_file" { "ready" } else { "approval" }, round)
        })
    }

    pub fn approve(&mut self, approval_id: &str, allow: bool) -> Result<()> {
        self.transaction(|db| {
            let state = if allow { "ready" } else { "denied" };
            let task: String = db
                .query_row(
                    "UPDATE operations SET state=?2 WHERE id=?1 AND state='approval' RETURNING task",
                    params![approval_id, state],
                    |r| r.get(0),
                )
                .optional()
                .db()?
                .ok_or("no such pending approval")?;
            if allow {
                db.execute("UPDATE tasks SET status='waiting' WHERE id=?1", [task]).db()?;
                Ok(())
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
                         SELECT o.rowid FROM operations o WHERE o.state='ready' AND o.reserved=0 ORDER BY o.rowid LIMIT 1
                     ) RETURNING task,action",
                    [],
                    |r| Ok((r.get::<_, String>(0)?, action_at(r, 1)?)),
                )
                .optional()
                .db()?;
            let Some((task, action)) = claimed else {
                return Ok(None);
            };
            let staged: bool = db
                .query_row(
                    "SELECT COALESCE((SELECT p.sandbox='staged' FROM root_policies p JOIN tasks t ON t.root=p.root WHERE t.id=?1), 0)",
                    [&task],
                    |r| r.get(0),
                )
                .db()?;
            Ok(Some(ToolJob { task, action, staged }))
        })
    }

    #[cfg(test)]
    pub fn claim_tool(&mut self) -> Result<Option<(String, Value)>> {
        Ok(self.claim_tool_job()?.map(|j| (j.task, j.action)))
    }

    pub fn tool_result(&mut self, task: &str, result: std::result::Result<&str, &str>) -> Result<()> {
        self.transaction(|db| {
            let (ok,text) = match result { Ok(text) => (true,text), Err(error) => (false,error) };
            let action = db.query_row("UPDATE operations SET state='done',reserved=0,ok=?2,result=?3 WHERE task=?1 AND state='started' RETURNING action",
                params![task,ok,text], |r| action_at(r,0)).optional().db()?.ok_or("task has no started tool")?;
            if action["op"] == "verify" {
                if let Some(root) = tree::work_root(db, task)? {
                    tree::finish_check(db, &root, ok, text)
                } else {
                    finish_check(db, task, ok, text)
                }
            } else {
                queue(db, task, json!({"task":task,"tool":action["op"],"ok":ok,"result":text}))
            }
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
                        OR EXISTS(SELECT 1 FROM tree_runs WHERE root=?1 AND phase='reporting')",
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
        let root: String = self
            .db
            .query_row("SELECT root FROM tasks WHERE id=?1", [task_or_root], |r| r.get(0))
            .optional()
            .db()?
            .ok_or_else(|| format!("unknown task: {task_or_root}"))?;
        let exists: bool =
            self.db.query_row("SELECT EXISTS(SELECT 1 FROM tree_runs WHERE root=?1)", [&root], |r| r.get(0)).db()?;
        if !exists {
            return Err("legacy tasks do not have a collaborative whiteboard".into());
        }
        let (seq, entries) = tree::whiteboard_entries(&self.db, &root, None, None, None)?;
        Ok(json!({"root": root, "seq": seq, "entries": entries}))
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
            let history: Vec<Value> = if is_tree {
                let mut q = db.prepare("SELECT role,content FROM node_messages WHERE task=?1 ORDER BY rowid").db()?;
                let rows = q
                    .query_map([task], |r| Ok(json!({"role":r.get::<_,String>(0)?,"content":r.get::<_,String>(1)?})))
                    .db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            } else {
                let mut q = db.prepare("SELECT role,content FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
                let rows = q
                    .query_map([&agent], |r| Ok(json!({"role":r.get::<_,String>(0)?,"content":r.get::<_,String>(1)?})))
                    .db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            };
            let len = history.len();
            if len < 4 {
                return Ok(None);
            }
            let mut head_keep = 2usize.min(len.saturating_sub(2));
            if head_keep > 1 && history[head_keep - 1]["role"] == "assistant" {
                head_keep -= 1;
            }
            let mut tail_keep = if len >= 10 { 6 } else { 2.min(len.saturating_sub(head_keep + 2)) };
            let mut mid_end = len.saturating_sub(tail_keep);
            if tail_keep > 1
                && mid_end > 0
                && history[mid_end - 1]["role"] == "assistant"
                && history[mid_end]["role"] == "user"
            {
                tail_keep -= 1;
                mid_end = len.saturating_sub(tail_keep);
            }
            if mid_end <= head_keep + 1 {
                return Ok(None);
            }
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

    pub fn apply_compaction(&mut self, task: &str, head_keep: usize, tail_keep: usize, summary: &str) -> Result<()> {
        self.transaction(|db| {
            let agent = take_model(db, task)?;
            let is_tree = tree::work_root(db, task)?.is_some();
            let history: Vec<(String, String)> = if is_tree {
                let mut q = db.prepare("SELECT role,content FROM node_messages WHERE task=?1 ORDER BY rowid").db()?;
                let rows = q.query_map([task], |r| Ok((r.get(0)?, r.get(1)?))).db()?;
                rows.collect::<rusqlite::Result<_>>().db()?
            } else {
                let mut q = db.prepare("SELECT role,content FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
                let rows = q.query_map([&agent], |r| Ok((r.get(0)?, r.get(1)?))).db()?;
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
                for (role, content) in &history[..head_keep.min(len)] {
                    node_message(db, task, role, content)?;
                }
                node_message(db, task, "user", &checkpoint)?;
                for (role, content) in &history[tail_start..] {
                    node_message(db, task, role, content)?;
                }
            } else {
                db.execute("DELETE FROM messages WHERE agent=?1", [&agent]).db()?;
                for (role, content) in &history[..head_keep.min(len)] {
                    message(db, &agent, role, content)?;
                }
                message(db, &agent, "user", &checkpoint)?;
                for (role, content) in &history[tail_start..] {
                    message(db, &agent, role, content)?;
                }
            }
            db.execute("UPDATE tasks SET status='queued' WHERE id=?1", [task]).db()?;
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
            for task in lost {
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
    LEFT JOIN operations o ON (
        (o.task=t.id AND o.round=t.round)
        OR (g.root IS NOT NULL AND t.id=g.root AND COALESCE(g.active_root,g.root)<>g.root
            AND o.task=g.active_root AND o.round=g.round AND json_extract(o.action,'$.op')='verify')
    ) AND o.state IN ('approval','ready','started','unknown')";

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
