//! R2-P3 multi-instance scheduling (plan §5, §7): one coordinator per state
//! root discovers the session's instances and drives every ACTIVE one with
//! its own phase machine (the P2 driver) over the shared single-writer
//! storage worker. Spawned instances join without a restart; terminated
//! instances retire their drivers. Fairness: every phase step is a bounded
//! unit of async work that yields at each control transaction, so the tokio
//! scheduler interleaves instances and none starves; user input wakes its
//! instance immediately, giving interactive work priority over polling (§7).

use super::driver::{bootstrap, spawn_driver, DriverConfig, Shared};
use super::storage::Storage;
use crate::providers::Provider;
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use teamagents_core::kernel::KernelProfile;
use teamagents_core::v2::{Command, Identity};

pub struct SupervisorConfig<P, F> {
    /// Provider marker: the factory below yields one provider per driver.
    pub marker: std::marker::PhantomData<P>,
    pub session_db: PathBuf,
    pub session_id: String,
    /// The leader instance: created by bootstrap with this profile.
    pub leader_id: String,
    pub leader_profile: KernelProfile,
    /// Coordinator lock lives here; per-instance state under instances/<id>.
    pub state_root: PathBuf,
    pub workspace: PathBuf,
    /// Trusted session permission mode ("approved_scope" | "full_auto"), D-41.
    pub permissions: String,
    pub catalog: teamagents_core::models::UserConfig,
    pub bindings: Vec<String>,
    /// Transient transport retries inside one request (single retry owner, §7).
    pub max_retries: usize,
    pub storage_queue: usize,
    pub poll: Duration,
    /// Goal limits (e.g. max_total_tokens) recorded at bootstrap.
    pub goal_limits: Json,
    pub require_shell_approval: bool,
    /// One provider per instance driver, built from its stored profile.
    pub provider_factory: F,
}

struct InstanceDriver {
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

/// Client handle: user operations go through the same serialized storage
/// worker as the drivers, so commands linearize with driver transitions (§9).
pub struct SupervisorHandle {
    storage: Storage,
    session_id: String,
    shared: Arc<tokio::sync::Notify>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    drivers: Arc<std::sync::Mutex<HashMap<String, InstanceDriver>>>,
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), String>>>>,
    _lock: std::fs::File,
}

fn command(id: impl Into<String>, method: &str, params: Json) -> Command {
    Command { command_id: id.into(), method: method.into(), params }
}

impl SupervisorHandle {
    async fn submit(&self, cmd: Command, identity: Identity) -> Result<Json, String> {
        self.storage.call(move |control| control.submit(cmd, identity)).await?
    }

    /// User-identity command with a client-chosen command id (§9: stable
    /// across reconnects; the control plane dedups replays by id). The
    /// target instance wakes immediately (interactive priority, §7).
    pub async fn submit_user(&self, cmd: Command) -> Result<Json, String> {
        let instance = cmd.params["instance_id"].as_str().map(str::to_string);
        let result = self.submit(cmd, Identity::User).await?;
        if let Some(instance) = instance {
            if let Some(driver) = self.drivers.lock().unwrap().get(&instance) {
                driver.shared.wake.notify_one();
            }
        }
        Ok(result)
    }

    /// User input at the accept boundary (§5.4); replay-safe by command id.
    /// The target instance wakes immediately (interactive priority, §7).
    pub async fn input(&self, instance_id: &str, text: &str) -> Result<Json, String> {
        let envelope = format!("env-{}", uuid::Uuid::new_v4());
        let result = self
            .submit(
                command(
                    format!("input-{envelope}"),
                    "submit_input",
                    json!({"instance_id": instance_id, "envelope_id": envelope, "text": text}),
                ),
                Identity::User,
            )
            .await?;
        if let Some(driver) = self.drivers.lock().unwrap().get(instance_id) {
            driver.shared.wake.notify_one();
        }
        Ok(result)
    }

