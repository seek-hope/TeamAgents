//! Session bootstrap (session.py::open_session): paths + lock + core session,
//! member runners, runtime loop.

use crate::chat::ChatRunner;
use crate::codex::{CodexOptions, CodexRunner};
use crate::config::{load_user_config, user_config_path};
use crate::core_client::CoreClient;
use crate::gateway::{ApprovalGate, PermissionPolicy};
use crate::runtime::{AgentRunner, Notify, RunnerFactory, Runtime, RuntimeLimits, ToolExecutor};
use crate::scripted::{BarrierRegistry, ScriptedMember, Step};
use crate::sessions::{acquire_session_lock, new_session_id, session_paths};
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use teamagents_core::models::{AgentSpec, ModelProfile, RuntimeKind, TeamAction, UserConfig};

pub const LEADER_INSTRUCTIONS: &str = "You are the Leader of a team of agents. Understand the user's goal, decide
whether to work alone or build a team, delegate with assign_task, coordinate
with send_message, and report completion with signal_done. Keep task descriptions
specific, include acceptance criteria, and never bypass runtime permissions.
";

pub fn default_leader_spec(profile: &str, tools: &[&str]) -> Json {
    json!({
        "leader_id": "leader",
        "agents": [{
            "id": "leader",
            "name": "Leader",
            "role": "leader",
            "runtime_kind": "deepagents",
            "instructions": LEADER_INSTRUCTIONS,
            "model_profile": profile,
            "tool_bindings": tools,
        }],
        "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    })
}

pub struct OpenOptions {
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
    pub full_auto: bool,
    pub initial_spec: Option<Json>,
    pub catalog: Option<UserConfig>,
    /// Worker/test mode: deterministic members instead of model backends.
    pub scripts: Option<HashMap<String, Vec<Step>>>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self { cwd: None, session_id: None, full_auto: false, initial_spec: None, catalog: None, scripts: None }
    }
}

pub struct OpenedSession {
    pub runtime: Arc<Runtime>,
    pub core: Arc<CoreClient>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub catalog: UserConfig,
    release: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl OpenedSession {
    pub fn catalog(&self) -> Json {
        serde_json::to_value(&self.catalog).unwrap_or(Json::Null)
    }

    pub fn close(&self) {
        self.runtime.close();
        if let Some(release) = self.release.lock().unwrap().take() {
            release();
        }
    }
}

fn member_workdir(session_id: &str, agent_id: &str) -> PathBuf {
    let dir = session_paths(session_id).base.join("workspaces").join(agent_id);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn open_session(opts: OpenOptions) -> Result<Arc<OpenedSession>, String> {
    let cwd = match &opts.cwd {
        Some(cwd) => std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone()),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let catalog = match opts.catalog {
        Some(catalog) => catalog,
        None => load_user_config(&user_config_path())?,
    };
    let session_id = opts.session_id.clone().unwrap_or_else(|| new_session_id(&cwd));
    let paths = session_paths(&session_id);
    std::fs::create_dir_all(&paths.artifacts).map_err(|e| e.to_string())?;
    let release: Box<dyn FnOnce() + Send> = Box::new(acquire_session_lock(&session_id)?);
    let db = paths.db.to_string_lossy().into_owned();
    let core = CoreClient::open(&db, &session_id)?;

    let result = (|| -> Result<Arc<OpenedSession>, String> {
        let probe = core.call("state", json!({"session_id": session_id})).ok();
        let exists = probe
            .as_ref()
            .and_then(|state| state.get("session"))
            .map(|session| !session.is_null())
            .unwrap_or(false);
        if !exists {
            core.call("create_session", json!({
                "session_id": session_id,
                "cwd": cwd.to_string_lossy(),
                "permissions_mode": if opts.full_auto { "full_auto" } else { "approved_scope" },
            }))?;
            let spec = opts.initial_spec.clone().unwrap_or_else(|| default_leader_spec("leader_main", &["files", "shell", "web"]));
            core.call("save_spec", json!({"session_id": session_id, "spec": spec}))?;
        } else if opts.full_auto {
            if let Ok(state) = core.state() {
                if state.get("session").and_then(|s| s.get("permissions_mode")).and_then(|v| v.as_str()) != Some("full_auto") {
                    let receipt = core.submit(&TeamAction {
                        action_id: teamagents_core::models::new_id("mode"),
                        session_id: session_id.clone(),
                        actor_id: "user".into(),
                        run_id: None,
                        kind: teamagents_core::models::ActionKind::SetPermissionMode,
                        payload: json!({"mode": "full_auto"}),
                    })?;
                    if !receipt.ok {
                        return Err(receipt.error.unwrap_or_else(|| "cannot enable full auto".into()));
                    }
                }
            }
        }

        let state = core.state()?;
        let mode = state
            .get("session")
            .and_then(|s| s.get("permissions_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("approved_scope");
        let policy = PermissionPolicy { mode: mode.to_string(), ..Default::default() };
        let approvals = ApprovalGate::new(core.clone(), policy);
        let agents: Vec<AgentSpec> = serde_json::from_value(state.get("spec").and_then(|s| s.get("agents")).cloned().unwrap_or(Json::Null))
            .map_err(|e| format!("bad team spec: {e}"))?;

        let barriers: BarrierRegistry = Arc::new(Mutex::new(HashMap::new()));
        let notify = Notify::new(core.clone());
        let make_runner: RunnerFactory = make_runner_factory(
            core.clone(),
            notify.clone(),
            approvals.clone(),
            catalog.clone(),
            session_id.clone(),
            opts.scripts.clone(),
            barriers,
        );

        let mut runners: HashMap<String, Arc<dyn AgentRunner>> = HashMap::new();
        for agent in &agents {
            let runner = make_runner(agent)?;
            runners.insert(agent.id.clone(), runner);
        }
        let executor: ToolExecutor = Arc::new(crate::tools::workspace_executor(cwd.clone()));
        let limits = RuntimeLimits {
            turn_active_timeout_s: state.get("limits").and_then(|l| l.get("turn_active_timeout_s")).and_then(|v| v.as_i64()).unwrap_or(1200),
            cancel_confirm_timeout_s: state.get("limits").and_then(|l| l.get("cancel_confirm_timeout_s")).and_then(|v| v.as_i64()).unwrap_or(60),
            max_model_steps_per_turn: state.get("limits").and_then(|l| l.get("max_model_steps_per_turn")).and_then(|v| v.as_i64()).unwrap_or(200),
            max_parallel_workers: state.get("limits").and_then(|l| l.get("max_parallel_workers")).and_then(|v| v.as_i64()).unwrap_or(8),
        };
        let runtime = Runtime::new(core.clone(), notify, approvals, executor, Some(make_runner), limits);
        for (id, runner) in runners {
            runtime.add_runner(&id, runner);
        }
        Ok(Arc::new(OpenedSession {
            runtime,
            core,
            session_id,
            cwd,
            catalog,
            release: Mutex::new(Some(release)),
        }))
    })();

    match result {
        Ok(opened) => Ok(opened),
        Err(e) => Err(e),
    }
}

fn make_runner_factory(
    core: Arc<CoreClient>,
    notify: Arc<Notify>,
    approvals: Arc<ApprovalGate>,
    catalog: UserConfig,
    session_id: String,
    scripts: Option<HashMap<String, Vec<Step>>>,
    barriers: BarrierRegistry,
) -> RunnerFactory {
    Box::new(move |agent: &AgentSpec| {
        if let Some(scripts) = &scripts {
            let steps = scripts.get(&agent.id).cloned().unwrap_or_else(|| vec![Step::End]);
            return Ok(ScriptedMember::new(&agent.id, steps, barriers.clone()));
        }
        let profile: Option<ModelProfile> = catalog.models.get(&agent.model_profile).cloned();
        if agent.runtime_kind == RuntimeKind::Codex {
            let mut overrides: Vec<(String, Json)> = vec![];
            let mut model = None;
            if let Some(profile) = &profile {
                model = Some(profile.model.clone());
                if !profile.provider.is_empty() {
                    overrides.push(("model_provider".into(), json!(profile.provider)));
                }
                for (key, value) in &profile.generation_options {
                    overrides.push((key.clone(), value.clone()));
                }
            }
            return Ok(CodexRunner::new(
                CodexOptions {
                    agent_id: agent.id.clone(),
                    session_id: session_id.clone(),
                    workdir: member_workdir(&session_id, &agent.id),
                    sandbox: "workspace-write".into(),
                    approval_policy: "on-request".into(),
                    effort: Some("xhigh".into()),
                    model,
                    codex_bin: None,
                    codex_home: None,
                    env: vec![],
                    config_overrides: overrides,
                },
                core.clone(),
                approvals.clone(),
                notify.clone(),
            ));
        }
        let Some(profile) = profile else {
            return Err(format!("unknown model profile {}", agent.model_profile));
        };
        let agent_json = serde_json::to_value(agent).map_err(|e| e.to_string())?;
        Ok(ChatRunner::new(
            &agent_json,
            profile,
            Some(member_workdir(&session_id, &agent.id).to_string_lossy().into_owned()),
            notify.clone(),
        ))
    })
}
