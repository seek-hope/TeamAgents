//! R2-P2 persistent single-instance driver (plan §3, §6): the async phase
//! machine over the v2 control plane. Every model/tool wait happens outside
//! transactions; every state transition goes through `Control::submit`.
//! Recovery is state-driven (§6.3): known results are reused, in-flight
//! losses are recorded honestly, and nothing side-effecting is re-executed
//! on a guess.

use crate::hooks::Hooks;
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
use teamagents_core::models::WorkspacePolicy;
use teamagents_core::v2::{Command, Identity};

/// Output cap mirrored from the synchronous shell path (tools.rs MAX_OUTPUT).
const MAX_OUTPUT: usize = 200_000;
const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

/// Default bound on required-check repair rounds before the goal parks
/// BLOCKED (§8 requires bounded repair, not a specific number — this is a
/// policy knob, overridable per goal via limits.max_check_rounds).
const DEFAULT_MAX_CHECK_ROUNDS: i64 = 3;

/// Summary input cap and the output reserve default for the L2 trigger
/// (ported D-28/L2 constants: 100k summary input, 8k reserve).
const SUMMARY_INPUT_CAP: usize = 100_000;
const DEFAULT_OUTPUT_RESERVE: u64 = 8192;

/// R22/A20 compaction prompt: the sections that must survive a summary are the
/// user's original request, its later revisions, the acceptance conditions and
/// the open questions — not just progress prose (plan §7, D-28).
const SUMMARY_PROMPT: &str = "You are compacting an agent conversation to free context space. Summarize it for continuation, in this exact structure:
1. Original request: the user's objective and its constraints, verbatim where it matters
2. User revisions: later corrections or changes to that request
3. Acceptance: what must be true for the work to count as done
4. Open questions: unresolved items, errors and what was tried
5. Progress: what has been done, with key decisions and why
6. Files: paths created/modified/read that matter, one line each
7. Tasks: pending tasks with their ids/assignees if mentioned
8. Next: the immediate next step
Mention the tool_call_ids of tool calls whose full output may be needed later. Be dense; omit small talk.

Conversation to compact:

";

pub struct DriverConfig<P> {
    pub session_db: PathBuf,
    pub session_id: String,
    pub instance_id: String,
    /// Jobs, artifacts and the coordinator lock live here (one lock per root, §6.1).
    pub state_root: PathBuf,
    /// Per-instance directories (`<session root>/instances`, §12.3): a spawn
    /// resolves its workspace policy against this directory.
    pub instances_dir: PathBuf,
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
    /// Simulate a crash: abort the driver task and await its unwind. The coordinator
    /// lock lives in this handle, and the aborted task must stop before a replacement
    /// driver can take it (tests crash and restart inside one process, on a
    /// current-thread runtime where a blocking wait would stall the very task it is
    /// waiting for).
    pub async fn crash(self) {
        self.task.abort();
        let _ = self.task.await; // JoinError::Cancelled once the abort lands
    }

    /// Stop driving; submitted commands stay committed (§4.1).
    /// Run one diagnostics closure on the storage worker — the same single
    /// connection the driver submits through, so tests can inject real
    /// storage-level conditions (e.g. a page cap for A31).
    pub async fn with_control<R, F>(&self, f: F) -> Result<R, String>
    where
        R: Send + 'static,
        F: FnOnce(&mut teamagents_core::v2::Control) -> R + Send + 'static,
    {
        self.storage.call(f).await
    }

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
    /// A31/§4.4 disk-full latch: once a submit fails StorageFull the driver
    /// stops stepping and only retries the park until it lands.
    storage_full: bool,
    /// R22/A20 circuit breaker: consecutive failed summaries for this
    /// instance. Three in a row stop the attempts (the context keeps growing
    /// until the provider itself reports the overflow, exactly as D-28 L2
    /// decided) instead of burning a model call on every step.
    compact_failures: u32,
    /// R22/A20 L2 input: the provider-reported prompt size of the last
    /// completed turn. It is cached from live usage and read back from the
    /// store once per driver (after a crash/restart), so the trigger costs no
    /// storage round trip on the turn path.
    last_prompt: u64,
    last_prompt_loaded: bool,
    /// User hooks (`[hooks] notify` / `pre_tool`), if the config configures any.
    hooks: Option<Arc<Hooks>>,
    /// Last lifecycle seen by the loop: `run_paused` fires on the edge, and the
    /// boot value never counts as a transition.
    last_lifecycle: Option<String>,
}

