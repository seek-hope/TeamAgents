#![allow(dead_code)] // each integration test binary compiles this module and uses a subset
//! Shared harness for the ported scenario tests (tests/conftest.py parity).

use serde_json::{json, Value as Json};
use std::sync::Arc;
use teamagents_engine::core_client::CoreClient;
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy};
use teamagents_engine::runtime::{AgentRunner, Notify, Runtime, RuntimeLimits, ToolExecutor};
use teamagents_engine::scripted::{BarrierRegistry, ScriptedMember};

pub struct Harness {
    pub core: Arc<CoreClient>,
    pub runtime: Arc<Runtime>,
}

pub fn isolated_state_home(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ta-engine-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("XDG_STATE_HOME", &dir);
    dir
}

pub fn member(id: &str, role: &str) -> Json {
    json!({"id": id, "name": id, "role": role, "runtime_kind": "deepagents", "model_profile": "m"})
}

pub fn message_channel(source: &str, targets: &[&str]) -> Json {
    json!({"source": source, "targets": targets, "mode": "message"})
}

pub fn task_channel(source: &str, targets: &[&str]) -> Json {
    json!({"source": source, "targets": targets, "mode": "task"})
}

pub fn core_with_spec(session: &str, spec: Json) -> Arc<CoreClient> {
    let core = CoreClient::open(":memory:", session).expect("core");
    core.call("create_session", json!({"session_id": session, "cwd": "/tmp"})).expect("create");
    // the test catalog mirrors the Python harness (profile "test"/"m")
    core.call("set_catalog", json!({"session_id": session, "catalog": test_catalog()}))
        .expect("catalog");
    core.call("save_spec", json!({"session_id": session, "spec": spec})).expect("spec");
    core
}

/// Minimal user config for tests: one model profile per name used by specs.
pub fn test_catalog() -> Json {
    json!({
        "models": {
            "m": {"provider": "openai", "protocol": "openai", "model": "test"},
            "test": {"provider": "openai", "protocol": "openai", "model": "test"},
            "other": {"provider": "openai", "protocol": "openai", "model": "other"},
            "leader_main": {"provider": "openai", "protocol": "openai", "model": "test"},
            "coding": {"provider": "openai", "protocol": "openai", "model": "test"},
        },
        "tools": {}, "skills_paths": [], "instruction_files": [],
    })
}

pub fn barriers() -> BarrierRegistry {
    Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub fn scripted(id: &str, steps: &Json, barriers: BarrierRegistry) -> Arc<ScriptedMember> {
    let steps = steps
        .as_array()
        .expect("steps array")
        .iter()
        .map(teamagents_engine::scripted::Step::from_json)
        .collect::<Result<Vec<_>, _>>()
        .expect("steps");
    ScriptedMember::new(id, steps, barriers)
}

pub fn harness_with(core: Arc<CoreClient>, members: Vec<(&str, Arc<dyn AgentRunner>)>) -> Harness {
    let notify = Notify::new(core.clone());
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let executor: ToolExecutor = Arc::new(|_agent: &str, tool: &str, _args: &Json| {
        Err(format!("no tool executor configured for {tool}"))
    });
    let runtime = Runtime::new(core.clone(), notify, approvals, executor, None, RuntimeLimits::default());
    for (id, member) in members {
        runtime.add_runner(id, member);
    }
    Harness { core, runtime }
}

pub fn wait_for<F: Fn() -> bool>(probe: F, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if probe() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    probe()
}

pub fn submit(core: &Arc<CoreClient>, action_id: &str, actor: &str, kind: &str, payload: Json) -> teamagents_core::models::Receipt {
    let action: teamagents_core::models::TeamAction = serde_json::from_value(json!({
        "action_id": action_id,
        "session_id": core.session_id,
        "actor_id": actor,
        "kind": kind,
        "payload": payload,
    }))
    .expect("action");
    core.submit(&action).expect("submit")
}
