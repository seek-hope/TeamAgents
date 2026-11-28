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
pub(super) const MIGRATE_V5: &str = "
ALTER TABLE tasks ADD COLUMN round INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tasks ADD COLUMN kind TEXT CHECK(kind IS NULL OR kind IN ('summarizer'));
ALTER TABLE tree_runs ADD COLUMN round INTEGER NOT NULL DEFAULT 1;
ALTER TABLE tree_runs ADD COLUMN active_root TEXT REFERENCES tasks(id);
ALTER TABLE tree_runs ADD COLUMN continuation_node TEXT REFERENCES tasks(id);
ALTER TABLE tree_runs ADD COLUMN round_turns_used INTEGER NOT NULL DEFAULT 0;
ALTER TABLE clarifications ADD COLUMN round INTEGER NOT NULL DEFAULT 1;
ALTER TABLE operations ADD COLUMN round INTEGER NOT NULL DEFAULT 1;
CREATE TABLE node_messages (
    task TEXT NOT NULL REFERENCES tasks(id),
    role TEXT NOT NULL CHECK(role IN ('assistant','user')),
    content TEXT NOT NULL
);
CREATE INDEX node_chat ON node_messages(task);
CREATE TABLE tree_rounds (
    root TEXT NOT NULL REFERENCES tree_runs(root),
    round INTEGER NOT NULL,
    subtree_root TEXT NOT NULL REFERENCES tasks(id),
    prompt TEXT NOT NULL,
    check_command TEXT,
    outcome TEXT CHECK(outcome IN ('done','failed','unknown','cancelled')),
    verification TEXT,
    result TEXT,
    continuation_node TEXT REFERENCES tasks(id),
    PRIMARY KEY (root, round)
);
UPDATE tree_runs SET
    active_root = root,
    round_turns_used = COALESCE((SELECT turns_used FROM tasks WHERE id = tree_runs.root), 0),
    continuation_node = CASE
        WHEN phase = 'complete' AND outcome <> 'unknown' AND (
            SELECT count(*) FROM tasks t
            WHERE t.root = tree_runs.root
              AND NOT EXISTS (SELECT 1 FROM tasks c WHERE c.parent = t.id)
        ) = 1 THEN (
            SELECT t.id FROM tasks t
            WHERE t.root = tree_runs.root
              AND NOT EXISTS (SELECT 1 FROM tasks c WHERE c.parent = t.id)
            LIMIT 1
        )
        ELSE NULL
    END;
INSERT INTO tree_rounds(root, round, subtree_root, prompt, check_command, outcome, result, continuation_node)
    SELECT g.root, g.round, g.root, t.prompt, t.check_command, g.outcome, g.result, g.continuation_node
    FROM tree_runs g JOIN tasks t ON t.id = g.root;
";
const MAX_CHILDREN: usize = 64;
// Leave room in the daemon frame and provider request for JSON encoding, ids and status.
const PUBLIC_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const INTENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_AGENT_BYTES: usize = 128;
const DIGEST_MAX_BYTES: usize = 4096;
const WB_ENTRY_BYTES: usize = 64 * 1024;
const WB_BOARD_BYTES: usize = 1024 * 1024;

pub(super) const MIGRATE_V7: &str = "
ALTER TABLE root_policies ADD COLUMN sandbox TEXT NOT NULL DEFAULT 'direct' CHECK(sandbox IN ('direct','staged'));
ALTER TABLE tasks ADD COLUMN wb_wait_key TEXT;
ALTER TABLE tasks ADD COLUMN wb_wait_seq INTEGER;
ALTER TABLE tasks ADD COLUMN wb_seen_seq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tree_runs ADD COLUMN wb_seq INTEGER NOT NULL DEFAULT 0;
CREATE TABLE whiteboard_entries (
    root TEXT NOT NULL REFERENCES tree_runs(root),
    key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('note','contract','claim','question','decision')),
    content TEXT NOT NULL,
    author_task TEXT,
    author_agent TEXT NOT NULL,
    round INTEGER NOT NULL DEFAULT 1,
    version INTEGER NOT NULL DEFAULT 1,
    seq INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','resolved','archived')),
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (root, key)
);
CREATE INDEX whiteboard_order ON whiteboard_entries(root, seq);
";

fn unix_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn valid_wb_key(key: &str) -> Result<&str> {
    let trimmed = key.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_AGENT_BYTES
        || trimmed.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
    {
        return Err("whiteboard key must be 1..=128 bytes with no whitespace or control characters".into());
    }
    Ok(trimmed)
}

fn check_wb_content(db: &Connection, root: &str, key: &str, content: &str) -> Result<()> {
    if content.len() > WB_ENTRY_BYTES {
        return Err(format!("whiteboard content exceeds {WB_ENTRY_BYTES} bytes"));
    }
    let others: i64 = db
        .query_row(
            "SELECT coalesce(sum(length(CAST(content AS BLOB))),0) FROM whiteboard_entries WHERE root=?1 AND key<>?2",
            params![root, key],
            |r| r.get(0),
        )
        .db()?;
    if others.saturating_add(content.len() as i64) > WB_BOARD_BYTES as i64 {
        return Err(format!("whiteboard exceeds {WB_BOARD_BYTES} bytes; resolve or delete entries first"));
    }
    Ok(())
}

fn wb_row_json(r: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "key": r.get::<_, String>(0)?,
        "kind": r.get::<_, String>(1)?,
        "content": r.get::<_, String>(2)?,
        "author_task": r.get::<_, Option<String>>(3)?,
        "author_agent": r.get::<_, String>(4)?,
        "round": r.get::<_, i64>(5)?,
        "version": r.get::<_, i64>(6)?,
        "seq": r.get::<_, i64>(7)?,
        "status": r.get::<_, String>(8)?,
        "updated_at": r.get::<_, i64>(9)?
    }))
}

