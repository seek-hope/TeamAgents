//! CodexRunner against a fake app-server (tests/test_p5_codex_adapter.py port).

mod support;

use serde_json::{json, Value as Json};
use std::sync::Arc;
use support::*;
use teamagents_core::control::TurnOutcome;
use teamagents_core::models::{TurnRun, TurnStatus};
use teamagents_engine::codex::{CodexOptions, CodexRunner};
use teamagents_engine::core_client::CoreClient;
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::runtime::{AgentRunner, Notify};

fn codex_agent() -> Json {
    json!({"id": "cx", "name": "C", "role": "worker", "runtime_kind": "codex", "model_profile": "m"})
}

fn setup(session: &str, mode: &str) -> (Arc<CoreClient>, Arc<CodexRunner>) {
    isolated_state_home("codex");
    let core = core_with_spec(
        session,
        json!({"leader_id": "leader", "agents": [member("leader", "leader"), codex_agent()]}),
    );
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let notify = Notify::new(core.clone());
    let runner = CodexRunner::new(
        CodexOptions {
            agent_id: "cx".into(),
            session_id: session.into(),
            workdir: std::env::temp_dir(),
            sandbox: "workspace-write".into(),
            approval_policy: "on-request".into(),
            effort: None,
            model: None,
            codex_bin: Some(env!("CARGO_BIN_EXE_fake-codex").to_string()),
            codex_home: None,
            env: vec![("FAKE_CODEX_MODE".into(), mode.into())],
            config_overrides: vec![],
        },
        core.clone(),
        approvals,
        notify,
    );
    (core, runner)
}

fn turn_run(session: &str, run_id: &str) -> TurnRun {
    serde_json::from_value(json!({
        "run_id": run_id, "session_id": session, "task_id": null, "goal_id": null, "agent_id": "cx",
        "config_revision": 1, "topology_revision": 1, "status": "QUEUED", "input_delivery_ids": [],
        "context_ref": null, "external_turn_id": null, "cancel_requested": false, "waiting_on": [],
        "created_at": 0, "updated_at": 0,
    }))
    .unwrap()
}

fn view() -> Json {
    json!({"agent_id": "cx", "assignment": [], "inbox_delta": [], "permitted_shared_delta": [],
           "relevant_topology": {"revision": 1}, "capabilities": [], "delivery_ids": [], "batch_no": 0})
}

fn gateway(core: &Arc<CoreClient>, run_id: &str) -> Arc<ToolGateway> {
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    ToolGateway::new(
        core.clone(),
        "cx",
        run_id,
        approvals,
        Some(Arc::new(|tool: &str, _args: &Json| Err(format!("no executor for {tool}")))),
    )
}

#[test]
fn codex_simple_turn_completes_and_persists_the_thread() {
    let (core, runner) = setup("cx1", "simple");
    let run = turn_run("cx1", "run_cx1");
    let outcome: TurnOutcome =
        runner.start_or_resume(&run, &view(), &gateway(&core, "run_cx1"), &json!({"reason": "new_input"}));
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert!(outcome.reply_text.unwrap_or_default().contains("fake work done"));
    let thread = core.call_in_session("get_codex_thread", json!({"agent_id": "cx"})).unwrap();
    assert!(thread.get("thread_id").and_then(|v| v.as_str()).is_some(), "thread id persisted before the turn");
    assert_eq!(runner.query_state("run_cx1"), Some(TurnStatus::Completed));
    runner.close();
}

#[test]
fn codex_approval_flow_parks_decides_and_resumes() {
    let (core, runner) = setup("cx2", "approval");
    let run = turn_run("cx2", "run_cx2");
    let gw = gateway(&core, "run_cx2");
    let started = {
        let runner = runner.clone();
        let view = view();
        let wake = json!({"reason": "new_input"});
        std::thread::spawn(move || runner.start_or_resume(&run, &view, &gw, &wake))
    };
    // wait for the approval request to land in the core
    let approval = wait_for(
        || {
            core.call_in_session("state", json!({}))
                .ok()
                .and_then(|state| state.get("pending_approvals").and_then(|v| v.as_array()).cloned())
                .map(|pending| !pending.is_empty())
                .unwrap_or(false)
        },
        5000,
    );
    assert!(approval, "approval request recorded");
    let state = core.call_in_session("state", json!({})).unwrap();
    let pending = state.get("pending_approvals").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let approval_id = pending[0].get("approval_id").and_then(|v| v.as_str()).unwrap().to_string();
    assert!(runner.resolve_approval(&approval_id, "once"), "user approves once");
    let outcome = started.join().unwrap();
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert!(
        outcome.reply_text.unwrap_or_default().contains("approval=accept"),
        "the decision reached the app-server"
    );
    runner.close();
}

#[test]
fn codex_interrupt_stops_a_slow_turn_with_confirmed_status() {
    let (core, runner) = setup("cx3", "slow");
    let run = turn_run("cx3", "run_cx3");
    let gw = gateway(&core, "run_cx3");
    let started = {
        let runner = runner.clone();
        let view = view();
        let wake = json!({"reason": "new_input"});
        std::thread::spawn(move || runner.start_or_resume(&run, &view, &gw, &wake))
    };
    std::thread::sleep(std::time::Duration::from_millis(300));
    let status = runner.request_interrupt("run_cx3");
    assert_eq!(status, TurnStatus::Cancelled);
    assert_eq!(started.join().unwrap().status, TurnStatus::Cancelled);
    runner.close();
}
