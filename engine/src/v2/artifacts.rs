//! The state root's artifacts, headless (D-253): what a root holds on disk, and the on-demand half of §4.3's
//! collection.
//!
//! DESIGN §4.3/§4.4: "GC first claims an unreferenced object as DELETING inside a transaction, then refuses new
//! references and only afterwards deletes the file, and a failed deletion can be retried"; "artifact collection
//! and history retention are scheduled separately and protect live references and evaluation evidence". This
//! build scheduled collection at a *driver's boot* only (D-191), so ACCEPTANCE's known gap recorded what that
//! leaves: "a state root whose last driver never boots again therefore keeps its `DELETING` rows and their
//! bytes … A cadence — an interval, or a maintenance verb to run on demand — is the user's call."
//!
//! **The verb is the on-demand half, and the census is why it matters.** `artifacts list` reports what the store
//! records (id, kind, size, completeness, the fact that owns it, the path) and whether those bytes are really on
//! disk — a fact that was invisible before, and the one a user needs to decide whether a root should be pruned.
//! `artifacts gc` runs exactly the two commands a boot runs (`artifact_gc_claim` then `artifact_collect`, with
//! the byte removal between them, outside any transaction) and reports what it claimed, freed and skipped.
//!
//! **It deliberately refuses while a session is live**, and that is the whole safety: §6.1's coordinator lock
//! ("one coordinator per state root") is taken by the verb for the duration of the sweep, so a maintenance pass
//! and a driver can never write together; a held lock is answered with the levers the user has (stop the session
//! with `daemon --stop`, D-248 — its next boot sweeps anyway, or run this then).
//!
//! **The census reads what the root really holds.** It opens the store the ordinary way first (the same open a
//! session's boot and `doctor` use), because a read-only open of a write-ahead-log database depends on its
//! shared-memory file: measured while wiring this, a leftover `-shm` made a read-only reader answer **zero**
//! artifacts with no error at all, and an unwritable directory with no `-shm` made SQLite answer an unhelpful
//! "attempt to write a readonly database". Only a root this process cannot write falls back to
//! `store::open_read_only`, which probes the log and refuses *by name* when it cannot attach it — a silent
//! under-report is the one answer a census must never give.
//!
//! **What it does not do, measured 2026-09-28**: it does not invent a *release* policy. Both places that stage an
//! artifact write an `owner_ref` (`driver.rs`: a model response's request, a tool output's operation), nothing
//! ever clears it, and the reference clauses in `artifact_gc_claim` ask for `owner_ref IS NULL` — so a real
//! session's artifacts are never claimable while their owning fact exists, and that fact is never pruned. A
//! scripted session with a 300 KB tool output left three LIVE artifacts, 0 claimable. `V2Artifact.tla` says the
//! same thing structurally: the "LIVE and unreferenced" state exists there only under the counterfactual
//! `GcIgnoresHolders`. So `gc` today is the recovery half (a sweep that a *crash* left half-done) plus the
//! census; whether artifact bytes should expire with `[retention] history_days` — they are "ordinary history" in
//! §9's words — or under their own knob is a policy the user has not stated, and it is recorded as an open
//! question rather than guessed at here.

use rusqlite::OptionalExtension;
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};

/// What the user asked the artifact surface to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactsCommand {
    /// What this state root holds: the catalog's rows and whether their bytes are on disk.
    List,
    /// §4.3's collection, on demand: claim → delete the bytes → collect, under the coordinator lock.
    Gc,
}

pub struct ArtifactsOptions {
    pub state_root: PathBuf,
    pub command: ArtifactsCommand,
    pub json_out: bool,
}

/// One row of the census.
struct Row {
    id: String,
    kind: String,
    size: i64,
    completeness: String,
    owner_ref: Option<String>,
    storage_ref: Option<String>,
}

