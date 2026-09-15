//! PROBE (review round 3): an in-process run parked in WAITING_TASK has no live
//! thread (same as WAITING_APPROVAL, established in round 2), but
//! agent_has_live_run only exempts WaitingApproval -> a boundary patch stalls.
//! Run: cp review/tmp/probe-live-run-waiting-task.rs core/tests/ && \
//!      cd core && cargo test --offline --test probe-live-run-waiting-task -- --nocapture ; \
//!      rm core/tests/probe-live-run-waiting-task.rs
use serde_json::json;
use teamagents_core::models::*;
use teamagents_core::storage::Store;
use teamagents_core::control::Control;

fn harness() -> Control {
    let store = Store::open_memory().unwrap();
    store.create_session("s1", "/tmp", "approved_scope").unwrap();
    let spec: TeamSpec = serde_json::from_value(json!({
        "leader_id": "leader",
        "agents": [
            {"id": "leader", "name": "L", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "b", "name": "B", "role": "worker", "runtime_kind": "deepagents", "model_profile": "m"}
        ],
        "channels": [{"source": "leader", "targets": ["b"], "mode": "task"}]
    })).unwrap();
    store.save_team_spec("s1", &spec).unwrap();
    let ctl = Control::new(store, "s1");
    for a in &spec.agents { ctl.store.ensure_agent("s1", &a.id).unwrap(); }
    ctl
}

fn action(id: &str, actor: &str, kind: ActionKind, payload: serde_json::Value, run_id: Option<String>) -> teamagents_core::models::TeamAction {
    teamagents_core::models::TeamAction {
        action_id: id.into(), session_id: "s1".into(), actor_id: actor.into(),
        run_id, kind, payload,
    }
}

#[test]
fn probe_waiting_task_parked_run_blocks_boundary() {
    let mut ctl = harness();
    ctl.submit(&action("u1", "user", ActionKind::UserMessage, json!({"text": "go"}), None)).unwrap();
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    ctl.submit(&action("a1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id.clone()))).unwrap();
    // a long-running task for b to wait on
    ctl.submit(&action("a2", "leader", ActionKind::AssignTask, json!({"assignee": "leader", "description": "slow"}), Some(leader_run.run_id))).unwrap();
    let b_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().into_iter().find(|r| r.agent_id == "b").unwrap();
    let w_task = ctl.store.tasks_for_session("s1", &["PENDING"]).unwrap().into_iter().find(|t| t.assignee == "leader").unwrap();
    // park b's run in WAITING_TASK, in-process (external_turn_id is None)
    ctl.store.update_run_status_where(&b_run.run_id, TurnStatus::Queued, TurnStatus::Running).unwrap();
    ctl.store.update_run_status_where(&b_run.run_id, TurnStatus::Running, TurnStatus::WaitingTask).unwrap();
    ctl.store.deliver_wait_registration(&b_run.run_id, &[w_task.task_id.clone()]).unwrap();

    let r = ctl.submit(&action("p1", "b", ActionKind::ProposeTeamChange,
        json!({"operations": [{"op": "update_agent", "agent_id": "b", "changes": {"name": "B2"}}]}), None)).unwrap();
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();
    let r = ctl.submit(&action("p2", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None)).unwrap();
    println!("PROBE waiting_task patch status: {} (approval-parked equivalent applies immediately)", r.result["status"]);
    assert_eq!(r.result["status"], json!("APPLIED"),
        "in-process WAITING_TASK run has no live thread; patch should not wait for its boundary");
}
