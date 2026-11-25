//! One-way work trees. A row in `tree_runs` identifies a new-protocol root.
//! The work stage stays in `tasks` and its independent check/report lifecycle stays here.
use super::*;

pub(super) const MIGRATE_V3: &str = "
ALTER TABLE agents ADD COLUMN internal INTEGER NOT NULL DEFAULT 0 CHECK(internal IN (0,1));
ALTER TABLE tasks ADD COLUMN handoff_context TEXT;
CREATE INDEX tree_work ON tasks(root,agent);
CREATE INDEX task_children ON tasks(parent);
CREATE TABLE tree_runs (
    root TEXT PRIMARY KEY NOT NULL REFERENCES tasks(id),
    reporter TEXT UNIQUE NOT NULL,
    phase TEXT NOT NULL DEFAULT 'work' CHECK(phase IN ('work','check','report_ready','reporting','complete')),
    outcome TEXT CHECK(outcome IN ('done','failed','unknown','cancelled')),
    directory_used INTEGER NOT NULL DEFAULT 0,
    result TEXT, question TEXT
);
CREATE INDEX report_queue ON tree_runs(phase);
CREATE TABLE clarifications (root TEXT NOT NULL REFERENCES tree_runs(root), question TEXT NOT NULL, answer TEXT NOT NULL);
CREATE INDEX clarification_history ON clarifications(root);
";
const MAX_CHILDREN: usize = 16;
// Leave room in the daemon frame and provider request for JSON encoding, ids and status.
const PUBLIC_OUTPUT_BYTES: usize = 256 * 1024;
const INTENT_BYTES: usize = 768 * 1024;
const MAX_AGENT_BYTES: usize = 128;

pub(super) fn work_root(db: &Connection, task: &str) -> Result<Option<String>> {
    db.query_row("SELECT g.root FROM tree_runs g JOIN tasks t ON t.root=g.root WHERE t.id=?1", [task], |r| r.get(0))
        .optional()
        .db()
}

pub(super) fn report_root(db: &Connection, job: &str) -> Result<Option<String>> {
    db.query_row("SELECT root FROM tree_runs WHERE reporter=?1 AND phase='reporting'", [job], |r| r.get(0))
        .optional()
        .db()
}

fn insert_child(db: &Connection, root: &str, parent: &str, target: &str, prompt: &str, context: &str) -> Result<()> {
    db.execute(
        "INSERT INTO tasks(id,root,parent,agent,prompt,handoff_context,status) VALUES (?1,?2,?3,?4,?5,?6,'queued')",
        params![id(), root, parent, target, prompt, context],
    )
    .db()?;
    Ok(())
}

fn handoff(db: &Connection, root: &str, parent: &str, context: &str, children: &Value) -> Result<()> {
    let children = children.as_array().ok_or("expected handoff children")?;
    if !(1..=MAX_CHILDREN).contains(&children.len()) {
        return Err(format!("handoff must name 1 to {MAX_CHILDREN} children"));
    }
    for child in children {
        let target = nonempty(field(child, "agent")?, "agent name")?;
        let prompt = nonempty(field(child, "text")?, "child assignment")?;
        if target.len() > MAX_AGENT_BYTES {
            return Err("child agent name exceeds 128 bytes".into());
        }
        let used: bool = db
            .query_row("SELECT EXISTS (SELECT 1 FROM tasks WHERE root=?1 AND agent=?2)", params![root, target], |r| {
                r.get(0)
            })
            .db()?;
        if used {
            return Err(format!("member {target} already has a stage in this tree"));
        }
        // Existing public members can take new work even when the session is at capacity.
        db.execute("INSERT OR IGNORE INTO agents(name) SELECT ?1 WHERE (SELECT count(*) FROM agents WHERE internal=0)<(SELECT max_agents FROM settings)",[target]).db()?;
        check_member(db, target).map_err(|_| "child unavailable or session agent limit reached".to_owned())?;
        insert_child(db, root, parent, target, prompt, context)?;
    }
    Ok(())
}

