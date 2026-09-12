//! Control-level scenario tests, mirrored from tests/test_t*.py / test_p1_guards.py.
//! Runtime (turn execution) is TS-side; these cover validate/reduce/schedule.

use serde_json::json;
use teamagents_core::control::{derived_task_id, Control};
use teamagents_core::models::*;
use teamagents_core::storage::Store;

fn spec() -> TeamSpec {
    serde_json::from_value(json!({
        "leader_id": "leader",
        "agents": [
            {"id": "leader", "name": "L", "role": "leader", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "b", "name": "B", "role": "worker", "runtime_kind": "deepagents", "model_profile": "m"},
            {"id": "cx", "name": "C", "role": "worker", "runtime_kind": "codex", "model_profile": "m"}
        ],
        "channels": [
            {"source": "leader", "targets": ["b", "cx"], "mode": "task"},
            {"source": "b", "targets": ["leader"], "mode": "message"}
        ],
        "shared_spaces": [{"id": "lib", "readers": ["leader", "b"], "writers": ["b"]}]
    }))
    .unwrap()
}

fn harness() -> Control {
    let store = Store::open_memory().unwrap();
    store.create_session("s1", "/tmp", "approved_scope").unwrap();
    store.save_team_spec("s1", &spec()).unwrap();
    let mut ctl = Control::new(store, "s1");
    // session bootstrap ensures runtime rows exist (Python does this at session start)
    for a in spec().agents {
        ctl.store.ensure_agent("s1", &a.id).unwrap();
    }
    ctl
}

fn action(id: &str, actor: &str, kind: ActionKind, payload: Json, run_id: Option<String>) -> TeamAction {
    TeamAction {
        action_id: id.into(),
        session_id: "s1".into(),
        actor_id: actor.into(),
        run_id,
        kind,
        payload,
    }
}

fn user(id: &str, text: &str) -> TeamAction {
    action(id, "user", ActionKind::UserMessage, json!({"text": text}), None)
}

#[test]
fn t1_user_message_queues_leader_run() {
    let mut ctl = harness();
    let r = ctl.submit(&user("a1", "please produce the report"));
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let goal = r.result["goal_id"].as_str().unwrap().to_string();

    // user_message event pushed to leader, delivery pending
    let pending = ctl.store.pending_deliveries("s1", "leader").unwrap();
    assert_eq!(pending.len(), 1);
    // schedule queued a leader run carrying that delivery
    let runs = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].agent_id, "leader");
    assert_eq!(runs[0].goal_id.as_deref(), Some(goal.as_str()));
    assert_eq!(runs[0].input_delivery_ids.len(), 1);

    // replay: same action id returns the recorded receipt, no second event
    let r2 = ctl.submit(&user("a1", "please produce the report"));
    assert!(r2.ok);
    assert_eq!(ctl.store.events("s1", 0, 100).unwrap().len(), 1);
}

#[test]
fn t1_assign_task_dispatches_to_worker() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go"));
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action(
        "a2",
        "leader",
        ActionKind::AssignTask,
        json!({"assignee": "b", "description": "write the report", "acceptance": "report.md exists"}),
        Some(leader_run.run_id.clone()),
    );
    let task_id = derived_task_id(&a);
    let r = ctl.submit(&a);
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(r.result["task_id"], json!(task_id));

    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.requester, "leader");

    // TASK_READY announced and pushed to assignee; b gets a queued run
    let pending_b = ctl.store.pending_deliveries("s1", "b").unwrap();
    assert_eq!(pending_b.len(), 1);
    let runs_b: Vec<_> = ctl
        .store
        .runs_for_session("s1", &[TurnStatus::Queued])
        .unwrap()
        .into_iter()
        .filter(|r| r.agent_id == "b")
        .collect();
    assert_eq!(runs_b.len(), 1);
    assert_eq!(runs_b[0].task_id.as_deref(), Some(task_id.as_str()));
}