pub(super) fn whiteboard_entries(
    db: &Connection,
    root: &str,
    key: Option<&str>,
    prefix: Option<&str>,
    since_seq: Option<i64>,
) -> Result<(i64, Vec<Value>)> {
    let wb_seq: i64 = db.query_row("SELECT wb_seq FROM tree_runs WHERE root=?1", [root], |r| r.get(0)).db()?;
    let min_seq = since_seq.unwrap_or(-1);
    let mut q = db
        .prepare(
            "SELECT key,kind,content,author_task,author_agent,round,version,seq,status,updated_at
             FROM whiteboard_entries WHERE root=?1 AND seq>?2 ORDER BY seq",
        )
        .db()?;
    let rows = q.query_map(params![root, min_seq], wb_row_json).db()?;
    let mut out = Vec::new();
    for row in rows {
        let entry = row.db()?;
        let entry_key = entry["key"].as_str().unwrap_or("");
        if let Some(k) = key {
            if entry_key != k {
                continue;
            }
        }
        if let Some(p) = prefix {
            if !entry_key.starts_with(p) {
                continue;
            }
        }
        out.push(entry);
    }
    Ok((wb_seq, out))
}

pub(super) fn whiteboard_digest(db: &Connection, root: &str, seen_seq: i64) -> Result<Option<(i64, String)>> {
    let (wb_seq, entries) = whiteboard_entries(db, root, None, None, Some(seen_seq))?;
    if wb_seq <= seen_seq {
        return Ok(None);
    }
    let unread = entries.len();
    let mut out = format!(
        "Collaborative Whiteboard Digest (seq: {wb_seq}, {unread} update(s) since your last read at seq {seen_seq}):\n"
    );
    if entries.is_empty() {
        out.push_str("(The whiteboard changed, but no updated entry remains; an entry was deleted.)\n");
        return Ok(Some((wb_seq, out)));
    }
    // Newest first: a bounded digest must never crowd out the most recent unread update.
    for (idx, e) in entries.iter().rev().enumerate() {
        let key = e["key"].as_str().unwrap_or("?");
        let kind = e["kind"].as_str().unwrap_or("note");
        let agent = e["author_agent"].as_str().unwrap_or("?");
        let ver = e["version"].as_i64().unwrap_or(1);
        let seq = e["seq"].as_i64().unwrap_or(0);
        let status = e["status"].as_str().unwrap_or("active");
        let first_line =
            e["content"].as_str().unwrap_or("").lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        let truncated_preview = if first_line.len() > 100 {
            format!("{}...", &first_line[..first_line.floor_char_boundary(100)])
        } else {
            first_line.to_owned()
        };
        let marker = if seq > seen_seq { "*" } else { " " };
        let line = format!("{marker} [{kind}] {key} (v{ver}, seq {seq}, @{agent}, {status}): {truncated_preview}\n");
        if out.len() + line.len() + 80 > DIGEST_MAX_BYTES {
            out.push_str(&format!(
                "... ({} older unread entries; call wb_read for full details)\n",
                entries.len() - idx
            ));
            break;
        }
        out.push_str(&line);
    }
    Ok(Some((wb_seq, out)))
}

