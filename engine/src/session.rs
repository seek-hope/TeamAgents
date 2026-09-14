//! Session bootstrap (session.py::open_session): paths + lock + core session,
//! member runners, runtime loop.

use crate::chat::ChatRunner;
use crate::codex::{CodexOptions, CodexRunner};
use crate::config::{expand_home, load_user_config_for, user_config_path};
use crate::core_client::CoreClient;
use crate::gateway::{ApprovalGate, PermissionPolicy, TurnControl};
use crate::runtime::{AgentRunner, Notify, RunnerFactory, Runtime, RuntimeLimits, ToolExecutor};
use crate::scripted::{BarrierRegistry, ScriptedMember, Step};
use crate::sessions::{acquire_session_lock, new_session_id, session_paths, SessionLock};
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

/// Per-agent usage snapshot source, registered when the runner is built
/// (the AgentRunner trait stays usage-agnostic; runtime.rs untouched).
type UsageProbe = Box<dyn Fn() -> Json + Send + Sync>;
type UsageProbes = Arc<Mutex<HashMap<String, UsageProbe>>>;

/// Feature 5 (/model): per-member session-level model/effort override. Either
/// field left None falls back to the member's ModelProfile.
#[derive(Clone, Debug, Default)]
pub struct ModelOverride {
    pub model: Option<String>,
    pub effort: Option<String>,
}
pub type ModelOverrides = Arc<Mutex<HashMap<String, ModelOverride>>>;

pub struct OpenedSession {
    pub runtime: Arc<Runtime>,
    pub core: Arc<CoreClient>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub catalog: UserConfig,
    usage_probes: UsageProbes,
    model_overrides: ModelOverrides,
    /// Held for the session's lifetime; dropping it (normal close or any error
    /// path during open) releases the session for other processes.
    lock: Mutex<Option<SessionLock>>,
}

impl OpenedSession {
    pub fn catalog(&self) -> Json {
        serde_json::to_value(&self.catalog).unwrap_or(Json::Null)
    }

    /// Per-agent token usage for /status (worker "usage" method, CLI status).
    /// Session-memory counters only — a restarted session starts from zero.
    pub fn usage_report(&self) -> Json {
        let agents = self
            .core
            .state()
            .ok()
            .and_then(|state| state.get("spec").and_then(|s| s.get("agents")).cloned())
            .and_then(|agents| agents.as_array().cloned())
            .unwrap_or_default();
        let probes = self.usage_probes.lock().unwrap();
        let report: Vec<Json> = agents
            .iter()
            .map(|agent| {
                let id = agent.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let profile_name = agent.get("model_profile").and_then(|v| v.as_str()).unwrap_or("");
                let profile = self.catalog.models.get(profile_name);
                let usage = probes.get(id).map(|probe| probe());
                // codex reports its own window; the profile wins when set
                let context_window = profile
                    .and_then(|p| p.context_window)
                    .or_else(|| usage.as_ref().and_then(|u| u.get("codex_context_window")).and_then(Json::as_u64));
                json!({
                    "agent_id": id,
                    "name": agent.get("name").and_then(|v| v.as_str()).unwrap_or(id),
                    "model_profile": profile_name,
                    "model": profile.map(|p| p.model.as_str()),
                    "context_window": context_window,
                    "usage": usage,
                })
            })
            .collect();
        json!({"session_id": self.session_id, "agents": report})
    }