/// The rows of one session's artifacts, plus the set of session ids the store holds artifacts for (A33 means one
/// in practice; the loop keeps the verb honest if that ever changes).
fn census(control: &teamagents_core::v2::Control) -> Result<(Vec<String>, Vec<Row>), String> {
    let connection = control.connection();
    let mut sessions = Vec::new();
    {
        let mut stmt = connection
            .prepare("SELECT DISTINCT session_id FROM artifacts ORDER BY session_id")
            .map_err(|e| format!("artifacts: {e}"))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| format!("artifacts: {e}"))?;
        for row in rows {
            sessions.push(row.map_err(|e| format!("artifacts: {e}"))?);
        }
    }
    let mut out = Vec::new();
    let mut stmt = connection
        .prepare("SELECT id, kind, size, completeness, owner_ref, storage_ref FROM artifacts ORDER BY id")
        .map_err(|e| format!("artifacts: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Row {
                id: row.get(0)?,
                kind: row.get(1)?,
                size: row.get(2)?,
                completeness: row.get(3)?,
                owner_ref: row.get(4)?,
                storage_ref: row.get(5)?,
            })
        })
        .map_err(|e| format!("artifacts: {e}"))?;
    for row in rows {
        out.push(row.map_err(|e| format!("artifacts: {e}"))?);
    }
    Ok((sessions, out))
}

/// One artifact command, for the CLI. Errors are (exit code, message).
pub fn run(options: ArtifactsOptions) -> i32 {
    match execute(&options) {
        Ok(report) => {
            print_report(&options, &report);
            0
        }
        Err((code, message)) => {
            eprintln!("{message}");
            code
        }
    }
}

pub fn execute(options: &ArtifactsOptions) -> Result<Json, (i32, String)> {
    let db = options.state_root.join("session.sqlite");
    if !db.exists() {
        return Err((
            2,
            format!("artifacts: no session database at {} — this state root has never held a session", db.display()),
        ));
    }
    let listing = options.command == ArtifactsCommand::List;
    // §6.1: one coordinator per state root. `state_lock` waits a moment for the fork window (a tool child that
    // has not execd yet) and then refuses a real holder — which is exactly a live session.
    let _lock = if listing {
        None
    } else {
        match crate::jobs::state_lock(&options.state_root.join("coordinator.lock")) {
            Ok(lock) => Some(lock),
            Err(error) => {
                return Err((
                    1,
                    format!(
                        "artifacts gc: {error}\na live session collects when its driver boots; stop it \
                         (`teamagents daemon --stop`) and run this, or wait for its next boot"
                    ),
                ))
            }
        }
    };
    // The session id is the store's own (a read is session-agnostic; the claim command is scoped to it).
    let reader = open(&db, !listing)?;
    let (sessions, rows) = census(&reader).map_err(|error| (2, error))?;
    drop(reader);
    let on_disk: Vec<Json> = rows
        .iter()
        .map(|row| {
            let path = row.storage_ref.as_deref().map(Path::new);
            json!({
                "id": row.id,
                "kind": row.kind,
                "size": row.size,
                "completeness": row.completeness,
                "owner_ref": row.owner_ref,
                "storage_ref": row.storage_ref,
                "bytes_present": path.map(|path| path.is_file()).unwrap_or(false),
            })
        })
        .collect();
    let bytes: i64 = rows.iter().map(|row| row.size).sum();
    if !listing {
        return sweep(&db, &options.state_root, &sessions, rows, bytes);
    }
    Ok(json!({
        "session_id": if sessions.len() == 1 { Json::String(sessions[0].clone()) } else { Json::Null },
        "state_root": options.state_root.to_string_lossy(),
        "artifacts": on_disk,
        "count": rows.len(),
        "bytes": bytes,
    }))
}

