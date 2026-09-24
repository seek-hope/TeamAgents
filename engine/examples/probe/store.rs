use super::{atomic_bytes, ensure, init_root, now_ms, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub struct Store {
    pub db: Connection,
    root: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        init_root(root)?;
        let db = Connection::open(root.join("session.sqlite"))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.busy_timeout(std::time::Duration::from_secs(2))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS inbox(id TEXT PRIMARY KEY, body TEXT NOT NULL, applied INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS context(seq INTEGER PRIMARY KEY, input_id TEXT UNIQUE NOT NULL REFERENCES inbox(id), body TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS execution(id TEXT PRIMARY KEY, phase TEXT NOT NULL, revision INTEGER NOT NULL);
             INSERT OR IGNORE INTO execution VALUES('instance','READY',0);
             INSERT OR IGNORE INTO execution VALUES('waiter','WAITING',0);
             CREATE TABLE IF NOT EXISTS artifacts(id TEXT PRIMARY KEY, digest TEXT NOT NULL, owner TEXT, state TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS artifact_refs(owner TEXT NOT NULL, artifact TEXT NOT NULL REFERENCES artifacts(id), PRIMARY KEY(owner,artifact));
             CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, state TEXT NOT NULL, cancel_requested INTEGER NOT NULL DEFAULT 0, receipt TEXT);
             CREATE TABLE IF NOT EXISTS events(seq INTEGER PRIMARY KEY, kind TEXT NOT NULL, payload TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS commands(id TEXT PRIMARY KEY, method TEXT NOT NULL, reply TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS demo(id TEXT PRIMARY KEY, status TEXT NOT NULL, ticks INTEGER NOT NULL);",
        )?;
        std::fs::create_dir_all(root.join("artifacts"))?;
        Ok(Self { db, root: root.into() })
    }

    pub fn ingest(&self, id: &str, body: &str) -> Result<()> {
        self.db.execute("INSERT OR IGNORE INTO inbox(id,body) VALUES(?1,?2)", params![id, body])?;
        let saved: String = self.db.query_row("SELECT body FROM inbox WHERE id=?1", [id], |r| r.get(0))?;
        ensure(saved == body, "the same input id carried a different payload")
    }

    pub fn apply(&mut self, id: &str, crash: Option<&str>) -> Result<()> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO context(input_id,body) SELECT id,body FROM inbox WHERE id=?1 AND applied=0",
            [id],
        )?;
        let changed = tx.execute("UPDATE inbox SET applied=1 WHERE id=?1 AND applied=0", [id])?;
        if changed != 0 {
            tx.execute("UPDATE execution SET phase='READY',revision=revision+1 WHERE id='instance'", [])?;
            tx.execute("INSERT INTO events(kind,payload) VALUES('input',?1)", [id])?;
        }
        if crash == Some("before") {
            kill_self();
        }
        tx.commit()?;
        if crash == Some("after") {
            kill_self();
        }
        Ok(())
    }

    pub fn reserve_artifact(&self, id: &str, owner: &str, bytes: &[u8]) -> Result<()> {
        validate_id(id)?;
        self.db.execute("INSERT INTO artifacts VALUES(?1,?2,?3,'STAGING')", params![id, hash(bytes), owner])?;
        Ok(())
    }

    pub fn publish(&self, id: &str, bytes: &[u8]) -> Result<()> {
        validate_id(id)?;
        let (digest, state): (String, String) =
            self.db
                .query_row("SELECT digest,state FROM artifacts WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        ensure(state == "STAGING" && digest == hash(bytes), "an unstaged artifact, or a digest mismatch")?;
        atomic_bytes(&self.root.join("artifacts").join(id), bytes)
    }

    pub fn attach(&mut self, id: &str, owner: &str) -> Result<()> {
        validate_id(id)?;
        let digest = hash(&std::fs::read(self.root.join("artifacts").join(id))?);
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (expected, state): (String, String) =
            tx.query_row("SELECT digest,state FROM artifacts WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        ensure(
            state == "STAGING" || state == "LIVE",
            "references cannot be added while the artifact is being collected",
        )?;
        ensure(expected == digest, "the artifact is missing or corrupt")?;
        tx.execute("INSERT OR IGNORE INTO artifact_refs VALUES(?1,?2)", params![owner, id])?;
        tx.execute("UPDATE artifacts SET state='LIVE',owner=NULL WHERE id=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn abandon(&self, id: &str) -> Result<()> {
        self.db.execute("UPDATE artifacts SET state='ABANDONED',owner=NULL WHERE id=?1 AND state='STAGING'", [id])?;
        Ok(())
    }

    pub fn claim_gc(&mut self) -> Result<Vec<String>> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE artifacts SET state='DELETING' WHERE state IN ('ABANDONED','LIVE') AND owner IS NULL AND NOT EXISTS(SELECT 1 FROM artifact_refs WHERE artifact=artifacts.id)", [])?;
        let ids = tx
            .prepare("SELECT id FROM artifacts WHERE state='DELETING' ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        tx.commit()?;
        Ok(ids)
    }

    pub fn collect(&mut self) -> Result<usize> {
        let ids = self.claim_gc()?;
        for id in &ids {
            match std::fs::remove_file(self.root.join("artifacts").join(id)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.db.execute("DELETE FROM artifacts WHERE id=?1 AND state='DELETING'", [id])?;
        }
        Ok(ids.len())
    }

    pub fn prepare_job(&self, id: &str) -> Result<()> {
        self.db.execute("INSERT OR IGNORE INTO operations(id,state) VALUES(?1,'PREPARED')", [id])?;
        Ok(())
    }

    pub fn dispatch(&self, id: &str) -> Result<()> {
        let changed = self.db.execute(
            "UPDATE operations SET state='DISPATCH_COMMITTED' WHERE id=?1 AND state='PREPARED' AND cancel_requested=0",
            [id],
        )?;
        ensure(changed == 1, "the operation is not dispatchable")
    }

    pub fn cancel(&mut self, id: &str) -> Result<()> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE operations SET cancel_requested=1 WHERE id=?1", [id])?;
        tx.execute("INSERT INTO events(kind,payload) VALUES('cancel',?1)", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn recovery_method(&self, id: &str) -> Result<&'static str> {
        let cancel: bool =
            self.db.query_row("SELECT cancel_requested FROM operations WHERE id=?1", [id], |r| r.get(0))?;
        Ok(if cancel { "cancel" } else { "go" })
    }

    pub fn import_receipt(&mut self, id: &str, receipt: &Value) -> Result<()> {
        let text = serde_json::to_string(receipt)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<String> =
            tx.query_row("SELECT receipt FROM operations WHERE id=?1", [id], |r| r.get(0))?;
        if let Some(previous) = previous {
            ensure(previous == text, "the same operation has a conflicting receipt")?;
        } else {
            tx.execute("UPDATE operations SET state='TERMINAL',receipt=?2 WHERE id=?1", params![id, text])?;
            tx.execute("INSERT INTO events(kind,payload) VALUES('receipt',?1)", [id])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn demo_command(&mut self, id: &str, method: &str) -> Result<Value> {
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cached: Option<(String, String)> = tx
            .query_row("SELECT method,reply FROM commands WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        if let Some((saved, reply)) = cached {
            ensure(saved == method, "the same command_id carried a different payload")?;
            return Ok(serde_json::from_str(&reply)?);
        }
        match method {
            "start" => {
                tx.execute("INSERT OR IGNORE INTO demo VALUES('p0-task','RUNNING',0)", [])?;
            }
            "pause" => {
                tx.execute("UPDATE demo SET status='PAUSED' WHERE status='RUNNING'", [])?;
            }
            "resume" => {
                tx.execute("UPDATE demo SET status='RUNNING' WHERE status='PAUSED'", [])?;
            }
            "cancel" => {
                tx.execute("UPDATE demo SET status='CANCELLED' WHERE status!='CANCELLED'", [])?;
            }
            "tick" => {
                tx.execute("UPDATE demo SET ticks=ticks+1 WHERE status='RUNNING'", [])?;
            }
            "status" => {}
            _ => return Err("unknown control method".into()),
        }
        if method != "status" {
            tx.execute("INSERT INTO events(kind,payload) VALUES('demo',?1)", [method])?;
        }
        let task: Option<(String, i64)> = tx
            .query_row("SELECT status,ticks FROM demo WHERE id='p0-task'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let seq: i64 = tx.query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))?;
        let result = json!({"ok":true,"version":1,"task_id":"p0-task","status":task.as_ref().map(|t|t.0.as_str()).unwrap_or("IDLE"),"ticks":task.as_ref().map(|t|t.1).unwrap_or(0),"sequence":seq,"observed_ms":now_ms()});
        if method != "status" && method != "tick" {
            tx.execute("INSERT INTO commands VALUES(?1,?2,?3)", params![id, method, result.to_string()])?;
        }
        tx.commit()?;
        Ok(result)
    }
}

pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_id(id: &str) -> Result<()> {
    ensure(!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "invalid artifact id")
}

fn kill_self() {
    // Fault injection only, never called by the production backend.
    unsafe {
        libc::kill(libc::getpid(), libc::SIGKILL);
    }
    std::process::abort();
}

pub fn transaction_child(args: &[String]) -> Result<()> {
    let mut store = Store::open(Path::new(args.first().ok_or("missing directory")?))?;
    store.apply("input", Some(args.get(1).ok_or("missing failure point")?))
}

pub fn artifact_child(args: &[String]) -> Result<()> {
    let store = Store::open(Path::new(args.first().ok_or("missing directory")?))?;
    let bytes = b"complete-response";
    store.reserve_artifact("response", "request", bytes)?;
    store.publish("response", bytes)?;
    kill_self();
    Ok(())
}
