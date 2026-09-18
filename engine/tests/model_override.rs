//! Feature 5 (/model): session-level model/effort overrides — set, report,
//! clear, validation, and the runner-cache drop semantics behind them.

mod support;

use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use support::*;
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{TurnRun, TurnStatus, UserConfig};
use teamagents_engine::gateway::ToolGateway;
use teamagents_engine::runtime::AgentRunner;
use teamagents_engine::scripted::Step;
use teamagents_engine::session::{open_session, OpenOptions};

// These tests mutate the process-wide XDG_STATE_HOME used by session paths.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn catalog() -> UserConfig {
    serde_json::from_value(json!({
        "models": {"m": {"provider": "openai", "protocol": "openai", "model": "gpt-default",
                         "generation_options": {"reasoning_effort": "medium"}},
                   "claude": {"provider":"anthropic", "protocol":"anthropic", "model":"claude-test",
                              "generation_options":{"output_config":{"effort":"high"}}}},
    }))
    .unwrap()
}

fn spec() -> Json {
    json!({
        "leader_id": "leader",
        "agents": [
            {"id": "leader", "name": "Leader", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "cod", "name": "Cod", "role": "dev", "runtime_kind": "codex", "model_profile": "m"},
        ],
        "shared_spaces": [{"id": "main", "readers": ["leader"], "writers": ["leader"]}],
    })
}

#[test]
fn set_model_override_applies_reports_and_clears() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = isolated_state_home("model-override");
    let cwd = home.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let scripts: HashMap<String, Vec<Step>> =
        HashMap::from([("leader".to_string(), vec![Step::End]), ("cod".to_string(), vec![Step::End])]);
    let opened = open_session(OpenOptions {
        cwd: Some(cwd),
        session_id: Some("proj_model_ov".into()),
        full_auto: false,
        initial_spec: Some(spec()),
        catalog: Some(catalog()),
        scripts: Some(scripts),
    })
    .expect("open");

    let report = opened.model_report();
    let agents = report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let leader = agents.iter().find(|a| a["agent_id"] == "leader").cloned().expect("leader");
    assert_eq!(leader["model"], json!("gpt-default"));
    assert_eq!(leader["effort"], json!("medium"));
    assert_eq!(leader["overridden"], json!(false));
    let cod = agents.iter().find(|a| a["agent_id"] == "cod").cloned().expect("cod");
    assert_eq!(cod["effort"], json!("xhigh"), "codex runner default effort");

    let set = opened.set_model_override("leader", Some("gpt-5".into()), Some("high".into())).expect("set");
    assert_eq!(set["model"], json!("gpt-5"));
    assert_eq!(set["effort"], json!("high"));
    assert_eq!(set["overridden"], json!(true));
    // the cached runner is dropped so the member's next turn picks the override up
    assert!(opened.runtime.runner("leader").is_none(), "runner cache dropped");
    let report = opened.model_report();
    let agents = report.get("agents").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let leader = agents.iter().find(|a| a["agent_id"] == "leader").cloned().expect("leader");
    assert_eq!(leader["model"], json!("gpt-5"), "{report}");
    assert_eq!(leader["overridden"], json!(true));

    // a model-only override keeps the runner default effort
    let set = opened.set_model_override("cod", Some("gpt-5-codex".into()), None).expect("set cod");
    assert_eq!(set["model"], json!("gpt-5-codex"));
    assert_eq!(set["effort"], json!("xhigh"));

    // both params empty = clear, back to the profile default
    let cleared = opened.set_model_override("leader", None, None).expect("clear");
    assert_eq!(cleared["model"], json!("gpt-default"));
    assert_eq!(cleared["effort"], json!("medium"));
    assert_eq!(cleared["overridden"], json!(false));

    // validation: unknown member, blank model, effort outside the known set
    let err = opened.set_model_override("ghost", Some("x".into()), None).unwrap_err();
    assert!(err.contains("unknown member"), "{err}");
    let err = opened.set_model_override("leader", Some("  ".into()), None).unwrap_err();
    assert!(err.contains("non-empty"), "{err}");
    let err = opened.set_model_override("leader", None, Some("insane".into())).unwrap_err();
    assert!(err.contains("effort"), "{err}");
    let selected = opened.set_model_selection("leader", Some("claude".into()), None, None).unwrap();
    assert_eq!(selected["provider"], "anthropic");
    assert_eq!(selected["model"], "claude-test");
    assert_eq!(selected["effort"], "high");
    let selected = opened.set_model_override("leader", Some("claude-other".into()), Some("LOW".into())).unwrap();
    assert_eq!(selected["provider"], "anthropic", "manual model names keep the selected provider");
    assert_eq!(selected["effort"], "low");
    assert!(opened.set_model_selection("cod", Some("claude".into()), None, None).unwrap_err().contains("Responses"));
    assert_eq!(opened.set_model_override("leader", None, None).unwrap()["model_profile"], "m");
    opened.close();
}

struct SlowRunner {
    closed: Arc<AtomicBool>,
}