    pub async fn set_lifecycle(&self, instance_id: &str, target: &str) -> Result<Json, String> {
        self.submit(
            command(
                format!("lifecycle-{}-{}", target.to_lowercase(), uuid::Uuid::new_v4()),
                "set_lifecycle",
                json!({"instance_id": instance_id, "lifecycle": target}),
            ),
            Identity::User,
        )
        .await
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Session snapshot for status views (read from the same worker).
    pub async fn snapshot(&self) -> Result<Json, String> {
        self.storage
            .call(|control| {
                let conn = control.connection();
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
                let goal: Option<(String, String, i64)> = conn
                    .query_row("SELECT status, known_usage_json, unknown_usage FROM goals LIMIT 1", [], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .ok();
                Ok::<_, String>(json!({"instances": instances, "goal": goal.map(|(status, usage, unknown)|
                    json!({"status": status, "known_usage": serde_json::from_str::<Json>(&usage).unwrap_or(Json::Null),
                           "unknown_usage": unknown}))}))
            })
            .await?
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

    /// Stop the supervisor and every driver; submitted commands stay
    /// committed (§4.1, §6.4). Shared form: the daemon holds the handle in
    /// an Arc and stops it without consuming it; later calls join nothing.
    pub async fn shutdown_shared(&self) -> Result<(), String> {
        self.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        let drivers: Vec<InstanceDriver> = self.drivers.lock().unwrap().drain().map(|(_, d)| d).collect();
        for driver in &drivers {
            driver.shared.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            driver.shared.wake.notify_one();
        }
        self.shared.notify_one();
        for driver in drivers {
            let _ = driver.task.await;
        }
        let task = self.task.lock().unwrap().take();
        match task {
            Some(task) => match task.await {
                Ok(result) => result,
                Err(e) => Err(format!("supervisor task join: {e}")),
            },
            None => Ok(()),
        }
    }

    /// Stop the supervisor and every driver; submitted commands stay
    /// committed (§4.1, §6.4).
    pub async fn shutdown(self) -> Result<(), String> {
        self.shutdown_shared().await
    }
}

/// Profile stored on the instance row by spawn (§5.2); the leader's own
/// profile comes from the supervisor config. Missing fields fall back to
/// the leader's (same model/window family, D-36 window still required).
fn stored_profile(json: &Json, fallback: &KernelProfile) -> KernelProfile {
    KernelProfile {
        model: json["model"].as_str().unwrap_or(&fallback.model).to_string(),
        instructions: json["instructions"].as_str().unwrap_or("").to_string(),
        tools: json["tools"].as_array().cloned().unwrap_or_default(),
        options: json.get("options").cloned().unwrap_or(json!({})),
        context_window: json["context_window"].as_u64().or(fallback.context_window),
    }
}

/// Start the supervisor: one coordinator lock, one storage worker, then a
/// discovery loop that spawns and retires per-instance drivers (§5/§7).
pub async fn start<P, F>(mut config: SupervisorConfig<P, F>) -> Result<SupervisorHandle, String>
where
    P: Provider + 'static,
    F: Fn(&str, &KernelProfile) -> P + Send + Sync + 'static,
{
    // every instance driver inherits this root; isolated shell binds need it
    // absolute even when the caller passed a relative path
    std::fs::create_dir_all(&config.state_root).map_err(|e| format!("state root: {e}"))?;
    config.state_root = std::fs::canonicalize(&config.state_root)
        .map_err(|e| format!("state root {}: {e}", config.state_root.display()))?;
    let lock = crate::jobs::state_lock(&config.state_root.join("coordinator.lock"))?;
    let storage = Storage::open(&config.session_db, &config.session_id, true, config.storage_queue)?;
    bootstrap(&storage, &config.leader_id, &config.workspace.to_string_lossy(), &config.goal_limits).await?;
    let session_id = config.session_id.clone();
    let drivers: Arc<std::sync::Mutex<HashMap<String, InstanceDriver>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let wake = Arc::new(tokio::sync::Notify::new());
    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let supervisor: Supervisor<P, F> = Supervisor {
        storage: storage.clone(),
        drivers: drivers.clone(),
        wake: wake.clone(),
        shutdown: shutdown.clone(),
        config,
    };
    let task = tokio::spawn(supervisor.run());
    Ok(SupervisorHandle {
        storage,
        session_id,
        shared: wake,
        shutdown,
        drivers,
        task: std::sync::Mutex::new(Some(task)),
        _lock: lock,
    })
}

struct Supervisor<P, F> {
    storage: Storage,
    drivers: Arc<std::sync::Mutex<HashMap<String, InstanceDriver>>>,
    wake: Arc<tokio::sync::Notify>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    config: SupervisorConfig<P, F>,
}

impl<P, F> Supervisor<P, F>
where
    P: Provider + 'static,
    F: Fn(&str, &KernelProfile) -> P + Send + Sync + 'static,
{
    async fn run(self) -> Result<(), String> {
        while !self.shutdown.load(std::sync::atomic::Ordering::SeqCst) {
            let instances: Vec<(String, String, String, String)> = self
                .storage
                .call(|control| {
                    let mut stmt = control
                        .connection()
                        .prepare("SELECT id, lifecycle, workspace_ref, profile_json FROM instances")
                        .map_err(|e| format!("discover prepare: {e}"))?;
                    let rows = stmt
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                        .map_err(|e| format!("discover query: {e}"))?;
                    rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("discover collect: {e}"))
                })
                .await??;
            {
                let mut drivers = self.drivers.lock().unwrap();
                // retire finished or terminated drivers (§5.4)
                drivers.retain(|id, driver| {
                    let alive = instances.iter().any(|(iid, lifecycle, _, _)| iid == id && lifecycle != "TERMINATED");
                    alive && !driver.task.is_finished()
                });
                for (id, lifecycle, workspace_ref, profile_json) in &instances {
                    if lifecycle != "ACTIVE" || drivers.contains_key(id) {
                        continue;
                    }
                    let profile = if *id == self.config.leader_id {
                        self.config.leader_profile.clone()
                    } else {
                        let stored: Json = serde_json::from_str(profile_json).unwrap_or(json!({}));
                        stored_profile(&stored, &self.config.leader_profile)
                    };
                    let instance_root = self.config.state_root.join("instances").join(id);
                    let workspace = if workspace_ref.is_empty() {
                        self.config.workspace.clone()
                    } else {
                        PathBuf::from(workspace_ref)
                    };
                    let driver_config = DriverConfig {
                        session_db: self.config.session_db.clone(),
                        session_id: self.config.session_id.clone(),
                        instance_id: id.clone(),
                        state_root: instance_root,
                        workspace,
                        permissions: self.config.permissions.clone(),
                        // the kernel gets the wire-effective profile while
                        // the factory still sees the catalog key (R17)
                        profile: crate::providers::resolve_profile(profile.clone(), &self.config.catalog),
                        provider: (self.config.provider_factory)(id, &profile),
                        catalog: self.config.catalog.clone(),
                        bindings: self.config.bindings.clone(),
                        max_retries: self.config.max_retries,
                        storage_queue: self.config.storage_queue,
                        poll: self.config.poll,
                        goal_limits: self.config.goal_limits.clone(),
                        require_shell_approval: self.config.require_shell_approval,
                    };
                    let (shared, task) = spawn_driver(driver_config, &self.storage)?;
                    drivers.insert(id.clone(), InstanceDriver { shared, task });
                }
                if drivers.is_empty() && instances.iter().all(|(_, lifecycle, _, _)| lifecycle == "TERMINATED") {
                    break; // every instance retired: the session is done (§5.4)
                }
            }
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(self.config.poll) => {}
            }
        }
        // stop whatever a racing discovery pass left behind; committed
        // commands stay committed (§4.1, §6.4)
        let remaining: Vec<InstanceDriver> = self.drivers.lock().unwrap().drain().map(|(_, d)| d).collect();
        for driver in &remaining {
            driver.shared.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            driver.shared.wake.notify_one();
        }
        for driver in remaining {
            let _ = driver.task.await;
        }
        Ok(())
    }
}