pub(super) fn apply(db: &Connection, root: &str, task: &str, agent: &str, action: &Value) -> Result<()> {
    let op = field(action, "op")?;
    if op == "members" {
        if root == task {
            db.execute("UPDATE tree_runs SET directory_used=1 WHERE root=?1", [root]).db()?;
        }
        message(db, agent, "assistant", &action.to_string())?;
        return queue(db, task, json!({"members":members(db)?}));
    }
    let text = nonempty(field(action, "text")?, "stage text")?;
    match op {
        "ask" if root == task => {
            let allowed = db
                .execute(
                    "UPDATE tree_runs SET question=?2 WHERE root=?1 AND directory_used=0
                AND NOT EXISTS (SELECT 1 FROM operations WHERE task=?1)",
                    params![root, text],
                )
                .db()?;
            if allowed == 0 {
                return Err("root may clarify only before using tools".into());
            }
            db.execute("UPDATE tasks SET status='waiting' WHERE id=?1", [task]).db()?;
        }
        op @ ("handoff" | "finish") => {
            if op == "handoff" {
                handoff(db, root, task, text, &action["children"])?;
            }
            db.execute("UPDATE tasks SET status='done',result=?2 WHERE id=?1", params![task, text]).db()?;
        }
        other => return Err(format!("action {other} unavailable for this tree stage")),
    }
    message(db, agent, "assistant", &action.to_string())?;
    advance(db, root)
}

impl Store {
    pub fn answer(&mut self, root: &str, text: &str) -> Result<()> {
        nonempty(text, "answer")?;
        self.transaction(|db| {
            let changed = db.execute("INSERT INTO clarifications(root,question,answer) SELECT root,question,?2 FROM tree_runs WHERE root=?1 AND phase='work' AND question IS NOT NULL",params![root,text]).db()?;
            if changed != 1 { return Err("root is not waiting for an answer".into()); }
            db.execute("UPDATE tree_runs SET question=NULL WHERE root=?1",[root]).db()?;
            queue(db,root,json!({"answer":text}))
        })
    }

    /// Daemon I/O may return after cancellation. Check the actual job and release its reservation.
    pub fn discard_late_completion(&mut self, task: &str) -> Result<bool> {
        self.transaction(|db| {
            let stale = db
                .query_row(
                    "SELECT CASE WHEN EXISTS(SELECT 1 FROM tree_runs WHERE reporter=?1) THEN
                NOT EXISTS(SELECT 1 FROM tree_runs WHERE reporter=?1 AND phase='reporting') ELSE
                EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND status IN ('cancelled','failed','unknown')) OR
                EXISTS(SELECT 1 FROM operations WHERE task=?1 AND reserved=1 AND state='unknown') END",
                    [task],
                    |r| r.get::<_, bool>(0),
                )
                .db()?;
            if stale {
                super::release_reservations(db, task)?;
            }
            Ok(stale)
        })
    }
}

/// Public work projection. Diagnostics, prompts, tools and private member history never enter the reporter envelope.
pub(super) struct Outputs {
    pub nodes: Value,
    pub leaves: Value,
    truncated: bool,
}