/// Open the store for the census (D-253).
///
/// The normal, writable open comes first — the same one `doctor` and a session's boot use, which is what makes a
/// read see every committed row (a *read-only* open of a write-ahead-log database depends on the shared-memory
/// file, and a leftover one can answer a stale view; measured while wiring this). Only a root this process
/// cannot write falls back to `store::open_read_only`, which refuses when it cannot be trusted (`a WAL with
/// commits and no shm`). `gc` always opens writable: a sweep that has to migrate a root migrates it the way the
/// product does.
fn open(db: &Path, writable: bool) -> Result<teamagents_core::v2::Control, (i32, String)> {
    if writable {
        return teamagents_core::v2::Control::open(db, "maintenance", false)
            .map_err(|error| (2, format!("artifacts: {}: {error}", db.display())));
    }
    match teamagents_core::v2::Control::open(db, "maintenance", false) {
        Ok(control) => Ok(control),
        Err(writable_error) => {
            teamagents_core::v2::Control::open_read_only(db, "maintenance").map_err(|read_only_error| {
                (2, format!("artifacts: {read_only_error}\n(an ordinary open failed too: {writable_error})"))
            })
        }
    }
}

/// The collector's algorithm, driven through a session's own single writer (D-351). Claim the unreferenced
/// LIVE artifacts (one command, one transaction), delete their bytes *outside* it — a file operation may not
/// run inside one — then collect each row. The order is the model's (`V2Artifact::GcClaim` then `GcDelete`),
/// and a failure between the deletion and the row removal leaves the row DELETING, which the next sweep
/// retries: a file that is already gone is the outcome we wanted, not an error.
///
/// Both triggers that have a live writer run exactly this: a driver's boot, and — since D-351, the decision
/// D-341 made for D-191 — the daemon's start, through the supervisor that holds the coordinator lock. The
/// on-demand verb cannot use this transport: no session is live, so *it* holds that lock and owns the store,
/// and drives the same two commands through `collect_once`. There is one algorithm here; what differs is who
/// owns the writer, which is exactly what the lock decides.
pub async fn sweep_through(storage: &super::storage::Storage, limit: i64) -> Result<Json, String> {
    let claimed = storage
        .call({
            let command_id = format!("gc-claim-{}", uuid::Uuid::new_v4());
            move |control| {
                control.submit(
                    teamagents_core::v2::Command {
                        command_id,
                        method: "artifact_gc_claim".into(),
                        params: json!({"limit": limit}),
                    },
                    teamagents_core::v2::Identity::System,
                )
            }
        })
        .await??;
    let ids: Vec<String> = claimed["claimed"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect();
    let mut collected: Vec<String> = Vec::new();
    let mut freed = 0u64;
    let mut failed: Vec<Json> = Vec::new();
    for id in ids {
        let row: Option<(String, i64)> = storage
            .call({
                let id = id.clone();
                move |control| {
                    control
                        .connection()
                        .query_row(
                            "SELECT storage_ref, size FROM artifacts WHERE id = ?1 AND completeness = 'DELETING'",
                            [&id],
                            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                        )
                        .optional()
                        .map_err(|e| format!("collect read {id}: {e}"))
                }
            })
            .await??;
        let mut removed = 0u64;
        if let Some((path, size)) = row {
            match std::fs::remove_file(&path) {
                Ok(()) => removed = size.max(0) as u64,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    failed.push(json!({"id": id, "path": path, "error": error.to_string()}));
                    continue; // the row stays DELETING; the next sweep retries
                }
            }
        }
        storage
            .call({
                let id = id.clone();
                move |control| {
                    control.submit(
                        teamagents_core::v2::Command {
                            command_id: format!("collect-{id}"),
                            method: "artifact_collect".into(),
                            params: json!({"id": id}),
                        },
                        teamagents_core::v2::Identity::System,
                    )
                }
            })
            .await??;
        collected.push(id);
        freed += removed;
    }
    Ok(json!({"collected": collected, "freed_bytes": freed, "failed": failed}))
}