    /// (leader_id, current conversation thread) for rewind/fork (D-26).
    /// The thread is `ctx:{agent}:{context_epoch}` (control.rs::context_ref).
    fn leader_thread(&self) -> Result<(String, String), String> {
        let state = self.core.call_in_session("state", json!({"include_events": false}))?;
        let leader = state
            .get("leader_id")
            .and_then(|v| v.as_str())
            .ok_or("no leader")?
            .to_string();
        let epoch = state
            .get("agents")
            .and_then(|v| v.as_array())
            .and_then(|agents| agents.iter().find(|a| a.get("id").and_then(|v| v.as_str()) == Some(leader.as_str())))
            .and_then(|a| a.get("context_epoch"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        Ok((leader.clone(), format!("ctx:{leader}:{epoch}")))
    }

    /// Rewind targets on the leader's conversation (user inputs, newest first).
    pub fn rewind_points(&self) -> Result<Json, String> {
        let (leader, thread) = self.leader_thread()?;
        let points = self
            .runtime
            .runner(&leader)
            .map(|r| r.rewind_points(&thread))
            .unwrap_or_default();
        Ok(json!({"agent_id": leader, "thread": thread, "points": points}))
    }

    /// Move the leader conversation's live tip (`node_id`, None = empty).
    /// Refused while the leader has an active turn: the in-flight tail would
    /// graft onto the rewound tip (ponytail ceiling, chat.rs run_segment).
    pub fn rewind(&self, node_id: Option<String>) -> Result<Json, String> {
        let (leader, thread) = self.leader_thread()?;
        let state = self.core.call_in_session("state", json!({"include_events": false}))?;
        let busy = state
            .get("runs")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .any(|r| {
                r.get("agent_id").and_then(|v| v.as_str()) == Some(leader.as_str())
                    && r.get("status").and_then(|v| v.as_str()).map(|s| s == "QUEUED" || s == "RUNNING").unwrap_or(false)
            });
        if busy {
            return Err("leader 回合进行中，等它结束后再 rewind".into());
        }
        let runner = self.runtime.runner(&leader).ok_or("leader 还没有运行器（尚未有过回合）")?;
        let depth = runner.rewind(&thread, node_id.as_deref())?;
        Ok(json!({"agent_id": leader, "thread": thread, "depth": depth}))
    }

    fn spec_agents(&self) -> Result<Vec<AgentSpec>, String> {
        let state = self.core.state()?;
        let agents = state.get("spec").and_then(|s| s.get("agents")).cloned().unwrap_or(Json::Null);
        serde_json::from_value(agents).map_err(|e| format!("bad spec agents: {e}"))
    }

    /// What one member's next turn would run with: override fields win, the
    /// rest falls back to the profile (codex effort defaults to the runner's
    /// built-in "xhigh").
    fn effective_model(&self, agent: &AgentSpec) -> Json {
        let profile = self.catalog.models.get(&agent.model_profile);
        let ov = self.model_overrides.lock().unwrap().get(&agent.id).cloned();
        let (o_model, o_effort) = ov.as_ref().map(|o| (o.model.clone(), o.effort.clone())).unwrap_or_default();
        let model = o_model.or_else(|| profile.map(|p| p.model.clone()));
        let effort = o_effort.or_else(|| {
            if agent.runtime_kind == RuntimeKind::Codex {
                Some("xhigh".into())
            } else {
                profile
                    .and_then(|p| p.generation_options.get("reasoning_effort"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }
        });
        json!({
            "agent_id": agent.id,
            "name": agent.name,
            "model_profile": agent.model_profile,
            "model": model,
            "effort": effort,
            "overridden": ov.is_some(),
        })
    }

    /// Feature 5 (/model): switch one member's model / reasoning effort for
    /// this session; both None clears the override (profile default). The
    /// cached runner is dropped so the change lands on the member's next turn.
    /// ponytail: session memory only, never written back to the TeamSpec —
    /// persist overrides when a user actually asks for cross-session sticky.
    pub fn set_model_override(&self, agent_id: &str, model: Option<String>, effort: Option<String>) -> Result<Json, String> {
        let agents = self.spec_agents()?;
        let agent = agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| format!("unknown member {agent_id}"))?;
        if let Some(model) = &model {
            if model.trim().is_empty() {
                return Err("model must be a non-empty string".into());
            }
        }
        if let Some(effort) = &effort {
            const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
            if !EFFORTS.contains(&effort.to_ascii_lowercase().as_str()) {
                return Err(format!("effort must be one of {}", EFFORTS.join("/")));
            }
        }
        {
            let mut overrides = self.model_overrides.lock().unwrap();
            if model.is_none() && effort.is_none() {
                overrides.remove(agent_id);
            } else {
                overrides.insert(agent_id.to_string(), ModelOverride { model, effort });
            }
        }
        self.runtime.drop_runner(agent_id);
        Ok(self.effective_model(agent))
    }

    /// `/model` with no args / worker "model": every member's effective values.
    pub fn model_report(&self) -> Json {
        let agents = self.spec_agents().unwrap_or_default();
        json!({
            "session_id": self.session_id,
            "agents": agents.iter().map(|a| self.effective_model(a)).collect::<Vec<_>>(),
        })
    }

    pub fn close(&self) {
        self.runtime.close();
        let _ = self.lock.lock().unwrap().take();
    }
}

/// session.py::_skills_and_memory — the member's skills directories and
/// instruction (memory) files, with their contents read for prompt injection.
///
/// ponytail: the Python build mounts them as a virtual filesystem the member
/// reads with file tools; here the bounded contents go straight into the system
/// prompt (upgrade path: a read_skill tool once skills outgrow the prompt).
fn member_context(catalog: &UserConfig, cwd: &std::path::Path, session_id: &str, agent: &AgentSpec) -> Vec<(String, String)> {
    const PER_FILE: usize = 8_000;
    const TOTAL: usize = 32_000;
    let mut out: Vec<(String, String)> = vec![];
    let mut budget = TOTAL;

    // Only the member's selected skills are injected (plan §12.1: discovery +
    // on-demand read via the `skill` tool; injection is the Leader's
    // distribution channel, set per member in TeamSpec/topology patches).
    // Later roots override earlier ones on a name clash: user < project < member.
    let mut roots: Vec<PathBuf> = catalog.skills_paths.iter().map(|p| expand_home(p)).collect();
    roots.push(cwd.join(".teamagents").join("skills"));
    roots.push(session_paths(session_id).base.join("members").join(&agent.id).join("skills"));
    let mut selected: Vec<(String, PathBuf)> = vec![];
    // skill_candidates canonicalizes and rejects symlink escapes (P2-6)
    for dir in roots.into_iter().rev() {
        for (name, path) in crate::tools::skill_candidates(&dir) {
            if !agent.skills.iter().any(|s| *s == name) || selected.iter().any(|(n, _)| *n == name) {
                continue;
            }
            selected.push((name, path));
        }
    }
    for name in &agent.skills {
        if !selected.iter().any(|(n, _)| n == name) {
            eprintln!("member {}: selected skill {name:?} not found in skills_paths", agent.id);
        }
    }
    for (name, path) in selected {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let text: String = text.chars().take(PER_FILE).collect();
        if text.chars().count() > budget {
            break;
        }
        budget -= text.chars().count();
        out.push((format!("skill {name}"), text));
    }

    let mut memory: Vec<PathBuf> = catalog.instruction_files.iter().map(|p| expand_home(p)).collect();
    for candidate in [cwd.join("AGENTS.md"), user_config_path().parent().map(|p| p.join("AGENTS.md")).unwrap_or_default()] {
        if candidate.is_file() {
            memory.push(candidate);
        }
    }
    for path in memory {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let text: String = text.chars().take(PER_FILE).collect();
        if text.chars().count() > budget {
            break;
        }
        budget -= text.chars().count();
        let label = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "instructions".into());
        out.push((format!("instructions {label}"), text));
    }
    out
}

/// `sessions/<id>/members/<agent>/` — the member's private directory
/// (session.py::_member_workspace, same layout as the Python build).
fn member_dir(session_id: &str, agent_id: &str) -> PathBuf {
    let dir = session_paths(session_id).base.join("members").join(agent_id);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Workspace policy decides one member's working directory — and therefore
/// what its file tools and its backend can reach (plan §8/P5, workspace.py).
fn member_root(agent: &AgentSpec, cwd: &std::path::Path, session_id: &str) -> Result<PathBuf, String> {
    let workspace = crate::workspace::prepare(agent, cwd, &member_dir(session_id, &agent.id))?;
    if let Some(note) = &workspace.note {
        eprintln!("member {}: {}", agent.id, note);
    }
    Ok(workspace.path)
}

pub fn open_session(opts: OpenOptions) -> Result<Arc<OpenedSession>, String> {
    let cwd = match &opts.cwd {
        Some(cwd) => std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone()),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let catalog = match opts.catalog {
        Some(catalog) => catalog,
        None => load_user_config_for(&cwd)?,
    };
    let config_full_auto = crate::config::permission_mode_from_config()? == "full_auto";
    let full_auto = opts.full_auto || config_full_auto;
    let session_id = opts.session_id.clone().unwrap_or_else(|| new_session_id(&cwd));
    let paths = session_paths(&session_id);
    std::fs::create_dir_all(&paths.artifacts).map_err(|e| e.to_string())?;
    // dropped on every early return below, so a failed open never keeps the lock
    let lock = acquire_session_lock(&session_id)?;
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
            match core.call("create_session", json!({
                "session_id": session_id,
                "cwd": cwd.to_string_lossy(),
                "permissions_mode": if full_auto { "full_auto" } else { "approved_scope" },
            })) {
                Ok(_) => {
                    let spec = opts
                        .initial_spec
                        .clone()
                        .unwrap_or_else(|| default_leader_spec("leader_main", &["files", "shell", "web"]));
                    core.call("save_spec", json!({"session_id": session_id, "spec": spec}))?;
                }
                // the row exists but `state` failed: a session left without a
                // loadable spec by an earlier failed open. An explicitly given
                // spec repairs it; otherwise keep going so the real problem is
                // reported instead of a UNIQUE-constraint error.
                Err(e) if e.contains("UNIQUE") => {
                    if let Some(spec) = opts.initial_spec.clone() {
                        core.call("save_spec", json!({"session_id": session_id, "spec": spec}))?;
                    }
                }
                Err(e) => return Err(e),
            }
        } else if full_auto {
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

        // keep validation (topology patches, member profiles) in sync with the
        // user config the engine loaded
        core.call("set_catalog", json!({"session_id": session_id, "catalog": catalog}))?;
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
        let usage_probes: UsageProbes = Arc::new(Mutex::new(HashMap::new()));
        let model_overrides: ModelOverrides = Arc::new(Mutex::new(HashMap::new()));
        let make_runner: RunnerFactory = make_runner_factory(
            core.clone(),
            notify.clone(),
            approvals.clone(),
            catalog.clone(),
            session_id.clone(),
            cwd.clone(),
            opts.scripts.clone(),
            barriers,
            usage_probes.clone(),
            model_overrides.clone(),
        );

        let mut runners: HashMap<String, Arc<dyn AgentRunner>> = HashMap::new();
        for agent in &agents {
            let runner = make_runner(agent)?;
            runners.insert(agent.id.clone(), runner);
        }
        let executor = member_executor_factory(core.clone(), catalog.clone(), session_id.clone(), cwd.clone());
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
            usage_probes,
            model_overrides,
            lock: Mutex::new(Some(lock)),
        }))
    })();
    result
}

