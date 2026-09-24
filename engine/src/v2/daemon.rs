//! R2-P4 R19 session daemon (plan §9): one Unix-socket JSON-lines server per
//! state root, owning the v2 supervisor. The TUI and `exec` are thin
//! clients; nothing executes in the client process. Writes go through
//! `SupervisorHandle::submit_user` so client commands linearize with driver
//! transitions on the single-writer storage worker and the control plane
//! keeps its authorization/dedup contract (command ids are client-chosen
//! and stable across reconnects, §9). Reads use an independent WAL
//! connection so a slow client never blocks the writer.
//!
//! Reconnect contract (§9): `checkpoint` returns the session snapshot and
//! its event watermark from one read transaction; `events` serves
//! everything after a watermark. Events are never reclaimed in this first
//! version, so a pruned watermark cannot occur — the `resync_required`
//! flag is the upgrade path once retention lands.

use super::supervisor::{SupervisorConfig, SupervisorHandle};
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use teamagents_core::v2::{Command, Control};

/// Wire version of this protocol; the greeting advertises it and every
/// request must repeat it (§9 handshake: incompatible versions refuse).
pub const PROTOCOL_VERSION: u64 = 1;

pub struct DaemonConfig<P, F> {
    /// The supervised session (session_db/session_id/state_root inside).
    pub supervisor: SupervisorConfig<P, F>,
    /// Socket path, conventionally `<state_root>/daemon.sock`.
    pub socket: PathBuf,
}

pub struct DaemonHandle {
    shutdown: Arc<AtomicBool>,
    supervisor: Arc<SupervisorHandle>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl DaemonHandle {
    /// Stop accepting, stop the supervisor, wait for the serve task.
    pub async fn shutdown(self) -> Result<(), String> {
        self.shutdown.store(true, Ordering::SeqCst);
        self.supervisor.shutdown_shared().await?;
        (&mut { self.task }).await.map_err(|e| format!("daemon join: {e}"))?
    }
}

/// Serve until `shutdown`. The supervisor is already running when the first
/// client can connect, so a client never observes a half-booted session.
pub async fn serve<P, F>(config: DaemonConfig<P, F>) -> Result<DaemonHandle, String>
where
    P: crate::providers::Provider + 'static,
    F: Fn(&str, &teamagents_core::kernel::KernelProfile) -> P + Send + Sync + 'static,
{
    let session_db = config.supervisor.session_db.clone();
    let session_id = config.supervisor.session_id.clone();
    let state_root = config.supervisor.state_root.clone();
    let supervisor = Arc::new(super::supervisor::start(config.supervisor).await?);
    if let Some(parent) = config.socket.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("socket dir: {e}"))?;
        // the socket directory is private to this OS user (§9)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    let _ = std::fs::remove_file(&config.socket); // stale socket from a crashed daemon
    let listener = match tokio::net::UnixListener::bind(&config.socket) {
        Ok(listener) => listener,
        Err(e) => {
            supervisor.shutdown_shared().await.ok();
            return Err(format!("bind {}: {e}", config.socket.display()));
        }
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let task = {
        let shutdown = shutdown.clone();
        let supervisor = supervisor.clone();
        let socket = config.socket.clone();
        tokio::spawn(async move {
            let result = accept_loop(listener, supervisor, session_db, session_id, state_root, shutdown).await;
            let _ = std::fs::remove_file(&socket);
            result
        })
    };
    Ok(DaemonHandle { shutdown, supervisor, task })
}

async fn accept_loop(
    listener: tokio::net::UnixListener,
    supervisor: Arc<SupervisorHandle>,
    session_db: PathBuf,
    session_id: String,
    state_root: PathBuf,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => continue,
        };
        let (stream, _) = match accepted {
            Ok(pair) => pair,
            Err(e) => return Err(format!("accept: {e}")),
        };
        let (supervisor, session_db, session_id, state_root) =
            (supervisor.clone(), session_db.clone(), session_id.clone(), state_root.clone());
        tokio::spawn(async move {
            if let Err(error) = serve_client(stream, supervisor, &session_db, &session_id, &state_root).await {
                eprintln!("daemon client: {error}");
            }
        });
    }
}

async fn serve_client(
    stream: tokio::net::UnixStream,
    supervisor: Arc<SupervisorHandle>,
    session_db: &Path,
    session_id: &str,
    state_root: &Path,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (read, mut write) = stream.into_split();
    // greeting first: version, session and state root let the client refuse
    // an old server, a different root or an incompatible build (§9)
    let greeting = json!({"server": "teamagents-daemon", "protocol_version": PROTOCOL_VERSION,
                          "session_id": session_id, "state_root": state_root.to_string_lossy()});
    write.write_all(format!("{greeting}\n").as_bytes()).await.map_err(|e| format!("greeting: {e}"))?;
    let mut lines = tokio::io::BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Json>(&line) {
            Ok(request) => handle(&request, &supervisor, session_db, session_id).await,
            Err(e) => json!({"request_id": Json::Null, "ok": false, "error": format!("bad request JSON: {e}")}),
        };
        if write.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
            return Ok(()); // client went away
        }
    }
    Ok(())
}

