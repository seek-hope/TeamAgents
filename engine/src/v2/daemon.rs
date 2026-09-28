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
    stopped: Arc<tokio::sync::Notify>,
    supervisor: Arc<SupervisorHandle>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl DaemonHandle {
    /// Resolve when the accept loop has ended on its own — a client asked this session to stop over its socket
    /// (D-248), or the loop failed. The reply is already on the wire by then (`serve_client` sets the flag only
    /// *after* it), so this is how the daemon process learns that the stop landed without a signal and runs the
    /// same designed shutdown `shutdown` runs.
    ///
    /// It deliberately does **not** await the join handle: `shutdown` awaits that exactly once, and a
    /// `JoinHandle` polled twice panics (`JoinHandle polled after completion` — measured 2026-09-27 while
    /// wiring this). Dropping this future cancels nothing.
    pub async fn stopped(&self) {
        self.stopped.notified().await;
    }

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
    let facts = Arc::new(SessionFacts {
        session_id: session_id.clone(),
        state_root: state_root.to_string_lossy().into_owned(),
        workspace: config.supervisor.workspace.to_string_lossy().into_owned(),
        permissions: config.supervisor.permissions.clone(),
        goal_limits: config.supervisor.goal_limits.clone(),
    });
    // D-227: refuse before binding, so the answer names the limit and the fix instead of the OS's
    // `bind …: path must be shorter than SUN_LEN` (`cli::require_socket_path_fits` holds the boundary).
    crate::cli::require_socket_path_fits(&config.socket)?;
    crate::cli::require_state_paths_kind(config.socket.parent().unwrap_or(&config.socket))?;
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
    let stopped = Arc::new(tokio::sync::Notify::new());
    let task = {
        let shutdown = shutdown.clone();
        let stopped = stopped.clone();
        let supervisor = supervisor.clone();
        let socket = config.socket.clone();
        tokio::spawn(async move {
            let result = accept_loop(listener, supervisor, session_db, facts, shutdown).await;
            let _ = std::fs::remove_file(&socket);
            // D-248: wake the process. A client that asked for the stop has its receipt already; without this
            // the daemon would sit in its signal wait forever while its socket was gone (measured 2026-09-27).
            stopped.notify_one();
            result
        })
    };
    Ok(DaemonHandle { shutdown, stopped, supervisor, task })
}

/// Facts every client is told in the greeting: the session id, its state root,
/// and the two settings that are fixed for the session's lifetime (the
/// workspace it runs in and the permission mode it booted with, D-41/D-57). A
/// client that joins a running session learns the truth here instead of
/// assuming the settings it asked for took effect.
#[derive(Clone)]
struct SessionFacts {
    session_id: String,
    state_root: String,
    workspace: String,
    permissions: String,
    /// D-268: the session's `[limits]`/`[[checks]]` (`config::goal_limits`), applied to **every** goal the session
    /// creates (D-64) — including one a client opens later, which until this existed carried no ceiling and no
    /// required check because only the bootstrap supplied them.
    goal_limits: Json,
}

async fn accept_loop(
    listener: tokio::net::UnixListener,
    supervisor: Arc<SupervisorHandle>,
    session_db: PathBuf,
    facts: Arc<SessionFacts>,
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
        let (supervisor, session_db, facts, shutdown_in_client) =
            (supervisor.clone(), session_db.clone(), facts.clone(), shutdown.clone());
        tokio::spawn(async move {
            if let Err(error) = serve_client(stream, supervisor, &session_db, &facts, shutdown_in_client).await {
                eprintln!("daemon client: {error}");
            }
        });
    }
}

async fn serve_client(
    stream: tokio::net::UnixStream,
    supervisor: Arc<SupervisorHandle>,
    session_db: &Path,
    facts: &SessionFacts,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (read, mut write) = stream.into_split();
    // greeting first: version, session and state root let the client refuse
    // an old server, a different root or an incompatible build (§9)
    let greeting = json!({"server": "teamagents-daemon", "protocol_version": PROTOCOL_VERSION,
                          "session_id": facts.session_id, "state_root": facts.state_root,
                          "permissions": facts.permissions, "workspace": facts.workspace});
    write.write_all(format!("{greeting}\n").as_bytes()).await.map_err(|e| format!("greeting: {e}"))?;
    let mut lines = tokio::io::BufReader::new(read).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Json>(&line) {
            Ok(request) => handle(&request, &supervisor, session_db, facts).await,
            Err(e) => json!({"request_id": Json::Null, "ok": false, "error": format!("bad request JSON: {e}")}),
        };
        if write.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
            return Ok(()); // client went away
        }
        // D-248: `shutdown` is answered *before* the daemon stops — the flag is set here, after the reply is on the
        // wire, and the accept loop (a different task, polling at 50 ms) sees it and leaves. A client that asked
        // to stop therefore always gets its receipt, however quickly the daemon goes.
        if reply["ok"] == json!(true) && reply["result"]["stopping"] == json!(true) {
            shutdown.store(true, Ordering::SeqCst);
        }
    }
    Ok(())
}