/// §4.3's collection, exactly as a driver's boot runs it: claim (one transaction), delete the bytes (outside it),
/// collect the row. A deletion that fails leaves the row DELETING — the next run retries, which is the design's
/// "a failed deletion can be retried".
fn sweep(db: &Path, state_root: &Path, sessions: &[String], rows: Vec<Row>, bytes: i64) -> Result<Json, (i32, String)> {
    let mut claimed_all: Vec<String> = Vec::new();
    let mut freed = 0u64;
    let mut failed: Vec<Json> = Vec::new();
    for session_id in sessions {
        let mut control = teamagents_core::v2::Control::open(db, session_id, false)
            .map_err(|error| (2, format!("artifacts gc: {}: {error}", db.display())))?;
        let claimed = control
            .submit(
                teamagents_core::v2::Command {
                    command_id: format!("artifacts-gc-claim-{}", uuid::Uuid::new_v4()),
                    method: "artifact_gc_claim".into(),
                    params: json!({"limit": 1000}),
                },
                teamagents_core::v2::Identity::System,
            )
            .map_err(|error| (1, format!("artifacts gc: the session refused the claim: {error}")))?;
        let ids: Vec<String> = claimed["claimed"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        for id in ids {
            let path = rows.iter().find(|row| row.id == id).and_then(|row| row.storage_ref.clone());
            let mut removed = 0u64;
            if let Some(path) = path {
                match std::fs::remove_file(&path) {
                    Ok(()) => {
                        removed = rows.iter().find(|row| row.id == id).map(|row| row.size.max(0) as u64).unwrap_or(0)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        // the row stays DELETING; the next run retries, which is the designed answer
                        failed.push(json!({"id": id, "error": error.to_string()}));
                        continue;
                    }
                }
            }
            control
                .submit(
                    teamagents_core::v2::Command {
                        command_id: format!("artifacts-collect-{id}"),
                        method: "artifact_collect".into(),
                        params: json!({"id": id}),
                    },
                    teamagents_core::v2::Identity::System,
                )
                .map_err(|error| (1, format!("artifacts gc: collecting {id} was refused: {error}")))?;
            freed += removed;
            claimed_all.push(id);
        }
    }
    Ok(json!({
        "session_id": if sessions.len() == 1 { Json::String(sessions[0].clone()) } else { Json::Null },
        "state_root": state_root.to_string_lossy(),
        "count": rows.len(),
        "bytes": bytes,
        "collected": claimed_all,
        "freed_bytes": freed,
        "failed": failed,
    }))
}

fn print_report(options: &ArtifactsOptions, report: &Json) {
    if options.json_out {
        println!("{}", serde_json::to_string(report).unwrap_or_else(|_| "{}".into()));
        return;
    }
    let root = options.state_root.display();
    let mb = |bytes: i64| format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0));
    let total = mb(report["bytes"].as_i64().unwrap_or(0));
    if options.command == ArtifactsCommand::Gc {
        println!(
            "{root}: {} artifact(s), {total} in the catalog before the sweep",
            report["count"].as_i64().unwrap_or(0)
        );
        let collected = report["collected"].as_array().map(Vec::len).unwrap_or(0);
        let failed = report["failed"].as_array().map(Vec::len).unwrap_or(0);
        println!(
            "collected {collected}, freed {} — {failed} failed (a failed deletion stays DELETING and the next \
             run retries)",
            mb(report["freed_bytes"].as_i64().unwrap_or(0))
        );
        return;
    }
    let rows = report["artifacts"].as_array().cloned().unwrap_or_default();
    println!("{root}: {} artifact(s), {total} in the catalog", rows.len());
    for row in &rows {
        println!(
            "  {}  {}  {}  {}  {}{}",
            row["id"].as_str().unwrap_or(""),
            row["kind"].as_str().unwrap_or(""),
            mb(row["size"].as_i64().unwrap_or(0)),
            row["completeness"].as_str().unwrap_or(""),
            match row["owner_ref"].as_str() {
                Some(owner) => format!("owned by {owner}"),
                None => "unreferenced".to_string(),
            },
            if row["bytes_present"] == json!(true) { "" } else { "  (bytes missing)" },
        );
    }
}
