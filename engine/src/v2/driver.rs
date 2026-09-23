//! R2-P2 persistent single-instance driver (plan §3, §6): the async phase
//! machine over the v2 control plane. Every model/tool wait happens outside
//! transactions; every state transition goes through `Control::submit`.
//! Recovery is state-driven (§6.3): known results are reused, in-flight
//! losses are recorded honestly, and nothing side-effecting is re-executed
//! on a guess.

use crate::jobs::{client, JobSpec, Journal};
use crate::providers::{Cancel, ErrorClass, Provider, ProviderEvent};
use crate::reference::readback_receipt;
use crate::tools::{shell_command_spec, ShellMode, V2Toolkit};
use crate::v2::storage::Storage;
use rusqlite::OptionalExtension;
use serde_json::{json, Value as Json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use teamagents_core::kernel::*;
use teamagents_core::v2::{Command, Identity};

/// Output cap mirrored from the synchronous shell path (tools.rs MAX_OUTPUT).
const MAX_OUTPUT: usize = 200_000;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

pub struct DriverConfig<P> {
    pub session_db: PathBuf,
    pub session_id: String,
    pub instance_id: String,
    /// Jobs, artifacts and the coordinator lock live here (one lock per root, §6.1).
    pub state_root: PathBuf,
    pub workspace: PathBuf,
    /// Trusted session permission mode ("approved_scope" | "full_auto"), D-41.
    pub permissions: String,
    pub profile: KernelProfile,
    pub provider: P,
    pub catalog: teamagents_core::models::UserConfig,
    pub bindings: Vec<String>,
    /// Transient transport retries inside one request (single retry owner, §7).
    pub max_retries: usize,
    pub storage_queue: usize,
    pub poll: Duration,
    /// Goal limits (e.g. max_total_tokens) recorded at bootstrap.
    pub goal_limits: Json,
    /// ponytail: sessions run with a fixed permission snapshot (revision 0);
    /// live permission changes bump the revision and re-authorize at dispatch.
    pub require_shell_approval: bool,
}

#[derive(Clone, Debug)]
struct Snapshot {
    lifecycle: String,
    phase: String,
    revision: i64,
    epoch: i64,
    active_request: Option<String>,
    active_goal: Option<String>,
}

pub(crate) struct Shared {
    pub(crate) wake: tokio::sync::Notify,
    pub(crate) shutdown: AtomicBool,
    pub(crate) cancel_attempt: Mutex<Option<Cancel>>,
}

/// Client handle: user operations go through the same serialized storage
/// worker as the driver, so commands linearize with driver transitions (§9).
pub struct DriverHandle {
    storage: Storage,
    shared: Arc<Shared>,
    session_id: String,
    instance_id: String,
    goal_id: String,
    task: tokio::task::JoinHandle<Result<(), String>>,
    _lock: std::fs::File,
}

fn command(id: impl Into<String>, method: &str, params: Json) -> Command {
    Command { command_id: id.into(), method: method.into(), params }
}

impl DriverHandle {
    async fn submit(&self, cmd: Command, identity: Identity) -> Result<Json, String> {
        let result = self.storage.call(move |control| control.submit(cmd, identity)).await??;
        self.shared.wake.notify_one();
        Ok(result)
    }

    /// User input at the accept boundary (§5.4); replay-safe by command id.
    pub async fn input(&self, text: &str) -> Result<Json, String> {
        let envelope = format!("env-{}", uuid::Uuid::new_v4());
        self.submit(
            command(
                format!("input-{envelope}"),
                "submit_input",
                json!({"instance_id": self.instance_id, "envelope_id": envelope, "text": text}),
            ),
            Identity::User,
        )
        .await
    }

    pub async fn pause(&self) -> Result<Json, String> {
        self.lifecycle("PAUSED").await
    }

    pub async fn resume(&self) -> Result<Json, String> {
        self.lifecycle("ACTIVE").await
    }

    async fn lifecycle(&self, target: &str) -> Result<Json, String> {
        self.submit(
            command(
                format!("lifecycle-{}-{}", target.to_lowercase(), uuid::Uuid::new_v4()),
                "set_lifecycle",
                json!({"instance_id": self.instance_id, "lifecycle": target}),
            ),
            Identity::User,
        )
        .await
    }

    pub async fn cancel_operation(&self, operation_id: &str) -> Result<Json, String> {
        self.submit(
            command(
                format!("cancel-op-{operation_id}-{}", uuid::Uuid::new_v4()),
                "cancel_operation",
                json!({"operation_id": operation_id}),
            ),
            Identity::User,
        )
        .await
    }

    /// Cancel the in-flight model request: the local read is abandoned and
    /// the request closes; a late provider answer is archived, never applied.
    pub async fn cancel_turn(&self) -> Result<Json, String> {
        if let Some(cancel) = self.shared.cancel_attempt.lock().unwrap().as_ref() {
            cancel.cancel();
        }
        let request: Option<String> = self
            .storage
            .call({
                let instance = self.instance_id.clone();
                move |control| {
                    control
                        .connection()
                        .query_row("SELECT active_request_id FROM instances WHERE id = ?1", [&instance], |row| {
                            row.get(0)
                        })
                        .ok()
                        .flatten()
                }
            })
            .await?;
        let Some(request) = request else { return Ok(json!({"status": "no active request"})) };
        self.submit(
            command(
                format!("cancel-req-{request}-{}", uuid::Uuid::new_v4()),
                "cancel_request",
                json!({"request_id": request}),
            ),
            Identity::User,
        )
        .await
    }

    pub async fn approve(&self, approval_id: &str) -> Result<Json, String> {
        self.submit(
            command(format!("approve-{approval_id}"), "approve", json!({"approval_id": approval_id})),
            Identity::User,
        )
        .await
    }

    pub async fn deny(&self, approval_id: &str) -> Result<Json, String> {
        self.submit(command(format!("deny-{approval_id}"), "deny", json!({"approval_id": approval_id})), Identity::User)
            .await
    }

    /// Instance + goal snapshot for status views (read from the same worker).
    pub async fn snapshot(&self) -> Result<Json, String> {
        let (instance, goal) = (self.instance_id.clone(), self.goal_id.clone());
        self.storage
            .call(move |control| {
                let conn = control.connection();
                let instance: Option<(String, String, i64, Option<String>)> = conn
                    .query_row(
                        "SELECT lifecycle, phase, revision, active_request_id FROM instances WHERE id = ?1",
                        [&instance],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .ok();
                let goal: Option<(String, String, String, i64)> = conn
                    .query_row(
                        "SELECT status, known_usage_json, reservations_json, unknown_usage FROM goals WHERE id = ?1",
                        [&goal],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .ok();
                let pending_approvals: i64 = conn
                    .query_row("SELECT COUNT(*) FROM approvals WHERE status = 'PENDING'", [], |row| row.get(0))
                    .unwrap_or(0);
                Ok::<_, String>(json!({
                    "instance": instance.map(|(lifecycle, phase, revision, request)|
                        json!({"lifecycle": lifecycle, "phase": phase, "revision": revision, "active_request": request})),
                    "goal": goal.map(|(status, usage, reservations, unknown)|
                        json!({"status": status, "known_usage": serde_json::from_str::<Json>(&usage).unwrap_or(Json::Null),
                               "reservations": serde_json::from_str::<Json>(&reservations).unwrap_or(Json::Null),
                               "unknown_usage": unknown})),
                    "pending_approvals": pending_approvals,
                }))
            })
            .await?
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Events after the given sequence (§9 reconnect contract, minimal form).
    pub async fn events(&self, since: i64) -> Result<Vec<Json>, String> {
        self.storage
            .call(move |control| {
                let mut stmt = control
                    .connection()
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
            })
            .await?
    }

    /// Simulate a hard driver crash (tests only): the task is aborted
    /// mid-await, committed state stays, runners survive independently (A11).
    pub fn crash(self) {
        self.task.abort();
    }

    /// Stop driving; submitted commands stay committed (§4.1).
    pub async fn shutdown(self) -> Result<(), String> {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.shared.wake.notify_one();
        match self.task.await {
            Ok(result) => result,
            Err(e) => Err(format!("driver task join: {e}")),
        }
    }
}

pub struct Driver<P: Provider> {
    config: DriverConfig<P>,
    storage: Storage,
    shared: Arc<Shared>,
    toolkit: Arc<V2Toolkit>,
    shell_state: PathBuf,
    /// Fixed requests held in memory; after a restart they are rebuilt from
    /// the persisted context under the same request id (§6.3).
    prepared: std::collections::HashMap<String, ModelRequest>,
}

/// Start the driver over its state root. Bootstrap commands use fixed command
/// ids, so restarting over an existing session replays receipts instead of
/// duplicating instances, goals or input (§6.3 row 1).
pub async fn start<P: Provider + 'static>(config: DriverConfig<P>) -> Result<DriverHandle, String> {
    std::fs::create_dir_all(&config.state_root).map_err(|e| format!("state root: {e}"))?;
    let lock = crate::jobs::state_lock(&config.state_root.join("coordinator.lock"))?;
    let storage = Storage::open(&config.session_db, &config.session_id, true, config.storage_queue)?;
    let session_id = config.session_id.clone();
    let instance_id = config.instance_id.clone();
    let goal_id = format!("goal-{session_id}");
    bootstrap(&storage, &instance_id, &config.workspace.to_string_lossy(), &config.goal_limits).await?;
    let (shared, task) = spawn_driver(config, &storage)?;
    Ok(DriverHandle { storage, shared, session_id, instance_id, goal_id, task, _lock: lock })
}

/// Idempotent leader bootstrap (§6.3 row 1): fixed command ids replay
/// receipts on restart instead of duplicating instances, goals or input.
pub(crate) async fn bootstrap(
    storage: &Storage,
    instance_id: &str,
    workspace: &str,
    goal_limits: &Json,
) -> Result<(), String> {
    storage
        .call({
            let instance = instance_id.to_string();
            let workspace = workspace.to_string();
            let goal_limits = goal_limits.clone();
            move |control| {
                let exists: bool = control
                    .connection()
                    .query_row("SELECT COUNT(*) FROM instances WHERE id = ?1", [&instance], |row| row.get::<_, i64>(0))
                    .map(|n| n > 0)
                    .unwrap_or(false);
                if exists {
                    return Ok::<(), String>(());
                }
                control.submit(
                    command("boot-instance", "create_instance", json!({"id": instance, "workspace_ref": workspace})),
                    Identity::User,
                )?;
                control.submit(
                    command(
                        "boot-goal",
                        "create_goal",
                        json!({"id": format!("goal-{}", control.session_id),
                            "instance_id": instance, "limits": goal_limits}),
                    ),
                    Identity::User,
                )?;
                Ok(())
            }
        })
        .await??;
    Ok(())
}

/// One running instance driver: shared flags plus the join handle.
pub(crate) type SpawnedDriver = (Arc<Shared>, tokio::task::JoinHandle<Result<(), String>>);

/// Construct and spawn one instance driver over a shared storage worker
/// (§6.1 single writer). No coordinator lock, no bootstrap: the caller
/// (single-instance `start` or the P3 supervisor) owns both.
pub(crate) fn spawn_driver<P: Provider + 'static>(
    config: DriverConfig<P>,
    storage: &Storage,
) -> Result<SpawnedDriver, String> {
    std::fs::create_dir_all(config.state_root.join("jobs")).map_err(|e| format!("jobs dir: {e}"))?;
    std::fs::create_dir_all(config.state_root.join("artifacts")).map_err(|e| format!("artifacts dir: {e}"))?;
    let shared = Arc::new(Shared {
        wake: tokio::sync::Notify::new(),
        shutdown: AtomicBool::new(false),
        cancel_attempt: Mutex::new(None),
    });
    let toolkit = Arc::new(V2Toolkit::new(
        config.workspace.clone(),
        config.catalog.clone(),
        config.bindings.clone(),
        Some(config.state_root.join("artifacts")),
        Some(config.state_root.join("shell")),
    ));
    let driver = Driver {
        shell_state: config.state_root.join("shell"),
        config,
        storage: storage.clone(),
        shared: shared.clone(),
        toolkit,
        prepared: std::collections::HashMap::new(),
    };
    let task = tokio::spawn(driver.run());
    Ok((shared, task))
}

impl<P: Provider> Driver<P> {
    async fn run(mut self) -> Result<(), String> {
        self.recover().await?;
        loop {
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
            let snapshot = self.snapshot().await?;
            if snapshot.lifecycle == "TERMINATED" {
                return Ok(()); // the supervisor retires this driver (§5.4)
            }
            if snapshot.lifecycle != "ACTIVE" {
                self.wait().await;
                continue;
            }
            let stepped = match snapshot.phase.as_str() {
                "READY" => self.step_ready(&snapshot).await?,
                "MODEL_PENDING" => {
                    self.step_model(&snapshot).await?;
                    true
                }
                "TOOLS_PENDING" => self.step_tools(&snapshot).await?,
                "COMPLETION_PENDING" => {
                    self.step_completion(&snapshot).await?;
                    true
                }
                // WAITING: input or a wake flips the phase in one transaction
                _ => false,
            };
            if !stepped {
                if snapshot.phase == "WAITING" {
                    // parked drains are the wake path for envelope-borne
                    // facts (§5.3, A23): applying one satisfies a matching
                    // wait in the same transaction; due timers close at
                    // poll granularity. Both commands are replay-safe.
                    self.submit(
                        self.command(
                            format!("drain-{}", uuid::Uuid::new_v4()),
                            "drain_inbox",
                            json!({"instance_id": self.config.instance_id}),
                        ),
                        Identity::Instance(self.config.instance_id.clone()),
                    )
                    .await?;
                    self.submit(
                        self.command(format!("timer-{}", uuid::Uuid::new_v4()), "fire_timer", json!({})),
                        Identity::System,
                    )
                    .await?;
                }
                self.wait().await;
            }
        }
    }

    async fn wait(&self) {
        tokio::select! {
            _ = self.shared.wake.notified() => {}
            _ = tokio::time::sleep(self.config.poll) => {}
        }
    }

    fn command(&self, id: impl Into<String>, method: &str, params: Json) -> Command {
        Command { command_id: id.into(), method: method.into(), params }
    }

    async fn submit(&self, cmd: Command, identity: Identity) -> Result<Json, String> {
        self.storage.call(move |control| control.submit(cmd, identity)).await?
    }

    async fn snapshot(&self) -> Result<Snapshot, String> {
        let instance = self.config.instance_id.clone();
        self.storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT lifecycle, phase, revision, context_epoch, active_request_id, active_goal_id
                         FROM instances WHERE id = ?1",
                        [&instance],
                        |row| {
                            Ok(Snapshot {
                                lifecycle: row.get(0)?,
                                phase: row.get(1)?,
                                revision: row.get(2)?,
                                epoch: row.get(3)?,
                                active_request: row.get(4)?,
                                active_goal: row.get(5)?,
                            })
                        },
                    )
                    .map_err(|e| format!("snapshot {instance}: {e}"))
            })
            .await?
    }

    async fn context_entries(&self, snapshot: &Snapshot) -> Result<Vec<ContextEntry>, String> {
        let instance = self.config.instance_id.clone();
        let epoch = snapshot.epoch;
        self.storage
            .call(move |control| {
                let mut stmt = control
                    .connection()
                    .prepare(
                        "SELECT id, kind, message_json, refs_json FROM context_entries
                         WHERE instance_id = ?1 AND epoch = ?2 ORDER BY idx",
                    )
                    .map_err(|e| format!("context prepare: {e}"))?;
                let rows = stmt
                    .query_map(rusqlite::params![instance, epoch], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })
                    .map_err(|e| format!("context query: {e}"))?;
                let mut entries = Vec::new();
                for row in rows {
                    let (id, kind, message, refs) = row.map_err(|e| format!("context row: {e}"))?;
                    let kind = match kind.as_str() {
                        "user" => EntryKind::User,
                        "assistant" => EntryKind::Assistant,
                        "tool_result" => EntryKind::ToolResult,
                        "note" => EntryKind::Note,
                        _ => EntryKind::Note,
                    };
                    let mut entry = ContextEntry::new(
                        id,
                        kind,
                        serde_json::from_str(&message).map_err(|e| format!("context message: {e}"))?,
                    );
                    entry.refs = serde_json::from_str(&refs).unwrap_or_default();
                    entries.push(entry);
                }
                Ok(entries)
            })
            .await?
    }

    fn kernel(&self, snapshot: &Snapshot) -> KernelInstance {
        KernelInstance::new(self.config.instance_id.clone(), snapshot.epoch as u64, self.config.profile.clone())
    }

    /// Kernel with the collaboration surface (§5.2): wait is always
    /// offered; send/delegate/spawn schemas appear only while the instance
    /// holds a matching grant (§5.1). The dispatch boundary re-checks the
    /// grant regardless (§6.1), so a stale schema never authorizes.
    async fn team_kernel(&self, snapshot: &Snapshot) -> Result<KernelInstance, String> {
        let instance = self.config.instance_id.clone();
        let actions = self
            .storage
            .call(move |control| {
                let conn = control.connection();
                let holds = |action: &str| -> Result<bool, String> {
                    conn.query_row(
                        "SELECT COUNT(*) FROM grants WHERE subject = ?1 AND action = ?2 AND revoked_at IS NULL",
                        rusqlite::params![instance, action],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|n| n > 0)
                    .map_err(|e| format!("grant read: {e}"))
                };
                let mut actions = vec![teamagents_core::kernel::WAIT_TOOL];
                if holds("message")? {
                    actions.push(teamagents_core::kernel::SEND_TOOL);
                }
                if holds("delegate")? {
                    actions.push(teamagents_core::kernel::DELEGATE_TOOL);
                }
                if holds("manage")? {
                    actions.push(teamagents_core::kernel::SPAWN_TOOL);
                }
                Ok::<Vec<&str>, String>(actions)
            })
            .await??;
        let mut profile = self.config.profile.clone();
        profile.tools.extend(teamagents_core::kernel::collaboration_tool_schemas(&actions));
        Ok(KernelInstance::new(self.config.instance_id.clone(), snapshot.epoch as u64, profile))
    }

    /// Crash recovery (§6.3): classify the persisted position once, then let
    /// the normal phase steps do the work.
    async fn recover(&mut self) -> Result<(), String> {
        let snapshot = self.snapshot().await?;
        if snapshot.phase == "MODEL_PENDING" {
            if let Some(request_id) = snapshot.active_request.clone() {
                let selected: Option<String> = self
                    .storage
                    .call({
                        let request = request_id.clone();
                        move |control| {
                            control
                                .connection()
                                .query_row(
                                    "SELECT selected_attempt_id FROM model_requests WHERE request_id = ?1 AND status = 'PENDING'",
                                    [&request],
                                    |row| row.get(0),
                                )
                                .ok()
                                .flatten()
                        }
                    })
                    .await?;
                if selected.is_none() {
                    // an in-flight attempt left no row: record the possible
                    // duplicate billing honestly, then retry under one policy
                    let lost = format!("{request_id}/a{}", self.attempt_count(&request_id).await? + 1);
                    self.submit(
                        self.command(
                            format!("recover-lost-{lost}"),
                            "record_attempt",
                            json!({"attempt_id": lost, "request_id": request_id, "status": "FAILED",
                                   "error_class": "lost", "unknown_usage": true}),
                        ),
                        Identity::System,
                    )
                    .await?;
                }
            }
        }
        // orphan STAGING artifacts (crash between stage and the referencing
        // commit) are marked ABANDONED, never silently deleted (§4.3)
        let orphans: Vec<String> = self
            .storage
            .call(|control| {
                let mut stmt = control
                    .connection()
                    .prepare(
                        "SELECT id FROM artifacts WHERE completeness = 'STAGING'
                         AND id NOT IN (SELECT response_ref FROM attempts WHERE response_ref IS NOT NULL)",
                    )
                    .map_err(|e| format!("orphan prepare: {e}"))?;
                let rows = stmt.query_map([], |row| row.get(0)).map_err(|e| format!("orphan query: {e}"))?;
                rows.collect::<Result<Vec<String>, _>>().map_err(|e| format!("orphan collect: {e}"))
            })
            .await??;
        for orphan in orphans {
            let _ = self
                .submit(
                    self.command(format!("abandon-{orphan}"), "artifact_abandon", json!({"id": orphan})),
                    Identity::System,
                )
                .await;
        }
        Ok(())
    }

    async fn attempt_count(&self, request_id: &str) -> Result<i64, String> {
        let request = request_id.to_string();
        self.storage
            .call(move |control| {
                control
                    .connection()
                    .query_row("SELECT COUNT(*) FROM attempts WHERE request_id = ?1", [&request], |row| row.get(0))
                    .map_err(|e| format!("attempt count: {e}"))
            })
            .await?
    }

    /// READY: consume input or last tool results into a fixed request, then
    /// reserve budget and register it — one transaction (§3).
    async fn step_ready(&mut self, snapshot: &Snapshot) -> Result<bool, String> {
        // queued envelopes apply at this safe boundary, before the request
        // is fixed (§5.3); a replayed drain is a no-op. An applying drain
        // bumps the instance revision — begin against the post-drain value
        // so the executor check (§6.1) compares the freshest snapshot.
        let drained = self
            .submit(
                self.command(
                    format!("drain-{}", uuid::Uuid::new_v4()),
                    "drain_inbox",
                    json!({"instance_id": self.config.instance_id}),
                ),
                Identity::Instance(self.config.instance_id.clone()),
            )
            .await?;
        let revision = drained["revision"].as_i64().unwrap_or(snapshot.revision);
        let entries = self.context_entries(snapshot).await?;
        // nothing unconsumed: the last word was the assistant's — idle,
        // unless assigned work still waits in the queue (§5.3): a settled
        // task's finish message must not park a worker with open tasks
        if entries.last().is_none_or(|entry| entry.kind == EntryKind::Assistant) && self.open_tasks().await? == 0 {
            return Ok(false);
        }
        // adopt assigned work: the turn that addresses a PENDING task also
        // marks it RUNNING; a lost race just means someone else moved it
        if let Some(task_id) = self.oldest_task("PENDING").await? {
            let started = self
                .submit(
                    self.command(format!("start-task-{task_id}"), "start_task", json!({"task_id": task_id})),
                    Identity::Instance(self.config.instance_id.clone()),
                )
                .await;
            match started {
                Ok(_) => {}
                Err(error) if error.contains("not startable") => {}
                Err(error) => return Err(error),
            }
        }
        let kernel = self.team_kernel(snapshot).await?;
        let request_id = format!("req-{}", uuid::Uuid::new_v4());
        let request = kernel.prepare_request(&entries, &request_id);
        let begun = self
            .submit(
                self.command(
                    format!("begin-{request_id}"),
                    "begin_request",
                    json!({"instance_id": self.config.instance_id, "request_id": request_id,
                           "revision": revision, "est_prompt_tokens": request.est_prompt_tokens}),
                ),
                Identity::Instance(self.config.instance_id.clone()),
            )
            .await;
        match begun {
            Ok(result) if result["budget_refused"] == json!(true) => {
                // user budgets stay authoritative (§8): park with the reason
                let reason = result["reason"].as_str().unwrap_or("budget refused").to_string();
                self.submit(
                    self.command(
                        format!("park-budget-{}", uuid::Uuid::new_v4()),
                        "set_lifecycle",
                        json!({"instance_id": self.config.instance_id, "lifecycle": "PARKED", "reason": reason}),
                    ),
                    Identity::System,
                )
                .await?;
                Ok(false)
            }
            Ok(_) => {
                self.prepared.insert(request_id, request);
                Ok(true)
            }
            Err(error) if error.contains("stale executor") || error.contains("not READY") => {
                // someone else advanced the instance; re-read and continue
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }

    /// MODEL_PENDING: transport attempts with single-owner retry, then the
    /// atomic response import (§4.2).
    async fn step_model(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        let request_id = snapshot.active_request.clone().ok_or("MODEL_PENDING without active request")?;
        // recovery: a selected complete attempt is imported from its artifact,
        // never re-requested (§6.3)
        let selected: Option<(String, Option<String>)> = self
            .storage
            .call({
                let request = request_id.clone();
                move |control| {
                    control
                        .connection()
                        .query_row(
                            "SELECT a.attempt_id, a.response_ref FROM attempts a
                             JOIN model_requests r ON r.selected_attempt_id = a.attempt_id
                             WHERE r.request_id = ?1 AND r.status = 'PENDING'",
                            [&request],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .ok()
                }
            })
            .await?;
        if let Some((_, Some(response_ref))) = selected {
            return self.import_stored_response(&request_id, &response_ref, snapshot).await;
        }
        let request = match self.prepared.get(&request_id) {
            Some(request) => request.clone(),
            None => {
                let entries = self.context_entries(snapshot).await?;
                self.team_kernel(snapshot).await?.prepare_request(&entries, &request_id)
            }
        };
        let attempt = self.attempt_count(&request_id).await? + 1;
        let attempt_id = format!("{request_id}/a{attempt}");
        let cancel = Cancel::new();
        *self.shared.cancel_attempt.lock().unwrap() = Some(cancel.clone());
        let started = std::time::Instant::now();
        let outcome = {
            let provider = &self.config.provider;
            let request_ref = &request;
            let cancel_ref = &cancel;
            let mut preview = |_: ProviderEvent| {};
            provider.complete(request_ref, cancel_ref, &mut preview).await
        };
        self.shared.cancel_attempt.lock().unwrap().take();
        match outcome {
            Ok(outcome) => {
                let response_ref = self.store_response_artifact(&attempt_id, &outcome).await?;
                let recorded = self
                    .submit(
                        self.command(
                            format!("attempt-{attempt_id}"),
                            "record_attempt",
                            json!({"attempt_id": attempt_id, "request_id": request_id, "status": "COMPLETE",
                                   "elapsed_ms": outcome.elapsed_ms, "usage": outcome.response.usage.map(|u| json!({
                                       "prompt_tokens": u.prompt, "completion_tokens": u.completion, "total_tokens": u.total})),
                                   "response_ref": response_ref, "publish": [response_ref]}),
                        ),
                        Identity::System,
                    )
                    .await;
                let recorded = match recorded {
                    Ok(value) => value,
                    Err(error) if error.contains("closed requests") => {
                        // the turn was cancelled mid-flight: archive is honest,
                        // but the orphaned staged artifact must be marked
                        let _ = self
                            .submit(
                                self.command(
                                    format!("abandon-{response_ref}"),
                                    "artifact_abandon",
                                    json!({"id": response_ref}),
                                ),
                                Identity::System,
                            )
                            .await;
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                };
                if recorded["selected"] != json!(true) {
                    return Ok(()); // a late complete: archived and billed (§7)
                }
                let entry_id = format!("{}:assistant", request_id);
                let interpretation = self.kernel(snapshot).interpret_response(&outcome.response, &entry_id);
                self.import_interpretation(&request_id, interpretation, snapshot).await
            }
            Err(error) => {
                let class = format!("{:?}", error.class);
                let _ = self
                    .submit(
                        self.command(
                            format!("attempt-{attempt_id}"),
                            "record_attempt",
                            json!({"attempt_id": attempt_id, "request_id": request_id, "status": "FAILED",
                                   "error_class": class, "elapsed_ms": started.elapsed().as_millis() as u64}),
                        ),
                        Identity::System,
                    )
                    .await;
                match error.class {
                    ErrorClass::Transient if attempt <= self.config.max_retries as i64 => {
                        let wait = error
                            .retry_after
                            .unwrap_or_else(|| Duration::from_millis((500u64 << (attempt as u32).min(5)).min(8000)))
                            .min(Duration::from_secs(30));
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = cancel.cancelled() => {}
                        }
                        Ok(())
                    }
                    ErrorClass::Interrupted => {
                        self.submit(
                            self.command(
                                format!("cancel-{request_id}"),
                                "cancel_request",
                                json!({"request_id": request_id}),
                            ),
                            Identity::System,
                        )
                        .await?;
                        Ok(())
                    }
                    other => {
                        // permanent failures park the instance once (A07): the
                        // input stays, no turn storm
                        let reason = match other {
                            ErrorClass::ContextOverflow => format!("context overflow: {}", error.message),
                            ErrorClass::Transient => format!("transient retries exhausted: {}", error.message),
                            ErrorClass::Permanent => format!("permanent model error: {}", error.message),
                            ErrorClass::Interrupted => unreachable!(),
                        };
                        let _ = self
                            .submit(
                                self.command(
                                    format!("fail-{request_id}"),
                                    "fail_request",
                                    json!({"request_id": request_id, "reason": reason, "park": true}),
                                ),
                                Identity::System,
                            )
                            .await;
                        Ok(())
                    }
                }
            }
        }
    }

    /// Import a response that was fully received before a crash: the artifact
    /// is read, the import commits once (§6.3 row 3).
    async fn import_stored_response(
        &mut self,
        request_id: &str,
        response_ref: &str,
        snapshot: &Snapshot,
    ) -> Result<(), String> {
        let stored: Option<(String, String)> = self
            .storage
            .call({
                let artifact = response_ref.to_string();
                move |control| {
                    control
                        .connection()
                        .query_row(
                            "SELECT storage_ref, completeness FROM artifacts WHERE id = ?1",
                            [&artifact],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .ok()
                }
            })
            .await?;
        let Some((path, completeness)) = stored else {
            return Err(format!("response artifact {response_ref} missing from the catalog"));
        };
        if completeness != "LIVE" {
            return Err(format!("response artifact {response_ref} is {completeness}, not LIVE"));
        }
        let body = std::fs::read_to_string(&path).map_err(|e| format!("read response artifact {path}: {e}"))?;
        let raw: Json = serde_json::from_str(&body).map_err(|e| format!("parse response artifact {path}: {e}"))?;
        let response = ModelResponse {
            message: raw["message"].clone(),
            usage: Usage::from_json(&json!({"usage": raw["usage"]})),
            native: raw["native"].clone(),
        };
        let entry_id = format!("{}:assistant", request_id);
        let interpretation = self.kernel(snapshot).interpret_response(&response, &entry_id);
        self.import_interpretation(request_id, interpretation, snapshot).await
    }

    async fn import_interpretation(
        &mut self,
        request_id: &str,
        interpretation: Interpretation,
        _snapshot: &Snapshot,
    ) -> Result<(), String> {
        let decision_id = format!("d-{request_id}");
        let (intents, completion, wait) = match &interpretation.output {
            KernelOutput::ToolIntents(intents) => (
                intents
                    .iter()
                    .map(|intent| json!({"index": intent.index, "call_id": intent.call_id, "name": intent.name, "args": intent.args}))
                    .collect::<Vec<_>>(),
                Json::Null,
                Json::Null,
            ),
            KernelOutput::Completion(candidate) => (vec![], serde_json::to_value(candidate).unwrap_or(Json::Null), Json::Null),
            KernelOutput::Wait(wait) => (vec![], Json::Null, wait.clone()),
            KernelOutput::Reply(_) => (vec![], Json::Null, Json::Null),
        };
        let mut params = json!({
            "request_id": request_id,
            "decision_id": decision_id,
            "entry": interpretation.entry.message,
            "intents": intents,
            "grant_revision": 0,
        });
        if !completion.is_null() {
            params["completion"] = completion;
        }
        if !wait.is_null() {
            params["wait"] = wait;
        }
        for note in &interpretation.notes {
            // protocol notes join the context as their own entry in a follow-up
            // input; they never block the import itself
            eprintln!("driver: protocol note for {request_id}: {note}");
        }
        let imported = self
            .submit(self.command(format!("import-{decision_id}"), "import_response", params), Identity::System)
            .await;
        match imported {
            Ok(_) => {
                self.prepared.remove(request_id);
                Ok(())
            }
            // recovery replay: already imported before the crash
            Err(error) if error.contains("already imported") => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Persist the complete response as an artifact before referencing it
    /// (§4.3): tmp file → sync → rename → stage; the LIVE flip rides the
    /// record_attempt transaction.
    async fn store_response_artifact(
        &self,
        attempt_id: &str,
        outcome: &crate::providers::AttemptOutcome,
    ) -> Result<String, String> {
        let artifact_id = format!("resp-{}", attempt_id.replace('/', "-"));
        let dir = self.config.state_root.join("artifacts");
        let final_path = dir.join(format!("{artifact_id}.json"));
        let tmp_path = dir.join(format!("{artifact_id}.tmp"));
        let body = json!({"message": outcome.response.message, "usage": outcome.response.usage, "native": outcome.response.native});
        let bytes = body.to_string();
        std::fs::write(&tmp_path, &bytes).map_err(|e| format!("write response artifact: {e}"))?;
        std::fs::File::open(&tmp_path)
            .map_err(|e| format!("open response artifact: {e}"))?
            .sync_all()
            .map_err(|e| format!("sync: {e}"))?;
        std::fs::rename(&tmp_path, &final_path).map_err(|e| format!("publish response artifact: {e}"))?;
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(bytes.as_bytes()));
        self.submit(
            self.command(
                format!("stage-{artifact_id}"),
                "artifact_stage",
                json!({"id": artifact_id, "digest": digest, "size": bytes.len(), "kind": "model_response",
                       "owner_scope": "request", "storage_ref": final_path.to_string_lossy(), "owner_ref": outcome_request(attempt_id)}),
            ),
            Identity::System,
        )
        .await?;
        Ok(artifact_id)
    }

    /// TOOLS_PENDING: dispatch/execute the fixed intents in order (§6.2) and
    /// import terminal receipts; DISPATCH_COMMITTED at entry means recovery.
    async fn step_tools(&mut self, snapshot: &Snapshot) -> Result<bool, String> {
        let ops: Vec<(String, String, String, bool)> = self
            .storage
            .call({
                let instance = self.config.instance_id.clone();
                move |control| {
                    let mut stmt = control
                        .connection()
                        .prepare(
                            "SELECT o.operation_id, o.status, o.intent_json, o.cancel_requested
                             FROM operations o
                             JOIN decisions d ON o.decision_id = d.decision_id
                             JOIN model_requests r ON d.request_id = r.request_id
                             WHERE r.instance_id = ?1
                               AND o.status IN ('PREPARED', 'DISPATCH_COMMITTED', 'RUNNING')
                             ORDER BY o.decision_id, o.tool_index",
                        )
                        .map_err(|e| format!("ops prepare: {e}"))?;
                    let rows = stmt
                        .query_map([&instance], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, i64>(3)? != 0,
                            ))
                        })
                        .map_err(|e| format!("ops query: {e}"))?;
                    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("ops collect: {e}"))
                }
            })
            .await??;
        if ops.is_empty() {
            return Ok(true); // consumption already flipped the phase; re-read
        }
        for (operation_id, status, intent_json, _cancel_requested) in ops {
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Ok(false);
            }
            let intent: Json = serde_json::from_str(&intent_json).map_err(|e| format!("intent {operation_id}: {e}"))?;
            if status == "PREPARED" {
                // unique command id per dispatch attempt: the operation state
                // machine is the idempotency anchor, not the command receipt —
                // a replayed APPROVAL_REQUIRED receipt would block dispatch
                // forever after the user approves (§6.2)
                let dispatched = self
                    .submit(
                        self.command(
                            format!("dispatch-{operation_id}-{}", uuid::Uuid::new_v4()),
                            "dispatch_operation",
                            json!({"operation_id": operation_id,
                                   "approval_required": intent["name"].as_str() == Some("shell") && self.config.require_shell_approval,
                                   "permission_revision": 0}),
                        ),
                        Identity::System,
                    )
                    .await;
                match dispatched {
                    Ok(result) if result["status"] == json!("APPROVAL_REQUIRED") => {
                        // wait for the user's decision; approve/deny wakes us
                        return Ok(false);
                    }
                    Ok(_) => {}
                    Err(error) if error.contains("not dispatchable") || error.contains("lost the dispatch race") => {
                        continue; // another executor advanced it; re-read
                    }
                    Err(error) => {
                        self.complete_with_error(&operation_id, &intent, "dispatch_refused", &error).await?;
                        continue;
                    }
                }
            }
            // DISPATCH_COMMITTED/RUNNING here (fresh or recovered): execute or reconnect
            let name = intent["name"].as_str().unwrap_or("");
            match name {
                "shell" => self.execute_shell(&operation_id, &intent).await?,
                "read_history" => self.execute_readback(&operation_id, &intent, snapshot).await?,
                teamagents_core::kernel::SEND_TOOL
                | teamagents_core::kernel::DELEGATE_TOOL
                | teamagents_core::kernel::SPAWN_TOOL => self.execute_collaboration(&operation_id, &intent).await?,
                _ => self.execute_inline(&operation_id, &intent).await?,
            }
        }
        Ok(true)
    }

    async fn execute_shell(&mut self, operation_id: &str, intent: &Json) -> Result<(), String> {
        let job_dir = self.config.state_root.join("jobs").join(operation_id.replace(['/', ':'], "_"));
        let mode = ShellMode::from_permissions(Some(&self.config.permissions))?;
        let command_text = intent["args"]["command"].as_str().unwrap_or("");
        let timeout = intent["args"]["timeout"].as_u64().unwrap_or(120);
        let network = intent["args"]["network"].as_bool().unwrap_or(false);
        let spec =
            match shell_command_spec(command_text, &self.config.workspace, network, Some(&self.shell_state), mode) {
                Ok(spec) => spec,
                Err(error) => {
                    // isolation/setup failure: the command never started (A14)
                    let receipt =
                        self.receipt_skeleton(operation_id, intent, false, json!({"error": error.reason}).to_string());
                    let receipt = ToolReceipt {
                        error: Some(ReceiptError { class: error.class.into(), reason: error.reason.clone() }),
                        ..receipt
                    };
                    return self
                        .complete_op(
                            operation_id,
                            "FAILED",
                            &serde_json::to_value(receipt).unwrap_or(Json::Null),
                            vec![],
                        )
                        .await;
                }
            };
        let job = JobSpec {
            job_id: operation_id.to_string(),
            program: spec.program,
            args: spec.args,
            cwd: spec.cwd,
            env: spec.env,
            deadline_ms: crate::jobs::now_ms() + timeout * 1000,
            // filled by the runner spawn; reconnects read it from job.json
            token: String::new(),
        };
        if !job_dir.join("journal.json").exists() {
            // no runner ever persisted READY: (re)spawn over the same dir —
            // the persisted token keeps the job identity stable (A11)
            if let Err(error) = client::spawn(&job_dir, &job).await {
                // spawn failed before the READY handshake and GO is only sent
                // after READY, so the command never started (A14): fail the
                // op honestly instead of killing the driver task
                let reason = format!("job runner did not start: {error}");
                let receipt = self.receipt_skeleton(operation_id, intent, false, json!({"error": reason}).to_string());
                let receipt = ToolReceipt { error: Some(ReceiptError { class: "spawn".into(), reason }), ..receipt };
                return self
                    .complete_op(operation_id, "FAILED", &serde_json::to_value(receipt).unwrap_or(Json::Null), vec![])
                    .await;
            }
        }
        // GO dedups: a replay never starts a second command (A10)
        let _ = client::go(&job_dir).await;
        let journal = self.await_job(operation_id, &job_dir).await?;
        let output = client::read_output(&job_dir, MAX_OUTPUT);
        let (status, ok, error_class, reason) = match journal.state.as_str() {
            "SUCCEEDED" => ("SUCCEEDED", true, None, None),
            "FAILED" => ("FAILED", false, Some("exit"), journal.exit_code.map(|c| format!("command exited {c}"))),
            "CANCELLED" => {
                let timed_out = journal.finished_ms.is_some()
                    && journal.started_ms.is_some_and(|s| {
                        journal.finished_ms.unwrap().saturating_sub(s) >= (job.deadline_ms.saturating_sub(s))
                    });
                let reason = if journal.cancel_saved {
                    "cancelled by user".to_string()
                } else {
                    format!("command timed out after {timeout}s")
                };
                let _ = timed_out;
                ("CANCELLED", false, Some(if journal.cancel_saved { "cancelled" } else { "timeout" }), Some(reason))
            }
            "OUTCOME_UNKNOWN" => (
                "OUTCOME_UNKNOWN",
                false,
                Some("outcome_unknown"),
                Some("runner crashed across the execution boundary; the outcome is unknown".into()),
            ),
            other => return Err(format!("job {operation_id} in unexpected state {other}")),
        };
        // large outputs spill into a referenced artifact; the receipt keeps
        // the model-facing head (§4.3)
        let mut publish = vec![];
        let mut output_ref = None;
        let full = std::fs::metadata(job_dir.join("output.log")).map(|m| m.len()).unwrap_or(0);
        let content = if full > MAX_OUTPUT as u64 && full <= MAX_ARTIFACT_BYTES {
            let artifact_id = format!("out-{}", operation_id.replace(['/', ':'], "-"));
            let body = std::fs::read(job_dir.join("output.log")).map_err(|e| format!("read job output: {e}"))?;
            use sha2::{Digest, Sha256};
            let digest = format!("{:x}", Sha256::digest(&body));
            let final_path = self.config.state_root.join("artifacts").join(format!("{artifact_id}.log"));
            std::fs::write(&final_path, &body).map_err(|e| format!("write output artifact: {e}"))?;
            self.submit(
                self.command(
                    format!("stage-{artifact_id}"),
                    "artifact_stage",
                    json!({"id": artifact_id, "digest": digest, "size": body.len(), "kind": "tool_output",
                           "owner_scope": "operation", "storage_ref": final_path.to_string_lossy(), "owner_ref": operation_id}),
                ),
                Identity::System,
            )
            .await?;
            output_ref = Some(artifact_id.clone());
            publish.push(artifact_id);
            json!({"output": format!("{output}\n[truncated: full output stored as {full} bytes]")}).to_string()
        } else {
            match error_class {
                Some(_) if output.is_empty() => json!({"error": reason.clone().unwrap_or_default()}).to_string(),
                _ => {
                    let suffix =
                        if ok { String::new() } else { format!("\n(exit {})", journal.exit_code.unwrap_or(-1)) };
                    json!({"output": format!("{output}{suffix}")}).to_string()
                }
            }
        };
        let mut receipt = self.receipt_skeleton(operation_id, intent, journal.starts > 0, content);
        receipt.ok = ok;
        receipt.mode = Some(match mode {
            ShellMode::Sandbox => "approved_scope".into(),
            ShellMode::Host => "full_auto".into(),
        });
        receipt.cwd = Some(job.cwd);
        receipt.exit_code = journal.exit_code;
        receipt.signal = journal.signal;
        receipt.duration_ms = match (journal.started_ms, journal.finished_ms) {
            (Some(start), Some(end)) => end.saturating_sub(start),
            _ => 0,
        };
        receipt.output_ref = output_ref;
        receipt.error =
            error_class.map(|class| ReceiptError { class: class.into(), reason: reason.unwrap_or_default() });
        self.complete_op(operation_id, status, &serde_json::to_value(receipt).unwrap_or(Json::Null), publish).await
    }

    /// Poll the job until terminal; cancellation is forwarded once the cancel
    /// request is durable in the database (§6.4).
    async fn await_job(&mut self, operation_id: &str, job_dir: &Path) -> Result<Journal, String> {
        let mut cancel_sent = false;
        loop {
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Err("driver shutdown with a job in flight".into());
            }
            match client::status(job_dir).await {
                Ok(journal) if journal.terminal() => return Ok(journal),
                Ok(_) => {}
                Err(_) => {
                    // runner unreachable: the persisted journal judges (A11)
                    if let Some(journal) = client::persisted_journal(job_dir) {
                        if journal.terminal() {
                            return Ok(journal);
                        }
                        // dead mid-execution: a fresh runner over the same dir
                        // recovers the journal as OUTCOME_UNKNOWN (§6.3)
                        let spec: JobSpec = serde_json::from_slice(
                            &std::fs::read(job_dir.join("job.json")).map_err(|e| format!("read job.json: {e}"))?,
                        )
                        .map_err(|e| format!("parse job.json: {e}"))?;
                        if client::spawn(job_dir, &spec).await.is_ok() {
                            if let Ok(recovered) = client::status(job_dir).await {
                                if recovered.terminal() {
                                    return Ok(recovered);
                                }
                                // READY again: the original GO may have died
                                // with the old runner; a duplicate GO is a
                                // no-op once a command started (A10)
                                let _ = client::go(job_dir).await;
                            }
                        }
                    } else {
                        return Err(format!("job {operation_id} vanished without a journal"));
                    }
                }
            }
            if !cancel_sent {
                let flagged: bool = self
                    .storage
                    .call({
                        let op = operation_id.to_string();
                        move |control| {
                            control
                                .connection()
                                .query_row(
                                    "SELECT cancel_requested FROM operations WHERE operation_id = ?1",
                                    [&op],
                                    |row| row.get::<_, i64>(0),
                                )
                                .map(|v| v != 0)
                                .unwrap_or(false)
                        }
                    })
                    .await?;
                if flagged {
                    let _ = client::cancel(job_dir).await;
                    cancel_sent = true;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn execute_readback(&mut self, operation_id: &str, intent: &Json, snapshot: &Snapshot) -> Result<(), String> {
        let entries = self.context_entries(snapshot).await?;
        let typed = ToolIntent {
            index: intent["index"].as_u64().unwrap_or(0) as usize,
            call_id: intent["call_id"].as_str().unwrap_or("").into(),
            name: intent["name"].as_str().unwrap_or("").into(),
            args: intent["args"].clone(),
            args_hash: intent["args_hash"].as_str().unwrap_or("").into(),
        };
        let receipt = readback_receipt(operation_id, &typed, &entries);
        self.complete_op(
            operation_id,
            if receipt.ok { "SUCCEEDED" } else { "FAILED" },
            &serde_json::to_value(receipt).unwrap_or(Json::Null),
            vec![],
        )
        .await
    }

    /// Collaboration intents (§5.2): send/delegate/spawn run as control
    /// commands under the instance identity — the grant check inside the
    /// command is the second line behind the dispatch re-check (§6.1). The
    /// command result becomes the tool receipt the model observes.
    async fn execute_collaboration(&mut self, operation_id: &str, intent: &Json) -> Result<(), String> {
        let name = intent["name"].as_str().unwrap_or("");
        let args = &intent["args"];
        let (method, params) = match name {
            teamagents_core::kernel::SEND_TOOL => {
                ("send_message", json!({"recipient": args["recipient"], "text": args["text"]}))
            }
            teamagents_core::kernel::DELEGATE_TOOL => {
                let task_id =
                    args["task_id"].as_str().map(str::to_string).unwrap_or_else(|| format!("t-{operation_id}"));
                let mut params = json!({"task_id": task_id, "assignee": args["assignee"],
                                        "description": args["description"],
                                        "goal_id": format!("goal-{}", self.config.session_id)});
                if let Some(acceptance) = args.get("acceptance_refs") {
                    params["acceptance_refs"] = acceptance.clone();
                }
                ("delegate_task", params)
            }
            teamagents_core::kernel::SPAWN_TOOL => {
                let mut profile = self.config.profile.clone();
                profile.instructions = args["instructions"].as_str().unwrap_or("").to_string();
                let mut params = json!({"instance_id": args["instance_id"], "instructions": args["instructions"],
                                        "profile": {"model": profile.model, "instructions": profile.instructions,
                                                    "tools": profile.tools, "options": profile.options,
                                                    "context_window": profile.context_window},
                                        "workspace_ref": self.config.workspace.to_string_lossy(),
                                        "goal_id": format!("goal-{}", self.config.session_id)});
                if let Some(task) = args.get("task") {
                    params["task"] = task.clone();
                }
                ("spawn_instance", params)
            }
            other => return Err(format!("execute_collaboration: unknown tool {other}")),
        };
        let result = self
            .submit(
                self.command(format!("collab-{operation_id}"), method, params),
                Identity::Instance(self.config.instance_id.clone()),
            )
            .await;
        match result {
            Ok(value) => {
                let receipt = self.receipt_skeleton(operation_id, intent, true, value.to_string());
                let receipt = teamagents_core::kernel::ToolReceipt { ok: true, ..receipt };
                self.complete_op(
                    operation_id,
                    "SUCCEEDED",
                    &serde_json::to_value(receipt).unwrap_or(Json::Null),
                    vec![],
                )
                .await
            }
            Err(error) => self.complete_with_error(operation_id, intent, "collaboration", &error).await,
        }
    }

    /// Read-only inline tools (web search/fetch): no runner needed (§6.2);
    /// recovery re-executes them because they are idempotent reads.
    async fn execute_inline(&mut self, operation_id: &str, intent: &Json) -> Result<(), String> {
        let typed = ToolIntent {
            index: intent["index"].as_u64().unwrap_or(0) as usize,
            call_id: intent["call_id"].as_str().unwrap_or("").into(),
            name: intent["name"].as_str().unwrap_or("").into(),
            args: intent["args"].clone(),
            args_hash: intent["args_hash"].as_str().unwrap_or("").into(),
        };
        let toolkit = self.toolkit.clone();
        let op = operation_id.to_string();
        let mode = ShellMode::from_permissions(Some(&self.config.permissions))?;
        let receipt = tokio::task::spawn_blocking(move || {
            toolkit.call(&op, &typed, &crate::gateway::TurnControl::default(), mode)
        })
        .await
        .map_err(|e| format!("tool worker join: {e}"))?;
        self.complete_op(
            operation_id,
            if receipt.ok { "SUCCEEDED" } else { "FAILED" },
            &serde_json::to_value(receipt).unwrap_or(Json::Null),
            vec![],
        )
        .await
    }

    async fn complete_with_error(
        &mut self,
        operation_id: &str,
        intent: &Json,
        class: &str,
        reason: &str,
    ) -> Result<(), String> {
        let mut receipt = self.receipt_skeleton(operation_id, intent, false, json!({"error": reason}).to_string());
        receipt.error = Some(ReceiptError { class: class.into(), reason: reason.into() });
        self.complete_op(operation_id, "FAILED", &serde_json::to_value(receipt).unwrap_or(Json::Null), vec![]).await
    }

    fn receipt_skeleton(&self, operation_id: &str, intent: &Json, started: bool, content: String) -> ToolReceipt {
        ToolReceipt {
            operation_id: operation_id.into(),
            tool: intent["name"].as_str().unwrap_or("").into(),
            args_hash: intent["args_hash"].as_str().unwrap_or("").into(),
            ok: false,
            started,
            mode: None,
            cwd: None,
            exit_code: None,
            signal: None,
            duration_ms: 0,
            output_ref: None,
            content,
            error: None,
        }
    }

    async fn complete_op(
        &mut self,
        operation_id: &str,
        status: &str,
        receipt: &Json,
        publish: Vec<String>,
    ) -> Result<(), String> {
        let mut params = json!({"operation_id": operation_id, "status": status, "receipt": receipt});
        if !publish.is_empty() {
            params["publish"] = json!(publish);
        }
        let completed = self
            .submit(self.command(format!("complete-{operation_id}"), "complete_operation", params), Identity::System)
            .await;
        match completed {
            Ok(_) => Ok(()),
            // a replay after a lost reply returns the stored state (§4.2)
            Err(error) if error.contains("already terminal") => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// COMPLETION_PENDING: close the goal against the stored candidate (§8).
    /// Open assigned tasks (PENDING + RUNNING) — the queue depth that keeps
    /// a worker from parking on its own assistant tail (§5.3).
    async fn open_tasks(&self) -> Result<i64, String> {
        let instance = self.config.instance_id.clone();
        self.storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT COUNT(*) FROM tasks WHERE assignee = ?1 AND status IN ('PENDING', 'RUNNING')",
                        [&instance],
                        |row| row.get(0),
                    )
                    .map_err(|e| format!("open tasks: {e}"))
            })
            .await?
    }

    /// Oldest assigned task in the given status — FIFO queue order.
    async fn oldest_task(&self, status: &str) -> Result<Option<String>, String> {
        let instance = self.config.instance_id.clone();
        let status = status.to_string();
        self.storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT id FROM tasks WHERE assignee = ?1 AND status = ?2 ORDER BY rowid LIMIT 1",
                        rusqlite::params![instance, status],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| format!("oldest task: {e}"))
            })
            .await?
    }

    /// Oldest open (PENDING or RUNNING) assigned task.
    async fn oldest_open_task(&self) -> Result<Option<String>, String> {
        let instance = self.config.instance_id.clone();
        self.storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT id FROM tasks WHERE assignee = ?1 AND status IN ('PENDING', 'RUNNING') ORDER BY rowid LIMIT 1",
                        [&instance],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| format!("oldest open task: {e}"))
            })
            .await?
    }

    /// The stored finish candidate — read from its decision, never re-taken
    /// from the model (mirrors complete_goal's read).
    async fn completion_candidate(&self) -> Result<Json, String> {
        let instance = self.config.instance_id.clone();
        let stored: Option<String> = self
            .storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT d.completion_json FROM decisions d
                         JOIN model_requests r ON d.request_id = r.request_id
                         WHERE r.instance_id = ?1 AND d.completion_json IS NOT NULL
                         ORDER BY d.rowid DESC LIMIT 1",
                        [&instance],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| format!("completion candidate: {e}"))
            })
            .await??;
        Ok(stored.as_deref().and_then(|c| serde_json::from_str(c).ok()).unwrap_or(Json::Null))
    }

    async fn step_completion(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        if let Some(goal) = snapshot.active_goal.clone() {
            self.submit(
                self.command(
                    format!("complete-goal-{goal}"),
                    "complete_goal",
                    json!({"goal_id": goal, "instance_id": self.config.instance_id}),
                ),
                Identity::System,
            )
            .await?;
            return Ok(());
        }
        // no owned goal: the finish settles the oldest open assigned task
        // from the stored candidate (§5.3); an empty queue just closes
        match self.oldest_open_task().await? {
            Some(task_id) => {
                let candidate = self.completion_candidate().await?;
                let status = match candidate["outcome"].as_str().unwrap_or("failed") {
                    "success" => "SUCCEEDED",
                    "blocked" => "BLOCKED",
                    _ => "FAILED",
                };
                let summary = candidate["summary"].as_str().unwrap_or("").to_string();
                let evidence = candidate.get("evidence").cloned().unwrap_or(json!([]));
                self.submit(
                    self.command(
                        format!("complete-task-{task_id}"),
                        "complete_task",
                        json!({"task_id": task_id, "status": status, "summary": summary, "result_refs": evidence}),
                    ),
                    Identity::Instance(self.config.instance_id.clone()),
                )
                .await?;
            }
            None => {
                self.submit(
                    self.command(
                        format!("close-completion-{}", uuid::Uuid::new_v4()),
                        "close_completion",
                        json!({"instance_id": self.config.instance_id}),
                    ),
                    Identity::System,
                )
                .await?;
            }
        }
        Ok(())
    }
}

fn outcome_request(attempt_id: &str) -> String {
    attempt_id.split('/').next().unwrap_or(attempt_id).to_string()
}