fn make_runner_factory(
    core: Arc<CoreClient>,
    notify: Arc<Notify>,
    approvals: Arc<ApprovalGate>,
    catalog: UserConfig,
    session_id: String,
    cwd: PathBuf,
    scripts: Option<HashMap<String, Vec<Step>>>,
    barriers: BarrierRegistry,
    usage_probes: UsageProbes,
    model_overrides: ModelOverrides,
) -> RunnerFactory {
    Box::new(move |agent: &AgentSpec| {
        if let Some(scripts) = &scripts {
            let steps = scripts.get(&agent.id).cloned().unwrap_or_else(|| vec![Step::End]);
            return Ok(ScriptedMember::new(&agent.id, steps, barriers.clone()));
        }
        let profile: Option<ModelProfile> = catalog.models.get(&agent.model_profile).cloned();
        // feature 5 (/model): a session-level override wins over the profile
        let ov = model_overrides.lock().unwrap().get(&agent.id).cloned().unwrap_or_default();
        if agent.runtime_kind == RuntimeKind::Codex {
            let opts = codex_options(agent, profile.as_ref(), &ov, &session_id, &cwd)?;
            let runner = CodexRunner::new(
                opts,
                core.clone(),
                approvals.clone(),
                notify.clone(),
            );
            let probe = runner.clone();
            usage_probes
                .lock()
                .unwrap()
                .insert(agent.id.clone(), Box::new(move || probe.usage_snapshot()));
            return Ok(runner);
        }
        let Some(profile) = profile else {
            return Err(format!("unknown model profile {}", agent.model_profile));
        };
        let profile = apply_model_override(profile, &ov);
        let agent_json = serde_json::to_value(agent).map_err(|e| e.to_string())?;
        let bound = crate::bound::BoundTools::load(&catalog, &agent.tool_bindings)?;
        // a required web service that cannot load fails the member, not the call;
        // the resolved set is also what the model gets advertised, so an explicit
        // binding name (anything but the literal "web") still exposes the tools
        let web = crate::tools::web_tools(&catalog, &agent.tool_bindings)?;
        let context = member_context(&catalog, &cwd, &session_id, agent);
        let runner = ChatRunner::new(
            &agent_json,
            profile,
            Some(member_root(agent, &cwd, &session_id)?.to_string_lossy().into_owned()),
            notify.clone(),
            bound,
            context,
            (web.search.is_some(), web.fetch.is_some()),
        );
        let probe = runner.clone();
        usage_probes
            .lock()
            .unwrap()
            .insert(agent.id.clone(), Box::new(move || probe.usage_snapshot()));
        Ok(runner)
    })
}

