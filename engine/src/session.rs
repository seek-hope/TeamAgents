//! Session bootstrap: paths + lock + core session,
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
use teamagents_core::models::{AgentSpec, ChannelMode, ChannelSpec, ModelProfile, RuntimeKind, TeamAction, UserConfig};

pub const LEADER_INSTRUCTIONS: &str = "You are the Leader of a team of agents. Understand the user's goal, decide
whether to work alone or build a team, delegate with assign_task, coordinate
with send_message, and report completion with signal_done. Keep task descriptions
specific, include acceptance criteria, and never bypass runtime permissions.
When you add a member, give it the tool bindings its work needs (for example
[\"files\", \"shell\"] for coding) and add a channel for it in the same patch —
a member without bindings can only send messages and tasks, and can only reach
you over an existing channel.
When creating a member via apply_topology_patch add_agent, you may omit
model_profile (a per-member profile is auto-created from your current model),
or set it to a model id (reuses your connection) or an existing profile name.
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

#[derive(Default)]
pub struct OpenOptions {
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,
    pub full_auto: bool,
    pub initial_spec: Option<Json>,
    pub catalog: Option<UserConfig>,
    /// Worker/test mode: deterministic members instead of model backends.
    pub scripts: Option<HashMap<String, Vec<Step>>>,
}

/// Per-agent usage source; counters may outlive a backend but never own it.
type UsageProbe = Box<dyn Fn() -> Json + Send + Sync>;
type UsageProbes = Arc<Mutex<HashMap<String, UsageProbe>>>;

fn usage_probe<T: Send + Sync + 'static>(runner: &Arc<T>, snapshot: fn(&T) -> Json) -> UsageProbe {
    let last = Mutex::new(snapshot(runner));
    let weak = Arc::downgrade(runner);
    Box::new(move || {
        let mut last = last.lock().unwrap();
        if let Some(runner) = weak.upgrade() {
            *last = snapshot(&runner);
        }
        last.clone()
    })
}

/// Feature 5 (/model): per-member session-level model/effort override. Either
/// field left None falls back to the member's ModelProfile. Persisted per
/// session (D-29), so a reopened session keeps its /model choices.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ModelOverride {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}
pub type ModelOverrides = Arc<Mutex<HashMap<String, ModelOverride>>>;

/// `sessions/<id>/model_overrides.json` (D-29).
fn overrides_path(session_id: &str) -> PathBuf {
    session_paths(session_id).base.join("model_overrides.json")
}

/// `sessions/<id>/profiles.json` — per-session model profiles auto-created when
/// the Leader adds a member without naming an existing profile (D-30).
/// Within one session each member maps to its own profile; another session gets
/// another set.
fn profiles_path(session_id: &str) -> PathBuf {
    session_paths(session_id).base.join("profiles.json")
}

fn load_session_profiles(session_id: &str) -> HashMap<String, ModelProfile> {
    let Ok(text) = std::fs::read_to_string(profiles_path(session_id)) else { return HashMap::new() };
    serde_json::from_str(&text).unwrap_or_default()
}

/// Atomic rewrite of `profiles.json` (tmp + rename).
fn save_session_profiles(session_id: &str, profiles: &HashMap<String, ModelProfile>) -> Result<(), String> {
    let path = profiles_path(session_id);
    let tmp = path.with_extension("tmp");
    let text = serde_json::to_string_pretty(profiles).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("persist session profiles: {e}"))
}

/// Load persisted overrides, dropping entries that no longer validate against
/// the current spec/catalog (members or profiles may have changed since).
fn load_model_overrides(
    session_id: &str,
    catalog: &UserConfig,
    agents: &[AgentSpec],
) -> HashMap<String, ModelOverride> {
    let Ok(text) = std::fs::read_to_string(overrides_path(session_id)) else { return HashMap::new() };
    let parsed: HashMap<String, ModelOverride> = serde_json::from_str(&text).unwrap_or_default();
    parsed
        .into_iter()
        .filter(|(id, ov)| {
            let Some(agent) = agents.iter().find(|a| &a.id == id) else { return false };
            let Some(profile) = catalog.models.get(ov.profile.as_ref().unwrap_or(&agent.model_profile)) else {
                return false;
            };
            if ov.profile.is_some() && agent.runtime_kind == RuntimeKind::Codex && !codex_compatible(profile) {
                return false;
            }
            ov.effort.as_ref().is_none_or(|e| model_efforts(&profile.protocol).contains(&e.as_str()))
        })
        .collect()
}

pub struct OpenedSession {
    pub runtime: Arc<Runtime>,
    pub core: Arc<CoreClient>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub catalog: UserConfig,
    /// Session-scoped auto-created profiles (D-30); win over user config names.
    session_profiles: Arc<Mutex<HashMap<String, ModelProfile>>>,
    usage_probes: UsageProbes,
    model_overrides: ModelOverrides,
    /// Held for the session's lifetime; dropping it (normal close or any error
    /// path during open) releases the session for other processes.
    lock: Mutex<Option<SessionLock>>,
}

impl OpenedSession {
    /// Workspace evidence for the human UI, never part of a member view or tool.
    pub fn review(&self, agent_id: &str, path: Option<&str>, offset: usize) -> Result<Json, String> {
        self.review_checked(agent_id, path, offset, None)
    }

    pub fn review_checked(
        &self,
        agent_id: &str,
        path: Option<&str>,
        offset: usize,
        revision: Option<&str>,
    ) -> Result<Json, String> {
        self.review_cancellable(agent_id, path, offset, revision, &std::sync::atomic::AtomicBool::new(false))
    }