/// Start the driver over its state root. Bootstrap commands use fixed command
/// ids, so restarting over an existing session replays receipts instead of
/// duplicating instances, goals or input (§6.3 row 1).
pub async fn start<P: Provider + 'static>(mut config: DriverConfig<P>) -> Result<DriverHandle, String> {
    std::fs::create_dir_all(&config.state_root).map_err(|e| format!("state root: {e}"))?;
    // the isolated shell binds absolute source paths: a relative state root
    // would reach bwrap as a relative bind and fail with a confusing error
    config.state_root = std::fs::canonicalize(&config.state_root)
        .map_err(|e| format!("state root {}: {e}", config.state_root.display()))?;
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
/// Receipt references of one stored entry: a plain array for ordinary
/// entries, the provenance object a compaction summary carries (§7, A20).
fn entry_refs(raw: &str) -> Vec<String> {
    let strings = |items: &Vec<Json>| items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    match serde_json::from_str::<Json>(raw).unwrap_or(Json::Null) {
        Json::Array(items) => strings(&items),
        Json::Object(map) => map.get("refs").and_then(Json::as_array).map(strings).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Disk-full signatures from both layers (A31): core classifies SQLite's
/// SQLITE_FULL as "StorageFull: …" at the submit boundary; artifact and
/// journal file writes surface the OS error verbatim.
fn storage_full(error: &str) -> bool {
    error.contains("StorageFull") || error.contains("No space left on device")
}

pub(crate) fn spawn_driver<P: Provider + 'static>(
    mut config: DriverConfig<P>,
    storage: &Storage,
) -> Result<SpawnedDriver, String> {
    std::fs::create_dir_all(config.state_root.join("jobs")).map_err(|e| format!("jobs dir: {e}"))?;
    std::fs::create_dir_all(config.state_root.join("artifacts")).map_err(|e| format!("artifacts dir: {e}"))?;
    // one coordinator per state root: the supervisor hands its own (possibly
    // relative) root down, and isolated shell binds need an absolute path
    config.state_root = std::fs::canonicalize(&config.state_root)
        .map_err(|e| format!("state root {}: {e}", config.state_root.display()))?;
    let shared = Arc::new(Shared {
        wake: tokio::sync::Notify::new(),
        shutdown: AtomicBool::new(false),
        cancel_attempt: Mutex::new(None),
    });
    // Bound MCP services load at boot: a required service that is unavailable
    // fails the boot honestly, an optional one only drops its capability (§7).
    let toolkit = Arc::new(V2Toolkit::new(
        config.workspace.clone(),
        config.catalog.clone(),
        config.bindings.clone(),
        Some(config.state_root.join("artifacts")),
        Some(config.state_root.join("shell")),
    )?);
    let hooks = Hooks::from_config(&config.catalog, &config.session_id);
    let driver = Driver {
        shell_state: config.state_root.join("shell"),
        config,
        storage: storage.clone(),
        shared: shared.clone(),
        toolkit,
        prepared: std::collections::HashMap::new(),
        storage_full: false,
        compact_failures: 0,
        last_prompt: 0,
        last_prompt_loaded: false,
        hooks,
        last_lifecycle: None,
    };
    let task = tokio::spawn(driver.run());
    Ok((shared, task))
}

impl<P: Provider> Driver<P> {
    async fn run(mut self) -> Result<(), String> {
        // Reap bound MCP server processes however the loop ends (§6.4); the
        // clients' own Drop is the backstop.
        let result = self.drive().await;
        self.toolkit.close_mcp();
        result
    }

    async fn drive(&mut self) -> Result<(), String> {
        self.recover().await?;
        loop {
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
            let snapshot = self.snapshot().await?;
            if self.last_lifecycle.as_deref() != Some(snapshot.lifecycle.as_str()) {
                // the boot value is not a transition; a later PAUSED is
                if self.last_lifecycle.is_some() && snapshot.lifecycle == "PAUSED" {
                    if let Some(hooks) = &self.hooks {
                        hooks.fire(
                            "run_paused",
                            json!({"session_id": self.config.session_id, "instance_id": self.config.instance_id}),
                        );
                    }
                }
                self.last_lifecycle = Some(snapshot.lifecycle.clone());
            }
            if snapshot.lifecycle == "TERMINATED" {
                return Ok(()); // the supervisor retires this driver (§5.4)
            }
            if snapshot.lifecycle != "ACTIVE" {
                self.wait().await;
                continue;
            }
            if self.storage_full {
                // Disk-full latch (A31, §4.4): the failed step is never
                // retried by the driver — new side effects stay stopped and
                // only the park is retried at poll pace until it lands. The
                // user frees space and resumes the instance explicitly; the
                // parked reason reports the in-flight persistence loss.
                let parked = self
                    .submit(
                        self.command(
                            format!("park-storage-{}", uuid::Uuid::new_v4()),
                            "set_lifecycle",
                            json!({"instance_id": self.config.instance_id, "lifecycle": "PARKED",
                                   "reason": "storage full: an in-flight result could not be persisted (§4.4); free space, then resume the instance"}),
                        ),
                        Identity::System,
                    )
                    .await
                    .is_ok();
                if parked {
                    self.storage_full = false;
                }
                self.wait().await;
                continue;
            }
            match self.drive_once(&snapshot).await {
                Ok(()) => {}
                Err(error) if storage_full(&error) => {
                    self.storage_full = true;
                    eprintln!("driver: {} hit storage full, parking: {error}", self.config.instance_id);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The user's `pre_tool` hook may veto one native tool call before it runs
    /// (exit 2 denies, stderr is the reason; anything else allows). It is a
    /// blocking host process, so it runs on the blocking pool. Recovery never
    /// re-asks: the decision was made when the call was first dispatched.
    async fn pre_tool_denied(&self, name: &str, args: &Json) -> Option<String> {
        let hooks = self.hooks.clone()?;
        if !hooks.has_pre_tool() {
            return None;
        }
        let payload = json!({"session_id": self.config.session_id, "instance_id": self.config.instance_id,
                             "tool": name, "arguments": args});
        tokio::task::spawn_blocking(move || hooks.deny_reason(&payload)).await.ok().flatten()
    }

    /// One native tool call, reported to the user's `notify` hook (if any). The
    /// arguments travel with it so the hook can tell what actually ran.
    fn notify_tool_call(&self, tool: &str, args: &Json, ok: bool, error: Option<String>) {
        if let Some(hooks) = &self.hooks {
            hooks.fire(
                "tool_call",
                json!({"session_id": self.config.session_id, "instance_id": self.config.instance_id,
                       "tool": tool, "arguments": args, "ok": ok, "error": error}),
            );
        }
    }

    /// One step of the ACTIVE lifecycle: phase work, plus the WAITING drains
    /// that double as the wake path for envelope-borne facts (§5.3, A23).
    async fn drive_once(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        let stepped = match snapshot.phase.as_str() {
            "READY" => self.step_ready(snapshot).await?,
            "MODEL_PENDING" => {
                self.step_model(snapshot).await?;
                true
            }
            "TOOLS_PENDING" => self.step_tools(snapshot).await?,
            "COMPLETION_PENDING" => {
                self.step_completion(snapshot).await?;
                true
            }
            // WAITING: input or a wake flips the phase in one transaction
            _ => false,
        };
        if !stepped {
            if snapshot.phase == "WAITING" {
                // parked drains are the wake path for envelope-borne facts
                // (§5.3, A23): applying one satisfies a matching wait in the
                // same transaction; due timers close at poll granularity.
                // Both commands are replay-safe.
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
        Ok(())
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

    /// Model-visible context (§7): entries a compaction summary does not
    /// cover. At most one summary is visible — a later summary covers the
    /// older one — and it always precedes the retained tail it summarizes.
    async fn context_entries(&self, snapshot: &Snapshot) -> Result<Vec<ContextEntry>, String> {
        self.read_entries(snapshot, true).await
    }

    /// The whole epoch, covered entries included: readback must still reach
    /// originals after a summary hid them (A20).
    async fn stored_entries(&self, snapshot: &Snapshot) -> Result<Vec<ContextEntry>, String> {
        self.read_entries(snapshot, false).await
    }

    async fn read_entries(&self, snapshot: &Snapshot, visible_only: bool) -> Result<Vec<ContextEntry>, String> {
        let instance = self.config.instance_id.clone();
        let epoch = snapshot.epoch;
        self.storage
            .call(move |control| {
                let sql = if visible_only {
                    "SELECT id, kind, message_json, refs_json FROM context_entries
                     WHERE instance_id = ?1 AND epoch = ?2 AND compressed_by IS NULL
                     ORDER BY CASE WHEN kind = 'summary' THEN 0 ELSE 1 END, idx"
                } else {
                    "SELECT id, kind, message_json, refs_json FROM context_entries
                     WHERE instance_id = ?1 AND epoch = ?2 ORDER BY idx"
                };
                let mut stmt = control.connection().prepare(sql).map_err(|e| format!("context prepare: {e}"))?;
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
                        // notes and compaction summaries both ride as user-role
                        // text; neither is a user turn
                        "note" | "summary" => EntryKind::Note,
                        _ => EntryKind::Note,
                    };
                    let mut entry = ContextEntry::new(
                        id,
                        kind,
                        serde_json::from_str(&message).map_err(|e| format!("context message: {e}"))?,
                    );
                    entry.refs = entry_refs(&refs);
                    entries.push(entry);
                }
                Ok(entries)
            })
            .await?
    }

    /// The provider-reported prompt size of the most recent completed turn:
    /// real usage beats any local estimate (D-28 L2 keeps the larger of the
    /// two), and unknown usage honestly reads as 0. Live usage updates the
    /// cache; only a fresh driver reads it back from the store.
    async fn last_prompt_tokens(&mut self) -> Result<u64, String> {
        if self.last_prompt_loaded {
            return Ok(self.last_prompt);
        }
        let instance = self.config.instance_id.clone();
        let usage: Option<String> = self
            .storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT a.usage_json FROM attempts a
                         JOIN model_requests r ON r.request_id = a.request_id
                         WHERE r.instance_id = ?1 AND r.kind = 'turn' AND a.status = 'COMPLETE'
                           AND a.usage_json IS NOT NULL AND a.usage_json != 'null'
                         ORDER BY a.rowid DESC LIMIT 1",
                        [&instance],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| format!("last usage: {e}"))
            })
            .await??;
        self.last_prompt = usage
            .and_then(|raw| serde_json::from_str::<Json>(&raw).ok())
            .and_then(|usage| usage["prompt_tokens"].as_u64())
            .unwrap_or(0);
        self.last_prompt_loaded = true;
        Ok(self.last_prompt)
    }

    /// L2 trigger (§7, A20): the real last prompt (or this request's
    /// estimate, whichever is larger) is over 90% of the native window minus
    /// the output reserve. An unknown window never triggers, and the circuit
    /// breaker stops after three consecutive failures.
    fn over_threshold(&self, request: &ModelRequest, last_prompt_tokens: u64) -> bool {
        let Some(window) = self.config.profile.context_window.filter(|window| *window > 0) else { return false };
        if self.compact_failures >= 3 {
            return false;
        }
        let reserve = self
            .config
            .profile
            .options
            .get("max_completion_tokens")
            .or_else(|| self.config.profile.options.get("max_tokens"))
            .and_then(Json::as_u64)
            .unwrap_or(DEFAULT_OUTPUT_RESERVE)
            .min(window / 4);
        let threshold = window.saturating_sub(reserve).min((window as f64 * COMPACT_AT) as u64);
        last_prompt_tokens.max(request.est_prompt_tokens) > threshold
    }

    /// L2 compaction (§7, A20, the D-28 contract): one model call condenses
    /// the covered prefix into a summary entry; the originals stay in the
    /// epoch and stay reachable. Returns whether a summary committed — a
    /// failed summary is a lost optimization, never a failed turn.
    async fn compact(&mut self, entries: &[ContextEntry]) -> Result<bool, String> {
        let Some(window) = self.config.profile.context_window.filter(|window| *window > 0) else { return Ok(false) };
        // The retained tail is the newest user input and everything after it:
        // a summary never hides the working set the current turn is using —
        // the tool results it just produced — so compaction covers exactly
        // what the turn before left behind (§7, A20).
        // ponytail: the tail itself is never trimmed. If one turn's own input
        // plus its tool traffic exceeds the native window, only the provider's
        // overflow verdict remains as an honest signal — §7 forbids pretending
        // the window is smaller than it is.
        let keep_from = entries.iter().rposition(|entry| entry.kind == EntryKind::User).unwrap_or(entries.len());
        let covered: Vec<&ContextEntry> = entries[..keep_from].iter().collect();
        // nothing to summarize while no assistant response is covered yet
        if !covered.iter().any(|entry| entry.kind == EntryKind::Assistant) {
            return Ok(false);
        }
        let keep_ids: Vec<&str> = entries[keep_from..].iter().map(|entry| entry.id.as_str()).collect();

        // the summary input: covered entries only, one truncated block each,
        // middle-elided when the whole blob exceeds the input cap (§4.3)
        let mut blob = String::new();
        for entry in &covered {
            let message = &entry.message;
            let role = message["role"].as_str().unwrap_or(entry.kind.as_str());
            let content = message["content"].as_str().unwrap_or("");
            let mut content: String = if content.chars().count() > 2_000 {
                format!("{}…[{} chars]", content.chars().take(2_000).collect::<String>(), content.chars().count())
            } else {
                content.to_string()
            };
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    if let Some(id) = call["id"].as_str() {
                        let name = call["function"]["name"].as_str().unwrap_or("?");
                        let args: String =
                            call["function"]["arguments"].as_str().unwrap_or("").chars().take(200).collect();
                        content.push_str(&format!("\n[tool_call_id={id}: {name} {args}]"));
                    }
                }
            }
            if let Some(id) = message["tool_call_id"].as_str() {
                content = format!("[tool_call_id={id}] {content}");
            }
            blob.push_str(&format!("{role}: {content}\n\n"));
        }
        let summary_cap = window.saturating_mul(2).min(SUMMARY_INPUT_CAP as u64) as usize;
        if blob.chars().count() > summary_cap {
            let half = summary_cap / 2;
            let chars: Vec<char> = blob.chars().collect();
            blob = format!(
                "{}\n[...middle omitted...]\n{}",
                chars[..half].iter().collect::<String>(),
                chars[chars.len().saturating_sub(half)..].iter().collect::<String>()
            );
        }
        // the receipt index keeps covered calls discoverable even when the
        // model omits ids from its prose or the middle was elided
        let mut index = String::from("Tool output index (read_history tool_call_id):\n");
        for entry in &covered {
            if let Some(id) = entry.message["tool_call_id"].as_str() {
                let refs = if entry.refs.is_empty() { String::new() } else { format!(" [{}]", entry.refs.join(" ")) };
                index.push_str(&format!("{id}{refs}\n"));
            }
        }
        let messages = vec![json!({"role": "user", "content": format!("{SUMMARY_PROMPT}{blob}\n{index}")})];
        let request_id = format!("comp-{}", uuid::Uuid::new_v4());
        let request = ModelRequest {
            request_id: request_id.clone(),
            model: self.config.profile.model.clone(),
            est_prompt_tokens: estimated_tokens(&json!({"messages": messages, "tools": []})),
            messages,
            tools: vec![],
            options: self.config.profile.options.clone(),
        };
        let begun = self
            .submit(
                self.command(
                    format!("begin-compression-{request_id}"),
                    "begin_compression",
                    json!({"instance_id": self.config.instance_id, "request_id": request_id,
                           "request_ref": format!("compression:{}", self.config.instance_id),
                           "est_prompt_tokens": request.est_prompt_tokens}),
                ),
                Identity::System,
            )
            .await?;
        if begun["budget_refused"] == json!(true) || begun["deadline_refused"] == json!(true) {
            // the goal budget and deadline stay authoritative (§8, A35): the
            // turn proceeds uncompressed and its own request is gated the same way
            return Ok(false);
        }
        // one attempt per summary, no retry: transport retry stays owned by
        // the request that asked for the turn (§7)
        let attempt_id = format!("{request_id}/a1");
        let cancel = Cancel::new();
        *self.shared.cancel_attempt.lock().unwrap() = Some(cancel.clone());
        let outcome = self.config.provider.complete(&request, &cancel, &mut |_| {}).await;
        self.shared.cancel_attempt.lock().unwrap().take();
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = self
                    .submit(
                        self.command(
                            format!("attempt-{attempt_id}"),
                            "record_attempt",
                            json!({"attempt_id": attempt_id, "request_id": request_id, "status": "FAILED",
                                   "error_class": format!("{:?}", error.class)}),
                        ),
                        Identity::System,
                    )
                    .await;
                return self.abandon_compression(&request_id, &format!("{:?}: {}", error.class, error.message)).await;
            }
        };
        let summary = outcome.response.message["content"].as_str().unwrap_or("").trim().to_string();
        if summary.is_empty() {
            let _ = self
                .submit(
                    self.command(
                        format!("attempt-{attempt_id}"),
                        "record_attempt",
                        json!({"attempt_id": attempt_id, "request_id": request_id, "status": "FAILED",
                               "error_class": "empty_summary", "elapsed_ms": outcome.elapsed_ms}),
                    ),
                    Identity::System,
                )
                .await;
            return self.abandon_compression(&request_id, "the summary was empty").await;
        }
        let response_ref = self.store_response_artifact(&attempt_id, &outcome).await?;
        let recorded = self
            .submit(
                self.command(
                    format!("attempt-{attempt_id}"),
                    "record_attempt",
                    json!({"attempt_id": attempt_id, "request_id": request_id, "status": "COMPLETE",
                           "elapsed_ms": outcome.elapsed_ms,
                           "usage": outcome.response.usage.map(|u| json!({
                               "prompt_tokens": u.prompt, "completion_tokens": u.completion, "total_tokens": u.total})),
                           "response_ref": response_ref, "publish": [response_ref]}),
                ),
                Identity::System,
            )
            .await;
        if let Err(error) = recorded {
            // cancelled mid-flight or a storage failure: the archive stays
            // honest, the summary never commits, the originals stay in view
            let _ = self
                .submit(
                    self.command(format!("abandon-{response_ref}"), "artifact_abandon", json!({"id": response_ref})),
                    Identity::System,
                )
                .await;
            return self.abandon_compression(&request_id, &error).await;
        }
        let committed = self
            .submit(
                self.command(
                    format!("compress-{request_id}"),
                    "compress_context",
                    json!({"instance_id": self.config.instance_id, "request_id": request_id,
                           "attempt_id": attempt_id, "summary": summary, "keep_ids": keep_ids}),
                ),
                Identity::System,
            )
            .await;
        match committed {
            Ok(_) => {
                self.compact_failures = 0;
                Ok(true)
            }
            Err(error) => self.abandon_compression(&request_id, &error).await,
        }
    }

    /// Close a summary that cannot commit: the reservation is released, the
    /// instance is untouched and the breaker counts the failure. The originals
    /// are still in the view, so the turn continues uncompressed (§7, A20).
    async fn abandon_compression(&mut self, request_id: &str, reason: &str) -> Result<bool, String> {
        self.compact_failures = self.compact_failures.saturating_add(1);
        eprintln!("driver: {} could not compact ({}/3): {reason}", self.config.instance_id, self.compact_failures);
        self.submit(
            self.command(
                format!("fail-compression-{request_id}"),
                "fail_compression",
                json!({"request_id": request_id, "reason": reason}),
            ),
            Identity::System,
        )
        .await?;
        Ok(false)
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
        // Bound MCP tools advertise per request (§5.2). They merge here rather
        // than into the stored profile so a spawned child never inherits the
        // parent's bound services; the child's own driver merges its own.
        profile.tools.extend(self.toolkit.mcp_schemas());
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
        // a compression request left PENDING by a crash never committed its
        // summary (the commit is one transaction): close it, release its
        // reservation and continue uncompressed — redoing it is safe (§7, A20)
        let stale: Vec<String> = self
            .storage
            .call({
                let instance = self.config.instance_id.clone();
                move |control| {
                    let mut stmt = control
                        .connection()
                        .prepare(
                            "SELECT request_id FROM model_requests
                             WHERE instance_id = ?1 AND kind = 'compression' AND status = 'PENDING'",
                        )
                        .map_err(|e| format!("compression recovery prepare: {e}"))?;
                    let rows = stmt
                        .query_map([&instance], |row| row.get(0))
                        .map_err(|e| format!("compression recovery: {e}"))?;
                    rows.collect::<Result<Vec<String>, _>>().map_err(|e| format!("compression recovery collect: {e}"))
                }
            })
            .await??;
        for request_id in stale {
            self.abandon_compression(&request_id, "an in-flight summary was lost to a restart").await?;
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
        let mut entries = self.context_entries(snapshot).await?;
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
        let mut request = kernel.prepare_request(&entries, &request_id);
        // L2 (§7, A20): a summary is taken before the request is fixed and
        // registered, so the compacted view is what begin_request reserves
        // budget for and what the provider receives
        let last_prompt = self.last_prompt_tokens().await?;
        if self.over_threshold(&request, last_prompt) && self.compact(&entries).await? {
            entries = self.context_entries(snapshot).await?;
            request = kernel.prepare_request(&entries, &request_id);
        }
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
            Ok(result) if result["budget_refused"] == json!(true) || result["deadline_refused"] == json!(true) => {
                // user budgets and goal deadlines stay authoritative (§8,
                // A35): park with the reason
                let reason = result["reason"].as_str().unwrap_or("request refused").to_string();
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
                // the real prompt size of this turn feeds the L2 trigger
                if let Some(usage) = outcome.response.usage {
                    self.last_prompt = usage.prompt;
                    self.last_prompt_loaded = true;
                }
                let entry_id = format!("{}:assistant", request_id);
                let interpretation = self.kernel(snapshot).interpret_response(&outcome.response, &entry_id);
                let imported = self.import_interpretation(&request_id, interpretation, snapshot).await;
                if imported.is_ok() {
                    if let Some(hooks) = &self.hooks {
                        hooks.fire(
                            "run_completed",
                            json!({"session_id": self.config.session_id, "instance_id": self.config.instance_id,
                                   "request_id": request_id}),
                        );
                    }
                }
                imported
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
                        if let Some(hooks) = &self.hooks {
                            hooks.fire(
                                "run_cancelled",
                                json!({"session_id": self.config.session_id, "instance_id": self.config.instance_id,
                                       "request_id": request_id}),
                            );
                        }
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
                        let failed = self
                            .submit(
                                self.command(
                                    format!("fail-{request_id}"),
                                    "fail_request",
                                    json!({"request_id": request_id, "reason": reason, "park": true}),
                                ),
                                Identity::System,
                            )
                            .await;
                        if failed.is_ok() {
                            if let Some(hooks) = &self.hooks {
                                hooks.fire(
                                    "run_failed",
                                    json!({"session_id": self.config.session_id,
                                           "instance_id": self.config.instance_id,
                                           "request_id": request_id, "reason": reason.clone()}),
                                );
                            }
                        }
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
            // DISPATCH_COMMITTED/RUNNING here (fresh or recovered): execute or reconnect.
            // A status other than PREPARED at query time means the dispatch was
            // committed by a previous boot — this execution is a recovery.
            let recovered = status != "PREPARED";
            let name = intent["name"].as_str().unwrap_or("");
            match name {
                "shell" => self.execute_shell(&operation_id, &intent, !recovered).await?,
                "read_history" => self.execute_readback(&operation_id, &intent, snapshot).await?,
                teamagents_core::kernel::SEND_TOOL
                | teamagents_core::kernel::DELEGATE_TOOL
                | teamagents_core::kernel::SPAWN_TOOL => self.execute_collaboration(&operation_id, &intent).await?,
                _ if recovered && self.toolkit.is_mcp_tool(name) => {
                    // A25/§6.3: the call crossed the process boundary before the
                    // interruption, so the remote effect cannot be verified and
                    // must not be re-issued — server idempotence annotations
                    // never authorize a replay of a remote effect.
                    let reason = format!(
                        "MCP tool {name} was dispatched before an interruption; the remote outcome cannot be verified, so it was not retried"
                    );
                    let mut receipt =
                        self.receipt_skeleton(&operation_id, &intent, true, json!({"error": reason}).to_string());
                    receipt.error = Some(ReceiptError { class: "outcome_unknown".into(), reason });
                    self.complete_op(
                        &operation_id,
                        "OUTCOME_UNKNOWN",
                        &serde_json::to_value(receipt).unwrap_or(Json::Null),
                        vec![],
                    )
                    .await?;
                }
                _ => self.execute_inline(&operation_id, &intent, !recovered).await?,
            }
        }
        Ok(true)
    }

    /// `pre_tool`: run the user's policy hook first. False for a recovered call
    /// (the decision was made at dispatch) and for the required checks, which are
    /// the user's own acceptance commands rather than model tool calls.
    async fn execute_shell(&mut self, operation_id: &str, intent: &Json, pre_tool: bool) -> Result<(), String> {
        if pre_tool {
            if let Some(reason) = self.pre_tool_denied("shell", &intent["args"]).await {
                self.notify_tool_call("shell", &intent["args"], false, Some(reason.clone()));
                let message = format!("denied by pre_tool hook: {reason}");
                return self.complete_with_error(operation_id, intent, "hook_denied", &message).await;
            }
        }
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
        let call_ok = receipt.ok;
        let call_error = receipt.error.as_ref().map(|error| error.reason.clone());
        let args = intent["args"].clone();
        let result =
            self.complete_op(operation_id, status, &serde_json::to_value(receipt).unwrap_or(Json::Null), publish).await;
        self.notify_tool_call("shell", &args, call_ok, call_error);
        result
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
        // the whole epoch, covered entries included: a summary never makes an
        // original unreachable (A20)
        let entries = self.stored_entries(snapshot).await?;
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
    /// §12.3: resolve the workspace policy of one `spawn` call. shared (the
    /// project directory) is the default; a worktree request on a dirty or
    /// unversioned project falls back to shared and says why. The policy is
    /// recorded before the instance can boot, so retirement cleans up exactly
    /// what was created. Errors are tool-call failures, not driver failures.
    fn prepare_spawn_workspace(&self, args: &Json) -> Result<(String, Json), String> {
        let instance_id = args["instance_id"].as_str().unwrap_or("");
        let policy = match args.get("workspace").and_then(|value| value.as_str()) {
            None => WorkspacePolicy::Shared,
            Some(name) => serde_json::from_value::<WorkspacePolicy>(json!(name))
                .map_err(|_| format!("unknown workspace {name:?}: expected shared, isolated or git_worktree"))?,
        };
        let member_dir = self.config.instances_dir.join(instance_id);
        let prepared = crate::workspace::prepare(instance_id, policy, &self.config.workspace, &member_dir)?;
        crate::workspace::save(&prepared, &member_dir)?;
        let info = json!({"path": prepared.path.to_string_lossy(),
                          "policy": serde_json::to_value(prepared.policy).unwrap_or(Json::Null),
                          "note": prepared.note});
        Ok((prepared.path.to_string_lossy().into_owned(), info))
    }

    async fn execute_collaboration(&mut self, operation_id: &str, intent: &Json) -> Result<(), String> {
        let name = intent["name"].as_str().unwrap_or("");
        let args = &intent["args"];
        let mut prepared_path: Option<String> = None;
        let mut workspace_info: Option<Json> = None;
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
                let instance_id = args["instance_id"].as_str().unwrap_or("").to_string();
                // a bad workspace request is this tool call's failure, not the
                // driver's: the model gets the error and carries on
                let (workspace_path, info) = match self.prepare_spawn_workspace(args) {
                    Ok(pair) => pair,
                    Err(error) => return self.complete_with_error(operation_id, intent, "collaboration", &error).await,
                };
                prepared_path = Some(workspace_path.clone());
                workspace_info = Some(info);
                let mut profile = self.config.profile.clone();
                profile.instructions = args["instructions"].as_str().unwrap_or("").to_string();
                let mut params = json!({"instance_id": instance_id, "instructions": args["instructions"],
                                        "profile": {"model": profile.model, "instructions": profile.instructions,
                                                    "tools": profile.tools, "options": profile.options,
                                                    "context_window": profile.context_window},
                                        "workspace_ref": workspace_path,
                                        "goal_id": format!("goal-{}", self.config.session_id)});
                if let Some(task) = args.get("task") {
                    params["task"] = task.clone();
                }
                ("spawn_instance", params)
            }
            other => return Err(format!("execute_collaboration: unknown tool {other}")),
        };
        let action = name.to_string();
        let result = self
            .submit(
                self.command(format!("collab-{operation_id}"), method, params),
                Identity::Instance(self.config.instance_id.clone()),
            )
            .await;
        let (action_ok, action_error) = match &result {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error.clone())),
        };
        if let Some(hooks) = &self.hooks {
            hooks.fire(
                "team_action",
                json!({"session_id": self.config.session_id, "instance_id": self.config.instance_id,
                       "action": action, "ok": action_ok, "error": action_error}),
            );
        }
        match result {
            Ok(value) => {
                let mut content = value.clone();
                if let Some(info) = &workspace_info {
                    content["workspace"] = info.clone();
                }
                let receipt = self.receipt_skeleton(operation_id, intent, true, content.to_string());
                let receipt = teamagents_core::kernel::ToolReceipt { ok: true, ..receipt };
                self.complete_op(
                    operation_id,
                    "SUCCEEDED",
                    &serde_json::to_value(receipt).unwrap_or(Json::Null),
                    vec![],
                )
                .await
            }
            Err(error) => {
                if let Some(path) = &prepared_path {
                    // never delete on a guess: a rejected spawn keeps whatever it
                    // prepared, and a retry with the same instance id reuses it
                    eprintln!("spawn kept its prepared workspace at {path}: {error}");
                }
                self.complete_with_error(operation_id, intent, "collaboration", &error).await
            }
        }
    }

    /// Read-only inline tools (web search/fetch): no runner needed (§6.2);
    /// recovery re-executes them because they are idempotent reads.
    /// `pre_tool`: see [`Driver::execute_shell`] — recovery never re-asks.
    async fn execute_inline(&mut self, operation_id: &str, intent: &Json, pre_tool: bool) -> Result<(), String> {
        let typed = ToolIntent {
            index: intent["index"].as_u64().unwrap_or(0) as usize,
            call_id: intent["call_id"].as_str().unwrap_or("").into(),
            name: intent["name"].as_str().unwrap_or("").into(),
            args: intent["args"].clone(),
            args_hash: intent["args_hash"].as_str().unwrap_or("").into(),
        };
        let name = typed.name.clone();
        let args = typed.args.clone();
        if pre_tool {
            if let Some(reason) = self.pre_tool_denied(&name, &args).await {
                self.notify_tool_call(&name, &args, false, Some(reason.clone()));
                let message = format!("denied by pre_tool hook: {reason}");
                return self.complete_with_error(operation_id, intent, "hook_denied", &message).await;
            }
        }
        let toolkit = self.toolkit.clone();
        let op = operation_id.to_string();
        let mode = ShellMode::from_permissions(Some(&self.config.permissions))?;
        let receipt =
            tokio::task::spawn_blocking(move || toolkit.call(&op, &typed, &crate::tools::TurnControl::default(), mode))
                .await
                .map_err(|e| format!("tool worker join: {e}"))?;
        self.notify_tool_call(&name, &args, receipt.ok, receipt.error.as_ref().map(|e| e.reason.clone()));
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
    /// from the model (mirrors complete_goal's read). The decision rowid
    /// orders check rounds against the candidate that triggered them (§8).
    async fn completion_candidate(&self) -> Result<(i64, Json), String> {
        let instance = self.config.instance_id.clone();
        let stored: Option<(i64, String)> = self
            .storage
            .call(move |control| {
                control
                    .connection()
                    .query_row(
                        "SELECT d.rowid, d.completion_json FROM decisions d
                         JOIN model_requests r ON d.request_id = r.request_id
                         WHERE r.instance_id = ?1 AND d.completion_json IS NOT NULL
                         ORDER BY d.rowid DESC LIMIT 1",
                        [&instance],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| format!("completion candidate: {e}"))
            })
            .await??;
        Ok(match stored {
            Some((row, json)) => (row, serde_json::from_str(&json).unwrap_or(Json::Null)),
            None => (0, Json::Null),
        })
    }

    async fn step_completion(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        if let Some(goal) = snapshot.active_goal.clone() {
            if !self.goal_is_active(&goal).await? {
                // the goal already reached a terminal status: this turn just
                // ends, it never re-runs the checks or re-closes the goal
                self.submit(
                    self.command(
                        format!("close-completion-{}", uuid::Uuid::new_v4()),
                        "close_completion",
                        json!({"instance_id": self.config.instance_id}),
                    ),
                    Identity::System,
                )
                .await?;
                return Ok(());
            }
            let (candidate_row, candidate) = self.completion_candidate().await?;
            let outcome = candidate["outcome"].as_str().unwrap_or("failed");
            // only a claimed success is verified; a candidate that admits
            // undelivered work is never upgraded by passing checks (§8)
            if outcome == "success" {
                let (checks, max_rounds) = self.required_checks(&goal).await?;
                if !checks.is_empty() {
                    return self.step_completion_checks(&goal, candidate_row, &checks, max_rounds).await;
                }
            }
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
                let (_, candidate) = self.completion_candidate().await?;
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

    /// Is this goal still the instance's live goal? A terminal goal must not
    /// run another completion round (§8): a finish that arrives after the goal
    /// closed ends its turn instead of re-verifying anything.
    async fn goal_is_active(&self, goal: &str) -> Result<bool, String> {
        let goal = goal.to_string();
        let status: Option<String> = self
            .storage
            .call(move |control| {
                control
                    .connection()
                    .query_row("SELECT status FROM goals WHERE id = ?1", [&goal], |row| row.get(0))
                    .optional()
                    .map_err(|e| format!("goal status: {e}"))
            })
            .await??;
        Ok(status.as_deref() == Some("ACTIVE"))
    }

    /// Goal-level required checks from the stored limits (§8): predefined by
    /// the user or project bootstrap at create_goal; an empty list settles
    /// completion on the candidate alone. Returns (checks, max_rounds).
    async fn required_checks(&self, goal: &str) -> Result<(Vec<Json>, i64), String> {
        let goal = goal.to_string();
        let stored: Option<String> = self
            .storage
            .call(move |control| {
                control
                    .connection()
                    .query_row("SELECT limits_json FROM goals WHERE id = ?1", [&goal], |row| row.get(0))
                    .optional()
                    .map_err(|e| format!("goal limits: {e}"))
            })
            .await??;
        let limits: Json = stored.as_deref().and_then(|l| serde_json::from_str(l).ok()).unwrap_or(json!({}));
        let checks = limits["required_checks"].as_array().cloned().unwrap_or_default();
        let max_rounds = limits["max_check_rounds"].as_i64().unwrap_or(DEFAULT_MAX_CHECK_ROUNDS).max(1);
        Ok((checks, max_rounds))
    }

    /// The latest check round registered against the goal that is newer than
    /// the triggering candidate, plus how many rounds the goal has used.
    /// Rounds from before the candidate belong to an older finish.
    async fn check_round_state(&self, goal: &str, candidate_row: i64) -> Result<(i64, Option<CheckRound>), String> {
        let goal = goal.to_string();
        self.storage
            .call(move |control| {
                let conn = control.connection();
                let rounds: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM model_requests WHERE request_ref = 'required_check' AND goal_id = ?1",
                        [&goal],
                        |row| row.get(0),
                    )
                    .map_err(|e| format!("check rounds: {e}"))?;
                let latest: Option<(String, i64)> = conn
                    .query_row(
                        "SELECT d.decision_id, d.rowid FROM decisions d
                         JOIN model_requests r ON d.request_id = r.request_id
                         WHERE r.request_ref = 'required_check' AND r.goal_id = ?1
                         ORDER BY d.rowid DESC LIMIT 1",
                        [&goal],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| format!("check decision: {e}"))?;
                let Some((decision_id, row)) = latest else { return Ok((rounds, None)) };
                if row <= candidate_row {
                    return Ok((rounds, None));
                }
                let mut stmt = conn
                    .prepare(
                        "SELECT operation_id, status, intent_json, receipt_json FROM operations
                         WHERE decision_id = ?1 ORDER BY tool_index",
                    )
                    .map_err(|e| format!("check ops prepare: {e}"))?;
                let rows = stmt
                    .query_map([&decision_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    })
                    .map_err(|e| format!("check ops query: {e}"))?;
                let mut open = Vec::new();
                let mut terminal = Vec::new();
                for row in rows {
                    let (operation_id, status, intent, receipt) = row.map_err(|e| format!("check op row: {e}"))?;
                    if matches!(status.as_str(), "PREPARED" | "DISPATCH_COMMITTED" | "RUNNING") {
                        open.push((operation_id, status, intent));
                    } else {
                        let intent: Json =
                            serde_json::from_str(&intent).map_err(|e| format!("check intent {operation_id}: {e}"))?;
                        let receipt = receipt.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Json::Null);
                        terminal.push((operation_id, status, intent, receipt));
                    }
                }
                Ok((rounds, Some(CheckRound { open, terminal })))
            })
            .await?
    }

    /// The completion-check lifecycle (§8): register a round, execute it
    /// through the shell ledger, then settle SUCCEEDED, feed the failure
    /// receipts back as a repair turn, or park BLOCKED. Every state is
    /// persisted, so a crash mid-round resumes at exactly this machine.
    async fn step_completion_checks(
        &mut self,
        goal: &str,
        candidate_row: i64,
        checks: &[Json],
        max_rounds: i64,
    ) -> Result<(), String> {
        let (rounds, current) = self.check_round_state(goal, candidate_row).await?;
        if let Some(round) = current {
            if !round.open.is_empty() {
                return self.execute_check_ops(&round.open).await;
            }
            let failures = self.check_verdict(&round.terminal);
            if failures.is_empty() {
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
            // infrastructure failures are not model-repairable (§8 bounded
            // handling): the model cannot fix a refused dispatch or a runner
            // that never started — park instead of burning repair rounds
            let infra =
                failures.iter().any(|f| matches!(f["class"].as_str(), Some("dispatch_refused") | Some("spawn")));
            if infra || rounds >= max_rounds {
                let summary: Vec<String> = failures
                    .iter()
                    .map(|f| {
                        format!("{}:{}", f["check_id"].as_str().unwrap_or("?"), f["class"].as_str().unwrap_or("?"))
                    })
                    .collect();
                self.submit(
                    self.command(
                        format!("block-goal-{goal}"),
                        "block_goal",
                        json!({"goal_id": goal, "instance_id": self.config.instance_id,
                               "reason": format!("required checks failed ({}) after {rounds} round(s)", summary.join(", "))}),
                    ),
                    Identity::System,
                )
                .await?;
                return Ok(());
            }
            self.submit(
                self.command(
                    format!("repair-completion-{goal}-{rounds}"),
                    "repair_completion",
                    json!({"instance_id": self.config.instance_id, "goal_id": goal, "round": rounds,
                           "failures": failures}),
                ),
                Identity::System,
            )
            .await?;
            return Ok(());
        }
        if rounds >= max_rounds {
            self.submit(
                self.command(
                    format!("block-goal-{goal}"),
                    "block_goal",
                    json!({"goal_id": goal, "instance_id": self.config.instance_id,
                           "reason": format!("required checks did not pass within {max_rounds} rounds")}),
                ),
                Identity::System,
            )
            .await?;
            return Ok(());
        }
        let observed = self.observe_check_inputs(checks);
        self.submit(
            self.command(
                format!("check-round-{goal}-{}", rounds + 1),
                "register_check_runs",
                json!({"goal_id": goal, "instance_id": self.config.instance_id,
                       "round": rounds + 1, "checks": observed}),
            ),
            Identity::System,
        )
        .await?;
        Ok(())
    }

    /// Execute the open operations of a check round in order (§6.2 ledger):
    /// dispatch without an approval prompt (the user pre-authorized exactly
    /// these commands), then run or reconnect the shell job. The instance
    /// stays COMPLETION_PENDING throughout; consumption feeds the receipts
    /// into context without flipping the phase.
    async fn execute_check_ops(&mut self, open: &[(String, String, String)]) -> Result<(), String> {
        for (operation_id, status, intent_json) in open {
            if self.shared.shutdown.load(Ordering::SeqCst) {
                return Ok(());
            }
            let intent: Json = serde_json::from_str(intent_json).map_err(|e| format!("intent {operation_id}: {e}"))?;
            if status == "PREPARED" {
                let dispatched = self
                    .submit(
                        self.command(
                            format!("dispatch-{operation_id}-{}", uuid::Uuid::new_v4()),
                            "dispatch_operation",
                            json!({"operation_id": operation_id, "approval_required": false, "permission_revision": 0}),
                        ),
                        Identity::System,
                    )
                    .await;
                match dispatched {
                    Ok(_) => {}
                    Err(error) if error.contains("not dispatchable") || error.contains("lost the dispatch race") => {
                        continue;
                    }
                    Err(error) => {
                        self.complete_with_error(operation_id, &intent, "dispatch_refused", &error).await?;
                        continue;
                    }
                }
            }
            self.execute_shell(operation_id, &intent, false).await?;
        }
        Ok(())
    }

    /// Verdict over a closed round (§8): a check passes only when its job
    /// succeeded and its declared inputs still hash to the values observed
    /// at registration; anything else is a failure handed to repair.
    fn check_verdict(&self, terminal: &[(String, String, Json, Json)]) -> Vec<Json> {
        let mut failures = Vec::new();
        for (_operation_id, status, intent, receipt) in terminal {
            let check_id = intent["args"]["check_id"].as_str().unwrap_or("?").to_string();
            if status != "SUCCEEDED" {
                let class = receipt["error"]["class"].as_str().unwrap_or("failed");
                let reason = receipt["error"]["reason"].as_str().unwrap_or(status).to_string();
                failures.push(json!({"check_id": check_id, "class": class, "reason": reason}));
                continue;
            }
            if let Some(observed) = intent["args"].get("inputs_observed").and_then(Json::as_object) {
                let current = hash_workspace_inputs(&self.config.workspace, observed.keys().cloned().collect());
                for (path, before) in observed {
                    if current.get(path).unwrap_or(&Json::Null) != before {
                        failures.push(json!({"check_id": check_id, "class": "stale_inputs",
                                             "reason": format!("declared input {path} changed since the check ran")}));
                    }
                }
            }
        }
        failures
    }

    /// Hash each check's declared inputs at observation time (§8): the
    /// recorded hashes bind the check results to input versions and are
    /// re-verified before completion. Unsafe paths never arrive here — the
    /// create_goal gate rejects absolute paths and `..` escapes.
    fn observe_check_inputs(&self, checks: &[Json]) -> Vec<Json> {
        checks
            .iter()
            .map(|check| {
                let mut check = check.clone();
                if let Some(inputs) = check["inputs"].as_array() {
                    let paths: Vec<String> = inputs.iter().filter_map(|p| p.as_str().map(str::to_string)).collect();
                    check["inputs_observed"] = json!(hash_workspace_inputs(&self.config.workspace, paths));
                }
                check
            })
            .collect()
    }
}

/// One persisted required-check round (§8): the operations of the synthetic
/// check decision, split by execution state.
struct CheckRound {
    open: Vec<(String, String, String)>,
    terminal: Vec<(String, String, Json, Json)>,
}

/// sha256 of each workspace-relative input (§8): missing or unreadable
/// entries hash as null — the binding covers what was actually observed.
/// ponytail: directories currently hash as null; recursive directory
/// binding is the upgrade path if a check ever declares one.
fn hash_workspace_inputs(workspace: &Path, paths: Vec<String>) -> serde_json::Map<String, Json> {
    use sha2::{Digest, Sha256};
    let mut observed = serde_json::Map::new();
    for path in paths {
        let hash = std::fs::read(workspace.join(&path)).ok().map(|bytes| format!("{:x}", Sha256::digest(&bytes)));
        observed.insert(path, hash.map(Json::String).unwrap_or(Json::Null));
    }
    observed
}

fn outcome_request(attempt_id: &str) -> String {
    attempt_id.split('/').next().unwrap_or(attempt_id).to_string()
}