async fn handle(request: &Json, supervisor: &SupervisorHandle, session_db: &Path, session_id: &str) -> Json {
    let request_id = request.get("request_id").cloned().unwrap_or(Json::Null);
    let reply = |ok: bool, payload: Json| {
        if ok {
            json!({"request_id": request_id, "ok": true, "result": payload})
        } else {
            json!({"request_id": request_id, "ok": false, "error": payload})
        }
    };
    if request.get("protocol_version") != Some(&json!(PROTOCOL_VERSION)) {
        return reply(false, json!(format!("protocol_version must be {PROTOCOL_VERSION}")));
    }
    let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = request.get("params").cloned().unwrap_or(json!({}));
    match method {
        "checkpoint" | "events" | "history" | "tasks" | "grants" | "approvals" => {
            match read_only(session_db, session_id, |conn| read_method(method, &params, conn)) {
                Ok(result) => reply(true, result),
                Err(error) => reply(false, json!(error)),
            }
        }
        // A business command: the v2 command vocabulary is the write surface
        // and the control plane authorizes/dedups it (§9). The client-chosen
        // command id rides at the request level, stable across reconnects.
        _ => {
            let command_id = request.get("command_id").and_then(|v| v.as_str()).unwrap_or("");
            if command_id.is_empty() {
                return reply(false, json!(format!("method {method:?} needs a command_id")));
            }
            let command = Command { command_id: command_id.to_string(), method: method.to_string(), params };
            match supervisor.submit_user(command).await {
                Ok(result) => reply(true, result),
                Err(error) => reply(false, json!(error)),
            }
        }
    }
}

fn read_only<T>(
    session_db: &Path,
    session_id: &str,
    f: impl FnOnce(&rusqlite::Connection) -> Result<T, String>,
) -> Result<T, String> {
    let control = Control::open(session_db, session_id, false)?;
    f(control.connection())
}

/// Snapshot query shared by `checkpoint` (same read transaction as the
/// watermark) — instances with lifecycle/phase plus the single-goal view.
fn read_snapshot(conn: &rusqlite::Connection) -> Result<Json, String> {
    let mut stmt = conn
        .prepare("SELECT id, lifecycle, phase FROM instances ORDER BY id")
        .map_err(|e| format!("snapshot prepare: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(json!({"id": row.get::<_, String>(0)?, "lifecycle": row.get::<_, String>(1)?,
                      "phase": row.get::<_, String>(2)?}))
        })
        .map_err(|e| format!("snapshot query: {e}"))?;
    let mut instances = Vec::new();
    for row in rows {
        instances.push(row.map_err(|e| format!("snapshot row: {e}"))?);
    }
    let goal: Option<(String, String, i64, String)> = conn
        .query_row("SELECT status, known_usage_json, unknown_usage, limits_json FROM goals LIMIT 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .ok();
    Ok(json!({"instances": instances, "goal": goal.map(|(status, usage, unknown, limits)|
        json!({"status": status, "known_usage": serde_json::from_str::<Json>(&usage).unwrap_or(Json::Null),
               "unknown_usage": unknown,
               "limits": serde_json::from_str::<Json>(&limits).unwrap_or(Json::Null)}))}))
}

