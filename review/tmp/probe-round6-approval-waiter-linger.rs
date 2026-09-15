//! Round-6 review probe (temporary, deleted after the run): does
//! server_exited release approval waiters parked in on_request?
mod support;

use serde_json::{json, Value as Json};
use std::sync::Arc;
use support::*;
use teamagents_core::models::{TurnRun, TurnStatus};
use teamagents_engine::codex::{CodexOptions, CodexRunner};
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::runtime::{AgentRunner, Notify};

#[test]
fn probe_server_death_leaves_approval_waiter_parked_until_timeout() {
    std::env::set_var("TEAMAGENTS_CODEX_APPROVAL_WAIT_S", "2");
    isolated_state_home("codex");
    let core = core_with_spec(
        "probe-linger",
        json!({"leader_id": "leader", "agents": [member("leader", "leader"),
            {"id": "cx", "name": "C", "role": "worker", "runtime_kind": "codex", "model_profile": "m"}]}),
    );
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let notify = Notify::new(core.clone());
    let runner = Arc::new(CodexRunner::new(
        CodexOptions {
            agent_id: "cx".into(),
            session_id: "probe-linger".into(),
            workdir: "/tmp".into(),
            sandbox: "danger-full-access".into(),
            approval_policy: "never".into(),
            model: None,
            codex_bin: Some(env!("CARGO_BIN_EXE_fake-codex").to_string()),
            codex_home: None,
            env: vec![("FAKE_CODEX_MODE".into(), "approval".into())],
            config_overrides: vec![],
            effort: None,
        },
        core.clone(),
        approvals,
        notify,
    ));
    let run: TurnRun = serde_json::from_value(json!({
        "run_id": "run_probe", "session_id": "probe-linger", "task_id": null, "goal_id": null,
        "agent_id": "cx", "config_revision": 1, "topology_revision": 1, "status": "QUEUED",
        "input_delivery_ids": [], "context_ref": null, "external_turn_id": null,
        "cancel_requested": false, "waiting_on": [], "created_at": 0, "updated_at": 0,
    })).unwrap();
    let view: Json = json!({"agent_id": "cx", "assignment": [], "inbox_delta": [],
        "permitted_shared_delta": [], "relevant_topology": {"revision": 1}, "capabilities": [],
        "delivery_ids": [], "batch_no": 0});
    let gw = Arc::new(ToolGateway::new(core.clone(), "cx", "run_probe",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()), None));
    let started = {
        let runner = runner.clone();
        std::thread::spawn(move || runner.start_or_resume(&run, &view, &gw, &json!({"reason": "new_input"})))
    };
    let mut approval_id = String::new();
    for _ in 0..100 {
        let state = core.call_in_session("state", json!({})).unwrap();
        let pending = state.get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if !pending.is_empty() {
            approval_id = pending[0].get("approval_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!approval_id.is_empty(), "approval parked");
    runner.close();
    let outcome = started.join().expect("driver joined");
    assert_eq!(outcome.status, TurnStatus::Failed, "driver released with Failed");
    assert_eq!(runner.query_state("run_probe"), Some(TurnStatus::Failed));
    // the waiter thread parked in on_request survives the server's death:
    // resolve_approval still finds it (it only goes away at approval_wait_timeout)
    assert!(
        runner.resolve_approval(&approval_id, "once"),
        "linger: waiter still parked after server death"
    );
}

#[test]
fn probe_approval_timeout_overwrites_terminal_state_in_memory() {
    std::env::set_var("TEAMAGENTS_CODEX_APPROVAL_WAIT_S", "1");
    isolated_state_home("codex");
    let core = core_with_spec(
        "probe-stale",
        json!({"leader_id": "leader", "agents": [member("leader", "leader"),
            {"id": "cx", "name": "C", "role": "worker", "runtime_kind": "codex", "model_profile": "m"}]}),
    );
    let runner = Arc::new(CodexRunner::new(
        CodexOptions {
            agent_id: "cx".into(), session_id: "probe-stale".into(),
            workdir: "/tmp".into(), sandbox: "danger-full-access".into(),
            approval_policy: "never".into(), model: None,
            codex_bin: Some(env!("CARGO_BIN_EXE_fake-codex").to_string()),
            codex_home: None,
            env: vec![("FAKE_CODEX_MODE".into(), "approval".into())],
            config_overrides: vec![], effort: None,
        },
        core.clone(),
        ApprovalGate::new(core.clone(), PermissionPolicy::default()),
        Notify::new(core.clone()),
    ));
    let run: TurnRun = serde_json::from_value(json!({
        "run_id": "run_stale", "session_id": "probe-stale", "task_id": null, "goal_id": null,
        "agent_id": "cx", "config_revision": 1, "topology_revision": 1, "status": "QUEUED",
        "input_delivery_ids": [], "context_ref": null, "external_turn_id": null,
        "cancel_requested": false, "waiting_on": [], "created_at": 0, "updated_at": 0,
    })).unwrap();
    let view: Json = json!({"agent_id": "cx", "assignment": [], "inbox_delta": [],
        "permitted_shared_delta": [], "relevant_topology": {"revision": 1}, "capabilities": [],
        "delivery_ids": [], "batch_no": 0});
    let gw = Arc::new(ToolGateway::new(core.clone(), "cx", "run_stale",
        ApprovalGate::new(core.clone(), PermissionPolicy::default()), None));
    let started = {
        let runner = runner.clone();
        std::thread::spawn(move || runner.start_or_resume(&run, &view, &gw, &json!({"reason": "new_input"})))
    };
    // wait until the approval handler thread is parked on rx
    for _ in 0..100 {
        let state = core.call_in_session("state", json!({})).unwrap();
        let pending = state.get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if !pending.is_empty() { break; }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    runner.close();
    let outcome = started.join().expect("driver joined");
    assert_eq!(outcome.status, TurnStatus::Failed);
    assert_eq!(runner.query_state("run_stale"), Some(TurnStatus::Failed), "terminal after death");
    // the parked handler outlives the server; at its 1s wait timeout it
    // expires the (already dead) approval and blindly sets Running again
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert_eq!(
        runner.query_state("run_stale"),
        Some(TurnStatus::Running),
        "stale: timed-out approval handler overwrote the terminal in-memory status"
    );
}