async fn handle(request: &Json, supervisor: &SupervisorHandle, session_db: &Path, facts: &SessionFacts) -> Json {
    let session_id = facts.session_id.as_str();
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
        "checkpoint" | "events" | "history" | "tasks" | "grants" | "approvals" | "goals" => {
            match read_only(session_db, session_id, |conn| read_method(method, &params, conn)) {
                Ok(result) => reply(true, result),
                Err(error) => reply(false, json!(error)),
            }
        }
        // D-248: the session's own stop lever. Addressed by *this socket* — the state root's socket is the
        // identity, so there is no pid to record, guess or reuse, and the command can only ever reach the daemon
        // serving this state root (the shape ACCEPTANCE's known gap asked for: "a `teamagents daemon --stop` (or
        // a protocol command) would be new surface, and it would have to answer the identity question a pid file
        // raises"). It is not a control-plane command: stopping a daemon is not a state transition of the
        // session's data, and a member instance never holds this socket.
        "shutdown" => reply(true, json!({"stopping": true, "state_root": facts.state_root})),
        // A business command: the v2 command vocabulary is the write surface
        // and the control plane authorizes/dedups it (§9). The client-chosen
        // command id rides at the request level, stable across reconnects.
        _ => {
            let command_id = request.get("command_id").and_then(|v| v.as_str()).unwrap_or("");
            if command_id.is_empty() {
                return reply(false, json!(format!("method {method:?} needs a command_id")));
            }
            let mut params = params;
            if method == "create_goal" {
                apply_session_goal_limits(&mut params, &facts.goal_limits);
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
    // `model` rides along (D-69): a team can span providers (`spawn(model = …)`),
    // and without it no client can say which member runs on what.
    // `reason` rides along too (D-165): the last lifecycle transition's own words, which is what answers "why
    // is this instance not running?" — for a leader that parked, the runtime's sentence about the cause. It
    // lived only in the event log, so `instances` could show `PARKED` and the guide's promise that it "shows
    // the reason" was not true of the surface a user reads first. The checkpoint is that surface for every
    // client (`instances`, `exec`'s guard, the TUI panel), so the field belongs on the row.
    //
    // The reasons come from a query of their own, ordered by sequence, so the last transition per scope is the
    // current one — and the row query below stays as it was: qualifying a column with a table alias here would
    // read as a use of an unrelated *config* key of the same name to `review/config_reference.py`'s name
    // search, which credited this file as a reader of `[tools.*] kind` when the correlated subquery did it
    // (measured 2026-09-27: the count for `kind` went 12 → 13 files with nothing actually reading it).
    let mut reasons: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT scope, payload_json FROM events WHERE kind = 'instance_lifecycle' ORDER BY sequence")
            .map_err(|e| format!("snapshot reasons prepare: {e}"))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("snapshot reasons query: {e}"))?;
        for row in rows {
            let (scope, payload) = row.map_err(|e| format!("snapshot reasons row: {e}"))?;
            // a transition recorded without words leaves the previous sentence in place; an instance that never
            // transitioned has no entry at all (the row's `reason` is null, and nothing is invented)
            if let Some(reason) = serde_json::from_str::<Json>(&payload)
                .ok()
                .and_then(|payload| payload["reason"].as_str().map(str::to_string))
            {
                reasons.insert(scope, reason);
            }
        }
    }
    let mut stmt = conn
        .prepare("SELECT id, lifecycle, phase, profile_json FROM instances ORDER BY id")
        .map_err(|e| format!("snapshot prepare: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let profile: Json = serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(Json::Null);
            let id = row.get::<_, String>(0)?;
            let reason = reasons.get(&id);
            Ok(json!({"id": id, "lifecycle": row.get::<_, String>(1)?,
                      "phase": row.get::<_, String>(2)?, "model": profile["model"], "reason": reason}))
        })
        .map_err(|e| format!("snapshot query: {e}"))?;
    let mut instances = Vec::new();
    for row in rows {
        instances.push(row.map_err(|e| format!("snapshot row: {e}"))?);
    }
    let goal: Option<(String, String, i64, String, Option<f64>)> = conn
        .query_row(
            // D-266: a session can now carry more than one goal (the boot goal plus one the user opened), and
            // `LIMIT 1` without an order returned an arbitrary row — a client could be shown the settled goal
            // while an active one existed. The active goal wins; among those (and among settled ones) the newest.
            "SELECT status, known_usage_json, unknown_usage, limits_json, deadline FROM goals
             ORDER BY (status = 'ACTIVE') DESC, rowid DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .ok();
    Ok(json!({"instances": instances, "goal": goal.map(|(status, usage, unknown, limits, deadline)|
        json!({"status": status, "known_usage": serde_json::from_str::<Json>(&usage).unwrap_or(Json::Null),
               "unknown_usage": unknown,
               "limits": serde_json::from_str::<Json>(&limits).unwrap_or(Json::Null),
               // the absolute deadline (D-64): a client that shows limits must be able
               // to say when the goal stops accepting new requests
               "deadline": deadline}))}))
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

/// D-268: a goal a *client* opens is bounded the way the bootstrap bounds its own (D-64: "`[limits]` in the user
/// config bounds every goal the session creates"). The session's ceiling and deadline fill in only what the
/// command left out — a client that names its own keeps them — and the session's configured required checks are
/// **unioned** with the command's, because both are the user's: a client must not be able to drop a check the
/// user's config requires. `deadline_minutes` is converted here exactly as `driver::bootstrap` converts it, so
/// the two goal-creating paths cannot disagree.
fn apply_session_goal_limits(params: &mut Json, session_limits: &Json) {
    let mut limits = params.get("limits").cloned().filter(Json::is_object).unwrap_or_else(|| json!({}));
    if let Some(max) = session_limits.get("max_total_tokens") {
        if limits.get("max_total_tokens").is_none() {
            limits["max_total_tokens"] = max.clone();
        }
    }
    let configured = session_limits.get("required_checks").and_then(Json::as_array).cloned().unwrap_or_default();
    if !configured.is_empty() {
        let mut merged = configured;
        for check in limits.get("required_checks").and_then(Json::as_array).cloned().unwrap_or_default() {
            let id = check.get("id").and_then(Json::as_str).unwrap_or("");
            if !merged.iter().any(|existing| existing.get("id").and_then(Json::as_str) == Some(id)) {
                merged.push(check);
            }
        }
        limits["required_checks"] = Json::Array(merged);
    }
    if !limits.as_object().map(|map| map.is_empty()).unwrap_or(true) {
        params["limits"] = limits;
    }
    if params.get("deadline").is_none() {
        if let Some(minutes) = session_limits.get("deadline_minutes").and_then(Json::as_u64) {
            params["deadline"] = json!(teamagents_core::models::now() + (minutes * 60) as f64);
        }
    }
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
            // Newest first, then reversed: the page is the newest `limit`
            // entries, in chronological (epoch, idx) order. The envelope id rides
            // along (D-72) because it is the only exact name for "the entry this
            // run's input landed as" — a client that must not mistake another
            // turn's outcome for its own cannot do that by matching text.
            let mut stmt = conn
                .prepare(
                    "SELECT epoch, idx, kind, message_json, envelope_id FROM context_entries
                     WHERE instance_id = ?1 ORDER BY epoch DESC, idx DESC LIMIT ?2",
                )
                .map_err(|e| format!("history prepare: {e}"))?;
            let rows = stmt
                .query_map(rusqlite::params![instance, limit], |row| {
                    Ok(json!({"epoch": row.get::<_, i64>(0)?, "idx": row.get::<_, i64>(1)?,
                              "kind": row.get::<_, String>(2)?, "envelope_id": row.get::<_, Option<String>>(4)?,
                              "message": serde_json::from_str::<Json>(&row.get::<_, String>(3)?).unwrap_or(Json::Null)}))
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
        // D-267: every goal a session carries, with the instance each is attached to. The checkpoint reports
        // *one* goal (the active one, D-266) because every client needs a single "the goal"; a user opening the
        // next goal needs the list — which one is active, which have settled, and what limits each carries —
        // and nothing else in the protocol exposed it.
        "goals" => {
            let mut stmt = conn
                .prepare(
                    "SELECT g.id, g.status, g.deadline, g.limits_json, g.known_usage_json, g.unknown_usage,
                            (SELECT GROUP_CONCAT(i.id, ',') FROM instances i WHERE i.active_goal_id = g.id)
                     FROM goals g
                     -- the goal the session is *working on* is the one an instance is attached to (D-266: a
                     -- user-opened goal attaches to the leader); then the active ones, newest first
                     ORDER BY EXISTS(SELECT 1 FROM instances i WHERE i.active_goal_id = g.id) DESC,
                              (g.status = 'ACTIVE') DESC, g.rowid DESC",
                )
                .map_err(|e| format!("goals prepare: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    let attached: Option<String> = row.get(6)?;
                    Ok(json!({
                        "id": row.get::<_, String>(0)?,
                        "status": row.get::<_, String>(1)?,
                        "deadline": row.get::<_, Option<f64>>(2)?,
                        "limits": serde_json::from_str::<Json>(&row.get::<_, String>(3)?).unwrap_or(json!({})),
                        "known_usage": serde_json::from_str::<Json>(&row.get::<_, String>(4)?).unwrap_or(json!({})),
                        "unknown_usage": row.get::<_, i64>(5)?,
                        "attached_instances": attached.map(|list| list.split(',').map(str::to_string).collect::<Vec<_>>()).unwrap_or_default(),
                    }))
                })
                .map_err(|e| format!("goals query: {e}"))?;
            let goals: Vec<Json> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("goals rows: {e}"))?;
            Ok(json!({"goals": goals}))
        }
        // The grant view is what the user's authority surface reads and the
        // only thing a revoke can name (D-61): the id, who issued it and where
        // it derives from ride along with the visible subject/action/scope, and
        // the session's grant revision is reported once. Without the id no
        // client could revoke a single grant — the surface was read-only in
        // practice while `revoke_grant` had no user-facing caller.
        "grants" => {
            let mut stmt = conn
                .prepare(
                    "SELECT id, issuer, subject, action, resource_scope, parent_grant_id, revoked_at
                     FROM grants ORDER BY subject, action, id",
                )
                .map_err(|e| format!("grants prepare: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(json!({"id": row.get::<_, String>(0)?, "issuer": row.get::<_, String>(1)?,
                              "subject": row.get::<_, String>(2)?, "action": row.get::<_, String>(3)?,
                              "resource_scope": row.get::<_, String>(4)?,
                              "parent_grant_id": row.get::<_, Option<String>>(5)?,
                              "revoked": row.get::<_, Option<f64>>(6)?.is_some()}))
                })
                .map_err(|e| format!("grants query: {e}"))?;
            let grants: Vec<Json> = rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("grants: {e}"))?;
            let revision: i64 = conn
                .query_row("SELECT value FROM meta WHERE key = 'grant_revision'", [], |row| row.get::<_, String>(0))
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            Ok(json!({"grants": grants, "revision": revision}))
        }
        other => Err(format!("unknown read method {other:?}")),
    }
}

/// v2 leader instructions: the collaboration vocabulary is the v2 kernel's
/// (spawn/delegate/send/wait/finish), not the legacy team tool names.
pub const LEADER_INSTRUCTIONS: &str = "You are the Leader of a team of agents. Understand the user's goal, decide
whether to work alone or build a team. Work directly on what you can finish in one pass: writing independent
pieces is cheap for a single instance, and delegating them costs at least as much as it saves (D-262's
measurements). Delegate parts that each need their own run-and-fix loop — set it up, run it, see what broke,
fix it — because a member can green its part while you work on yours, and that is where a team is measurably
faster. Give every delegated task a specific description and its own acceptance check, then wait on the task
ids you delegated, with a short timer, instead of polling: a member's outcome arrives as a task result, not as a
chat message, and an assignee that ends its turn without settling will never wake you — that part is then yours,
so re-delegate it as a new task or do it yourself. Check the finished work with one gate rather than re-running
every part's command in turn. Report completion with the finish tool. Keep task descriptions specific, include
acceptance criteria, and never bypass runtime permissions. Work is anchored to an active goal, and a settled goal
cannot be reopened: when it is settled, report that further work needs a new goal from the user (the
runtime opens none by itself) instead of retrying a delegation that has nowhere to be charged.";