fn read_events(conn: &rusqlite::Connection, since: i64) -> Result<Vec<Json>, String> {
    let mut stmt = conn
        .prepare("SELECT sequence, kind, scope, payload_json FROM events WHERE sequence > ?1 ORDER BY sequence")
        .map_err(|e| format!("events prepare: {e}"))?;
    let rows = stmt
        .query_map([since], |row| {
            Ok(json!({"sequence": row.get::<_, i64>(0)?, "kind": row.get::<_, String>(1)?,
                      "scope": row.get::<_, String>(2)?,
                      "payload": serde_json::from_str::<Json>(&row.get::<_, String>(3)?).unwrap_or(Json::Null)}))
        })
        .map_err(|e| format!("events query: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("events collect: {e}"))
}

fn watermark(conn: &rusqlite::Connection) -> Result<i64, String> {
    conn.query_row("SELECT COALESCE(MAX(sequence), 0) FROM events", [], |row| row.get(0))
        .map_err(|e| format!("watermark: {e}"))
}

fn read_method(method: &str, params: &Json, conn: &rusqlite::Connection) -> Result<Json, String> {
    match method {
        // snapshot and watermark in one read transaction (§9 reconnect):
        // the client learns exactly which point its snapshot is consistent with
        "checkpoint" => {
            let tx = conn.unchecked_transaction().map_err(|e| format!("checkpoint tx: {e}"))?;
            let snapshot = read_snapshot(&tx)?;
            let watermark = watermark(&tx)?;
            tx.commit().map_err(|e| format!("checkpoint commit: {e}"))?;
            Ok(json!({"snapshot": snapshot, "watermark": watermark}))
        }
        "events" => {
            let since = params.get("since").and_then(|v| v.as_i64()).unwrap_or(0);
            if since < 0 {
                return Err("events.since must be >= 0".into());
            }
            let current = watermark(conn)?;
            let events = read_events(conn, since)?;
            Ok(json!({"events": events, "watermark": current, "resync_required": false}))
        }
        "history" => {
            let instance = params.get("instance_id").and_then(|v| v.as_str()).unwrap_or("");
            if instance.is_empty() {
                return Err("history.instance_id required".into());
            }
            let limit = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(200).clamp(1, 1000);
            let mut stmt = conn
                .prepare("SELECT idx, kind, message_json FROM context_entries WHERE instance_id = ?1 ORDER BY idx DESC LIMIT ?2")
                .map_err(|e| format!("history prepare: {e}"))?;
            let rows = stmt
                .query_map(rusqlite::params![instance, limit], |row| {
                    Ok(json!({"idx": row.get::<_, i64>(0)?, "kind": row.get::<_, String>(1)?,
                              "message": serde_json::from_str::<Json>(&row.get::<_, String>(2)?).unwrap_or(Json::Null)}))
                })
                .map_err(|e| format!("history query: {e}"))?;
            let mut entries: Vec<Json> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("history: {e}"))?;
            entries.reverse(); // chronological order
            Ok(json!({"instance_id": instance, "entries": entries}))
        }
        "tasks" => {
            let mut stmt = conn
                .prepare("SELECT id, goal_id, assignee, status FROM tasks ORDER BY rowid")
                .map_err(|e| format!("tasks prepare: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(json!({"id": row.get::<_, String>(0)?, "goal_id": row.get::<_, String>(1)?,
                              "assignee": row.get::<_, String>(2)?, "status": row.get::<_, String>(3)?}))
                })
                .map_err(|e| format!("tasks query: {e}"))?;
            let tasks: Vec<Json> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("tasks: {e}"))?;
            Ok(json!({"tasks": tasks}))
        }
        // pending approvals with the operation's fixed intent (§9 first
        // version: approvals and unknown outcomes) — the client renders a preview and
        // decides through the write surface; args stay a bounded preview
        "approvals" => {
            let mut stmt = conn
                .prepare(
                    "SELECT a.id, a.operation_id, o.intent_json FROM approvals a
                     JOIN operations o ON a.operation_id = o.operation_id
                     WHERE a.status = 'PENDING' ORDER BY a.rowid",
                )
                .map_err(|e| format!("approvals prepare: {e}"))?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
                .map_err(|e| format!("approvals query: {e}"))?;
            let mut approvals = Vec::new();
            for row in rows {
                let (id, operation_id, intent_json) = row.map_err(|e| format!("approvals row: {e}"))?;
                let intent: Json = serde_json::from_str(&intent_json).unwrap_or(Json::Null);
                let args = intent["args"].clone();
                let preview = args["command"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| args.to_string().chars().take(120).collect());
                approvals.push(json!({"id": id, "operation_id": operation_id,
                                      "tool": intent["name"].as_str().unwrap_or(""), "preview": preview}));
            }
            Ok(json!({"approvals": approvals}))
        }
        "grants" => {
            let mut stmt = conn
                .prepare("SELECT subject, action, resource_scope, revoked_at FROM grants ORDER BY subject, action")
                .map_err(|e| format!("grants prepare: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(json!({"subject": row.get::<_, String>(0)?, "action": row.get::<_, String>(1)?,
                              "resource_scope": row.get::<_, String>(2)?,
                              "revoked": row.get::<_, Option<String>>(3)?.is_some()}))
                })
                .map_err(|e| format!("grants query: {e}"))?;
            let grants: Vec<Json> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("grants: {e}"))?;
            Ok(json!({"grants": grants}))
        }
        other => Err(format!("unknown read method {other:?}")),
    }
}

/// v2 leader instructions: the collaboration vocabulary is the v2 kernel's
/// (spawn/delegate/send/wait/finish), not the legacy team tool names.
pub const LEADER_INSTRUCTIONS: &str = "You are the Leader of a team of agents. Understand the user's goal, decide
whether to work alone or build a team. Work directly on small or tightly
coupled tasks. For bounded, independent work that can progress alongside
your own, spawn a worker instance and delegate tasks to it; wait on results
instead of polling. Report completion with the finish tool. Keep task
descriptions specific, include acceptance criteria, and never bypass
runtime permissions. Work is anchored to an active goal: when the current goal
is settled, create a new goal before delegating further work.";