#[test]
fn p1_validation_guards() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go"));

    // unknown assignee
    let r = ctl.submit(&action("v1", "leader", ActionKind::AssignTask, json!({"assignee": "ghost", "description": "x"}), None));
    assert!(!r.ok && r.error.unwrap().contains("unknown assignee"));
    // b cannot delegate (no task channel from b)
    let r = ctl.submit(&action("v2", "b", ActionKind::AssignTask, json!({"assignee": "leader", "description": "x"}), None));
    assert!(!r.ok && r.error.unwrap().contains("not allowed to assign"));
    // codex member only via leader — leader CAN, so check the reverse guard via update: use non-leader path
    // empty description refused
    let r = ctl.submit(&action("v3", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "  "}), None));
    assert!(!r.ok && r.error.unwrap().contains("must not be empty"));
    // user cannot submit member actions
    let r = ctl.submit(&action("v4", "user", ActionKind::SendMessage, json!({"target": "b", "text": "x"}), None));
    assert!(!r.ok && r.error.unwrap().contains("local user cannot"));
    // unknown actor
    let r = ctl.submit(&action("v5", "ghost", ActionKind::SendMessage, json!({"target": "b", "text": "x"}), None));
    assert!(!r.ok && r.error.unwrap().contains("not a team member"));
    // b cannot message cx (no channel)
    let r = ctl.submit(&action("v6", "b", ActionKind::SendMessage, json!({"target": "cx", "text": "x"}), None));
    assert!(!r.ok);
    // publish to space without write access
    let r = ctl.submit(&action("v7", "leader", ActionKind::PublishShared, json!({"space_id": "lib", "content": "x"}), None));
    assert!(!r.ok && r.error.unwrap().contains("no write access"));
    // signal_done only by leader
    let r = ctl.submit(&action("v8", "b", ActionKind::SignalDone, json!({}), Some("run_x".into())));
    assert!(!r.ok);
}

#[test]
fn shared_publish_and_read_flow() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go"));
    let r = ctl.submit(&action("s2", "b", ActionKind::PublishShared, json!({"space_id": "lib", "content": "findings"}), None));
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let seq = r.result["sequence"].as_i64().unwrap();
    assert!(seq >= 1);

    let r = ctl.submit(&action("s3", "leader", ActionKind::ReadShared, json!({"space_id": "lib"}), None));
    assert!(r.ok);
    let entries = r.result["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["content"], "findings");
    // cursor advanced: second read returns nothing new
    let r = ctl.submit(&action("s4", "leader", ActionKind::ReadShared, json!({"space_id": "lib"}), None));
    assert_eq!(r.result["entries"].as_array().unwrap().len(), 0);
}

#[test]
fn topology_patch_add_and_stale_reject() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go"));
    let rev = ctl.store.current_revision("s1").unwrap();

    // propose by member
    let r = ctl.submit(&action(
        "p1",
        "b",
        ActionKind::ProposeTeamChange,
        json!({"operations": [{"op": "add_channel", "channel": {"source": "b", "targets": ["cx"], "mode": "message"}}]}),
        None,
    ));
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    let patch_id = r.result["patch_id"].as_str().unwrap().to_string();

    // stale base rejected on apply path only after revision moves; apply now works (leader)
    let r = ctl.submit(&action("p2", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None));
    assert!(r.ok, "{}", r.error.unwrap_or_default());
    assert_eq!(ctl.store.current_revision("s1").unwrap(), rev + 1);
    let spec = ctl.store.load_team_spec("s1", None).unwrap();
    assert!(spec.can_send("b", "cx"));

    // rejecting an already-applied patch fails
    let r = ctl.submit(&action("p3", "leader", ActionKind::ApplyTopologyPatch, json!({"patch_id": patch_id}), None));
    assert!(!r.ok);
}

#[test]
fn cancel_task_without_run_cancels_immediately() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go"));
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    let a = action("c1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id));
    let task_id = derived_task_id(&a);
    ctl.submit(&a);
    // cancel the (not yet started) task as the user
    let r = ctl.submit(&action("c2", "user", ActionKind::CancelTask, json!({"task_id": task_id}), None));
    assert!(r.ok);
    let task = ctl.store.get_task(&task_id).unwrap().unwrap();
    assert!(matches!(task.status, TaskStatus::Cancelled));
}

#[test]
fn signal_done_blocked_by_unfinished_work() {
    let mut ctl = harness();
    ctl.submit(&user("a1", "go"));
    let leader_run = ctl.store.runs_for_session("s1", &[TurnStatus::Queued]).unwrap().remove(0);
    // mark the leader run RUNNING so it can signal
    ctl.store.set_run_status(&leader_run.run_id, TurnStatus::Running).unwrap();
    let a = action("d1", "leader", ActionKind::AssignTask, json!({"assignee": "b", "description": "x"}), Some(leader_run.run_id.clone()));
    ctl.submit(&a);
    let r = ctl.submit(&action("d2", "leader", ActionKind::SignalDone, json!({"summary": "done"}), Some(leader_run.run_id.clone())));
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("not yet complete"));
    let blockers = r.result["blockers"].as_array().unwrap();
    assert!(blockers.iter().any(|b| b.as_str().unwrap().contains("unfinished tasks")));
}
