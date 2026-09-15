//! PROBE (review round 2): cancel a task whose run is parked in WAITING_TASK.
//! Run: cp review/tmp/probe-cancel-waiting-task.rs core/tests/ && \
//!      cd core && cargo test --offline --test probe-cancel-waiting-task -- --nocapture ; \
//!      rm core/tests/probe-cancel-waiting-task.rs
//! Observed output:
//!   PROBE after cancel: a-task=Cancelled run-b=WaitingTask cancel_requested=false
//!   PROBE after w-task finalize+schedule: run-b=Running
//!   PROBE final: a-task=Succeeded          <-- cancelled task resurrected
use serde_json::json;
use teamagents_core::control::{Control, TurnOutcome};
use teamagents_core::models::*;
use teamagents_core::storage::Store;

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

fn task(id: &str, assignee: &str, status: TaskStatus) -> Task {
    let mut t: Task = serde_json::from_value(json!({
        "task_id": id, "requester": "leader", "assignee": assignee, "description": "d"
    })).unwrap();
    t.status = status;
    t
}

fn run(id: &str, agent: &str, task_id: Option<&str>, status: TurnStatus, waiting_on: Vec<String>) -> TurnRun {
    TurnRun {
        run_id: id.into(), session_id: "s1".into(),
        task_id: task_id.map(str::to_string), goal_id: None, agent_id: agent.into(),
        config_revision: 0, topology_revision: 0, status,
        input_delivery_ids: vec![], context_ref: None, external_turn_id: None,
        cancel_requested: false, waiting_on, created_at: 1000.0, updated_at: 1000.0,
    }
}

#[test]
fn probe_cancel_task_with_waiting_run() {
    let mut ctl = harness();
    ctl.store.insert_task("s1", &task("w-task", "leader", TaskStatus::Running)).unwrap();
    ctl.store.insert_task("s1", &task("a-task", "b", TaskStatus::Running)).unwrap();
    ctl.store.insert_run(&run("run-b", "b", Some("a-task"), TurnStatus::WaitingTask, vec!["w-task".into()])).unwrap();
    ctl.store.insert_run(&run("run-l", "leader", Some("w-task"), TurnStatus::Running, vec![])).unwrap();

    // user cancels a-task while its run is parked in WAITING_TASK
    let cancel = TeamAction {
        action_id: "c1".into(), session_id: "s1".into(), actor_id: "user".into(),
        run_id: None, kind: ActionKind::CancelTask, payload: json!({"task_id": "a-task"}),
    };
    let r = ctl.submit(&cancel).unwrap();
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let rb = ctl.store.get_run("run-b").unwrap().unwrap();
    println!("PROBE after cancel: a-task={:?} run-b={:?} cancel_requested={}",
        ctl.store.get_task("a-task").unwrap().unwrap().status, rb.status, rb.cancel_requested);

    // leader finishes w-task through the real finalize path (pushes wake the waiter)
    ctl.store.record_completion_request("run-l", "w-task", &[], "w done").unwrap();
    ctl.finalize_run("run-l", &TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None }, &[]).unwrap();
    ctl.schedule().unwrap();
    let rb = ctl.store.get_run("run-b").unwrap().unwrap();
    println!("PROBE after w-task finalize+schedule: run-b={:?}", rb.status);

    // the member's resumed turn then completes the cancelled task
    if rb.status == TurnStatus::Running {
        ctl.store.record_completion_request("run-b", "a-task", &[], "done").unwrap();
        ctl.finalize_run("run-b", &TurnOutcome { status: TurnStatus::Completed, error: None, note: None, reply_text: None }, &[]).unwrap();
        println!("PROBE final: a-task={:?}", ctl.store.get_task("a-task").unwrap().unwrap().status);
    }
}
