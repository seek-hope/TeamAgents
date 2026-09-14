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

pub struct OpenedSession {
    pub runtime: Arc<Runtime>,
    pub core: Arc<CoreClient>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub catalog: UserConfig,
    /// Held for the session's lifetime; dropping it (normal close or any error
    /// path during open) releases the session for other processes.
    lock: Mutex<Option<SessionLock>>,
}

impl OpenedSession {
    pub fn catalog(&self) -> Json {
        serde_json::to_value(&self.catalog).unwrap_or(Json::Null)
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
        let make_runner: RunnerFactory = make_runner_factory(
            core.clone(),
            notify.clone(),
            approvals.clone(),
            catalog.clone(),
            session_id.clone(),
            cwd.clone(),
            opts.scripts.clone(),
            barriers,
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
                    workdir: member_root(agent, &cwd, &session_id)?,
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
        let bound = crate::bound::BoundTools::load(&catalog, &agent.tool_bindings)?;
        // a required web service that cannot load fails the member, not the call;
        // the resolved set is also what the model gets advertised, so an explicit
        // binding name (anything but the literal "web") still exposes the tools
        let web = crate::tools::web_tools(&catalog, &agent.tool_bindings)?;
        let context = member_context(&catalog, &cwd, &session_id, agent);
        Ok(ChatRunner::new(
            &agent_json,
            profile,
            Some(member_root(agent, &cwd, &session_id)?.to_string_lossy().into_owned()),
            notify.clone(),
            bound,
            context,
            (web.search.is_some(), web.fetch.is_some()),
        ))
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
