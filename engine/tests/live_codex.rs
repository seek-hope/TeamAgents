//! Opt-in real Codex/model check with a real task and cold recovery.
//! TEAMAGENTS_LIVE_CODEX=1 enables it; the context window must be explicitly
//! supplied or present in the selected native Codex config. No credentials
//! or model responses are saved to the repository.

mod support;

use serde_json::{json, Value as Json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;
use teamagents_core::models::{TurnRun, TurnStatus};
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::session::{open_session, OpenOptions};

#[test]
fn live_codex_turn_and_cold_recovery_through_app_server() {
    if std::env::var("TEAMAGENTS_LIVE_CODEX").as_deref() != Ok("1") {
        eprintln!("skip: set TEAMAGENTS_LIVE_CODEX=1 to run the real codex/model recovery check");
        return;
    }
    let source_home = std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").expect("HOME for native Codex configuration")).join(".codex")
    });
    let source_config = std::env::var_os("TEAMAGENTS_LIVE_CODEX_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| source_home.join("config.toml"));
    let source: toml::Value = std::fs::read_to_string(&source_config).unwrap().parse().unwrap();
    let window = std::env::var("TEAMAGENTS_LIVE_CODEX_CONTEXT_WINDOW")
        .ok()
        .map(|v| v.parse::<u64>().expect("native context window must be an integer"))
        .or_else(|| source.get("model_context_window").and_then(toml::Value::as_integer).map(|v| v as u64))
        .expect("supply the model's native TEAMAGENTS_LIVE_CODEX_CONTEXT_WINDOW");
    assert!(window > 0 && window <= i64::MAX as u64);
    let model = source["model"].as_str().expect("selected native config must name its model").to_string();
    let provider = source.get("model_provider").and_then(toml::Value::as_str).unwrap_or("openai").to_string();
    let mut env = isolated_state_home("live-codex-recovery");
    let workdir = env.join("work");
    let codex_home = env.join("codex");
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::create_dir_all(&codex_home).unwrap();
    // Isolate external history/configuration and select only execution/model
    // settings. Do not import unrelated MCP servers, hooks or host projects.
    let mut config = toml::map::Map::new();
    for key in ["model", "model_provider", "model_providers", "model_reasoning_effort"] {
        if let Some(value) = source.get(key) {
            config.insert(key.into(), value.clone());
        }
    }
    config.insert("model_context_window".into(), toml::Value::Integer(window as i64));
    let config = toml::to_string(&config).unwrap();
    std::fs::write(codex_home.join("config.toml"), &config).unwrap();
    std::fs::write(codex_home.join("live.config.toml"), &config).unwrap();
    if source_home.join("auth.json").is_file() {
        std::fs::copy(source_home.join("auth.json"), codex_home.join("auth.json")).unwrap();
    }
    env.set("CODEX_HOME", &codex_home);
    let catalog = json!({"models":{"m":{"provider":provider, "protocol":"responses",
        "model":model, "context_window":window, "codex_profile":"live"}}});
    let open = || {
        open_session(OpenOptions {
            cwd: Some(workdir.clone()),
            session_id: Some("codex-live-recovery".into()),
            catalog: Some(serde_json::from_value(catalog.clone()).unwrap()),
            initial_spec: Some(json!({"leader_id":"leader", "agents":[member("leader","leader"),
                {"id":"cx","name":"Codex","role":"worker","runtime_kind":"codex","model_profile":"m"}],
                "channels":[task_channel("leader",&["cx"])]})),
            ..Default::default()
        })
        .unwrap()
    };
    let session = open();
    let assigned = submit(
        &session.core,
        "live-task",
        "leader",
        "assign_task",
        json!({
            "assignee":"cx",
            "description":"Append exactly one line `recovered-once` to proof.txt in your workspace using your native execution tool. Read the file to verify its complete content is exactly that one line. Report LIVE_RECOVERY_OK. Do not repeat the append after it has succeeded.",
            "acceptance":"proof.txt contains exactly recovered-once followed by a newline; final output includes LIVE_RECOVERY_OK"
        }),
    );
    assert!(assigned.ok, "{assigned:?}");
    let state = session.core.state_brief().unwrap();
    let queued = state["runs"].as_array().unwrap().iter().find(|r| r["agent_id"] == "cx").unwrap();
    let run: TurnRun = serde_json::from_value(
        session.core.call_in_session("begin_run", json!({"run_id":queued["run_id"]})).unwrap()["run"].clone(),
    )
    .unwrap();
    let view = session.core.call_in_session("agent_view", json!({"agent_id":"cx"})).unwrap();
    let runner = session.runtime.runner("cx").unwrap();
    let gateway = ToolGateway::new(
        session.core.clone(),
        "cx",
        &run.run_id,
        ApprovalGate::new(session.core.clone(), PermissionPolicy::default()),
        None,
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    let handle = {
        let (runner, run) = (Arc::clone(&runner), run.clone());
        std::thread::spawn(move || {
            let _ = tx.send(runner.start_or_resume(&run, &view, &gateway, &Json::Null));
        })
    };
    let result = rx.recv_timeout(Duration::from_secs(240));
    // Leave the core run unfinalized: the external model completed, but the
    // application has not yet archived its result. A new session must recover.
    session.close();
    handle.join().unwrap();
    let outcome = result.expect("real Codex turn must finish within 240s");
    assert_eq!(outcome.status, TurnStatus::Completed, "{outcome:?}");
    assert!(outcome.reply_text.as_deref().is_some_and(|s| s.contains("LIVE_RECOVERY_OK")), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(workdir.join("proof.txt")).unwrap(), "recovered-once\n");
    let thread = session.core.call_in_session("get_codex_thread", json!({"agent_id":"cx"})).unwrap();
    drop(runner);
    drop(session);
    let restored = open();
    restored.runtime.reconcile();
    let state = restored.core.state().unwrap();
    let same_thread = restored.core.call_in_session("get_codex_thread", json!({"agent_id":"cx"})).unwrap();
    restored.close();
    assert_eq!(thread, same_thread);
    let recovered = state["runs"].as_array().unwrap().iter().find(|r| r["run_id"] == run.run_id).unwrap();
    assert_eq!(recovered["status"], "COMPLETED", "{state}");
    assert_eq!(state["tasks"][0]["status"], "SUCCEEDED", "{state}");
    assert!(state["events"].as_array().unwrap().iter().any(|e| e["kind"] == "task_completed"
        && e["payload"]["summary"].as_str().is_some_and(|s| s.contains("LIVE_RECOVERY_OK"))));
    assert_eq!(std::fs::read_to_string(workdir.join("proof.txt")).unwrap(), "recovered-once\n");
    let evidence = json!({"model":model, "provider":provider, "context_window":window,
        "duration_ms":started.elapsed().as_millis(), "run_status":recovered["status"],
        "task_status":state["tasks"][0]["status"], "same_thread":true, "proof_lines":1,
        "external_turn_id":recovered["external_turn_id"], "thread_id":thread["thread_id"]});
    eprintln!("live codex recovery evidence: {evidence}");
    if let Some(path) = std::env::var_os("TEAMAGENTS_LIVE_CODEX_EVIDENCE") {
        std::fs::write(path, serde_json::to_string_pretty(&evidence).unwrap()).unwrap();
    }
}