/// Chat-runner profile with the session override applied (feature 5).
fn apply_model_override(mut profile: ModelProfile, ov: &ModelOverride) -> ModelProfile {
    if let Some(model) = &ov.model {
        profile.model = model.clone();
    }
    if let Some(effort) = &ov.effort {
        profile.generation_options.insert("reasoning_effort".into(), json!(effort));
    }
    profile
}

/// CodexOptions for one codex member; the session override wins over the
/// profile model and over the runner's default "xhigh" effort (feature 5).
fn codex_options(
    agent: &AgentSpec,
    profile: Option<&ModelProfile>,
    ov: &ModelOverride,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<CodexOptions, String> {
    let mut config: Vec<(String, Json)> = vec![];
    let mut model = None;
    if let Some(profile) = profile {
        model = Some(profile.model.clone());
        if !profile.provider.is_empty() {
            config.push(("model_provider".into(), json!(profile.provider)));
        }
        for (key, value) in &profile.generation_options {
            config.push((key.clone(), value.clone()));
        }
    }
    if let Some(m) = &ov.model {
        model = Some(m.clone());
    }
    Ok(CodexOptions {
        agent_id: agent.id.clone(),
        session_id: session_id.into(),
        workdir: member_root(agent, cwd, session_id)?,
        sandbox: "workspace-write".into(),
        approval_policy: "on-request".into(),
        effort: ov.effort.clone().or_else(|| Some("xhigh".into())),
        model,
        codex_bin: None,
        codex_home: None,
        env: vec![],
        config_overrides: config,
    })
}

/// One executor per member root, refreshed when its config revision changes
/// (members added mid-session resolve on their first tool call).
fn member_executor_factory(
    core: Arc<CoreClient>,
    catalog: UserConfig,
    session_id: String,
    cwd: PathBuf,
) -> ToolExecutor {
    type MemberExecutor = Arc<dyn Fn(&str, &Json, &TurnControl) -> Result<Json, String> + Send + Sync>;
    let cache: Mutex<HashMap<String, (i64, MemberExecutor)>> = Mutex::new(HashMap::new());
    let artifacts = session_paths(&session_id).artifacts;
    Arc::new(move |agent_id: &str, tool: &str, args: &Json, control: &TurnControl| {
        control.check()?;
        // cheap per-call probe; the full state pull below only happens when the
        // executor must be (re)built, not on every tool call (P2-8)
        let revision = core
            .call_in_session("agent_config_revision", json!({"agent_id": agent_id}))?
            .get("config_revision")
            .and_then(|v| v.as_i64())
            .ok_or("bad agent_config_revision response")?;
        let cached = cache.lock().unwrap().get(agent_id).cloned();
        if let Some((cached_revision, executor)) = cached {
            if cached_revision == revision { return executor(tool, args, control); }
        }
        let state = core.call_in_session("state", json!({"include_events": false}))?;
        let agent = state.get("spec").and_then(|spec| spec.get("agents")).cloned()
            .and_then(|agents| serde_json::from_value::<Vec<AgentSpec>>(agents).ok())
            .and_then(|agents| agents.into_iter().find(|a| a.id == agent_id)).ok_or("member is no longer configured")?;
        let executor: MemberExecutor = Arc::new(crate::tools::member_executor_with_control(
            member_root(&agent, &cwd, &session_id)?,
            catalog.clone(),
            agent.tool_bindings.clone(),
            Some(artifacts.clone()),
        ));
        cache.lock().unwrap().insert(agent_id.to_string(), (revision, executor.clone()));
        executor(tool, args, control)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_context_collects_skills_and_instruction_files() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        let home = root.join("home");
        std::fs::create_dir_all(cwd.join(".teamagents/skills/review")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(cwd.join("AGENTS.md"), "project instructions").unwrap();
        std::fs::write(cwd.join(".teamagents/skills/review/SKILL.md"), "review skill body").unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_STATE_HOME", home.join(".state"));
        std::fs::create_dir_all(crate::config::user_config_path().parent().unwrap()).unwrap();
        std::fs::write(crate::config::user_config_path().parent().unwrap().join("AGENTS.md"), "user memory").unwrap();

        let catalog = UserConfig::default();
        let picked = AgentSpec {
            id: "leader".into(), name: "Leader".into(), role: "leader".into(),
            runtime_kind: RuntimeKind::Deepagents, instructions: String::new(),
            model_profile: String::new(), tool_bindings: vec![],
            skills: vec!["review".into()], workspace_policy: teamagents_core::models::WorkspacePolicy::Shared,
        };
        let context = member_context(&catalog, &cwd, "s1", &picked);
        let labels: Vec<&str> = context.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"skill review"), "{labels:?}");
        assert!(labels.contains(&"instructions AGENTS.md"), "{labels:?}");
        let bodies: Vec<&str> = context.iter().map(|(_, c)| c.as_str()).collect();
        assert!(bodies.iter().any(|b| b.contains("review skill body")));
        assert!(bodies.iter().any(|b| b.contains("project instructions")));
        assert!(bodies.iter().any(|b| b.contains("user memory")));
        // skills not named in the member's spec are not injected (discovery is
        // the `skill` tool's job; injection is the Leader's distribution channel)
        let unpicked = AgentSpec { skills: vec![], ..picked.clone() };
        let context = member_context(&catalog, &cwd, "s1", &unpicked);
        assert!(!context.iter().any(|(l, _)| l == "skill review"), "{context:?}");
        assert!(context.iter().any(|(l, _)| l == "instructions AGENTS.md"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn member_context_rejects_symlinked_skills() {
        // P2-6: a skill dir/file that symlinks outside the registry root is dropped
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-symlink-skill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        let home = root.join("home");
        let registry = root.join("registry");
        let outside = root.join("outside");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(registry.join("legit")).unwrap();
        std::fs::create_dir_all(outside.join("loot")).unwrap();
        std::fs::write(registry.join("legit/SKILL.md"), "legit body").unwrap();
        std::fs::write(outside.join("loot/SKILL.md"), "outside secret").unwrap();
        std::os::unix::fs::symlink(outside.join("loot"), registry.join("escape")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_STATE_HOME", home.join(".state"));

        let mut catalog = UserConfig::default();
        catalog.skills_paths = vec![registry.to_string_lossy().into_owned()];
        let agent = AgentSpec {
            id: "m".into(), name: "M".into(), role: "worker".into(),
            runtime_kind: RuntimeKind::Deepagents, instructions: String::new(),
            model_profile: String::new(), tool_bindings: vec![],
            skills: vec!["legit".into(), "escape".into()],
            workspace_policy: teamagents_core::models::WorkspacePolicy::Shared,
        };
        let context = member_context(&catalog, &cwd, "s-sym", &agent);
        assert!(context.iter().any(|(l, _)| l == "skill legit"), "{context:?}");
        assert!(!context.iter().any(|(l, _)| l == "skill escape"), "{context:?}");
        assert!(!context.iter().any(|(_, body)| body.contains("outside secret")), "{context:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn session_override_rewrites_chat_profile_and_codex_options() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-model-ov-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("project")).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", root.join("home/.config"));
        std::env::set_var("XDG_STATE_HOME", root.join("home/.state"));

        let profile = ModelProfile {
            provider: "openai".into(), protocol: "openai".into(), model: "gpt-default".into(),
            base_url: None, api_key_env: None, timeout: 120, max_retries: 5,
            generation_options: HashMap::from([("reasoning_effort".to_string(), json!("medium"))]),
            context_window: None,
        };
        let ov = ModelOverride { model: Some("gpt-5".into()), effort: Some("high".into()) };
        let rewritten = apply_model_override(profile.clone(), &ov);
        assert_eq!(rewritten.model, "gpt-5");
        assert_eq!(rewritten.generation_options["reasoning_effort"], json!("high"));
        let untouched = apply_model_override(profile.clone(), &ModelOverride::default());
        assert_eq!(untouched.model, "gpt-default");
        assert_eq!(untouched.generation_options["reasoning_effort"], json!("medium"));

        // codex member: the override lands in opts.model / opts.effort
        let agent = AgentSpec {
            id: "cod".into(), name: "Cod".into(), role: "dev".into(),
            runtime_kind: RuntimeKind::Codex, instructions: String::new(),
            model_profile: "m".into(), tool_bindings: vec![], skills: vec![],
            workspace_policy: teamagents_core::models::WorkspacePolicy::Shared,
        };
        let opts = codex_options(&agent, Some(&profile), &ov, "s-ov", &root.join("project")).unwrap();
        assert_eq!(opts.model.as_deref(), Some("gpt-5"));
        assert_eq!(opts.effort.as_deref(), Some("high"));
        assert!(opts.config_overrides.iter().any(|(k, v)| k == "model_provider" && v == &json!("openai")));
        // profile generation_options still pass through as config overrides
        assert!(opts.config_overrides.iter().any(|(k, v)| k == "reasoning_effort" && v == &json!("medium")));
        // model-only override keeps the runner's default effort
        let opts = codex_options(
            &agent, Some(&profile),
            &ModelOverride { model: Some("gpt-5-codex".into()), effort: None },
            "s-ov", &root.join("project"),
        ).unwrap();
        assert_eq!(opts.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(opts.effort.as_deref(), Some("xhigh"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn member_executor_factory_probes_config_revision() {
        // P2-8: the factory resolves members through the lightweight
        // agent_config_revision endpoint (unknown method/bad reply would fail here)
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-factory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        let home = root.join("home");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
        std::env::set_var("XDG_STATE_HOME", home.join(".state"));

        let core = CoreClient::open(":memory:", "s-factory").expect("core");
        core.call("create_session", json!({"session_id": "s-factory", "cwd": cwd})).expect("create");
        core.call("set_catalog", json!({"session_id": "s-factory", "catalog": {
            "models": {"m": {"provider": "openai", "protocol": "openai", "model": "test"}},
            "tools": {}, "skills_paths": [], "instruction_files": [],
        }})).expect("catalog");
        core.call("save_spec", json!({"session_id": "s-factory", "spec": {
            "leader_id": "lead",
            "agents": [{"id": "lead", "name": "L", "role": "leader", "runtime_kind": "deepagents",
                        "model_profile": "m"},
                       {"id": "m", "name": "M", "role": "worker", "runtime_kind": "deepagents",
                        "model_profile": "m", "tool_bindings": ["files"]}],
            "shared_spaces": [{"id": "main", "readers": ["m"], "writers": ["m"]}],
        }})).expect("spec");

        let factory = member_executor_factory(core, UserConfig::default(), "s-factory".into(), cwd.clone());
        let control = TurnControl::default();
        factory("m", "write_file", &json!({"path": "note.txt", "content": "hello"}), &control).unwrap();
        // cached executor serves the second call
        let read = factory("m", "read_file", &json!({"path": "note.txt"}), &control).unwrap();
        assert_eq!(read, json!("hello"));
        // an unknown member is still a clean error, not a panic or stale cache hit
        let err = factory("ghost", "read_file", &json!({"path": "note.txt"}), &control).unwrap_err();
        assert!(err.contains("no longer configured"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
