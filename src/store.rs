use crate::Result;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use uuid::Uuid;

mod tree;

pub struct Store {
    db: Connection,
}

#[derive(Debug)]
pub struct Turn {
    pub kind: String,
    pub task: String,
    pub agent: String,
    pub messages: Vec<Value>,
}

// These pragmas and the schema are committed together, before any work is admitted.
const APP_ID: i64 = 0x5357524d; // SWRM
const MAX_CHECK_ATTEMPTS: u32 = 3;
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

fn report(db: &Connection, task: &str, body: Value) -> Result<()> {
    db.execute(
        "INSERT INTO messages(agent,role,content) SELECT agent,'user',?2 FROM tasks WHERE id=?1",
        params![task, body.to_string()],
    )
    .db()?;
    Ok(())
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
    let (attempt, budget_left): (u32, bool) = db.query_row(
        "SELECT (SELECT count(*) FROM operations WHERE task=?1 AND state='done' AND json_extract(action,'$.op')='verify'),
                r.turns_used < s.turn_budget
         FROM tasks t JOIN tasks r ON r.id=t.root CROSS JOIN settings s WHERE t.id=?1",
        [task], |r| Ok((r.get(0)?, r.get(1)?)),
    ).db()?;
    let mut feedback =
        json!({"task":task,"tool":"verify","ok":ok,"result":text,"attempt":attempt,"max_attempts":MAX_CHECK_ATTEMPTS});
    if ok {
        report(db, task, feedback)?;
        let proposal: String = db.query_row("SELECT proposal FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
        return done(db, task, &proposal);
    }

    db.execute("UPDATE tasks SET proposal=NULL WHERE id=?1", [task]).db()?;
    if budget_left && attempt < MAX_CHECK_ATTEMPTS {
        feedback["next"] = json!("Repair the failure and propose completion again to rerun the submitted check.");
        return queue(db, task, feedback);
    }
    report(db, task, feedback)?;
    let reason =
        if attempt >= MAX_CHECK_ATTEMPTS { "check attempt limit reached" } else { "root model turn budget exhausted" };
    stop_chain(db, task, "failed", &format!("verification failed ({reason}): {text}"))
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

fn operation(db: &Connection, task: &str, action: &Value, state: &str) -> Result<()> {
    db.execute(
        "INSERT INTO operations(id,task,action,state) VALUES (?1,?2,?3,?4)",
        params![id(), task, action.to_string(), state],
    )
    .db()?;
    let status = if state == "approval" { "approval" } else { "waiting" };
    db.execute("UPDATE tasks SET status=?2 WHERE id=?1", params![task, status]).db()?;
    Ok(())
}

fn schema_version(db: &Connection) -> Result<i64> {
    let app: i64 = db.pragma_query_value(None, "application_id", |r| r.get(0)).db()?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0)).db()?;
    if app == APP_ID && matches!(version, 1..=3) {
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
    pub fn open(path: &Path, max_agents: u32, turn_budget: u32) -> Result<Self> {
        if max_agents == 0 || turn_budget == 0 {
            return Err("limits must be positive".into());
        }
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
            tx.execute(
                "INSERT INTO settings(singleton,max_agents,turn_budget) VALUES (1,?1,?2)",
                params![max_agents, turn_budget],
            )
            .db()?;
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
        tx.query_row(
            "SELECT 1 FROM settings,agents WHERE singleton=1 AND max_agents>0 AND turn_budget>0 AND name='default'",
            [],
            |_| Ok(()),
        )
        .db()?;
        // Reject missing schema in the same transaction as the migration.
        tx.prepare(&format!("{TASK_VIEW} LIMIT 0")).db()?;
        tx.prepare("SELECT role,content FROM messages LIMIT 0").db()?;
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
        self.submit_root(agent, prompt, check, review, true)
    }

    fn submit_root(
        &mut self,
        agent: &str,
        prompt: &str,
        check: Option<&str>,
        review: bool,
        tree: bool,
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
                "INSERT INTO root_policies(root,policy) VALUES (?1,?2)",
                params![task, if review { "review" } else { "auto" }],
            )
            .db()?;
            if tree {
                db.execute("INSERT INTO tree_runs(root,reporter) VALUES (?1,?2)", params![task, id()]).db()?;
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
        let mut q =
            tx.prepare("SELECT action,ok,result FROM operations WHERE task=?1 AND state='done' ORDER BY rowid").db()?;
        let evidence = q
            .query_map([id], |r| {
                Ok(json!({"action":action_at(r,0)?,"ok":r.get::<_,bool>(1)?,"result":r.get::<_,String>(2)?}))
            })
            .db()?;
        let evidence = evidence.collect::<rusqlite::Result<Vec<_>>>().db()?;
        drop(q);
        let mut checks = evidence.iter().filter(|e| e["action"]["op"] == "verify").peekable();
        let interrupted = task["tool_state"] == "unknown" && task["action"]["op"] == "verify";
        let attempted = interrupted || checks.peek().is_some();
        let passed = checks.any(|e| e["ok"] == true);
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
        self.transaction(|db| loop {
            if let Some(reporter) = tree::claim_report(db)? {
                return Ok(Some(reporter));
            }
            let next = db.query_row(
                "SELECT id,root,agent,prompt,introduced,EXISTS(SELECT 1 FROM tree_runs WHERE tree_runs.root=tasks.root) FROM tasks
                 WHERE status='queued' AND agent NOT IN occupied_agents ORDER BY rowid LIMIT 1",
                [], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?,
                           r.get::<_,String>(3)?, r.get::<_,bool>(4)?, r.get::<_,bool>(5)?)),
            ).optional().db()?;
            let Some((task, root, agent, prompt, introduced, is_tree)) = next else { return Ok(None); };
            let charged = db.execute(
                "UPDATE tasks SET turns_used=turns_used+1 WHERE id=?1 AND turns_used<(SELECT turn_budget - (?2 AND turn_budget>1) FROM settings)",
                params![root,is_tree],
            ).db()?;
            if charged == 0 {
                stop_work(db, &task, "failed", "root model turn budget exhausted")?;
                continue;
            }
            db.execute("UPDATE tasks SET status='running',introduced=1,model_reserved=1 WHERE id=?1", [&task]).db()?;
            if !introduced {
                let context:Option<String> = db.query_row("SELECT handoff_context FROM tasks WHERE id=?1",[&task],|r|r.get(0)).db()?;
                if let Some(text) = context { message(db,&agent,"user",&text)?; }
                message(db, &agent, "user", &prompt)?;
            }
            let mut q = db.prepare("SELECT role,content FROM messages WHERE agent=?1 ORDER BY rowid").db()?;
            let rows = q.query_map([&agent], |r| {
                Ok(json!({"role":r.get::<_,String>(0)?, "content":r.get::<_,String>(1)?}))
            }).db()?;
            let mut messages: Vec<Value> = rows.collect::<rusqlite::Result<_>>().db()?;
            if introduced {
                // A peer may have handled other work while this task was waiting. Restore its focus.
                messages.push(json!({"role":"user","content":format!("Continue task {task}. Original request: {prompt}")}));
            }
            let kind = if !is_tree { "legacy" } else if task==root { "root" } else { "node" };
            return Ok(Some(Turn { kind:kind.into(),task,agent,messages }));
        })
    }

    pub fn apply(&mut self, task: &str, action: &Value) -> Result<()> {
        self.transaction(|db| {
            if let Some(root) = tree::report_root(db, task)? {
                return tree::finish_report(db, &root, action);
            }
            let agent = take_model(db,task)?;
            if let Some(root) = tree::work_root(db, task)? {
                return tree::apply(db, &root, task, &agent, action);
            }
            message(db,&agent,"assistant",&action.to_string())?;
            match field(action,"op")? {
                "finish" => {
                    let text = nonempty(field(action,"text")?,"finish text")?;
                    let check: Option<String> = db.query_row("SELECT check_command FROM tasks WHERE id=?1",[task],|r|r.get(0)).db()?;
                    if let Some(command) = check {
                        db.execute("UPDATE tasks SET proposal=?2 WHERE id=?1",params![task,text]).db()?;
                        operation(db,task,&json!({"op":"verify","command":command}),"ready")
                    } else { done(db,task,text) }
                }
                "members" => queue(db,task,json!({"members":members(db)?})),
                "spawn" => {
                    let name = nonempty(field(action,"agent")?,"agent name")?;
                    let inserted = db.execute("INSERT INTO agents(name) SELECT ?1 WHERE (SELECT count(*) FROM agents WHERE internal=0)<(SELECT max_agents FROM settings)", [name]).db()?;
                    if inserted == 0 { return Err("session agent limit reached".into()); }
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
            let auto: bool = db
                .query_row(
                    "SELECT p.policy='auto' FROM tasks t JOIN root_policies p ON p.root=t.root WHERE t.id=?1",
                    [task],
                    |r| r.get(0),
                )
                .db()?;
            message(db, &agent, "assistant", &action.to_string())?;
            operation(db, task, action, if auto || op == "read_file" { "ready" } else { "approval" })
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

    pub fn claim_tool(&mut self) -> Result<Option<(String, Value)>> {
        self.transaction(|db| {
            db.query_row(
                "UPDATE operations SET state='started',reserved=1 WHERE rowid=(
                     SELECT o.rowid FROM operations o JOIN tasks t ON t.id=o.task
                     WHERE o.state='ready' AND t.agent NOT IN occupied_agents ORDER BY o.rowid LIMIT 1
                 ) RETURNING task,action",
                [],
                |r| Ok((r.get(0)?, action_at(r, 1)?)),
            )
            .optional()
            .db()
        })
    }

    pub fn tool_result(&mut self, task: &str, result: std::result::Result<&str, &str>) -> Result<()> {
        self.transaction(|db| {
            let (ok,text) = match result { Ok(text) => (true,text), Err(error) => (false,error) };
            let action = db.query_row("UPDATE operations SET state='done',reserved=0,ok=?2,result=?3 WHERE task=?1 AND state='started' RETURNING action",
                params![task,ok,text], |r| action_at(r,0)).optional().db()?.ok_or("task has no started tool")?;
            if action["op"] == "verify" {
                if tree::work_root(db,task)?.is_some() {
                    tree::finish_check(db,task,ok,text)
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

    #[cfg(test)]
    pub fn release_cancelled(&mut self, task: &str) -> Result<()> {
        self.transaction(|db| release_reservations(db, task))
    }

    pub fn recover(&mut self) -> Result<()> {
        self.transaction(|db| {
            // Reserved turns are already charged. A repeated model request gets a new charge.
            db.execute("UPDATE tasks SET status='queued' WHERE status='running'", []).db()?;
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
        db.execute("UPDATE tasks SET status=?2,result=?3 WHERE id=?1", params![task, status, reason]).db()?;
        tree::advance(db, &root)
    } else {
        stop_chain(db, task, status, reason)
    }
}

const TASK_VIEW: &str = "SELECT t.id,t.root,t.parent,t.agent,
    CASE WHEN t.id<>g.root OR g.root IS NULL THEN t.status WHEN g.phase='complete' THEN g.outcome
         WHEN g.question IS NOT NULL THEN 'input' WHEN t.status IN ('queued','running','approval','waiting') THEN t.status ELSE 'waiting' END,
    CASE WHEN t.id=g.root THEN g.result ELSE t.result END,t.prompt,t.check_command,t.proposal,
    o.action,CASE WHEN o.state='approval' THEN o.id END,o.state,r.turns_used,s.turn_budget,p.policy,
    CASE WHEN g.root IS NULL THEN 'legacy' WHEN t.id=g.root THEN 'root' ELSE 'node' END,
    CASE WHEN t.id=g.root AND g.question IS NOT NULL THEN 'input' ELSE t.status END,
    CASE WHEN t.id=g.root THEN g.question END
    FROM tasks t JOIN tasks r ON r.id=t.root JOIN root_policies p ON p.root=r.id CROSS JOIN settings s LEFT JOIN tree_runs g ON g.root=t.root
    LEFT JOIN operations o ON o.task=t.id AND o.state IN ('approval','ready','started','unknown')";

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
        "approval_id":text(10)?,"tool_state":text(11)?,"turns_used":r.get::<_,u32>(12)?,"turn_budget":r.get::<_,u32>(13)?,"policy":text(14)?,"kind":text(15)?,"stage_status":text(16)?,"question":text(17)?}))
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