    pub(crate) fn review_cancellable(
        &self,
        agent_id: &str,
        path: Option<&str>,
        offset: usize,
        revision: Option<&str>,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Json, String> {
        let state = self.core.call_in_session("state", json!({"include_events":false}))?;
        let agent = state["spec"]["agents"]
            .as_array()
            .and_then(|agents| agents.iter().find(|a| a["id"].as_str() == Some(agent_id)))
            .ok_or("审查成员不存在")?;
        let paths = session_paths(&self.session_id);
        let root = crate::review::registered_root(&paths.base.join("members").join(agent_id))?;
        let mut report = crate::review::report_cancellable(&paths.base, &root, path, offset, revision, cancelled)?;
        report["agent_id"] = json!(agent_id);
        report["shared"] = json!(root == self.cwd);
        report["policy"] = agent["workspace_policy"].clone();
        Ok(report)
    }

    pub fn catalog(&self) -> Json {
        let mut merged = self.catalog.clone();
        merged.models.extend(self.session_profiles.lock().unwrap().clone());
        serde_json::to_value(&merged).unwrap_or(Json::Null)
    }

    /// Session profiles shadow user-config profiles of the same name (D-30).
    fn lookup_model(&self, name: &str) -> Option<ModelProfile> {
        self.session_profiles.lock().unwrap().get(name).cloned().or_else(|| self.catalog.models.get(name).cloned())
    }

    /// User config models + session profiles (session wins), sorted by name.
    fn merged_models(&self) -> std::collections::BTreeMap<String, ModelProfile> {
        let mut merged: std::collections::BTreeMap<String, ModelProfile> =
            self.catalog.models.clone().into_iter().collect();
        merged.extend(self.session_profiles.lock().unwrap().clone());
        merged
    }

    /// Save the connection globally and expose it to this session's runner factory.
    pub fn add_model_provider(&self, input: crate::config::CustomProvider) -> Result<Json, String> {
        let (name, profile) = input.profile()?;
        let mut local = self.session_profiles.lock().unwrap();
        if local.contains_key(&name)
            || self.catalog.models.contains_key(&name)
            || local.values().chain(self.catalog.models.values()).any(|p| p.provider == name)
        {
            return Err("此供应商名称或同名模型配置已存在，请使用其他名称".into());
        }
        crate::config::save_custom_provider(&name, &profile)?;
        // Existing factory closures share this overlay. New sessions load the
        // saved user config; no restart or replacement of a live runner is needed.
        local.insert(name.clone(), profile);
        drop(local);
        let mut report = self.model_report();
        report["added_profile"] = json!(name);
        Ok(report)
    }

    /// Per-agent token usage for /status (worker "usage" method, CLI status).
    /// Chat counters persist; Codex counters are supplied by the live backend.
    pub fn usage_report(&self) -> Json {
        let agents = self
            .core
            .state_brief()
            .ok()
            .and_then(|state| state.get("spec").and_then(|s| s.get("agents")).cloned())
            .and_then(|agents| agents.as_array().cloned())
            .unwrap_or_default();
        // Do not prune by this snapshot: a concurrent patch may already have
        // registered a new member's probe. Removed probes hold only counters.
        let probes = self.usage_probes.lock().unwrap();
        let report: Vec<Json> = agents
            .iter()
            .map(|agent| {
                let id = agent.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let ov = self.model_overrides.lock().unwrap().get(id).cloned().unwrap_or_default();
                let profile_name = ov
                    .profile
                    .as_deref()
                    .unwrap_or_else(|| agent.get("model_profile").and_then(|v| v.as_str()).unwrap_or(""));
                let profile = self.lookup_model(profile_name);
                let usage = probes.get(id).map(|probe| probe());
                // codex reports its own window; the profile wins when set
                let context_window = profile
                    .as_ref()
                    .and_then(|p| p.context_window)
                    .or_else(|| usage.as_ref().and_then(|u| u.get("codex_context_window")).and_then(Json::as_u64));
                json!({
                    "agent_id": id,
                    "name": agent.get("name").and_then(|v| v.as_str()).unwrap_or(id),
                    "model_profile": profile_name,
                    "model": ov.model.as_deref().or_else(|| profile.as_ref().map(|p| p.model.as_str())),
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
        let leader = state.get("leader_id").and_then(|v| v.as_str()).ok_or("no leader")?.to_string();
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
        let points = self.runtime.runner(&leader).map(|r| r.rewind_points(&thread)).transpose()?.unwrap_or_default();
        Ok(json!({"agent_id": leader, "thread": thread, "points": points}))
    }

    /// Move the leader conversation's live tip (`node_id`, None = empty).
    /// Refused while the leader has an active turn: the in-flight tail would
    /// graft onto the rewound tip (ponytail ceiling, chat.rs run_segment).
    pub fn rewind(&self, node_id: Option<String>) -> Result<Json, String> {
        let (leader, thread) = self.leader_thread()?;
        let state = self.core.call_in_session("state", json!({"include_events": false}))?;
        let busy = state.get("runs").and_then(|v| v.as_array()).cloned().unwrap_or_default().into_iter().any(|r| {
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
        let ov = self.model_overrides.lock().unwrap().get(&agent.id).cloned();
        let profile_name = ov.as_ref().and_then(|o| o.profile.as_ref()).unwrap_or(&agent.model_profile);
        let profile = self.lookup_model(profile_name);
        let (o_model, o_effort) = ov.as_ref().map(|o| (o.model.clone(), o.effort.clone())).unwrap_or_default();
        let model = o_model.or_else(|| profile.as_ref().map(|p| p.model.clone()));
        let effort = o_effort.or_else(|| {
            if agent.runtime_kind == RuntimeKind::Codex {
                Some("xhigh".into())
            } else {
                profile.as_ref().and_then(profile_effort).map(str::to_string)
            }
        });
        json!({
            "agent_id": agent.id,
            "name": agent.name,
            "runtime_kind": agent.runtime_kind,
            "model_profile": profile_name,
            "provider": profile.as_ref().map(|p| p.provider.as_str()),
            "model": model,
            "effort": effort,
            "overridden": ov.is_some(),
        })
    }

    /// Feature 5 (/model): switch one member's model / reasoning effort for
    /// this session; both None clears the override (profile default). The
    /// cached runner is invalidated so the change lands on the next turn.
    /// Overrides persist per session (D-29), never written back to the TeamSpec.
    pub fn set_model_override(
        &self,
        agent_id: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Json, String> {
        let profile = if model.is_none() && effort.is_none() {
            None
        } else {
            self.model_overrides.lock().unwrap().get(agent_id).and_then(|o| o.profile.clone())
        };
        self.set_model_selection(agent_id, profile, model, effort)
    }

    /// A selected profile owns the provider endpoint, auth reference and protocol.
    pub fn set_model_selection(
        &self,
        agent_id: &str,
        profile: Option<String>,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<Json, String> {
        let agents = self.spec_agents()?;
        let agent = agents.iter().find(|a| a.id == agent_id).ok_or_else(|| format!("unknown member {agent_id}"))?;
        let selected = match &profile {
            Some(name) => Some(self.lookup_model(name).ok_or_else(|| format!("unknown model profile {name}"))?),
            None => self.lookup_model(&agent.model_profile),
        };
        if profile.is_some()
            && agent.runtime_kind == RuntimeKind::Codex
            && selected.as_ref().is_some_and(|p| !codex_compatible(p))
        {
            return Err("Codex 成员需要支持 Responses API 的 OpenAI 兼容供应商".into());
        }
        if let Some(model) = &model {
            if model.trim().is_empty() {
                return Err("model must be a non-empty string".into());
            }
        }
        let effort = effort.map(|s| s.to_ascii_lowercase());
        if let Some(effort) = &effort {
            let choices = model_efforts(selected.as_ref().map(|p| p.protocol.as_str()).unwrap_or("openai"));
            if !choices.contains(&effort.as_str()) {
                return Err(format!("effort must be one of {}", choices.join("/")));
            }
        }
        {
            let mut overrides = self.model_overrides.lock().unwrap();
            if profile.is_none() && model.is_none() && effort.is_none() {
                overrides.remove(agent_id);
            } else {
                overrides.insert(agent_id.to_string(), ModelOverride { profile, model, effort });
            }
        }
        self.persist_model_overrides()?;
        // Preserve the latest counters across the idle interval before the
        // next model runner is built, without retaining its history or tools.
        if let Some(probe) = self.usage_probes.lock().unwrap().get(agent_id) {
            probe();
        }
        self.runtime.drop_runner(agent_id);
        Ok(self.effective_model(agent))
    }

    /// `/model` with no args / worker "model": every member's effective values.
    pub fn model_report(&self) -> Json {
        let agents = self.spec_agents().unwrap_or_default();
        let merged = self.merged_models();
        let mut profiles: Vec<_> = merged.iter().collect();
        profiles.sort_by_key(|(name, p)| (&p.provider, &p.model, name.as_str()));
        json!({
            "session_id": self.session_id,
            "leader_id": self.core.state().ok().and_then(|s| s.get("leader_id").cloned()),
            "agents": agents.iter().map(|a| self.effective_model(a)).collect::<Vec<_>>(),
            "profiles": profiles.iter().map(|(name, p)| json!({
                "id": name, "provider": p.provider, "model": p.model, "protocol": p.protocol,
                "effort": profile_effort(p), "efforts": model_efforts(&p.protocol),
            })).collect::<Vec<_>>(),
        })
    }

    /// Read-only network discovery. The worker runs it outside its request loop.
    pub fn discover_models(&self, provider: &str) -> Result<Json, String> {
        let merged = self.merged_models();
        let profiles: Vec<_> = merged.iter().filter(|(_, p)| p.provider == provider).collect();
        if profiles.is_empty() {
            return Err(format!("unknown provider {provider}"));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = std::collections::HashSet::new();
        let mut models = vec![];
        let mut errors = vec![];
        for (name, profile) in profiles {
            let base = crate::chat::resolve_base_url(profile);
            if !seen.insert((base, profile.protocol.clone(), profile.api_key_env.clone())) {
                continue;
            }
            match fetch_model_ids(profile, deadline) {
                Ok(ids) => {
                    for model in ids {
                        // Keep explicitly configured models with their own options.
                        if merged.values().any(|p| {
                            p.provider == provider
                                && p.model == model
                                && p.protocol == profile.protocol
                                && p.api_key_env == profile.api_key_env
                                && crate::chat::resolve_base_url(p) == crate::chat::resolve_base_url(profile)
                        }) {
                            continue;
                        }
                        models.push(json!({"id": name, "provider": provider, "model": model,
                            "protocol": profile.protocol, "efforts": model_efforts(&profile.protocol), "discovered": true}));
                    }
                }
                Err(error) => errors.push(format!("{name}: {error}")),
            }
        }
        Ok(json!({"session_id":self.session_id, "provider":provider, "models":models, "errors":errors}))
    }

    /// Atomic rewrite of `model_overrides.json` (tmp + rename).
    fn persist_model_overrides(&self) -> Result<(), String> {
        let map = self.model_overrides.lock().unwrap();
        let path = overrides_path(&self.session_id);
        let tmp = path.with_extension("tmp");
        let text = serde_json::to_string_pretty(&*map).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("persist model overrides: {e}"))
    }

    pub fn close(&self) {
        self.runtime.close();
        let _ = self.lock.lock().unwrap().take();
    }
}

fn codex_compatible(profile: &ModelProfile) -> bool {
    // Legacy `openai` profiles also served Codex; explicit chat-only profiles do not.
    matches!(profile.protocol.as_str(), "openai" | "responses")
}

fn profile_effort(profile: &ModelProfile) -> Option<&str> {
    profile
        .generation_options
        .get("reasoning_effort")
        .and_then(Json::as_str)
        .or_else(|| profile.generation_options.get("output_config")?.get("effort")?.as_str())
}

fn model_efforts(protocol: &str) -> &'static [&'static str] {
    match protocol {
        "anthropic" => &["low", "medium", "high", "xhigh", "max"],
        "deepseek" => &["low", "medium", "high", "max"],
        _ => &["none", "minimal", "low", "medium", "high", "xhigh", "max"],
    }
}

fn fetch_model_ids(profile: &ModelProfile, deadline: std::time::Instant) -> Result<Vec<String>, String> {
    let anthropic = profile.protocol == "anthropic";
    let base = crate::chat::resolve_base_url(profile);
    let url = if anthropic { format!("{}/v1/models", base.trim_end_matches("/v1")) } else { format!("{base}/models") };
    let key = profile
        .api_key_env
        .as_ref()
        .map(|name| std::env::var(name).map_err(|_| format!("缺少环境变量 {name}")))
        .transpose()?;
    let client = ureq::AgentBuilder::new().redirects(0).build();
    let mut ids = std::collections::BTreeSet::new();
    let mut cursor = String::new();
    // ponytail: bounded catalog reads, no persistent cache; raise these limits
    // if a configured provider actually publishes more than 20 pages / 2 MB.
    for _ in 0..20 {
        let remaining = deadline.checked_duration_since(std::time::Instant::now()).ok_or("获取模型列表超时")?;
        let mut request = client.get(&url).timeout(remaining.min(std::time::Duration::from_secs(8)));
        if anthropic {
            request = request.set("anthropic-version", "2023-06-01").query("limit", "1000");
            if !cursor.is_empty() {
                request = request.query("after_id", &cursor);
            }
        }
        if let Some(key) = &key {
            request = if anthropic {
                request.set("x-api-key", key)
            } else {
                request.set("authorization", &format!("Bearer {key}"))
            };
        }
        let response = request.call().map_err(|error| match error {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            ureq::Error::Transport(t) => format!("模型目录连接失败：{:?}", t.kind()),
        })?;
        let data: Json = serde_json::from_reader(std::io::Read::take(response.into_reader(), 2_000_000))
            .map_err(|_| "模型目录不是有效 JSON，或响应超过 2 MB")?;
        let rows = data["data"].as_array().ok_or("模型目录缺少 data 数组")?;
        for row in rows {
            let id = row["id"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
                .ok_or("模型目录包含无效模型 ID")?;
            ids.insert(id.to_string());
        }
        if !anthropic || data["has_more"] != true {
            return Ok(ids.into_iter().collect());
        }
        let next = data["last_id"].as_str().filter(|s| !s.is_empty() && *s != cursor).ok_or("模型目录分页游标无效")?;
        cursor = next.into();
    }
    Err("模型目录超过 20 页".into())
}

/// The member's skills directories and instruction (memory) files, with
/// their contents read for prompt injection.
///
/// ponytail: bounded contents go straight into the system prompt instead of
/// a virtual filesystem the member reads with file tools (upgrade path:
/// a read_skill tool once skills outgrow the prompt).
fn member_context(
    catalog: &UserConfig,
    cwd: &std::path::Path,
    session_id: &str,
    agent: &AgentSpec,
) -> Vec<(String, String)> {
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
            if !agent.skills.contains(&name) || selected.iter().any(|(n, _)| *n == name) {
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
    for candidate in
        [cwd.join("AGENTS.md"), user_config_path().parent().map(|p| p.join("AGENTS.md")).unwrap_or_default()]
    {
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
        let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "instructions".into());
        out.push((format!("instructions {label}"), text));
    }
    out
}

/// `sessions/<id>/members/<agent>/` — the member's private directory.
fn member_dir(session_id: &str, agent_id: &str) -> PathBuf {
    let dir = session_paths(session_id).base.join("members").join(agent_id);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Workspace policy decides one member's working directory — and therefore
/// what its file tools and its backend can reach (plan §8/P5).
fn member_root(agent: &AgentSpec, cwd: &std::path::Path, session_id: &str) -> Result<PathBuf, String> {
    let member = member_dir(session_id, &agent.id);
    let workspace = crate::workspace::prepare(agent, cwd, &member)?;
    if workspace.policy == teamagents_core::models::WorkspacePolicy::Shared {
        validate_shared_workspace(&workspace.path)?;
    }
    if let Some(note) = &workspace.note {
        eprintln!("member {}: {}", agent.id, note);
    }
    if let Err(error) = crate::review::register(&session_paths(session_id).base, &member, &workspace.path) {
        eprintln!("成员 {} 的工作区审查基线未建立：{error}", agent.id);
    }
    Ok(workspace.path)
}

/// A workspace bind must never reintroduce the runtime's hidden state. Managed
/// isolated/worktree roots are confined to their own `members/<id>/work` subtree;
/// shared roots (including a git-worktree fallback) have no such boundary.
fn validate_shared_workspace(root: &std::path::Path) -> Result<(), String> {
    fn resolved(path: &std::path::Path) -> Result<PathBuf, String> {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir().map_err(|e| e.to_string())?.join(path)
        };
        // A config directory can be absent at session open. Resolve its deepest
        // existing ancestor so symlinked XDG homes cannot evade the comparison.
        for ancestor in path.ancestors() {
            match std::fs::canonicalize(ancestor) {
                Ok(mut base) => {
                    for part in path.strip_prefix(ancestor).map_err(|e| e.to_string())?.components() {
                        match part {
                            std::path::Component::ParentDir => {
                                base.pop();
                            }
                            std::path::Component::Normal(name) => base.push(name),
                            _ => {}
                        }
                    }
                    return Ok(base);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if std::fs::symlink_metadata(ancestor).is_ok() {
                        return Err(format!("无法核对目录 {}：{error}", path.display()));
                    }
                }
                Err(error) => return Err(format!("无法核对目录 {}：{error}", path.display())),
            }
        }
        Err(format!("无法核对目录 {}", path.display()))
    }

    let root = resolved(root)?;
    let codex_home = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| expand_home("~/.codex"));
    let config = user_config_path();
    for private in [crate::config::state_dir(), config.parent().unwrap().to_path_buf(), codex_home] {
        let private = resolved(&private)?;
        if root.starts_with(&private) || private.starts_with(&root) {
            return Err(format!(
                "工作目录与私有运行数据重叠：{} 与 {}。请将项目目录与 TeamAgents/Codex 配置及状态目录分开。",
                root.display(),
                private.display()
            ));
        }
    }
    Ok(())
}

pub fn open_session(opts: OpenOptions) -> Result<Arc<OpenedSession>, String> {
    // whitelist check first: a bad id must not create directories outside
    // the sessions root before the lock's own validation can refuse it
    if let Some(id) = &opts.session_id {
        crate::sessions::validate_session_id(id)?;
    }
    let cwd = match &opts.cwd {
        Some(cwd) => std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone()),
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let catalog = match opts.catalog {
        Some(catalog) => catalog,
        None => load_user_config_for(&cwd)?,
    };
    // retention (user config [retention] archived_days): sweeps archived sessions
    // through the guarded delete path on every open.
    // ponytail: once per open, no background scheduler — a long-lived serve
    // process prunes when it opens sessions, which is often enough.
    let retention_days = catalog.retention.archived_days;
    if retention_days > 0 {
        let _ = crate::sessions::prune_archived(retention_days, None, false);
    }
    let config_full_auto = crate::config::permission_mode_from_config()? == "full_auto";
    let full_auto = opts.full_auto || config_full_auto;
    let session_id = opts.session_id.clone().unwrap_or_else(|| new_session_id(&cwd));
    let paths = session_paths(&session_id);
    std::fs::create_dir_all(&paths.artifacts).map_err(|e| e.to_string())?;
    // dropped on every early return below, so a failed open never keeps the lock
    let lock = acquire_session_lock(&session_id)?;
    // history retention runs after the lock: this session is ours, and the lock is
    // what stops another process from pruning the same database underneath us
    let history_days = catalog.retention.history_days;
    if history_days > 0 {
        let _ = crate::sessions::prune_session_history(&session_id, history_days, false);
    }
    // resuming an archived id must fail loudly, not create an empty session
    // over it: the next archive of that fresh session would remove_dir_all the
    // original archived data. Checked under the lock: a concurrent archive
    // cannot move the directory between this check and the open below.
    let archived = crate::config::sessions_dir().join("archived").join(&session_id);
    if !paths.db.exists() && archived.exists() {
        // remove the ghost this open just created, but only the items it
        // created: with no team.db the active dir normally holds just our
        // artifacts/ + session.lock, and leaving them would let a later
        // archive_session overwrite the real archive with the ghost. If the
        // dir still holds anything else (e.g. member worktrees after team.db
        // was deleted by hand) rmdir fails and the content is left untouched
        // instead of being wiped by remove_dir_all (round 5, F3).
        drop(lock);
        let _ = std::fs::remove_dir_all(&paths.artifacts);
        let _ = std::fs::remove_file(&paths.lock);
        let _ = std::fs::remove_dir(&paths.base);
        return Err(format!(
            "session {session_id} is archived at {}; move it back to the sessions root to resume",
            archived.display()
        ));
    }
    let db = paths.db.to_string_lossy().into_owned();
    let core = CoreClient::open(&db, &session_id)?;

    let result = (|| -> Result<Arc<OpenedSession>, String> {
        let metadata = core.call("session_metadata", json!({"session_id": session_id}))?;
        let exists = !metadata["session"].is_null();
        let has_spec = metadata["has_spec"].as_bool().ok_or("会话元数据缺少配置存在状态")?;
        if !exists {
            core.call(
                "create_session",
                json!({
                    "session_id": session_id,
                    "cwd": cwd.to_string_lossy(),
                    "permissions_mode": if full_auto { "full_auto" } else { "approved_scope" },
                }),
            )?;
        }
        if !has_spec {
            // An explicit spec can repair an initial save that failed before
            // writing any revision. Existing revisions must never be replaced
            // merely because loading or validating them failed.
            let initial_spec = opts
                .initial_spec
                .clone()
                .or_else(|| (!exists).then(|| default_leader_spec("leader_main", &["files", "shell", "web"])));
            if let Some(spec) = initial_spec {
                core.call("save_spec", json!({"session_id": session_id, "spec": spec}))?;
            }
        }
        // Validate persisted work before changing permission mode or creating
        // any member runtime, including when the caller supplies a new spec.
        let state = core.state()?;
        let enable_full_auto =
            exists && full_auto && state["session"]["permissions_mode"].as_str() != Some("full_auto");

        // keep validation (topology patches, member profiles) in sync with the
        // user config the engine loaded, plus this session's auto-created
        // member profiles (D-30)
        let session_profiles: Arc<Mutex<HashMap<String, ModelProfile>>> =
            Arc::new(Mutex::new(load_session_profiles(&session_id)));
        {
            let mut merged = catalog.clone();
            merged.models.extend(session_profiles.lock().unwrap().clone());
            core.call("set_catalog", json!({"session_id": session_id, "catalog": merged}))?;
        }
        let state = core.state()?;
        let mode = state
            .get("session")
            .and_then(|s| s.get("permissions_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("approved_scope");
        let policy = PermissionPolicy { mode: mode.to_string(), ..Default::default() };
        let approvals = ApprovalGate::new(core.clone(), policy);
        let agents: Vec<AgentSpec> =
            serde_json::from_value(state.get("spec").and_then(|s| s.get("agents")).cloned().unwrap_or(Json::Null))
                .map_err(|e| format!("bad team spec: {e}"))?;

        let barriers: BarrierRegistry = Arc::new(Mutex::new(HashMap::new()));
        let notify = Notify::new(core.clone());
        let usage_probes: UsageProbes = Arc::new(Mutex::new(HashMap::new()));
        let mut effective_catalog = catalog.clone();
        effective_catalog.models.extend(session_profiles.lock().unwrap().clone());
        let model_overrides: ModelOverrides =
            Arc::new(Mutex::new(load_model_overrides(&session_id, &effective_catalog, &agents)));
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
            session_profiles.clone(),
        );

        let mut runners: HashMap<String, Arc<dyn AgentRunner>> = HashMap::new();
        for agent in &agents {
            let runner = make_runner(agent)?;
            runners.insert(agent.id.clone(), runner);
        }
        let executor = member_executor_factory(core.clone(), catalog.clone(), session_id.clone(), cwd.clone());
        let limits = RuntimeLimits {
            turn_active_timeout_s: state
                .get("limits")
                .and_then(|l| l.get("turn_active_timeout_s"))
                .and_then(|v| v.as_i64())
                .unwrap_or(1200),
            cancel_confirm_timeout_s: state
                .get("limits")
                .and_then(|l| l.get("cancel_confirm_timeout_s"))
                .and_then(|v| v.as_i64())
                .unwrap_or(60),
            max_model_steps_per_turn: state
                .get("limits")
                .and_then(|l| l.get("max_model_steps_per_turn"))
                .and_then(|v| v.as_i64())
                .unwrap_or(200),
            max_parallel_workers: state
                .get("limits")
                .and_then(|l| l.get("max_parallel_workers"))
                .and_then(|v| v.as_i64())
                .unwrap_or(8),
        };
        // hooks first: the notify sink must exist before the runtime starts, and
        // the gateway needs the same object for its pre_tool policy
        let hooks = crate::hooks::Hooks::from_config(&catalog, &session_id);
        if let Some(hooks) = hooks.clone() {
            notify.set_event_sink(Box::new(move |event, payload| hooks.fire(event, payload.clone())));
        }
        let runtime = Runtime::new(core.clone(), notify, approvals, executor, Some(make_runner), limits);
        if let Some(hooks) = hooks {
            runtime.set_hooks(hooks);
        }
        runtime.set_topology_prepare(topology_prepare_hook(
            core.clone(),
            session_id.clone(),
            catalog.clone(),
            session_profiles.clone(),
            model_overrides.clone(),
        ));
        for (id, runner) in runners {
            runtime.add_runner(&id, runner);
        }
        if enable_full_auto {
            let prepare_mode = || -> Result<(), String> {
                // Mode changes schedule work too. Protect old queued
                // checkpoints before this first post-restart team transaction.
                runtime.prepare_reconciliation()?;
                let receipt = core.submit(&TeamAction {
                    action_id: teamagents_core::models::new_id("mode"),
                    session_id: session_id.clone(),
                    actor_id: "user".into(),
                    run_id: None,
                    kind: teamagents_core::models::ActionKind::SetPermissionMode,
                    payload: json!({"mode": "full_auto"}),
                })?;
                if receipt.ok {
                    Ok(())
                } else {
                    Err(receipt.error.unwrap_or_else(|| "cannot enable full auto".into()))
                }
            };
            if let Err(error) = prepare_mode() {
                runtime.close();
                return Err(error);
            }
        }
        Ok(Arc::new(OpenedSession {
            runtime,
            core,
            session_id,
            cwd,
            catalog,
            session_profiles,
            usage_probes,
            model_overrides,
            lock: Mutex::new(Some(lock)),
        }))
    })();
    result
}

/// D-30/D-33 hook: before an apply_topology_patch submit, every add_agent gets
/// (1) the Leader's tool bindings when it names none — a member without tools
/// can only message and wait, so it can never do the work it is given;
/// (2) message channels in both directions with the Leader, so delegation and
/// reporting actually have a way to travel;
/// (3) a member-named session profile cloned from the Leader's effective model
/// config when it names none (D-30; a non-empty unknown value is treated as a
/// requested model id on the Leader's connection).
/// Runs on the caller's thread.
fn topology_prepare_hook(
    core: Arc<CoreClient>,
    session_id: String,
    catalog: UserConfig,
    session_profiles: Arc<Mutex<HashMap<String, ModelProfile>>>,
    model_overrides: ModelOverrides,
) -> crate::gateway::TopologyPrepare {
    Arc::new(move |payload: &mut Json| {
        let Some(ops) = payload.get_mut("operations").and_then(|v| v.as_array_mut()) else { return Ok(()) };
        let mut changed = false;
        let mut team: Option<(String, Vec<String>, String, Vec<ChannelSpec>)> = None;
        // A failure partway through preparation must not publish its prefix.
        // Persist the complete candidate before installing its in-memory view.
        let mut profiles = session_profiles.lock().unwrap().clone();
        for op in ops.iter_mut().filter(|o| o.get("op").and_then(|v| v.as_str()) == Some("add_agent")) {
            let aid = op
                .get("agent")
                .and_then(|a| a.get("id"))
                .and_then(|v| v.as_str())
                .ok_or("add_agent: missing agent.id")?
                .to_string();
            let (leader_id, leader_tools, leader_profile, channels) = match &team {
                Some(known) => known.clone(),
                None => {
                    let state = core.state().map_err(|e| format!("add_agent: {e}"))?;
                    let leader_id = state.get("leader_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let agents: Vec<AgentSpec> = serde_json::from_value(
                        state.get("spec").and_then(|s| s.get("agents")).cloned().unwrap_or(Json::Null),
                    )
                    .map_err(|e| format!("add_agent: bad spec: {e}"))?;
                    let leader = agents.iter().find(|a| a.id == leader_id).ok_or("add_agent: no leader in spec")?;
                    let channels: Vec<ChannelSpec> = serde_json::from_value(
                        state.get("spec").and_then(|s| s.get("channels")).cloned().unwrap_or(Json::Null),
                    )
                    .unwrap_or_default();
                    let known = (leader_id, leader.tool_bindings.clone(), leader.model_profile.clone(), channels);
                    team = Some(known.clone());
                    known
                }
            };
            // D-33 (1): a member without bindings inherits the Leader's, exactly
            // like model_profile inheritance: it grants nothing the Leader could
            // not already grant explicitly.
            // an omitted list inherits; an explicit list (even []) is respected,
            // so "messaging-only member" stays expressible
            let inherit = op.get("agent").and_then(|a| a.get("tool_bindings")).is_none();
            if inherit {
                if let Some(agent) = op.get_mut("agent").and_then(|v| v.as_object_mut()) {
                    agent.insert("tool_bindings".into(), json!(leader_tools));
                }
            }
            // D-33 (2): a conversation channel both ways with the Leader. Members
            // talk to each other through shared spaces only, so every directed
            // pair the runtime needs exists here.
            let mut added: Vec<Json> = vec![];
            for (source, target) in [(&leader_id, &aid), (&aid, &leader_id)] {
                let covered = channels.iter().any(|c| {
                    c.source == *source && c.mode == ChannelMode::Message && c.targets.iter().any(|t| t == target)
                });
                if !covered {
                    added.push(json!({"source": source, "targets": [target], "mode": "message"}));
                }
            }
            if !added.is_empty() {
                let slot = op.as_object_mut().expect("op is an object").entry("channels").or_insert_with(|| json!([]));
                if let Some(list) = slot.as_array_mut() {
                    list.extend(added);
                }
            }
            let agent = op.get_mut("agent").and_then(|v| v.as_object_mut()).ok_or("add_agent: missing agent")?;
            let requested = match agent.get("model_profile") {
                None => "",
                Some(value) => value.as_str().ok_or("add_agent.model_profile 必须是字符串；省略时继承 Leader 模型")?,
            }
            .to_string();
            if !requested.is_empty() && (catalog.models.contains_key(&requested) || profiles.contains_key(&requested)) {
                continue;
            }
            if requested.is_empty() && profiles.contains_key(&aid) {
                agent.insert("model_profile".into(), json!(aid));
                continue;
            }
            if catalog.models.contains_key(&aid) {
                return Err(format!(
                    "成员 ID {aid:?} 与已有模型配置同名，不能覆盖该配置；请更换成员 ID 或显式引用已有 model_profile"
                ));
            }
            if profiles.contains_key(&aid) {
                // A rejected earlier patch may have left an unused D-30 profile.
                // Allow correction of its model ID, but never change a profile
                // that an existing member (including a /model override) uses.
                let state = core.state_brief()?;
                let referenced = state["spec"]["agents"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|member| member["model_profile"] == aid)
                    || model_overrides.lock().unwrap().values().any(|ov| ov.profile.as_deref() == Some(&aid));
                if referenced {
                    return Err(format!("模型配置 {aid:?} 已被成员使用；请更换新成员 ID 或显式引用已有 model_profile"));
                }
            }
            let ov = model_overrides.lock().unwrap().get(&leader_id).cloned().unwrap_or_default();
            let base_name = ov.profile.clone().unwrap_or(leader_profile);
            let mut base = profiles
                .get(&base_name)
                .cloned()
                .or_else(|| catalog.models.get(&base_name).cloned())
                .ok_or_else(|| format!("add_agent: leader model profile {base_name} not found"))?;
            if let Some(model) = ov.model {
                base.model = model;
            }
            if let Some(effort) = ov.effort {
                base.generation_options.insert("reasoning_effort".into(), json!(effort));
            }
            if !requested.is_empty() {
                base.model = requested;
            }
            profiles.insert(aid.clone(), base);
            agent.insert("model_profile".into(), json!(aid));
            changed = true;
        }
        if changed {
            save_session_profiles(&session_id, &profiles)?;
            let mut merged = catalog.clone();
            merged.models.extend(profiles.clone());
            // ponytail: push-then-submit is not atomic with the patch; control
            // re-validates on submit, so a lost race fails the patch loudly and
            // the Leader can retry. Serial leader applies make this theoretical.
            core.call("set_catalog", json!({"session_id": session_id, "catalog": merged}))?;
            *session_profiles.lock().unwrap() = profiles;
        }
        Ok(())
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "Session bootstrap supplies the shared services once at this composition boundary."
)]
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
    session_profiles: Arc<Mutex<HashMap<String, ModelProfile>>>,
) -> RunnerFactory {
    Box::new(move |agent: &AgentSpec| {
        if let Some(scripts) = &scripts {
            let steps = scripts.get(&agent.id).cloned().unwrap_or_else(|| vec![Step::End]);
            return Ok(ScriptedMember::new(&agent.id, steps, barriers.clone()));
        }
        // feature 5 (/model): a session-level override wins over the profile
        let ov = model_overrides.lock().unwrap().get(&agent.id).cloned().unwrap_or_default();
        let profile_name = ov.profile.as_ref().unwrap_or(&agent.model_profile);
        // D-30: member-named session profiles shadow user-config profiles
        let profile: Option<ModelProfile> = session_profiles
            .lock()
            .unwrap()
            .get(profile_name)
            .cloned()
            .or_else(|| catalog.models.get(profile_name).cloned());
        // Keep the same bounded, symlink-safe Skills/instruction selection for
        // both member backends. Codex receives the selected text through its
        // developerInstructions field; it must not discover arbitrary host
        // files through its own process environment.
        let context = member_context(&catalog, &cwd, &session_id, agent);
        if agent.runtime_kind == RuntimeKind::Codex {
            let instructions = codex_member_instructions(&agent.instructions, &context);
            let opts = codex_options_with_instructions(agent, profile.as_ref(), &ov, &session_id, &cwd, instructions)?;
            let runner = CodexRunner::new(opts, core.clone(), approvals.clone(), notify.clone());
            usage_probes.lock().unwrap().insert(agent.id.clone(), usage_probe(&runner, CodexRunner::usage_snapshot));
            return Ok(runner);
        }
        let Some(profile) = profile else {
            return Err(format!("unknown model profile {}", agent.model_profile));
        };
        let profile = apply_model_override(profile, &ov);
        let agent_json = serde_json::to_value(agent).map_err(|e| e.to_string())?;
        let root = member_root(agent, &cwd, &session_id)?;
        let bound = crate::bound::BoundTools::load_in(&catalog, &agent.tool_bindings, &root)?;
        // a required web service that cannot load fails the member, not the call;
        // the resolved set is also what the model gets advertised, so an explicit
        // binding name (anything but the literal "web") still exposes the tools
        let web = crate::tools::web_tools(&catalog, &agent.tool_bindings)?;
        let runner = ChatRunner::new(
            &agent_json,
            profile,
            Some(root.to_string_lossy().into_owned()),
            notify.clone(),
            bound,
            context,
            (web.search.is_some(), web.fetch.is_some()),
        );
        usage_probes.lock().unwrap().insert(agent.id.clone(), usage_probe(&runner, ChatRunner::usage_snapshot));
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
fn codex_member_instructions(base: &str, context: &[(String, String)]) -> String {
    let mut instructions = base.to_string();
    for (label, content) in context {
        if !instructions.trim().is_empty() {
            instructions.push_str("\n\n");
        }
        instructions.push_str("<member_context source=\"");
        instructions.push_str(label);
        instructions.push_str("\">\n");
        instructions.push_str(content);
        instructions.push_str("\n</member_context>");
    }
    instructions
}

/// Flatten `$CODEX_HOME/<name>.config.toml` into `-c key=value` overrides.
/// Nested tables become dotted keys (`model_providers.deepseek.base_url`), which
/// is exactly how the CLI spells them.
fn codex_profile_overrides(name: &str) -> Result<Vec<(String, Json)>, String> {
    let home = std::env::var("CODEX_HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::config::expand_home("~/.codex"));
    let path = home.join(format!("{name}.config.toml"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("codex profile {name:?}: cannot read {}: {e}", path.display()))?;
    let parsed: toml::Value = toml::from_str(&text).map_err(|e| format!("codex profile {name:?}: bad toml: {e}"))?;
    let value = serde_json::to_value(parsed).map_err(|e| format!("codex profile {name:?}: {e}"))?;
    let mut out = vec![];
    flatten_json("", &value, &mut out);
    if out.is_empty() {
        return Err(format!("codex profile {name:?} is empty: {}", path.display()));
    }
    Ok(out)
}

fn flatten_json(prefix: &str, value: &Json, out: &mut Vec<(String, Json)>) {
    match value {
        Json::Object(map) => {
            for (key, child) in map {
                let key = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
                flatten_json(&key, child, out);
            }
        }
        other if !prefix.is_empty() => out.push((prefix.to_string(), other.clone())),
        _ => {}
    }
}

#[cfg(test)]
fn codex_options(
    agent: &AgentSpec,
    profile: Option<&ModelProfile>,
    ov: &ModelOverride,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<CodexOptions, String> {
    codex_options_with_instructions(agent, profile, ov, session_id, cwd, agent.instructions.clone())
}

fn codex_options_with_instructions(
    agent: &AgentSpec,
    profile: Option<&ModelProfile>,
    ov: &ModelOverride,
    session_id: &str,
    cwd: &std::path::Path,
    instructions: String,
) -> Result<CodexOptions, String> {
    let mut config: Vec<(String, Json)> = vec![];
    let mut model = None;
    // A Codex config profile (`$CODEX_HOME/<name>.config.toml`) owns
    // provider/model/credentials, so it is layered instead of the TeamAgents
    // profile: `codex_profile = "deepseek"` runs the member on DeepSeek rather
    // than the official subscription. The installed CLI refuses `--profile` for
    // `app-server`, so the file is read here and passed as `-c` overrides.
    if let Some(name) = profile.and_then(|p| p.codex_profile.clone()).filter(|name| !name.is_empty()) {
        let mut config = codex_profile_overrides(&name)?;
        // an explicit per-member model/effort override still wins
        if let Some(model) = &ov.model {
            config.retain(|(key, _)| key != "model");
            config.push(("model".into(), json!(model)));
        }
        return Ok(CodexOptions {
            agent_id: agent.id.clone(),
            session_id: session_id.into(),
            instructions,
            workdir: member_root(agent, cwd, session_id)?,
            sandbox: "workspace-write".into(),
            approval_policy: "on-request".into(),
            effort: ov.effort.clone(),
            model: None,
            codex_bin: None,
            codex_home: None,
            env: vec![],
            config_overrides: config,
        });
    }
    if let Some(profile) = profile {
        model = Some(profile.model.clone());
        if !profile.provider.is_empty() {
            config.push(("model_provider".into(), json!(profile.provider)));
        }
        if ov.profile.is_some() && profile.base_url.is_some() {
            // A private provider id avoids inheriting built-in OpenAI auth flags.
            config.retain(|(key, _)| key != "model_provider");
            config.push(("model_provider".into(), json!("teamagents_session")));
            for (key, value) in [
                ("name", json!(profile.provider)),
                ("base_url", json!(profile.base_url)),
                ("wire_api", json!("responses")),
            ] {
                config.push((format!("model_providers.teamagents_session.{key}"), value));
            }
            if let Some(env) = &profile.api_key_env {
                config.push(("model_providers.teamagents_session.env_key".into(), json!(env)));
            }
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
        instructions,
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
            if cached_revision == revision {
                return executor(tool, args, control);
            }
        }
        let state = core.call_in_session("state", json!({"include_events": false}))?;
        let agent = state
            .get("spec")
            .and_then(|spec| spec.get("agents"))
            .cloned()
            .and_then(|agents| serde_json::from_value::<Vec<AgentSpec>>(agents).ok())
            .and_then(|agents| agents.into_iter().find(|a| a.id == agent_id))
            .ok_or("member is no longer configured")?;
        let member = member_dir(&session_id, &agent.id);
        let executor: MemberExecutor = Arc::new(crate::tools::member_executor_with_control(
            member_root(&agent, &cwd, &session_id)?,
            catalog.clone(),
            agent.tool_bindings.clone(),
            crate::tools::ArtifactPaths::for_member(artifacts.clone(), &member),
            // per-member shell continuity (`cd`, exports) survives the sandbox
            Some(member.join("shell")),
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
            id: "leader".into(),
            name: "Leader".into(),
            role: "leader".into(),
            runtime_kind: RuntimeKind::Deepagents,
            instructions: String::new(),
            model_profile: String::new(),
            tool_bindings: vec![],
            skills: vec!["review".into()],
            workspace_policy: teamagents_core::models::WorkspacePolicy::Shared,
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

        let catalog =
            UserConfig { skills_paths: vec![registry.to_string_lossy().into_owned()], ..UserConfig::default() };
        let agent = AgentSpec {
            id: "m".into(),
            name: "M".into(),
            role: "worker".into(),
            runtime_kind: RuntimeKind::Deepagents,
            instructions: String::new(),
            model_profile: String::new(),
            tool_bindings: vec![],
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
    fn codex_member_instructions_keep_member_rules_before_selected_context() {
        let context = vec![
            ("skill review".to_string(), "review skill body".to_string()),
            ("instructions AGENTS.md".to_string(), "project instructions".to_string()),
        ];
        let instructions = codex_member_instructions("Review parser edge cases.", &context);
        assert!(instructions.starts_with("Review parser edge cases."));
        assert!(instructions.contains("<member_context source=\"skill review\">\nreview skill body\n</member_context>"));
        assert!(instructions
            .contains("<member_context source=\"instructions AGENTS.md\">\nproject instructions\n</member_context>"));
        assert!(
            instructions.find("Review parser edge cases.").unwrap() < instructions.find("review skill body").unwrap()
        );
        assert!(instructions.find("review skill body").unwrap() < instructions.find("project instructions").unwrap());
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
            provider: "openai".into(),
            protocol: "openai".into(),
            model: "gpt-default".into(),
            base_url: None,
            api_key_env: None,
            timeout: 120,
            max_retries: 5,
            generation_options: HashMap::from([("reasoning_effort".to_string(), json!("medium"))]),
            context_window: None,
            codex_profile: None,
        };
        let ov = ModelOverride { model: Some("gpt-5".into()), effort: Some("high".into()), ..Default::default() };
        let rewritten = apply_model_override(profile.clone(), &ov);
        assert_eq!(rewritten.model, "gpt-5");
        assert_eq!(rewritten.generation_options["reasoning_effort"], json!("high"));
        let untouched = apply_model_override(profile.clone(), &ModelOverride::default());
        assert_eq!(untouched.model, "gpt-default");
        assert_eq!(untouched.generation_options["reasoning_effort"], json!("medium"));

        // codex member: the override lands in opts.model / opts.effort
        let agent = AgentSpec {
            id: "cod".into(),
            name: "Cod".into(),
            role: "dev".into(),
            runtime_kind: RuntimeKind::Codex,
            instructions: "Review parser edge cases.".into(),
            model_profile: "m".into(),
            tool_bindings: vec![],
            skills: vec![],
            workspace_policy: teamagents_core::models::WorkspacePolicy::Shared,
        };
        let opts = codex_options(&agent, Some(&profile), &ov, "s-ov", &root.join("project")).unwrap();
        assert_eq!(opts.model.as_deref(), Some("gpt-5"));
        assert_eq!(opts.instructions, "Review parser edge cases.");
        assert_eq!(opts.effort.as_deref(), Some("high"));
        assert!(opts.config_overrides.iter().any(|(k, v)| k == "model_provider" && v == &json!("openai")));
        // profile generation_options still pass through as config overrides
        assert!(opts.config_overrides.iter().any(|(k, v)| k == "reasoning_effort" && v == &json!("medium")));
        // model-only override keeps the runner's default effort
        let opts = codex_options(
            &agent,
            Some(&profile),
            &ModelOverride { model: Some("gpt-5-codex".into()), ..Default::default() },
            "s-ov",
            &root.join("project"),
        )
        .unwrap();
        assert_eq!(opts.model.as_deref(), Some("gpt-5-codex"));
        assert_eq!(opts.effort.as_deref(), Some("xhigh"));
        let custom = ModelProfile {
            base_url: Some("http://127.0.0.1:1234/v1".into()),
            api_key_env: Some("TEST_MODEL_KEY".into()),
            ..profile
        };
        let opts = codex_options(
            &agent,
            Some(&custom),
            &ModelOverride { profile: Some("custom".into()), ..Default::default() },
            "s-ov",
            &root.join("project"),
        )
        .unwrap();
        assert!(opts.config_overrides.contains(&("model_provider".into(), json!("teamagents_session"))));
        assert!(opts
            .config_overrides
            .contains(&("model_providers.teamagents_session.base_url".into(), json!("http://127.0.0.1:1234/v1"))));
        assert!(opts
            .config_overrides
            .contains(&("model_providers.teamagents_session.env_key".into(), json!("TEST_MODEL_KEY"))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn model_discovery_uses_auth_pagination_and_keeps_configured_models_on_failure() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let _env = crate::env_lock();
        std::env::set_var("TA_DISCOVERY_TEST_KEY", "fake-discovery-key");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for step in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request).unwrap().to_lowercase();
                assert!(request.starts_with("get /v1/models"));
                if step == 0 {
                    assert!(request.contains("authorization: bearer fake-discovery-key"));
                } else {
                    assert!(
                        request.contains("x-api-key: fake-discovery-key")
                            && request.contains("anthropic-version: 2023-06-01")
                    );
                }
                let data = match step {
                    0 => json!({"data":[{"id":"configured"},{"id":"remote"},{"id":"remote"}]}),
                    1 => json!({"data":[{"id":"claude-a"}],"has_more":true,"last_id":"claude-a"}),
                    2 => {
                        assert!(request.contains("after_id=claude-a"));
                        json!({"data":[{"id":"claude-b"}],"has_more":false})
                    }
                    _ => json!({"error":"fake-discovery-key must not be echoed in UI errors"}),
                }
                .to_string();
                let status = if step == 3 { "403 Forbidden" } else { "200 OK" };
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}", data.len()).unwrap();
            }
        });
        // Use open_session's existing scripted mode: discovery never invokes a model.
        let root = std::env::temp_dir().join(format!("ta-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("XDG_STATE_HOME", &root);
        let profile = json!({"provider":"local","protocol":"openai","model":"configured","base_url":format!("{base}/v1"),"api_key_env":"TA_DISCOVERY_TEST_KEY"});
        let catalog: UserConfig = serde_json::from_value(json!({"models":{"a":profile,"duplicate":profile,
            "claude":{"provider":"anthropic","protocol":"anthropic","model":"configured-claude","base_url":base,"api_key_env":"TA_DISCOVERY_TEST_KEY"}}})).unwrap();
        let opened = open_session(OpenOptions {
            cwd: Some(root.clone()),
            session_id: Some("discovery".into()),
            catalog: Some(catalog),
            initial_spec: Some(default_leader_spec("a", &[])),
            scripts: Some(HashMap::new()),
            ..Default::default()
        })
        .unwrap();
        let openai = opened.discover_models("local").unwrap();
        assert_eq!(
            openai["models"].as_array().unwrap().len(),
            1,
            "deduplicate endpoint, configured IDs and response IDs"
        );
        assert_eq!(openai["models"][0]["id"], "a");
        assert_eq!(openai["models"][0]["model"], "remote");
        let anthropic = opened.discover_models("anthropic").unwrap();
        assert_eq!(anthropic["models"].as_array().unwrap().len(), 2);
        let failed = opened.discover_models("anthropic").unwrap();
        assert!(failed["models"].as_array().unwrap().is_empty());
        assert!(failed["errors"].to_string().contains("HTTP 403"));
        assert!(!failed.to_string().contains("fake-discovery-key"));
        assert!(opened.model_report()["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["model"] == "configured-claude"));
        assert!(opened.discover_models("missing").is_err());
        opened.close();
        server.join().unwrap();
        std::env::remove_var("TA_DISCOVERY_TEST_KEY");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resume_of_an_archived_session_is_refused_and_the_archive_survives() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-arch-resume-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", root.join("home/.config"));
        std::env::set_var("XDG_STATE_HOME", root.join("home/.state"));

        let opened = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("s-arch".into()),
            catalog: Some(UserConfig::default()),
            initial_spec: Some(default_leader_spec("a", &[])),
            scripts: Some(HashMap::new()),
            ..Default::default()
        })
        .unwrap();
        opened.close();
        crate::sessions::archive_session("s-arch", None).expect("archive");

        let err = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("s-arch".into()),
            catalog: Some(UserConfig::default()),
            ..Default::default()
        })
        .err()
        .expect("archived resume must fail");
        assert!(err.contains("archived"), "{err}");
        assert!(err.contains("move it back to the sessions root"), "{err}");
        assert!(crate::config::sessions_dir().join("archived/s-arch/team.db").exists(), "archive destroyed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Probe chain from review round 4 (F1): archive -> refused resume must
    /// not leave a ghost dir; a second archive must neither succeed on the
    /// ghost nor destroy the real archive (sessions/<id>/team.db survives).
    #[test]
    fn refused_archived_resume_leaves_no_ghost_and_rearchive_keeps_the_archive() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-arch-ghost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", root.join("home/.config"));
        std::env::set_var("XDG_STATE_HOME", root.join("home/.state"));

        let opened = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("s-ghost".into()),
            catalog: Some(UserConfig::default()),
            initial_spec: Some(default_leader_spec("a", &[])),
            scripts: Some(HashMap::new()),
            ..Default::default()
        })
        .unwrap();
        opened.close();
        crate::sessions::archive_session("s-ghost", None).expect("archive");

        let err = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("s-ghost".into()),
            catalog: Some(UserConfig::default()),
            ..Default::default()
        })
        .err()
        .expect("archived resume must fail");
        assert!(err.contains("archived"), "{err}");
        let active = crate::config::sessions_dir().join("s-ghost");
        assert!(!active.exists(), "refused resume left a ghost dir at {active:?}");

        // even with a ghost forced back (the pre-fix state), archiving must
        // refuse a source without team.db and keep the real archive
        std::fs::create_dir_all(active.join("artifacts")).unwrap();
        let err = crate::sessions::archive_session("s-ghost", None).expect_err("ghost must not archive");
        assert!(err.contains("does not exist"), "{err}");
        assert!(crate::config::sessions_dir().join("archived/s-ghost/team.db").exists(), "archive destroyed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Round 5 (F3): the refused-resume cleanup deletes only its own ghost
    /// items; a base dir that still holds member worktrees (team.db removed
    /// by hand) is left in place instead of being wiped by remove_dir_all.
    #[test]
    fn refused_archived_resume_preserves_leftover_member_worktrees() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-arch-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cwd = root.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", root.join("home/.config"));
        std::env::set_var("XDG_STATE_HOME", root.join("home/.state"));

        let opened = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("s-keep".into()),
            catalog: Some(UserConfig::default()),
            initial_spec: Some(default_leader_spec("a", &[])),
            scripts: Some(HashMap::new()),
            ..Default::default()
        })
        .unwrap();
        opened.close();
        crate::sessions::archive_session("s-keep", None).expect("archive");

        // team.db deleted by hand, a member worktree left behind
        let active = crate::config::sessions_dir().join("s-keep");
        std::fs::create_dir_all(active.join("members/m1")).unwrap();
        std::fs::write(active.join("members/m1/work.txt"), b"wip").unwrap();

        let err = open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("s-keep".into()),
            catalog: Some(UserConfig::default()),
            ..Default::default()
        })
        .err()
        .expect("archived resume must fail");
        assert!(err.contains("archived"), "{err}");
        assert_eq!(std::fs::read(active.join("members/m1/work.txt")).unwrap(), b"wip", "member worktree preserved");
        assert!(!active.join("artifacts").exists(), "the ghost items are still cleaned");
        assert!(!active.join("session.lock").exists(), "the ghost items are still cleaned");
        assert!(crate::config::sessions_dir().join("archived/s-keep/team.db").exists(), "archive destroyed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn codex_profile_layers_into_config_overrides() {
        let _env = crate::env_lock();
        let root = std::env::temp_dir().join(format!("ta-codex-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("deepseek.config.toml"),
            "model = \"deepseek-flash\"\nmodel_provider = \"deepseek\"\nmodel_reasoning_effort = \"high\"\n\n[model_providers.deepseek]\nname = \"DeepSeek\"\nbase_url = \"https://api.deepseek.com/v1\"\nenv_key = \"DEEPSEEK_API_KEY\"\nwire_api = \"responses\"\n",
        )
        .unwrap();
        std::env::set_var("CODEX_HOME", &root);
        let overrides = codex_profile_overrides("deepseek").unwrap();
        let find = |key: &str| overrides.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(find("model"), Some(json!("deepseek-flash")));
        assert_eq!(find("model_provider"), Some(json!("deepseek")));
        assert_eq!(find("model_providers.deepseek.base_url"), Some(json!("https://api.deepseek.com/v1")));
        assert_eq!(
            find("model_providers.deepseek.env_key"),
            Some(json!("DEEPSEEK_API_KEY")),
            "the provider's own key env travels with the profile"
        );
        assert!(codex_profile_overrides("nope").unwrap_err().contains("cannot read"));
        std::env::remove_var("CODEX_HOME");
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
        core.call(
            "set_catalog",
            json!({"session_id": "s-factory", "catalog": {
                "models": {"m": {"provider": "openai", "protocol": "openai", "model": "test"}},
                "tools": {}, "skills_paths": [], "instruction_files": [],
            }}),
        )
        .expect("catalog");
        core.call(
            "save_spec",
            json!({"session_id": "s-factory", "spec": {
                "leader_id": "lead",
                "agents": [{"id": "lead", "name": "L", "role": "leader", "runtime_kind": "deepagents",
                            "model_profile": "m"},
                           {"id": "m", "name": "M", "role": "worker", "runtime_kind": "deepagents",
                            "model_profile": "m", "tool_bindings": ["files"]}],
                "shared_spaces": [{"id": "main", "readers": ["m"], "writers": ["m"]}],
            }}),
        )
        .expect("spec");

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
