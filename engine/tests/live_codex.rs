//! Live Codex app-server check.
//! Real `codex` CLI + real model; run explicitly:
//!   TEAMAGENTS_LIVE_CODEX=1 cargo test --offline --test live_codex -- --nocapture
//! Skips itself (no failure) when the flag is absent.

mod support;

use serde_json::{json, Value as Json};
use support::*;
use teamagents_core::models::{TurnRun, TurnStatus};
use teamagents_engine::codex::{CodexOptions, CodexRunner};
use teamagents_engine::gateway::{ApprovalGate, PermissionPolicy, ToolGateway};
use teamagents_engine::runtime::{AgentRunner, Notify};

#[test]
fn live_codex_turn_through_app_server() {
    if std::env::var("TEAMAGENTS_LIVE_CODEX").is_err() {
        eprintln!("skip: set TEAMAGENTS_LIVE_CODEX=1 to run the live codex check");
        return;
    }
    isolated_state_home("live-codex");
    let workdir = std::env::temp_dir().join("ta-live-codex");
    std::fs::create_dir_all(&workdir).unwrap();
    let core = core_with_spec(
        "cx-live",
        json!({"leader_id": "leader", "agents": [
            member("leader", "leader"),
            {"id": "cx", "name": "C", "role": "worker", "runtime_kind": "codex", "model_profile": "m"}
        ]}),
    );
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let notify = Notify::new(core.clone());
    let runner = CodexRunner::new(
        CodexOptions {
            agent_id: "cx".into(),
            session_id: "cx-live".into(),
            workdir: workdir.clone(),
            sandbox: "workspace-write".into(),
            approval_policy: "on-request".into(),
            effort: None,
            model: None,
            codex_bin: None,
            codex_home: None,
            env: vec![],
            config_overrides: vec![],
        },
        core.clone(),
        approvals,
        notify,
    );
    let run: TurnRun = serde_json::from_value(json!({
        "run_id": "run_live", "session_id": "cx-live", "task_id": null, "goal_id": null, "agent_id": "cx",
        "config_revision": 1, "topology_revision": 1, "status": "QUEUED", "input_delivery_ids": [],
        "context_ref": null, "external_turn_id": null, "cancel_requested": false, "waiting_on": [],
        "created_at": 0, "updated_at": 0,
    }))
    .unwrap();
    let view: Json = json!({
        "agent_id": "cx",
        "assignment": [{"task_id": "task_live", "description": "Run `pwd` in your workspace and report the output in one line.",
                        "acceptance": "one line report", "status": "RUNNING", "requester": "leader"}],
        "inbox_delta": [], "permitted_shared_delta": [],
        "relevant_topology": {"revision": 1, "members": ["leader", "cx"], "can_send_to": ["leader"], "can_delegate_to": [], "shared_spaces": []},
        "capabilities": [], "delivery_ids": [], "batch_no": 0,
    });
    let approvals = ApprovalGate::new(core.clone(), PermissionPolicy::default());
    let gateway = ToolGateway::new(
        core.clone(),
        "cx",
        "run_live",
        approvals,
        Some(std::sync::Arc::new(|tool: &str, _args: &Json| Err(format!("no executor for {tool}")))),
    );
    let outcome = runner.start_or_resume(&run, &view, &gateway, &json!({"reason": "new_input"}));
    eprintln!("live codex outcome: {:?} error={:?} reply={:?}", outcome.status, outcome.error, outcome.reply_text);
    assert!(
        matches!(outcome.status, TurnStatus::Completed | TurnStatus::Failed),
        "unexpected status {:?}",
        outcome.status
    );
    assert_eq!(outcome.status, TurnStatus::Completed, "live codex turn must complete");
    let thread = core.call_in_session("get_codex_thread", json!({"agent_id": "cx"})).unwrap();
    assert!(thread.get("thread_id").and_then(|v| v.as_str()).is_some());
    runner.close();
}