pub(super) fn wake_wb_waiters(db: &Connection, root: &str) -> Result<()> {
    let info: Option<(String, i64, i64)> = db
        .query_row("SELECT phase,round,wb_seq FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .db()?;
    let Some((phase, round, wb_seq)) = info else {
        return Ok(());
    };
    if phase != "work" {
        return Ok(());
    }
    let waiters: Vec<(String, Option<String>, i64)> = {
        let mut q = db
            .prepare(
                "SELECT id,wb_wait_key,wb_wait_seq FROM tasks
                 WHERE root=?1 AND round=?2 AND status='waiting' AND wb_wait_seq IS NOT NULL ORDER BY rowid",
            )
            .db()?;
        let rows = q.query_map(params![root, round], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).db()?;
        rows.collect::<rusqlite::Result<_>>().db()?
    };
    if waiters.is_empty() {
        return Ok(());
    }
    let (_, all_entries) = whiteboard_entries(db, root, None, None, None)?;
    let mut woke_any = false;
    for (task_id, wait_key, wait_seq) in &waiters {
        if wb_seq <= *wait_seq {
            continue;
        }
        let matching: Vec<Value> = all_entries
            .iter()
            .filter(|e| {
                e["seq"].as_i64().unwrap_or(0) > *wait_seq
                    && wait_key.as_deref().is_none_or(|k| {
                        let ek = e["key"].as_str().unwrap_or("");
                        ek == k || ek.starts_with(k)
                    })
            })
            .cloned()
            .collect();
        if !matching.is_empty() || wait_key.is_none() {
            db.execute("UPDATE tasks SET wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1", [task_id]).db()?;
            queue(db, task_id, json!({"whiteboard":true,"woke":"updated","seq":wb_seq,"entries":matching}))?;
            woke_any = true;
        }
    }
    if woke_any {
        return Ok(());
    }
    // Prevent mutual deadlock: if every active stage in the round is parked on wb_wait (no writer remains), wake all.
    let other_active: i64 = db
        .query_row(
            "SELECT count(*) FROM tasks WHERE root=?1 AND round=?2
             AND (status IN ('queued','running','approval') OR (status='waiting' AND wb_wait_seq IS NULL))",
            params![root, round],
            |r| r.get(0),
        )
        .db()?;
    if other_active == 0 {
        for (task_id, _, _) in waiters {
            db.execute("UPDATE tasks SET wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1", [&task_id]).db()?;
            queue(
                db,
                &task_id,
                json!({"whiteboard":true,"woke":"no_other_writers","seq":wb_seq,"entries":all_entries}),
            )?;
        }
    }
    Ok(())
}

/// A delete advances `wb_seq` but removes the row, so the normal wake scan cannot see it.
/// Wake matching waiters while the key still exists, before the row is removed.
fn wake_deleted_waiters(db: &Connection, root: &str, key: &str, seq: i64) -> Result<()> {
    let waiters: Vec<(String, Option<String>)> = {
        let mut q = db
            .prepare(
                "SELECT id,wb_wait_key FROM tasks
                 WHERE root=?1 AND status='waiting' AND wb_wait_seq IS NOT NULL AND wb_wait_seq < ?2
                 ORDER BY rowid",
            )
            .db()?;
        let rows = q.query_map(params![root, seq], |r| Ok((r.get(0)?, r.get(1)?))).db()?;
        rows.collect::<rusqlite::Result<_>>().db()?
    };
    for (task_id, wait_key) in waiters {
        if wait_key.as_deref().is_none_or(|k| key == k || key.starts_with(k)) {
            db.execute("UPDATE tasks SET wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1", [&task_id]).db()?;
            queue(
                db,
                &task_id,
                json!({
                    "whiteboard": true,
                    "woke": "updated",
                    "seq": seq,
                    "entries": [],
                    "deleted": key
                }),
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wb_write_entry(
    db: &Connection,
    root: &str,
    round: i64,
    author_task: Option<&str>,
    author_agent: &str,
    raw_key: &str,
    mode: &str,
    kind: Option<&str>,
    content: Option<&str>,
    expected_version: Option<i64>,
) -> Result<Value> {
    let key = valid_wb_key(raw_key)?;
    if !matches!(mode, "set" | "append" | "resolve" | "delete") {
        return Err("whiteboard write mode must be set, append, resolve, or delete".into());
    }
    if let Some(k) = kind {
        if !matches!(k, "note" | "contract" | "claim" | "question" | "decision") {
            return Err("whiteboard kind must be note, contract, claim, question, or decision".into());
        }
    }
    let existing: Option<(String, String, i64)> = db
        .query_row(
            "SELECT kind,content,version FROM whiteboard_entries WHERE root=?1 AND key=?2",
            params![root, key],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .db()?;
    let current_version = existing.as_ref().map_or(0, |e| e.2);
    if matches!(mode, "set" | "delete") {
        if let Some(expected) = expected_version {
            if expected != current_version {
                let current_entry = db
                    .query_row(
                        "SELECT key,kind,content,author_task,author_agent,round,version,seq,status,updated_at
                         FROM whiteboard_entries WHERE root=?1 AND key=?2",
                        params![root, key],
                        wb_row_json,
                    )
                    .optional()
                    .db()?;
                return Ok(json!({
                    "whiteboard": true,
                    "op": "wb_write",
                    "ok": false,
                    "conflict": true,
                    "key": key,
                    "expected_version": expected,
                    "actual_version": current_version,
                    "current": current_entry,
                }));
            }
        }
    }
    let now = unix_secs();
    let seq: i64 = db
        .query_row("UPDATE tree_runs SET wb_seq=wb_seq+1 WHERE root=?1 RETURNING wb_seq", [root], |r| r.get(0))
        .db()?;
    match mode {
        "set" => {
            let text = nonempty(content.ok_or("wb_write set requires content")?, "whiteboard content")?;
            check_wb_content(db, root, key, text)?;
            let eff_kind = kind.unwrap_or_else(|| existing.as_ref().map_or("note", |e| e.0.as_str()));
            db.execute(
                "INSERT INTO whiteboard_entries(root,key,kind,content,author_task,author_agent,round,version,seq,status,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8,'active',?9)
                 ON CONFLICT(root,key) DO UPDATE SET
                     kind=excluded.kind, content=excluded.content, author_task=excluded.author_task,
                     author_agent=excluded.author_agent, round=excluded.round,
                     version=whiteboard_entries.version+1, seq=excluded.seq, status='active', updated_at=excluded.updated_at",
                params![root, key, eff_kind, text, author_task, author_agent, round, seq, now],
            )
            .db()?;
        }
        "append" => {
            let text = nonempty(content.ok_or("wb_write append requires content")?, "whiteboard content")?;
            let addition = format!("[@{author_agent} · r{round}]\n{text}");
            let merged = match &existing {
                Some((_, prev, _)) if !prev.is_empty() => format!("{prev}\n\n{addition}"),
                _ => addition,
            };
            check_wb_content(db, root, key, &merged)?;
            let eff_kind = kind.unwrap_or_else(|| existing.as_ref().map_or("note", |e| e.0.as_str()));
            db.execute(
                "INSERT INTO whiteboard_entries(root,key,kind,content,author_task,author_agent,round,version,seq,status,updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8,'active',?9)
                 ON CONFLICT(root,key) DO UPDATE SET
                     kind=excluded.kind, content=excluded.content, author_task=excluded.author_task,
                     author_agent=excluded.author_agent, round=excluded.round,
                     version=whiteboard_entries.version+1, seq=excluded.seq, status='active', updated_at=excluded.updated_at",
                params![root, key, eff_kind, merged, author_task, author_agent, round, seq, now],
            )
            .db()?;
        }
        "resolve" => {
            let Some((_, prev, _)) = existing else {
                return Err(format!("unknown whiteboard key: {key}"));
            };
            let updated = match content.map(str::trim).filter(|s| !s.is_empty()) {
                Some(note) => format!("{prev}\n\n[resolved by @{author_agent} · r{round}]\n{note}"),
                None => prev,
            };
            check_wb_content(db, root, key, &updated)?;
            db.execute(
                "UPDATE whiteboard_entries SET status='resolved',content=?3,version=version+1,seq=?4,updated_at=?5 WHERE root=?1 AND key=?2",
                params![root, key, updated, seq, now],
            )
            .db()?;
        }
        "delete" => {
            if existing.is_none() {
                return Err(format!("unknown whiteboard key: {key}"));
            }
            wake_deleted_waiters(db, root, key, seq)?;
            db.execute("DELETE FROM whiteboard_entries WHERE root=?1 AND key=?2", params![root, key]).db()?;
        }
        _ => unreachable!(),
    }
    if let Some(task) = author_task {
        // Do not mark unrelated earlier entries as read just because this worker wrote.
        // Only consume the author's own new sequence when it had already seen the board.
        let seen: i64 = db.query_row("SELECT wb_seen_seq FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
        if seen >= seq.saturating_sub(1) {
            db.execute("UPDATE tasks SET wb_seen_seq=?2 WHERE id=?1", params![task, seq]).db()?;
        }
    }
    let saved = db
        .query_row(
            "SELECT key,kind,content,author_task,author_agent,round,version,seq,status,updated_at
             FROM whiteboard_entries WHERE root=?1 AND key=?2",
            params![root, key],
            wb_row_json,
        )
        .optional()
        .db()?;
    wake_wb_waiters(db, root)?;
    Ok(json!({"whiteboard":true,"op":"wb_write","ok":true,"mode":mode,"key":key,"seq":seq,"entry":saved}))
}

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

fn insert_child(
    db: &Connection,
    root: &str,
    parent: &str,
    round: i64,
    target: &str,
    prompt: &str,
    context: &str,
) -> Result<()> {
    db.execute(
        "INSERT INTO tasks(id,root,parent,agent,prompt,handoff_context,status,round) VALUES (?1,?2,?3,?4,?5,?6,'queued',?7)",
        params![id(), root, parent, target, prompt, context, round],
    )
    .db()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handoff(
    db: &Connection,
    root: &str,
    parent: &str,
    round: i64,
    context: &str,
    children: &Value,
    inherit: bool,
    action: &Value,
    meta: Option<&Value>,
) -> Result<()> {
    let children = children.as_array().ok_or("expected handoff children")?;
    if !(1..=MAX_CHILDREN).contains(&children.len()) {
        return Err(format!("handoff must name 1 to {MAX_CHILDREN} children"));
    }
    let mut validated = Vec::with_capacity(children.len());
    let mut batch_agents = HashSet::with_capacity(children.len());
    for child in children {
        let target = nonempty(field(child, "agent")?, "agent name")?;
        let prompt = nonempty(field(child, "text")?, "child assignment")?;
        if target.len() > MAX_AGENT_BYTES {
            return Err("child agent name exceeds 128 bytes".into());
        }
        if target.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control()) {
            return Err("child agent name must not contain whitespace or control characters".into());
        }
        if !batch_agents.insert(target) {
            return Err(format!("member {target} already has a stage in this round"));
        }
        let used: bool = db
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM tasks WHERE root=?1 AND round=?2 AND agent=?3)",
                params![root, round, target],
                |r| r.get(0),
            )
            .db()?;
        if used {
            return Err(format!("member {target} already has a stage in this round"));
        }
        // Existing public members can take new work alongside arbitrary named workers.
        db.execute("INSERT OR IGNORE INTO agents(name) VALUES (?1)", [target]).db()?;
        check_member(db, target).map_err(|_| "child unavailable".to_owned())?;
        validated.push((target, prompt));
    }

    node_message_with_meta(db, parent, "assistant", &action.to_string(), meta)?;
    let (last_in, last_bytes, wb_seen): (i64, i64, i64) = db
        .query_row("SELECT last_input_tokens,last_history_bytes,wb_seen_seq FROM tasks WHERE id=?1", [parent], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .db()?;
    for (target, prompt) in validated {
        if inherit {
            let child_id = id();
            db.execute(
                "INSERT INTO tasks(id,root,parent,agent,prompt,handoff_context,status,round,last_input_tokens,last_history_bytes,wb_seen_seq)
                 VALUES (?1,?2,?3,?4,?5,NULL,'queued',?6,?7,?8,?9)",
                params![child_id, root, parent, target, prompt, round, last_in, last_bytes, wb_seen],
            )
            .db()?;
            db.execute(
                "INSERT INTO node_messages(task,role,content,meta)
                 SELECT ?1,role,content,meta FROM node_messages WHERE task=?2 ORDER BY rowid",
                params![child_id, parent],
            )
            .db()?;
            node_message(db, &child_id, "user", r#"{"handoff":"accepted"}"#)?;
        } else {
            insert_child(db, root, parent, round, target, prompt, context)?;
        }
    }
    Ok(())
}

pub(super) fn apply(db: &Connection, root: &str, task: &str, action: &Value, meta: Option<&Value>) -> Result<()> {
    let (round, active_root, wb_seq): (i64, String, i64) = db
        .query_row("SELECT round,COALESCE(active_root,root),wb_seq FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .db()?;
    let op = field(action, "op")?;
    if task == active_root && matches!(op, "members" | "wb_read" | "wb_write" | "wb_wait") {
        db.execute("UPDATE tree_runs SET directory_used=1 WHERE root=?1", [root]).db()?;
    }
    match op {
        "members" => {
            node_message_with_meta(db, task, "assistant", &action.to_string(), meta)?;
            return queue(db, task, json!({"members":members(db)?}));
        }
        "wb_read" => {
            let key = action.get("key").and_then(Value::as_str);
            let prefix = action.get("prefix").and_then(Value::as_str);
            let since_seq = action.get("since_seq").and_then(Value::as_i64);
            let (seq, entries) = whiteboard_entries(db, root, key, prefix, since_seq)?;
            node_message_with_meta(db, task, "assistant", &action.to_string(), meta)?;
            // Only an unfiltered read covering up through `wb_seen_seq` is a complete watermark;
            // incremental `since_seq <= wb_seen_seq` also guarantees no skipped intermediate entries.
            if key.is_none() && prefix.is_none() {
                let current_seen: i64 =
                    db.query_row("SELECT wb_seen_seq FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
                if since_seq.is_none_or(|s| s <= current_seen) {
                    db.execute("UPDATE tasks SET wb_seen_seq=?2 WHERE id=?1", params![task, seq]).db()?;
                }
            }
            return queue(db, task, json!({"whiteboard":true,"op":"wb_read","seq":seq,"entries":entries}));
        }
        "wb_write" => {
            let key = field(action, "key")?;
            let mode = action.get("mode").and_then(Value::as_str).unwrap_or("set");
            let kind = action.get("kind").and_then(Value::as_str);
            let content = action.get("content").and_then(Value::as_str);
            let expected_version = action.get("expected_version").and_then(Value::as_i64);
            let author_agent: String =
                db.query_row("SELECT agent FROM tasks WHERE id=?1", [task], |r| r.get(0)).db()?;
            let receipt =
                wb_write_entry(db, root, round, Some(task), &author_agent, key, mode, kind, content, expected_version)?;
            node_message_with_meta(db, task, "assistant", &action.to_string(), meta)?;
            return queue(db, task, receipt);
        }
        "wb_wait" => {
            let wait_key = action.get("key").and_then(Value::as_str).map(valid_wb_key).transpose()?;
            let since_seq = action.get("since_seq").and_then(Value::as_i64).unwrap_or(wb_seq);
            node_message_with_meta(db, task, "assistant", &action.to_string(), meta)?;
            db.execute(
                "UPDATE tasks SET status='waiting',wb_wait_key=?2,wb_wait_seq=?3 WHERE id=?1",
                params![task, wait_key, since_seq],
            )
            .db()?;
            wake_wb_waiters(db, root)?;
            return advance(db, root);
        }
        "ask" if task == active_root => {
            let text = nonempty(field(action, "text")?, "stage text")?;
            let allowed = db
                .execute(
                    "UPDATE tree_runs SET question=?2 WHERE root=?1 AND directory_used=0
                AND NOT EXISTS (SELECT 1 FROM operations WHERE task=?3 AND round=?4)",
                    params![root, text, task, round],
                )
                .db()?;
            if allowed == 0 {
                return Err("root may clarify only before using tools".into());
            }
            db.execute("UPDATE tasks SET status='waiting' WHERE id=?1", [task]).db()?;
        }
        "handoff" => {
            let text = nonempty(field(action, "text")?, "stage text")?;
            let inherit = match action.get("inherit_context") {
                None | Some(Value::Null) => true,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err("handoff inherit_context must be a boolean".into()),
            };
            handoff(db, root, task, round, text, &action["children"], inherit, action, meta)?;
            db.execute("UPDATE tasks SET status='done',result=?2 WHERE id=?1", params![task, text]).db()?;
            return advance(db, root);
        }
        "finish" => {
            let text = nonempty(field(action, "text")?, "stage text")?;
            db.execute("UPDATE tasks SET status='done',result=?2 WHERE id=?1", params![task, text]).db()?;
        }
        other => return Err(format!("action {other} unavailable for this tree stage")),
    }
    node_message_with_meta(db, task, "assistant", &action.to_string(), meta)?;
    advance(db, root)
}

impl Store {
    pub fn answer(&mut self, root: &str, text: &str) -> Result<()> {
        nonempty(text, "answer")?;
        self.transaction(|db| {
            let active_root: Option<String> = db
                .query_row(
                    "SELECT COALESCE(active_root,root) FROM tree_runs WHERE root=?1 AND phase='work' AND question IS NOT NULL",
                    [root],
                    |r| r.get(0),
                )
                .optional()
                .db()?;
            let Some(active_root) = active_root else {
                return Err("root is not waiting for an answer".into());
            };
            db.execute(
                "INSERT INTO clarifications(root,question,answer,round) SELECT root,question,?2,round FROM tree_runs WHERE root=?1 AND phase='work' AND question IS NOT NULL",
                params![root, text],
            )
            .db()?;
            db.execute("UPDATE tree_runs SET question=NULL WHERE root=?1", [root]).db()?;
            queue(db, &active_root, json!({"answer":text}))
        })
    }

    /// Daemon I/O may return after cancellation. Check the actual job and release its reservation.
    pub fn discard_late_completion(&mut self, task: &str) -> Result<bool> {
        self.transaction(|db| {
            let stale = db
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM tree_runs WHERE reporter=?1 AND phase='reporting')
                        AND NOT EXISTS(SELECT 1 FROM tasks WHERE id=?1 AND status='running' AND model_reserved=1)",
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
    let round: i64 = db.query_row("SELECT round FROM tree_runs WHERE root=?1", [root], |r| r.get(0)).db()?;
    let (count, bytes): (u32, i64) = db
        .query_row(
            "SELECT count(*),coalesce(sum(length(CAST(t.result AS BLOB))),0)
        FROM tasks t JOIN tree_runs g ON g.root=t.root WHERE t.root=?1 AND t.round=?2 AND t.status='done'
        AND (t.kind IS NULL OR t.id=COALESCE(g.active_root,g.root))
        AND NOT EXISTS(SELECT 1 FROM tasks c WHERE c.parent=t.id AND c.round=?2)",
            params![root, round],
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
        CASE WHEN t.id=COALESCE(g.active_root,g.root) AND g.question IS NOT NULL THEN 'input' ELSE t.status END,
        CASE WHEN t.kind='summarizer' THEN 'summarizer' WHEN t.id=g.root THEN 'root' WHEN a.internal THEN 'repair' ELSE 'node' END,
        t.round=?2 AND (t.kind IS NULL OR t.id=COALESCE(g.active_root,g.root)) AND NOT EXISTS(SELECT 1 FROM tasks c WHERE c.parent=t.id AND c.round=?2), t.result
        FROM tasks t JOIN tree_runs g ON g.root=t.root JOIN agents a ON a.name=t.agent
        WHERE t.root=?1 ORDER BY t.rowid",
        )
        .db()?;
    let mut rows = q.query(params![root, round]).db()?;
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

fn verification_for_round(db: &Connection, active_root: &str, round: i64) -> Result<String> {
    db.query_row(
        "SELECT CASE WHEN t.check_command IS NULL THEN 'not_requested' ELSE coalesce((
        SELECT CASE WHEN state='unknown' THEN 'unknown' WHEN ok THEN 'passed' ELSE 'failed' END
        FROM operations WHERE task=?1 AND round=?2 AND state IN ('done','unknown') AND json_extract(action,'$.op')='verify'
        ORDER BY rowid DESC LIMIT 1),'not_run') END
        FROM tasks t WHERE t.id=?1",
        params![active_root, round],
        |r| r.get(0),
    )
    .db()
}

fn settle_or_report(db: &Connection, root: &str, round: i64, active_root: &str, outcome: &str) -> Result<()> {
    let mut q = db
        .prepare(
            "SELECT t.id,t.status,t.result FROM tasks t
             WHERE t.root=?1 AND t.round=?2
               AND (t.kind IS NULL OR t.id=?3)
               AND NOT EXISTS(SELECT 1 FROM tasks c WHERE c.parent=t.id AND c.round=?2)
             ORDER BY t.rowid",
        )
        .db()?;
    let leaves: Vec<(String, String, Option<String>)> = q
        .query_map(params![root, round, active_root], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .db()?
        .collect::<rusqlite::Result<_>>()
        .db()?;
    drop(q);
    if leaves.len() == 1 {
        let (leaf_id, leaf_status, leaf_result) = &leaves[0];
        let verification = verification_for_round(db, active_root, round)?;
        let result = if outcome == "done" {
            leaf_result.clone().unwrap_or_default()
        } else if verification == "unknown" {
            format!(
                "No verified result (submitted check outcome is unknown). Leaf result: {}",
                leaf_result.as_deref().unwrap_or("(none)")
            )
        } else if verification == "failed" {
            format!("Submitted check failed. Leaf result: {}", leaf_result.as_deref().unwrap_or("(none)"))
        } else {
            leaf_result.clone().unwrap_or_else(|| format!("Stage ended with status {leaf_status}."))
        };
        let continuation = (outcome != "unknown").then_some(leaf_id.as_str());
        db.execute(
            "UPDATE tree_runs SET phase='complete',outcome=?2,result=?3,continuation_node=?4 WHERE root=?1",
            params![root, outcome, result, continuation],
        )
        .db()?;
        db.execute(
            "UPDATE tree_rounds SET outcome=?3,verification=?4,result=?5,continuation_node=?6 WHERE root=?1 AND round=?2",
            params![root, round, outcome, verification, result, continuation],
        )
        .db()?;
        return Ok(());
    }
    queue_report(db, root, outcome)
}

/// Called in the same write transaction as every work-stage settlement. Siblings settle independently.
pub(super) fn advance(db: &Connection, root: &str) -> Result<()> {
    wake_wb_waiters(db, root)?;
    let (phase, round, active_root): (String, i64, String) = db
        .query_row("SELECT phase,round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .db()?;
    if phase != "work" {
        return Ok(());
    }
    let (active, unknown, failed): (bool, bool, bool) = db
        .query_row(
            "SELECT max(status IN ('queued','running','waiting','approval')), max(status='unknown'), max(status IN ('failed','cancelled'))
             FROM tasks WHERE root=?1 AND round=?2",
            params![root, round],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .db()?;
    if active {
        return Ok(());
    }
    if unknown {
        return settle_or_report(db, root, round, &active_root, "unknown");
    }
    if failed {
        return settle_or_report(db, root, round, &active_root, "failed");
    }
    let check: Option<String> =
        db.query_row("SELECT check_command FROM tasks WHERE id=?1", [&active_root], |r| r.get(0)).db()?;
    if let Some(command) = check {
        // Do not change the finished root stage or append check output to its private history.
        db.execute(
            "INSERT INTO operations(id,task,action,state,round) VALUES (?1,?2,?3,'ready',?4)",
            params![id(), active_root, json!({"op":"verify","command":command}).to_string(), round],
        )
        .db()?;
        db.execute("UPDATE tree_runs SET phase='check' WHERE root=?1", [root]).db()?;
        return Ok(());
    }
    settle_or_report(db, root, round, &active_root, "done")
}

fn intent(db: &Connection, root: &str, round: i64, active_root: &str) -> Result<Option<Value>> {
    let bytes: i64 = db
        .query_row(
            "SELECT coalesce((SELECT length(CAST(prompt AS BLOB)) FROM tree_rounds WHERE root=?1 AND round=?2), length(CAST(prompt AS BLOB))) +
        (SELECT coalesce(sum(length(CAST(question AS BLOB))+length(CAST(answer AS BLOB))+32),0) FROM clarifications WHERE root=?1 AND round=?2)
        FROM tasks WHERE id=?3",
            params![root, round, active_root],
            |r| r.get(0),
        )
        .db()?;
    if bytes > INTENT_BYTES as i64 {
        return Ok(None);
    }
    let prompt: String = db
        .query_row(
            "SELECT coalesce((SELECT prompt FROM tree_rounds WHERE root=?1 AND round=?2), (SELECT prompt FROM tasks WHERE id=?3))",
            params![root, round, active_root],
            |r| r.get(0),
        )
        .db()?;
    let mut q =
        db.prepare("SELECT question,answer FROM clarifications WHERE root=?1 AND round=?2 ORDER BY rowid").db()?;
    let answers = q
        .query_map(params![root, round], |r| {
            Ok(json!({"question":r.get::<_,String>(0)?,"answer":r.get::<_,String>(1)?}))
        })
        .db()?;
    let answers: Vec<Value> = answers.collect::<rusqlite::Result<_>>().db()?;
    Ok(Some(json!({"intent":prompt,"clarifications":answers})))
}

pub(super) fn finish_check(db: &Connection, root: &str, ok: bool, diagnostics: &str) -> Result<()> {
    let (round, active_root): (i64, String) = db
        .query_row("SELECT round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .db()?;
    if ok {
        return settle_or_report(db, root, round, &active_root, "done");
    }
    let attempt: u32 = db
        .query_row(
            "SELECT count(*) FROM operations WHERE task=?1 AND round=?2 AND state='done' AND json_extract(action,'$.op')='verify'",
            params![active_root, round],
            |r| r.get(0),
        )
        .db()?;
    if attempt >= MAX_CHECK_ATTEMPTS {
        return settle_or_report(db, root, round, &active_root, "failed");
    }
    // Repairs are forward system stages with private identities. They do not consume a persistent member slot.
    let Some(mut context) = intent(db, root, round, &active_root)? else {
        return settle_or_report(db, root, round, &active_root, "failed");
    };
    let repair = format!("repair-{}", id());
    db.execute("INSERT INTO agents(name,internal) VALUES (?1,1)", [&repair]).db()?;
    context["tool"] = json!("verify");
    context["ok"] = json!(false);
    context["result"] = json!(diagnostics);
    context["attempt"] = json!(attempt);
    context["max_attempts"] = json!(MAX_CHECK_ATTEMPTS);
    let command: String =
        db.query_row("SELECT check_command FROM tasks WHERE id=?1", [&active_root], |r| r.get(0)).db()?;
    context["command"] = json!(command);
    insert_child(
        db,
        root,
        &active_root,
        round,
        &repair,
        "Repair the failed submitted check, then finish or hand off the remaining work.",
        &context.to_string(),
    )?;
    db.execute("UPDATE tree_runs SET phase='work' WHERE root=?1", [root]).db()?;
    Ok(())
}

fn report_outcome(db: &Connection, root: &str) -> Result<(String, String)> {
    let (outcome, round, active_root): (String, i64, String) = db
        .query_row("SELECT outcome,round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .db()?;
    Ok((outcome, verification_for_round(db, &active_root, round)?))
}

fn bounded_whiteboard_snapshot(db: &Connection, root: &str) -> Result<Vec<Value>> {
    let (_, entries) = whiteboard_entries(db, root, None, None, None)?;
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let count = entries.len();
    let mut kept = Vec::new();
    let mut total = 0usize;
    for (idx, entry) in entries.into_iter().enumerate() {
        let len = entry.to_string().len();
        if total + len > PUBLIC_OUTPUT_BYTES {
            kept.push(json!({
                "truncated": true,
                "omitted_entries": count - idx
            }));
            break;
        }
        total += len;
        kept.push(entry);
    }
    Ok(kept)
}

fn report_packet(db: &Connection, root: &str) -> Result<Option<Value>> {
    let (round, active_root): (i64, String) = db
        .query_row("SELECT round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .db()?;
    let leaves = outputs(db, root)?;
    if leaves.truncated {
        return Ok(None);
    }
    let Some(mut input) = intent(db, root, round, &active_root)? else {
        return Ok(None);
    };
    input["leaf_results"] = leaves.leaves;
    let (outcome, verification) = report_outcome(db, root)?;
    input["outcome"] = json!(outcome);
    input["verification"] = json!(verification);
    let wb = bounded_whiteboard_snapshot(db, root)?;
    if !wb.is_empty() {
        input["whiteboard"] = Value::Array(wb);
    }
    Ok(Some(input))
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
        let (round, active_root): (i64, String) = db
            .query_row("SELECT round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [&root], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .db()?;
        let leaves = outputs(db, &root)?;
        let Some(mut input) = intent(db, &root, round, &active_root)? else {
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
        let wb = bounded_whiteboard_snapshot(db, &root)?;
        if !wb.is_empty() {
            input["whiteboard"] = Value::Array(wb);
        }
        db.execute("UPDATE tree_runs SET round_turns_used=round_turns_used+1 WHERE root=?1", [&root]).db()?;
        db.execute(
            "UPDATE tasks SET turns_used=(SELECT round_turns_used FROM tree_runs WHERE root=?1) WHERE id=?1",
            [&root],
        )
        .db()?;
        db.execute("UPDATE tree_runs SET phase='reporting' WHERE root=?1", [&root]).db()?;
        let messages = vec![json!({"role":"user","content":input.to_string()})];
        let settings = root_model_settings(db, &root)?;
        return Ok(Some(Turn {
            kind: TurnKind::Reporter,
            root,
            task,
            agent: "reporter".into(),
            messages,
            can_compact: false,
            settings,
            last_input_tokens: 0,
            last_history_bytes: 0,
        }));
    }
}

struct SummarizerNode<'a> {
    root: &'a str,
    round: i64,
    active_root: &'a str,
    reporter_id: &'a str,
    status: &'a str,
    result: &'a str,
    input_json: &'a str,
    assistant_json: Option<&'a str>,
    assistant_meta: Option<&'a Value>,
}

fn create_summarizer_node(db: &Connection, node: &SummarizerNode<'_>) -> Result<()> {
    let agent_name = format!("summarizer-{}", node.reporter_id);
    let prompt: String = db
        .query_row(
            "SELECT coalesce((SELECT prompt FROM tree_rounds WHERE root=?1 AND round=?2), (SELECT prompt FROM tasks WHERE id=?3))",
            params![node.root, node.round, node.active_root],
            |r| r.get(0),
        )
        .db()?;
    db.execute("INSERT OR IGNORE INTO agents(name,internal) VALUES (?1,1)", [&agent_name]).db()?;
    db.execute(
        "INSERT INTO tasks(id,root,parent,agent,prompt,status,introduced,result,round,kind) VALUES (?1,?2,?3,?4,?5,?6,1,?7,?8,'summarizer')",
        params![node.reporter_id, node.root, node.active_root, agent_name, prompt, node.status, node.result, node.round],
    )
    .db()?;
    node_message(db, node.reporter_id, "user", node.input_json)?;
    if let Some(asst) = node.assistant_json {
        node_message_with_meta(db, node.reporter_id, "assistant", asst, node.assistant_meta)?;
    }
    Ok(())
}

pub(super) fn finish_report(db: &Connection, root: &str, action: &Value, meta: Option<&Value>) -> Result<()> {
    if field(action, "op")? != "finish" {
        return Err("reporter may only finish".into());
    }
    let text = nonempty(field(action, "text")?, "report text")?;
    let (reporter_id, round, active_root, outcome): (String, i64, String, String) = db
        .query_row(
            "SELECT reporter,round,COALESCE(active_root,root),outcome FROM tree_runs WHERE root=?1 AND phase='reporting'",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .db()?
        .ok_or("reporter is no longer active")?;
    let packet = report_packet(db, root)?.unwrap_or_else(|| json!({"outcome": outcome}));
    let verification = verification_for_round(db, &active_root, round)?;
    let continuation = (outcome != "unknown").then_some(reporter_id.as_str());
    create_summarizer_node(
        db,
        &SummarizerNode {
            root,
            round,
            active_root: &active_root,
            reporter_id: &reporter_id,
            status: "done",
            result: text,
            input_json: &packet.to_string(),
            assistant_json: Some(&action.to_string()),
            assistant_meta: meta,
        },
    )?;
    db.execute(
        "UPDATE tree_runs SET phase='complete',result=?2,continuation_node=?3 WHERE root=?1 AND phase='reporting'",
        params![root, text, continuation],
    )
    .db()?;
    db.execute(
        "UPDATE tree_rounds SET outcome=?3,verification=?4,result=?5,continuation_node=?6 WHERE root=?1 AND round=?2",
        params![root, round, outcome, verification, text, continuation],
    )
    .db()?;
    Ok(())
}

pub(super) fn fallback(db: &Connection, root: &str, reason: &str) -> Result<()> {
    let row: Option<(String, i64, String)> = db
        .query_row(
            "SELECT reporter,round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1 AND phase IN ('report_ready','reporting')",
            [root],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .db()?;
    let Some((reporter_id, round, active_root)) = row else {
        return Ok(());
    };
    let leaves = outputs(db, root)?.leaves;
    let (outcome, verification) = report_outcome(db, root)?;
    let final_outcome = if outcome == "done" { "failed" } else { outcome.as_str() };
    let summary =
        format!("Partial report ({reason}). Work: {outcome}. Verification: {verification}. Leaf results: {leaves}");
    let context = report_packet(db, root)?
        .unwrap_or_else(|| json!({"outcome": final_outcome, "verification": verification, "leaf_results": leaves}));
    let continuation = (final_outcome != "unknown").then_some(reporter_id.as_str());
    create_summarizer_node(
        db,
        &SummarizerNode {
            root,
            round,
            active_root: &active_root,
            reporter_id: &reporter_id,
            status: "failed",
            result: &summary,
            input_json: &context.to_string(),
            assistant_json: None,
            assistant_meta: None,
        },
    )?;
    db.execute(
        "UPDATE tree_runs SET phase='complete',outcome=?2,result=?3,continuation_node=?4 WHERE root=?1 AND phase IN ('report_ready','reporting')",
        params![root, final_outcome, summary, continuation],
    )
    .db()?;
    db.execute(
        "UPDATE tree_rounds SET outcome=?3,verification=?4,result=?5,continuation_node=?6 WHERE root=?1 AND round=?2",
        params![root, round, final_outcome, verification, summary, continuation],
    )
    .db()?;
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
    let (round, active_root): (i64, String) = db
        .query_row("SELECT round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .db()?;
    let check: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM operations WHERE task=?1 AND round=?2 AND state='unknown' AND json_extract(action,'$.op')='verify')",
            params![task, round],
            |r| r.get(0),
        )
        .db()?;
    if check {
        return settle_or_report(db, root, round, &active_root, "unknown");
    }
    stop_work(db, task, "unknown", "tool was started without a durable result; outcome unknown")
}

pub(super) fn cancel(db: &Connection, root: &str, task: &str) -> Result<()> {
    let (phase, round, active_root): (String, i64, String) = db
        .query_row("SELECT phase,round,COALESCE(active_root,root) FROM tree_runs WHERE root=?1", [root], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .db()?;
    if phase == "complete" {
        return Err("tree is already terminal".into());
    }
    let active: Vec<String> = if task == root {
        let mut q = db
            .prepare(
                "SELECT id FROM tasks WHERE root=?1 AND round=?2 AND status IN ('queued','running','waiting','approval')",
            )
            .db()?;
        let rows = q.query_map(params![root, round], |r| r.get::<_, String>(0)).db()?;
        rows.collect::<rusqlite::Result<_>>().db()?
    } else {
        let mut q = db
            .prepare(
                "WITH RECURSIVE subtree(id) AS (SELECT ?1 UNION ALL SELECT t.id FROM tasks t JOIN subtree s ON t.parent=s.id WHERE t.round=?2)
                 SELECT t.id FROM tasks t JOIN subtree s ON t.id=s.id WHERE t.round=?2 AND t.status IN ('queued','running','waiting','approval')",
            )
            .db()?;
        let rows = q.query_map(params![task, round], |r| r.get::<_, String>(0)).db()?;
        rows.collect::<rusqlite::Result<_>>().db()?
    };
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
        db.execute(
            "UPDATE tasks SET status=?2,result=?3,wb_wait_key=NULL,wb_wait_seq=NULL WHERE id=?1",
            params![id, status, reason],
        )
        .db()?;
    }
    if task != root {
        return advance(db, root);
    }
    // Cancelling the whole root stops any pending round-level check on active_root too.
    cancel_operations(db, &active_root)?;
    let unknown: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM operations o JOIN tasks t ON t.id=o.task WHERE t.root=?1 AND o.state='unknown')",
            [root],
            |r| r.get(0),
        )
        .db()?;
    let (outcome, reason) = if unknown {
        ("unknown", "cancelled with an unknown tool outcome")
    } else {
        ("cancelled", "cancelled by user")
    };
    let sole_leaf: Option<String> = if outcome != "unknown" {
        let leaves: Vec<String> = {
            let mut lq = db
                .prepare(
                    "SELECT t.id FROM tasks t WHERE t.root=?1 AND t.round=?2
                     AND (t.kind IS NULL OR t.id=?3)
                     AND NOT EXISTS(SELECT 1 FROM tasks c WHERE c.parent=t.id AND c.round=?2)
                     ORDER BY t.rowid",
                )
                .db()?;
            let rows = lq.query_map(params![root, round, active_root], |r| r.get(0)).db()?;
            rows.collect::<rusqlite::Result<_>>().db()?
        };
        if leaves.len() == 1 {
            Some(leaves.into_iter().next().unwrap())
        } else {
            None
        }
    } else {
        None
    };
    let verification = verification_for_round(db, &active_root, round)?;
    db.execute(
        "UPDATE tree_runs SET phase='complete',question=NULL,outcome=?2,result=?3,continuation_node=?4 WHERE root=?1",
        params![root, outcome, reason, sole_leaf],
    )
    .db()?;
    db.execute(
        "UPDATE tree_rounds SET outcome=?3,verification=?4,result=?5,continuation_node=?6 WHERE root=?1 AND round=?2",
        params![root, round, outcome, verification, reason, sole_leaf],
    )
    .db()?;
    Ok(())
}

fn cancel_operations(db: &Connection, task: &str) -> Result<bool> {
    db.execute("UPDATE operations SET state='cancelled' WHERE task=?1 AND state IN ('approval','ready')", [task])
        .db()?;
    Ok(db.execute("UPDATE operations SET state='unknown' WHERE task=?1 AND state='started'", [task]).db()? > 0)
}