impl AgentRunner for SlowRunner {
    fn start_or_resume(&self, _run: &TurnRun, _view: &Json, _gateway: &ToolGateway, _wake: &Json) -> TurnOutcome {
        std::thread::sleep(std::time::Duration::from_millis(400));
        TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None }
    }
    fn request_interrupt(&self, _run_id: &str) -> TurnStatus {
        TurnStatus::Cancelled
    }
    fn query_state(&self, _run_id: &str) -> Option<TurnStatus> {
        None
    }
    fn deliver_mid_turn(&self, _run_id: &str, _items: Vec<Json>) {}
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// drop_runner removes the cache entry so the next turn rebuilds, but a runner
/// with a turn in flight is not closed out from under it.
#[test]
fn drop_runner_keeps_an_inflight_turn_alive() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _home = isolated_state_home("drop-runner");
    let spec = json!({
        "leader_id": "leader",
        "agents": [member("leader", "leader"), member("w", "worker")],
        "channels": [task_channel("leader", &["w"]), message_channel("w", &["leader"])],
    });
    let core = core_with_spec("drop", spec);
    let leader = scripted(
        "leader",
        &json!([
            ["call", "assign_task", {"assignee": "w", "description": "slow"}],
            ["wait"],
            ["end"],
        ]),
        barriers(),
    );
    let closed = Arc::new(AtomicBool::new(false));
    let slow: Arc<dyn AgentRunner> = Arc::new(SlowRunner { closed: closed.clone() });
    let h = harness_with(core.clone(), vec![("leader", leader), ("w", slow)]);
    h.runtime.start();
    h.runtime.user_message("go", false).unwrap();

    let running = wait_for(
        || {
            h.core
                .state_brief()
                .ok()
                .map(|s| {
                    s.get("runs").and_then(|v| v.as_array()).cloned().unwrap_or_default().iter().any(|r| {
                        r.get("agent_id").and_then(|v| v.as_str()) == Some("w")
                            && r.get("status").and_then(|v| v.as_str()) == Some("RUNNING")
                    })
                })
                .unwrap_or(false)
        },
        5000,
    );
    assert!(running, "w's turn started");

    h.runtime.drop_runner("w");
    assert!(h.runtime.runner("w").is_some(), "active runner remains reachable until the next turn");
    std::thread::sleep(std::time::Duration::from_millis(800)); // outlives the 400ms turn
    assert!(!closed.load(Ordering::SeqCst), "a busy runner is not closed under the live turn");

    // an idle runner is closed immediately
    let idle_closed = Arc::new(AtomicBool::new(false));
    h.runtime.add_runner("idle", Arc::new(SlowRunner { closed: idle_closed.clone() }));
    h.runtime.drop_runner("idle");
    assert!(h.runtime.runner("idle").is_none());
    assert!(idle_closed.load(Ordering::SeqCst), "idle runner closed on drop");
    h.runtime.close();
}

/// D-29: /model overrides persist across reopening the same session; restoring
/// the default removes the persisted entry.
#[test]
fn model_overrides_survive_session_reopen() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = isolated_state_home("model-override-reopen");
    let cwd = home.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let open = || {
        let scripts: HashMap<String, Vec<Step>> =
            HashMap::from([("leader".to_string(), vec![Step::End]), ("cod".to_string(), vec![Step::End])]);
        open_session(OpenOptions {
            cwd: Some(cwd.clone()),
            session_id: Some("proj_model_reopen".into()),
            full_auto: false,
            initial_spec: Some(spec()),
            catalog: Some(catalog()),
            scripts: Some(scripts),
        })
        .expect("open")
    };
    let effective = |opened: &teamagents_engine::session::OpenedSession, id: &str| {
        let report = opened.model_report();
        report["agents"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|a| a["agent_id"] == id)
            .unwrap_or_else(|| panic!("agent {id} in {report}"))
    };

    let first = open();
    first.set_model_selection("leader", Some("claude".into()), None, Some("low".into())).expect("set");
    first.close();

    let second = open();
    let leader = effective(&second, "leader");
    assert_eq!(leader["model_profile"], json!("claude"), "{leader}");
    assert_eq!(leader["model"], json!("claude-test"));
    assert_eq!(leader["effort"], json!("low"));
    assert_eq!(leader["overridden"], json!(true));
    assert_eq!(effective(&second, "cod")["overridden"], json!(false), "unset member stays default");

    second.set_model_override("leader", None, None).expect("restore default");
    second.close();
    let third = open();
    let leader = effective(&third, "leader");
    assert_eq!(leader["model_profile"], json!("m"), "{leader}");
    assert_eq!(leader["overridden"], json!(false));
    third.close();
}

/// Overrides referencing members/profiles that no longer exist (or that violate
/// the codex/effort rules) are dropped at load instead of failing the open.
#[test]
fn stale_model_overrides_are_dropped_on_open() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = isolated_state_home("model-override-stale");
    let cwd = home.join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let dir = home.join("teamagents/sessions/proj_model_stale");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("model_overrides.json"),
        serde_json::to_string(&json!({
            "ghost": {"profile": null, "model": "x", "effort": null},
            "cod": {"profile": "claude", "model": null, "effort": null},
            "leader": {"profile": "gone", "model": null, "effort": null},
        }))
        .unwrap(),
    )
    .unwrap();
    let scripts: HashMap<String, Vec<Step>> =
        HashMap::from([("leader".to_string(), vec![Step::End]), ("cod".to_string(), vec![Step::End])]);
    let opened = open_session(OpenOptions {
        cwd: Some(cwd),
        session_id: Some("proj_model_stale".into()),
        full_auto: false,
        initial_spec: Some(spec()),
        catalog: Some(catalog()),
        scripts: Some(scripts),
    })
    .expect("open despite stale overrides");
    let report = opened.model_report();
    let agents = report["agents"].as_array().cloned().unwrap_or_default();
    assert!(agents.iter().all(|a| a["overridden"] == json!(false)), "{report}");
    opened.close();
}