pub(super) fn outputs(db: &Connection, root: &str) -> Result<Outputs> {
    let (count, bytes): (u32, i64) = db
        .query_row(
            "SELECT count(*),coalesce(sum(length(CAST(result AS BLOB))),0)
        FROM tasks t WHERE t.root=?1 AND t.status='done' AND NOT EXISTS(SELECT 1 FROM tasks c WHERE c.parent=t.id)",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .db()?;
    let per_leaf = if bytes <= PUBLIC_OUTPUT_BYTES as i64 {
        PUBLIC_OUTPUT_BYTES
    } else {
        PUBLIC_OUTPUT_BYTES / count.max(1) as usize
    };
    let mut q = db
        .prepare(
            "SELECT t.id,t.parent,t.agent,
        CASE WHEN t.id=g.root AND g.question IS NOT NULL THEN 'input' ELSE t.status END,
        CASE WHEN t.id=g.root THEN 'root' WHEN a.internal THEN 'repair' ELSE 'node' END,
        NOT EXISTS(SELECT 1 FROM tasks c WHERE c.parent=t.id), t.result
        FROM tasks t JOIN tree_runs g ON g.root=t.root JOIN agents a ON a.name=t.agent
        WHERE t.root=?1 ORDER BY t.rowid",
        )
        .db()?;
    let mut rows = q.query([root]).db()?;
    let mut nodes = Vec::new();
    let mut leaves = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next().db()? {
        let task: String = row.get(0).db()?;
        let parent: Option<String> = row.get(1).db()?;
        let agent: String = row.get(2).db()?;
        let status: String = row.get(3).db()?;
        let kind: String = row.get(4).db()?;
        nodes.push(json!({"task":task,"parent":parent,"agent":agent,"kind":kind,"status":status}));
        if row.get::<_, bool>(5).db()? {
            let result = if status == "done" {
                let mut text: String = row.get(6).db()?;
                if text.len() > per_leaf {
                    text.truncate(text.floor_char_boundary(per_leaf));
                    text.push_str(" [truncated in root summary]");
                    truncated = true;
                }
                Some(text)
            } else {
                None
            };
            leaves.push(json!({"task":task,"agent":agent,"status":status,"result":result}));
        }
    }
    Ok(Outputs { nodes: Value::Array(nodes), leaves: Value::Array(leaves), truncated })
}

fn queue_report(db: &Connection, root: &str, outcome: &str) -> Result<()> {
    db.execute(
        "UPDATE tree_runs SET phase='report_ready',outcome=?2 WHERE root=?1 AND phase IN ('work','check')",
        params![root, outcome],
    )
    .db()?;
    Ok(())
}

/// Called in the same write transaction as every work-stage settlement. Siblings settle independently.
pub(super) fn advance(db: &Connection, root: &str) -> Result<()> {
    let phase: String = db.query_row("SELECT phase FROM tree_runs WHERE root=?1", [root], |r| r.get(0)).db()?;
    if phase != "work" {
        return Ok(());
    }
    let (active, unknown, failed):(bool,bool,bool) = db.query_row("SELECT
        max(status IN ('queued','running','waiting','approval')), max(status='unknown'), max(status IN ('failed','cancelled'))
        FROM tasks WHERE root=?1",[root],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).db()?;
    if active {
        return Ok(());
    }
    if unknown {
        return queue_report(db, root, "unknown");
    }
    if failed {
        return queue_report(db, root, "failed");
    }
    let check: Option<String> =
        db.query_row("SELECT check_command FROM tasks WHERE id=?1", [root], |r| r.get(0)).db()?;
    if let Some(command) = check {
        // Do not change the finished root stage or append check output to its private history.
        db.execute(
            "INSERT INTO operations(id,task,action,state) VALUES (?1,?2,?3,'ready')",
            params![id(), root, json!({"op":"verify","command":command}).to_string()],
        )
        .db()?;
        db.execute("UPDATE tree_runs SET phase='check' WHERE root=?1", [root]).db()?;
        return Ok(());
    }
    queue_report(db, root, "done")
}

fn intent(db: &Connection, root: &str) -> Result<Option<Value>> {
    let bytes:i64 = db.query_row("SELECT length(CAST(prompt AS BLOB)) +
        (SELECT coalesce(sum(length(CAST(question AS BLOB))+length(CAST(answer AS BLOB))+32),0) FROM clarifications WHERE root=?1)
        FROM tasks WHERE id=?1",[root],|r|r.get(0)).db()?;
    if bytes > INTENT_BYTES as i64 {
        return Ok(None);
    }
    let prompt: String = db.query_row("SELECT prompt FROM tasks WHERE id=?1", [root], |r| r.get(0)).db()?;
    let mut q = db.prepare("SELECT question,answer FROM clarifications WHERE root=?1 ORDER BY rowid").db()?;
    let answers =
        q.query_map([root], |r| Ok(json!({"question":r.get::<_,String>(0)?,"answer":r.get::<_,String>(1)?}))).db()?;
    let answers: Vec<Value> = answers.collect::<rusqlite::Result<_>>().db()?;
    Ok(Some(json!({"intent":prompt,"clarifications":answers})))
}

pub(super) fn finish_check(db: &Connection, root: &str, ok: bool, diagnostics: &str) -> Result<()> {
    if ok {
        return queue_report(db, root, "done");
    }
    let (attempt, budget): (u32, bool) = db
        .query_row(
            "SELECT
        (SELECT count(*) FROM operations WHERE task=?1 AND state='done' AND json_extract(action,'$.op')='verify'),
        turns_used + 1 < turn_budget FROM tasks CROSS JOIN settings WHERE id=?1",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .db()?;
    if !budget || attempt >= MAX_CHECK_ATTEMPTS {
        return queue_report(db, root, "failed");
    }
    // Repairs are forward system stages with private identities. They do not consume a persistent member slot.
    let Some(mut context) = intent(db, root)? else {
        return queue_report(db, root, "failed");
    };
    let repair = format!("repair-{}", id());
    db.execute("INSERT INTO agents(name,internal) VALUES (?1,1)", [&repair]).db()?;
    context["tool"] = json!("verify");
    context["ok"] = json!(false);
    context["result"] = json!(diagnostics);
    context["attempt"] = json!(attempt);
    context["max_attempts"] = json!(MAX_CHECK_ATTEMPTS);
    let command: String = db.query_row("SELECT check_command FROM tasks WHERE id=?1", [root], |r| r.get(0)).db()?;
    context["command"] = json!(command);
    insert_child(
        db,
        root,
        root,
        &repair,
        "Repair the failed submitted check, then finish or hand off the remaining work.",
        &context.to_string(),
    )?;
    db.execute("UPDATE tree_runs SET phase='work' WHERE root=?1", [root]).db()?;
    Ok(())
}

fn report_outcome(db: &Connection, root: &str) -> Result<(String, String)> {
    db.query_row(
        "SELECT outcome, CASE WHEN t.check_command IS NULL THEN 'not_requested' ELSE coalesce((
        SELECT CASE WHEN state='unknown' THEN 'unknown' WHEN ok THEN 'passed' ELSE 'failed' END
        FROM operations WHERE task=?1 AND state IN ('done','unknown') AND json_extract(action,'$.op')='verify'
        ORDER BY rowid DESC LIMIT 1),'not_run') END
        FROM tree_runs g JOIN tasks t ON t.id=g.root WHERE g.root=?1",
        [root],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .db()
}

pub(super) fn claim_report(db: &Connection) -> Result<Option<Turn>> {
    loop {
        let pair: Option<(String, String)> = db
            .query_row(
                "SELECT root,reporter FROM tree_runs WHERE phase='report_ready' ORDER BY rowid LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .db()?;
        let Some((root, task)) = pair else {
            return Ok(None);
        };
        let leaves = outputs(db, &root)?;
        let Some(mut input) = intent(db, &root)? else {
            fallback(db, &root, "intent exceeds report context limit")?;
            continue;
        };
        if leaves.truncated {
            fallback(db, &root, "leaf outputs exceed report context limit")?;
            continue;
        }
        input["leaf_results"] = leaves.leaves;
        let (outcome, verification) = report_outcome(db, &root)?;
        input["outcome"] = json!(outcome);
        input["verification"] = json!(verification);
        let charged = db.execute("UPDATE tasks SET turns_used=turns_used+1 WHERE id=?1 AND turns_used<(SELECT turn_budget FROM settings)",[&root]).db()?;
        if charged == 0 {
            fallback(db, &root, "root model turn budget exhausted")?;
            continue;
        }
        db.execute("UPDATE tree_runs SET phase='reporting' WHERE root=?1", [&root]).db()?;
        let messages = vec![json!({"role":"user","content":input.to_string()})];
        let settings = root_model_settings(db, &root)?;
        return Ok(Some(Turn { kind: "reporter".into(), task, agent: "reporter".into(), messages, settings }));
    }
}

pub(super) fn finish_report(db: &Connection, root: &str, action: &Value) -> Result<()> {
    if field(action, "op")? != "finish" {
        return Err("reporter may only finish".into());
    }
    let text = nonempty(field(action, "text")?, "report text")?;
    db.execute(
        "UPDATE tree_runs SET phase='complete',result=?2 WHERE root=?1 AND phase='reporting'",
        params![root, text],
    )
    .db()?;
    Ok(())
}

pub(super) fn fallback(db: &Connection, root: &str, reason: &str) -> Result<()> {
    let leaves = outputs(db, root)?.leaves;
    let (outcome, verification) = report_outcome(db, root)?;
    let summary =
        format!("Partial report ({reason}). Work: {outcome}. Verification: {verification}. Leaf results: {leaves}");
    db.execute("UPDATE tree_runs SET phase='complete',outcome=CASE WHEN outcome='done' THEN 'failed' ELSE outcome END,result=?2 WHERE root=?1 AND phase IN ('report_ready','reporting')",params![root,summary]).db()?;
    Ok(())
}

pub(super) fn recover_reports(db: &Connection) -> Result<()> {
    let mut q = db.prepare("SELECT root FROM tree_runs WHERE phase='reporting'").db()?;
    let roots = q.query_map([], |r| r.get::<_, String>(0)).db()?.collect::<rusqlite::Result<Vec<_>>>().db()?;
    for root in roots {
        fallback(db, &root, "reporter interrupted")?;
    }
    Ok(())
}

pub(super) fn lost_operation(db: &Connection, root: &str, task: &str) -> Result<()> {
    let check: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE task=?1 AND state='unknown' AND json_extract(action,'$.op')='verify')",[task],|r|r.get(0)).db()?;
    if check {
        return queue_report(db, root, "unknown");
    }
    stop_work(db, task, "unknown", "tool was started without a durable result; outcome unknown")
}

pub(super) fn cancel(db: &Connection, root: &str, task: &str) -> Result<()> {
    let phase: String = db.query_row("SELECT phase FROM tree_runs WHERE root=?1", [root], |r| r.get(0)).db()?;
    if phase == "complete" {
        return Err("tree is already terminal".into());
    }
    let mut q = db.prepare("WITH RECURSIVE subtree(id) AS (SELECT ?1 UNION ALL SELECT t.id FROM tasks t JOIN subtree s ON t.parent=s.id)
        SELECT t.id FROM tasks t JOIN subtree s ON t.id=s.id WHERE t.status IN ('queued','running','waiting','approval')").db()?;
    let active = q.query_map([task], |r| r.get::<_, String>(0)).db()?.collect::<rusqlite::Result<Vec<_>>>().db()?;
    if active.is_empty() && task != root {
        return Err("stage has no unfinished work".into());
    }
    for id in active {
        let unknown = cancel_operations(db, &id)?;
        let (status, reason) = if unknown {
            ("unknown", "cancelled after tool start; outcome unknown")
        } else {
            ("cancelled", "cancelled by user")
        };
        db.execute("UPDATE tasks SET status=?2,result=?3 WHERE id=?1", params![id, status, reason]).db()?;
    }
    if task != root {
        return advance(db, root);
    }
    // The root check and reporter run outside its already finished stage.
    cancel_operations(db, root)?;
    let unknown: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM operations o JOIN tasks t ON t.id=o.task WHERE t.root=?1 AND o.state='unknown')",[root],|r|r.get(0)).db()?;
    let (outcome, reason) = if unknown {
        ("unknown", "cancelled with an unknown tool outcome")
    } else {
        ("cancelled", "cancelled by user")
    };
    db.execute(
        "UPDATE tree_runs SET phase='complete',question=NULL,outcome=?2,result=?3 WHERE root=?1",
        params![root, outcome, reason],
    )
    .db()?;
    Ok(())
}

fn cancel_operations(db: &Connection, task: &str) -> Result<bool> {
    db.execute("UPDATE operations SET state='cancelled' WHERE task=?1 AND state IN ('approval','ready')", [task])
        .db()?;
    Ok(db.execute("UPDATE operations SET state='unknown' WHERE task=?1 AND state='started'", [task]).db()? > 0)
}
